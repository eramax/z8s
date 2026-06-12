//! # Resource Trait and Type System
//!
//! This is the heart of the type system. Every resource type implements
//! [`Resource`], which provides a uniform interface for:
//!
//! - Metadata access (`metadata()`, `kind()`, `uid()`)
//! - Type-erased conversion (`into_any()`, `from_any()`)
//! - Database storage and gossip
//!
//! ## Key Types
//!
//! - [`Resource`] — trait that all resource types implement
//! - [`AnyResource`] — type-erased enum wrapping all concrete types
//! - [`ResourceRecord`] — full DB record: spec + status + generation
//! - [`ResourceStatus`] — current state computed by the controller
//!
//! ## How It Works
//!
//! ```text
//! User writes:  Pod { metadata: ..., spec: { containers: [...] } }
//!                                  ↓ into_any()
//! AnyResource::Pod(pod)
//!                                  ↓ store.write_spec()
//! ResourceRecord {
//!     spec: AnyResource::Pod(pod),
//!     status: ResourceStatus { phase: Pending, ... },
//!     generation: 1,
//!     observed_generation: 0,
//!     assigned_node: None,
//! }
//!                                  ↓ controller reads
//! reconcile(record)  // record.generation != record.observed_generation
//!                                  ↓ runtime.start_pod()
//! ResourceStatus { phase: Running, pod_ip: "10.42.0.5", ... }
//!                                  ↓ store.write_status()
//! ResourceRecord { ... observed_generation: 1, status: { phase: Running } }
//! ```

use serde::{Deserialize, Serialize};

use super::meta::ObjectMeta;

// ── Resource Trait ────────────────────────────────────────────────────────

/// The core trait that all resource types must implement.
///
/// This trait provides:
/// - Static metadata (kind, api_version, namespaced)
/// - Dynamic metadata access (metadata(), name(), uid(), namespace())
/// - Type-erased conversion (into_any(), from_any())
///
/// # Example
///
/// ```rust,ignore
/// impl Resource for Pod {
///     fn kind() -> &'static str { "Pod" }
///     fn api_version() -> &'static str { "v1" }
///     fn namespaced() -> bool { true }
///     fn metadata(&self) -> &ObjectMeta { &self.metadata }
///     fn metadata_mut(&mut self) -> &mut ObjectMeta { &mut self.metadata }
///     fn into_any(self) -> AnyResource { AnyResource::Pod(self) }
///     fn from_any(any: AnyResource) -> Option<Self> {
///         match any { AnyResource::Pod(p) => Some(p), _ => None }
///     }
/// }
/// ```
pub trait Resource: Clone + Send + Sync + 'static {
    /// The resource kind: "Pod", "Service", "Deployment", etc.
    fn kind() -> &'static str;

    /// The API version: "v1", "apps/v1", "z8s.io/v1", etc.
    fn api_version() -> &'static str;

    /// Whether this resource is namespace-scoped (true) or cluster-scoped (false).
    fn namespaced() -> bool;

    /// Short name for kubectl: "po", "svc", "no", etc.
    fn short_name() -> &'static str { "" }

    /// Verbs this resource supports: "get", "list", "create", "delete", etc.
    fn verbs() -> &'static [&'static str] { &["get", "list", "watch", "create", "delete"] }

    /// Get immutable reference to metadata.
    fn metadata(&self) -> &ObjectMeta;

    /// Get mutable reference to metadata.
    fn metadata_mut(&mut self) -> &mut ObjectMeta;

    /// Get the namespace (None for cluster-scoped resources).
    fn namespace(&self) -> Option<&str> {
        self.metadata().namespace.as_deref()
    }

    /// Get the resource name.
    fn name(&self) -> &str {
        self.metadata().name.as_deref().unwrap_or("")
    }

    /// Get the unique identifier.
    fn uid(&self) -> &str {
        self.metadata().uid.as_deref().unwrap_or("")
    }

    /// Get the generation (monotonic counter on spec changes).
    fn generation(&self) -> i64 {
        self.metadata().generation.unwrap_or(1)
    }

    /// Check if this resource has the given label.
    fn has_label(&self, key: &str, value: &str) -> bool {
        self.metadata().has_label(key, value)
    }

    /// Convert to type-erased AnyResource.
    fn into_any(self) -> AnyResource;

    /// Try to convert from AnyResource back to concrete type.
    fn from_any(any: AnyResource) -> Option<Self>;
}

