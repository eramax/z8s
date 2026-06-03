//! Response enrichment for kinds that need computed status (A2).

use std::collections::{BTreeMap, HashMap};

use axum::http::HeaderMap;
use axum::response::IntoResponse;

use crate::api::compat::{self, WireContext};
use crate::api::server::*;
use crate::api::table::{self, build_table, make_table};
use crate::store::AnyResource;

async fn pod_json_with_runtime(s: &AppState, resource: &AnyResource, state: &ResourceState) -> serde_json::Value {
    let name = resource.name();
    let local_ready = s.process_tracker.is_ready(name).await;
    // For pods on remote nodes, the local ProcessTracker doesn't know about them.
    // Use gossiped ResourceState::Running as fallback — it's only set after the
    // container successfully starts on the worker node.
    let ready = local_ready || matches!(state, ResourceState::Running);
    tracing::trace!("pod_json_with_runtime name={} state={:?} local_ready={} ready={}", name, state, local_ready, ready);
    let restarts = s.process_tracker.pod_restart_counts(name).await;
    let ip = s
        .process_tracker
        .pod_ip(name)
        .await
        .map(|a| a.to_string());
    crate::components::compute::status::resource_to_pod_json_with_status(
        resource, state, ready, &restarts, ip.as_deref(),
    )
}

fn pod_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "format": "name", "priority": 0},
        {"name": "Ready", "type": "string", "priority": 0},
        {"name": "Status", "type": "string", "priority": 0},
        {"name": "Restarts", "type": "string", "priority": 0},
        {"name": "Age", "type": "string", "priority": 0},
    ]);
    let rows: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            let meta = &item["metadata"];
            let status = &item["status"];
            let name = meta["name"].as_str().unwrap_or("");
            let phase = status["phase"].as_str().unwrap_or("Unknown");
            let age = age_from_timestamp(meta["creationTimestamp"].as_str().unwrap_or(""));
            let (ready, total, restarts) = status["containerStatuses"]
                .as_array()
                .map(|cs| {
                    let ready = cs
                        .iter()
                        .filter(|c| c["ready"].as_bool().unwrap_or(false))
                        .count();
                    let restarts: i32 = cs
                        .iter()
                        .map(|c| c["restartCount"].as_i64().unwrap_or(0) as i32)
                        .sum();
                    (ready, cs.len(), restarts)
                })
                .unwrap_or((0, 0, 0));
            serde_json::json!({
                "cells": [name, format!("{ready}/{total}"), phase, restarts, age],
                "object": item,
            })
        })
        .collect();
    make_table(columns, rows)
}

pub fn extract_label_selector(raw_query: &str) -> Vec<(String, Option<String>)> {
    if raw_query.is_empty() {
        return vec![];
    }
    for part in raw_query.split('&') {
        if part.is_empty() {
            continue;
        }
        let part = urlpath_decode(part);
        if let Some(v) = part.strip_prefix("labelSelector=") {
            return parse_label_selector(v);
        }
    }
    vec![]
}

fn parse_label_selector(raw: &str) -> Vec<(String, Option<String>)> {
    if raw.is_empty() {
        return vec![];
    }
    raw.split(',')
        .filter(|s| !s.is_empty())
        .map(|req| {
            let req = urlpath_decode(req.trim());
            if let Some((k, v)) = req.split_once('=') {
                (
                    k.trim().to_string(),
                    Some(v.trim_start_matches('=').to_string()),
                )
            } else {
                (req, None)
            }
        })
        .collect()
}

