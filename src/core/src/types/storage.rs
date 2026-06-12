//! # Storage Resources — PV, PVC, StorageClass, ConfigMap, Secret
//!
//! ## PersistentVolume (PV)
//!
//! A piece of storage in the cluster. Created by admin or dynamically provisioned.
//!
//! ## PersistentVolumeClaim (PVC)
//!
//! A request for storage by a user. Bound to a PV.
//!
//! ## StorageClass
//!
//! Defines a class of storage (e.g., "fast", "slow") with provisioning parameters.
//!
//! ## ConfigMap
//!
//! Stores configuration data as key-value pairs. Can be mounted as files or env vars.
//!
//! ## Secret
//!
//! Stores sensitive data (passwords, tokens). Similar to ConfigMap but base64-encoded.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::meta::ObjectMeta;

// ── PersistentVolume ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolume {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<PersistentVolumeSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PersistentVolumeStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeSpec {
    pub capacity: Option<BTreeMap<String, String>>,
    pub access_modes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_path: Option<HostPathVolumeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent_volume_reclaim_policy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostPathVolumeSource {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PersistentVolumeStatus {
    pub phase: Option<String>,
}

// ── PersistentVolumeClaim ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeClaim {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<PersistentVolumeClaimSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PersistentVolumeClaimStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeClaimSpec {
    pub access_modes: Option<Vec<String>>,
    pub resources: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PersistentVolumeClaimStatus {
    pub phase: Option<String>,
}

// ── StorageClass ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StorageClass {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub provisioner: String,
    pub parameters: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_binding_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reclaim_policy: Option<String>,
}

// ── ConfigMap ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMap {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_data: Option<BTreeMap<String, String>>,
}

impl ConfigMap {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.as_ref()?.get(key).map(|s| s.as_str())
    }
}

// ── Secret ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Secret {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_data: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

impl Secret {
    /// Get a raw value (base64-encoded as stored).
    pub fn get_raw(&self, key: &str) -> Option<&str> {
        self.data.as_ref()?.get(key).map(|s| s.as_str())
    }

    /// Get a string_data value (already decoded).
    pub fn get_string(&self, key: &str) -> Option<&str> {
        self.string_data.as_ref()?.get(key).map(|s| s.as_str())
    }
}
