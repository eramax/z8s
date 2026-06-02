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

pub fn fill_deployment_metadata(deploy: &mut crate::types::Deployment) {
    let meta = &mut deploy.metadata;
    if meta.creation_timestamp.is_none() {
        meta.creation_timestamp = Some(now_time());
    }
    if meta.resource_version.is_none() {
        meta.resource_version = Some("1".into());
    }
    if meta.uid.is_none() {
        let ns = meta.namespace.as_deref().unwrap_or("default");
        let name = meta.name.as_deref().unwrap_or("unknown");
        meta.uid = Some(format!("Deployment/{ns}/{name}"));
    }
}

pub fn resource_to_deploy_json(
    resource: &AnyResource,
    ready_count: Option<usize>,
    available_count: Option<usize>,
) -> serde_json::Value {
    let deploy = match resource {
        AnyResource::Deployment(d) => d,
        _ => return serde_json::Value::Null,
    };

    let time = now_time();
    let desired = deploy.spec.as_ref().and_then(|s| s.replicas);
    let running = ready_count.map(|n| n as i32).or(desired);
    let available = available_count.map(|n| n as i32).or(desired);
    let all_ready = running.unwrap_or(0) >= desired.unwrap_or(1);

    let status = DeploymentStatus {
        replicas: desired,
        ready_replicas: running,
        available_replicas: available,
        updated_replicas: available,
        conditions: Some(vec![
            DeploymentCondition {
                type_: "Available".into(),
                status: if all_ready { "True" } else { "False" }.into(),
                last_update_time: Some(time.clone()),
                last_transition_time: Some(time.clone()),
                reason: Some(
                    if all_ready {
                        "MinimumReplicasAvailable"
                    } else {
                        "MinimumReplicasUnavailable"
                    }
                    .into(),
                ),
                message: Some(if all_ready {
                    "Deployment has minimum availability.".into()
                } else {
                    format!(
                        "{}/{} pods ready",
                        running.unwrap_or(0),
                        desired.unwrap_or(1)
                    )
                }),
            },
            DeploymentCondition {
                type_: "Progressing".into(),
                status: "True".into(),
                last_update_time: Some(time.clone()),
                last_transition_time: Some(time),
                reason: Some("NewReplicaSetAvailable".into()),
                message: Some("ReplicaSet has successfully progressed.".into()),
            },
        ]),
        ..Default::default()
    };

    let mut deploy = deploy.clone();
    fill_deployment_metadata(&mut deploy);
    deploy.status = Some(status);
    serde_json::to_value(&deploy).unwrap_or_default()
}

pub async fn count_deployment_pods(
    resource: &AnyResource,
    pods: &[crate::store::ResourceTracker],
    tracker: &crate::scheduler::process::ProcessTracker,
) -> (usize, usize) {
    use crate::store::extract_containers;
    let deploy = match resource {
        AnyResource::Deployment(d) => d,
        _ => return (0, 0),
    };
    let deploy_name = deploy.metadata.name.as_deref().unwrap_or("");
    let namespace = deploy.metadata.namespace.as_deref().unwrap_or("default");
    let selector = deploy
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref());

    let Some(labels) = selector else {
        return (0, 0);
    };

    let matching: Vec<_> = pods
        .iter()
        .filter(|t| {
            if let AnyResource::Pod(pod) = &t.resource {
                pod.metadata.namespace.as_deref() == Some(namespace)
                    && crate::components::compute::deployment::pod_owned_by_deployment(
                        pod, deploy_name,
                    )
                    && pod
                        .metadata
                        .labels
                        .as_ref()
                        .map_or(false, |pl| labels.iter().all(|(k, v)| pl.get(k) == Some(v)))
            } else {
                false
            }
        })
        .collect();

    let desired = deploy.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1) as usize;

    let mut ready = 0;
    for t in &matching {
        for c in extract_containers(&t.resource) {
            let cid = format!("{}-{}", t.resource.name(), c.name);
            if tracker.is_container_ready(&cid).await {
                ready += 1;
                break;
            }
        }
    }

    let total = matching.len().min(desired);
    let ready = ready.min(desired);
    (ready, total)
}

