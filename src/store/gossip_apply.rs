use std::sync::Arc;

use tokio::sync::Notify;

use super::hub::StoreEventHub;
use super::ops::{StoreChange, StoreOp};
use super::{AnyResource, StoreBackend};

/// Apply gossip/sync resources in one store transaction and wake the orchestrator once.
pub async fn apply_incoming_batch(
    store: &Arc<dyn StoreBackend>,
    hub: &StoreEventHub,
    notify: &Arc<Notify>,
    resources: Vec<AnyResource>,
) {
    if resources.is_empty() {
        return;
    }
    let ops: Vec<StoreOp> = resources
        .iter()
        .cloned()
        .map(StoreOp::Upsert)
        .collect();
    if let Err(e) = store.apply_batch(ops).await {
        tracing::warn!("gossip apply_batch failed: {}", e);
        return;
    }
    for resource in resources {
        hub.emit_applied(resource, StoreChange::Updated);
    }
    notify.notify_one();
}
