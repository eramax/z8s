use crate::api::types::ResourceStore;
use crate::api::AnyResource;
use crate::supervisor::process::ProcessSupervisor;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Json};
use axum::routing::{any, delete, get, patch, post};
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tracing::info;

const Z8S_PORT: u16 = 6443;

type NamespaceStore = Arc<Mutex<HashMap<String, serde_json::Value>>>;
type EventStore = Arc<Mutex<Vec<serde_json::Value>>>;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<ResourceStore>,
    pub supervisor: Arc<ProcessSupervisor>,
    pub namespaces: NamespaceStore,
    pub events: EventStore,
}

pub async fn run_server(store: Arc<ResourceStore>, supervisor: Arc<ProcessSupervisor>) {
    let namespaces = Arc::new(Mutex::new(HashMap::new()));
    {
        let mut ns = namespaces.lock().await;
        ns.insert("default".into(), serde_json::json!({
            "metadata": {
                "name": "default",
                "uid": "ns-default",
                "creationTimestamp": "2024-01-01T00:00:00Z"
            },
            "status": { "phase": "Active" }
        }));
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    // Seed initial events
    {
        let mut ev = events.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        ev.push(serde_json::json!({
            "metadata": {"name": "z8s-started", "namespace": "default", "creationTimestamp": now},
            "involvedObject": {"kind": "Node", "name": "z8s-node", "uid": "z8s-node"},
            "reason": "Started", "message": "z8s daemon started",
            "type": "Normal", "count": 1,
            "firstTimestamp": now, "lastTimestamp": now
        }));
    }

    let state = AppState { store, supervisor: supervisor.clone(), namespaces, events };
    let cors = CorsLayer::permissive();

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/api", get(api_versions))
        .route("/api/v1", get(api_v1_resources))
        .route("/apis", get(api_groups))
        .route("/apis/apps/v1", get(api_apps_v1_resources))
        .route("/api/v1/pods", get(list_pods_all))
        .route("/api/v1/namespaces/{namespace}/pods", get(list_pods).post(create_pod))
        .route("/api/v1/namespaces/{namespace}/pods/{name}", any(pod_handler))
        .route("/api/v1/namespaces/{namespace}/pods/{name}/log", get(get_pod_log))
        .route("/api/v1/namespaces/{namespace}/pods/{name}/exec", get(crate::server::exec::exec_handler).post(crate::server::exec::exec_post_handler))

        .route("/apis/apps/v1/deployments", get(list_deployments_all))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments", get(list_deployments).post(create_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}", get(get_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale", patch(patch_deployment_scale))
        .route("/api/v1/namespaces", get(list_namespaces).post(create_namespace))
        .route("/api/v1/namespaces/{name}", get(get_namespace).delete(delete_namespace))
        .route("/api/v1/nodes", get(list_nodes))
        .route("/apis/metrics.k8s.io/v1beta1", get(metrics_api_resources))
        .route("/apis/metrics.k8s.io/v1beta1/nodes", get(metrics_top_nodes))
        .route("/apis/metrics.k8s.io/v1beta1/pods", get(top_pods_all))
        .route("/apis/metrics.k8s.io/v1beta1/namespaces/{namespace}/pods", get(top_pods))
        .route("/api/v1/events", get(list_events_all))
        .route("/api/v1/namespaces/{namespace}/events", get(list_events))
        .route("/openapi/v2", get(openapi_v2))
        .route("/openapi/v3", get(openapi_v3))
        .route("/version", get(version_handler))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/livez", get(livez))
        .fallback(fallback_handler)
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", Z8S_PORT);
    info!("Starting k8s API server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn root_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "paths": ["/api", "/apis", "/openapi/v2", "/healthz", "/readyz", "/livez", "/version"]
    }))
}

async fn api_versions() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "APIVersions",
        "versions": ["v1"],
        "serverAddressByClientCIDRs": null
    }))
}

