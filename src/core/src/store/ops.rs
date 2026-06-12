//! # Store Operations and Events
//!
//! ## StoreOp
//!
//! Represents a single write operation to the store. Used for batch operations.
//!
//! ## StoreEvent
//!
//! Emitted by the store after every write. Subscribers receive these events
//! to trigger reactive behavior (watch, gossip, DNS cache update).
//!
//! ## StoreChange
//!
//! Indicates whether the write was a create or update.

use crate::types::{AnyResource, ResourceRecord, ResourceStatus};

/// A single write operation. Used for batch writes.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StoreOp {
    /// Create or update spec + optional assignment.
    WriteSpec {
        spec: AnyResource,
        assigned_node: Option<String>,
    },
    /// Update status only (reconciler output).
    WriteStatus {
        uid: String,
        status: ResourceStatus,
    },
    /// Update observed_generation.
    SetObserved {
        uid: String,
        generation: u64,
    },
    /// Delete resource.
    Delete {
        uid: String,
    },
}

/// An event emitted by the store after a write operation.
///
/// Subscribers (API watch, gossip, DNS cache) receive these events
/// to trigger reactive behavior.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StoreEvent {
    /// A resource was created or updated.
    Applied {
        record: ResourceRecord,
        change: StoreChange,
    },
    /// A resource was deleted.
    Deleted {
        uid: String,
        kind: String,
        namespace: Option<String>,
        name: String,
    },
}

impl StoreEvent {
    /// Get the resource kind.
    pub fn kind(&self) -> &str {
        match self {
            Self::Applied { record, .. } => record.kind(),
            Self::Deleted { kind, .. } => kind,
        }
    }

    /// Get the resource UID.
    pub fn uid(&self) -> &str {
        match self {
            Self::Applied { record, .. } => record.uid(),
            Self::Deleted { uid, .. } => uid,
        }
    }

    /// Get the resource namespace.
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Self::Applied { record, .. } => record.spec.namespace(),
            Self::Deleted { namespace, .. } => namespace.as_deref(),
        }
    }

    /// Get the resource name.
    pub fn name(&self) -> &str {
        match self {
            Self::Applied { record, .. } => record.name(),
            Self::Deleted { name, .. } => name,
        }
    }
}

/// Whether the write was a create or update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreChange {
    Created,
    Updated,
}
