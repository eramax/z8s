//! ConfigMap and Secret types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ObjectMeta;

fn default_configmap_api_version() -> String { "v1".to_string() }
fn default_configmap_kind() -> String { "ConfigMap".to_string() }
fn default_secret_api_version() -> String { "v1".to_string() }
fn default_secret_kind() -> String { "Secret".to_string() }

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
