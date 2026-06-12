//! # MemoryBackend — In-Memory Store for Testing
//!
//! A simple in-memory implementation of `StoreBackend`. Useful for unit tests
//! where you don't want to create a redb database.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let store = MemoryBackend::new();
//! store.write_spec(pod.into_any(), None).await?;
//! let records = store.get_by_kind("Pod").await;
//! ```
//!
//! ## Limitations
//!
//! - Not persistent (data lost on restart)
//! - Not thread-safe across processes (single process only)
//! - No transaction support (each write is immediate)

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;

use crate::types::{AnyResource, EventRecord, EventType, ResourceRecord, ResourceStatus};
use super::{StoreBackend, StoreEventHub, StoreOp, StoreChange, StoreSnapshot};

/// In-memory store backend for testing.
pub struct MemoryBackend {
    records: Arc<RwLock<HashMap<String, ResourceRecord>>>,
    events: Arc<RwLock<Vec<EventRecord>>>,
    hub: StoreEventHub,
}

impl MemoryBackend {
    /// Create a new empty memory store.
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(Vec::new())),
            hub: StoreEventHub::new(1024),
        }
    }

    /// Get the event hub for subscribing to changes.
    pub fn hub(&self) -> &StoreEventHub {
        &self.hub
    }
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StoreBackend for MemoryBackend {
    async fn write_spec(
        &self,
        spec: AnyResource,
        assigned_node: Option<String>,
    ) -> anyhow::Result<()> {
        let uid = spec.uid().to_string();
        let mut records = self.records.write().await;

        let change = if let Some(existing) = records.get_mut(&uid) {
            // Update existing record
            existing.spec = spec;
            existing.assigned_node = assigned_node;
            existing.generation += 1;
            existing.last_updated = crate::types::helpers::now_epoch_ms();
            StoreChange::Updated
        } else {
            // Create new record
            let mut record = ResourceRecord::new(spec);
            record.assigned_node = assigned_node;
            records.insert(uid.clone(), record.clone());
            StoreChange::Created
        };

        if let Some(record) = records.get(&uid) {
            self.hub.emit_applied(record.clone(), change);
        }

        Ok(())
    }

    async fn assign_node(&self, uid: &str, node: &str) -> anyhow::Result<()> {
        let mut records = self.records.write().await;
        if let Some(record) = records.get_mut(uid) {
            record.assigned_node = Some(node.to_string());
            record.generation += 1;
            record.last_updated = crate::types::helpers::now_epoch_ms();
            self.hub.emit_applied(record.clone(), StoreChange::Updated);
        }
        Ok(())
    }

    async fn set_deletion_timestamp(&self, uid: &str) -> anyhow::Result<()> {
        let mut records = self.records.write().await;
        if let Some(record) = records.get_mut(uid) {
            record.spec.metadata_mut().deletion_timestamp = Some(crate::types::Time::now());
            record.generation += 1;
            record.last_updated = crate::types::helpers::now_epoch_ms();
            self.hub.emit_applied(record.clone(), StoreChange::Updated);
        }
        Ok(())
    }

    async fn write_status(&self, uid: &str, status: ResourceStatus) -> anyhow::Result<()> {
        let mut records = self.records.write().await;
        if let Some(record) = records.get_mut(uid) {
            record.status = status;
            record.last_updated = crate::types::helpers::now_epoch_ms();
            self.hub.emit_applied(record.clone(), StoreChange::Updated);
        }
        Ok(())
    }

    async fn set_observed_generation(&self, uid: &str, generation: u64) -> anyhow::Result<()> {
        let mut records = self.records.write().await;
        if let Some(record) = records.get_mut(uid) {
            record.observed_generation = generation;
            record.last_updated = crate::types::helpers::now_epoch_ms();
        }
        Ok(())
    }

    async fn get(&self, uid: &str) -> Option<ResourceRecord> {
        self.records.read().await.get(uid).cloned()
    }

    async fn get_all(&self) -> Vec<ResourceRecord> {
        self.records.read().await.values().cloned().collect()
    }

    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceRecord> {
        self.records.read().await.values()
            .filter(|r| r.kind() == kind)
            .cloned()
            .collect()
    }

    async fn get_by_node(&self, node: &str) -> Vec<ResourceRecord> {
        self.records.read().await.values()
            .filter(|r| r.assigned_node.as_deref() == Some(node))
            .cloned()
            .collect()
    }

    async fn get_unassigned(&self, kind: &str) -> Vec<ResourceRecord> {
        self.records.read().await.values()
            .filter(|r| r.kind() == kind && r.assigned_node.is_none())
            .cloned()
            .collect()
    }

    async fn get_needing_reconcile(&self, node: &str) -> Vec<ResourceRecord> {
        self.records.read().await.values()
            .filter(|r| r.assigned_node.as_deref() == Some(node) && r.needs_reconcile())
            .cloned()
            .collect()
    }

    async fn delete(&self, uid: &str) -> anyhow::Result<()> {
        let mut records = self.records.write().await;
        if let Some(record) = records.remove(uid) {
            self.hub.emit_deleted(
                uid,
                record.kind(),
                record.spec.namespace(),
                record.name(),
            );
        }
        Ok(())
    }

    async fn apply_batch(&self, ops: Vec<StoreOp>) -> anyhow::Result<()> {
        for op in ops {
            match op {
                StoreOp::WriteSpec { spec, assigned_node } => {
                    self.write_spec(spec, assigned_node).await?;
                }
                StoreOp::WriteStatus { uid, status } => {
                    self.write_status(&uid, status).await?;
                }
                StoreOp::SetObserved { uid, generation } => {
                    self.set_observed_generation(&uid, generation).await?;
                }
                StoreOp::Delete { uid } => {
                    self.delete(&uid).await?;
                }
            }
        }
        Ok(())
    }

    async fn snapshot(&self) -> StoreSnapshot {
        StoreSnapshot::from_records(self.get_all().await)
    }

    async fn record_event(
        &self,
        resource: &AnyResource,
        reason: &str,
        message: &str,
        source: &str,
        event_type: EventType,
    ) -> anyhow::Result<EventRecord> {
        let mut events = self.events.write().await;
        let event_id = events.iter()
            .filter(|e| e.resource_uid == resource.uid())
            .count() as u64 + 1;

        let mut event = EventRecord::new(
            reason,
            message,
            source,
            resource.kind(),
            resource.name(),
            resource.namespace(),
            resource.uid(),
            event_type,
        );
        event.event_id = event_id;
        events.push(event.clone());
        Ok(event)
    }

    async fn get_events(&self, resource_uid: &str) -> Vec<EventRecord> {
        let events = self.events.read().await;
        let mut result: Vec<EventRecord> = events.iter()
            .filter(|e| e.resource_uid == resource_uid)
            .cloned()
            .collect();
        result.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
        result
    }

    async fn get_events_by_kind(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Vec<EventRecord> {
        let events = self.events.read().await;
        events.iter()
            .filter(|e| e.resource_kind == kind)
            .filter(|e| namespace.is_none_or(|ns| e.resource_namespace.as_deref() == Some(ns)))
            .cloned()
            .collect()
    }

    async fn get_recent_events(&self, limit: usize) -> Vec<EventRecord> {
        let mut events = self.events.read().await.clone();
        events.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
        events.truncate(limit);
        events
    }

    async fn prune_events(&self, keep_per_resource: usize) -> anyhow::Result<usize> {
        let mut events = self.events.write().await;
        let mut by_resource: HashMap<String, Vec<usize>> = HashMap::new();

        for (i, event) in events.iter().enumerate() {
            by_resource.entry(event.resource_uid.clone())
                .or_default()
                .push(i);
        }

        let mut to_remove = Vec::new();
        for indices in by_resource.values() {
            if indices.len() > keep_per_resource {
                // indices are in insertion order, remove oldest (first N)
                to_remove.extend(indices.iter().take(indices.len() - keep_per_resource));
            }
        }

        to_remove.sort_unstable();
        to_remove.reverse();
        let count = to_remove.len();
        for idx in to_remove {
            events.remove(idx);
        }

        Ok(count)
    }
}
