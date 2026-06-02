use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::store::StoreBackend;
use crate::store::leases::{renew_lease, run_lease_loop};
use crate::types::{AnyResource, LeaseRecord, NodeState};

/// In-memory index tracking node load. Avoids full store scan per tick.
pub struct SchedulerIndex {
    node_loads: std::collections::HashMap<String, u32>,
}

impl SchedulerIndex {
    pub fn new() -> Self {
        Self {
            node_loads: std::collections::HashMap::new(),
        }
    }

    /// Incrementally update a node's load by delta.
    pub fn update_load(&mut self, node: &str, delta: i32) {
        let load = self.node_loads.entry(node.to_string()).or_default();
        *load = (*load as i32 + delta).max(0) as u32;
    }

    /// Register a node (from heartbeat or discovery).
    pub fn ensure_node(&mut self, node: &str) {
        self.node_loads.entry(node.to_string()).or_insert(0);
    }

    /// Remove a dead node from the index.
    pub fn remove_node(&mut self, node: &str) {
        self.node_loads.remove(node);
    }

    /// Return (node_name, load) pairs sorted by least-loaded.
    pub fn sorted_by_load(&self) -> Vec<(&str, u32)> {
        let mut pairs: Vec<(&str, u32)> = self
            .node_loads
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        pairs.sort_by_key(|(_, c)| *c);
        pairs
    }

    /// Find the least-loaded node.
    pub fn least_loaded(&self) -> Option<&str> {
        self.sorted_by_load().into_iter().next().map(|(n, _)| n)
    }

    /// Full rebuild from store (only at startup or after reconnect).
    pub async fn rebuild(
        &mut self,
        store: &Arc<dyn StoreBackend>,
        db: &Arc<crate::store::RedbBackend>,
    ) {
        self.node_loads.clear();
        // Start with local node
        let local = crate::config::get().node_name.clone();
        self.node_loads.insert(local, 0);

        // Add gossiped nodes
        for t in store.get_by_kind("Node").await {
            if let AnyResource::Node(n) = t.resource {
                if let Some(name) = n.metadata.name {
                    self.node_loads.entry(name).or_insert(0);
                }
            }
        }

        // Count pod assignments
        for t in store.get_by_kind("Pod").await {
            if let AnyResource::Pod(p) = t.resource {
                if let Some(node) = p.assigned_node {
                    *self.node_loads.entry(node).or_insert(0) += 1;
                }
            }
        }

        // Remove dead nodes
        let dead: Vec<String> = self
            .node_loads
            .keys()
            .filter(|name| {
                // Block on a future inside a filter is bad, so we check synchronously
                // by spawning a blocking task. For simplicity, we skip dead-node filtering
                // on rebuild and rely on the heartbeat deadline check in scheduler_tick.
                false
            })
            .cloned()
            .collect();
        for name in dead {
            self.node_loads.remove(&name);
        }
    }
}

/// Build a snapshot of live nodes and their current pod counts in one pass.
/// Returns a Vec of (node_name, pod_count) sorted least-loaded first.
async fn node_load_snapshot(
    store: &Arc<dyn StoreBackend>,
    db: &Arc<crate::store::RedbBackend>,
) -> Vec<(String, u32)> {
    // Collect all known node names (local + gossiped)
    let mut node_names = vec![crate::config::get().node_name.clone()];
    for t in store.get_by_kind("Node").await {
        if let AnyResource::Node(n) = t.resource {
            if let Some(name) = n.metadata.name {
                if !node_names.contains(&name) {
                    node_names.push(name);
                }
            }
        }
    }

    // Count pod assignments in a single pass over all pods
    let mut counts: std::collections::HashMap<String, u32> = node_names
        .iter()
        .map(|n| (n.clone(), 0u32))
        .collect();
    for t in store.get_by_kind("Pod").await {
        if let AnyResource::Pod(p) = t.resource {
            if let Some(node) = p.assigned_node {
                counts.entry(node).and_modify(|c| *c += 1);
            }
        }
    }

    // Filter out dead nodes
    let mut result = Vec::new();
    for name in node_names {
        if let Some(rec) = db.read_node(&name).await {
            if rec.state == NodeState::Dead {
                continue;
            }
        }
        let cnt = counts.get(&name).copied().unwrap_or(0);
        result.push((name, cnt));
    }

    // Stable sort: least-loaded first
    result.sort_by_key(|(_, c)| *c);
    result
}

