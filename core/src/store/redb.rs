//! # RedbBackend — Persistent Store Using redb
//!
//! The primary storage backend for z8s. Uses the redb embedded database
//! for ACID transactions and crash safety.
//!
//! ## Tables
//!
//! - `resources` — ResourceRecord keyed by UID
//! - `events` — EventRecord keyed by "{uid}/{event_id}"
//! - `leases` — Leader election leases
//! - `nodes` — Node heartbeat records
//! - `join_tokens` — Cluster join tokens
//!
//! ## In-Memory Index
//!
//! For fast lookups, we maintain an in-memory index alongside the redb tables:
//! - `by_kind` — kind → Vec<UID>
//! - `by_node` — node → Vec<UID>
//! - `unassigned` — kind → Vec<UID>
//!
//! The index is rebuilt on startup and updated on every write.
//!
//! ## Thread Safety
//!
//! redb handles its own locking. We wrap writes in `spawn_blocking` to avoid
//! blocking the async runtime.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::info;

use crate::types::{AnyResource, EventRecord, EventType, ResourceRecord, ResourceStatus};
use super::{StoreBackend, StoreEventHub, StoreOp, StoreChange, StoreSnapshot};

// ── Table Definitions ─────────────────────────────────────────────────────

const RESOURCES: TableDefinition<&str, &[u8]> = TableDefinition::new("resources");
const EVENTS: TableDefinition<&str, &[u8]> = TableDefinition::new("events");
const LEASES: TableDefinition<&str, &[u8]> = TableDefinition::new("leases");
const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");
const JOIN_TOKENS: TableDefinition<&str, &[u8]> = TableDefinition::new("join_tokens");

// ── RedbBackend ───────────────────────────────────────────────────────────

/// Persistent store backend using redb.
pub struct RedbBackend {
    db: Arc<Database>,
    hub: StoreEventHub,
    /// In-memory index for fast lookups.
    index: Arc<RwLock<StoreIndex>>,
}

/// In-memory index for fast queries.
#[derive(Debug, Default)]
struct StoreIndex {
    by_kind: HashMap<String, Vec<String>>,
    by_node: HashMap<String, Vec<String>>,
    unassigned: HashMap<String, Vec<String>>,
}

impl StoreIndex {
    /// Rebuild from a list of records.
    fn rebuild(&mut self, records: &[ResourceRecord]) {
        self.by_kind.clear();
        self.by_node.clear();
        self.unassigned.clear();

        for record in records {
            let uid = record.uid().to_string();
            let kind = record.kind().to_string();

            // by_kind
            self.by_kind.entry(kind.clone()).or_default().push(uid.clone());

            // by_node
            if let Some(ref node) = record.assigned_node {
                self.by_node.entry(node.clone()).or_default().push(uid.clone());
            }

            // unassigned
            if record.assigned_node.is_none() {
                self.unassigned.entry(kind).or_default().push(uid);
            }
        }
    }

    /// Update index on write.
    fn apply_write(&mut self, record: &ResourceRecord) {
        let uid = record.uid().to_string();
        let kind = record.kind().to_string();

        // Remove old entries
        for uids in self.by_kind.values_mut() {
            uids.retain(|u| u != &uid);
        }
        for uids in self.by_node.values_mut() {
            uids.retain(|u| u != &uid);
        }
        for uids in self.unassigned.values_mut() {
            uids.retain(|u| u != &uid);
        }

        // Add new entries
        self.by_kind.entry(kind.clone()).or_default().push(uid.clone());
        if let Some(ref node) = record.assigned_node {
            self.by_node.entry(node.clone()).or_default().push(uid.clone());
        }
        if record.assigned_node.is_none() {
            self.unassigned.entry(kind).or_default().push(uid);
        }
    }

    /// Remove from index on delete.
    fn apply_delete(&mut self, uid: &str) {
        for uids in self.by_kind.values_mut() {
            uids.retain(|u| u != uid);
        }
        for uids in self.by_node.values_mut() {
            uids.retain(|u| u != uid);
        }
        for uids in self.unassigned.values_mut() {
            uids.retain(|u| u != uid);
        }
    }
}

impl RedbBackend {
    /// Open or create a redb database at the given path.
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        std::fs::create_dir_all(path)?;
        let db_path = path.join("z8s.redb");
        info!("Opening redb database at {}", db_path.display());

        let db = if db_path.exists() {
            Database::open(&db_path)?
        } else {
            Database::create(&db_path)?
        };

