use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_endpoints_all(State(state): State<AppState>) -> Json<List<k8s_openapi::api::core::v1::Endpoints>> {
    list_endpoints_ns(&state, None).await
}

pub async fn list_endpoints(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<k8s_openapi::api::core::v1::Endpoints>> {
    list_endpoints_ns(&state, Some(namespace)).await
}

pub async fn list_endpoints_ns(state: &AppState, namespace: Option<String>) -> Json<List<k8s_openapi::api::core::v1::Endpoints>> {
    let svc_trackers = state.store.get_by_kind("Service").await;
    let mut items = Vec::new();
    for t in &svc_trackers {
        if namespace.as_deref().map_or(false, |ns| t.resource.namespace() != ns) { continue; }
        if let AnyResource::Service(svc) = &t.resource {
            items.push(state.ctx.net.compute_endpoints(svc).await);
        }
    }
    Json(List { items, metadata: make_list_meta() })
}

pub async fn get_endpoints(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<k8s_openapi::api::core::v1::Endpoints>, ApiError> {
    let trackers = state.store.get_by_kind("Service").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            if let AnyResource::Service(svc) = &t.resource {
                return Ok(Json(state.ctx.net.compute_endpoints(svc).await));
            }
        }
    }
    Err(ApiError::not_found(format!("endpoints \"{}/{}\" not found", namespace, name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/endpoints", get(list_endpoints_all))
        .route("/api/v1/namespaces/{namespace}/endpoints", get(list_endpoints))
        .route("/api/v1/namespaces/{namespace}/endpoints/{name}", get(get_endpoints))
}
