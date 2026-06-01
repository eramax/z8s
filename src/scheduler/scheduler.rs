use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::store::StoreBackend;
use crate::store::leases::{renew_lease, run_lease_loop};
use crate::types::{AnyResource, LeaseRecord, NodeState};

async fn pick_node(
    store: &Arc<dyn StoreBackend>,
    db: &Arc<crate::store::RedbBackend>,
) -> Option<String> {
    let mut best: Option<(String, u32)> = None;
    
    // Always include the local node
    let mut node_names = vec![crate::config::get().node_name.clone()];
    
    // Include all gossiped nodes
    for t in store.get_by_kind("Node").await {
        if let AnyResource::Node(n) = t.resource {
            if let Some(name) = n.metadata.name {
                if !node_names.contains(&name) {
                    node_names.push(name);
                }
            }
        }
    }

    for node_name in node_names {
        // Read local state for this node if we have it (e.g. for the local node itself)
        if let Some(rec) = db.read_node(&node_name).await {
            if rec.state == NodeState::Dead {
                continue;
            }
        }
        
        // Count pods assigned to this node
        let count = store.get_by_kind("Pod").await.iter()
            .filter(|t| matches!(&t.resource, AnyResource::Pod(p) if p.assigned_node.as_deref() == Some(node_name.as_str())))
            .count() as u32;

        if best.is_none() || count < best.as_ref().unwrap().1 {
            best = Some((node_name, count));
        }
    }
    
    best.map(|(n, _)| n)
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

    for t in &pods {
        if let AnyResource::Pod(p) = &t.resource {
            if p.assigned_node.is_some() {
                continue;
            }
            if let Some(node) = pick_node(store, db).await {
                let mut pod = p.clone();
                pod.assigned_node = Some(node.clone());
                pod.scheduler_epoch = lease.epoch;
                match store.apply(AnyResource::Pod(pod.clone())).await {
                    Ok(()) => {
                        if let Some(state) = gs {
                            state.lock().await.broadcast_write(&AnyResource::Pod(pod.clone())).await;
                        }
                        // Verify by reading back
                        let ns = p.metadata.namespace.as_deref().unwrap_or("default");
                        let uid = format!("Pod/{}/{}", ns, t.resource.name());
                        match store.get(&uid).await {
                            Some(tracker) => {
                                if let AnyResource::Pod(p) = &tracker.resource {
                                    tracing::info!(
                                        "Assigned {} -> {}, verify: assigned_node={:?}",
                                        t.resource.name(),
                                        node,
                                        p.assigned_node
                                    );
                                }
                            }
                            None => tracing::warn!(
                                "Assigned {} but verify not found!",
                                t.resource.name()
                            ),
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to assign {} -> {}: {}", t.resource.name(), node, e)
                    }
                }
                total += 1;
            }
        }
    }

    for t in &pods {
        if let AnyResource::Pod(p) = &t.resource {
            if let Some(ref assigned) = p.assigned_node {
                if let Some(rec) = db.read_node(assigned).await {
                    if rec.last_seen >= deadline && rec.state != NodeState::Dead {
                        continue;
                    }
                }
                if let Some(node) = pick_node(store, db).await {
                    let mut pod = p.clone();
                    pod.assigned_node = Some(node.clone());
                    pod.scheduler_epoch = lease.epoch;
                    store.apply(AnyResource::Pod(pod.clone())).await.ok();
                    if let Some(state) = gs {
                        state.lock().await.broadcast_write(&AnyResource::Pod(pod)).await;
                    }
                    total += 1;
                    info!(
                        "Re-assigned {} from dead {} -> {}",
                        t.resource.name(),
                        assigned,
                        node
                    );
                }
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
    info!("Scheduler {} active (epoch {})", node_name, lease.epoch);

    loop {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        if lease.expires_at_ms < now {
            lease = run_lease_loop(db.clone(), node_name.clone()).await;
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
                lease = run_lease_loop(db.clone(), node_name.clone()).await;
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
