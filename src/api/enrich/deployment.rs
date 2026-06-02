//! Response enrichment for kinds that need computed status (A2).

use std::collections::{BTreeMap, HashMap};

use axum::http::HeaderMap;
use axum::response::IntoResponse;

use crate::api::compat::{self, WireContext};
use crate::api::server::*;
use crate::api::table::{self, build_table, make_table};
use crate::store::AnyResource;

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