// ── AnyResource ───────────────────────────────────────────────────────────

/// Type-erased enum wrapping all concrete resource types.
///
/// This allows storing different resource types in the same collection
/// (e.g., Vec<AnyResource> in the DB). Pattern match to get the concrete type.
///
/// # Example
///
/// ```rust,ignore
/// match resource {
///     AnyResource::Pod(pod) => { /* handle pod */ }
///     AnyResource::Service(svc) => { /* handle service */ }
///     _ => { /* unknown type */ }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum AnyResource {
    Pod(super::compute::Pod),
    Deployment(super::compute::Deployment),
    Service(super::network::Service),
    EndpointSlice(super::network::EndpointSlice),
    ConfigMap(super::storage::ConfigMap),
    Secret(super::storage::Secret),
    PersistentVolume(super::storage::PersistentVolume),
    PersistentVolumeClaim(super::storage::PersistentVolumeClaim),
    StorageClass(super::storage::StorageClass),
    Namespace(super::control::Namespace),
    Node(super::control::Node),
    Event(super::control::Event),
    ServiceAccount(super::control::ServiceAccount),
    Role(super::control::Role),
    RoleBinding(super::control::RoleBinding),
    VNet(super::network::VNet),
    Subnet(super::network::Subnet),
    NetworkPolicy(super::network::NetworkPolicy),
    Ingress(super::network::Ingress),
    Nsg(super::network::Nsg),
    RouteTable(super::network::RouteTable),
}

impl AnyResource {
    /// Get the resource kind as a string.
    pub fn kind(&self) -> &str {
        match self {
            Self::Pod(_) => "Pod",
            Self::Deployment(_) => "Deployment",
            Self::Service(_) => "Service",
            Self::EndpointSlice(_) => "EndpointSlice",
            Self::ConfigMap(_) => "ConfigMap",
            Self::Secret(_) => "Secret",
            Self::PersistentVolume(_) => "PersistentVolume",
            Self::PersistentVolumeClaim(_) => "PersistentVolumeClaim",
            Self::StorageClass(_) => "StorageClass",
            Self::Namespace(_) => "Namespace",
            Self::Node(_) => "Node",
            Self::Event(_) => "Event",
            Self::ServiceAccount(_) => "ServiceAccount",
            Self::Role(_) => "Role",
            Self::RoleBinding(_) => "RoleBinding",
            Self::VNet(_) => "VNet",
            Self::Subnet(_) => "Subnet",
            Self::NetworkPolicy(_) => "NetworkPolicy",
            Self::Ingress(_) => "Ingress",
            Self::Nsg(_) => "Nsg",
            Self::RouteTable(_) => "RouteTable",
        }
    }

    /// Get immutable reference to metadata (works for any variant).
    pub fn metadata(&self) -> &ObjectMeta {
        match self {
            Self::Pod(r) => &r.metadata,
            Self::Deployment(r) => &r.metadata,
            Self::Service(r) => &r.metadata,
            Self::EndpointSlice(r) => &r.metadata,
            Self::ConfigMap(r) => &r.metadata,
            Self::Secret(r) => &r.metadata,
            Self::PersistentVolume(r) => &r.metadata,
            Self::PersistentVolumeClaim(r) => &r.metadata,
            Self::StorageClass(r) => &r.metadata,
            Self::Namespace(r) => &r.metadata,
            Self::Node(r) => &r.metadata,
            Self::Event(r) => &r.metadata,
            Self::ServiceAccount(r) => &r.metadata,
            Self::Role(r) => &r.metadata,
            Self::RoleBinding(r) => &r.metadata,
            Self::VNet(r) => &r.metadata,
            Self::Subnet(r) => &r.metadata,
            Self::NetworkPolicy(r) => &r.metadata,
            Self::Ingress(r) => &r.metadata,
            Self::Nsg(r) => &r.metadata,
            Self::RouteTable(r) => &r.metadata,
        }
    }

