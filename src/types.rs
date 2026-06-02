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
    Pod as "Pod", "v1" => (default_pod_api_version, default_pod_kind);
    Deployment as "Deployment", "apps/v1" => (default_deployment_api_version, default_deployment_kind);
    Service as "Service", "v1" => (default_service_api_version, default_service_kind);
    ConfigMap as "ConfigMap", "v1" => (default_configmap_api_version, default_configmap_kind);
    Secret as "Secret", "v1" => (default_secret_api_version, default_secret_kind);
    PersistentVolume as "PersistentVolume", "v1" => (default_persistentvolume_api_version, default_persistentvolume_kind);
    PersistentVolumeClaim as "PersistentVolumeClaim", "v1" => (default_persistentvolumeclaim_api_version, default_persistentvolumeclaim_kind);
    Ingress as "Ingress", "networking.k8s.io/v1" => (default_ingress_api_version, default_ingress_kind);
    NetworkPolicy as "NetworkPolicy", "networking.k8s.io/v1" => (default_networkpolicy_api_version, default_networkpolicy_kind);
    Namespace as "Namespace", "v1" => (default_namespace_api_version, default_namespace_kind);
    Node as "Node", "v1" => (default_node_api_version, default_node_kind);
    Endpoints as "Endpoints", "v1" => (default_endpoints_api_version, default_endpoints_kind);
    EndpointSlice as "EndpointSlice", "discovery.k8s.io/v1" => (default_endpointslice_api_version, default_endpointslice_kind);
    Event as "Event", "v1" => (default_event_api_version, default_event_kind);
    StorageClass as "StorageClass", "storage.k8s.io/v1" => (default_storageclass_api_version, default_storageclass_kind);
}

fn default_api_version() -> String { "z8s.io/v1".to_string() }
fn default_vnet_kind() -> String { "VNet".to_string() }
fn default_subnet_kind() -> String { "Subnet".to_string() }
fn default_nsg_kind() -> String { "NSG".to_string() }
fn default_routetable_kind() -> String { "RouteTable".to_string() }

#[path = "types/discovery.rs"]
mod discovery;
#[path = "types/rbac.rs"]
mod rbac;

pub use discovery::*;
pub use rbac::*;

// ── Time ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Time(pub String);

impl Time {
    pub fn now() -> Self {
        Self(crate::config::now_rfc3339())
    }
}

// ── Quantity ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Quantity(pub String);

// ── IntOrString ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum IntOrString {
    Int(i32),
    String(String),
}

impl Default for IntOrString {
    fn default() -> Self {
        IntOrString::Int(0)
    }
}

// ── ObjectMeta ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_grace_period_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_timestamp: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finalizers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_fields: Option<Vec<ManagedFieldsEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_references: Option<Vec<OwnerReference>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

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

// ── OwnerReference ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OwnerReference {
    pub api_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_owner_deletion: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<bool>,
    pub kind: String,
    pub name: String,
    pub uid: String,
}

// ── ListMeta / Status ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListMeta {
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_item_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causes: Option<Vec<StatusCause>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusCause {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── EnvFromSource types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnvFromSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_map_ref: Option<ConfigMapEnvSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretEnvSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMapEnvSource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretEnvSource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

// ── SecurityContext / Capabilities ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecurityContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_privilege_escalation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privileged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_as_group: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_as_user: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_as_non_root: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seccomp_profile: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PodSecurityContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_as_group: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_as_non_root: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_as_user: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fs_group: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seccomp_profile: Option<serde_json::Value>,
}

// ── Probe types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExecAction {
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPGetAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_headers: Option<Vec<HTTPHeader>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub port: IntOrString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TCPSocketAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub port: IntOrString,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_get: Option<HTTPGetAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp_socket: Option<TCPSocketAction>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub initial_delay_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub timeout_seconds: Option<i32>,
}

