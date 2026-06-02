use crate::api::server::*;
use crate::types::PersistentVolume;
use axum::Router;
use axum::routing::get;

pub async fn list_pvs(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list(&s, "PersistentVolume", "PersistentVolumeList")
        .await
        .into_response()
}

pub async fn get_pv(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "PersistentVolume", &name).await
}

pub async fn create_pv(
    State(s): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let pv: PersistentVolume = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid PersistentVolume: {}", e)))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::PersistentVolume(pv), "PersistentVolume").await
}

pub async fn delete_pv(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "PersistentVolume", &name).await
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/persistentvolumes", get(list_pvs).post(create_pv))
        .route(
            "/api/v1/persistentvolumes/{name}",
            get(get_pv).delete(delete_pv),
        )
}
