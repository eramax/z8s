use crate::api::server::*;
use crate::types::PersistentVolumeClaim;
use axum::Router;
use axum::routing::get;

pub async fn list_pvcs_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "PersistentVolumeClaim", "PersistentVolumeClaimList", None)
        .await
        .into_response()
}

pub async fn list_pvcs(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "PersistentVolumeClaim", "PersistentVolumeClaimList", Some(&ns))
        .await
        .into_response()
}

pub async fn get_pvc(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get_namespaced(&s, "PersistentVolumeClaim", &ns, &name).await
}

pub async fn create_pvc(
    State(s): State<AppState>,
    Path(ns): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let pvc: PersistentVolumeClaim = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid PersistentVolumeClaim: {}", e)))?;
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::PersistentVolumeClaim(pvc), &ns, "PersistentVolumeClaim").await
}

pub async fn delete_pvc(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "PersistentVolumeClaim", &ns, &name).await
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/persistentvolumeclaims", get(list_pvcs_all))
        .route(
            "/api/v1/namespaces/{namespace}/persistentvolumeclaims",
            get(list_pvcs).post(create_pvc),
        )
        .route(
            "/api/v1/namespaces/{namespace}/persistentvolumeclaims/{name}",
            get(get_pvc).delete(delete_pvc),
        )
}
