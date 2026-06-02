use crate::api::server::*;
use crate::types::{Role, RoleBinding, PolicyRule, Subject};
use axum::Router;
use axum::routing::get;
use axum::http::{HeaderMap, StatusCode};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

// ── Authorization middleware ───────────────────────────────────────

/// Map HTTP method to RBAC verb
fn method_to_verb(method: &str) -> &str {
    match method {
        "GET" => "get",
        "POST" => "create",
        "PUT" | "PATCH" => "update",
        "DELETE" => "delete",
        _ => "get",
    }
}

/// Map URI path to resource kind
fn uri_to_resource(uri: &str) -> Option<(&str, &str)> {
    // Returns (resource, namespace) — namespace is "" for cluster-scoped
    let parts: Vec<&str> = uri.trim_start_matches('/').split('/').collect();

    // Cluster-scoped: /apis/rbac.authorization.k8s.io/v1/roles
    if parts.len() >= 4 && parts[0] == "apis" {
        return match parts[3] {
            "roles" => Some(("roles", "")),
            "rolebindings" => Some(("rolebindings", "")),
            _ => None,
        };
    }

    // Namespace-scoped: /apis/rbac.authorization.k8s.io/v1/namespaces/{ns}/roles
    if parts.len() >= 6 && parts[0] == "apis" && parts[3] == "namespaces" {
        let ns = parts[4];
        return match parts[5] {
            "roles" => Some(("roles", ns)),
            "rolebindings" => Some(("rolebindings", ns)),
            _ => None,
        };
    }

    // Standard k8s resources: /api/v1/namespaces/{ns}/pods
    if parts.len() >= 4 && parts[0] == "api" && parts[1] == "v1" && parts[2] == "namespaces" {
        let ns = parts[3];
        return match parts[4] {
            "pods" => Some(("pods", ns)),
            "services" => Some(("services", ns)),
            "configmaps" => Some(("configmaps", ns)),
            "secrets" => Some(("secrets", ns)),
            "persistentvolumeclaims" => Some(("persistentvolumeclaims", ns)),
            "endpoints" => Some(("endpoints", ns)),
            "endpointslices" => Some(("endpointslices", ns)),
            "events" => Some(("events", ns)),
            "namespaces" => Some(("namespaces", "")),
            "persistentvolumes" => Some(("persistentvolumes", "")),
            "nodes" => Some(("nodes", "")),
            "services" => Some(("services", ns)),
            _ => None,
        };
    }

    // /apis/apps/v1/namespaces/{ns}/deployments
    if parts.len() >= 6 && parts[0] == "apis" && parts[2] == "v1" && parts[3] == "namespaces" {
        let ns = parts[4];
        return match parts[5] {
            "deployments" => Some(("deployments", ns)),
            _ => None,
        };
    }

    // /apis/networking.k8s.io/v1/namespaces/{ns}/ingresses
    if parts.len() >= 6 && parts[0] == "apis" && parts[3] == "namespaces" {
        let ns = parts[4];
        return match parts[5] {
            "ingresses" => Some(("ingresses", ns)),
            "networkpolicies" => Some(("networkpolicies", ns)),
            _ => None,
        };
    }

    // CRDs: /apis/z8s.io/v1/vnets
    if parts.len() >= 4 && parts[0] == "apis" && parts[1] == "z8s.io" {
        return match parts[3] {
            "vnets" => Some(("vnets", "")),
            "subnets" => Some(("subnets", "")),
            "nsgs" => Some(("nsgs", "")),
            "routetables" => Some(("routetables", "")),
            _ => None,
        };
    }

    None
}

/// Extract user identity from request headers.
/// Checks: Authorization: Bearer <token>, X-Remote-User, or falls back to "anonymous".
fn extract_user(headers: &HeaderMap) -> String {
    // X-Remote-User header (set by API gateway/proxy)
    if let Some(user) = headers.get("X-Remote-User") {
        if let Ok(s) = user.to_str() {
            return s.to_string();
        }
    }
    // Authorization: Bearer <token> — for now treat as user identity
    if let Some(auth) = headers.get("Authorization") {
        if let Ok(s) = auth.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                return token.to_string();
            }
        }
    }
    "anonymous".to_string()
}

/// Authorization middleware — checks RBAC on mutating requests.
/// Skips enforcement when no RoleBindings exist (RBAC not configured).
pub async fn authorize_middleware_with_store(
    headers: HeaderMap,
    request: Request,
    next: Next,
    store: Arc<dyn StoreBackend>,
) -> Result<Response, StatusCode> {
    let method = request.method().clone();
    let uri = request.uri().path().to_string();

    // Read-only requests are always allowed
    if method == "GET" || method == "HEAD" || method == "OPTIONS" {
        return Ok(next.run(request).await);
    }

    // If no RoleBindings exist, RBAC is not configured — allow everything
    let bindings = store.get_by_kind("RoleBinding").await;
    if bindings.is_empty() {
        return Ok(next.run(request).await);
    }

    // Determine resource and namespace from URI
    let (resource, namespace) = match uri_to_resource(&uri) {
        Some(r) => r,
        None => return Ok(next.run(request).await),
    };

    let verb = method_to_verb(method.as_str());
    let user = extract_user(&headers);

    if !crate::api::handlers::rbac::authorize(store.as_ref(), &user, namespace, resource, verb).await {
        tracing::warn!("RBAC denied: user='{}' verb='{}' resource='{}' ns='{}'", user, verb, resource, namespace);
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(request).await)
}

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