        // Ensure tables exist
        {
            let txn = db.begin_write()?;
            txn.open_table(RESOURCES)?;
            txn.open_table(EVENTS)?;
            txn.open_table(LEASES)?;
            txn.open_table(NODES)?;
            txn.open_table(JOIN_TOKENS)?;
            txn.commit()?;
        }

        // Load records for index
        let records = Self::load_all_records(&db)?;
        let mut index = StoreIndex::default();
        index.rebuild(&records);

        info!("Loaded {} records from database", records.len());

        Ok(Self {
            db: Arc::new(db),
            hub: StoreEventHub::new(1024),
            index: Arc::new(RwLock::new(index)),
        })
    }

    /// Get the event hub.
    pub fn hub(&self) -> &StoreEventHub {
        &self.hub
    }

    /// Load all records from the database.
    fn load_all_records(db: &Database) -> anyhow::Result<Vec<ResourceRecord>> {
        let txn = db.begin_read()?;
        let table = txn.open_table(RESOURCES)?;
        let mut records = Vec::new();

        for entry in table.iter()? {
            let entry = entry?;
            if let Ok(record) = serde_json::from_slice(entry.1.value()) {
                records.push(record);
            }
        }

        Ok(records)
    }

    /// Write a single record to the database.
    fn write_record(db: &Database, record: &ResourceRecord) -> anyhow::Result<()> {
        let txn = db.begin_write()?;
        {
            let mut table = txn.open_table(RESOURCES)?;
            let key = record.uid();
            let bytes = serde_json::to_vec(record)?;
            table.insert(key as &str, bytes.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Delete a record from the database.
    fn delete_record(db: &Database, uid: &str) -> anyhow::Result<()> {
        let txn = db.begin_write()?;
        {
            let mut table = txn.open_table(RESOURCES)?;
            table.remove(uid)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Batch write records in a single transaction.
    fn batch_write(db: &Database, records: &[ResourceRecord], deletes: &[&str]) -> anyhow::Result<()> {
        let txn = db.begin_write()?;
        {
            let mut table = txn.open_table(RESOURCES)?;
            for record in records {
                let key = record.uid();
                let bytes = serde_json::to_vec(record)?;
                table.insert(key as &str, bytes.as_slice())?;
            }
            for uid in deletes {
                table.remove(*uid)?;
            }
        }
        txn.commit()?;
        Ok(())
    }
}

#[async_trait]
impl StoreBackend for RedbBackend {
    async fn write_spec(
        &self,
        spec: AnyResource,
        assigned_node: Option<String>,
    ) -> anyhow::Result<()> {
        let db = self.db.clone();
        let hub = self.hub.clone();
        let index = self.index.clone();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let uid = spec.uid().to_string();

            // Read existing record
            let (existing, mut record) = {
                let txn = db.begin_read()?;
                let table = txn.open_table(RESOURCES)?;
                match table.get(uid.as_str()).ok().flatten() {
                    Some(guard) => {
                        let bytes = guard.value().to_vec();
                        let mut r: ResourceRecord = serde_json::from_slice(&bytes)?;
                        r.spec = spec;
                        r.generation += 1;
                        (true, r)
                    }
                    _ => (false, ResourceRecord::new(spec)),
                }
            };
            record.assigned_node = assigned_node;
            record.last_updated = crate::types::helpers::now_epoch_ms();

            Self::write_record(&db, &record)?;

            // Update index
            let change = if existing { StoreChange::Updated } else { StoreChange::Created };
            tokio::spawn(async move {
                index.write().await.apply_write(&record);
                hub.emit_applied(record, change);
            });

            Ok(())
        }).await?
    }

    async fn assign_node(&self, uid: &str, node: &str) -> anyhow::Result<()> {
        let db = self.db.clone();
        let hub = self.hub.clone();
        let index = self.index.clone();
        let uid = uid.to_string();
        let node = node.to_string();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let txn = db.begin_read()?;
            let table = txn.open_table(RESOURCES)?;
            let mut record = match table.get(uid.as_str()).ok().flatten() {
                Some(guard) => {
                    let bytes = guard.value().to_vec();
                    serde_json::from_slice::<ResourceRecord>(&bytes)?
                }
                _ => return Ok(()),
            };

            record.assigned_node = Some(node);
            record.generation += 1;
            record.last_updated = crate::types::helpers::now_epoch_ms();

            Self::write_record(&db, &record)?;

            tokio::spawn(async move {
                index.write().await.apply_write(&record);
                hub.emit_applied(record, StoreChange::Updated);
            });

            Ok(())
        }).await?
    }

    async fn set_deletion_timestamp(&self, uid: &str) -> anyhow::Result<()> {
        let db = self.db.clone();
        let hub = self.hub.clone();
        let index = self.index.clone();
        let uid = uid.to_string();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let txn = db.begin_read()?;
            let table = txn.open_table(RESOURCES)?;
            let mut record = match table.get(uid.as_str()).ok().flatten() {
                Some(v) => serde_json::from_slice::<ResourceRecord>(v.value())?,
                None => return Ok(()),
            };

            record.spec.metadata_mut().deletion_timestamp = Some(crate::types::Time::now());
            record.generation += 1;
            record.last_updated = crate::types::helpers::now_epoch_ms();

            Self::write_record(&db, &record)?;

            tokio::spawn(async move {
                index.write().await.apply_write(&record);
                hub.emit_applied(record, StoreChange::Updated);
            });

            Ok(())
        }).await?
    }

    async fn write_status(&self, uid: &str, status: ResourceStatus) -> anyhow::Result<()> {
        let db = self.db.clone();
        let hub = self.hub.clone();
        let index = self.index.clone();
        let uid = uid.to_string();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let txn = db.begin_read()?;
            let table = txn.open_table(RESOURCES)?;
            let mut record = match table.get(uid.as_str()).ok().flatten() {
                Some(v) => serde_json::from_slice::<ResourceRecord>(v.value())?,
                None => return Ok(()),
            };

            record.status = status;
            record.last_updated = crate::types::helpers::now_epoch_ms();

            Self::write_record(&db, &record)?;

            tokio::spawn(async move {
                index.write().await.apply_write(&record);
                hub.emit_applied(record, StoreChange::Updated);
            });

            Ok(())
        }).await?
    }

    async fn set_observed_generation(&self, uid: &str, generation: u64) -> anyhow::Result<()> {
        let db = self.db.clone();
        let uid = uid.to_string();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let txn = db.begin_read()?;
            let table = txn.open_table(RESOURCES)?;
            let mut record = match table.get(uid.as_str()).ok().flatten() {
                Some(v) => serde_json::from_slice::<ResourceRecord>(v.value())?,
                None => return Ok(()),
            };

            record.observed_generation = generation;
            record.last_updated = crate::types::helpers::now_epoch_ms();

            Self::write_record(&db, &record)?;
            Ok(())
        }).await?
    }

    async fn get(&self, uid: &str) -> Option<ResourceRecord> {
        let db = self.db.clone();
        let uid = uid.to_string();

        tokio::task::spawn_blocking(move || -> Option<ResourceRecord> {
            let txn = db.begin_read().ok()?;
            let table = txn.open_table(RESOURCES).ok()?;
            let guard = table.get(uid.as_str()).ok().flatten()?;
            let bytes = guard.value().to_vec();
            serde_json::from_slice(&bytes).ok()
        }).await.ok()?
    }

    async fn get_all(&self) -> Vec<ResourceRecord> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Vec<ResourceRecord> {
            let txn = match db.begin_read() {
                Ok(t) => t,
                Err(_) => return vec![],
            };
            let table = match txn.open_table(RESOURCES) {
                Ok(t) => t,
                Err(_) => return vec![],
            };
            let mut records = Vec::new();
            if let Ok(iter) = table.iter() {
                for entry in iter {
                    if let Ok(entry) = entry
                        && let Ok(record) = serde_json::from_slice(entry.1.value()) {
                            records.push(record);
                        }
                }
            }
            records
        }).await.unwrap_or_default()
    }

    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceRecord> {
        let index = self.index.read().await;
        let uids = index.by_kind.get(kind).cloned().unwrap_or_default();
        drop(index);

        let mut records = Vec::new();
        for uid in &uids {
            if let Some(record) = self.get(uid).await {
                records.push(record);
            }
        }
        records
    }

    async fn get_by_node(&self, node: &str) -> Vec<ResourceRecord> {
        let index = self.index.read().await;
        let uids = index.by_node.get(node).cloned().unwrap_or_default();
        drop(index);

        let mut records = Vec::new();
        for uid in &uids {
            if let Some(record) = self.get(uid).await {
                records.push(record);
            }
        }
        records
    }

    async fn get_unassigned(&self, kind: &str) -> Vec<ResourceRecord> {
        let index = self.index.read().await;
        let uids = index.unassigned.get(kind).cloned().unwrap_or_default();
        drop(index);

        let mut records = Vec::new();
        for uid in &uids {
            if let Some(record) = self.get(uid).await {
                records.push(record);
            }
        }
        records
    }

    async fn get_needing_reconcile(&self, node: &str) -> Vec<ResourceRecord> {
        let records = self.get_by_node(node).await;
        records.into_iter()
            .filter(|r| r.needs_reconcile())
            .collect()
    }

    async fn delete(&self, uid: &str) -> anyhow::Result<()> {
        let db = self.db.clone();
        let hub = self.hub.clone();
        let index = self.index.clone();
        let uid = uid.to_string();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            // Read before delete for event
            let record = {
                let txn = db.begin_read()?;
                let table = txn.open_table(RESOURCES)?;
                match table.get(uid.as_str()) {
                    Ok(Some(guard)) => {
                        let bytes = guard.value().to_vec();
                        serde_json::from_slice::<ResourceRecord>(&bytes).ok()
                    }
                    _ => None,
                }
            };

            Self::delete_record(&db, &uid)?;

            if let Some(record) = record {
                tokio::spawn(async move {
                    index.write().await.apply_delete(&uid);
                    hub.emit_deleted(&uid, record.kind(), record.spec.namespace(), record.name());
                });
            }

            Ok(())
        }).await?
    }

    async fn apply_batch(&self, ops: Vec<StoreOp>) -> anyhow::Result<()> {
        let db = self.db.clone();
        let hub = self.hub.clone();
        let index = self.index.clone();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut to_write = Vec::new();
            let mut to_delete = Vec::new();

            for op in ops {
                match op {
                    StoreOp::WriteSpec { spec, assigned_node } => {
                        let uid = spec.uid().to_string();
                        let (_existing, mut record) = {
                            let txn = db.begin_read()?;
                            let table = txn.open_table(RESOURCES)?;
                            match table.get(uid.as_str()) {
                                Ok(Some(guard)) => {
                                    let bytes = guard.value().to_vec();
                                    let mut r: ResourceRecord = serde_json::from_slice(&bytes)?;
                                    r.spec = spec;
                                    r.generation += 1;
                                    (true, r)
                                }
                                _ => (false, ResourceRecord::new(spec)),
                            }
                        };
                        record.assigned_node = assigned_node;
                        record.last_updated = crate::types::helpers::now_epoch_ms();
                        to_write.push(record);
                    }
                    StoreOp::WriteStatus { uid, status } => {
                        let txn = db.begin_read()?;
                        let table = txn.open_table(RESOURCES)?;
                        if let Ok(Some(v)) = table.get(uid.as_str()) {
                            let mut record: ResourceRecord = serde_json::from_slice(v.value())?;
                            record.status = status;
                            record.last_updated = crate::types::helpers::now_epoch_ms();
                            to_write.push(record);
                        }
                    }
                    StoreOp::SetObserved { uid, generation } => {
                        let txn = db.begin_read()?;
                        let table = txn.open_table(RESOURCES)?;
                        if let Ok(Some(v)) = table.get(uid.as_str()) {
                            let mut record: ResourceRecord = serde_json::from_slice(v.value())?;
                            record.observed_generation = generation;
                            record.last_updated = crate::types::helpers::now_epoch_ms();
                            to_write.push(record);
                        }
                    }
                    StoreOp::Delete { uid } => {
                        to_delete.push(uid);
                    }
                }
            }

            Self::batch_write(&db, &to_write, &to_delete.iter().map(|s| s.as_str()).collect::<Vec<_>>())?;

            // Update index and emit events
            tokio::spawn(async move {
                let mut idx = index.write().await;
                for record in &to_write {
                    idx.apply_write(record);
                    hub.emit_applied(record.clone(), StoreChange::Updated);
                }
                for uid in &to_delete {
                    idx.apply_delete(uid);
                    hub.emit_deleted(uid, "", None, "");
                }
            });

            Ok(())
        }).await?
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
        let db = self.db.clone();
        let resource = resource.clone();
        let reason = reason.to_string();
        let message = message.to_string();
        let source = source.to_string();

        tokio::task::spawn_blocking(move || -> anyhow::Result<EventRecord> {
            let txn = db.begin_read()?;
            let table = txn.open_table(EVENTS)?;

            // Count existing events for this resource to get next event_id
            let prefix = format!("{}/", resource.uid());
            let mut event_id = 0u64;
            for entry in table.range::<&str>(prefix.as_str()..)?.flatten() {
                let key = entry.0.value();
                if key.starts_with(&prefix) {
                    event_id += 1;
                }
            }
            event_id += 1;

            let event = EventRecord::new(
                &reason,
                &message,
                &source,
                resource.kind(),
                resource.name(),
                resource.namespace(),
                resource.uid(),
                event_type,
            );
            let mut event = event;
            event.event_id = event_id;

            // Write event
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(EVENTS)?;
                let key = event.key();
                let bytes = serde_json::to_vec(&event)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            write_txn.commit()?;

            Ok(event)
        }).await?
    }

    async fn get_events(&self, resource_uid: &str) -> Vec<EventRecord> {
        let db = self.db.clone();
        let prefix = format!("{}/", resource_uid);

        tokio::task::spawn_blocking(move || -> Vec<EventRecord> {
            let txn = match db.begin_read() {
                Ok(t) => t,
                Err(_) => return vec![],
            };
            let table = match txn.open_table(EVENTS) {
                Ok(t) => t,
                Err(_) => return vec![],
            };
            let mut events = Vec::new();
            if let Ok(iter) = table.range::<&str>(prefix.as_str()..) {
                for entry in iter.flatten() {
                    let key = entry.0.value();
                    if !key.starts_with(&prefix) {
                        break;
                    }
                    if let Ok(event) = serde_json::from_slice(entry.1.value()) {
                        events.push(event);
                    }
                }
            }
            events.reverse(); // newest first
            events
        }).await.unwrap_or_default()
    }

    async fn get_events_by_kind(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Vec<EventRecord> {
        let db = self.db.clone();
        let kind = kind.to_string();
        let namespace = namespace.map(|s| s.to_string());

        tokio::task::spawn_blocking(move || -> Vec<EventRecord> {
            let all = match Self::get_all_events_from_db(&db) {
                Ok(e) => e,
                Err(_) => return vec![],
            };
            all.into_iter()
                .filter(|e| e.resource_kind == kind)
                .filter(|e| namespace.as_deref().is_none_or(|ns| e.resource_namespace.as_deref() == Some(ns)))
                .collect()
        }).await.unwrap_or_default()
    }

    async fn get_recent_events(&self, limit: usize) -> Vec<EventRecord> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Vec<EventRecord> {
            let mut all = match Self::get_all_events_from_db(&db) {
                Ok(e) => e,
                Err(_) => return vec![],
            };
            all.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
            all.truncate(limit);
            all
        }).await.unwrap_or_default()
    }

    async fn prune_events(&self, keep_per_resource: usize) -> anyhow::Result<usize> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
            let txn = db.begin_read()?;
            let table = txn.open_table(EVENTS)?;

            // Group events by resource UID
            let mut by_resource: HashMap<String, Vec<(String, u64)>> = HashMap::new();
            if let Ok(iter) = table.iter() {
                for entry in iter.flatten() {
                    let key = entry.0.value().to_string();
                    if let Some(uid) = key.split('/').next()
                        && let Ok(event) = serde_json::from_slice::<EventRecord>(entry.1.value()) {
                            by_resource.entry(uid.to_string())
                                .or_default()
                                .push((key, event.event_id));
                        }
                }
            }

            // Find events to delete (keep newest N per resource)
            let mut to_delete = Vec::new();
            for (_uid, mut events) in by_resource {
                events.sort_by_key(|e| std::cmp::Reverse(e.1)); // newest first
                if events.len() > keep_per_resource {
                    for (key, _) in events.into_iter().skip(keep_per_resource) {
                        to_delete.push(key);
                    }
                }
            }

            let count = to_delete.len();
            if !to_delete.is_empty() {
                let write_txn = db.begin_write()?;
                {
                    let mut table = write_txn.open_table(EVENTS)?;
                    for key in &to_delete {
                        table.remove(key.as_str())?;
                    }
                }
                write_txn.commit()?;
            }

            Ok(count)
        }).await?
    }
}

// Helper method for RedbBackend to get all events
impl RedbBackend {
    fn get_all_events_from_db(db: &Database) -> anyhow::Result<Vec<EventRecord>> {
        let txn = db.begin_read()?;
        let table = txn.open_table(EVENTS)?;
        let mut events = Vec::new();
        if let Ok(iter) = table.iter() {
            for entry in iter {
                if let Ok(entry) = entry
                    && let Ok(event) = serde_json::from_slice(entry.1.value()) {
                        events.push(event);
                    }
            }
        }
        Ok(events)
    }
}
