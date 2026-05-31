use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::store::leases::{renew_lease, run_lease_loop};
use crate::store::StoreBackend;
use crate::types::{AnyResource, LeaseRecord, NodeState};

async fn pick_node(store: &Arc<dyn StoreBackend>, db: &Arc<crate::store::RedbBackend>) -> Option<String> {
    let mut best: Option<(String, u32)> = None;
    let node_name = crate::config::get().node_name.clone();
    tracing::info!("pick_node: reading node {}", node_name);
    if let Some(rec) = db.read_node(&node_name).await {
        tracing::info!("pick_node: found node {}, state={:?}, pods={}, last_seen={}", rec.node_name, rec.state, rec.pod_count, rec.last_seen);
        if rec.state != NodeState::Dead {
            let count = store.get_by_kind("Pod").await.iter()
                .filter(|t| matches!(&t.resource, AnyResource::Pod(p) if p.assigned_node.as_deref() == Some(&rec.node_name)))
                .count() as u32;
            best = Some((rec.node_name, count));
        }
    } else {
        tracing::info!("pick_node: no node record found for {}", node_name);
    }
    best.map(|(n, _)| n)
}

pub async fn scheduler_tick(
    store: &Arc<dyn StoreBackend>,
    db: &Arc<crate::store::RedbBackend>,
    lease: &LeaseRecord,
) -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let deadline = now - 30_000;
    let mut total = 0;

    let pods = store.get_by_kind("Pod").await;

    for t in &pods {
        if let AnyResource::Pod(p) = &t.resource {
            if p.assigned_node.is_some() { continue; }
            if let Some(node) = pick_node(store, db).await {
                let mut pod = p.clone();
                pod.assigned_node = Some(node.clone());
                pod.scheduler_epoch = lease.epoch;
                match store.apply(AnyResource::Pod(pod.clone())).await {
                    Ok(()) => {
                        // Verify by reading back
                        let uid = format!("Pod/default/{}", t.resource.name());
                        match store.get(&uid).await {
                            Some(tracker) => {
                                if let AnyResource::Pod(p) = &tracker.resource {
                                    tracing::info!("Assigned {} -> {}, verify: assigned_node={:?}", t.resource.name(), node, p.assigned_node);
                                }
                            }
                            None => tracing::warn!("Assigned {} but verify not found!", t.resource.name()),
                        }
                    }
                    Err(e) => tracing::error!("Failed to assign {} -> {}: {}", t.resource.name(), node, e),
                }
                total += 1;
            }
        }
    }

    for t in &pods {
        if let AnyResource::Pod(p) = &t.resource {
            if let Some(ref assigned) = p.assigned_node {
                if let Some(rec) = db.read_node(assigned).await {
                    if rec.last_seen >= deadline && rec.state != NodeState::Dead { continue; }
                }
                if let Some(node) = pick_node(store, db).await {
                    let mut pod = p.clone();
                    pod.assigned_node = Some(node.clone());
                    pod.scheduler_epoch = lease.epoch;
                    store.apply(AnyResource::Pod(pod)).await.ok();
                    total += 1;
                    info!("Re-assigned {} from dead {} -> {}", t.resource.name(), assigned, node);
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
) {
    let mut lease = run_lease_loop(db.clone(), node_name.clone()).await;
    info!("Scheduler {} active (epoch {})", node_name, lease.epoch);

    loop {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;

        if lease.expires_at_ms < now {
            lease = run_lease_loop(db.clone(), node_name.clone()).await;
            info!("Scheduler {} re-acquired lease (epoch {})", node_name, lease.epoch);
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

        let n = scheduler_tick(&store, &db, &lease).await;
        if n > 0 { debug!("Scheduler {} scheduled {} pods", node_name, n); }

        sleep(Duration::from_secs(3)).await;
    }
}
