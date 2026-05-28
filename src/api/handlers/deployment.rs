use axum::Router;
use axum::routing::{get, patch};
use crate::api::server::*;

pub fn fill_deployment_metadata(deploy: &mut k8s_openapi::api::apps::v1::Deployment) {
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
        meta.uid = Some(format!("Deployment/{}/{}", ns, name));
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
                reason: Some(if all_ready { "MinimumReplicasAvailable" } else { "MinimumReplicasUnavailable" }.into()),
                message: Some(if all_ready {
                    "Deployment has minimum availability.".into()
                } else {
                    format!("{}/{} pods ready", running.unwrap_or(0), desired.unwrap_or(1))
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
    pods: &[crate::types::ResourceTracker],
    tracker: &crate::scheduler::process::ProcessTracker,
) -> (usize, usize) {
    use crate::types::extract_containers;
    let deploy = match resource {
        AnyResource::Deployment(d) => d,
        _ => return (0, 0),
    };
    let deploy_name = deploy.metadata.name.as_deref().unwrap_or("");
    let namespace = deploy.metadata.namespace.as_deref().unwrap_or("default");
    let selector = deploy.spec.as_ref().and_then(|s| s.selector.match_labels.as_ref());

    let Some(labels) = selector else { return (0, 0) };

    let matching: Vec<_> = pods
        .iter()
        .filter(|t| {
            if let AnyResource::Pod(pod) = &t.resource {
                pod.metadata.namespace.as_deref() == Some(namespace)
                    && pod_managed_by_deployment(pod, deploy_name)
                    && pod.metadata.labels.as_ref().map_or(false, |pl| {
                        labels.iter().all(|(k, v)| pl.get(k) == Some(v))
                    })
            } else {
                false
            }
        })
        .collect();

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

    (ready, matching.len())
}


pub fn deployment_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
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
                "cells": [name, format!("{}/{}", ready, total), up_to_date, available, age],
                "object": item,
            })
        })
        .collect();
    make_table(columns, rows)
}


pub fn pod_managed_by_deployment(pod: &k8s_openapi::api::core::v1::Pod, deploy_name: &str) -> bool {
    pod.metadata
        .name
        .as_deref()
        .map_or(false, |n| n.starts_with(&format!("{deploy_name}-pod-")))
}


pub async fn list_deployments_all(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_deployments_in_ns(state, None, headers).await
}


pub async fn list_deployments(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_deployments_in_ns(state, Some(namespace), headers).await
}


pub async fn list_deployments_in_ns(
    state: AppState,
    namespace: Option<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    let tracker = state.process_tracker.clone();
    let pods = state.store.get_by_kind("Pod").await;

    let mut items: Vec<serde_json::Value> = Vec::new();
    for t in &trackers {
        if namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns) {
            let (ready, avail) = count_deployment_pods(&t.resource, &pods, &tracker).await;
            items.push(resource_to_deploy_json(&t.resource, Some(ready), Some(avail)));
        }
    }

    if accepts_table(&headers) {
        return Ok((StatusCode::OK, Json(deployment_list_to_table(&items))).into_response());
    }
    Ok(Json(serde_json::json!({
        "kind": "DeploymentList", "apiVersion": "apps/v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
    .into_response())
}


pub async fn get_deployment(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    let pods = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let (ready, avail) = count_deployment_pods(&t.resource, &pods, &state.process_tracker).await;
            return Ok(Json(resource_to_deploy_json(&t.resource, Some(ready), Some(avail))));
        }
    }
    Err(ApiError::not_found(format!("deployment \"{}\" not found", name)))
}


