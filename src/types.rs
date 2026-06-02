use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Macro for default apiVersion/kind functions ─────────────────────

macro_rules! define_kube_defaults {
    ($( $Type:ident as $kind:expr, $api_ver:expr => ($api_fn:ident, $kind_fn:ident); )+ ) => {
        $(
            fn $api_fn() -> String { $api_ver.to_string() }
            fn $kind_fn() -> String { $kind.to_string() }
        )+
    };
}

define_kube_defaults! {
    ConfigMap as "ConfigMap", "v1" => (default_configmap_api_version, default_configmap_kind);
    Secret as "Secret", "v1" => (default_secret_api_version, default_secret_kind);
    Namespace as "Namespace", "v1" => (default_namespace_api_version, default_namespace_kind);
    Node as "Node", "v1" => (default_node_api_version, default_node_kind);
    Event as "Event", "v1" => (default_event_api_version, default_event_kind);
}

#[path = "types/discovery.rs"]
mod discovery;
#[path = "types/rbac.rs"]
mod rbac;
#[path = "types/z8s_network.rs"]
mod z8s_network;
#[path = "types/any_resource.rs"]
mod any_resource;
#[path = "types/runtime.rs"]
mod runtime;
#[path = "types/helpers.rs"]
mod helpers;
#[path = "types/authz.rs"]
mod authz;
#[path = "types/scale.rs"]
mod scale;
#[path = "types/meta.rs"]
mod meta;
#[path = "types/workload.rs"]
mod workload;
#[path = "types/networking.rs"]
mod networking;
#[path = "types/storage.rs"]
mod storage;

pub use any_resource::AnyResource;
pub use meta::*;
pub use networking::*;
pub use storage::*;
pub use workload::*;
pub use authz::*;
pub use discovery::*;
pub use helpers::*;
pub use rbac::*;
pub use runtime::*;
pub use scale::*;
pub use z8s_network::*;

#[cfg(test)]
#[path = "types/tests.rs"]
mod tests;


// ── ConfigMap ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMap {
    #[serde(rename = "apiVersion", default = "default_configmap_api_version")]
    pub api_version: String,
    #[serde(default = "default_configmap_kind")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_data: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub immutable: Option<bool>,
    pub metadata: ObjectMeta,
}

// ── Secret ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Secret {
    #[serde(rename = "apiVersion", default = "default_secret_api_version")]
    pub api_version: String,
    #[serde(default = "default_secret_kind")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub immutable: Option<bool>,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_data: Option<BTreeMap<String, String>>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}


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


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<StatusDetails>,
    pub metadata: ListMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(rename = "apiVersion", skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            code: None,
            details: None,
            metadata: ListMeta::default(),
            message: None,
            reason: None,
            status: None,
            kind: Some("Status".into()),
            api_version: Some("v1".into()),
        }
    }
}
