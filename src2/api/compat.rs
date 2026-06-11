//! Wire encoding for kubectl compatibility paths (A1).

use crate::store::AnyResource;

/// API version to emit on list/get responses for a request path.
#[derive(Debug, Clone)]
pub struct WireContext {
    pub list_api_version: String,
}

impl WireContext {
    pub fn from_path(path: &str) -> Self {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let version = if parts.len() >= 3 && parts[0] == "apis" {
            match parts[1] {
                "z8s.io" if parts.get(2) == Some(&"v1") => "z8s.io/v1",
                "apps" if parts.get(2) == Some(&"v1") => "apps/v1",
                "networking.k8s.io" if parts.get(2) == Some(&"v1") => "networking.k8s.io/v1",
                "storage.k8s.io" if parts.get(2) == Some(&"v1") => "storage.k8s.io/v1",
                "rbac.authorization.k8s.io" if parts.get(2) == Some(&"v1") => {
                    "rbac.authorization.k8s.io/v1"
                }
                "discovery.k8s.io" if parts.get(2) == Some(&"v1") => "discovery.k8s.io/v1",
                other => other,
            }
        } else if parts.len() >= 2 && parts[0] == "api" && parts[1] == "v1" {
            "v1"
        } else {
            "z8s.io/v1"
        };
        Self {
            list_api_version: version.to_string(),
        }
    }

    pub fn for_entry(entry: &crate::api::catalog::ResourceEntry) -> Self {
        Self {
            list_api_version: entry.list_api_version.to_string(),
        }
    }
}

pub fn encode_resource_value(mut value: serde_json::Value, wire: &WireContext) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "apiVersion".to_string(),
            serde_json::Value::String(wire.list_api_version.clone()),
        );
    }
    value
}

pub fn encode_resource(resource: &AnyResource, wire: &WireContext) -> serde_json::Value {
    let value = serde_json::to_value(resource).unwrap_or_default();
    encode_resource_value(value, wire)
}