async fn api_v1_resources() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": "v1",
        "groupVersion": "v1",
        "resources": [
            {
                "name": "pods",
                "singularName": "pod",
                "namespaced": true,
                "kind": "Pod",
                "verbs": ["get", "list", "watch", "create", "update", "delete"],
                "shortNames": ["po"],
                "categories": ["all"]
            },
            {
                "name": "pods/exec",
                "singularName": "",
                "namespaced": true,
                "kind": "PodExecOptions",
                "verbs": ["create", "get"]
            },
            {
                "name": "pods/log",
                "singularName": "",
                "namespaced": true,
                "kind": "Pod",
                "verbs": ["get"]
            },
            {
                "name": "namespaces",
                "singularName": "namespace",
                "namespaced": false,
                "kind": "Namespace",
                "verbs": ["get", "list"],
                "shortNames": ["ns"]
            },
            {
                "name": "nodes",
                "singularName": "node",
                "namespaced": false,
                "kind": "Node",
                "verbs": ["get", "list"],
                "shortNames": ["no"]
            },
            {
                "name": "services",
                "singularName": "service",
                "namespaced": true,
                "kind": "Service",
                "verbs": ["get", "list"],
                "shortNames": ["svc"]
            },
            {
                "name": "configmaps",
                "singularName": "configmap",
                "namespaced": true,
                "kind": "ConfigMap",
                "verbs": ["get", "list"],
                "shortNames": ["cm"]
            },
            {
                "name": "secrets",
                "singularName": "secret",
                "namespaced": true,
                "kind": "Secret",
                "verbs": ["get", "list"],
                "shortNames": []
            },
            {
                "name": "events",
                "singularName": "event",
                "namespaced": true,
                "kind": "Event",
                "verbs": ["get", "list", "watch"],
                "shortNames": ["ev"]
            }
        ]
    }))
}

async fn api_groups() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "APIGroupList",
        "apiVersion": "v1",
        "groups": [
            {
                "name": "apps",
                "versions": [{"groupVersion": "apps/v1", "version": "v1"}],
                "preferredVersion": {"groupVersion": "apps/v1", "version": "v1"}
            },
            {
                "name": "metrics.k8s.io",
                "versions": [{"groupVersion": "metrics.k8s.io/v1beta1", "version": "v1beta1"}],
                "preferredVersion": {"groupVersion": "metrics.k8s.io/v1beta1", "version": "v1beta1"}
            }
        ]
    }))
}

async fn api_apps_v1_resources() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": "v1",
        "groupVersion": "apps/v1",
        "resources": [
            {
                "name": "deployments",
                "singularName": "deployment",
                "namespaced": true,
                "kind": "Deployment",
                "verbs": ["get", "list", "watch", "create", "update", "delete"],
                "shortNames": ["deploy"],
                "categories": ["all"]
            }
        ]
    }))
}

fn accepts_table(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("as=Table"))
        .unwrap_or(false)
}

fn age_from_timestamp(ts: &str) -> String {
    if let Ok(created) = chrono::DateTime::parse_from_rfc3339(ts) {
        let age = chrono::Utc::now().signed_duration_since(created);
        let secs = age.num_seconds().max(0);
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else if secs < 86400 {
            format!("{}h", secs / 3600)
        } else {
            format!("{}d", secs / 86400)
        }
    } else {
        "<unknown>".into()
    }
}

fn make_table(
    columns: serde_json::Value,
    rows: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "Table",
        "apiVersion": "meta.k8s.io/v1",
        "columnDefinitions": columns,
        "rows": rows,
    })
}

fn pod_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "format": "name", "description": "Pod name", "priority": 0},
        {"name": "Ready", "type": "string", "description": "Ready containers", "priority": 0},
        {"name": "Status", "type": "string", "description": "Pod status", "priority": 0},
        {"name": "Restarts", "type": "string", "description": "Restart count", "priority": 0},
        {"name": "Age", "type": "string", "description": "Creation age", "priority": 0},
    ]);
    let rows: Vec<serde_json::Value> = items.iter().map(|item| {
        let meta = &item["metadata"];
        let status = &item["status"];
        let name = meta["name"].as_str().unwrap_or("");
        let phase = status["phase"].as_str().unwrap_or("Unknown");
        let creation = meta["creationTimestamp"].as_str().unwrap_or("");
        let age = age_from_timestamp(creation);

        let (ready_count, total, restarts) = status["containerStatuses"].as_array().map(|cs| {
            let ready = cs.iter().filter(|c| c["ready"].as_bool().unwrap_or(false)).count();
            let restarts: i32 = cs.iter().map(|c| c["restartCount"].as_i64().unwrap_or(0) as i32).sum();
            (ready, cs.len(), restarts)
        }).unwrap_or((0, 0, 0));

        serde_json::json!({
            "cells": [
                name,
                format!("{}/{}", ready_count, total),
                phase,
                format!("{}", restarts),
                age,
            ],
            "object": item,
        })
    }).collect();

    make_table(columns, rows)
}

async fn list_pods_all(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_pods_in_namespace(state, None, headers).await
}

async fn list_pods(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_pods_in_namespace(state, Some(namespace), headers).await
}

