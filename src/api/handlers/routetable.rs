use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_routetables(State(s): State<AppState>) -> Json<serde_json::Value> {
    crate::api::handlers::crd::generic_list(&s, "RouteTable", "RouteTableList").await
}
pub async fn create_routetable(State(s): State<AppState>, raw: axum::body::Bytes) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::netmux::crds::RouteTable = serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::RouteTable(r), "RouteTable").await
}
pub async fn get_routetable(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "RouteTable", &n).await
}
pub async fn delete_routetable(State(s): State<AppState>, Path(n): Path<String>) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "RouteTable", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/routetables", get(list_routetables).post(create_routetable))
        .route("/apis/z8s.io/v1/routetables/{name}", get(get_routetable).delete(delete_routetable))
}