    /// Get mutable reference to metadata (works for any variant).
    pub fn metadata_mut(&mut self) -> &mut ObjectMeta {
        match self {
            Self::Pod(r) => &mut r.metadata,
            Self::Deployment(r) => &mut r.metadata,
            Self::Service(r) => &mut r.metadata,
            Self::EndpointSlice(r) => &mut r.metadata,
            Self::ConfigMap(r) => &mut r.metadata,
            Self::Secret(r) => &mut r.metadata,
            Self::PersistentVolume(r) => &mut r.metadata,
            Self::PersistentVolumeClaim(r) => &mut r.metadata,
            Self::StorageClass(r) => &mut r.metadata,
            Self::Namespace(r) => &mut r.metadata,
            Self::Node(r) => &mut r.metadata,
            Self::Event(r) => &mut r.metadata,
            Self::ServiceAccount(r) => &mut r.metadata,
            Self::Role(r) => &mut r.metadata,
            Self::RoleBinding(r) => &mut r.metadata,
            Self::VNet(r) => &mut r.metadata,
            Self::Subnet(r) => &mut r.metadata,
            Self::NetworkPolicy(r) => &mut r.metadata,
            Self::Ingress(r) => &mut r.metadata,
            Self::Nsg(r) => &mut r.metadata,
            Self::RouteTable(r) => &mut r.metadata,
        }
    }

    /// Get the resource name.
    pub fn name(&self) -> &str {
        self.metadata().name.as_deref().unwrap_or("")
    }

    /// Get the resource UID.
    pub fn uid(&self) -> &str {
        self.metadata().uid.as_deref().unwrap_or("")
    }

    /// Get the namespace (None for cluster-scoped resources).
    pub fn namespace(&self) -> Option<&str> {
        self.metadata().namespace.as_deref()
    }

    /// Check if this resource is being deleted.
    pub fn is_deleting(&self) -> bool {
        self.metadata().is_deleting()
    }
}

// ── ResourceRecord ────────────────────────────────────────────────────────

/// The full record stored in the database for each resource.
///
/// Contains three parts:
/// - `spec` — desired state (what the user wants)
/// - `status` — current state (what's actually running)
/// - `generation` — monotonic counter for reconciliation
///
/// # Reconciliation Contract
///
/// ```text
/// spec.generation != record.observed_generation  →  needs reconciliation
/// spec.generation == record.observed_generation  →  up to date
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceRecord {
    /// The desired state (what the user wants). Contains metadata + spec.
    pub spec: AnyResource,

    /// The current state (what's actually running). Computed by controller.
    pub status: ResourceStatus,

    /// Monotonically increasing counter. Incremented on every spec change.
    pub generation: u64,

    /// What the reconciler has processed. If != generation → needs reconciliation.
    pub observed_generation: u64,

    /// Which node should run this resource. Set by scheduler.
    pub assigned_node: Option<String>,

    /// Last time this record was updated (epoch ms).
    pub last_updated: i64,
}

impl ResourceRecord {
    /// Create a new record with generation=1, observed_generation=0.
    pub fn new(spec: AnyResource) -> Self {
        Self {
            spec,
            status: ResourceStatus::default(),
            generation: 1,
            observed_generation: 0,
            assigned_node: None,
            last_updated: crate::types::helpers::now_epoch_ms(),
        }
    }

    /// Check if this record needs reconciliation.
    pub fn needs_reconcile(&self) -> bool {
        self.generation > self.observed_generation
    }

    /// Get the resource UID.
    pub fn uid(&self) -> &str {
        self.spec.uid()
    }

    /// Get the resource kind.
    pub fn kind(&self) -> &str {
        self.spec.kind()
    }

    /// Get the resource name.
    pub fn name(&self) -> &str {
        self.spec.name()
    }
}

// ── ResourceStatus ────────────────────────────────────────────────────────

/// Current state of a resource — computed by the controller, persisted to DB.
///
/// The controller updates this after each reconciliation cycle.
/// The API server reads this to show current state to kubectl.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ResourceStatus {
    /// Current phase: Pending, Running, Succeeded, Failed, Unknown.
    pub phase: Phase,

    /// Human-readable message (e.g., "Waiting for image pull").
    pub message: Option<String>,

    /// Conditions (like k8s conditions). Latest by type.
    pub conditions: Vec<Condition>,

    /// Pod IP address (if running with network isolation).
    pub pod_ip: Option<String>,

    /// Node IP address (if assigned to a node).
    pub host_ip: Option<String>,

    /// Per-container statuses.
    pub container_statuses: Vec<ContainerStatus>,

    /// When the resource started running.
    pub started_at: Option<String>,

    /// When the resource finished (for succeeded/failed).
    pub finished_at: Option<String>,

    /// Total restart count across all containers.
    pub restart_count: u32,
}

