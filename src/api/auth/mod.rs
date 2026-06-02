//! RBAC authorization engine (R1–R3) and ServiceAccount tokens (R4).

pub mod apply;
pub mod token;

use axum::http::HeaderMap;
use std::collections::HashMap;

use crate::store::{AnyResource, StoreBackend};
use crate::types::{ClusterRole, ClusterRoleBinding, PolicyRule, Role, RoleBinding, Subject};

pub use token::{
    append_service_account_volumes, ServiceAccountMount, TokenRegistry, SA_CONTAINER_PATH,
};

#[derive(Debug, Clone)]
pub struct AuthzRequest<'a> {
    pub user: &'a str,
    pub namespace: &'a str,
    pub resource: &'a str,
    pub verb: &'a str,
    pub api_group: &'a str,
    pub name: Option<&'a str>,
}

pub async fn extract_user(headers: &HeaderMap, tokens: Option<&TokenRegistry>) -> String {
    if let Some(user) = headers.get("X-Remote-User") {
        if let Ok(s) = user.to_str() {
            return s.to_string();
        }
    }
    if let Some(auth) = headers.get("Authorization") {
        if let Ok(s) = auth.to_str() {
            if let Some(raw) = s.strip_prefix("Bearer ") {
                if let Some(reg) = tokens {
                    if let Some(sa_user) = reg.resolve_user(raw).await {
                        return sa_user;
                    }
                }
                return raw.to_string();
            }
        }
    }
    "anonymous".to_string()
}

pub fn verbs_for_http(method: &str, uri: &str) -> Vec<&'static str> {
    match method {
        "GET" | "HEAD" => {
            if uri.contains("watch=1") || uri.contains("watch=true") {
                return vec!["watch"];
            }
            if is_collection_path(uri) {
                vec!["list", "get"]
            } else {
                vec!["get", "list"]
            }
        }
        "POST" => vec!["create"],
        "PUT" => vec!["update"],
        "PATCH" => vec!["patch"],
        "DELETE" => vec!["delete"],
        _ => vec!["get"],
    }
}

fn is_collection_path(uri: &str) -> bool {
    let path = uri.split('?').next().unwrap_or(uri).trim_end_matches('/');
    let last = path.rsplit('/').next().unwrap_or("");
    matches!(
        last,
        "pods"
            | "services"
            | "configmaps"
            | "secrets"
            | "persistentvolumeclaims"
            | "persistentvolumes"
            | "nodes"
            | "namespaces"
            | "deployments"
            | "replicasets"
            | "daemonsets"
            | "statefulsets"
            | "jobs"
            | "cronjobs"
            | "ingresses"
            | "networkpolicies"
            | "events"
            | "endpoints"
            | "endpointslices"
            | "roles"
            | "rolebindings"
            | "clusterroles"
            | "clusterrolebindings"
            | "serviceaccounts"
            | "vnets"
            | "subnets"
            | "nsgs"
            | "routetables"
            | "storageclasses"
    )
}

pub fn api_group_for_resource(resource: &str) -> &'static str {
    match resource {
        "pods"
        | "services"
        | "configmaps"
        | "secrets"
        | "persistentvolumeclaims"
        | "persistentvolumes"
        | "namespaces"
        | "nodes"
        | "endpoints"
        | "endpointslices"
        | "events"
        | "serviceaccounts" => "",
        "deployments" | "replicasets" | "daemonsets" | "statefulsets" | "jobs" | "cronjobs" => {
            "apps"
        }
        "ingresses" | "networkpolicies" => "networking.k8s.io",
        "roles" | "rolebindings" | "clusterroles" | "clusterrolebindings" => {
            "rbac.authorization.k8s.io"
        }
        "pods/exec" | "pods/log" => "",
        "deployments/scale" => "apps",
        "vnets" | "subnets" | "nsgs" | "routetables" => "z8s.io",
        "storageclasses" => "storage.k8s.io",
        _ => "",
    }
}

pub fn rule_allows(rule: &PolicyRule, req: &AuthzRequest<'_>) -> bool {
    if !verbs_match(&rule.verbs, req.verb) {
        return false;
    }
    if !api_groups_match(&rule.api_groups, req.api_group) {
        return false;
    }
    if !resources_match(&rule.resources, req.resource) {
        return false;
    }
    if !resource_names_match(&rule.resource_names, req.name) {
        return false;
    }
    true
}

fn verbs_match(rule_verbs: &[String], verb: &str) -> bool {
    rule_verbs.iter().any(|v| v == "*" || v == verb)
}

fn api_groups_match(rule_groups: &[String], api_group: &str) -> bool {
    if rule_groups.is_empty() {
        return true;
    }
    rule_groups.iter().any(|g| g == "*" || g == api_group)
}