fn deployment_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "format": "name", "priority": 0},
        {"name": "Ready", "type": "string", "priority": 0},
        {"name": "Up-to-date", "type": "string", "priority": 0},
        {"name": "Available", "type": "string", "priority": 0},
        {"name": "Age", "type": "string", "priority": 0},
    ]);
    let rows: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            let meta = &item["metadata"];
            let status = &item["status"];
            let name = meta["name"].as_str().unwrap_or("");
            let age = age_from_timestamp(meta["creationTimestamp"].as_str().unwrap_or(""));
            let ready = status["readyReplicas"].as_i64().unwrap_or(0);
            let total = status["replicas"].as_i64().unwrap_or(0);
            let up_to_date = status["updatedReplicas"].as_i64().unwrap_or(0);
            let available = status["availableReplicas"].as_i64().unwrap_or(0);
            serde_json::json!({
                "cells": [name, format!("{ready}/{total}"), up_to_date, available, age],
                "object": item,
            })
        })
        .collect();
    make_table(columns, rows)
}

pub async fn list_deployments(
    s: &AppState,
    namespace: Option<&str>,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    let trackers = s.store.get_by_kind("Deployment").await;
    let pods = s.store.get_by_kind("Pod").await;
    let pt = &s.process_tracker;

    let mut items: Vec<serde_json::Value> = Vec::new();
    for t in &trackers {
        if namespace.map_or(true, |ns| t.resource.namespace() == ns) {
            let (ready, avail) = count_deployment_pods(&t.resource, &pods, pt).await;
            let value = resource_to_deploy_json(&t.resource, Some(ready), Some(avail));
            items.push(compat::encode_resource_value(value, wire));
        }
    }

    if accepts_table(headers) {
        return (StatusCode::OK, Json(deployment_list_to_table(&items))).into_response();
    }

    Json(serde_json::json!({
        "apiVersion": wire.list_api_version,
        "kind": "DeploymentList",
        "metadata": make_list_meta(),
        "items": items,
    }))
    .into_response()
}

pub async fn get_deployment(
    s: &AppState,
    namespace: &str,
    name: &str,
    wire: &WireContext,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = s.store.get_by_kind("Deployment").await;
    let pods = s.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let (ready, avail) =
                count_deployment_pods(&t.resource, &pods, &s.process_tracker).await;
            let value = resource_to_deploy_json(&t.resource, Some(ready), Some(avail));
            return Ok(Json(compat::encode_resource_value(value, wire)));
        }
    }
    Err(ApiError::not_found(format!(
        "deployment \"{name}\" not found"
    )))
}

pub async fn update_deployment(
    s: &AppState,
    namespace: &str,
    name: &str,
    raw: &axum::body::Bytes,
    wire: &WireContext,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(raw)?;
    let existing = find_deployment(s, namespace, name).await;
    let mut merged = existing
        .map(|d| serde_json::to_value(d).ok())
        .flatten()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut deploy: crate::types::Deployment = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Deployment: {e}")))?;
    deploy.metadata.namespace = Some(namespace.to_string());
    deploy.metadata.name = Some(name.to_string());
    fill_deployment_metadata(&mut deploy);
    let resource = AnyResource::Deployment(deploy);
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let pods = s.store.get_by_kind("Pod").await;
    let (ready, avail) = count_deployment_pods(&resource, &pods, &s.process_tracker).await;
    let value = resource_to_deploy_json(&resource, Some(ready), Some(avail));
    Ok(Json(compat::encode_resource_value(value, wire)))
}

pub async fn find_deployment(
    s: &AppState,
    namespace: &str,
    name: &str,
) -> Option<crate::types::Deployment> {
    let trackers = s.store.get_by_kind("Deployment").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            if let AnyResource::Deployment(d) = &t.resource {
                return Some(d.clone());
            }
        }
    }
    None
}

