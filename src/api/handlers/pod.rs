use crate::api::server::*;
use axum::Router;
use axum::routing::{any, get};

pub async fn list_pods_all(
    State(state): State<AppState>,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_pods_in_ns(state, None, raw_query.as_deref().unwrap_or(""), headers).await
}

pub async fn list_pods(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_pods_in_ns(
        state,
        Some(namespace),
        raw_query.as_deref().unwrap_or(""),
        headers,
    )
    .await
}

pub async fn list_pods_in_ns(
    state: AppState,
    namespace: Option<String>,
    raw_query: &str,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let label_selector = extract_label_selector(raw_query);

    let trackers = state.store.get_by_kind("Pod").await;
    let mut items = Vec::new();
    for t in &trackers {
        if namespace
            .as_deref()
            .map_or(true, |ns| t.resource.namespace() == ns)
        {
            if !label_selector.is_empty() {
                if let AnyResource::Pod(pod) = &t.resource {
                    let labels = pod.metadata.labels.clone().unwrap_or_default();
                    if !labels_match_selector(&label_selector, &labels) {
                        continue;
                    }
                }
            }
            let ready = state.process_tracker.is_ready(t.resource.name()).await;
            let restarts = state
                .process_tracker
                .pod_restart_counts(t.resource.name())
                .await;
            let ip = state
                .process_tracker
                .pod_ip(t.resource.name())
                .await
                .map(|a| a.to_string());
            items.push(resource_to_pod_json_with_status(
                &t.resource,
                &t.state,
                ready,
                &restarts,
                ip.as_deref(),
            ));
        }
    }
    if accepts_table(&headers) {
        return Ok((StatusCode::OK, Json(pod_list_to_table(&items))).into_response());
    }
    Ok(Json(serde_json::json!({
        "kind": "PodList", "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
    .into_response())
}

pub async fn get_pod(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let ready = state.process_tracker.is_ready(t.resource.name()).await;
            let restarts = state
                .process_tracker
                .pod_restart_counts(t.resource.name())
                .await;
            let ip = state
                .process_tracker
                .pod_ip(t.resource.name())
                .await
                .map(|a| a.to_string());
            return Ok(Json(resource_to_pod_json_with_status(
                &t.resource,
                &t.state,
                ready,
                &restarts,
                ip.as_deref(),
            )));
        }
    }
    Err(ApiError::not_found(format!("pod \"{}\" not found", name)))
}

pub async fn get_pod_log(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<String, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let containers = crate::store::extract_containers(&t.resource);
            if let Some(container) = containers.first() {
                let logs = state.process_tracker.get_logs(&name, &container.name).await;
                return Ok(logs.join("\n"));
            }
            return Err(ApiError::bad_request("no containers in pod".into()));
        }
    }
    Err(ApiError::not_found(format!("pod \"{}\" not found", name)))
}

pub async fn pod_handler(
    method: Method,
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    match method {
        Method::GET => get_pod(State(state), Path((namespace, name)))
            .await
            .map(IntoResponse::into_response),
        Method::DELETE => delete_pod(State(state), Path((namespace, name)))
            .await
            .map(IntoResponse::into_response),
        Method::PATCH | Method::PUT => {
            if body.is_empty() {
                return Err(ApiError::bad_request("missing body".into()));
            }
            let body = parse_body(&body)?;
            let mut pod: crate::types::Pod = serde_json::from_value(body)
                .map_err(|e| ApiError::bad_request(format!("invalid Pod: {}", e)))?;
            if pod.metadata.namespace.is_none() {
                pod.metadata.namespace = Some(namespace);
            }
            if pod.metadata.name.is_none() {
                pod.metadata.name = Some(name);
            }
            fill_pod_metadata(&mut pod);
            let resource = AnyResource::Pod(pod);
            state
                .apply_and_broadcast(resource.clone())
                .await
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let pod_name = resource.name().to_string();
            let tracker_state = state
                .store
                .get(&resource.uid())
                .await
                .map(|t| t.state)
                .unwrap_or(ResourceState::Pending);
            let is_ready = state.process_tracker.is_ready(&pod_name).await;
            let restarts = state.process_tracker.pod_restart_counts(&pod_name).await;
            let ip = state
                .process_tracker
                .pod_ip(&pod_name)
                .await
                .map(|a| a.to_string());
            Ok(Json(resource_to_pod_json_with_status(
                &resource,
                &tracker_state,
                is_ready,
                &restarts,
                ip.as_deref(),
            ))
            .into_response())
        }
        _ => Err(ApiError::method_not_allowed("method not allowed".into())),
    }
}

pub async fn delete_pod(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            info!("Deleted pod {}/{}", namespace, name);
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("pod \"{}\" not found", name)))
}

pub async fn create_pod(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("Pod");
    if kind != "Pod" {
        return Err(ApiError::bad_request(format!("expected Pod, got {}", kind)));
    }
    let mut pod: crate::types::Pod = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Pod: {}", e)))?;
    if pod.metadata.namespace.is_none() {
        pod.metadata.namespace = Some(namespace);
    }
    fill_pod_metadata(&mut pod);
    let resource = AnyResource::Pod(pod);
    state
        .apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let tracker_state = state
        .store
        .get(&resource.uid())
        .await
        .map(|t| t.state)
        .unwrap_or(ResourceState::Pending);
    let is_ready = state.process_tracker.is_ready(resource.name()).await;
    let restarts = state
        .process_tracker
        .pod_restart_counts(resource.name())
        .await;
    let ip = state
        .process_tracker
        .pod_ip(resource.name())
        .await
        .map(|a| a.to_string());
    Ok((
        StatusCode::CREATED,
        Json(resource_to_pod_json_with_status(
            &resource,
            &tracker_state,
            is_ready,
            &restarts,
            ip.as_deref(),
        )),
    )
        .into_response())
}

