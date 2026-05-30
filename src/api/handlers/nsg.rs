use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_nsgs(State(s): State<AppState>) -> Json<serde_json::Value> {
    crate::api::handlers::crd::generic_list(&s, "NSG", "NSGList").await
}
pub async fn create_nsg(State(s): State<AppState>, raw: axum::body::Bytes) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::netmux::crds::Nsg = serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::Nsg(r), "NSG").await
}
pub async fn get_nsg(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "NSG", &n).await
}
pub async fn delete_nsg(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "NSG", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/nsgs", get(list_nsgs).post(create_nsg))
        .route("/apis/z8s.io/v1/nsgs/{name}", get(get_nsg).delete(delete_nsg))
}