pub async fn delete_deployment_cascade(
    s: &AppState,
    namespace: &str,
    name: &str,
) -> Result<Json<Status>, ApiError> {
    let deploy = find_deployment(s, namespace, name)
        .await
        .ok_or_else(|| ApiError::not_found(format!("deployment \"{name}\" not found")))?;
    if let Some(selector) = deploy.spec.as_ref().and_then(|s| s.selector.match_labels.as_ref()) {
        let pods = s.store.get_by_kind("Pod").await;
        for pt in &pods {
            if pt.resource.namespace() != namespace {
                continue;
            }
            if let AnyResource::Pod(pod) = &pt.resource {
                let pod_labels = pod.metadata.labels.clone().unwrap_or_default();
                if labels_match(selector, &pod_labels)
                    && crate::components::compute::deployment::pod_owned_by_deployment(pod, name)
                {
                    s.registry.on_delete(&s.ctx, &pt.resource).await;
                    s.store.delete(&pt.resource).await.ok();
                }
            }
        }
    }
    let resource = AnyResource::Deployment(deploy);
    s.store.delete(&resource).await.ok();
    Ok(Json(ok_status()))
}

fn labels_match(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    selector
        .iter()
        .all(|(key, value)| labels.get(key) == Some(value))
}

// ── Service ─────────────────────────────────────────────────────────

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

// ── Pod ─────────────────────────────────────────────────────────────

