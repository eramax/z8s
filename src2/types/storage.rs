//! PersistentVolume, PersistentVolumeClaim, and StorageClass types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    HostPathVolumeSource, LabelSelector, ObjectMeta, ObjectReference, Quantity,
    ResourceRequirements, Time,
};

fn default_persistentvolume_api_version() -> String { "v1".to_string() }
fn default_persistentvolume_kind() -> String { "PersistentVolume".to_string() }
fn default_persistentvolumeclaim_api_version() -> String { "v1".to_string() }
fn default_persistentvolumeclaim_kind() -> String { "PersistentVolumeClaim".to_string() }
fn default_storageclass_api_version() -> String { "storage.k8s.io/v1".to_string() }
fn default_storageclass_kind() -> String { "StorageClass".to_string() }

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
