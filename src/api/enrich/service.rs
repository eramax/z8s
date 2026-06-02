//! Response enrichment for kinds that need computed status (A2).

use std::collections::{BTreeMap, HashMap};

use axum::http::HeaderMap;
use axum::response::IntoResponse;

use crate::api::compat::{self, WireContext};
use crate::api::server::*;
use crate::api::table::{self, build_table, make_table};
use crate::store::AnyResource;

static NODEPORT_COUNTER: std::sync::atomic::AtomicU16 =
    std::sync::atomic::AtomicU16::new(30000);

fn alloc_cluster_ip() -> String {
    crate::config::get().alloc_cluster_ip()
}

fn alloc_node_port() -> i32 {
    NODEPORT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as i32
}

fn prepare_service_spec(svc: &mut Service, on_create: bool) {
    let Some(spec) = svc.spec.as_mut() else {
        return;
    };
    let svc_type = spec.type_.as_deref().unwrap_or("ClusterIP");
    if on_create
        && spec.cluster_ip.as_deref().unwrap_or("").is_empty()
        && svc_type != "ExternalName"
    {
        spec.cluster_ip = Some(alloc_cluster_ip());
        spec.cluster_ips = spec.cluster_ip.clone().map(|ip| vec![ip]);
    }
    if let Some(ports) = spec.ports.as_mut() {
        for p in ports.iter_mut() {
            if p.target_port.is_none() {
                p.target_port = Some(crate::types::IntOrString::Int(p.port));
            }
            if on_create
                && (svc_type == "NodePort" || svc_type == "LoadBalancer")
                && p.node_port.is_none()
            {
                p.node_port = Some(alloc_node_port());
            }
        }
    }
    if on_create && svc.status.is_none() {
        svc.status = Some(ServiceStatus::default());
    }
}

fn service_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "priority": 0},
        {"name": "Type", "type": "string", "priority": 0},
        {"name": "Cluster-IP", "type": "string", "priority": 0},
        {"name": "External-IP", "type": "string", "priority": 0},
        {"name": "Port(s)", "type": "string", "priority": 0},
        {"name": "Age", "type": "string", "priority": 0},
    ]);
    let rows: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            let meta = &item["metadata"];
            let spec = &item["spec"];
            let name = meta["name"].as_str().unwrap_or("");
            let svc_type = spec["type"].as_str().unwrap_or("ClusterIP");
            let cluster_ip = spec["clusterIP"].as_str().unwrap_or("<none>");
            let external_ip = "<none>";
            let ports = spec["ports"]
                .as_array()
                .map(|ps| {
                    ps.iter()
                        .map(|p| {
                            let port = p["port"].as_i64().unwrap_or(0);
                            let proto = p["protocol"].as_str().unwrap_or("TCP");
                            if let Some(np) = p["nodePort"].as_i64() {
                                format!("{port}:{np}/{proto}")
                            } else {
                                format!("{port}/{proto}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let age = age_from_timestamp(meta["creationTimestamp"].as_str().unwrap_or(""));
            serde_json::json!({
                "cells": [name, svc_type, cluster_ip, external_ip, ports, age],
                "object": item,
            })
        })
        .collect();
    make_table(columns, rows)
}

pub async fn list_services(
    s: &AppState,
    namespace: Option<&str>,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    let items: Vec<serde_json::Value> = s
        .store
        .get_by_kind("Service")
        .await
        .into_iter()
        .filter(|t| namespace.map_or(true, |ns| t.resource.namespace() == ns))
        .filter_map(|t| serde_json::to_value(&t.resource).ok())
        .map(|v| compat::encode_resource_value(v, wire))
        .collect();

    if accepts_table(headers) {
        return (StatusCode::OK, Json(service_list_to_table(&items))).into_response();
    }

    Json(serde_json::json!({
        "apiVersion": wire.list_api_version,
        "kind": "ServiceList",
        "items": items,
        "metadata": make_list_meta(),
    }))
    .into_response()
}

pub async fn create_service(
    s: &AppState,
    namespace: &str,
    raw: &axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(raw)?;
    let mut svc: Service = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Service: {e}")))?;
    if svc.metadata.namespace.is_none() {
        svc.metadata.namespace = Some(namespace.to_string());
    }
    if svc.metadata.uid.is_none() {
        svc.metadata.uid = Some(crate::config::random_id());
    }
    if svc.metadata.creation_timestamp.is_none() {
        svc.metadata.creation_timestamp = Some(now_time());
    }
    prepare_service_spec(&mut svc, true);

    let already_exists = s.store.get_by_kind("Service").await.iter().any(|t| {
        t.resource.name() == svc.metadata.name.as_deref().unwrap_or("")
            && t.resource.namespace() == namespace
    });

    let resource = AnyResource::Service(svc);
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    let status = if already_exists {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(serde_json::to_value(&resource).unwrap_or_default()),
    )
        .into_response())
}

pub async fn update_service(
    s: &AppState,
    namespace: &str,
    name: &str,
    raw: &axum::body::Bytes,
    wire: &WireContext,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(raw)?;
    let existing = s
        .store
        .get_by_kind("Service")
        .await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| serde_json::to_value(&t.resource).ok());
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut svc: Service = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Service: {e}")))?;
    svc.metadata.namespace = Some(namespace.to_string());
    svc.metadata.name = Some(name.to_string());
    prepare_service_spec(&mut svc, false);
    let resource = AnyResource::Service(svc);
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let value = serde_json::to_value(&resource).unwrap_or_default();
    Ok(Json(compat::encode_resource_value(value, wire)))
}
