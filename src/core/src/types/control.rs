//! # Control Resources — Namespace, Node, Event, ServiceAccount, RBAC
//!
//! ## Namespace
//!
//! Logical grouping of resources. Pods, Services, etc. exist within a Namespace.
//!
//! ## Node
//!
//! A physical or virtual machine in the cluster. Reports capacity and allocatable resources.
//!
//! ## Event
//!
//! Records something that happened to a resource. Used for debugging and auditing.
//!
//! ## ServiceAccount
//!
//! Identity for Pods. Controls what API actions the Pod can perform.
//!
//! ## Role / RoleBinding
//!
//! RBAC: Role defines permissions, RoleBinding grants them to a ServiceAccount.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::meta::{ObjectMeta, Time};

// ── Namespace ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Namespace {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<NamespaceSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NamespaceStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NamespaceSpec {
    pub finalizers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NamespaceStatus {
    pub phase: Option<String>,
}

// ── Node ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<NodeSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NodeStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeSpec {
    pub pod_cidr: Option<String>,
    pub pod_cidrs: Option<Vec<String>>,
    pub provider_id: Option<String>,
    pub taints: Option<Vec<Taint>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Taint {
    pub effect: String,
    pub key: String,
    pub value: Option<String>,
    pub time_added: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub capacity: Option<BTreeMap<String, String>>,
    pub allocatable: Option<BTreeMap<String, String>>,
    pub conditions: Option<Vec<NodeCondition>>,
    pub addresses: Option<Vec<NodeAddress>>,
    pub daemon_endpoints: Option<NodeDaemonEndpoints>,
    pub node_info: Option<NodeSystemInfo>,
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeCondition {
    pub status: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub last_heartbeat_time: Option<Time>,
    pub last_transition_time: Option<Time>,
    pub message: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeAddress {
    pub address: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NodeDaemonEndpoints {
    pub kubelet_endpoint: Option<KubeletEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct KubeletEndpoint {
    pub port: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeSystemInfo {
    pub kubelet_version: Option<String>,
    pub os_image: Option<String>,
    pub kernel_version: Option<String>,
    pub container_runtime_version: Option<String>,
    pub architecture: Option<String>,
}

// ── Event ─────────────────────────────────────────────────────────────────

/// Legacy event type (for API compatibility). New events use EventRecord in the store.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub involved_object: Option<ObjectReference>,
    pub reason: String,
    pub message: String,
    pub source: EventSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_timestamp: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_timestamp: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectReference {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub uid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct EventSource {
    pub component: Option<String>,
    pub host: Option<String>,
}

// ── ServiceAccount ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAccount {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<ObjectReference>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_pull_secrets: Option<Vec<ObjectReference>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automount_service_account_token: Option<bool>,
}

// ── RBAC ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Role {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub rules: Vec<PolicyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    pub api_groups: Option<Vec<String>>,
    pub resources: Option<Vec<String>>,
    pub verbs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RoleBinding {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub subjects: Option<Vec<Subject>>,
    pub role_ref: Option<RoleRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Subject {
    #[serde(rename = "kind")]
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RoleRef {
    pub api_group: String,
    #[serde(rename = "kind")]
    pub kind: String,
    pub name: String,
}