pub async fn patch_deployment_scale(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    use k8s_openapi::api::autoscaling::v1::{Scale, ScaleSpec, ScaleStatus};
    let body = parse_body(&raw)?;

    let trackers = state.store.get_by_kind("Deployment").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            if let AnyResource::Deployment(ref mut deploy) = t.resource.clone() {
                let desired = body
                    .get("spec")
                    .and_then(|s| s.get("replicas"))
                    .and_then(|r| r.as_i64())
                    .or_else(|| body.get("replicas").and_then(|r| r.as_i64()));

                let replicas = desired.unwrap_or_else(|| {
                    deploy.spec.as_ref().and_then(|s| s.replicas).map(|r| r as i64).unwrap_or(1)
                }) as i32;

                if let Some(spec) = deploy.spec.as_mut() {
                    spec.replicas = Some(replicas);
                    info!("Scaled deployment {}/{} to {} replicas", namespace, name, replicas);
                    state.store.apply(AnyResource::Deployment(deploy.clone())).await.ok();
                }

                let selector = deploy
                    .spec
                    .as_ref()
                    .and_then(|s| s.selector.match_labels.as_ref())
                    .map(|l| {
                        l.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(",")
                    });

                let scale = Scale {
                    metadata: ObjectMeta {
                        name: Some(name.clone()),
                        namespace: Some(namespace.clone()),
                        uid: Some(format!("Deployment/{}/{}", namespace, name)),
                        ..Default::default()
                    },
                    spec: Some(ScaleSpec { replicas: Some(replicas) }),
                    status: Some(ScaleStatus { replicas, selector }),
                };
                return Ok(Json(serde_json::to_value(&scale).unwrap_or_default()));
            }
        }
    }
    Err(ApiError::not_found(format!("deployment \"{}\" not found", name)))
}


pub async fn create_deployment(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    if kind != "Deployment" {
        return Err(ApiError::bad_request(format!("expected Deployment, got {}", kind)));
    }
    let mut deploy: k8s_openapi::api::apps::v1::Deployment = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Deployment: {}", e)))?;
    if deploy.metadata.namespace.is_none() {
        deploy.metadata.namespace = Some(namespace);
    }
    fill_deployment_metadata(&mut deploy);
    let resource = AnyResource::Deployment(deploy);
    state.store.apply(resource.clone()).await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut value = serde_json::to_value(&resource).unwrap_or_default();
    value["status"] = serde_json::json!({ "replicas": 0 });
    Ok((StatusCode::CREATED, Json(value)).into_response())
}


pub async fn delete_deployment(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            if let AnyResource::Deployment(deploy) = &t.resource {
                let selector = deploy.spec.as_ref()
                    .and_then(|s| s.selector.match_labels.as_ref());
                if let Some(match_labels) = selector {
                    let pods = state.store.get_by_kind("Pod").await;
                    for pt in &pods {
                        if pt.resource.namespace() != namespace { continue; }
                        if let AnyResource::Pod(pod) = &pt.resource {
                            let pod_labels = pod.metadata.labels.clone().unwrap_or_default();
                            if labels_match(match_labels, &pod_labels)
                                && crate::components::compute::deployment::pod_owned_by_deployment(pod, &name)
                            {
                                info!("Deleting pod {} owned by deployment {}/{}", pt.resource.name(), namespace, name);
                                state.registry.on_delete(&state.ctx, &pt.resource).await;
                                state.store.delete(&pt.resource).await.ok();
                            }
                        }
                    }
                }
            }
            state.store.delete(&t.resource).await.ok();
            info!("Deleted deployment {}/{}", namespace, name);
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("deployment \"{}\" not found", name)))
}


pub async fn patch_deployment(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = state.store.get_by_kind("Deployment").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| serde_json::to_value(&t.resource).ok());
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut deploy: k8s_openapi::api::apps::v1::Deployment = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Deployment: {}", e)))?;
    if deploy.metadata.namespace.is_none() {
        deploy.metadata.namespace = Some(namespace);
    }
    if deploy.metadata.name.is_none() {
        deploy.metadata.name = Some(name);
    }
    fill_deployment_metadata(&mut deploy);
    let resource = AnyResource::Deployment(deploy);
    state.store.apply(resource.clone()).await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut value = serde_json::to_value(&resource).unwrap_or_default();
    value["status"] = serde_json::json!({ "replicas": 0 });
    Ok(Json(value))
}

pub fn labels_match(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    for (key, value) in selector {
        if labels.get(key) != Some(value) {
            return false;
        }
    }
    true
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/apps/v1/deployments", get(list_deployments_all))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments", get(list_deployments).post(create_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}", get(get_deployment).delete(delete_deployment).patch(patch_deployment).put(patch_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale", patch(patch_deployment_scale))
}
