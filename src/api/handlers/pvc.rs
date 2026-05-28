use axum::Router;
use axum::routing::get;
use crate::api::server::*;
use k8s_openapi::api::core::v1::PersistentVolumeClaim;

pub async fn list_pvcs_all(State(state): State<AppState>) -> Json<List<PersistentVolumeClaim>> {
    let items: Vec<PersistentVolumeClaim> = state.store.get_by_kind("PersistentVolumeClaim").await
        .into_iter()
        .filter_map(|t| match t.resource {
            AnyResource::PersistentVolumeClaim(pvc) => Some(pvc),
            _ => None,
        })
        .collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn list_pvcs(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<PersistentVolumeClaim>> {
    let items: Vec<PersistentVolumeClaim> = state.store.get_by_kind("PersistentVolumeClaim").await
        .into_iter()
        .filter(|t| t.resource.namespace() == namespace)
        .filter_map(|t| match t.resource {
            AnyResource::PersistentVolumeClaim(pvc) => Some(pvc),
            _ => None,
        })
        .collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn get_pvc(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<PersistentVolumeClaim>, ApiError> {
    state.store.get_by_kind("PersistentVolumeClaim").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| match t.resource {
            AnyResource::PersistentVolumeClaim(pvc) => Some(Json(pvc)),
            _ => None,
        })
        .ok_or_else(|| ApiError::not_found(format!("persistentvolumeclaim \"{}/{}\" not found", namespace, name)))
}

pub async fn create_pvc(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut pvc: PersistentVolumeClaim = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid PersistentVolumeClaim: {}", e)))?;
    if pvc.metadata.namespace.is_none() {
        pvc.metadata.namespace = Some(namespace);
    }
    if pvc.metadata.uid.is_none() {
        pvc.metadata.uid = Some(uuid::Uuid::new_v4().to_string());
    }
    if pvc.metadata.creation_timestamp.is_none() {
        pvc.metadata.creation_timestamp = Some(now_time());
    }
    let resource = AnyResource::PersistentVolumeClaim(pvc);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.registry.on_apply(&state.ctx, &resource).await;
    Ok((StatusCode::CREATED, Json(resource)).into_response())
}

pub async fn delete_pvc(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("PersistentVolumeClaim").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("persistentvolumeclaim \"{}/{}\" not found", namespace, name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/persistentvolumeclaims", get(list_pvcs_all))
        .route("/api/v1/namespaces/{namespace}/persistentvolumeclaims", get(list_pvcs).post(create_pvc))
        .route("/api/v1/namespaces/{namespace}/persistentvolumeclaims/{name}", get(get_pvc).delete(delete_pvc))
}
