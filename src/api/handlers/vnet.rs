use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_vnets(State(s): State<AppState>) -> Json<serde_json::Value> {
    crate::api::handlers::crd::generic_list(&s, "VNet", "VNetList").await
}
pub async fn create_vnet(State(s): State<AppState>, raw: axum::body::Bytes) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::netmux::crds::VNet = serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::VNet(r), "VNet").await
}
pub async fn get_vnet(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "VNet", &n).await
}
pub async fn delete_vnet(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "VNet", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/vnets", get(list_vnets).post(create_vnet))
        .route("/apis/z8s.io/v1/vnets/{name}", get(get_vnet).delete(delete_vnet))
}
