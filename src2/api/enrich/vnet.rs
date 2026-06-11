//! Response enrichment for kinds that need computed status (A2).

use std::collections::{BTreeMap, HashMap};

use axum::http::HeaderMap;
use axum::response::IntoResponse;

use crate::api::compat::{self, WireContext};
use crate::api::server::*;
use crate::api::table::{self, build_table, make_table};
use crate::store::AnyResource;

pub async fn list_vnets(
    s: &AppState,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    let mut resp =
        crate::api::handlers::crd::generic_list_wire(s, "VNet", "VNetList", &wire.list_api_version).await;
    let subnets = s.store.get_by_kind("Subnet").await;
    let pods = s.store.get_by_kind("Pod").await;
    let svcs = s.store.get_by_kind("Service").await;
    if let Some(items) = resp.0.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            let name = item["metadata"]["name"].as_str().unwrap_or("");
            let sc = subnets
                .iter()
                .filter(|t| {
                    if let AnyResource::Subnet(sn) = &t.resource {
                        sn.spec.vnet == name
                    } else {
                        false
                    }
                })
                .count();
            if let Some(spec) = item.get_mut("spec").and_then(|v| v.as_object_mut()) {
                spec.insert("_subnetCount".into(), serde_json::json!(sc));
                spec.insert("_podCount".into(), serde_json::json!(pods.len()));
                spec.insert("_serviceCount".into(), serde_json::json!(svcs.len()));
            }
        }
    }
    if crate::api::handlers::crd::wants_table(headers) {
        let cols = &[
            ("CIDR", ".spec.cidr", "string"),
            ("Role", ".spec.role", "string"),
            ("Internet", ".spec.internetAccess", "boolean"),
            ("Subnets", ".spec._subnetCount", "integer"),
            ("Pods", ".spec._podCount", "integer"),
            ("Services", ".spec._serviceCount", "integer"),
        ];
        return (
            StatusCode::OK,
            Json(table::build_table(
                resp.0["items"].as_array().unwrap_or(&Vec::new()),
                cols,
            )),
        )
            .into_response();
    }
    (StatusCode::OK, resp).into_response()
}

pub async fn get_vnet(
    s: &AppState,
    name: &str,
    wire: &WireContext,
) -> Result<Json<serde_json::Value>, ApiError> {
    let value = crate::api::handlers::crd::generic_get(s, "VNet", name).await?.0;
    Ok(Json(compat::encode_resource_value(value, wire)))
}
