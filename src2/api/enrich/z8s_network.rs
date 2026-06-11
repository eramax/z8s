//! z8s.io network kinds: Subnet, NSG, RouteTable list/table views.

use axum::http::HeaderMap;
use axum::response::IntoResponse;

use crate::api::compat::WireContext;
use crate::api::handlers::crd;
use crate::api::server::*;
use crate::api::table::build_table;
async fn list_with_table(
    s: &AppState,
    headers: &HeaderMap,
    wire: &WireContext,
    kind: &str,
    list_kind: &str,
    cols: &[(&str, &str, &str)],
) -> axum::response::Response {
    let resp = crd::generic_list_wire(s, kind, list_kind, &wire.list_api_version).await;
    if crd::wants_table(headers) {
        let empty = Vec::new();
        let items = resp.0["items"].as_array().unwrap_or(&empty);
        return (
            StatusCode::OK,
            Json(build_table(items, cols)),
        )
            .into_response();
    }
    (StatusCode::OK, resp).into_response()
}

pub async fn list_subnets(
    s: &AppState,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    list_with_table(
        s,
        headers,
        wire,
        "Subnet",
        "SubnetList",
        &[
            ("VNet", ".spec.vnet", "string"),
            ("CIDR", ".spec.cidr", "string"),
        ],
    )
    .await
}

pub async fn list_nsgs(
    s: &AppState,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    let mut resp =
        crd::generic_list_wire(s, "NSG", "NSGList", &wire.list_api_version).await;
    if let Some(items) = resp.0.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            let targets = item["spec"]["targetVnets"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0);
            let rules = item["spec"]["rules"].as_array().map(|a| a.len()).unwrap_or(0);
            if let Some(spec) = item.get_mut("spec").and_then(|v| v.as_object_mut()) {
                spec.insert("_targetCount".into(), serde_json::json!(targets));
                spec.insert("_ruleCount".into(), serde_json::json!(rules));
            }
        }
    }
    if crd::wants_table(headers) {
        let cols = &[
            ("Targets", ".spec._targetCount", "integer"),
            ("Rules", ".spec._ruleCount", "integer"),
        ];
        return (
            StatusCode::OK,
            Json(build_table(
                resp.0["items"].as_array().unwrap_or(&Vec::new()),
                cols,
            )),
        )
            .into_response();
    }
    (StatusCode::OK, resp).into_response()
}

pub async fn list_route_tables(
    s: &AppState,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    let mut resp = crd::generic_list_wire(s, "RouteTable", "RouteTableList", &wire.list_api_version)
        .await;
    if let Some(items) = resp.0.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            let rules = item["spec"]["rules"].as_array().map(|a| a.len()).unwrap_or(0);
            if let Some(spec) = item.get_mut("spec").and_then(|v| v.as_object_mut()) {
                spec.insert("_ruleCount".into(), serde_json::json!(rules));
            }
        }
    }
    if crd::wants_table(headers) {
        let cols = &[("Rules", ".spec._ruleCount", "integer")];
        return (
            StatusCode::OK,
            Json(build_table(
                resp.0["items"].as_array().unwrap_or(&Vec::new()),
                cols,
            )),
        )
            .into_response();
    }
    (StatusCode::OK, resp).into_response()
}