async fn list_pods_in_namespace(
    state: AppState,
    namespace: Option<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    let mut items = Vec::new();
    for t in &trackers {
        if namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns) {
            let ready = state.supervisor.is_pod_ready(t.resource.name()).await;
            items.push(resource_to_pod_json_with_status(&t.resource, ready));
        }
    }

    if accepts_table(&headers) {
        let table = pod_list_to_table(&items);
        return Ok((StatusCode::OK, Json(table)).into_response());
    }

    Ok(Json(serde_json::json!({
        "kind": "PodList",
        "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    })).into_response())
}

async fn get_pod(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let ready = state.supervisor.is_pod_ready(t.resource.name()).await;
            return Ok(Json(resource_to_pod_json_with_status(&t.resource, ready)));
        }
    }
    Err(ApiError::NotFound(format!("pod \"{}\" not found", name)))
}

async fn get_pod_log(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<String, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let containers = crate::api::types::extract_containers(&t.resource);
            if let Some(container) = containers.first() {
                let logs = state.supervisor.get_container_logs(&name, &container.name).await;
                return Ok(logs.join("\n"));
            }
            return Err(ApiError::BadRequest("no containers in pod".into()));
        }
    }
    Err(ApiError::NotFound(format!("pod \"{}\" not found", name)))
}

fn resource_to_pod_json(resource: &AnyResource) -> serde_json::Value {
    resource_to_pod_json_with_status(resource, true)
}

fn now_time() -> k8s_openapi::apimachinery::pkg::apis::meta::v1::Time {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::from_second(secs).unwrap_or(
            k8s_openapi::jiff::Timestamp::from_second(0).unwrap(),
        ),
    )
}

fn fill_pod_metadata(pod: &mut k8s_openapi::api::core::v1::Pod) {
    let meta = &mut pod.metadata;
    if meta.creation_timestamp.is_none() {
        meta.creation_timestamp = Some(now_time());
    }
    if meta.resource_version.is_none() {
        meta.resource_version = Some("1".into());
    }
    if meta.uid.is_none() {
        meta.uid = Some(format!("Pod/{}", meta.name.as_deref().unwrap_or("unknown")));
    }
}

fn resource_to_pod_json_with_status(resource: &AnyResource, is_ready: bool) -> serde_json::Value {
    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateRunning, ContainerStatus, HostIP, PodCondition, PodIP, PodStatus,
    };

    let pod = match resource {
        AnyResource::Pod(p) => p,
        _ => return serde_json::Value::Null,
    };

    let time = now_time();

    let ready_status = if is_ready { "True" } else { "False" };

    let status = PodStatus {
        phase: Some("Running".into()),
        host_ip: Some("10.0.0.1".into()),
        host_ips: Some(vec![HostIP { ip: "10.0.0.1".into() }]),
        pod_ip: Some("10.42.0.1".into()),
        pod_ips: Some(vec![PodIP { ip: "10.42.0.1".into() }]),
        start_time: Some(time.clone()),
        conditions: Some(vec![
            PodCondition {
                type_: "Initialized".into(),
                status: "True".into(),
                last_transition_time: Some(time.clone()),
                ..Default::default()
            },
            PodCondition {
                type_: "Ready".into(),
                status: ready_status.into(),
                last_transition_time: Some(time.clone()),
                ..Default::default()
            },
            PodCondition {
                type_: "ContainersReady".into(),
                status: ready_status.into(),
                last_transition_time: Some(time.clone()),
                ..Default::default()
            },
            PodCondition {
                type_: "PodScheduled".into(),
                status: "True".into(),
                last_transition_time: Some(time.clone()),
                ..Default::default()
            },
        ]),
        container_statuses: pod.spec.as_ref().map(|s| {
            s.containers.iter().map(|c| {
                ContainerStatus {
                    name: c.name.clone(),
                    image: c.image.clone().unwrap_or_default(),
                    image_id: c.image.clone().map(|i| format!("z8s://{}", i)).unwrap_or_default(),
                    ready: true,
                    restart_count: 0,
                    container_id: Some(format!("z8s://{}", c.name)),
                    state: Some(ContainerState {
                        running: Some(ContainerStateRunning {
                            started_at: Some(time.clone()),
                        }),
                        ..Default::default()
                    }),
                    started: Some(true),
                    ..Default::default()
                }
            }).collect()
        }),
        qos_class: Some("Burstable".into()),
        ..Default::default()
    };

    let mut pod = pod.clone();
    fill_pod_metadata(&mut pod);
    pod.status = Some(status);
    serde_json::to_value(&pod).unwrap_or_default()
}

