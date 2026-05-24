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

        .route("/apis/apps/v1/deployments", get(list_deployments_all).post(create_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments", get(list_deployments))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}", get(get_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale", patch(patch_deployment_scale))
        .route("/api/v1/namespaces", get(list_namespaces).post(create_namespace))
        .route("/api/v1/namespaces/{name}", get(get_namespace).delete(delete_namespace))
        .route("/api/v1/nodes", get(list_nodes))
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
        "versions": ["v1"],
        "serverAddressByClientCIDRs": null
    }))
}

async fn api_v1_resources() -> Json<serde_json::Value> {
    Json(serde_json::json!({
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
            }
        ]
    }))
}

async fn api_apps_v1_resources() -> Json<serde_json::Value> {
    Json(serde_json::json!({
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

async fn list_pods_all(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    list_pods_in_namespace(state, None).await
}

async fn list_pods(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    list_pods_in_namespace(state, Some(namespace)).await
}

async fn list_pods_in_namespace(
    state: AppState,
    namespace: Option<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    let mut items = Vec::new();
    for t in &trackers {
        if namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns) {
            let ready = state.supervisor.is_pod_ready(t.resource.name()).await;
            items.push(resource_to_pod_json_with_status(&t.resource, ready));
        }
    }

    Ok(Json(serde_json::json!({
        "kind": "PodList",
        "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    })))
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
    let is_ready = true; // default to true when supervisor not available
    resource_to_pod_json_with_status(resource, is_ready)
}

fn resource_to_pod_json_with_status(resource: &AnyResource, is_ready: bool) -> serde_json::Value {
    let pod = match resource {
        AnyResource::Pod(p) => p,
        _ => return serde_json::Value::Null,
    };

    let now = chrono::Utc::now().to_rfc3339();

    let ready_status = if is_ready { "True" } else { "False" };

    serde_json::json!({
        "kind": "Pod",
        "apiVersion": "v1",
        "metadata": {
            "name": pod.metadata.name,
            "namespace": pod.metadata.namespace,
            "uid": resource.uid(),
            "labels": pod.metadata.labels,
            "creationTimestamp": now,
            "resourceVersion": "1"
        },
        "spec": {
            "containers": pod.spec.as_ref().map(|s| {
                s.containers.iter().map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "image": c.image,
                        "ports": c.ports,
                        "resources": c.resources,
                        "imagePullPolicy": c.image_pull_policy,
                        "terminationMessagePolicy": c.termination_message_policy,
                        "terminationMessagePath": c.termination_message_path
                    })
                }).collect::<Vec<_>>()
            }),
            "nodeName": "z8s-node",
            "restartPolicy": pod.spec.as_ref().and_then(|s| s.restart_policy.as_ref()),
            "dnsPolicy": pod.spec.as_ref().and_then(|s| s.dns_policy.as_ref()),
            "securityContext": pod.spec.as_ref().and_then(|s| s.security_context.as_ref()),
        },
        "status": {
            "phase": if is_ready { "Running" } else { "Running" },
            "hostIP": "10.0.0.1",
            "podIP": "10.42.0.1",
            "podIPs": [{"ip": "10.42.0.1"}],
            "startTime": now,
            "conditions": [
                {"type": "Initialized", "status": "True", "lastTransitionTime": now},
                {"type": "Ready", "status": ready_status, "lastTransitionTime": now},
                {"type": "ContainersReady", "status": ready_status, "lastTransitionTime": now},
                {"type": "PodScheduled", "status": "True", "lastTransitionTime": now}
            ],
            "containerStatuses": pod.spec.as_ref().map(|s| {
                s.containers.iter().map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "state": {"running": {"startedAt": now}},
                        "lastState": {},
                        "ready": true,
                        "restartCount": 0,
                        "image": c.image,
                        "imageID": c.image.as_ref().map(|i| format!("z8s://{}", i)),
                        "containerID": format!("z8s://{}", c.name),
                        "started": true
                    })
                }).collect::<Vec<_>>()
            }),
            "qosClass": "Burstable"
        }
    })
}

async fn list_deployments_all(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    list_deployments_in_namespace(state, None).await
}

async fn list_deployments(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    list_deployments_in_namespace(state, Some(namespace)).await
}

