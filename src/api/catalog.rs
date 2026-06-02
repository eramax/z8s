//! Resource catalog — single source of truth for kinds, plurals, and path resolution (A1).

use crate::components::ResourceCategory;

#[derive(Debug, Clone, Copy)]
pub struct ResourceEntry {
    pub kind: &'static str,
    pub plural: &'static str,
    pub list_kind: &'static str,
    pub namespaced: bool,
    pub category: ResourceCategory,
    /// `apiVersion` on list responses for the canonical z8s.io path.
    pub list_api_version: &'static str,
    /// RBAC API group for PolicyRule matching.
    pub rbac_api_group: &'static str,
}

/// Flags for generic `{plural}` compat routes under a path prefix.
#[derive(Debug, Clone, Copy)]
pub struct CompatMountFlags {
    pub cluster_list: bool,
    pub cluster_item: bool,
    pub cluster_create: bool,
    pub namespaced: bool,
}

/// Upstream API path prefix served by `resource_handler` + catalog dispatch.
#[derive(Debug, Clone, Copy)]
pub struct CompatMount {
    pub prefix: &'static str,
    pub flags: CompatMountFlags,
}

/// Compat URL mounts (not per-kind — plural is a path parameter).
pub const COMPAT_MOUNTS: &[CompatMount] = &[
    CompatMount {
        prefix: "/api/v1",
        flags: CompatMountFlags {
            cluster_list: true,
            cluster_item: true,
            cluster_create: true,
            namespaced: true,
        },
    },
    CompatMount {
        prefix: "/apis/z8s.io/v1",
        flags: CompatMountFlags {
            cluster_list: true,
            cluster_item: true,
            cluster_create: true,
            namespaced: false,
        },
    },
    CompatMount {
        prefix: "/apis/apps/v1",
        flags: CompatMountFlags {
            cluster_list: true,
            cluster_item: false,
            cluster_create: false,
            namespaced: true,
        },
    },
    CompatMount {
        prefix: "/apis/networking.k8s.io/v1",
        flags: CompatMountFlags {
            cluster_list: false,
            cluster_item: false,
            cluster_create: false,
            namespaced: true,
        },
    },
    CompatMount {
        prefix: "/apis/discovery.k8s.io/v1",
        flags: CompatMountFlags {
            cluster_list: false,
            cluster_item: false,
            cluster_create: false,
            namespaced: true,
        },
    },
];

macro_rules! entry {
    ($kind:expr, $plural:expr, $list:expr, $ns:expr, $cat:expr, $wire:expr, $rbac:expr) => {
        ResourceEntry {
            kind: $kind,
            plural: $plural,
            list_kind: $list,
            namespaced: $ns,
            category: $cat,
            list_api_version: $wire,
            rbac_api_group: $rbac,
        }
    };
}