async fn pod_json_with_runtime(s: &AppState, resource: &AnyResource, state: &ResourceState) -> serde_json::Value {
    let name = resource.name();
    let ready = s.process_tracker.is_ready(name).await;
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

// ── Node ────────────────────────────────────────────────────────────

pub async fn list_nodes(
    s: &AppState,
    headers: &HeaderMap,
    wire: &WireContext,
) -> axum::response::Response {
    let local = make_local_node();
    let mut nodes_with_sync: Vec<(Node, Option<String>)> =
        vec![(local.clone(), Some(crate::config::now_rfc3339()))];

    for t in s.store.get_by_kind("Node").await {
        if let AnyResource::Node(n) = &t.resource {
            if !nodes_with_sync.iter().any(|(i, _)| i.metadata.name == n.metadata.name) {
                nodes_with_sync.push((n.clone(), Some(t.last_updated)));
            }
        }
    }

    if accepts_table(headers) {
        let pods = s.store.get_by_kind("Pod").await;
        let svc_count = s.store.get_by_kind("Service").await.len();
        let running: Vec<String> = futures_util::future::join_all(pods.iter().map(|t| async {
            if let AnyResource::Pod(p) = &t.resource {
                if s.process_tracker
                    .is_running(p.metadata.name.as_deref().unwrap_or(""))
                    .await
                {
                    return p.metadata.name.clone().unwrap_or_default();
                }
            }
            String::new()
        }))
        .await
        .into_iter()
        .filter(|n| !n.is_empty())
        .collect();
        return (
            StatusCode::OK,
            Json(node_list_to_table(&nodes_with_sync, &pods, svc_count, &running)),
        )
            .into_response();
    }

    let items: Vec<serde_json::Value> = nodes_with_sync
        .into_iter()
        .map(|(n, _)| {
            let v = serde_json::to_value(&n).unwrap_or_default();
            compat::encode_resource_value(v, wire)
        })
        .collect();

    Json(serde_json::json!({
        "apiVersion": wire.list_api_version,
        "kind": "NodeList",
        "items": items,
        "metadata": make_list_meta(),
    }))
    .into_response()
}

pub async fn get_node(s: &AppState, name: &str, wire: &WireContext) -> Result<Json<serde_json::Value>, ApiError> {
    let local = make_local_node();
    if local.metadata.name.as_deref() == Some(name) {
        let v = serde_json::to_value(&local).unwrap_or_default();
        return Ok(Json(compat::encode_resource_value(v, wire)));
    }
    for t in s.store.get_by_kind("Node").await {
        if let AnyResource::Node(n) = t.resource {
            if n.metadata.name.as_deref() == Some(name) {
                let v = serde_json::to_value(&n).unwrap_or_default();
                return Ok(Json(compat::encode_resource_value(v, wire)));
            }
        }
    }
    Err(ApiError::not_found(format!("node \"{name}\" not found")))
}

fn node_list_to_table(
    nodes: &[(Node, Option<String>)],
    pods: &[crate::store::ResourceTracker],
    svc_count: usize,
    running: &[String],
) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "format": "name", "priority": 0},
        {"name": "Status", "type": "string", "priority": 0},
        {"name": "Roles", "type": "string", "priority": 0},
        {"name": "Age", "type": "string", "priority": 0},
        {"name": "CPU", "type": "string", "priority": 0},
        {"name": "RAM", "type": "string", "priority": 0},
        {"name": "Pods", "type": "string", "priority": 0},
        {"name": "Svc", "type": "string", "priority": 0},
        {"name": "Last Sync", "type": "string", "priority": 0},
    ]);

    let now = std::time::SystemTime::now();
    let now_utc = crate::config::now_rfc3339();
    let rows: Vec<serde_json::Value> = nodes
        .iter()
        .map(|(node, last_sync)| {
            let name = node.metadata.name.as_deref().unwrap_or("");
            let age = node
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|t| table::format_ts_relative(&t.0, now))
                .unwrap_or_else(|| "<unknown>".into());

            let sync_str = last_sync
                .as_ref()
                .map(|t| {
                    if let Some(sync_secs) = crate::config::parse_rfc3339_secs(t) {
                        let now_secs = crate::config::parse_rfc3339_secs(&now_utc).unwrap_or(0);
                        let secs = (now_secs - sync_secs).max(0) as u64;
                        if secs < 60 {
                            format!("{secs}s")
                        } else if secs < 3600 {
                            format!("{}m", secs / 60)
                        } else {
                            format!("{}h", secs / 3600)
                        }
                    } else {
                        "<unknown>".to_string()
                    }
                })
                .unwrap_or_else(|| "<unknown>".into());

            let ready = node
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                .map(|c| {
                    if c.status == "True" {
                        "Ready"
                    } else {
                        "NotReady"
                    }
                })
                .unwrap_or("Unknown");

            let cpu_cap = node
                .status
                .as_ref()
                .and_then(|s| s.capacity.as_ref())
                .and_then(|c| c.get("cpu"))
                .map(|q| {
                    let v: f64 = q
                        .0
                        .trim_end_matches(|c: char| !c.is_ascii_digit())
                        .parse()
                        .unwrap_or(0.0);
                    format!("{}", v as u64)
                })
                .unwrap_or_else(|| "?".into());

            let mem_cap = node
                .status
                .as_ref()
                .and_then(|s| s.capacity.as_ref())
                .and_then(|c| c.get("memory"))
                .map(|q| {
                    let v = parse_ki(&q.0);
                    if v >= 1024 * 1024 * 1024 {
                        format!("{:.1}Gi", v as f64 / (1024.0 * 1024.0 * 1024.0))
                    } else if v >= 1024 * 1024 {
                        format!("{:.0}Mi", v / (1024 * 1024))
                    } else {
                        format!("{:.0}Ki", v / 1024)
                    }
                })
                .unwrap_or_else(|| "?".into());

            let total = pods
                .iter()
                .filter(|t| {
                    if let AnyResource::Pod(p) = &t.resource {
                        p.assigned_node.as_deref() == Some(name)
                    } else {
                        false
                    }
                })
                .count();
            let ready_pods = pods
                .iter()
                .filter(|t| {
                    if let AnyResource::Pod(p) = &t.resource {
                        p.assigned_node.as_deref() == Some(name)
                            && running
                                .contains(&p.metadata.name.as_deref().unwrap_or("").to_string())
                    } else {
                        false
                    }
                })
                .count();
            let pod_str = format!("{ready_pods}/{total}");

            serde_json::json!({
                "cells": [name, ready, "worker", age, cpu_cap, mem_cap, pod_str, svc_count.to_string(), sync_str],
                "object": node,
            })
        })
        .collect();

    make_table(columns, rows)
}

