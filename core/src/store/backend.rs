//! # StoreBackend Trait — The Contract for All Storage Backends
//!
//! This trait defines the interface that all storage backends must implement.
//! The API server, controller, and scheduler all use this trait — they don't
//! know or care whether the backend is redb, memory, or something else.
//!
//! ## Write Permissions
//!
//! - `write_spec()` — API server (kubectl apply) and scheduler (assignment)
//! - `write_status()` — Controller only (after reconciliation)
//! - `assign_node()` — Scheduler only (leader election)
//! - `set_deletion_timestamp()` — API server (kubectl delete)
//!
//! ## Read Operations
//!
//! - `get()` — single resource by UID
//! - `get_all()` — all resources (for snapshot)
//! - `get_by_kind()` — resources by type ("Pod", "Service", etc.)
//! - `get_by_node()` — resources assigned to a specific node
//! - `get_unassigned()` — resources not yet assigned to any node
//! - `get_needing_reconcile()` — resources where generation > observed_generation
//!
//! ## Event Emission
//!
//! Every write operation emits a `StoreEvent` via the `StoreEventHub`.
//! Subscribers (API watch, gossip, DNS cache) receive these events reactively.

use async_trait::async_trait;

use crate::types::{AnyResource, EventRecord, EventType, ResourceRecord, ResourceStatus};
use super::{StoreOp, StoreSnapshot};

/// The core store trait. All storage operations go through this interface.
///
/// # Implementors
///
/// - [`RedbBackend`] — persistent storage using redb database
/// - [`MemoryBackend`] — in-memory storage for testing
#[async_trait]
pub trait StoreBackend: Send + Sync {
    // ── Spec writes (user / scheduler) ──────────────────────────

    /// Create or update a resource spec. Increments generation.
    ///
    /// Called by:
    /// - API server when user runs `kubectl apply`
    /// - Scheduler when assigning a pod to a node
    async fn write_spec(
        &self,
        spec: AnyResource,
        assigned_node: Option<String>,
    ) -> anyhow::Result<()>;

    /// Update only the assigned_node field. Increments generation.
    ///
    /// Called by: scheduler only (leader election).
    async fn assign_node(&self, uid: &str, node: &str) -> anyhow::Result<()>;

    /// Set deletion timestamp (start graceful shutdown).
    ///
    /// Called by: API server when user runs `kubectl delete`.
    async fn set_deletion_timestamp(&self, uid: &str) -> anyhow::Result<()>;

    // ── Status writes (reconciler only) ─────────────────────────

    /// Update status fields. Does NOT increment generation.
    ///
    /// Called by: controller on the owning node after reconciliation.
    async fn write_status(&self, uid: &str, status: ResourceStatus) -> anyhow::Result<()>;

    /// Update observed_generation after successful reconciliation.
    ///
    /// Called by: controller on the owning node.
    async fn set_observed_generation(&self, uid: &str, generation: u64) -> anyhow::Result<()>;

    // ── Reads ───────────────────────────────────────────────────

    /// Read full record (spec + status + metadata).
    async fn get(&self, uid: &str) -> Option<ResourceRecord>;

    /// Read all records.
    async fn get_all(&self) -> Vec<ResourceRecord>;

    /// Read records for a specific kind ("Pod", "Service", etc.).
    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceRecord>;

    /// Read records assigned to a specific node (for reconcile).
    async fn get_by_node(&self, node: &str) -> Vec<ResourceRecord>;

    /// Read unassigned records (for scheduler).
    async fn get_unassigned(&self, kind: &str) -> Vec<ResourceRecord>;

    /// Read records where generation > observed_generation (need reconcile).
    async fn get_needing_reconcile(&self, node: &str) -> Vec<ResourceRecord>;

    /// Delete a resource (hard delete from DB).
    async fn delete(&self, uid: &str) -> anyhow::Result<()>;

    // ── Events ──────────────────────────────────────────────────

    /// Record an event for a resource. Appends to events table.
    async fn record_event(
        &self,
        resource: &AnyResource,
        reason: &str,
        message: &str,
        source: &str,
        event_type: EventType,
    ) -> anyhow::Result<EventRecord>;

    /// Get all events for a resource (newest first).
    async fn get_events(&self, resource_uid: &str) -> Vec<EventRecord>;

    /// Get events for a resource kind in a namespace.
    async fn get_events_by_kind(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Vec<EventRecord>;

    /// Get recent events across all resources.
    async fn get_recent_events(&self, limit: usize) -> Vec<EventRecord>;

    /// Prune old events (keep last N per resource).
    async fn prune_events(&self, keep_per_resource: usize) -> anyhow::Result<usize>;

    // ── Batch (single transaction) ──────────────────────────────

    /// Batch write — all ops in one transaction.
    async fn apply_batch(&self, ops: Vec<StoreOp>) -> anyhow::Result<()>;

    /// Snapshot for reconcile loop (one get_all call).
    async fn snapshot(&self) -> StoreSnapshot;
}