// ── Container ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<EnvVar>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_from: Option<Vec<EnvFromSource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_pull_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub liveness_probe: Option<Probe>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<ContainerPort>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness_probe: Option<Probe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resize_policy: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_policy_rules: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_context: Option<SecurityContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_probe: Option<Probe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin_once: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination_message_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination_message_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_devices: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_mounts: Option<Vec<VolumeMount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerPort {
    pub container_port: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostIP")]
    pub host_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_from: Option<EnvVarSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnvVarSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_map_key_ref: Option<ConfigMapKeySelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_ref: Option<ObjectFieldSelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_field_ref: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key_ref: Option<SecretKeySelector>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectFieldSelector {
    pub api_version: Option<String>,
    pub field_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMapKeySelector {
    pub key: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretKeySelector {
    pub key: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRequirements {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<BTreeMap<String, Quantity>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeMount {
    pub mount_path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_propagation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_path_expr: Option<String>,
}

// ── Volume ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_elastic_block_store: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_disk: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_file: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cephfs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cinder: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_map: Option<ConfigMapVolumeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csi: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downward_api: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_dir: Option<EmptyDirVolumeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fc: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flex_volume: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flocker: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gce_persistent_disk: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_repo: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glusterfs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_path: Option<HostPathVolumeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iscsi: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nfs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent_volume_claim: Option<PersistentVolumeClaimVolumeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photon_persistent_disk: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portworx_volume: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quobyte: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rbd: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_io: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretVolumeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storageos: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vsphere_volume: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostPathVolumeSource {
    pub path: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EmptyDirVolumeSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_limit: Option<Quantity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMapVolumeSource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_mode: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<KeyToPath>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretVolumeSource {
    pub secret_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_mode: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<KeyToPath>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeClaimVolumeSource {
    pub claim_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyToPath {
    pub key: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<i32>,
}

// ── PodSpec ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PodSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_deadline_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affinity: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automount_service_account_token: Option<bool>,
    pub containers: Vec<Container>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_service_links: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral_containers: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_aliases: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_ipc: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_network: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_pid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_users: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_pull_secrets: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_containers: Option<Vec<Container>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overhead: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preemption_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness_gates: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_claims: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduling_gates: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_context: Option<PodSecurityContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set_hostname_as_fqdn: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_process_namespace: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination_grace_period_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerations: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topology_spread_constraints: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volumes: Option<Vec<Volume>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload_ref: Option<serde_json::Value>,
}

// ── PodStatus types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PodCondition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_probe_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStateWaiting {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStateRunning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStateTerminated {
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<ContainerStateRunning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminated: Option<ContainerStateTerminated>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<ContainerStateWaiting>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocated_resources: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocated_resources_status: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    pub image: String,
    pub image_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_state: Option<ContainerState>,
    pub name: String,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
    pub restart_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ContainerState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_mounts: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostIP {
    pub ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PodIP {
    pub ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PodStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocated_resources: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<PodCondition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_statuses: Option<Vec<ContainerStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral_container_statuses: Option<Vec<ContainerStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extended_resource_claim_status: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostIP")]
    pub host_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostIPs")]
    pub host_ips: Option<Vec<HostIP>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_container_statuses: Option<Vec<ContainerStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nominated_node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "podIP")]
    pub pod_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "podIPs")]
    pub pod_ips: Option<Vec<PodIP>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qos_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resize: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_claim_statuses: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<Time>,
}

// ── Pod (our type with scheduler fields) ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pod {
    #[serde(rename = "apiVersion", default = "default_pod_api_version")]
    pub api_version: String,
    #[serde(default = "default_pod_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<PodSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PodStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_node: Option<String>,
    #[serde(default)]
    pub scheduler_epoch: u64,
}

impl Default for Pod {
    fn default() -> Self {
        Self {
            api_version: "v1".into(),
            kind: "Pod".into(),
            metadata: ObjectMeta::default(),
            spec: None,
            status: None,
            assigned_node: None,
            scheduler_epoch: 0,
        }
    }
}

impl Pod {
    pub fn new(metadata: ObjectMeta, spec: Option<PodSpec>) -> Self {
        Self {
            api_version: "v1".into(),
            kind: "Pod".into(),
            metadata,
            spec,
            status: None,
            assigned_node: None,
            scheduler_epoch: 0,
        }
    }
}

// ── Service ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServicePort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_port: Option<i32>,
    pub port: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_port: Option<IntOrString>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocate_load_balancer_node_ports: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "clusterIP")]
    pub cluster_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "clusterIPs")]
    pub cluster_ips: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalIPs")]
    pub external_ips: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_traffic_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_node_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_traffic_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_families: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_family_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "loadBalancerIP")]
    pub load_balancer_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer_source_ranges: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<ServicePort>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_not_ready_addresses: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity_config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_distribution: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LoadBalancerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancerStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress: Option<Vec<LoadBalancerIngress>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancerIngress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<PortStatus>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub port: i32,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    #[serde(rename = "apiVersion", default = "default_service_api_version")]
    pub api_version: String,
    #[serde(default = "default_service_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<ServiceSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ServiceStatus>,
}

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

