//! # z8s Types — Kubernetes-Compatible Resource Types
//!
//! This module defines all resource types used by z8s. Every type implements
//! the [`resource::Resource`] trait, which provides a uniform interface for metadata access,
//! serialization, and type-erased conversion via [`resource::AnyResource`].
//!
//! ## Design Principles
//!
//! 1. **Every resource is a `Resource`** — the trait provides `kind()`, `metadata()`,
//!    `into_any()`, `from_any()` for uniform handling without match arms.
//!
//! 2. **Spec vs Status separation** — resources carry desired state in `spec` and
//!    computed state in `status`. The controller reconciles spec → status.
//!
//! 3. **Generation tracking** — `metadata.generation` increments on spec writes.
//!    `observed_generation` tracks what the reconciler has processed.
//!
//! 4. **Serialization-first** — every type derives `Serialize`/`Deserialize` for
//!    DB storage, gossip, and API responses.
//!
//! ## Module Structure
//!
//! - `meta` — ObjectMeta, Time, Labels, Annotations
//! - `resource` — Resource trait, AnyResource enum, ResourceRecord, ResourceStatus
//! - `compute` — Pod, Deployment, ContainerSpec, Probes
//! - `network` — Service, EndpointSlice, VNet, Subnet, NSG
//! - `storage` — PV, PVC, StorageClass, ConfigMap, Secret
//! - `control` — Namespace, Node, Event, ServiceAccount
//! - `event` — EventRecord (audit trail for all resource changes)
//! - `helpers` — Quantity, IntOrString

pub mod meta;
pub mod resource;
pub mod compute;
pub mod network;
pub mod storage;
pub mod control;
pub mod event;
pub mod helpers;

// Re-export with explicit names to avoid ambiguous glob re-exports
pub use meta::{ObjectMeta, Time, OwnerReference, ManagedFieldsEntry};
pub use resource::{Resource, AnyResource, ResourceRecord, ResourceStatus, Phase, Condition, ContainerStatus, ContainerState, ContainerStateWaiting, ContainerStateRunning, ContainerStateTerminated};
pub use compute::{Pod, PodSpec, ContainerSpec, Deployment, DeploymentSpec, Volume, VolumeMount, ResourceRequirements, Probe, ExecAction, HTTPGetAction, TCPSocketAction, SecurityContext, Capabilities, PodSecurityContext, EnvVar, EnvVarSource, ConfigMapKeySelector, SecretKeySelector, EnvFromSource, ConfigMapEnvSource, SecretEnvSource, ContainerPort, EmptyDirVolumeSource, HostPathVolumeSource, ConfigMapVolumeSource, SecretVolumeSource, LabelSelector, LabelSelectorRequirement, PodTemplateSpec, DeploymentStrategy, RollingUpdateDeployment, DeploymentStatus, DeploymentCondition};
pub use network::{Service, ServiceSpec, ServicePort, ServiceStatus, LoadBalancerStatus, EndpointSlice, Endpoint, EndpointConditions, EndpointPort, ObjectReference, VNet, VNetSpec, VNetStatus, Subnet, SubnetSpec, Nsg, NsgSpec, NsgRule, NetworkPolicy, NetworkPolicySpec, NetworkPolicyIngressRule, NetworkPolicyEgressRule, Ingress, IngressSpec, IngressStatus, RouteTable, RouteTableSpec, Route};
pub use storage::{PersistentVolume, PersistentVolumeSpec, PersistentVolumeStatus, HostPathVolumeSource as StorageHostPathVolumeSource, PersistentVolumeClaim, PersistentVolumeClaimSpec, PersistentVolumeClaimStatus, StorageClass, ConfigMap, Secret};
pub use control::{Namespace, NamespaceSpec, NamespaceStatus, Node, NodeSpec, Taint, NodeStatus, NodeCondition, NodeAddress, NodeDaemonEndpoints, KubeletEndpoint, NodeSystemInfo, Event, EventSource, ServiceAccount, Role, PolicyRule, RoleBinding, Subject, RoleRef};
pub use event::{EventRecord, EventType, reasons};
pub use helpers::{Quantity, IntOrString};
