use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_endpointslices_all(State(state): State<AppState>) -> Json<List<EndpointSlice>> {
    list_endpointslices_ns(&state, None).await
}

pub async fn list_endpointslices(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<EndpointSlice>> {
    list_endpointslices_ns(&state, Some(namespace)).await
}

pub async fn list_endpointslices_ns(state: &AppState, namespace: Option<String>) -> Json<List<EndpointSlice>> {
    let svc_trackers = state.store.get_by_kind("Service").await;
    let mut items = Vec::new();
    for t in &svc_trackers {
        if namespace.as_deref().map_or(false, |ns| t.resource.namespace() != ns) { continue; }
        if let AnyResource::Service(svc) = &t.resource {
            items.extend(state.network.compute_endpointslices(svc).await);
        }
    }
    Json(List { items, metadata: make_list_meta() })
}

pub async fn get_endpointslice(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<EndpointSlice>, ApiError> {
    let trackers = state.store.get_by_kind("Service").await;
    for t in &trackers {
        if t.resource.namespace() != namespace { continue; }
        if let AnyResource::Service(svc) = &t.resource {
            for ep in state.network.compute_endpointslices(svc).await {
                if ep.metadata.name.as_deref() == Some(&name) {
                    return Ok(Json(ep));
                }
            }
        }
    }
    Err(ApiError::not_found(format!("endpointslices \"{}/{}\" not found", namespace, name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/discovery.k8s.io/v1/endpointslices", get(list_endpointslices_all))
        .route("/apis/discovery.k8s.io/v1/namespaces/{namespace}/endpointslices", get(list_endpointslices))
        .route("/apis/discovery.k8s.io/v1/namespaces/{namespace}/endpointslices/{name}", get(get_endpointslice))
}