// ── PersistentVolume ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_modes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_elastic_block_store: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_disk: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_file: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cephfs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cinder: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_ref: Option<ObjectReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csi: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fc: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flex_volume: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flocker: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gce_persistent_disk: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glusterfs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_path: Option<HostPathVolumeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iscsi: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nfs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_affinity: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent_volume_reclaim_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photon_persistent_disk: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portworx_volume: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quobyte: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rbd: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_io: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storageos: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_attributes_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vsphere_volume: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_phase_transition_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolume {
    #[serde(
        rename = "apiVersion",
        default = "default_persistentvolume_api_version"
    )]
    pub api_version: String,
    #[serde(default = "default_persistentvolume_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<PersistentVolumeSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PersistentVolumeStatus>,
}

// ── PersistentVolumeClaim ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeClaimSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_modes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_source: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_source_ref: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<LabelSelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_attributes_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeClaimStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_modes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocated_resource_statuses: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<BTreeMap<String, Quantity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_volume_attributes_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modify_volume_status: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeClaim {
    #[serde(
        rename = "apiVersion",
        default = "default_persistentvolumeclaim_api_version"
    )]
    pub api_version: String,
    #[serde(default = "default_persistentvolumeclaim_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<PersistentVolumeClaimSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PersistentVolumeClaimStatus>,
}

// ── ObjectReference ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectReference {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

// ── LabelSelector (for NetworkPolicy, PVC) ───────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LabelSelector {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_expressions: Option<Vec<LabelSelectorRequirement>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LabelSelectorRequirement {
    pub key: String,
    pub operator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

// ── ReplicaSet / Deployment types ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PodTemplateSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ObjectMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<PodSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_ready_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_deadline_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_history_limit: Option<i32>,
    pub selector: LabelSelector,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<DeploymentStrategy>,
    pub template: PodTemplateSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentStrategy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolling_update: Option<RollingUpdateDeployment>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RollingUpdateDeployment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_surge: Option<IntOrString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_unavailable: Option<IntOrString>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_replicas: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<DeploymentCondition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_replicas: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_replicas: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_replicas: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentCondition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_time: Option<Time>,
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
pub struct Deployment {
    #[serde(rename = "apiVersion", default = "default_deployment_api_version")]
    pub api_version: String,
    #[serde(default = "default_deployment_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<DeploymentSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<DeploymentStatus>,
}

