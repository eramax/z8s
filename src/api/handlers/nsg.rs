use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn list_nsgs(
    headers: axum::http::HeaderMap,
    State(s): State<AppState>,
) -> axum::response::Response {
    let mut resp = crate::api::handlers::crd::generic_list(&s, "NSG", "NSGList").await;
    if let Some(items) = resp.0.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            let rules = item["spec"]["rules"].as_array().map_or(0, |a| a.len());
            let targets = item["spec"]["target_vnets"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let allows = item["spec"]["rules"].as_array().map_or(0, |a| {
                a.iter()
                    .filter(|r| r["action"].as_str() == Some("allow"))
                    .count()
            });
            let denies = item["spec"]["rules"].as_array().map_or(0, |a| {
                a.iter()
                    .filter(|r| r["action"].as_str() == Some("deny"))
                    .count()
            });
            // Default deny rule added by backend (not in spec)
            let implicit_deny = if allows > 0 || denies > 0 { 1 } else { 0 };
            item["spec"]["_ruleCount"] = serde_json::json!(rules);
            item["spec"]["_allowCount"] = serde_json::json!(allows);
            item["spec"]["_denyCount"] = serde_json::json!(denies + implicit_deny);
            item["spec"]["_targets"] = serde_json::json!(targets);
        }
    }
    if crate::api::handlers::crd::wants_table(&headers) {
        let empty = vec![];
        let cols = &[
            ("Targets", ".spec._targets", "string"),
            ("Rules", ".spec._ruleCount", "integer"),
            ("Allows", ".spec._allowCount", "integer"),
            ("Denies", ".spec._denyCount", "integer"),
        ];
        return (
            StatusCode::OK,
            Json(build_table(
                resp.0["items"].as_array().unwrap_or(&empty),
                cols,
            )),
        )
            .into_response();
    }
    (StatusCode::OK, resp).into_response()
}
pub async fn create_nsg(
    State(s): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::types::Nsg =
        serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::Nsg(r), "NSG").await
}
pub async fn get_nsg(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "NSG", &n).await
}
pub async fn delete_nsg(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "NSG", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/nsgs", get(list_nsgs).post(create_nsg))
        .route(
            "/apis/z8s.io/v1/nsgs/{name}",
            get(get_nsg).delete(delete_nsg),
        )
}