fn urlpath_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push((hi as u8 * 16 + lo as u8) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn labels_match_selector(
    selector: &[(String, Option<String>)],
    labels: &BTreeMap<String, String>,
) -> bool {
    for (key, val) in selector {
        match val {
            Some(v) => {
                if labels.get(key).map(|s| s.as_str()) != Some(v.as_str()) {
                    return false;
                }
            }
            None => {
                if !labels.contains_key(key) {
                    return false;
                }
            }
        }
    }
    true
}

pub async fn list_pods(
    s: &AppState,
    namespace: Option<&str>,
    raw_query: &str,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    let label_selector = extract_label_selector(raw_query);
    let trackers = s.store.get_by_kind("Pod").await;
    let mut items = Vec::new();
    for t in &trackers {
        if namespace.map_or(true, |ns| t.resource.namespace() == ns) {
            if !label_selector.is_empty() {
                if let AnyResource::Pod(pod) = &t.resource {
                    let labels = pod.metadata.labels.clone().unwrap_or_default();
                    if !labels_match_selector(&label_selector, &labels) {
                        continue;
                    }
                }
            }
            let value =
                pod_json_with_runtime(s, &t.resource, &t.state).await;
            items.push(compat::encode_resource_value(value, wire));
        }
    }
    if accepts_table(headers) {
        return (StatusCode::OK, Json(pod_list_to_table(&items))).into_response();
    }
    Json(serde_json::json!({
        "apiVersion": wire.list_api_version,
        "kind": "PodList",
        "metadata": make_list_meta(),
        "items": items,
    }))
    .into_response()
}

pub async fn get_pod(
    s: &AppState,
    namespace: &str,
    name: &str,
    wire: &WireContext,
) -> Result<Json<serde_json::Value>, ApiError> {
    for t in s.store.get_by_kind("Pod").await {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let value = pod_json_with_runtime(s, &t.resource, &t.state).await;
            return Ok(Json(compat::encode_resource_value(value, wire)));
        }
    }
    Err(ApiError::not_found(format!("pod \"{name}\" not found")))
}

pub async fn create_pod(
    s: &AppState,
    namespace: &str,
    raw: &axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(raw)?;
    let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("Pod");
    if kind != "Pod" {
        return Err(ApiError::bad_request(format!("expected Pod, got {kind}")));
    }
    let mut pod: crate::types::Pod = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Pod: {e}")))?;
    if pod.metadata.namespace.is_none() {
        pod.metadata.namespace = Some(namespace.to_string());
    }
    crate::components::compute::status::fill_pod_metadata(&mut pod);
    let resource = AnyResource::Pod(pod);
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let tracker_state = s
        .store
        .get(&resource.uid())
        .await
        .map(|t| t.state)
        .unwrap_or(ResourceState::Pending);
    Ok((
        StatusCode::CREATED,
        Json(pod_json_with_runtime(s, &resource, &tracker_state).await),
    )
        .into_response())
}

pub async fn update_pod(
    s: &AppState,
    namespace: &str,
    name: &str,
    raw: &axum::body::Bytes,
    wire: &WireContext,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(raw)?;
    let existing = s
        .store
        .get_by_kind("Pod")
        .await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .map(|t| (serde_json::to_value(&t.resource).ok(), t.state));
    let (merged, prev_state) = match existing {
        Some((Some(v), st)) => (v, st),
        _ => (
            serde_json::Value::Object(Default::default()),
            ResourceState::Pending,
        ),
    };
    let mut merged = merged;
    json_merge_patch(&mut merged, &patch);
    let mut pod: crate::types::Pod = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Pod: {e}")))?;
    pod.metadata.namespace = Some(namespace.to_string());
    pod.metadata.name = Some(name.to_string());
    crate::components::compute::status::fill_pod_metadata(&mut pod);
    let resource = AnyResource::Pod(pod);
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let tracker_state = s
        .store
        .get(&resource.uid())
        .await
        .map(|t| t.state)
        .unwrap_or(prev_state);
    let value = pod_json_with_runtime(s, &resource, &tracker_state).await;
    Ok(Json(compat::encode_resource_value(value, wire)))
}

pub async fn delete_pod(
    s: &AppState,
    namespace: &str,
    name: &str,
) -> Result<Json<Status>, ApiError> {
    for t in s.store.get_by_kind("Pod").await {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            s.delete_and_notify(&t.resource)
                .await
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            tracing::info!("Deleted pod {namespace}/{name}");
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("pod \"{name}\" not found")))
}

