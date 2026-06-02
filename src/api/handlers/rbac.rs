use crate::api::server::*;
use crate::types::{Role, RoleBinding, PolicyRule, Subject};
use axum::Router;
use axum::routing::get;

// ── Role CRUD ──────────────────────────────────────────────────────

pub async fn list_roles_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list(&s, "Role", "RoleList").await.into_response()
}

pub async fn list_roles(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "Role", "RoleList", Some(&ns))
        .await
        .into_response()
}

pub async fn get_role(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get_namespaced(&s, "Role", &ns, &name).await
}

pub async fn create_role(
    State(s): State<AppState>,
    Path(ns): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let role: Role = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Role: {}", e)))?;
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::Role(role), &ns, "Role").await
}

pub async fn delete_role(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "Role", &ns, &name).await
}

// ── RoleBinding CRUD ───────────────────────────────────────────────

pub async fn list_rolebindings_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list(&s, "RoleBinding", "RoleBindingList").await.into_response()
}

pub async fn list_rolebindings(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "RoleBinding", "RoleBindingList", Some(&ns))
        .await
        .into_response()
}

pub async fn get_rolebinding(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get_namespaced(&s, "RoleBinding", &ns, &name).await
}

pub async fn create_rolebinding(
    State(s): State<AppState>,
    Path(ns): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let rb: RoleBinding = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid RoleBinding: {}", e)))?;
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::RoleBinding(rb), &ns, "RoleBinding").await
}

pub async fn delete_rolebinding(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "RoleBinding", &ns, &name).await
}

// ── Authorization check ────────────────────────────────────────────

/// Check if a user is authorized to perform `verb` on `resource` in `namespace`.
/// Walks RoleBindings → Roles → PolicyRules.
pub async fn authorize(
    store: &dyn StoreBackend,
    user: &str,
    namespace: &str,
    resource: &str,
    verb: &str,
) -> bool {
    let bindings = store.get_by_kind("RoleBinding").await;
    let roles = store.get_by_kind("Role").await;

    // Index roles by (namespace, name) for O(1) lookup
    let mut role_map: std::collections::HashMap<(String, String), &Role> = std::collections::HashMap::new();
    for t in &roles {
        if let AnyResource::Role(role) = &t.resource {
            let ns = role.metadata.namespace.as_deref().unwrap_or("default");
            let name = role.metadata.name.as_deref().unwrap_or("");
            role_map.insert((ns.to_string(), name.to_string()), role);
        }
    }

    for t in &bindings {
        if let AnyResource::RoleBinding(rb) = &t.resource {
            let rb_ns = rb.metadata.namespace.as_deref().unwrap_or("default");
            // RoleBinding must be in the same namespace as the request
            if rb_ns != namespace {
                continue;
            }
            // Check if user matches any subject
            let user_matches = rb.subjects.iter().any(|s| {
                match s.kind.as_str() {
                    "User" => s.name == user,
                    "ServiceAccount" => {
                        // ServiceAccount format: "system:serviceaccount:<ns>:<name>"
                        let sa_name = format!("system:serviceaccount:{}:{}", s.namespace, s.name);
                        user == sa_name || user == s.name
                    }
                    _ => false,
                }
            });
            if !user_matches {
                continue;
            }
            // Check if the role grants the requested permission
            if let Some(role) = role_map.get(&(rb.role_ref.name.clone(), namespace.to_string())) {
                for rule in &role.rules {
                    if rule.verbs.contains(&verb.to_string()) && rule.resources.contains(&resource.to_string()) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn routes() -> Router<AppState> {
    Router::new()
        // Cluster-scoped
        .route("/apis/rbac.authorization.k8s.io/v1/roles", get(list_roles_all))
        .route("/apis/rbac.authorization.k8s.io/v1/rolebindings", get(list_rolebindings_all))
        // Namespace-scoped
        .route(
            "/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/roles",
            get(list_roles).post(create_role),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/roles/{name}",
            get(get_role).delete(delete_role),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/rolebindings",
            get(list_rolebindings).post(create_rolebinding),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/rolebindings/{name}",
            get(get_rolebinding).delete(delete_rolebinding),
        )
}
