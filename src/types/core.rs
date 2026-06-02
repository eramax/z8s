//! Namespace, Node, and Event types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{ObjectMeta, ObjectReference, Quantity, Time};

fn default_namespace_api_version() -> String { "v1".to_string() }
fn default_namespace_kind() -> String { "Namespace".to_string() }
fn default_node_api_version() -> String { "v1".to_string() }
fn default_node_kind() -> String { "Node".to_string() }
fn default_event_api_version() -> String { "v1".to_string() }
fn default_event_kind() -> String { "Event".to_string() }

// ── Namespace ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finalizers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Namespace {
    #[serde(rename = "apiVersion", default = "default_namespace_api_version")]
    pub api_version: String,
    #[serde(default = "default_namespace_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<NamespaceSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NamespaceStatus>,
}

// ── Node ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "podCIDR")]
    pub pod_cidr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "podCIDRs")]
    pub pod_cidrs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taints: Option<Vec<Taint>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Taint {
    pub effect: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_added: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeAddress {
    pub address: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeCondition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub status: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeSystemInfo {
    pub architecture: String,
    pub boot_id: String,
    pub container_runtime_version: String,
    pub kernel_version: String,
    pub kube_proxy_version: String,
    pub kubelet_version: String,
    pub machine_id: String,
    pub operating_system: String,
    pub os_image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap: Option<serde_json::Value>,
    pub system_uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeDaemonEndpoints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kubelet_endpoint: Option<DaemonEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonEndpoint {
    pub port: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<NodeAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocatable: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<NodeCondition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_endpoints: Option<NodeDaemonEndpoints>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_info: Option<NodeSystemInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    #[serde(rename = "apiVersion", default = "default_node_api_version")]
    pub api_version: String,
    #[serde(default = "default_node_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<NodeSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NodeStatus>,
}

// ── Event ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventSeries {
    pub count: i32,
    pub last_observed_time: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(rename = "apiVersion", default = "default_event_api_version")]
    pub api_version: String,
    #[serde(default = "default_event_kind")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_timestamp: Option<Time>,
    pub involved_object: ObjectReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_timestamp: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<ObjectReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reporting_component: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reporting_instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<EventSeries>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<EventSource>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}
