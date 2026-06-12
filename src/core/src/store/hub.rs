//! # StoreEventHub — Reactive Event Distribution
//!
//! The StoreEventHub is the central event bus for the store. When a resource
//! is created, updated, or deleted, the store emits an event through this hub.
//!
//! ## How It Works
//!
//! ```text
//! store.write_spec(pod)  →  store emits StoreEvent::Applied
//!                              ↓
//!                     StoreEventHub::emit_applied(event)
//!                              ↓
//!                     ┌────────┼────────┐
//!                     ▼        ▼        ▼
//!                  API      Sync     DNS
//!                (watch)  (gossip) (cache)
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! // Subscribe to events
//! let mut rx = hub.subscribe();
//!
//! // Receive events
//! while let Ok(event) = rx.recv().await {
//!     match event {
//!         StoreEvent::Applied { record, change } => { /* handle */ }
//!         StoreEvent::Deleted { uid, .. } => { /* handle */ }
//!     }
//! }
//! ```
//!
//! ## Thread Safety
//!
//! `StoreEventHub` is `Clone` — it can be shared across async tasks.
//! Internally it uses `tokio::sync::broadcast` for efficient fan-out.

use tokio::sync::broadcast;

use super::ops::{StoreEvent, StoreChange};
use crate::types::ResourceRecord;

/// Central event bus for store changes.
///
/// All store writes emit events through this hub. Subscribers receive
/// events reactively without polling.
#[derive(Clone)]
pub struct StoreEventHub {
    tx: broadcast::Sender<StoreEvent>,
}

impl StoreEventHub {
    /// Create a new event hub with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Emit an "applied" event (resource created or updated).
    pub fn emit_applied(&self, record: ResourceRecord, change: StoreChange) {
        let _ = self.tx.send(StoreEvent::Applied { record, change });
    }

    /// Emit a "deleted" event.
    pub fn emit_deleted(&self, uid: &str, kind: &str, namespace: Option<&str>, name: &str) {
        let _ = self.tx.send(StoreEvent::Deleted {
            uid: uid.to_string(),
            kind: kind.to_string(),
            namespace: namespace.map(|s| s.to_string()),
            name: name.to_string(),
        });
    }

    /// Subscribe to events. Returns a receiver that gets all future events.
    pub fn subscribe(&self) -> broadcast::Receiver<StoreEvent> {
        self.tx.subscribe()
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for StoreEventHub {
    fn default() -> Self {
        Self::new(1024)
    }
}
