use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::store::StoreBackend;
use crate::store::leases::{renew_lease, run_lease_loop};
use crate::types::{AnyResource, LeaseRecord, NodeState};

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
            let gs_lock = state.lock().await;
            for resource in &to_broadcast {
                gs_lock.broadcast_write(resource).await;
            }
        }
    }

    total
}

pub async fn run_scheduler(
    store: Arc<dyn StoreBackend>,
    node_name: String,
    db: Arc<crate::store::RedbBackend>,
    gs: Option<Arc<tokio::sync::Mutex<crate::store::gossip::GossipState>>>,
) {
    let mut lease = run_lease_loop(db.clone(), node_name.clone()).await;
    crate::config::set_scheduler_leader(true);
    info!("Scheduler {} active (epoch {})", node_name, lease.epoch);

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

        let n = scheduler_tick(&store, &db, &lease, &gs).await;
        if n > 0 {
            debug!("Scheduler {} scheduled {} pods", node_name, n);
        }

        sleep(Duration::from_secs(3)).await;
    }
}