fn resources_match(rule_resources: &[String], resource: &str) -> bool {
    rule_resources.iter().any(|r| {
        r == "*"
            || r.as_str() == resource
            || r == &format!("{resource}s")
            || (resource.ends_with('s') && r.as_str() == &resource[..resource.len() - 1])
    })
}

fn resource_names_match(rule_names: &[String], name: Option<&str>) -> bool {
    if rule_names.is_empty() {
        return true;
    }
    let Some(name) = name else {
        return false;
    };
    rule_names.iter().any(|n| n == name)
}

pub async fn has_any_rbac_policy(store: &dyn StoreBackend) -> bool {
    !store.get_by_kind("RoleBinding").await.is_empty()
        || !store.get_by_kind("ClusterRoleBinding").await.is_empty()
}

/// Back-compat alias.
pub async fn has_any_role_binding(store: &dyn StoreBackend) -> bool {
    has_any_rbac_policy(store).await
}

pub async fn authorize(store: &dyn StoreBackend, req: &AuthzRequest<'_>) -> bool {
    let roles = load_namespace_roles(store).await;
    let cluster_roles = load_cluster_roles(store).await;

    if authorize_role_bindings(store, req, &roles, &cluster_roles).await {
        return true;
    }
    authorize_cluster_role_bindings(store, req, &cluster_roles).await
}

async fn load_namespace_roles(store: &dyn StoreBackend) -> HashMap<(String, String), Role> {
    let mut role_map = HashMap::new();
    for t in store.get_by_kind("Role").await {
        if let AnyResource::Role(role) = t.resource {
            let ns = role.metadata.namespace.as_deref().unwrap_or("default");
            let name = role.metadata.name.as_deref().unwrap_or("");
            role_map.insert((ns.to_string(), name.to_string()), role);
        }
    }
    role_map
}

async fn load_cluster_roles(store: &dyn StoreBackend) -> HashMap<String, ClusterRole> {
    let mut map = HashMap::new();
    for t in store.get_by_kind("ClusterRole").await {
        if let AnyResource::ClusterRole(role) = t.resource {
            let name = role.metadata.name.as_deref().unwrap_or("").to_string();
            map.insert(name, role);
        }
    }
    map
}

async fn authorize_role_bindings(
    store: &dyn StoreBackend,
    req: &AuthzRequest<'_>,
    roles: &HashMap<(String, String), Role>,
    cluster_roles: &HashMap<String, ClusterRole>,
) -> bool {
    for t in store.get_by_kind("RoleBinding").await {
        let AnyResource::RoleBinding(rb) = t.resource else {
            continue;
        };
        let rb_ns = rb.metadata.namespace.as_deref().unwrap_or("default");
        if rb_ns != req.namespace {
            continue;
        }
        if !subject_matches_user(&rb.subjects, req.user, rb_ns) {
            continue;
        }
        match rb.role_ref.kind.as_str() {
            "Role" => {
                let key = (rb_ns.to_string(), rb.role_ref.name.clone());
                if let Some(role) = roles.get(&key) {
                    for rule in &role.rules {
                        if rule_allows(rule, req) {
                            return true;
                        }
                    }
                }
            }
            "ClusterRole" => {
                if let Some(role) = cluster_roles.get(&rb.role_ref.name) {
                    for rule in &role.rules {
                        if rule_allows(rule, req) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

async fn authorize_cluster_role_bindings(
    store: &dyn StoreBackend,
    req: &AuthzRequest<'_>,
    cluster_roles: &HashMap<String, ClusterRole>,
) -> bool {
    for t in store.get_by_kind("ClusterRoleBinding").await {
        let AnyResource::ClusterRoleBinding(crb) = t.resource else {
            continue;
        };
        if !subject_matches_user(&crb.subjects, req.user, req.namespace) {
            continue;
        }
        if crb.role_ref.kind != "ClusterRole" {
            continue;
        }
        let Some(role) = cluster_roles.get(&crb.role_ref.name) else {
            continue;
        };
        for rule in &role.rules {
            if rule_allows(rule, req) {
                return true;
            }
        }
    }
    false
}

fn subject_matches_user(subjects: &[Subject], user: &str, default_ns: &str) -> bool {
    subjects.iter().any(|s| match s.kind.as_str() {
        "User" => s.name == user,
        "ServiceAccount" => {
            let ns = if s.namespace.is_empty() {
                default_ns
            } else {
                &s.namespace
            };
            let sa = format!("system:serviceaccount:{ns}:{}", s.name);
            user == sa || user == s.name
        }
        "Group" => user == s.name,
        _ => false,
    })
}
