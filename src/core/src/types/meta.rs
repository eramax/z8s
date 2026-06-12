//! # ObjectMeta — Resource Identity and Metadata
//!
//! Every Kubernetes-compatible resource has an `ObjectMeta` that identifies it.
//! This includes the resource's name, namespace, UID, labels, annotations,
//! and generation tracking.
//!
//! ## Key Fields
//!
//! - `uid` — Unique identifier (set once, never changes)
//! - `name` — User-chosen name (unique within namespace)
//! - `namespace` — Logical grouping (None for cluster-scoped resources)
//! - `labels` — Key-value pairs for selection and grouping
//! - `annotations` — Arbitrary metadata (not used for selection)
//! - `generation` — Monotonically increasing counter (incremented on spec writes)
//! - `creation_timestamp` — When this resource was created
//! - `deletion_timestamp` — When delete was requested (grace period starts)
//!
//! ## Generation vs Observed Generation
//!
//! `metadata.generation` is incremented by the API server whenever the spec changes.
//! The controller sets `observed_generation` (in ResourceRecord) to track what it
//! has reconciled. When `generation != observed_generation`, reconciliation is needed.

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

/// Wall-clock timestamp in RFC3339 format.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Time(pub String);

impl Time {
    /// Create a timestamp for the current moment.
    pub fn now() -> Self {
        Self(crate::types::helpers::now_rfc3339())
    }

    /// Parse the timestamp as seconds since epoch.
    pub fn as_secs(&self) -> Option<i64> {
        crate::types::helpers::parse_rfc3339_secs(&self.0)
    }
}

/// Labels are key-value pairs used for resource selection.
/// Stored as `BTreeMap` for deterministic serialization order.
pub type Labels = BTreeMap<String, String>;

/// Annotations are arbitrary key-value metadata.
/// Not used for selection, only for informational purposes.
pub type Annotations = BTreeMap<String, String>;

/// ObjectMeta contains metadata that every Kubernetes resource must have.
///
/// This is the identity of the resource — once created, the UID never changes.
/// The name must be unique within a namespace (or cluster-wide for cluster-scoped resources).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectMeta {
    /// Unique identifier for this resource. Set once by the API server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,

    /// User-chosen name. Must be unique within namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Logical namespace. None for cluster-scoped resources (Node, Namespace, PV).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Resource version for optimistic concurrency. Increments on every write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,

    /// Monotonically increasing generation. Increments on spec changes.
    /// The controller tracks this to know when reconciliation is needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<i64>,

    /// When this resource was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<Time>,

    /// When delete was requested. If set, the resource is being terminated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_timestamp: Option<Time>,

    /// Grace period after deletion_timestamp before force-delete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_grace_period_seconds: Option<i64>,

    /// Labels for resource selection and grouping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Labels>,

    /// Arbitrary metadata annotations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Finalizers prevent deletion until all controllers remove theirs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finalizers: Option<Vec<String>>,

    /// Owner references for garbage collection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_references: Option<Vec<OwnerReference>>,

    /// Managed fields for server-side apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_fields: Option<Vec<ManagedFieldsEntry>>,

    /// Self link (for API compatibility).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_link: Option<String>,

    /// Generate name prefix (for auto-generated names).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_name: Option<String>,
}

impl ObjectMeta {
    /// Create a new ObjectMeta with the given name and namespace.
    pub fn new(name: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            namespace: Some(namespace.into()),
            uid: Some(uuid()),
            creation_timestamp: Some(Time::now()),
            ..Default::default()
        }
    }

    /// Create a cluster-scoped ObjectMeta (no namespace).
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            uid: Some(uuid()),
            creation_timestamp: Some(Time::now()),
            ..Default::default()
        }
    }

    /// Set labels and return self for chaining.
    pub fn with_labels(mut self, labels: Labels) -> Self {
        self.labels = Some(labels);
        self
    }

    /// Set annotations and return self for chaining.
    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    /// Check if this resource is being deleted.
    pub fn is_deleting(&self) -> bool {
        self.deletion_timestamp.is_some()
    }

    /// Check if this resource has the given label.
    pub fn has_label(&self, key: &str, value: &str) -> bool {
        self.labels.as_ref()
            .map(|l| l.get(key).map(|v| v == value).unwrap_or(false))
            .unwrap_or(false)
    }

    /// Get a label value.
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.as_ref()?.get(key).map(|s| s.as_str())
    }

    /// Get an annotation value.
    pub fn annotation(&self, key: &str) -> Option<&str> {
        self.annotations.as_ref()?.get(key).map(|s| s.as_str())
    }
}

/// OwnerReference links to a resource that owns this one.
/// Used for garbage collection — when the owner is deleted, dependents are too.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OwnerReference {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub uid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_owner_deletion: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<bool>,
}

/// ManagedFieldsEntry tracks who last modified each field (for server-side apply).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedFieldsEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields_v1: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manager: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subresource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<Time>,
}

/// Generate a random UID. Uses a simple approach for now.
fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    format!("{:016x}-{:08x}-{:08x}-{:016x}",
        t.as_secs(),
        t.subsec_nanos(),
        std::process::id(),
        rand_u64())
}

/// Simple random u64 for UUID generation.
fn rand_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new();
    let mut h = s.build_hasher();
    h.write_u64(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64);
    h.finish()
}
