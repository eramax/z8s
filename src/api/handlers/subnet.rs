use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_subnets(State(s): State<AppState>) -> Json<serde_json::Value> {
    crate::api::handlers::crd::generic_list(&s, "Subnet", "SubnetList").await
}
pub async fn create_subnet(State(s): State<AppState>, raw: axum::body::Bytes) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::netmux::crds::Subnet = serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::Subnet(r), "Subnet").await
}
pub async fn get_subnet(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "Subnet", &n).await
}
pub async fn delete_subnet(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "Subnet", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/subnets", get(list_subnets).post(create_subnet))
        .route("/apis/z8s.io/v1/subnets/{name}", get(get_subnet).delete(delete_subnet))
}