pub const CATALOG: &[ResourceEntry] = &[
    entry!("Pod", "pods", "PodList", true, ResourceCategory::Compute, "v1", ""),
    entry!(
        "Deployment",
        "deployments",
        "DeploymentList",
        true,
        ResourceCategory::Compute,
        "apps/v1",
        "apps"
    ),
    entry!(
        "Service",
        "services",
        "ServiceList",
        true,
        ResourceCategory::Network,
        "v1",
        ""
    ),
    entry!(
        "ConfigMap",
        "configmaps",
        "ConfigMapList",
        true,
        ResourceCategory::Storage,
        "v1",
        ""
    ),
    entry!(
        "Secret",
        "secrets",
        "SecretList",
        true,
        ResourceCategory::Storage,
        "v1",
        ""
    ),
    entry!(
        "Namespace",
        "namespaces",
        "NamespaceList",
        false,
        ResourceCategory::Compute,
        "v1",
        ""
    ),
    entry!(
        "Node",
        "nodes",
        "NodeList",
        false,
        ResourceCategory::Compute,
        "v1",
        ""
    ),
    entry!(
        "PersistentVolume",
        "persistentvolumes",
        "PersistentVolumeList",
        false,
        ResourceCategory::Storage,
        "v1",
        ""
    ),
    entry!(
        "PersistentVolumeClaim",
        "persistentvolumeclaims",
        "PersistentVolumeClaimList",
        true,
        ResourceCategory::Storage,
        "v1",
        ""
    ),
    entry!(
        "StorageClass",
        "storageclasses",
        "StorageClassList",
        false,
        ResourceCategory::Storage,
        "storage.k8s.io/v1",
        "storage.k8s.io"
    ),
    entry!(
        "Ingress",
        "ingresses",
        "IngressList",
        true,
        ResourceCategory::Network,
        "networking.k8s.io/v1",
        "networking.k8s.io"
    ),
    entry!(
        "NetworkPolicy",
        "networkpolicies",
        "NetworkPolicyList",
        true,
        ResourceCategory::Network,
        "networking.k8s.io/v1",
        "networking.k8s.io"
    ),
    entry!(
        "Endpoints",
        "endpoints",
        "EndpointsList",
        true,
        ResourceCategory::Network,
        "v1",
        ""
    ),
    entry!(
        "EndpointSlice",
        "endpointslices",
        "EndpointSliceList",
        true,
        ResourceCategory::Network,
        "discovery.k8s.io/v1",
        "discovery.k8s.io"
    ),
    entry!(
        "Event",
        "events",
        "EventList",
        true,
        ResourceCategory::Storage,
        "v1",
        ""
    ),
    entry!(
        "VNet",
        "vnets",
        "VNetList",
        false,
        ResourceCategory::Network,
        "z8s.io/v1",
        "z8s.io"
    ),
    entry!(
        "Subnet",
        "subnets",
        "SubnetList",
        false,
        ResourceCategory::Network,
        "z8s.io/v1",
        "z8s.io"
    ),
    entry!(
        "NSG",
        "nsgs",
        "NSGList",
        false,
        ResourceCategory::Network,
        "z8s.io/v1",
        "z8s.io"
    ),
    entry!(
        "RouteTable",
        "routetables",
        "RouteTableList",
        false,
        ResourceCategory::Network,
        "z8s.io/v1",
        "z8s.io"
    ),
    entry!(
        "Role",
        "roles",
        "RoleList",
        true,
        ResourceCategory::Compute,
        "rbac.authorization.k8s.io/v1",
        "rbac.authorization.k8s.io"
    ),
    entry!(
        "RoleBinding",
        "rolebindings",
        "RoleBindingList",
        true,
        ResourceCategory::Compute,
        "rbac.authorization.k8s.io/v1",
        "rbac.authorization.k8s.io"
    ),
    entry!(
        "ClusterRole",
        "clusterroles",
        "ClusterRoleList",
        false,
        ResourceCategory::Compute,
        "rbac.authorization.k8s.io/v1",
        "rbac.authorization.k8s.io"
    ),
    entry!(
        "ClusterRoleBinding",
        "clusterrolebindings",
        "ClusterRoleBindingList",
        false,
        ResourceCategory::Compute,
        "rbac.authorization.k8s.io/v1",
        "rbac.authorization.k8s.io"
    ),
    entry!(
        "ServiceAccount",
        "serviceaccounts",
        "ServiceAccountList",
        true,
        ResourceCategory::Compute,
        "v1",
        ""
    ),
];

pub fn by_plural(plural: &str) -> Option<&'static ResourceEntry> {
    CATALOG.iter().find(|e| e.plural == plural)
}

pub fn by_kind(kind: &str) -> Option<&'static ResourceEntry> {
    CATALOG.iter().find(|e| e.kind == kind)
}

