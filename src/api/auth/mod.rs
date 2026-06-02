//! RBAC authorization engine (R1–R2).

use axum::http::HeaderMap;

use crate::store::{AnyResource, StoreBackend};
use crate::types::{PolicyRule, Role, RoleBinding};

#[derive(Debug, Clone)]
pub struct AuthzRequest<'a> {
    pub user: &'a str,
    pub namespace: &'a str,
    pub resource: &'a str,
    pub verb: &'a str,
    pub api_group: &'a str,
    pub name: Option<&'a str>,
}

pub fn extract_user(headers: &HeaderMap) -> String {
    if let Some(user) = headers.get("X-Remote-User") {
        if let Ok(s) = user.to_str() {
            return s.to_string();
        }
    }
    if let Some(auth) = headers.get("Authorization") {
        if let Ok(s) = auth.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                return token.to_string();
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
        | "events" => "",
        "deployments" | "replicasets" | "daemonsets" | "statefulsets" | "jobs" | "cronjobs" => {
            "apps"
        }
        "ingresses" | "networkpolicies" => "networking.k8s.io",
        "roles" | "rolebindings" => "rbac.authorization.k8s.io",
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

pub async fn has_any_role_binding(store: &dyn StoreBackend) -> bool {
    !store.get_by_kind("RoleBinding").await.is_empty()
}

pub async fn authorize(store: &dyn StoreBackend, req: &AuthzRequest<'_>) -> bool {
    let bindings = store.get_by_kind("RoleBinding").await;
    let roles = store.get_by_kind("Role").await;

    let mut role_map: std::collections::HashMap<(String, String), &Role> =
        std::collections::HashMap::new();
    for t in &roles {
        if let AnyResource::Role(role) = &t.resource {
            let ns = role.metadata.namespace.as_deref().unwrap_or("default");
            let name = role.metadata.name.as_deref().unwrap_or("");
            role_map.insert((ns.to_string(), name.to_string()), role);
        }
    }

    for t in &bindings {
        let AnyResource::RoleBinding(rb) = &t.resource else {
            continue;
        };
        let rb_ns = rb.metadata.namespace.as_deref().unwrap_or("default");
        if rb_ns != req.namespace {
            continue;
        }
        if !subject_matches_user(&rb.subjects, req.user) {
            continue;
        }
        let role_key = (rb_ns.to_string(), rb.role_ref.name.clone());
        let Some(role) = role_map.get(&role_key) else {
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

fn subject_matches_user(subjects: &[crate::types::Subject], user: &str) -> bool {
    subjects.iter().any(|s| match s.kind.as_str() {
        "User" => s.name == user,
        "ServiceAccount" => {
            let sa = format!("system:serviceaccount:{}:{}", s.namespace, s.name);
            user == sa || user == s.name
        }
        _ => false,
    })
}
