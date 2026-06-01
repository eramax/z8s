use crate::api::server::*;
use axum::Router;
use axum::routing::get;

fn ip_in_cidr(ip: &str, cidr: &str) -> bool {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return false;
    }
    let prefix_len: u32 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let ip_parts: Vec<u8> = ip.split('.').filter_map(|p| p.parse().ok()).collect();
    let cidr_parts: Vec<u8> = parts[0].split('.').filter_map(|p| p.parse().ok()).collect();
    if ip_parts.len() != 4 || cidr_parts.len() != 4 {
        return false;
    }
    let ip_u32 = u32::from_be_bytes([ip_parts[0], ip_parts[1], ip_parts[2], ip_parts[3]]);
    let cidr_u32 = u32::from_be_bytes([cidr_parts[0], cidr_parts[1], cidr_parts[2], cidr_parts[3]]);
    let mask = if prefix_len == 0 {
        0u32
    } else {
        u32::MAX << (32 - prefix_len)
    };
    (ip_u32 & mask) == (cidr_u32 & mask)
}

pub async fn list_subnets(
    headers: axum::http::HeaderMap,
    State(s): State<AppState>,
) -> axum::response::Response {
    let mut resp = crate::api::handlers::crd::generic_list(&s, "Subnet", "SubnetList").await;
    let svcs = s.store.get_by_kind("Service").await;
    if let Some(items) = resp.0.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            let cidr = item["spec"]["cidr"].as_str().unwrap_or("");
            // Count pods using ProcessTracker (store has no podIP)
            let pc = s
                .process_tracker
                .pod_ips()
                .await
                .iter()
                .filter(|(_, ip)| ip_in_cidr(&ip.to_string(), cidr))
                .count();
            let sc = svcs
                .iter()
                .filter(|t| {
                    if let AnyResource::Service(svc) = &t.resource {
                        let subnet_ann = svc
                            .metadata
                            .annotations
                            .as_ref()
                            .and_then(|a| a.get("z8s.io/subnet").map(|s| s.as_str()));
                        let name = item["metadata"]["name"].as_str().unwrap_or("");
                        subnet_ann == Some(name)
                    } else {
                        false
                    }
                })
                .count();
            item["spec"]["_podCount"] = serde_json::json!(pc);
            item["spec"]["_serviceCount"] = serde_json::json!(sc);
        }
    }
    if crate::api::handlers::crd::wants_table(&headers) {
        let empty = vec![];
        let cols = &[
            ("CIDR", ".spec.cidr", "string"),
            ("VNet", ".spec.vnet", "string"),
            ("Pods", ".spec._podCount", "integer"),
            ("Services", ".spec._serviceCount", "integer"),
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
pub async fn create_subnet(
    State(s): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let r: crate::types::Subnet =
        serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    crate::api::handlers::crd::generic_create(&s, AnyResource::Subnet(r), "Subnet").await
}
pub async fn get_subnet(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get(&s, "Subnet", &n).await
}
pub async fn delete_subnet(
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete(&s, "Subnet", &n).await
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/apis/z8s.io/v1/subnets",
            get(list_subnets).post(create_subnet),
        )
        .route(
            "/apis/z8s.io/v1/subnets/{name}",
            get(get_subnet).delete(delete_subnet),
        )
}
