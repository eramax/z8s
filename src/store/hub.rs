use std::sync::Arc;

use tokio::sync::broadcast;

use super::ops::{StoreChange, StoreEvent};
use crate::store::AnyResource;

const HUB_CAPACITY: usize = 4096;

/// Broadcast channel for store mutations (API, gossip, watcher → scheduler).
#[derive(Clone)]
pub struct StoreEventHub {
    tx: Arc<broadcast::Sender<StoreEvent>>,
}

impl StoreEventHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(HUB_CAPACITY);
        Self { tx: Arc::new(tx) }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StoreEvent> {
        self.tx.subscribe()
    }

    pub fn emit_applied(&self, resource: AnyResource, change: StoreChange) {
        let _ = self.tx.send(StoreEvent::Applied { resource, change });
    }

    pub fn emit_deleted(&self, resource: AnyResource) {
        let _ = self.tx.send(StoreEvent::Deleted { resource });
    }
}