pub fn make_local_node() -> Node {
    let cfg = crate::config::get();
    let time = now_time();
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get().to_string())
        .unwrap_or_else(|_| "1".into());
    let mut labels = BTreeMap::new();
    labels.insert("kubernetes.io/hostname".into(), cfg.node_name.clone());
    labels.insert("kubernetes.io/os".into(), "linux".into());
    labels.insert("kubernetes.io/arch".into(), detect_arch());
    labels.insert("beta.kubernetes.io/os".into(), "linux".into());
    labels.insert("beta.kubernetes.io/arch".into(), detect_arch());

    let mut capacity = BTreeMap::new();
    capacity.insert("cpu".into(), Quantity(cpu_count.clone()));
    capacity.insert("memory".into(), Quantity(host_memory_ki()));
    capacity.insert("pods".into(), Quantity("110".into()));

    Node {
        api_version: "v1".into(),
        kind: "Node".into(),
        metadata: ObjectMeta {
            name: Some(cfg.node_name.clone()),
            uid: Some(cfg.node_name.clone()),
            labels: Some(labels),
            creation_timestamp: Some(time.clone()),
            ..Default::default()
        },
        spec: Some(NodeSpec {
            pod_cidr: Some("10.42.0.0/24".into()),
            pod_cidrs: Some(vec!["10.42.0.0/24".into()]),
            ..Default::default()
        }),
        status: Some(NodeStatus {
            conditions: Some(vec![NodeCondition {
                type_: "Ready".into(),
                status: "True".into(),
                last_heartbeat_time: Some(time.clone()),
                last_transition_time: Some(time.clone()),
                reason: Some("KubeletReady".into()),
                message: Some("z8s is ready".into()),
                ..Default::default()
            }]),
            addresses: Some(vec![
                NodeAddress {
                    type_: "InternalIP".into(),
                    address: cfg.node_ip.clone(),
                },
                NodeAddress {
                    type_: "Hostname".into(),
                    address: cfg.node_name.clone(),
                },
            ]),
            daemon_endpoints: Some(NodeDaemonEndpoints {
                kubelet_endpoint: Some(DaemonEndpoint {
                    port: z8s_port() as i32,
                }),
            }),
            node_info: Some(NodeSystemInfo {
                machine_id: format!("z8s-{}", cfg.node_name),
                system_uuid: format!("z8s-{}", cfg.node_name),
                boot_id: format!("z8s-{}", cfg.node_name),
                kernel_version: kernel_version(),
                os_image: "Linux".into(),
                container_runtime_version: format!("z8s://{}", env!("CARGO_PKG_VERSION")),
                kubelet_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                kube_proxy_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                operating_system: "linux".into(),
                architecture: detect_arch(),
                ..Default::default()
            }),
            capacity: Some(capacity.clone()),
            allocatable: Some(capacity),
            ..Default::default()
        }),
    }
}

fn kernel_version() -> String {
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|s| s.split_whitespace().nth(2).map(|v| v.to_string()))
        .unwrap_or_else(|| "unknown".into())
}

fn host_memory_ki() -> String {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .map(|kb| format!("{kb}Ki"))
        })
        .unwrap_or_else(|| "8192Ki".into())
}

fn parse_ki(s: &str) -> u64 {
    let s = s.trim();
    if let Some(rest) = s.strip_suffix("Ki") {
        rest.parse().unwrap_or(0) * 1024
    } else if let Some(rest) = s.strip_suffix("Mi") {
        rest.parse().unwrap_or(0) * 1024 * 1024
    } else if let Some(rest) = s.strip_suffix("Gi") {
        rest.parse().unwrap_or(0) * 1024 * 1024 * 1024
    } else {
        s.parse().unwrap_or(0)
    }
}

// ── VNet (z8s.io) ───────────────────────────────────────────────────

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
