//! API discovery documents generated from `catalog::CATALOG` (A3).

use crate::api::catalog::{self, ResourceEntry};
use crate::api::server::api_resource;
use crate::types::{APIResource, APIResourceList};

const STD_VERBS: &[&str] = &[
    "get", "list", "watch", "create", "update", "patch", "delete",
];

fn singular(entry: &ResourceEntry) -> &str {
    match entry.plural {
        "pods" => "pod",
        "deployments" => "deployment",
        "services" => "service",
        "configmaps" => "configmap",
        "secrets" => "secret",
        "namespaces" => "namespace",
        "nodes" => "node",
        "persistentvolumes" => "persistentvolume",
        "persistentvolumeclaims" => "persistentvolumeclaim",
        "storageclasses" => "storageclass",
        "ingresses" => "ingress",
        "networkpolicies" => "networkpolicy",
        "endpoints" => "endpoints",
        "endpointslices" => "endpointslice",
        "events" => "event",
        "vnets" => "vnet",
        "subnets" => "subnet",
        "nsgs" => "nsg",
        "routetables" => "routetable",
        "roles" => "role",
        "rolebindings" => "rolebinding",
        "clusterroles" => "clusterrole",
        "clusterrolebindings" => "clusterrolebinding",
        "serviceaccounts" => "serviceaccount",
        _ => entry.plural,
    }
}

fn short_names(entry: &ResourceEntry) -> &'static [&'static str] {
    match entry.plural {
        "pods" => &["po"],
        "namespaces" => &["ns"],
        "nodes" => &["no"],
        "services" => &["svc"],
        "endpoints" => &["ep"],
        "configmaps" => &["cm"],
        "persistentvolumes" => &["pv"],
        "persistentvolumeclaims" => &["pvc"],
        "deployments" => &["deploy"],
        "ingresses" => &["ing"],
        "networkpolicies" => &["netpol"],
        "storageclasses" => &["sc"],
        "events" => &["ev"],
        "vnets" => &["vn"],
        "subnets" => &["sn"],
        "nsgs" => &["nsg"],
        "routetables" => &["rt"],
        _ => &[],
    }
}

pub fn entry_to_api_resource(entry: &ResourceEntry) -> APIResource {
    let short = short_names(entry);
    api_resource(
        entry.plural,
        singular(entry),
        entry.namespaced,
        entry.kind,
        STD_VERBS,
        short,
        &["all"],
    )
}

pub fn resources_for_api_version(api_version: &str) -> Vec<APIResource> {
    catalog::CATALOG
        .iter()
        .filter(|e| e.list_api_version == api_version)
        .map(entry_to_api_resource)
        .collect()
}

pub fn api_resource_list(api_version: &str) -> APIResourceList {
    APIResourceList {
        group_version: api_version.to_string(),
        resources: resources_for_api_version(api_version),
    }
}

fn col(name: &str, json_path: &str, col_type: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "type": col_type,
        "jsonPath": json_path,
        "description": name
    })
}

/// z8s.io/v1 discovery with printer columns for network CRDs.
pub fn z8s_v1_discovery_json() -> serde_json::Value {
    let mut resources: Vec<serde_json::Value> = catalog::CATALOG
        .iter()
        .filter(|e| e.list_api_version == "z8s.io/v1")
        .map(|e| {
            let mut r = serde_json::to_value(entry_to_api_resource(e)).unwrap_or_default();
            if let Some(obj) = r.as_object_mut() {
                apply_z8s_columns(e.plural, obj);
            }
            r
        })
        .collect();

    resources.sort_by(|a, b| {
        a["name"]
            .as_str()
            .cmp(&b["name"].as_str())
    });

    serde_json::json!({
        "groupVersion": "z8s.io/v1",
        "resources": resources,
    })
}

fn apply_z8s_columns(plural: &str, obj: &mut serde_json::Map<String, serde_json::Value>) {
    let cols = match plural {
        "vnets" => vec![
            col("CIDR", ".spec.cidr", "string"),
            col("Role", ".spec.role", "string"),
            col("Internet", ".spec.internetAccess", "boolean"),
            col("Subnets", ".spec._subnetCount", "integer"),
            col("Pods", ".spec._podCount", "integer"),
            col("Services", ".spec._serviceCount", "integer"),
            col("Age", ".metadata.creationTimestamp", "date"),
        ],
        "subnets" => vec![
            col("CIDR", ".spec.cidr", "string"),
            col("VNet", ".spec.vnet", "string"),
            col("Pods", ".spec._podCount", "integer"),
            col("Services", ".spec._serviceCount", "integer"),
            col("Age", ".metadata.creationTimestamp", "date"),
        ],
        "nsgs" => vec![
            col("Targets", ".spec._targets", "string"),
            col("Rules", ".spec._ruleCount", "integer"),
            col("Allows", ".spec._allowCount", "integer"),
            col("Denies", ".spec._denyCount", "integer"),
            col("Age", ".metadata.creationTimestamp", "date"),
        ],
        "routetables" => vec![
            col("Rules", ".spec._ruleCount", "integer"),
            col("Allows", ".spec._allowCount", "integer"),
            col("Denies", ".spec._denyCount", "integer"),
            col("Methods", ".spec._methods", "string"),
            col("Age", ".metadata.creationTimestamp", "date"),
        ],
        _ => return,
    };
    if !cols.is_empty() {
        obj.insert(
            "additionalPrinterColumns".to_string(),
            serde_json::Value::Array(cols),
        );
    }
}

pub fn api_groups_from_catalog() -> Vec<crate::types::APIGroup> {
    use crate::api::server::gvd;
    use crate::types::APIGroup;

    let mut groups = vec![
        APIGroup {
            name: "rbac.authorization.k8s.io".into(),
            versions: vec![gvd("rbac.authorization.k8s.io/v1", "v1")],
            preferred_version: Some(gvd("rbac.authorization.k8s.io/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
        APIGroup {
            name: "apps".into(),
            versions: vec![gvd("apps/v1", "v1")],
            preferred_version: Some(gvd("apps/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
        APIGroup {
            name: "authorization.k8s.io".into(),
            versions: vec![gvd("authorization.k8s.io/v1", "v1")],
            preferred_version: Some(gvd("authorization.k8s.io/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
        APIGroup {
            name: "discovery.k8s.io".into(),
            versions: vec![gvd("discovery.k8s.io/v1", "v1")],
            preferred_version: Some(gvd("discovery.k8s.io/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
        APIGroup {
            name: "networking.k8s.io".into(),
            versions: vec![gvd("networking.k8s.io/v1", "v1")],
            preferred_version: Some(gvd("networking.k8s.io/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
        APIGroup {
            name: "storage.k8s.io".into(),
            versions: vec![gvd("storage.k8s.io/v1", "v1")],
            preferred_version: Some(gvd("storage.k8s.io/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
        APIGroup {
            name: "z8s.io".into(),
            versions: vec![gvd("z8s.io/v1", "v1")],
            preferred_version: Some(gvd("z8s.io/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
        APIGroup {
            name: "apiextensions.k8s.io".into(),
            versions: vec![gvd("apiextensions.k8s.io/v1", "v1")],
            preferred_version: Some(gvd("apiextensions.k8s.io/v1", "v1")),
            server_address_by_client_cidrs: None,
        },
    ];
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}