fn deployment_list_to_table(items: &[serde_json::Value]) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "format": "name", "description": "Deployment name", "priority": 0},
        {"name": "Ready", "type": "string", "description": "Ready replicas", "priority": 0},
        {"name": "Up-to-date", "type": "string", "description": "Up-to-date replicas", "priority": 0},
        {"name": "Available", "type": "string", "description": "Available replicas", "priority": 0},
        {"name": "Age", "type": "string", "description": "Creation age", "priority": 0},
    ]);
    let rows: Vec<serde_json::Value> = items.iter().map(|item| {
        let meta = &item["metadata"];
        let status = &item["status"];
        let name = meta["name"].as_str().unwrap_or("");
        let creation = meta["creationTimestamp"].as_str().unwrap_or("");
        let age = age_from_timestamp(creation);
        let ready = status["readyReplicas"].as_i64().unwrap_or(0);
        let up_to_date = status["updatedReplicas"].as_i64().unwrap_or(0);
        let available = status["availableReplicas"].as_i64().unwrap_or(0);

        serde_json::json!({
            "cells": [name, format!("{}/{}", ready, status["replicas"].as_i64().unwrap_or(0)), up_to_date, available, age],
            "object": item,
        })
    }).collect();

    make_table(columns, rows)
}

async fn list_deployments_all(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_deployments_in_namespace(state, None, headers).await
}

async fn list_deployments(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_deployments_in_namespace(state, Some(namespace), headers).await
}

async fn list_deployments_in_namespace(
    state: AppState,
    namespace: Option<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    let running = state.supervisor.running.lock().await;
    let pods = state.store.get_by_kind("Pod").await;

    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| {
            namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns)
        })
        .map(|t| {
            let (ready, available) = count_deployment_pods(&t.resource, &pods, &running);
            resource_to_deploy_json(&t.resource, Some(ready), Some(available))
        })
        .collect();

    drop(running);

    if accepts_table(&headers) {
        return Ok((StatusCode::OK, Json(deployment_list_to_table(&items))).into_response());
    }

    Ok(Json(serde_json::json!({
        "kind": "DeploymentList",
        "apiVersion": "apps/v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    })).into_response())
}

async fn get_deployment(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    let running = state.supervisor.running.lock().await;
    let pods = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let (ready, available) = count_deployment_pods(&t.resource, &pods, &running);
            return Ok(Json(resource_to_deploy_json(&t.resource, Some(ready), Some(available))));
        }
    }
    Err(ApiError::NotFound(format!("deployment \"{}\" not found", name)))
}

async fn patch_deployment_scale(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    body: axum::extract::Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use k8s_openapi::api::autoscaling::v1::{Scale, ScaleSpec, ScaleStatus};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    let trackers = state.store.get_by_kind("Deployment").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            if let AnyResource::Deployment(ref mut deploy) = t.resource.clone() {
                let desired = body.get("spec")
                    .and_then(|s| s.get("replicas"))
                    .and_then(|r| r.as_i64())
                    .or_else(|| body.get("replicas").and_then(|r| r.as_i64()));

                let replicas = desired.unwrap_or(
                    deploy.spec.as_ref().and_then(|s| s.replicas).map(|r| r as i64).unwrap_or(1)
                ) as i32;

                if let Some(spec) = deploy.spec.as_mut() {
                    spec.replicas = Some(replicas);
                    info!("Scaled deployment {}/{} to {} replicas", namespace, name, replicas);
                    state.store.apply(AnyResource::Deployment(deploy.clone())).await.ok();
                }

                let selector = deploy.spec.as_ref()
                    .and_then(|s| s.selector.match_labels.as_ref())
                    .map(|labels| {
                        labels.iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect::<Vec<_>>()
                            .join(",")
                    });

                let scale = Scale {
                    metadata: ObjectMeta {
                        name: Some(name.clone()),
                        namespace: Some(namespace.clone()),
                        uid: Some(format!("Deployment/{}/{}", namespace, name)),
                        ..Default::default()
                    },
                    spec: Some(ScaleSpec {
                        replicas: Some(replicas),
                    }),
                    status: Some(ScaleStatus {
                        replicas,
                        selector,
                    }),
                };
                return Ok(Json(serde_json::to_value(&scale).unwrap_or_default()));
            }
        }
    }
    Err(ApiError::NotFound(format!("deployment \"{}\" not found", name)))
}

