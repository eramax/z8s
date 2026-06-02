use crate::api::auth::{self, api_group_for_resource, verbs_for_http, AuthzRequest};
use crate::api::server::*;
use crate::types::{Role, RoleBinding};
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;

fn is_exempt_path(uri: &str) -> bool {
    matches!(
        uri,
        "/healthz" | "/readyz" | "/livez" | "/version" | "/openapi/v2" | "/openapi/v3"
    ) || uri.starts_with("/openapi/")
}

/// Map URI path to (resource, namespace, optional resource name).
fn uri_to_resource(uri: &str) -> Option<(&str, &str, Option<&str>)> {
    let path = uri.split('?').next().unwrap_or(uri);
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() >= 4 && parts[0] == "apis" && parts[1] == "rbac.authorization.k8s.io" {
        return match parts.get(3).copied() {
            Some("roles") => Some(("roles", "", None)),
            Some("rolebindings") => Some(("rolebindings", "", None)),
            _ => None,
        };
    }

    if parts.len() >= 6 && parts[0] == "apis" && parts[3] == "namespaces" {
        let ns = parts[4];
        let resource = parts.get(5).copied()?;
        let name = parts.get(6).copied();
        return Some((resource, ns, name));
    }

    if parts.len() >= 4 && parts[0] == "api" && parts[1] == "v1" && parts[2] == "namespaces" {
        let ns = parts[3];
        let resource = parts.get(4).copied()?;
        let name = parts.get(5).copied();
        return Some((resource, ns, name));
    }

    if parts.len() >= 4 && parts[0] == "api" && parts[1] == "v1" {
        let resource = parts[2];
        let name = parts.get(3).copied();
        return Some((resource, "", name));
    }

    if parts.len() >= 4 && parts[0] == "apis" && parts[1] == "z8s.io" {
        let resource = parts[3];
        return Some((resource, "", parts.get(4).copied()));
    }

    None
}

pub async fn authorize_middleware_with_store(
    headers: HeaderMap,
    request: Request,
    next: Next,
    store: Arc<dyn StoreBackend>,
) -> Result<Response, StatusCode> {
    let method = request.method().as_str().to_string();
    let uri = request.uri().path().to_string();

    if is_exempt_path(&uri) {
        return Ok(next.run(request).await);
    }

    if !auth::has_any_role_binding(store.as_ref()).await {
        return Ok(next.run(request).await);
    }

    let Some((resource, namespace, name)) = uri_to_resource(&uri) else {
        return Ok(next.run(request).await);
    };

    let user = auth::extract_user(&headers);
    let api_group = api_group_for_resource(resource);
    let verbs = verbs_for_http(&method, &uri);

    for verb in verbs {
        let req = AuthzRequest {
            user: &user,
            namespace,
            resource,
            verb,
            api_group,
            name,
        };
        if auth::authorize(store.as_ref(), &req).await {
            return Ok(next.run(request).await);
        }
    }

    tracing::warn!(
        "RBAC denied: user='{}' uri='{}' resource='{}' ns='{}'",
        user,
        uri,
        resource,
        namespace
    );
    Err(StatusCode::FORBIDDEN)
}

pub async fn authorize(
    store: &dyn StoreBackend,
    user: &str,
    namespace: &str,
    resource: &str,
    verb: &str,
) -> bool {
    let req = AuthzRequest {
        user,
        namespace,
        resource,
        verb,
        api_group: api_group_for_resource(resource),
        name: None,
    };
    auth::authorize(store, &req).await
}

// ── Role CRUD ──────────────────────────────────────────────────────

pub async fn list_roles_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list(&s, "Role", "RoleList")
        .await
        .into_response()
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
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::Role(role), &ns, "Role")
        .await
}

pub async fn delete_role(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "Role", &ns, &name).await
}

pub async fn list_rolebindings_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list(&s, "RoleBinding", "RoleBindingList")
        .await
        .into_response()
}

pub async fn list_rolebindings(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(
        &s,
        "RoleBinding",
        "RoleBindingList",
        Some(&ns),
    )
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
    crate::api::handlers::crd::generic_create_namespaced(
        &s,
        AnyResource::RoleBinding(rb),
        &ns,
        "RoleBinding",
    )
    .await
}

pub async fn delete_rolebinding(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "RoleBinding", &ns, &name).await
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/apis/rbac.authorization.k8s.io/v1/roles",
            get(list_roles_all),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/rolebindings",
            get(list_rolebindings_all),
        )
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