/// Map request path to RBAC (resource plural, namespace, optional name).
pub fn authz_from_path(uri: &str) -> Option<(&'static str, &str, Option<&str>)> {
    let path = uri.split('?').next().unwrap_or(uri);
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() >= 4 && parts[0] == "apis" && parts[1] == "rbac.authorization.k8s.io" {
        return match parts.get(3).copied() {
            Some(plural) => by_plural(plural).map(|e| (e.plural, "", parts.get(4).copied())),
            _ => None,
        };
    }

    if parts.len() >= 6 && parts[0] == "apis" && parts[3] == "namespaces" {
        let ns = parts[4];
        let plural = parts.get(5).copied()?;
        if plural == "deployments" && parts.get(7) == Some(&"scale") {
            return Some(("deployments/scale", ns, parts.get(6).copied()));
        }
        return by_plural(plural).map(|e| (e.plural, ns, parts.get(6).copied()));
    }

    if parts.len() >= 4 && parts[0] == "apis" && parts[1] == "z8s.io" && parts[2] == "v1" {
        if parts.get(3) == Some(&"apply") {
            return None;
        }
        if parts.len() >= 5 && parts[3] == "namespaces" {
            let ns = parts[4];
            let plural = parts.get(5).copied()?;
            return by_plural(plural).map(|e| (e.plural, ns, parts.get(6).copied()));
        }
        let plural = parts.get(3).copied()?;
        return by_plural(plural).map(|e| (e.plural, "", parts.get(4).copied()));
    }

    if parts.len() >= 5 && parts[0] == "api" && parts[1] == "v1" && parts[2] == "namespaces" {
        let ns = parts[3];
        if parts[4] == "pods" {
            if parts.get(6) == Some(&"exec") {
                return Some(("pods/exec", ns, parts.get(5).copied()));
            }
            if parts.get(6) == Some(&"log") {
                return Some(("pods/log", ns, parts.get(5).copied()));
            }
        }
        let plural = parts.get(4).copied()?;
        return by_plural(plural).map(|e| (e.plural, ns, parts.get(5).copied()));
    }

    if parts.len() == 4 && parts[0] == "api" && parts[1] == "v1" && parts[2] == "namespaces" {
        return Some(("namespaces", "", Some(parts[3])));
    }

    if parts.len() >= 4 && parts[0] == "api" && parts[1] == "v1" {
        if parts[2] == "apply" {
            return None;
        }
        let plural = parts[2];
        return by_plural(plural).map(|e| (e.plural, "", parts.get(3).copied()));
    }

    if parts.len() >= 4
        && parts[0] == "apis"
        && parts[1] == "apps"
        && parts[2] == "v1"
        && parts[3] != "namespaces"
    {
        let plural = parts[3];
        return by_plural(plural).map(|e| (e.plural, "", parts.get(4).copied()));
    }

    if parts.len() >= 5
        && parts[0] == "apis"
        && parts[1] == "networking.k8s.io"
        && parts[2] == "v1"
        && parts[3] == "namespaces"
    {
        let ns = parts[4];
        let plural = parts.get(5).copied()?;
        return by_plural(plural).map(|e| (e.plural, ns, parts.get(6).copied()));
    }

    if parts.len() >= 4 && parts[0] == "apis" && parts[1] == "storage.k8s.io" && parts[2] == "v1" {
        let plural = parts.get(3).copied()?;
        return by_plural(plural).map(|e| (e.plural, "", parts.get(4).copied()));
    }

    None
}

pub fn rbac_api_group_for_plural(plural: &str) -> &'static str {
    by_plural(plural)
        .map(|e| e.rbac_api_group)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_configmap_and_vnet() {
        assert!(by_plural("configmaps").is_some());
        assert!(by_plural("vnets").is_some());
    }

    #[test]
    fn authz_resolves_namespaced_pod() {
        let (r, ns, name) = authz_from_path("/api/v1/namespaces/default/pods/nginx").unwrap();
        assert_eq!(r, "pods");
        assert_eq!(ns, "default");
        assert_eq!(name, Some("nginx"));
    }

    #[test]
    fn authz_resolves_pod_exec() {
        let (r, ns, _) = authz_from_path("/api/v1/namespaces/default/pods/nginx/exec").unwrap();
        assert_eq!(r, "pods/exec");
        assert_eq!(ns, "default");
    }

    #[test]
    fn authz_skips_apply_endpoint() {
        assert!(authz_from_path("/api/v1/apply").is_none());
    }

    #[test]
    fn authz_resolves_apps_deployments_and_scale() {
        let (r, ns, name) =
            authz_from_path("/apis/apps/v1/namespaces/default/deployments/nginx/scale").unwrap();
        assert_eq!(r, "deployments/scale");
        assert_eq!(ns, "default");
        assert_eq!(name, Some("nginx"));
        let (r, ns, _) = authz_from_path("/apis/apps/v1/deployments").unwrap();
        assert_eq!(r, "deployments");
        assert_eq!(ns, "");
    }

    #[test]
    fn authz_resolves_z8s_vnet() {
        let (r, ns, name) = authz_from_path("/apis/z8s.io/v1/vnets/default").unwrap();
        assert_eq!(r, "vnets");
        assert_eq!(ns, "");
        assert_eq!(name, Some("default"));
    }

    #[test]
    fn compat_mounts_non_empty() {
        assert!(!COMPAT_MOUNTS.is_empty());
        assert!(COMPAT_MOUNTS.iter().any(|m| m.prefix == "/api/v1"));
        assert!(COMPAT_MOUNTS.iter().any(|m| m.prefix == "/apis/z8s.io/v1"));
    }

    #[test]
    fn discovery_includes_core_and_z8s() {
        let v1 = crate::api::discovery::resources_for_api_version("v1");
        assert!(v1.iter().any(|r| r.name == "pods"));
        let z8s = crate::api::discovery::resources_for_api_version("z8s.io/v1");
        assert!(z8s.iter().any(|r| r.name == "vnets"));
    }
}