// ── Ingress ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressTLS {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HTTPIngressRuleValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPIngressRuleValue {
    pub paths: Vec<HTTPIngressPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPIngressPath {
    pub backend: IngressBackend,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressBackend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ObjectReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<IngressServiceBackend>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressServiceBackend {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<ServiceBackendPort>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceBackendPort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_backend: Option<IngressBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<IngressRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<Vec<IngressTLS>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LoadBalancerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ingress {
    #[serde(rename = "apiVersion", default = "default_ingress_api_version")]
    pub api_version: String,
    #[serde(default = "default_ingress_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<IngressSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<IngressStatus>,
}

// ── NetworkPolicy ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyPeer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_block: Option<IPBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace_selector: Option<LabelSelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pod_selector: Option<LabelSelector>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IPBlock {
    pub cidr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub except: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyPort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<IntOrString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyIngressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<NetworkPolicyPeer>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<NetworkPolicyPort>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyEgressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<NetworkPolicyPort>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<NetworkPolicyPeer>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicySpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egress: Option<Vec<NetworkPolicyEgressRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress: Option<Vec<NetworkPolicyIngressRule>>,
    pub pod_selector: LabelSelector,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    #[serde(rename = "apiVersion", default = "default_networkpolicy_api_version")]
    pub api_version: String,
    #[serde(default = "default_networkpolicy_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<NetworkPolicySpec>,
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

// ── Endpoints ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointAddress {
    pub ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<ObjectReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointPort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub port: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSubset {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<EndpointAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_ready_addresses: Option<Vec<EndpointAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<EndpointPort>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Endpoints {
    #[serde(rename = "apiVersion", default = "default_endpoints_api_version")]
    pub api_version: String,
    #[serde(default = "default_endpoints_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subsets: Option<Vec<EndpointSubset>>,
}

// ── EndpointSlice ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSliceEndpoint {
    pub addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<EndpointSliceConditions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<ObjectReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSliceConditions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serving: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminating: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSlicePort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSlice {
    #[serde(rename = "apiVersion", default = "default_endpointslice_api_version")]
    pub api_version: String,
    #[serde(default = "default_endpointslice_kind")]
    pub kind: String,
    pub address_type: String,
    pub endpoints: Vec<EndpointSliceEndpoint>,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<EndpointSlicePort>>,
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

// ── StorageClass ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StorageClass {
    #[serde(rename = "apiVersion", default = "default_storageclass_api_version")]
    pub api_version: String,
    #[serde(default = "default_storageclass_kind")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_volume_expansion: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_topologies: Option<Vec<serde_json::Value>>,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, String>>,
    pub provisioner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reclaim_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_binding_mode: Option<String>,
}

// ── SelfSubjectAccessReview (authorization/v1) ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelfSubjectAccessReview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ObjectMeta>,
    pub spec: SelfSubjectAccessReviewSpec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<SubjectAccessReviewStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelfSubjectAccessReviewSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_attributes: Option<ResourceAttributes>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceAttributes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subresource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectAccessReviewStatus {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluation_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── Scale (autoscaling/v1) ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScaleSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScaleStatus {
    pub replicas: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scale {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ObjectMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<ScaleSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ScaleStatus>,
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

fn default_true() -> bool {
    true
}
fn default_role() -> String {
    "spoke".to_string()
}
fn default_priority() -> u32 {
    1000
}
fn default_header_operator() -> String {
    "eq".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VNet {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_vnet_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: VNetSpec,
    #[serde(default)]
    pub status: Option<VNetStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VNetSpec {
    #[serde(default)]
    pub cidr: Option<String>,
    #[serde(default = "default_true")]
    pub internet_access: bool,
    #[serde(default = "default_role")]
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct VNetStatus {
    pub cidr: String,
    #[serde(default)]
    pub pod_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Subnet {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_subnet_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: SubnetSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubnetSpec {
    pub vnet: String,
    pub cidr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Nsg {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_nsg_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: NsgSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NsgSpec {
    pub target_vnets: Vec<String>,
    #[serde(default)]
    pub rules: Vec<NsgRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NsgRule {
    pub name: String,
    pub action: String,
    #[serde(alias = "src_cidrs", default)]
    pub srcCIDRs: Vec<String>,
    #[serde(alias = "dst_cidrs", default)]
    pub dstCIDRs: Vec<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub protocol: String,
    #[serde(default = "default_priority")]
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RouteTable {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_routetable_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: RouteTableSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RouteTableSpec {
    #[serde(default)]
    pub rules: Vec<RouteRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RouteRule {
    pub name: String,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub headers: Vec<HeaderMatch>,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HeaderMatch {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_header_operator")]
    pub operator: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// AnyResource — the primary enum used in the store backend and throughout z8s.
// ═══════════════════════════════════════════════════════════════════════════════

macro_rules! define_any_resource {
    (
        namespaced: [ $( $variant:ident($inner:ident) as $kind:expr ),+ $(,)? ];
        cluster_scoped: [ $( $cvariant:ident($cinner:ident) as $ckind:expr ),+ $(,)? ];
    ) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(tag = "resourceType")]
        pub enum AnyResource {
            $( $variant($inner), )+
            $( $cvariant($cinner), )+
        }

        impl AnyResource {
            pub fn metadata(&self) -> &ObjectMeta {
                match self {
                    $( Self::$variant(r) => &r.metadata, )+
                    $( Self::$cvariant(r) => &r.metadata, )+
                }
            }

            pub fn metadata_mut(&mut self) -> &mut ObjectMeta {
                match self {
                    $( Self::$variant(r) => &mut r.metadata, )+
                    $( Self::$cvariant(r) => &mut r.metadata, )+
                }
            }

            pub fn kind(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => $kind, )+
                    $( Self::$cvariant(_) => $ckind, )+
                }
            }

            pub fn name(&self) -> &str {
                self.metadata().name.as_deref().unwrap_or("<unnamed>")
            }

            pub fn namespace(&self) -> &str {
                match self {
                    $( Self::$variant(_) => {
                        self.metadata().namespace.as_deref().unwrap_or("default")
                    }, )+
                    $( Self::$cvariant(_) => "", )+
                }
            }

            pub fn uid(&self) -> String {
                format!("{}/{}/{}", self.kind(), self.namespace(), self.name())
            }

            pub fn from_yaml_value(
                value: serde_yaml::Value, kind: &str,
            ) -> anyhow::Result<Self> {
                match kind {
                    $(
                        $kind => Ok(Self::$variant(
                            serde_yaml::from_value(value)
                                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", $kind, e))?
                        )),
                    )+
                    $(
                        $ckind => Ok(Self::$cvariant(
                            serde_yaml::from_value(value)
                                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", $ckind, e))?
                        )),
                    )+
                    _ => anyhow::bail!("Unsupported resource kind: {}", kind),
                }
            }

            pub fn from_json_value(
                value: serde_json::Value, kind: &str,
            ) -> anyhow::Result<Self> {
                match kind {
                    $(
                        $kind => Ok(Self::$variant(serde_json::from_value(value)?)),
                    )+
                    $(
                        $ckind => Ok(Self::$cvariant(serde_json::from_value(value)?)),
                    )+
                    _ => anyhow::bail!("Unsupported kind: {}", kind),
                }
            }
        }
    };
}

define_any_resource! {
    namespaced: [
        Pod(Pod) as "Pod",
        Deployment(Deployment) as "Deployment",
        Service(Service) as "Service",
        ConfigMap(ConfigMap) as "ConfigMap",
        Secret(Secret) as "Secret",
        PersistentVolumeClaim(PersistentVolumeClaim) as "PersistentVolumeClaim",
        VNet(VNet) as "VNet",
        Subnet(Subnet) as "Subnet",
        Nsg(Nsg) as "NSG",
        RouteTable(RouteTable) as "RouteTable",
        Ingress(Ingress) as "Ingress",
        NetworkPolicy(NetworkPolicy) as "NetworkPolicy",
        Namespace(Namespace) as "Namespace",
        Node(Node) as "Node",
        Endpoints(Endpoints) as "Endpoints",
        EndpointSlice(EndpointSlice) as "EndpointSlice",
        Event(Event) as "Event",
        StorageClass(StorageClass) as "StorageClass",
        Role(Role) as "Role",
        RoleBinding(RoleBinding) as "RoleBinding",
        ServiceAccount(ServiceAccount) as "ServiceAccount",
    ];
    cluster_scoped: [
        PersistentVolume(PersistentVolume) as "PersistentVolume",
        ClusterRole(ClusterRole) as "ClusterRole",
        ClusterRoleBinding(ClusterRoleBinding) as "ClusterRoleBinding",
    ];
}

// ═══════════════════════════════════════════════════════════════════════════════
// ResourceState / ResourceTracker
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceState {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Terminated,
}

#[derive(Debug, Clone)]
pub struct ResourceTracker {
    pub resource: AnyResource,
    pub state: ResourceState,
    pub last_updated: String,
}

impl ResourceTracker {
    pub fn new(resource: AnyResource) -> Self {
        Self {
            resource,
            state: ResourceState::Pending,
            last_updated: crate::config::now_rfc3339(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// NodeRecord / LeaseRecord — cluster membership and scheduler lease
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeRecord {
    pub node_name: String,
    #[serde(rename = "nodeIP")]
    pub node_ip: String,
    pub last_seen: i64,
    pub state: NodeState,
    pub pod_count: u32,
    pub capacity_pods: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum NodeState {
    Active,
    Dead,
}

impl Default for NodeState {
    fn default() -> Self {
        NodeState::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LeaseRecord {
    pub holder: String,
    pub epoch: u64,
    pub expires_at_ms: i64,
    pub acquired_at_ms: i64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// YAML parsing
// ═══════════════════════════════════════════════════════════════════════════════

pub fn parse_manifest_yaml(yaml: &str) -> anyhow::Result<Vec<AnyResource>> {
    use anyhow::Context;
    let mut resources = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(yaml) {
        let value: serde_yaml::Value =
            serde_yaml::Value::deserialize(doc).context("Failed to parse YAML document")?;
        let kind = value
            .get("kind")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string())
            .context("Missing 'kind' field in YAML")?;
        resources.push(AnyResource::from_yaml_value(value, &kind)?);
    }
    Ok(resources)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helpers — these operate on AnyResource
// ═══════════════════════════════════════════════════════════════════════════════

pub fn extract_containers(resource: &AnyResource) -> Vec<Container> {
    match resource {
        AnyResource::Pod(pod) => pod.spec.as_ref().map_or_else(Vec::new, |s| {
            let mut c = s.containers.clone();
            if let Some(init) = &s.init_containers {
                c.extend(init.clone());
            }
            c
        }),
        AnyResource::Deployment(deploy) => deploy
            .spec
            .as_ref()
            .and_then(|s| s.template.spec.as_ref())
            .map_or_else(Vec::new, |s| {
                let mut c = s.containers.clone();
                if let Some(init) = &s.init_containers {
                    c.extend(init.clone());
                }
                c
            }),
        _ => Vec::new(),
    }
}

pub fn parse_quantity_bytes(q: &Quantity) -> u64 {
    let s = q.0.trim();
    if let Some(rest) = s.strip_suffix("Ti") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(4)
    } else if let Some(rest) = s.strip_suffix("Gi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(3)
    } else if let Some(rest) = s.strip_suffix("Mi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(2)
    } else if let Some(rest) = s.strip_suffix("Ki") {
        rest.parse::<u64>().unwrap_or(0) * 1024
    } else if let Some(rest) = s.strip_suffix("Pi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(5)
    } else if let Some(rest) = s.strip_suffix("Ei") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(6)
    } else if let Some(rest) = s.strip_suffix('T') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000_000_000
    } else if let Some(rest) = s.strip_suffix('G') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000_000
    } else if let Some(rest) = s.strip_suffix('M') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000
    } else if let Some(rest) = s.strip_suffix('k') {
        rest.parse::<u64>().unwrap_or(0) * 1000
    } else {
        s.parse::<u64>().unwrap_or(0)
    }
}

pub fn parse_quantity_cpu(q: &Quantity) -> (i64, i64) {
    let s = q.0.trim();
    if let Some(rest) = s.strip_suffix('m') {
        let millicores = rest.parse::<i64>().unwrap_or(0);
        (millicores * 100, 100_000)
    } else {
        let cores = s.parse::<f64>().unwrap_or(0.0);
        ((cores * 100_000.0) as i64, 100_000)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_includes_namespace() {
        let yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: my-pod\n  namespace: production\nspec:\n  containers:\n  - name: c\n    image: alpine\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        let r = &resources[0];
        assert_eq!(r.uid(), "Pod/production/my-pod");
    }

    #[test]
    fn uid_defaults_namespace_to_default() {
        let yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: my-pod\nspec:\n  containers:\n  - name: c\n    image: alpine\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources[0].uid(), "Pod/default/my-pod");
    }

    #[test]
    fn same_name_different_namespaces_get_different_uids() {
        let pod_a = parse_manifest_yaml(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: app\n  namespace: ns-a\nspec:\n  containers:\n  - name: c\n    image: alpine\n",
        ).unwrap().remove(0);
        let pod_b = parse_manifest_yaml(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: app\n  namespace: ns-b\nspec:\n  containers:\n  - name: c\n    image: alpine\n",
        ).unwrap().remove(0);
        assert_ne!(pod_a.uid(), pod_b.uid());
    }

    #[test]
    fn pv_namespace_is_empty() {
        let yaml = "apiVersion: v1\nkind: PersistentVolume\nmetadata:\n  name: my-pv\nspec:\n  capacity:\n    storage: 1Gi\n  accessModes:\n  - ReadWriteOnce\n  hostPath:\n    path: /data\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources[0].namespace(), "");
        assert_eq!(resources[0].uid(), "PersistentVolume//my-pv");
    }

    #[tokio::test]
    async fn store_namespaced_resources_do_not_collide() {
        use crate::store::{MemoryBackend, StoreBackend};
        let store = MemoryBackend::new();

        let cm_a_yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: config\n  namespace: ns-a\ndata:\n  key: value-a\n";
        let cm_b_yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: config\n  namespace: ns-b\ndata:\n  key: value-b\n";

        let res_a = parse_manifest_yaml(cm_a_yaml).unwrap().remove(0);
        let res_b = parse_manifest_yaml(cm_b_yaml).unwrap().remove(0);

        store.apply(res_a).await.unwrap();
        store.apply(res_b).await.unwrap();

        let all = store.get_by_kind("ConfigMap").await;
        assert_eq!(all.len(), 2);

        let a = store.get("ConfigMap/ns-a/config").await;
        let b = store.get("ConfigMap/ns-b/config").await;
        assert!(a.is_some());
        assert!(b.is_some());

        if let Some(AnyResource::ConfigMap(cm)) = a.map(|t| t.resource) {
            assert_eq!(cm.data.unwrap().get("key").unwrap(), "value-a");
        }
        if let Some(AnyResource::ConfigMap(cm)) = b.map(|t| t.resource) {
            assert_eq!(cm.data.unwrap().get("key").unwrap(), "value-b");
        }
    }

    #[test]
    fn parse_multi_document_yaml() {
        let yaml = "\
apiVersion: v1
kind: ConfigMap
metadata:
  name: cm1
  namespace: default
---
apiVersion: v1
kind: Secret
metadata:
  name: sec1
  namespace: default
";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].kind(), "ConfigMap");
        assert_eq!(resources[1].kind(), "Secret");
    }

    #[test]
    fn parse_pv_and_pvc() {
        let yaml = "\
apiVersion: v1
kind: PersistentVolume
metadata:
  name: pv1
spec:
  capacity:
    storage: 5Gi
  accessModes:
  - ReadWriteOnce
  hostPath:
    path: /data/pv1
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: pvc1
  namespace: default
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources.len(), 2);
        assert!(matches!(&resources[0], AnyResource::PersistentVolume(_)));
        assert!(matches!(
            &resources[1],
            AnyResource::PersistentVolumeClaim(_)
        ));
    }

    #[test]
    fn parse_unsupported_kind_returns_error() {
        let yaml = "apiVersion: v1\nkind: UnknownThing\nmetadata:\n  name: x\n";
        assert!(parse_manifest_yaml(yaml).is_err());
    }

    #[test]
    fn extract_containers_from_pod_includes_init() {
        let yaml = "\
apiVersion: v1
kind: Pod
metadata:
  name: p
spec:
  initContainers:
  - name: init
    image: busybox
  containers:
  - name: app
    image: alpine
";
        let resource = parse_manifest_yaml(yaml).unwrap().remove(0);
        let containers = extract_containers(&resource);
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].name, "app");
        assert_eq!(containers[1].name, "init");
    }

    #[test]
    fn extract_containers_from_non_pod_is_empty() {
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cm\n";
        let resource = parse_manifest_yaml(yaml).unwrap().remove(0);
        assert!(extract_containers(&resource).is_empty());
    }

    #[test]
    fn parse_quantity_ki() {
        assert_eq!(parse_quantity_bytes(&Quantity("128Ki".into())), 128 * 1024);
    }

    #[test]
    fn parse_quantity_mi() {
        assert_eq!(
            parse_quantity_bytes(&Quantity("256Mi".into())),
            256 * 1024 * 1024
        );
    }

    #[test]
    fn parse_quantity_gi() {
        assert_eq!(
            parse_quantity_bytes(&Quantity("1Gi".into())),
            1024 * 1024 * 1024
        );
    }

    #[test]
    fn parse_quantity_plain_bytes() {
        assert_eq!(parse_quantity_bytes(&Quantity("4096".into())), 4096);
    }

    #[test]
    fn parse_cpu_millicores() {
        let (quota, period) = parse_quantity_cpu(&Quantity("500m".into()));
        assert_eq!(quota, 50_000);
        assert_eq!(period, 100_000);
    }

    #[test]
    fn parse_cpu_cores() {
        let (quota, period) = parse_quantity_cpu(&Quantity("2".into()));
        assert_eq!(quota, 200_000);
        assert_eq!(period, 100_000);
    }
}
