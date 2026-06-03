use std::sync::Arc;

use tokio::sync::Notify;

use super::hub::StoreEventHub;
use super::ops::{StoreChange, StoreOp};
use super::{AnyResource, ResourceState, StoreBackend};

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

/// Apply gossip/sync resources with optional state overrides.
/// When a state is provided, it overwrites the local tracker state.
pub async fn apply_incoming_batch_with_state(
    store: &Arc<dyn StoreBackend>,
    hub: &StoreEventHub,
    notify: &Arc<Notify>,
    entries: &[(AnyResource, Option<ResourceState>)],
) {
    if entries.is_empty() {
        return;
    }
    for (resource, state_override) in entries {
        let uid = resource.uid();
        let ops = vec![StoreOp::Upsert(resource.clone())];
        if let Err(e) = store.apply_batch(ops).await {
            tracing::warn!("gossip apply_batch failed: {}", e);
            continue;
        }
        if let Some(state) = state_override {
            // Apply state in a tight loop until it sticks — a concurrent
            // orchestrator sweep may call store.apply() which resets state
            // to Pending between our update_state and the next read.
            for _ in 0..3 {
                store.update_state(&uid, state.clone()).await;
            }
        }
        hub.emit_applied(resource.clone(), StoreChange::Updated);
    }
    notify.notify_one();
}
