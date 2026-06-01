use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn list_vnets(
    headers: axum::http::HeaderMap,
    State(s): State<AppState>,
) -> axum::response::Response {
    let mut resp = crate::api::handlers::crd::generic_list(&s, "VNet", "VNetList").await;
    let subnets = s.store.get_by_kind("Subnet").await;
    let pods = s.store.get_by_kind("Pod").await;
    let svcs = s.store.get_by_kind("Service").await;
    if let Some(items) = resp.0.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            let name = item["metadata"]["name"].as_str().unwrap_or("");
            let sc = subnets
                .iter()
                .filter(|t| {
                    if let AnyResource::Subnet(s) = &t.resource {
                        s.spec.vnet == name
                    } else {
                        false
                    }
                })
                .count();
            item["spec"]["_subnetCount"] = serde_json::json!(sc);
            item["spec"]["_podCount"] = serde_json::json!(pods.len());
            item["spec"]["_serviceCount"] = serde_json::json!(svcs.len());
        }
    }
    // Return Table format when kubectl requests it (enables custom columns)
    if crate::api::handlers::crd::wants_table(&headers) {
        let cols = &[
            ("CIDR", ".spec.cidr", "string"),
            ("Role", ".spec.role", "string"),
            ("Internet", ".spec.internet_access", "boolean"),
            ("Subnets", ".spec._subnetCount", "integer"),
            ("Pods", ".spec._podCount", "integer"),
            ("Services", ".spec._serviceCount", "integer"),
        ];
        return (
            axum::http::StatusCode::OK,
            Json(build_table(
                resp.0["items"].as_array().unwrap_or(&vec![]),
                cols,
            )),
        )
            .into_response();
    }
    (axum::http::StatusCode::OK, resp).into_response()
}
pub async fn create_vnet(
    State(s): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::types::VNet =
        serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::VNet(r), "VNet").await
}
pub async fn get_vnet(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "VNet", &n).await
}
pub async fn delete_vnet(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "VNet", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/vnets", get(list_vnets).post(create_vnet))
        .route(
            "/apis/z8s.io/v1/vnets/{name}",
            get(get_vnet).delete(delete_vnet),
        )
}