pub async fn scheduler_tick(
    store: &Arc<dyn StoreBackend>,
    db: &Arc<crate::store::RedbBackend>,
    lease: &LeaseRecord,
    gs: &Option<Arc<tokio::sync::Mutex<crate::store::gossip::GossipState>>>,
    store_events: &crate::store::StoreEventHub,
    notify: &Arc<tokio::sync::Notify>,
) -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let deadline = now - 30_000;
    let mut total = 0;

    let pods = store.get_by_kind("Pod").await;

    // Snapshot node load once — O(nodes + pods) instead of O(pods * pods).
    // We maintain a local counter and update it as we assign, so each subsequent
    // assignment sees the updated load and distributes evenly.
    let mut node_loads = node_load_snapshot(store, db).await;
    if node_loads.is_empty() {
        return 0;
    }

    // --- Pass 1: schedule unassigned pods ---
    let mut to_broadcast: Vec<AnyResource> = Vec::new();
    for t in &pods {
        if let AnyResource::Pod(p) = &t.resource {
            if p.assigned_node.is_some() {
                continue;
            }
            // Pick the least-loaded node (first after sort)
            let node = node_loads[0].0.clone();
            let mut pod = p.clone();
            pod.assigned_node = Some(node.clone());
            pod.scheduler_epoch = lease.epoch;
            match store.apply(AnyResource::Pod(pod.clone())).await {
                Ok(()) => {
                    tracing::info!("Assigned {} -> {}", t.resource.name(), node);
                    // Update local load counter so next pod sees the correct balance
                    node_loads[0].1 += 1;
                    node_loads.sort_by_key(|(_, c)| *c);
                    to_broadcast.push(AnyResource::Pod(pod));
                    total += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to assign {} -> {}: {}", t.resource.name(), node, e);
                }
            }
        }
    }

    // --- Pass 2: re-assign pods on dead nodes ---
    for t in &pods {
        if let AnyResource::Pod(p) = &t.resource {
            if let Some(ref assigned) = p.assigned_node {
                if let Some(rec) = db.read_node(assigned).await {
                    if rec.last_seen >= deadline && rec.state != NodeState::Dead {
                        continue;
                    }
                }
                if node_loads.is_empty() {
                    continue;
                }
                let node = node_loads[0].0.clone();
                let mut pod = p.clone();
                pod.assigned_node = Some(node.clone());
                pod.scheduler_epoch = lease.epoch;
                if store.apply(AnyResource::Pod(pod.clone())).await.is_ok() {
                    node_loads[0].1 += 1;
                    node_loads.sort_by_key(|(_, c)| *c);
                    info!("Re-assigned {} from dead {} -> {}", t.resource.name(), assigned, node);
                    to_broadcast.push(AnyResource::Pod(pod));
                    total += 1;
                }
            }
        }
    }

    // --- Broadcast all assignments in one go, outside any per-pod lock ---
    if !to_broadcast.is_empty() {
        if let Some(state) = gs {
            let mut gs_lock = state.lock().await;
            for resource in &to_broadcast {
                gs_lock.queue_write(resource);
            }
            gs_lock.flush_batch().await;
        }
        for resource in &to_broadcast {
            store_events.emit_applied(resource.clone(), crate::store::StoreChange::Updated);
        }
        notify.notify_one();
    }

    total
}

pub async fn run_scheduler(
    store: Arc<dyn StoreBackend>,
    node_name: String,
    db: Arc<crate::store::RedbBackend>,
    gs: Option<Arc<tokio::sync::Mutex<crate::store::gossip::GossipState>>>,
    store_events: crate::store::StoreEventHub,
    notify: Arc<tokio::sync::Notify>,
) {
    let mut lease = run_lease_loop(db.clone(), node_name.clone()).await;
    crate::config::set_scheduler_leader(true);
    info!("Scheduler {} active (epoch {})", node_name, lease.epoch);

    // Build initial index from store
    let mut index = SchedulerIndex::new();
    index.rebuild(&store, &db).await;

    loop {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        if lease.expires_at_ms < now {
            crate::config::set_scheduler_leader(false);
            lease = run_lease_loop(db.clone(), node_name.clone()).await;
            crate::config::set_scheduler_leader(true);
            info!(
                "Scheduler {} re-acquired lease (epoch {})",
                node_name, lease.epoch
            );
            // Rebuild index after lease re-acquisition
            index.rebuild(&store, &db).await;
            continue;
        }
        if lease.expires_at_ms - now < 10_000 {
            if let Some(l) = renew_lease(db.clone(), &node_name, &lease).await {
                lease = l;
            } else {
                crate::config::set_scheduler_leader(false);
                lease = run_lease_loop(db.clone(), node_name.clone()).await;
                crate::config::set_scheduler_leader(true);
                continue;
            }
        }

        let n = scheduler_tick(&store, &db, &lease, &gs, &store_events, &notify).await;
        if n > 0 {
            // Rebuild index after scheduling to reflect new assignments
            index.rebuild(&store, &db).await;
            debug!("Scheduler {} scheduled {} pods", node_name, n);
        }

        sleep(Duration::from_secs(3)).await;
    }
}