async fn create_pod(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("Pod");
    if kind != "Pod" {
        return Err(ApiError::BadRequest(format!("expected Pod, got {}", kind)));
    }
    let mut pod: k8s_openapi::api::core::v1::Pod = serde_json::from_value(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid Pod: {}", e)))?;
    fill_pod_metadata(&mut pod);
    let resource = AnyResource::Pod(pod);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state.supervisor.start_pod(&resource).await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let mut value = serde_json::to_value(&resource).unwrap_or_default();
    value["status"] = serde_json::json!({ "phase": "Pending" });
    Ok(Json(value))
}

async fn create_deployment(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    if kind != "Deployment" {
        return Err(ApiError::BadRequest(format!("expected Deployment, got {}", kind)));
    }
    let mut deploy: k8s_openapi::api::apps::v1::Deployment = serde_json::from_value(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid Deployment: {}", e)))?;
    fill_deployment_metadata(&mut deploy);
    let resource = AnyResource::Deployment(deploy);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let mut value = serde_json::to_value(&resource).unwrap_or_default();
    value["status"] = serde_json::json!({ "replicas": 0 });
    Ok(Json(value))
}

async fn pod_handler(
    method: axum::http::Method,
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<axum::response::Response, ApiError> {
    match method {
        Method::GET => {
            let r = get_pod(State(state.clone()), Path((namespace, name))).await?;
            Ok(r.into_response())
        }
        Method::DELETE => {
            let r = delete_pod(State(state.clone()), Path((namespace, name))).await?;
            Ok(r.into_response())
        }
        _ => Err(ApiError::MethodNotAllowed("method not allowed".into())),
    }
}

async fn delete_pod(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            state.supervisor.stop_pod(&t.resource).await;
            state.store.delete(&t.resource).await.ok();
            info!("Deleted pod {}/{}", namespace, name);
            return Ok(Json(serde_json::json!({
                "kind": "Status",
                "apiVersion": "v1",
                "metadata": {},
                "status": "Success",
                "code": 200
            })));
        }
    }
    Err(ApiError::NotFound(format!("pod \"{}\" not found", name)))
}

fn count_deployment_pods(
    resource: &AnyResource,
    pods: &[crate::api::types::ResourceTracker],
    running: &std::collections::HashMap<String, crate::supervisor::process::RunningContainer>,
) -> (usize, usize) {
    use crate::api::types::extract_containers;
    let deploy = match resource {
        AnyResource::Deployment(d) => d,
        _ => return (0, 0),
    };

    let namespace = deploy.metadata.namespace.as_deref().unwrap_or("default");
    let selector = deploy.spec.as_ref()
        .and_then(|s| s.selector.match_labels.as_ref());

    let (ready, total) = if let Some(labels) = selector {
        let matching: Vec<_> = pods.iter()
            .filter(|t| {
                if let AnyResource::Pod(pod) = &t.resource {
                    pod.metadata.namespace.as_deref() == Some(namespace)
                        && pod.metadata.labels.as_ref().map_or(false, |pl| {
                            labels.iter().all(|(k, v)| pl.get(k) == Some(v))
                        })
                } else {
                    false
                }
            })
            .collect();

        let ready_count = matching.iter().filter(|t| {
            let containers = extract_containers(&t.resource);
            containers.iter().any(|c| {
                let cid = format!("{}-{}", t.resource.name(), c.name);
                running.get(&cid).map_or(false, |rc| {
                    rc.ready.clone().try_lock().map(|r| *r).unwrap_or(false)
                })
            })
        }).count();

        (ready_count, matching.len())
    } else {
        (0, 0)
    };

    (ready, total)
}

fn fill_deployment_metadata(deploy: &mut k8s_openapi::api::apps::v1::Deployment) {
    let meta = &mut deploy.metadata;
    if meta.creation_timestamp.is_none() {
        meta.creation_timestamp = Some(now_time());
    }
    if meta.resource_version.is_none() {
        meta.resource_version = Some("1".into());
    }
    if meta.uid.is_none() {
        meta.uid = Some(format!("Deployment/{}", meta.name.as_deref().unwrap_or("unknown")));
    }
}

fn resource_to_deploy_json(
    resource: &AnyResource,
    ready_count: Option<usize>,
    available_count: Option<usize>,
) -> serde_json::Value {
    use k8s_openapi::api::apps::v1::{DeploymentCondition, DeploymentStatus};

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
                    format!("Deployment does not have minimum availability. {}/{} pods ready", running.unwrap_or(0), desired.unwrap_or(1))
                }),
            },
            DeploymentCondition {
                type_: "Progressing".into(),
                status: if all_ready { "True" } else { "True" }.into(),
                last_update_time: Some(time.clone()),
                last_transition_time: Some(time.clone()),
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

async fn list_namespaces(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let ns = state.namespaces.lock().await;
    let items: Vec<serde_json::Value> = ns.values().cloned().collect();
    Json(serde_json::json!({
        "kind": "NamespaceList",
        "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}

async fn get_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ns = state.namespaces.lock().await;
    match ns.get(&name) {
        Some(n) => Ok(Json(n.clone())),
        None => Err(ApiError::NotFound(format!("namespace \"{}\" not found", name))),
    }
}

async fn create_namespace(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Try to extract name from JSON body, or parse from protobuf wrapper
    let name = if let Ok(s) = std::str::from_utf8(&body) {
        serde_json::from_str::<serde_json::Value>(s).ok()
            .and_then(|v| v.get("metadata")?.get("name")?.as_str().map(|n| n.to_string()))
    } else {
        None
    };

    // Handle protobuf-wrapped JSON (k8s\x00 + protobuf(GroupVersion, JSON))
    let name = name.or_else(|| {
        if body.starts_with(b"k8s\x00") && body.len() > 8 {
            let payload = &body[4..];
            // Parse protobuf: skip field 1 (GroupVersion), extract field 2 (Object/JSON)
            let mut pos = 0;
            // Field 1: tag byte + varint length + data
            if pos < payload.len() && payload[pos] == 0x0a {
                pos += 1;
                if let Some(len1) = decode_varint(payload, &mut pos) {
                    pos += len1 as usize; // skip field 1 data
                }
            }
            // Field 2: tag byte + varint length + JSON data
            if pos < payload.len() && payload[pos] == 0x12 {
                pos += 1;
                if let Some(json_len) = decode_varint(payload, &mut pos) {
                    let end = pos + json_len as usize;
                    if end <= payload.len() {
                        let json_bytes = &payload[pos..end];
                        if let Ok(s) = std::str::from_utf8(json_bytes) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                                return v.get("metadata")
                                    .and_then(|m| m.get("name"))
                                    .and_then(|n| n.as_str())
                                    .map(|n| n.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    })
    .unwrap_or_else(|| format!("ns-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("x")));

    fn decode_varint(data: &[u8], pos: &mut usize) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0;
        loop {
            if *pos >= data.len() { return None; }
            let byte = data[*pos] as u64;
            *pos += 1;
            result |= (byte & 0x7f) << shift;
            if byte & 0x80 == 0 { return Some(result); }
            shift += 7;
        }
    }

    let mut ns = state.namespaces.lock().await;
    if ns.contains_key(&name) {
        return Err(ApiError::BadRequest(format!("namespace \"{}\" already exists", name)));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let entry = serde_json::json!({
        "kind": "Namespace",
        "apiVersion": "v1",
        "metadata": {
            "name": name,
            "uid": format!("ns-{}", name),
            "creationTimestamp": now,
        },
        "status": { "phase": "Active" }
    });

    info!("Created namespace: {}", name);
    ns.insert(name.to_string(), entry.clone());
    Ok(Json(entry))
}

async fn delete_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if name == "default" {
        return Err(ApiError::BadRequest("cannot delete default namespace".into()));
    }
    let mut ns = state.namespaces.lock().await;
    match ns.remove(&name) {
        Some(_) => {
            info!("Deleted namespace: {}", name);
            Ok(Json(serde_json::json!({
                "kind": "Status",
                "apiVersion": "v1",
                "metadata": {},
                "status": "Success",
                "code": 200
            })))
        }
        None => Err(ApiError::NotFound(format!("namespace \"{}\" not found", name))),
    }
}

async fn list_events_all(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let ev = state.events.lock().await;
    Json(serde_json::json!({
        "kind": "EventList",
        "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": *ev
    }))
}

async fn list_events(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<serde_json::Value> {
    let ev = state.events.lock().await;
    let items: Vec<&serde_json::Value> = ev.iter().filter(|e| {
        e.get("metadata").and_then(|m| m.get("namespace")).and_then(|n| n.as_str()) == Some(&namespace)
    }).collect();
    Json(serde_json::json!({
        "kind": "EventList",
        "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}

pub async fn add_event(state: &AppState, namespace: &str, name: &str, kind: &str, reason: &str, message: &str, event_type: &str) {
    let mut ev = state.events.lock().await;
    let now = chrono::Utc::now().to_rfc3339();
    let uid = format!("{}-{}", name, ev.len());
    ev.push(serde_json::json!({
        "metadata": {"name": uid, "namespace": namespace, "creationTimestamp": now, "uid": uid},
        "involvedObject": {"kind": kind, "name": name, "namespace": namespace},
        "reason": reason, "message": message,
        "type": event_type, "count": 1,
        "firstTimestamp": now, "lastTimestamp": now
    }));
    if ev.len() > 100 { ev.remove(0); }
}

async fn metrics_api_resources() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": "v1",
        "groupVersion": "metrics.k8s.io/v1beta1",
        "resources": [
            {"name": "pods", "singularName": "", "namespaced": true, "kind": "PodMetrics", "verbs": ["get", "list"]},
            {"name": "nodes", "singularName": "", "namespaced": false, "kind": "NodeMetrics", "verbs": ["get", "list"]}
        ]
    }))
}

async fn metrics_top_nodes() -> Json<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    Json(serde_json::json!({
        "kind": "NodeMetricsList",
        "apiVersion": "metrics.k8s.io/v1beta1",
        "metadata": { "resourceVersion": "1" },
        "items": [{
            "metadata": {"name": "z8s-node", "creationTimestamp": now},
            "timestamp": now, "window": "1m0s",
            "usage": {"cpu": "100m", "memory": "128Mi"}
        }]
    }))
}

async fn top_pods_all(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    top_pods_in_namespace(state, None).await
}

async fn top_pods(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<serde_json::Value> {
    top_pods_in_namespace(state, Some(namespace)).await
}

async fn top_pods_in_namespace(
    state: AppState,
    namespace: Option<String>,
) -> Json<serde_json::Value> {
    let trackers = state.store.get_by_kind("Pod").await;
    let mut items = Vec::new();
    for t in &trackers {
        if namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns) {
            let pod_name = t.resource.name();
            let pod_uid = t.resource.uid();
            let mut pod_cpu: u64 = 0;
            let mut pod_mem: u64 = 0;

            // Try to read cgroup stats for this pod
            let cg_base = "/sys/fs/cgroup/z8s";
            let cg_name = pod_uid.replace('/', "_").replace('.', "_").replace(':', "_");
            let cg_path = format!("{}/{}", cg_base, cg_name);

            // Memory
            if let Ok(mem) = std::fs::read_to_string(format!("{}/memory.current", cg_path)) {
                pod_mem = mem.trim().parse::<u64>().unwrap_or(0);
            }

            // CPU (from cpu.stat: usage_usec)
            if let Ok(cpu_stat) = std::fs::read_to_string(format!("{}/cpu.stat", cg_path)) {
                for line in cpu_stat.lines() {
                    if let Some(val) = line.strip_prefix("usage_usec ") {
                        pod_cpu = val.trim().parse::<u64>().unwrap_or(0);
                        break;
                    }
                }
            }

            let now = chrono::Utc::now().to_rfc3339();
            items.push(serde_json::json!({
                "metadata": {
                    "name": pod_name,
                    "namespace": t.resource.namespace(),
                    "creationTimestamp": now,
                },
                "timestamp": now,
                "window": "1m0s",
                "containers": [{
                    "name": pod_name,
                    "usage": {
                        "cpu": format!("{}n", pod_cpu * 1000),
                        "memory": format!("{}Ki", pod_mem / 1024),
                    }
                }]
            }));
        }
    }

    Json(serde_json::json!({
        "kind": "PodMetricsList",
        "apiVersion": "metrics.k8s.io/v1beta1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}

async fn list_nodes() -> Json<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    Json(serde_json::json!({
        "kind": "NodeList",
        "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": [{
            "metadata": {
                "name": "z8s-node",
                "uid": "z8s-node",
                "labels": {
                    "kubernetes.io/hostname": "z8s-node",
                    "kubernetes.io/os": "linux",
                    "kubernetes.io/arch": "amd64",
                    "beta.kubernetes.io/os": "linux",
                    "beta.kubernetes.io/arch": "amd64"
                },
                "creationTimestamp": now
            },
            "spec": {"podCIDR": "10.42.0.0/24", "podCIDRs": ["10.42.0.0/24"]},
            "status": {
                "conditions": [
                    {"type": "Ready", "status": "True", "lastHeartbeatTime": now, "lastTransitionTime": now, "reason": "KubeletReady", "message": "z8s is ready"}
                ],
                "addresses": [
                    {"type": "InternalIP", "address": "127.0.0.1"},
                    {"type": "Hostname", "address": "z8s-node"}
                ],
                "daemonEndpoints": {"kubeletEndpoint": {"Port": Z8S_PORT}},
                "nodeInfo": {
                    "machineID": "z8s-1",
                    "systemUUID": "z8s-1",
                    "bootID": "z8s-1",
                    "kernelVersion": "6.2.0",
                    "osImage": "Linux",
                    "containerRuntimeVersion": "z8s://0.1.0",
                    "kubeletVersion": "z8s-0.1.0",
                    "kubeProxyVersion": "z8s-0.1.0",
                    "operatingSystem": "linux",
                    "architecture": "amd64"
                },
                "capacity": {
                    "cpu": "4",
                    "memory": "8192Ki",
                    "pods": "110"
                },
                "allocatable": {
                    "cpu": "4",
                    "memory": "8192Ki",
                    "pods": "110"
                }
            }
        }]
    }))
}

async fn openapi_v2(
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let schema = serde_json::json!({
        "swagger": "2.0",
        "info": {"title": "z8s", "version": "0.1.0"},
        "paths": {
            "/api/v1/pods": {"get": {"produces": ["application/json"], "responses": {"200": {"description": "OK"}}}},
            "/api/v1/namespaces/{namespace}/pods": {"get": {"produces": ["application/json"], "responses": {"200": {"description": "OK"}}}},
            "/api/v1/namespaces/{namespace}/pods/{name}": {"get": {"produces": ["application/json"], "responses": {"200": {"description": "OK"}}}},
            "/apis/apps/v1/deployments": {"get": {"produces": ["application/json"], "responses": {"200": {"description": "OK"}}}},
            "/apis/apps/v1/namespaces/{namespace}/deployments": {"get": {"produces": ["application/json"], "responses": {"200": {"description": "OK"}}}}
        },
        "definitions": {
            "io.k8s.api.core.v1.Pod": {"properties": {}},
            "io.k8s.api.core.v1.PodList": {"properties": {}},
            "io.k8s.api.apps.v1.Deployment": {"properties": {}},
            "io.k8s.api.apps.v1.DeploymentList": {"properties": {}},
            "io.k8s.api.core.v1.Namespace": {"properties": {}},
            "io.k8s.api.core.v1.Node": {"properties": {}}
        }
    });

    Ok((
        StatusCode::OK,
        [("Content-Type", "application/json")],
        Json(schema),
    ).into_response())
}

async fn openapi_v3() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({
        "openapi": "3.0.0",
        "info": {"title": "z8s", "version": "0.1.0"},
        "paths": {}
    })))
}

async fn version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "major": "0",
        "minor": "1",
        "gitVersion": "z8s-v0.1.0",
        "gitCommit": "dev",
        "buildDate": chrono::Utc::now().to_rfc3339(),
        "goVersion": "go1.21",
        "compiler": "rustc",
        "platform": "linux/amd64"
    }))
}

async fn healthz() -> &'static str { "ok" }
async fn readyz() -> &'static str { "ok" }
async fn livez() -> &'static str { "ok" }

async fn fallback_handler(uri: Uri) -> impl IntoResponse {
    let body = serde_json::json!({
        "kind": "Status",
        "apiVersion": "v1",
        "metadata": {},
        "status": "Failure",
        "message": format!("no route found for {}", uri.path()),
        "reason": "NotFound",
        "code": 404,
        "details": {
            "path": uri.path()
        }
    });
    (StatusCode::NOT_FOUND, Json(body))
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    #[allow(non_snake_case)]
    fn NotFound(msg: String) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: msg }
    }
    fn BadRequest(msg: String) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: msg }
    }
    fn MethodNotAllowed(msg: String) -> Self {
        Self { status: StatusCode::METHOD_NOT_ALLOWED, message: msg }
    }
    fn UnsupportedMediaType(msg: String) -> Self {
        Self { status: StatusCode::UNSUPPORTED_MEDIA_TYPE, message: msg }
    }
    fn NotAcceptable(msg: String) -> Self {
        Self { status: StatusCode::NOT_ACCEPTABLE, message: msg }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let reason = match self.status.as_u16() {
            400 => "BadRequest",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "NotFound",
            405 => "MethodNotAllowed",
            406 => "NotAcceptable",
            409 => "Conflict",
            415 => "UnsupportedMediaType",
            429 => "TooManyRequests",
            500 => "InternalError",
            503 => "ServiceUnavailable",
            _ => "Unknown",
        };
        let body = serde_json::json!({
            "kind": "Status",
            "apiVersion": "v1",
            "metadata": {},
            "status": "Failure",
            "message": self.message,
            "reason": reason,
            "code": self.status.as_u16()
        });
        (self.status, Json(body)).into_response()
    }
}