impl ResourceStatus {
    /// Check if the resource is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, Phase::Succeeded | Phase::Failed)
    }

    /// Check if all containers are ready.
    pub fn all_ready(&self) -> bool {
        self.container_statuses.iter().all(|c| c.ready)
    }

    /// Get the ready container count as a fraction string.
    pub fn ready_string(&self) -> String {
        let total = self.container_statuses.len();
        let ready = self.container_statuses.iter().filter(|c| c.ready).count();
        format!("{}/{}", ready, total)
    }

    /// Find or create a condition by type.
    pub fn condition(&self, type_: &str) -> Option<&Condition> {
        self.conditions.iter().find(|c| c.type_ == type_)
    }

    /// Set or update a condition by type.
    pub fn set_condition(&mut self, type_: &str, status: &str, reason: &str, message: &str) {
        if let Some(c) = self.conditions.iter_mut().find(|c| c.type_ == type_) {
            c.status = status.to_string();
            c.reason = Some(reason.to_string());
            c.message = Some(message.to_string());
            c.last_transition_time = Some(crate::types::helpers::now_rfc3339());
        } else {
            self.conditions.push(Condition {
                type_: type_.to_string(),
                status: status.to_string(),
                reason: Some(reason.to_string()),
                message: Some(message.to_string()),
                last_transition_time: Some(crate::types::helpers::now_rfc3339()),
            });
        }
    }
}

// ── Phase ─────────────────────────────────────────────────────────────────

/// Resource phase — the high-level state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum Phase {
    /// Resource is created but not yet running (waiting for scheduling, image pull, etc.).
    #[default]
    Pending,

    /// Resource is running.
    Running,

    /// Resource completed successfully (for Jobs, one-off tasks).
    Succeeded,

    /// Resource failed (exit code != 0, crash loop, etc.).
    Failed,

    /// State cannot be determined.
    Unknown,
}

impl Phase {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Unknown => "Unknown",
        }
    }
}

// ── Condition ─────────────────────────────────────────────────────────────

/// A condition describes the current state of a resource aspect.
///
/// Conditions are additive — they accumulate over time.
/// The latest condition of each type is what matters.
///
/// # Standard Conditions (Pods)
///
/// - `Ready` — all containers are ready
/// - `PodScheduled` — pod has been assigned to a node
/// - `Initialized` — init containers have completed
/// - `ContainersReady` — all containers are ready
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Condition {
    /// Condition type: "Ready", "PodScheduled", "Initialized", etc.
    #[serde(rename = "type")]
    pub type_: String,

    /// Condition status: "True", "False", "Unknown".
    pub status: String,

    /// Why this condition changed: "ContainersReady", "ImagePullBackOff", etc.
    pub reason: Option<String>,

    /// Human-readable detail.
    pub message: Option<String>,

    /// When this condition last changed.
    pub last_transition_time: Option<String>,
}

// ── ContainerStatus ───────────────────────────────────────────────────────

/// Status of a single container.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct ContainerStatus {
    /// Container name.
    pub name: String,

    /// Whether the container is ready to accept traffic.
    pub ready: bool,

    /// Number of times this container has been restarted.
    pub restart_count: u32,

    /// Image reference.
    pub image: String,

    /// Container ID (runtime-specific).
    pub container_id: Option<String>,

    /// Current container state (waiting, running, or terminated).
    pub state: ContainerState,
}


// ── ContainerState ────────────────────────────────────────────────────────

/// Current state of a container — exactly one variant is active.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ContainerState {
    pub waiting: Option<ContainerStateWaiting>,
    pub running: Option<ContainerStateRunning>,
    pub terminated: Option<ContainerStateTerminated>,
}

impl ContainerState {
    /// Create a waiting state.
    pub fn waiting(reason: &str, message: Option<&str>) -> Self {
        Self {
            waiting: Some(ContainerStateWaiting {
                reason: reason.to_string(),
                message: message.map(|s| s.to_string()),
            }),
            running: None,
            terminated: None,
        }
    }

