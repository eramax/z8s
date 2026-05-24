use crate::api::types::ResourceStore;
use crate::api::AnyResource;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;

const Z8S_PORT: u16 = 6443;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<ResourceStore>,
}

pub async fn run_server(store: Arc<ResourceStore>) {
    let state = AppState { store };
    let cors = CorsLayer::permissive();

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/api", get(api_versions))
        .route("/api/v1", get(api_v1_resources))
        .route("/apis", get(api_groups))
        .route("/apis/apps/v1", get(api_apps_v1_resources))
        .route("/api/v1/pods", get(list_pods_all))
        .route("/api/v1/namespaces/{namespace}/pods", get(list_pods))
        .route("/api/v1/namespaces/{namespace}/pods/{name}", get(get_pod))
        .route("/apis/apps/v1/deployments", get(list_deployments_all))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments", get(list_deployments))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}", get(get_deployment))
        .route("/api/v1/namespaces", get(list_namespaces))
        .route("/api/v1/nodes", get(list_nodes))
        .route("/openapi/v2", get(openapi_v2))
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
    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| {
            namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns)
        })
        .map(|t| resource_to_pod_json(&t.resource))
        .collect();

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
            return Ok(Json(resource_to_pod_json(&t.resource)));
        }
    }
    Err(ApiError::NotFound(format!("pod \"{}\" not found", name)))
}

fn resource_to_pod_json(resource: &AnyResource) -> serde_json::Value {
    let pod = match resource {
        AnyResource::Pod(p) => p,
        _ => return serde_json::Value::Null,
    };

    let now = chrono::Utc::now().to_rfc3339();

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
            "phase": "Running",
            "hostIP": "10.0.0.1",
            "podIP": "10.42.0.1",
            "podIPs": [{"ip": "10.42.0.1"}],
            "startTime": now,
            "conditions": [
                {"type": "Initialized", "status": "True", "lastTransitionTime": now},
                {"type": "Ready", "status": "True", "lastTransitionTime": now},
                {"type": "ContainersReady", "status": "True", "lastTransitionTime": now},
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

async fn list_namespaces() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "NamespaceList",
        "apiVersion": "v1",
        "items": [
            {
                "metadata": {
                    "name": "default",
                    "uid": "ns-default",
                    "creationTimestamp": "2024-01-01T00:00:00Z"
                },
                "status": {"phase": "Active"}
            }
        ]
    }))
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

async fn openapi_v2() -> Json<serde_json::Value> {
    Json(serde_json::json!({
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
    }))
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
