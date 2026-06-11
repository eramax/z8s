//! # Event Record — Audit Trail for Resource Changes
//!
//! Every action on a resource creates an event. Events are append-only,
//! stored in the DB, and queryable via the API.
//!
//! ## Event Lifecycle
//!
//! ```text
//! User applies Pod
//!     → Event: "Created" (source: api)
//!
//! Scheduler assigns Pod
//!     → Event: "Scheduled" (source: scheduler)
//!
//! Controller starts Pod
//!     → Event: "Pulling" (source: controller)
//!     → Event: "Pulled" (source: controller)
//!     → Event: "Started" (source: controller)
//!
//! User deletes Pod
//!     → Event: "Killing" (source: api)
//!     → Event: "Deleted" (source: controller)
//! ```
//!
//! ## How to Query Events
//!
//! ```rust,ignore
//! // Get all events for a Pod
//! let events = store.get_events_by_uid("pod-uid-123").await;
//!
//! // Get events by kind and namespace
//! let events = store.get_events_by_kind("Pod", Some("default")).await;
//!
//! // Get recent events across all resources
//! let events = store.get_recent_events(100).await;
//! ```
//!
//! ## Event Deduplication
//!
//! Events are deduplicated by (resource_uid, reason, message). If the same
//! event happens again within 1 minute, the count is incremented instead of
//! creating a new event.

use serde::{Deserialize, Serialize};

/// A single event record — one thing that happened to a resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    /// Unique event ID (monotonic per resource).
    pub event_id: u64,

    /// What happened: "Scheduled", "Pulled", "Created", "Started", "Killing", etc.
    pub reason: String,

    /// Human-readable message: "Successfully assigned nginx to node-1"
    pub message: String,

    /// When this happened (epoch ms).
    pub timestamp: i64,

    /// Who/what caused this: "controller", "scheduler", "api", "kubelet"
    pub source: String,

    /// Resource kind: "Pod", "Service", "Deployment"
    pub resource_kind: String,

    /// Resource name: "nginx"
    pub resource_name: String,

    /// Resource namespace: "default" (None for cluster-scoped)
    pub resource_namespace: Option<String>,

    /// Resource UID (links to ResourceRecord).
    pub resource_uid: String,

    /// Event type: "Normal" or "Warning"
    pub event_type: EventType,

    /// Count of repeated events (for dedup).
    pub count: u32,
}

impl EventRecord {
    /// Create a new event record.
    pub fn new(
        reason: &str,
        message: &str,
        source: &str,
        resource_kind: &str,
        resource_name: &str,
        resource_namespace: Option<&str>,
        resource_uid: &str,
        event_type: EventType,
    ) -> Self {
        Self {
            event_id: 0, // Set by store
            reason: reason.to_string(),
            message: message.to_string(),
            timestamp: crate::types::helpers::now_epoch_ms(),
            source: source.to_string(),
            resource_kind: resource_kind.to_string(),
            resource_name: resource_name.to_string(),
            resource_namespace: resource_namespace.map(|s| s.to_string()),
            resource_uid: resource_uid.to_string(),
            event_type,
            count: 1,
        }
    }

    /// Get the DB key for this event.
    pub fn key(&self) -> String {
        format!("{}/{}", self.resource_uid, self.event_id)
    }

    /// Get dedup key (for checking if same event already exists).
    pub fn dedup_key(&self) -> String {
        format!("{}:{}:{}", self.resource_uid, self.reason, self.message)
    }
}

/// Event type — normal or warning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum EventType {
    Normal,
    Warning,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "Normal"),
            Self::Warning => write!(f, "Warning"),
        }
    }
}

/// Common event reasons used across the codebase.
pub mod reasons {
    pub const SCHEDULED: &str = "Scheduled";
    pub const PULLING: &str = "Pulling";
    pub const PULLED: &str = "Pulled";
    pub const CREATED: &str = "Created";
    pub const STARTED: &str = "Started";
    pub const KILLING: &str = "Killing";
    pub const FAILED: &str = "Failed";
    pub const FAILED_SCHEDULING: &str = "FailedScheduling";
    pub const SCALING_UP: &str = "ScalingUp";
    pub const SCALING_DOWN: &str = "ScalingDown";
    pub const RECONCILING: &str = "Reconciling";
    pub const ALLOCATED_IP: &str = "AllocatedIP";
    pub const CREATED_VETH: &str = "CreatedVeth";
    pub const SERVICE_UPDATE: &str = "ServiceUpdate";
}