    /// Create a running state.
    pub fn running(started_at: &str) -> Self {
        Self {
            waiting: None,
            running: Some(ContainerStateRunning {
                started_at: started_at.to_string(),
            }),
            terminated: None,
        }
    }

    /// Create a terminated state.
    pub fn terminated(exit_code: i32, reason: &str, message: Option<&str>) -> Self {
        Self {
            waiting: None,
            running: None,
            terminated: Some(ContainerStateTerminated {
                exit_code,
                reason: reason.to_string(),
                message: message.map(|s| s.to_string()),
                started_at: None,
                finished_at: Some(crate::types::helpers::now_rfc3339()),
            }),
        }
    }
}

/// Container is waiting (image pull, crash loop backoff, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContainerStateWaiting {
    pub reason: String,
    pub message: Option<String>,
}

/// Container is running.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContainerStateRunning {
    pub started_at: String,
}

/// Container has terminated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContainerStateTerminated {
    pub exit_code: i32,
    pub reason: String,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

// ── Implementations ───────────────────────────────────────────────────────

impl Resource for super::compute::Pod {
    fn kind() -> &'static str { "Pod" }
    fn api_version() -> &'static str { "v1" }
    fn namespaced() -> bool { true }
    fn short_name() -> &'static str { "po" }
    fn metadata(&self) -> &ObjectMeta { &self.metadata }
    fn metadata_mut(&mut self) -> &mut ObjectMeta { &mut self.metadata }
    fn into_any(self) -> AnyResource { AnyResource::Pod(self) }
    fn from_any(any: AnyResource) -> Option<Self> {
        match any { AnyResource::Pod(p) => Some(p), _ => None }
    }
}

impl Resource for super::compute::Deployment {
    fn kind() -> &'static str { "Deployment" }
    fn api_version() -> &'static str { "apps/v1" }
    fn namespaced() -> bool { true }
    fn short_name() -> &'static str { "deploy" }
    fn metadata(&self) -> &ObjectMeta { &self.metadata }
    fn metadata_mut(&mut self) -> &mut ObjectMeta { &mut self.metadata }
    fn into_any(self) -> AnyResource { AnyResource::Deployment(self) }
    fn from_any(any: AnyResource) -> Option<Self> {
        match any { AnyResource::Deployment(d) => Some(d), _ => None }
    }
}

impl Resource for super::network::Service {
    fn kind() -> &'static str { "Service" }
    fn api_version() -> &'static str { "v1" }
    fn namespaced() -> bool { true }
    fn short_name() -> &'static str { "svc" }
    fn metadata(&self) -> &ObjectMeta { &self.metadata }
    fn metadata_mut(&mut self) -> &mut ObjectMeta { &mut self.metadata }
    fn into_any(self) -> AnyResource { AnyResource::Service(self) }
    fn from_any(any: AnyResource) -> Option<Self> {
        match any { AnyResource::Service(s) => Some(s), _ => None }
    }
}

impl Resource for super::control::Namespace {
    fn kind() -> &'static str { "Namespace" }
    fn api_version() -> &'static str { "v1" }
    fn namespaced() -> bool { false }
    fn short_name() -> &'static str { "ns" }
    fn metadata(&self) -> &ObjectMeta { &self.metadata }
    fn metadata_mut(&mut self) -> &mut ObjectMeta { &mut self.metadata }
    fn into_any(self) -> AnyResource { AnyResource::Namespace(self) }
    fn from_any(any: AnyResource) -> Option<Self> {
        match any { AnyResource::Namespace(n) => Some(n), _ => None }
    }
}

impl Resource for super::control::Node {
    fn kind() -> &'static str { "Node" }
    fn api_version() -> &'static str { "v1" }
    fn namespaced() -> bool { false }
    fn short_name() -> &'static str { "no" }
    fn metadata(&self) -> &ObjectMeta { &self.metadata }
    fn metadata_mut(&mut self) -> &mut ObjectMeta { &mut self.metadata }
    fn into_any(self) -> AnyResource { AnyResource::Node(self) }
    fn from_any(any: AnyResource) -> Option<Self> {
        match any { AnyResource::Node(n) => Some(n), _ => None }
    }
}