async fn list_deployments_in_namespace(
    state: AppState,
    namespace: Option<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| {
            namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns)
        })
        .map(|t| resource_to_deploy_json(&t.resource))
        .collect();

    Ok(Json(serde_json::json!({
        "kind": "DeploymentList",
        "apiVersion": "apps/v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    })))
}

async fn get_deployment(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(resource_to_deploy_json(&t.resource)));
        }
    }
    Err(ApiError::NotFound(format!("deployment \"{}\" not found", name)))
}

async fn patch_deployment_scale(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    body: axum::extract::Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            if let AnyResource::Deployment(ref mut deploy) = t.resource.clone() {
                if let Some(spec) = deploy.spec.as_mut() {
                    if let Some(replicas) = body.get("spec").and_then(|s| s.get("replicas")).and_then(|r| r.as_i64()) {
                        spec.replicas = Some(replicas as i32);
                        info!("Scaled deployment {}/{} to {} replicas", namespace, name, replicas);
                        state.store.apply(AnyResource::Deployment(deploy.clone())).await.ok();
                        return Ok(Json(resource_to_deploy_json(&AnyResource::Deployment(deploy.clone()))));
                    }
                }
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
    let pod: k8s_openapi::api::core::v1::Pod = serde_json::from_value(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid Pod: {}", e)))?;
    let resource = AnyResource::Pod(pod);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state.supervisor.start_pod(&resource).await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "kind": "Pod",
        "apiVersion": "v1",
        "metadata": {
            "name": resource.name(),
            "namespace": resource.namespace(),
            "uid": resource.uid(),
            "creationTimestamp": chrono::Utc::now().to_rfc3339(),
        },
        "status": { "phase": "Pending" }
    })))
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
    let deploy: k8s_openapi::api::apps::v1::Deployment = serde_json::from_value(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid Deployment: {}", e)))?;
    let resource = AnyResource::Deployment(deploy);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "kind": "Deployment",
        "apiVersion": "apps/v1",
        "metadata": {
            "name": resource.name(),
            "namespace": resource.namespace(),
            "uid": resource.uid(),
            "creationTimestamp": chrono::Utc::now().to_rfc3339(),
        },
        "status": { "replicas": 0 }
    })))
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

fn resource_to_deploy_json(resource: &AnyResource) -> serde_json::Value {
    let deploy = match resource {
        AnyResource::Deployment(d) => d,
        _ => return serde_json::Value::Null,
    };

    let now = chrono::Utc::now().to_rfc3339();
    let spec = deploy.spec.as_ref();
    let replicas = spec.map(|s| s.replicas.unwrap_or(1));

    serde_json::json!({
        "kind": "Deployment",
        "apiVersion": "apps/v1",
        "metadata": {
            "name": deploy.metadata.name,
            "namespace": deploy.metadata.namespace,
            "uid": resource.uid(),
            "labels": deploy.metadata.labels,
            "creationTimestamp": now,
            "resourceVersion": "1"
        },
        "spec": {
            "replicas": replicas,
            "selector": spec.map(|s| {
                serde_json::json!({
                    "matchLabels": s.selector.match_labels
                })
            }),
            "template": spec.map(|s| s.template.clone())
        },
        "status": {
            "replicas": replicas,
            "readyReplicas": replicas,
            "availableReplicas": replicas,
            "updatedReplicas": replicas,
            "conditions": [
                {"type": "Available", "status": "True", "lastUpdateTime": now, "lastTransitionTime": now, "reason": "MinimumReplicasAvailable", "message": "Deployment has minimum availability."},
                {"type": "Progressing", "status": "True", "lastUpdateTime": now, "lastTransitionTime": now, "reason": "NewReplicaSetAvailable", "message": "ReplicaSet has successfully progressed."}
            ]
        }
    })
}

async fn list_namespaces(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let ns = state.namespaces.lock().await;
    let items: Vec<serde_json::Value> = ns.values().cloned().collect();
    Json(serde_json::json!({
        "kind": "NamespaceList",
        "apiVersion": "v1",
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

async fn list_nodes() -> Json<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    Json(serde_json::json!({
        "kind": "NodeList",
        "apiVersion": "v1",
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
        let body = serde_json::json!({
            "kind": "Status",
            "apiVersion": "v1",
            "metadata": {},
            "status": "Failure",
            "message": self.message,
            "reason": "NotFound",
            "code": self.status.as_u16()
        });
        (self.status, Json(body)).into_response()
    }
}
