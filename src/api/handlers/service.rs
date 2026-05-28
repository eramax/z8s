use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub fn alloc_cluster_ip() -> String {
    crate::config::get().alloc_cluster_ip()
}


pub fn alloc_node_port() -> i32 {
    NODEPORT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as i32
}


pub fn service_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "priority": 0},
        {"name": "Type", "type": "string", "priority": 0},
        {"name": "Cluster-IP", "type": "string", "priority": 0},
        {"name": "External-IP", "type": "string", "priority": 0},
        {"name": "Port(s)", "type": "string", "priority": 0},
        {"name": "Age", "type": "string", "priority": 0},
    ]);
    let rows: Vec<serde_json::Value> = items.iter().map(|item| {
        let meta = &item["metadata"];
        let spec = &item["spec"];
        let name = meta["name"].as_str().unwrap_or("");
        let svc_type = spec["type"].as_str().unwrap_or("ClusterIP");
        let cluster_ip = spec["clusterIP"].as_str().unwrap_or("<none>");
        let external_ip = "<none>";
        let ports = spec["ports"].as_array().map(|ps| {
            ps.iter().map(|p| {
                let port = p["port"].as_i64().unwrap_or(0);
                let proto = p["protocol"].as_str().unwrap_or("TCP");
                if let Some(np) = p["nodePort"].as_i64() {
                    format!("{}:{}/{}", port, np, proto)
                } else {
                    format!("{}/{}", port, proto)
                }
            }).collect::<Vec<_>>().join(",")
        }).unwrap_or_default();
        let age = age_from_timestamp(meta["creationTimestamp"].as_str().unwrap_or(""));
        serde_json::json!({
            "cells": [name, svc_type, cluster_ip, external_ip, ports, age],
            "object": item,
        })
    }).collect();
    make_table(columns, rows)
}


pub async fn list_services_all(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    list_services_in_ns(&state, None, headers).await
}


pub async fn list_services(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    list_services_in_ns(&state, Some(namespace), headers).await
}


pub async fn list_services_in_ns(state: &AppState, namespace: Option<String>, headers: axum::http::HeaderMap) -> axum::response::Response {
    let svcs: Vec<Service> = state.store.get_by_kind("Service").await
        .into_iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .filter_map(|t| if let AnyResource::Service(s) = t.resource { Some(s) } else { None })
        .collect();
    if accepts_table(&headers) {
        let items: Vec<serde_json::Value> = svcs.iter()
            .filter_map(|s| serde_json::to_value(s).ok())
            .collect();
        return (StatusCode::OK, Json(service_list_to_table(&items))).into_response();
    }
    Json(List::<Service> {
        items: svcs,
        metadata: ListMeta { resource_version: Some("1".into()), ..Default::default() },
    }).into_response()
}


pub async fn get_service(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Service").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("service \"{}/{}\" not found", namespace, name)))
}


pub async fn create_service(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut svc: Service = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Service: {}", e)))?;
    if svc.metadata.namespace.is_none() {
        svc.metadata.namespace = Some(namespace.clone());
    }
    if svc.metadata.uid.is_none() {
        svc.metadata.uid = Some(uuid::Uuid::new_v4().to_string());
    }
    if svc.metadata.creation_timestamp.is_none() {
        svc.metadata.creation_timestamp = Some(now_time());
    }

    if let Some(spec) = svc.spec.as_mut() {
        let svc_type = spec.type_.as_deref().unwrap_or("ClusterIP");
        if spec.cluster_ip.as_deref().unwrap_or("").is_empty() && svc_type != "ExternalName" {
            spec.cluster_ip = Some(alloc_cluster_ip());
            spec.cluster_ips = spec.cluster_ip.clone().map(|ip| vec![ip]);
        }
        if let Some(ports) = spec.ports.as_mut() {
            for p in ports.iter_mut() {
                if p.target_port.is_none() {
                    p.target_port = Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(p.port));
                }
                if (svc_type == "NodePort" || svc_type == "LoadBalancer") && p.node_port.is_none() {
                    p.node_port = Some(alloc_node_port());
                }
            }
        }
    }
    if svc.status.is_none() {
        svc.status = Some(ServiceStatus::default());
    }

    let already_exists = state.store.get_by_kind("Service").await.iter()
        .any(|t| t.resource.name() == svc.metadata.name.as_deref().unwrap_or("") && t.resource.namespace() == namespace);

    let resource = AnyResource::Service(svc.clone());
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;

    state.registry.on_apply(&state.ctx, &resource).await;

    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}


pub async fn update_service(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = state.store.get_by_kind("Service").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::Service(svc) = t.resource { serde_json::to_value(svc).ok() } else { None });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut svc: Service = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Service: {}", e)))?;
    if svc.metadata.namespace.is_none() { svc.metadata.namespace = Some(namespace); }
    if svc.metadata.name.is_none() { svc.metadata.name = Some(name); }
    if let Some(spec) = svc.spec.as_mut() {
        if let Some(ports) = spec.ports.as_mut() {
            for p in ports.iter_mut() {
                if p.target_port.is_none() {
                    p.target_port = Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(p.port));
                }
            }
        }
    }
    let resource = AnyResource::Service(svc.clone());
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.registry.on_apply(&state.ctx, &resource).await;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}


pub async fn delete_service(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Service").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("service \"{}/{}\" not found", namespace, name)))
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/services", get(list_services_all))
        .route("/api/v1/namespaces/{namespace}/services", get(list_services).post(create_service))
        .route("/api/v1/namespaces/{namespace}/services/{name}", get(get_service).put(update_service).patch(update_service).delete(delete_service))
}
