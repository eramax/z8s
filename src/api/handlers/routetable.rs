use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn list_routetables(
    headers: axum::http::HeaderMap,
    State(s): State<AppState>,
) -> axum::response::Response {
    let mut resp =
        crate::api::handlers::crd::generic_list(&s, "RouteTable", "RouteTableList").await;
    if let Some(items) = resp.0.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            let rules = item["spec"]["rules"].as_array().map_or(0, |a| a.len());
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
            let methods: Vec<String> = item["spec"]["rules"].as_array().map_or(vec![], |a| {
                a.iter()
                    .filter_map(|r| {
                        let action = r["action"].as_str().unwrap_or("");
                        let methods = r["methods"]
                            .as_array()
                            .map(|m| {
                                m.iter()
                                    .filter_map(|m| m.as_str())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_default();
                        if methods.is_empty() {
                            None
                        } else {
                            Some(format!("{action}: {methods}"))
                        }
                    })
                    .collect()
            });
            item["spec"]["_ruleCount"] = serde_json::json!(rules);
            item["spec"]["_allowCount"] = serde_json::json!(allows);
            item["spec"]["_denyCount"] = serde_json::json!(denies);
            item["spec"]["_methods"] = serde_json::json!(methods);
        }
    }
    if crate::api::handlers::crd::wants_table(&headers) {
        let empty = vec![];
        let cols = &[
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
pub async fn create_routetable(
    State(s): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::types::RouteTable =
        serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::RouteTable(r), "RouteTable").await
}
pub async fn get_routetable(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "RouteTable", &n).await
}
pub async fn delete_routetable(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "RouteTable", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/apis/z8s.io/v1/routetables",
            get(list_routetables).post(create_routetable),
        )
        .route(
            "/apis/z8s.io/v1/routetables/{name}",
            get(get_routetable).delete(delete_routetable),
        )
}
