use axum::Router;
use axum::routing::get;
use crate::api::server::*;
use k8s_openapi::api::core::v1::PersistentVolume;

pub async fn list_pvs(State(state): State<AppState>) -> Json<List<PersistentVolume>> {
    let items: Vec<PersistentVolume> = state.store.get_by_kind("PersistentVolume").await
        .into_iter()
        .filter_map(|t| match t.resource {
            AnyResource::PersistentVolume(pv) => Some(pv),
            _ => None,
        })
        .collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn get_pv(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<PersistentVolume>, ApiError> {
    state.store.get_by_kind("PersistentVolume").await
        .into_iter()
        .find(|t| t.resource.name() == name)
        .and_then(|t| match t.resource {
            AnyResource::PersistentVolume(pv) => Some(Json(pv)),
            _ => None,
        })
        .ok_or_else(|| ApiError::not_found(format!("persistentvolume \"{}\" not found", name)))
}

pub async fn create_pv(
    State(state): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut pv: PersistentVolume = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid PersistentVolume: {}", e)))?;
    if pv.metadata.uid.is_none() {
        pv.metadata.uid = Some(uuid::Uuid::new_v4().to_string());
    }
    if pv.metadata.creation_timestamp.is_none() {
        pv.metadata.creation_timestamp = Some(now_time());
    }
    let resource = AnyResource::PersistentVolume(pv);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.registry.on_apply(&state.ctx, &resource).await;
    Ok((StatusCode::CREATED, Json(resource)).into_response())
}

pub async fn delete_pv(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("PersistentVolume").await;
    for t in &trackers {
        if t.resource.name() == name {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("persistentvolume \"{}\" not found", name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/persistentvolumes", get(list_pvs).post(create_pv))
        .route("/api/v1/persistentvolumes/{name}", get(get_pv).delete(delete_pv))
}