pub fn fill_pod_metadata(pod: &mut crate::types::Pod) {
    let meta = &mut pod.metadata;
    if meta.creation_timestamp.is_none() {
        meta.creation_timestamp = Some(now_time());
    }
    if meta.resource_version.is_none() {
        meta.resource_version = Some("1".into());
    }
    if meta.uid.is_none() {
        let ns = meta.namespace.as_deref().unwrap_or("default");
        let name = meta.name.as_deref().unwrap_or("unknown");
        meta.uid = Some(format!("Pod/{}/{}", ns, name));
    }
}

pub fn resource_to_pod_json_with_status(
    resource: &AnyResource,
    state: &ResourceState,
    is_ready: bool,
    restart_counts: &std::collections::HashMap<String, u32>,
    pod_ip: Option<&str>,
) -> serde_json::Value {
    let pod = match resource {
        AnyResource::Pod(p) => p,
        _ => return serde_json::Value::Null,
    };

    let time = now_time();
    let phase = if is_ready {
        "Running"
    } else {
        match state {
            ResourceState::Pending => "Pending",
            ResourceState::Running => "Running",
            ResourceState::Succeeded => "Succeeded",
            ResourceState::Failed(_) => "Failed",
            ResourceState::Terminated => "Succeeded",
        }
    };
    let ready = if is_ready { "True" } else { "False" };
    let ip = pod_ip.unwrap_or("127.0.0.1").to_string();
    let status = PodStatus {
        phase: Some(phase.into()),
        host_ip: Some(ip.clone()),
        host_ips: Some(vec![HostIP { ip: ip.clone() }]),
        pod_ip: Some(ip.clone()),
        pod_ips: Some(vec![PodIP { ip }]),
        start_time: Some(time.clone()),
        conditions: Some(vec![
            pod_condition("Initialized", "True", &time),
            pod_condition("Ready", ready, &time),
            pod_condition("ContainersReady", ready, &time),
            pod_condition("PodScheduled", "True", &time),
        ]),
        container_statuses: pod.spec.as_ref().map(|s| {
            s.containers
                .iter()
                .map(|c| {
                    let restarts = restart_counts.get(&c.name).copied().unwrap_or(0) as i32;
                    let crash_loop = restarts >= 3 && !is_ready;
                    let cstate = Some(if is_ready {
                        ContainerState {
                            running: Some(ContainerStateRunning {
                                started_at: Some(time.clone()),
                            }),
                            ..Default::default()
                        }
                    } else if matches!(state, ResourceState::Succeeded | ResourceState::Terminated)
                    {
                        ContainerState {
                            terminated: Some(ContainerStateTerminated {
                                exit_code: 0,
                                reason: Some("Completed".into()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    } else if let ResourceState::Failed(msg) = state {
                        ContainerState {
                            terminated: Some(ContainerStateTerminated {
                                exit_code: 1,
                                reason: Some("Error".into()),
                                message: Some(msg.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    } else {
                        ContainerState {
                            waiting: Some(ContainerStateWaiting {
                                reason: Some(
                                    if crash_loop {
                                        "CrashLoopBackOff"
                                    } else {
                                        "ContainerCreating"
                                    }
                                    .into(),
                                ),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    });
                    ContainerStatus {
                        name: c.name.clone(),
                        image: c.image.clone().unwrap_or_default(),
                        image_id: c
                            .image
                            .clone()
                            .map(|i| format!("z8s://{}", i))
                            .unwrap_or_default(),
                        ready: is_ready,
                        restart_count: restarts,
                        container_id: Some(format!("z8s://{}", c.name)),
                        state: cstate,
                        started: Some(matches!(state, ResourceState::Running) && !crash_loop),
                        ..Default::default()
                    }
                })
                .collect()
        }),
        message: if let ResourceState::Failed(msg) = state {
            Some(msg.clone())
        } else {
            None
        },
        reason: if restart_counts.values().any(|&v| v >= 3) && !is_ready {
            Some("CrashLoopBackOff".into())
        } else {
            None
        },
        qos_class: Some("Burstable".into()),
        ..Default::default()
    };

    let mut pod = pod.clone();
    fill_pod_metadata(&mut pod);
    pod.status = Some(status);
    serde_json::to_value(&pod).unwrap_or_default()
}

pub fn pod_condition(type_: &str, status: &str, time: &crate::types::Time) -> PodCondition {
    PodCondition {
        type_: type_.into(),
        status: status.into(),
        last_transition_time: Some(time.clone()),
        ..Default::default()
    }
}

pub fn pod_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
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
                "cells": [name, format!("{}/{}", ready, total), phase, restarts, age],
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

pub fn parse_label_selector(raw: &str) -> Vec<(String, Option<String>)> {
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

pub fn urlpath_decode(s: &str) -> String {
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

pub fn labels_match_selector(
    selector: &[(String, Option<String>)],
    labels: &std::collections::BTreeMap<String, String>,
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

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/pods", get(list_pods_all))
        .route(
            "/api/v1/namespaces/{namespace}/pods",
            get(list_pods).post(create_pod),
        )
        .route(
            "/api/v1/namespaces/{namespace}/pods/{name}",
            any(pod_handler),
        )
        .route(
            "/api/v1/namespaces/{namespace}/pods/{name}/log",
            get(get_pod_log),
        )
        .route(
            "/api/v1/namespaces/{namespace}/pods/{name}/exec",
            get(crate::cri::exec::exec_handler).post(crate::cri::exec::exec_post_handler),
        )
}
