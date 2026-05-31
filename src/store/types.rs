use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Time ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Time(pub String);

impl Time {
    pub fn now() -> Self {
        Self(chrono::Utc::now().to_rfc3339())
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
pub struct Status {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<StatusDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub metadata: ListMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
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
    pub env_from: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_pull_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub liveness_probe: Option<serde_json::Value>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<ContainerPort>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness_probe: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resize_policy: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_policy_rules: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_context: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_probe: Option<serde_json::Value>,
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
    pub security_context: Option<serde_json::Value>,
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
    pub host_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub pod_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pod {
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<PodSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PodStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_node: Option<String>,
    pub scheduler_epoch: u64,
}

impl Pod {
    pub fn new(metadata: ObjectMeta, spec: Option<PodSpec>) -> Self {
        Self { metadata, spec, status: None, assigned_node: None, scheduler_epoch: 0 }
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
    pub cluster_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_ips: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub pod_cidr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub architecture: Option<String>,
    pub boot_id: Option<String>,
    pub container_runtime_version: Option<String>,
    pub kernel_version: Option<String>,
    pub kube_proxy_version: Option<String>,
    pub kubelet_version: Option<String>,
    pub machine_id: Option<String>,
    pub operating_system: Option<String>,
    pub os_image: Option<String>,
    pub system_uuid: Option<String>,
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

// ── API discovery types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct APIVersions {
    pub versions: Vec<String>,
    pub server_address_by_client_cidrs: Vec<ServerAddressByClientCIDR>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerAddressByClientCIDR {
    pub client_cidr: String,
    pub server_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct APIGroupList {
    pub groups: Vec<APIGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct APIGroup {
    pub name: String,
    pub versions: Vec<GroupVersionForDiscovery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_version: Option<GroupVersionForDiscovery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_address_by_client_cidrs: Option<Vec<ServerAddressByClientCIDR>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupVersionForDiscovery {
    pub group_version: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct APIResourceList {
    pub group_version: String,
    pub resources: Vec<APIResource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct APIResource {
    pub name: String,
    pub singular_name: String,
    pub namespaced: bool,
    pub kind: String,
    pub verbs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_version_hash: Option<String>,
}

// ── Generic API typed list wrapper ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct List<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    pub metadata: ListMeta,
    pub items: Vec<T>,
}

// ── StoredResource (replaces AnyResource) ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StoredResource {
    Pod(Pod),
    Service(Service),
    ConfigMap(ConfigMap),
    Secret(Secret),
    PersistentVolume(PersistentVolume),
    PersistentVolumeClaim(PersistentVolumeClaim),
    Deployment(Deployment),
    Ingress(Ingress),
    NetworkPolicy(NetworkPolicy),
    Namespace(Namespace),
    Node(Node),
    Endpoints(Endpoints),
    EndpointSlice(EndpointSlice),
    Event(Event),
    StorageClass(StorageClass),
}

impl StoredResource {
    pub fn metadata(&self) -> &ObjectMeta {
        match self {
            StoredResource::Pod(r) => &r.metadata,
            StoredResource::Service(r) => &r.metadata,
            StoredResource::ConfigMap(r) => &r.metadata,
            StoredResource::Secret(r) => &r.metadata,
            StoredResource::PersistentVolume(r) => &r.metadata,
            StoredResource::PersistentVolumeClaim(r) => &r.metadata,
            StoredResource::Deployment(r) => &r.metadata,
            StoredResource::Ingress(r) => &r.metadata,
            StoredResource::NetworkPolicy(r) => &r.metadata,
            StoredResource::Namespace(r) => &r.metadata,
            StoredResource::Node(r) => &r.metadata,
            StoredResource::Endpoints(r) => &r.metadata,
            StoredResource::EndpointSlice(r) => &r.metadata,
            StoredResource::Event(r) => &r.metadata,
            StoredResource::StorageClass(r) => &r.metadata,
        }
    }

    pub fn metadata_mut(&mut self) -> &mut ObjectMeta {
        match self {
            StoredResource::Pod(r) => &mut r.metadata,
            StoredResource::Service(r) => &mut r.metadata,
            StoredResource::ConfigMap(r) => &mut r.metadata,
            StoredResource::Secret(r) => &mut r.metadata,
            StoredResource::PersistentVolume(r) => &mut r.metadata,
            StoredResource::PersistentVolumeClaim(r) => &mut r.metadata,
            StoredResource::Deployment(r) => &mut r.metadata,
            StoredResource::Ingress(r) => &mut r.metadata,
            StoredResource::NetworkPolicy(r) => &mut r.metadata,
            StoredResource::Namespace(r) => &mut r.metadata,
            StoredResource::Node(r) => &mut r.metadata,
            StoredResource::Endpoints(r) => &mut r.metadata,
            StoredResource::EndpointSlice(r) => &mut r.metadata,
            StoredResource::Event(r) => &mut r.metadata,
            StoredResource::StorageClass(r) => &mut r.metadata,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            StoredResource::Pod(_) => "Pod",
            StoredResource::Service(_) => "Service",
            StoredResource::ConfigMap(_) => "ConfigMap",
            StoredResource::Secret(_) => "Secret",
            StoredResource::PersistentVolume(_) => "PersistentVolume",
            StoredResource::PersistentVolumeClaim(_) => "PersistentVolumeClaim",
            StoredResource::Deployment(_) => "Deployment",
            StoredResource::Ingress(_) => "Ingress",
            StoredResource::NetworkPolicy(_) => "NetworkPolicy",
            StoredResource::Namespace(_) => "Namespace",
            StoredResource::Node(_) => "Node",
            StoredResource::Endpoints(_) => "Endpoints",
            StoredResource::EndpointSlice(_) => "EndpointSlice",
            StoredResource::Event(_) => "Event",
            StoredResource::StorageClass(_) => "StorageClass",
        }
    }

    pub fn name(&self) -> &str {
        self.metadata().name.as_deref().unwrap_or("<unnamed>")
    }

    pub fn namespace(&self) -> &str {
        match self {
            StoredResource::PersistentVolume(_) => "",
            _ => self.metadata().namespace.as_deref().unwrap_or("default"),
        }
    }

    pub fn uid(&self) -> String {
        format!("{}/{}/{}", self.kind(), self.namespace(), self.name())
    }
}
