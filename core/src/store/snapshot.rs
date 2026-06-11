//! # StoreSnapshot — Point-in-Time View of the Store
//!
//! A snapshot is a single `get_all()` call that captures the entire store state.
//! Used by the controller and reconciler to make decisions without repeated DB reads.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let snap = store.snapshot().await;
//!
//! // Get all pods
//! let pods = snap.by_kind("Pod");
//!
//! // Get pods assigned to a node
//! let mine = snap.by_node("node-1");
//!
//! // Get unassigned pods
//! let unassigned = snap.unassigned("Pod");
//! ```

use std::collections::HashMap;

use crate::types::ResourceRecord;

/// Point-in-time view of the store for a single reconcile pass.
#[derive(Debug, Clone, Default)]
pub struct StoreSnapshot {
    /// All records indexed by UID.
    by_uid: HashMap<String, ResourceRecord>,
    /// All records (for iteration).
    records: Vec<ResourceRecord>,
}

impl StoreSnapshot {
    /// Create a snapshot from a list of records.
    pub fn from_records(records: Vec<ResourceRecord>) -> Self {
        let by_uid = records.iter()
            .map(|r| (r.uid().to_string(), r.clone()))
            .collect();
        Self { by_uid, records }
    }

    /// Create an empty snapshot.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Get the number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Check if the snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Get all records.
    pub fn all(&self) -> &[ResourceRecord] {
        &self.records
    }

    /// Get a record by UID.
    pub fn get(&self, uid: &str) -> Option<&ResourceRecord> {
        self.by_uid.get(uid)
    }

    /// Get records by kind.
    pub fn by_kind(&self, kind: &str) -> Vec<&ResourceRecord> {
        self.records.iter()
            .filter(|r| r.kind() == kind)
            .collect()
    }

    /// Get records assigned to a specific node.
    pub fn by_node(&self, node: &str) -> Vec<&ResourceRecord> {
        self.records.iter()
            .filter(|r| r.assigned_node.as_deref() == Some(node))
            .collect()
    }

    /// Get unassigned records of a specific kind.
    pub fn unassigned(&self, kind: &str) -> Vec<&ResourceRecord> {
        self.records.iter()
            .filter(|r| r.kind() == kind && r.assigned_node.is_none())
            .collect()
    }

    /// Get records needing reconciliation (generation > observed_generation).
    pub fn needing_reconcile(&self, node: &str) -> Vec<&ResourceRecord> {
        self.records.iter()
            .filter(|r| r.assigned_node.as_deref() == Some(node) && r.needs_reconcile())
            .collect()
    }
}
