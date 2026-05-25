use crate::api::types::{ResourceState, ResourceStore};
use crate::api::AnyResource;
use crate::supervisor::process::ProcessSupervisor;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Json};
use axum::routing::{any, get, patch, post};
use axum::Router;
use k8s_openapi::api::apps::v1::{DeploymentCondition, DeploymentStatus};
use k8s_openapi::api::authorization::v1::{
    SelfSubjectAccessReview, SelfSubjectAccessReviewSpec, SubjectAccessReviewStatus,
};
use k8s_openapi::api::core::v1::{
    ConfigMap, ContainerState, ContainerStateRunning, ContainerStatus, DaemonEndpoint, Event,
    EventSource, HostIP, Namespace, NamespaceStatus, Node, NodeAddress,
    NodeCondition, NodeDaemonEndpoints, NodeSpec, NodeStatus, NodeSystemInfo,
    ObjectReference, PodCondition, PodIP, PodStatus, Secret,
};
use k8s_openapi::List;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{
    APIGroup, APIGroupList, APIResource, APIResourceList, APIVersions, GroupVersionForDiscovery,
    ListMeta, ObjectMeta, Status,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::info;

pub const Z8S_PORT: u16 = 6443;

/// Accept either JSON or k8s protobuf request bodies.
// RFC 7396 JSON Merge Patch: null values delete keys, objects recurse.
fn json_merge_patch(base: &mut serde_json::Value, patch: &serde_json::Value) {
    if let (serde_json::Value::Object(b), serde_json::Value::Object(p)) = (base, patch) {
        for (k, v) in p {
            if v.is_null() {
                b.remove(k);
            } else if v.is_object() {
                let entry = b.entry(k.clone()).or_insert(serde_json::Value::Object(Default::default()));
                json_merge_patch(entry, v);
            } else {
                b.insert(k.clone(), v.clone());
            }
        }
    }
}

fn parse_body(bytes: &axum::body::Bytes) -> Result<serde_json::Value, ApiError> {
    if let Some(v) = crate::server::proto::try_proto_to_json(bytes) {
        return Ok(v);
    }
    if let Ok(v) = serde_json::from_slice(bytes) {
        return Ok(v);
    }
    // kubectl apply sends application/apply-patch+yaml — fall back to YAML
    serde_yaml::from_slice::<serde_yaml::Value>(bytes)
        .ok()
        .and_then(|y| serde_json::to_value(y).ok())
        .ok_or_else(|| ApiError::bad_request("invalid body: not valid JSON or YAML".to_string()))
}

type NamespaceStore = Arc<RwLock<HashMap<String, Namespace>>>;
type EventStore = Arc<Mutex<Vec<Event>>>;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<ResourceStore>,
    pub supervisor: Arc<ProcessSupervisor>,
    pub namespaces: NamespaceStore,
    pub events: EventStore,
}

pub async fn run_server(store: Arc<ResourceStore>, supervisor: Arc<ProcessSupervisor>) {
    let namespaces: NamespaceStore = Arc::new(RwLock::new(HashMap::new()));
    {
        let mut ns = namespaces.write().await;
        ns.insert("default".into(), make_namespace("default", "ns-default"));
    }

    let events: EventStore = Arc::new(Mutex::new(Vec::new()));
    {
        let mut ev = events.lock().await;
        ev.push(make_event(
            "z8s-started",
            "default",
            "Node",
            "z8s-node",
            "Started",
            "z8s daemon started",
            "Normal",
        ));
    }

    let state = AppState { store, supervisor, namespaces, events };

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/api", get(api_versions))
        .route("/api/v1", get(api_v1_resources))
        .route("/apis", get(api_groups))
        .route("/apis/apps/v1", get(api_apps_v1_resources))
        .route("/apis/authorization.k8s.io/v1", get(api_authz_v1_resources))
        .route("/apis/authorization.k8s.io/v1/selfsubjectaccessreviews", post(self_subject_access_review))
        .route("/apis/authorization.k8s.io/v1/subjectaccessreviews", post(self_subject_access_review))
        // Pods
        .route("/api/v1/pods", get(list_pods_all))
        .route("/api/v1/namespaces/{namespace}/pods", get(list_pods).post(create_pod))
        .route("/api/v1/namespaces/{namespace}/pods/{name}", any(pod_handler))
        .route("/api/v1/namespaces/{namespace}/pods/{name}/log", get(get_pod_log))
        .route(
            "/api/v1/namespaces/{namespace}/pods/{name}/exec",
            get(crate::server::exec::exec_handler).post(crate::server::exec::exec_post_handler),
        )
        // Deployments
        .route("/apis/apps/v1/deployments", get(list_deployments_all))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments", get(list_deployments).post(create_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}",
            get(get_deployment).delete(delete_deployment).patch(patch_deployment).put(patch_deployment))
        .route("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale", patch(patch_deployment_scale))
        // Namespaces
        .route("/api/v1/namespaces", get(list_namespaces).post(create_namespace))
        .route("/api/v1/namespaces/{name}", get(get_namespace).delete(delete_namespace))
        // Cluster-level
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/nodes/{name}", get(get_node))
        .route("/api/v1/events", get(list_events_all))
        .route("/api/v1/namespaces/{namespace}/events", get(list_events))
        // ConfigMaps
        .route("/api/v1/configmaps", get(list_configmaps_all))
        .route("/api/v1/namespaces/{namespace}/configmaps", get(list_configmaps).post(create_configmap))
        .route("/api/v1/namespaces/{namespace}/configmaps/{name}",
            get(get_configmap).put(update_configmap).patch(update_configmap).delete(delete_configmap))
        // Secrets
        .route("/api/v1/secrets", get(list_secrets_all))
        .route("/api/v1/namespaces/{namespace}/secrets", get(list_secrets).post(create_secret))
        .route("/api/v1/namespaces/{namespace}/secrets/{name}",
            get(get_secret).put(update_secret).patch(update_secret).delete(delete_secret))
        // Metrics
        .route("/apis/metrics.k8s.io/v1beta1", get(metrics_api_resources))
        .route("/apis/metrics.k8s.io/v1beta1/nodes", get(metrics_top_nodes))
        .route("/apis/metrics.k8s.io/v1beta1/pods", get(top_pods_all))
        .route("/apis/metrics.k8s.io/v1beta1/namespaces/{namespace}/pods", get(top_pods))
        // Discovery
        .route("/openapi/v2", get(openapi_v2))
        .route("/openapi/v3", get(openapi_v3))
        .route("/version", get(version_handler))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/livez", get(livez))
        .fallback(fallback_handler)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", Z8S_PORT);
    info!("Starting k8s API server on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn now_time() -> k8s_openapi::apimachinery::pkg::apis::meta::v1::Time {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::from_second(secs)
            .unwrap_or_else(|_| k8s_openapi::jiff::Timestamp::from_second(0).unwrap()),
    )
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn make_namespace(name: &str, uid: &str) -> Namespace {
    Namespace {
        metadata: ObjectMeta {
            name: Some(name.into()),
            uid: Some(uid.into()),
            creation_timestamp: Some(now_time()),
            ..Default::default()
        },
        spec: None,
        status: Some(NamespaceStatus {
            phase: Some("Active".into()),
            ..Default::default()
        }),
    }
}

fn make_event(
    name: &str,
    namespace: &str,
    obj_kind: &str,
    obj_name: &str,
    reason: &str,
    message: &str,
    event_type: &str,
) -> Event {
    let time = now_time();
    Event {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some(namespace.into()),
            creation_timestamp: Some(time.clone()),
            ..Default::default()
        },
        involved_object: ObjectReference {
            kind: Some(obj_kind.into()),
            name: Some(obj_name.into()),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        reason: Some(reason.into()),
        message: Some(message.into()),
        type_: Some(event_type.into()),
        count: Some(1),
        first_timestamp: Some(time.clone()),
        last_timestamp: Some(time),
        source: Some(EventSource {
            component: Some("z8s".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
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
        let secs = chrono::Utc::now()
            .signed_duration_since(created)
            .num_seconds()
            .max(0);
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

fn make_table(columns: serde_json::Value, rows: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "kind": "Table",
        "apiVersion": "meta.k8s.io/v1",
        "columnDefinitions": columns,
        "rows": rows,
    })
}

fn ok_status() -> Status {
    Status {
        status: Some("Success".into()),
        code: Some(200),
        ..Default::default()
    }
}

fn make_list_meta() -> ListMeta {
    ListMeta {
        resource_version: Some("1".into()),
        ..Default::default()
    }
}

// ── Discovery endpoints ──────────────────────────────────────────────────────

async fn root_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "paths": ["/api", "/apis", "/openapi/v2", "/healthz", "/readyz", "/livez", "/version"]
    }))
}

async fn api_versions() -> Json<APIVersions> {
    Json(APIVersions {
        versions: vec!["v1".into()],
        server_address_by_client_cidrs: vec![],
    })
}

async fn api_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "v1".into(),
        resources: vec![
            api_resource("pods", "pod", true, "Pod", &["get", "list", "watch", "create", "update", "delete"], &["po"], &["all"]),
            api_resource("pods/exec", "", true, "PodExecOptions", &["create", "get"], &[], &[]),
            api_resource("pods/log", "", true, "Pod", &["get"], &[], &[]),
            api_resource("namespaces", "namespace", false, "Namespace", &["get", "list", "create", "delete"], &["ns"], &[]),
            api_resource("nodes", "node", false, "Node", &["get", "list"], &["no"], &[]),
            api_resource("services", "service", true, "Service", &["get", "list", "create", "delete"], &["svc"], &[]),
            api_resource("configmaps", "configmap", true, "ConfigMap", &["get", "list", "create", "delete"], &["cm"], &[]),
            api_resource("secrets", "secret", true, "Secret", &["get", "list", "create", "delete"], &[], &[]),
            api_resource("events", "event", true, "Event", &["get", "list", "watch"], &["ev"], &[]),
        ],
    })
}

async fn api_groups() -> Json<APIGroupList> {
    Json(APIGroupList {
        groups: vec![
            APIGroup {
                name: "apps".into(),
                versions: vec![gvd("apps/v1", "v1")],
                preferred_version: Some(gvd("apps/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "metrics.k8s.io".into(),
                versions: vec![gvd("metrics.k8s.io/v1beta1", "v1beta1")],
                preferred_version: Some(gvd("metrics.k8s.io/v1beta1", "v1beta1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "authorization.k8s.io".into(),
                versions: vec![gvd("authorization.k8s.io/v1", "v1")],
                preferred_version: Some(gvd("authorization.k8s.io/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
        ],
    })
}

async fn api_apps_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "apps/v1".into(),
        resources: vec![api_resource(
            "deployments",
            "deployment",
            true,
            "Deployment",
            &["get", "list", "watch", "create", "update", "patch", "delete"],
            &["deploy"],
            &["all"],
        )],
    })
}

async fn api_authz_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "authorization.k8s.io/v1".into(),
        resources: vec![
            api_resource("selfsubjectaccessreviews", "", false, "SelfSubjectAccessReview", &["create"], &[], &[]),
            api_resource("subjectaccessreviews", "", false, "SubjectAccessReview", &["create"], &[], &[]),
        ],
    })
}

fn api_resource(
    name: &str,
    singular: &str,
    namespaced: bool,
    kind: &str,
    verbs: &[&str],
    short_names: &[&str],
    categories: &[&str],
) -> APIResource {
    APIResource {
        name: name.into(),
        singular_name: singular.into(),
        namespaced,
        kind: kind.into(),
        verbs: verbs.iter().map(|s| s.to_string()).collect(),
        short_names: if short_names.is_empty() {
            None
        } else {
            Some(short_names.iter().map(|s| s.to_string()).collect())
        },
        categories: if categories.is_empty() {
            None
        } else {
            Some(categories.iter().map(|s| s.to_string()).collect())
        },
        group: None,
        version: None,
        storage_version_hash: None,
    }
}

fn gvd(group_version: &str, version: &str) -> GroupVersionForDiscovery {
    GroupVersionForDiscovery {
        group_version: group_version.into(),
        version: version.into(),
    }
}

async fn self_subject_access_review(
    _body: axum::body::Bytes,
) -> Json<SelfSubjectAccessReview> {
    Json(SelfSubjectAccessReview {
        metadata: ObjectMeta::default(),
        spec: SelfSubjectAccessReviewSpec::default(),
        status: Some(SubjectAccessReviewStatus {
            allowed: true,
            reason: Some("z8s grants all access".into()),
            ..Default::default()
        }),
    })
}

// ── Pods ─────────────────────────────────────────────────────────────────────

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

fn resource_to_pod_json_with_status(
    resource: &AnyResource,
    state: &ResourceState,
    is_ready: bool,
) -> serde_json::Value {
    let pod = match resource {
        AnyResource::Pod(p) => p,
        _ => return serde_json::Value::Null,
    };

    let time = now_time();
    let phase = match state {
        ResourceState::Pending => "Pending",
        ResourceState::Running => "Running",
        ResourceState::Failed(_) => "Failed",
        ResourceState::Terminated => "Succeeded",
    };
    let ready = if is_ready && matches!(state, ResourceState::Running) { "True" } else { "False" };

    let status = PodStatus {
        phase: Some(phase.into()),
        host_ip: Some("10.0.0.1".into()),
        host_ips: Some(vec![HostIP { ip: "10.0.0.1".into() }]),
        pod_ip: Some("10.42.0.1".into()),
        pod_ips: Some(vec![PodIP { ip: "10.42.0.1".into() }]),
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
                .map(|c| ContainerStatus {
                    name: c.name.clone(),
                    image: c.image.clone().unwrap_or_default(),
                    image_id: c.image.clone().map(|i| format!("z8s://{}", i)).unwrap_or_default(),
                    ready: is_ready,
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
                })
                .collect()
        }),
        qos_class: Some("Burstable".into()),
        ..Default::default()
    };

    let mut pod = pod.clone();
    fill_pod_metadata(&mut pod);
    pod.status = Some(status);
    serde_json::to_value(&pod).unwrap_or_default()
}

fn pod_condition(
    type_: &str,
    status: &str,
    time: &k8s_openapi::apimachinery::pkg::apis::meta::v1::Time,
) -> PodCondition {
    PodCondition {
        type_: type_.into(),
        status: status.into(),
        last_transition_time: Some(time.clone()),
        ..Default::default()
    }
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
                    let ready = cs.iter().filter(|c| c["ready"].as_bool().unwrap_or(false)).count();
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

async fn list_pods_all(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_pods_in_ns(state, None, headers).await
}

async fn list_pods(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_pods_in_ns(state, Some(namespace), headers).await
}

async fn list_pods_in_ns(
    state: AppState,
    namespace: Option<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    let mut items = Vec::new();
    for t in &trackers {
        if namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns) {
            let ready = state.supervisor.is_pod_ready(t.resource.name()).await;
            items.push(resource_to_pod_json_with_status(&t.resource, &t.state, ready));
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

async fn get_pod(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let ready = state.supervisor.is_pod_ready(t.resource.name()).await;
            return Ok(Json(resource_to_pod_json_with_status(&t.resource, &t.state, ready)));
        }
    }
    Err(ApiError::not_found(format!("pod \"{}\" not found", name)))
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
            return Err(ApiError::bad_request("no containers in pod".into()));
        }
    }
    Err(ApiError::not_found(format!("pod \"{}\" not found", name)))
}

async fn pod_handler(
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
            let mut pod: k8s_openapi::api::core::v1::Pod = serde_json::from_value(body)
                .map_err(|e| ApiError::bad_request(format!("invalid Pod: {}", e)))?;
            if pod.metadata.namespace.is_none() {
                pod.metadata.namespace = Some(namespace);
            }
            if pod.metadata.name.is_none() {
                pod.metadata.name = Some(name);
            }
            fill_pod_metadata(&mut pod);
            let resource = AnyResource::Pod(pod);
            state.store.apply(resource.clone()).await
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let tracker_state = state.store.get(&resource.uid()).await
                .map(|t| t.state)
                .unwrap_or(ResourceState::Pending);
            let is_ready = state.supervisor.is_pod_ready(resource.name()).await;
            Ok(Json(resource_to_pod_json_with_status(&resource, &tracker_state, is_ready)).into_response())
        }
        _ => Err(ApiError::method_not_allowed("method not allowed".into())),
    }
}

async fn delete_pod(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            state.supervisor.stop_pod(&t.resource).await;
            state.store.delete(&t.resource).await.ok();
            info!("Deleted pod {}/{}", namespace, name);
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("pod \"{}\" not found", name)))
}

async fn create_pod(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("Pod");
    if kind != "Pod" {
        return Err(ApiError::bad_request(format!("expected Pod, got {}", kind)));
    }
    let mut pod: k8s_openapi::api::core::v1::Pod = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Pod: {}", e)))?;
    if pod.metadata.namespace.is_none() {
        pod.metadata.namespace = Some(namespace);
    }
    fill_pod_metadata(&mut pod);
    let resource = AnyResource::Pod(pod);
    state
        .store
        .apply(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let pod_state = match state.supervisor.start_pod(&resource).await {
        Ok(_) => ResourceState::Running,
        Err(e) => {
            tracing::warn!("Failed to start pod {}: {}", resource.name(), e);
            ResourceState::Failed(e.to_string())
        }
    };
    state.store.update_state(&resource.uid(), pod_state).await;
    let tracker_state = state.store.get(&resource.uid()).await
        .map(|t| t.state)
        .unwrap_or(ResourceState::Pending);
    let is_ready = state.supervisor.is_pod_ready(resource.name()).await;
    Ok((StatusCode::CREATED, Json(resource_to_pod_json_with_status(&resource, &tracker_state, is_ready))).into_response())
}

// ── Deployments ───────────────────────────────────────────────────────────────

fn resource_to_deploy_json(
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

fn count_deployment_pods(
    resource: &AnyResource,
    pods: &[crate::api::types::ResourceTracker],
    running: &HashMap<String, crate::supervisor::process::RunningContainer>,
) -> (usize, usize) {
    use crate::api::types::extract_containers;
    let deploy = match resource {
        AnyResource::Deployment(d) => d,
        _ => return (0, 0),
    };
    let namespace = deploy.metadata.namespace.as_deref().unwrap_or("default");
    let selector = deploy.spec.as_ref().and_then(|s| s.selector.match_labels.as_ref());

    let Some(labels) = selector else { return (0, 0) };

    let matching: Vec<_> = pods
        .iter()
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

    let ready = matching
        .iter()
        .filter(|t| {
            extract_containers(&t.resource).iter().any(|c| {
                let cid = format!("{}-{}", t.resource.name(), c.name);
                running
                    .get(&cid)
                    .map(|rc| {
                        let arc = rc.ready.clone();
                        arc.try_lock().map(|g| *g).unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
        })
        .count();

    (ready, matching.len())
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
                "cells": [name, format!("{}/{}", ready, total), up_to_date, available, age],
                "object": item,
            })
        })
        .collect();
    make_table(columns, rows)
}

async fn list_deployments_all(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_deployments_in_ns(state, None, headers).await
}

async fn list_deployments(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    list_deployments_in_ns(state, Some(namespace), headers).await
}

async fn list_deployments_in_ns(
    state: AppState,
    namespace: Option<String>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    let running = state.supervisor.running.lock().await;
    let pods = state.store.get_by_kind("Pod").await;

    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .map(|t| {
            let (ready, avail) = count_deployment_pods(&t.resource, &pods, &running);
            resource_to_deploy_json(&t.resource, Some(ready), Some(avail))
        })
        .collect();
    drop(running);

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

async fn get_deployment(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    let running = state.supervisor.running.lock().await;
    let pods = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let (ready, avail) = count_deployment_pods(&t.resource, &pods, &running);
            return Ok(Json(resource_to_deploy_json(&t.resource, Some(ready), Some(avail))));
        }
    }
    Err(ApiError::not_found(format!("deployment \"{}\" not found", name)))
}

async fn patch_deployment_scale(
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

async fn create_deployment(
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

async fn delete_deployment(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Deployment").await;
    for t in &trackers {
        if t.resource.name() == name && t.resource.namespace() == namespace {
            if let AnyResource::Deployment(deploy) = &t.resource {
                // Cascade delete: stop and remove all pods matching this deployment's selector
                let selector = deploy.spec.as_ref()
                    .and_then(|s| s.selector.match_labels.as_ref());
                if let Some(match_labels) = selector {
                    let pods = state.store.get_by_kind("Pod").await;
                    for pt in &pods {
                        if pt.resource.namespace() != namespace { continue; }
                        if let AnyResource::Pod(pod) = &pt.resource {
                            let pod_labels = pod.metadata.labels.clone().unwrap_or_default();
                            if labels_match(match_labels, &pod_labels) {
                                info!("Deleting pod {} owned by deployment {}/{}", pt.resource.name(), namespace, name);
                                state.supervisor.stop_pod(&pt.resource).await;
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

async fn patch_deployment(
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

// ── Namespaces ────────────────────────────────────────────────────────────────

async fn list_namespaces(State(state): State<AppState>) -> Json<List<Namespace>> {
    let ns = state.namespaces.read().await;
    Json(List {
        items: ns.values().cloned().collect(),
        metadata: make_list_meta(),
    })
}

async fn get_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Namespace>, ApiError> {
    let ns = state.namespaces.read().await;
    ns.get(&name)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("namespace \"{}\" not found", name)))
}

async fn create_namespace(
    State(state): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let val = parse_body(&raw)?;
    let mut ns: Namespace = serde_json::from_value(val)
        .map_err(|e| ApiError::bad_request(format!("invalid Namespace: {}", e)))?;
    let name = ns
        .metadata
        .name
        .clone()
        .unwrap_or_else(|| format!("ns-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("x")));

    ns.metadata.name = Some(name.clone());
    if ns.metadata.uid.is_none() {
        ns.metadata.uid = Some(format!("ns-{}", name));
    }
    if ns.metadata.creation_timestamp.is_none() {
        ns.metadata.creation_timestamp = Some(now_time());
    }
    ns.status = Some(NamespaceStatus {
        phase: Some("Active".into()),
        ..Default::default()
    });

    let mut store = state.namespaces.write().await;
    if store.contains_key(&name) {
        return Err(ApiError::bad_request(format!("namespace \"{}\" already exists", name)));
    }
    info!("Created namespace: {}", name);
    store.insert(name, ns.clone());
    Ok((StatusCode::CREATED, Json(ns)).into_response())
}

async fn delete_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Status>, ApiError> {
    if name == "default" {
        return Err(ApiError::bad_request("cannot delete default namespace".into()));
    }
    let mut ns = state.namespaces.write().await;
    if ns.remove(&name).is_some() {
        info!("Deleted namespace: {}", name);
        Ok(Json(ok_status()))
    } else {
        Err(ApiError::not_found(format!("namespace \"{}\" not found", name)))
    }
}

// ── Nodes ─────────────────────────────────────────────────────────────────────

async fn list_nodes() -> Json<List<Node>> {
    let time = now_time();
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get().to_string())
        .unwrap_or_else(|_| "1".into());
    let mut labels = BTreeMap::new();
    labels.insert("kubernetes.io/hostname".into(), "z8s-node".into());
    labels.insert("kubernetes.io/os".into(), "linux".into());
    labels.insert("kubernetes.io/arch".into(), detect_arch());
    labels.insert("beta.kubernetes.io/os".into(), "linux".into());
    labels.insert("beta.kubernetes.io/arch".into(), detect_arch());

    let mut capacity = BTreeMap::new();
    capacity.insert("cpu".into(), Quantity(cpu_count.clone()));
    capacity.insert("memory".into(), Quantity(host_memory_ki()));
    capacity.insert("pods".into(), Quantity("110".into()));

    Json(List {
        items: vec![Node {
            metadata: ObjectMeta {
                name: Some("z8s-node".into()),
                uid: Some("z8s-node".into()),
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
                    NodeAddress { type_: "InternalIP".into(), address: "127.0.0.1".into() },
                    NodeAddress { type_: "Hostname".into(), address: "z8s-node".into() },
                ]),
                daemon_endpoints: Some(NodeDaemonEndpoints {
                    kubelet_endpoint: Some(DaemonEndpoint { port: Z8S_PORT as i32 }),
                }),
                node_info: Some(NodeSystemInfo {
                    machine_id: "z8s-1".into(),
                    system_uuid: "z8s-1".into(),
                    boot_id: "z8s-1".into(),
                    kernel_version: kernel_version(),
                    os_image: "Linux".into(),
                    container_runtime_version: format!("z8s://{}", env!("CARGO_PKG_VERSION")),
                    kubelet_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                    kube_proxy_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                    operating_system: "linux".into(),
                    architecture: detect_arch(),
                    swap: None,
                }),
                capacity: Some(capacity.clone()),
                allocatable: Some(capacity),
                ..Default::default()
            }),
        }],
        metadata: make_list_meta(),
    })
}

async fn get_node(Path(name): Path<String>) -> Result<Json<Node>, ApiError> {
    if name == "z8s-node" {
        let list = list_nodes().await;
        list.0.items.into_iter().next()
            .map(Json)
            .ok_or_else(|| ApiError::not_found("node not found".into()))
    } else {
        Err(ApiError::not_found(format!("node \"{}\" not found", name)))
    }
}

fn detect_arch() -> String {
    #[cfg(target_arch = "x86_64")]
    return "amd64".into();
    #[cfg(target_arch = "aarch64")]
    return "arm64".into();
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return std::env::consts::ARCH.into();
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
                .map(|kb| format!("{}Ki", kb))
        })
        .unwrap_or_else(|| "8192Ki".into())
}

// ── Events ────────────────────────────────────────────────────────────────────

async fn list_events_all(State(state): State<AppState>) -> Json<List<Event>> {
    let ev = state.events.lock().await;
    Json(List {
        items: ev.clone(),
        metadata: make_list_meta(),
    })
}

async fn list_events(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<Event>> {
    let ev = state.events.lock().await;
    Json(List {
        items: ev
            .iter()
            .filter(|e| e.metadata.namespace.as_deref() == Some(&namespace))
            .cloned()
            .collect(),
        metadata: make_list_meta(),
    })
}

// ── Metrics ───────────────────────────────────────────────────────────────────

async fn metrics_api_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "metrics.k8s.io/v1beta1".into(),
        resources: vec![
            api_resource("pods", "", true, "PodMetrics", &["get", "list"], &[], &[]),
            api_resource("nodes", "", false, "NodeMetrics", &["get", "list"], &[], &[]),
        ],
    })
}

async fn metrics_top_nodes() -> Json<serde_json::Value> {
    let now = now_rfc3339();
    let cpu_usec = cgroup_cpu_usage("z8s");
    let mem_bytes = cgroup_memory_current("z8s");
    Json(serde_json::json!({
        "kind": "NodeMetricsList",
        "apiVersion": "metrics.k8s.io/v1beta1",
        "metadata": { "resourceVersion": "1" },
        "items": [{
            "metadata": {"name": "z8s-node", "creationTimestamp": now},
            "timestamp": now, "window": "1m0s",
            "usage": {
                "cpu": format!("{}n", cpu_usec * 1000),
                "memory": format!("{}Ki", mem_bytes / 1024)
            }
        }]
    }))
}

async fn top_pods_all(State(state): State<AppState>) -> Json<serde_json::Value> {
    top_pods_in_ns(state, None).await
}

async fn top_pods(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<serde_json::Value> {
    top_pods_in_ns(state, Some(namespace)).await
}

async fn top_pods_in_ns(state: AppState, namespace: Option<String>) -> Json<serde_json::Value> {
    let trackers = state.store.get_by_kind("Pod").await;
    let now = now_rfc3339();
    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .map(|t| {
            let pod_uid = t.resource.uid();
            let cg = sanitize_cg(&pod_uid);
            let mem = cgroup_memory_current(&cg);
            let cpu = cgroup_cpu_usage(&cg);
            serde_json::json!({
                "metadata": {
                    "name": t.resource.name(),
                    "namespace": t.resource.namespace(),
                    "creationTimestamp": now,
                },
                "timestamp": now, "window": "1m0s",
                "containers": [{
                    "name": t.resource.name(),
                    "usage": {
                        "cpu": format!("{}n", cpu * 1000),
                        "memory": format!("{}Ki", mem / 1024),
                    }
                }]
            })
        })
        .collect();
    Json(serde_json::json!({
        "kind": "PodMetricsList",
        "apiVersion": "metrics.k8s.io/v1beta1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}

fn sanitize_cg(name: &str) -> String {
    name.replace(['/', '.', ':'], "_")
}

fn cgroup_memory_current(cg: &str) -> u64 {
    std::fs::read_to_string(format!("/sys/fs/cgroup/z8s/{}/memory.current", cg))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn cgroup_cpu_usage(cg: &str) -> u64 {
    std::fs::read_to_string(format!("/sys/fs/cgroup/z8s/{}/cpu.stat", cg))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("usage_usec "))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

// ── OpenAPI / version / health ────────────────────────────────────────────────

async fn openapi_v2(headers: axum::http::HeaderMap) -> Result<axum::response::Response, ApiError> {
    let _ = headers;
    let schema = serde_json::json!({
        "swagger": "2.0",
        "info": {"title": "z8s", "version": env!("CARGO_PKG_VERSION")},
        "paths": {}
    });
    Ok((StatusCode::OK, [("Content-Type", "application/json")], Json(schema)).into_response())
}

async fn openapi_v3() -> impl IntoResponse {
    Json(serde_json::json!({
        "openapi": "3.0.0",
        "info": {"title": "z8s", "version": env!("CARGO_PKG_VERSION")},
        "paths": {}
    }))
}

async fn version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "major": "0",
        "minor": "1",
        "gitVersion": format!("z8s-v{}", env!("CARGO_PKG_VERSION")),
        "gitCommit": "dev",
        "buildDate": now_rfc3339(),
        "goVersion": "go1.21",
        "compiler": "rustc",
        "platform": format!("linux/{}", detect_arch())
    }))
}

async fn healthz() -> &'static str { "ok" }
async fn readyz() -> &'static str { "ok" }
async fn livez() -> &'static str { "ok" }

async fn fallback_handler(uri: Uri) -> impl IntoResponse {
    let status = Status {
        status: Some("Failure".into()),
        message: Some(format!("no route found for {}", uri.path())),
        reason: Some("NotFound".into()),
        code: Some(404),
        ..Default::default()
    };
    (StatusCode::NOT_FOUND, Json(status))
}

// ── ConfigMaps ────────────────────────────────────────────────────────────────

async fn list_configmaps_all(State(state): State<AppState>) -> Json<serde_json::Value> {
    list_configmaps_in_ns(&state, None).await
}

async fn list_configmaps(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<serde_json::Value> {
    list_configmaps_in_ns(&state, Some(namespace)).await
}

async fn list_configmaps_in_ns(state: &AppState, namespace: Option<String>) -> Json<serde_json::Value> {
    let trackers = state.store.get_by_kind("ConfigMap").await;
    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .filter_map(|t| serde_json::to_value(&t.resource).ok())
        .collect();
    Json(serde_json::json!({
        "kind": "ConfigMapList", "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}

async fn get_configmap(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("ConfigMap").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("configmap \"{}/{}\" not found", namespace, name)))
}

async fn create_configmap(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut cm: ConfigMap = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid ConfigMap: {}", e)))?;
    if cm.metadata.namespace.is_none() {
        cm.metadata.namespace = Some(namespace);
    }
    if cm.metadata.uid.is_none() {
        cm.metadata.uid = Some(uuid::Uuid::new_v4().to_string());
    }
    if cm.metadata.creation_timestamp.is_none() {
        cm.metadata.creation_timestamp = Some(now_time());
    }
    let resource = AnyResource::ConfigMap(cm);
    let already_exists = state.store.get_by_kind("ConfigMap").await.iter()
        .any(|t| t.resource.name() == resource.name() && t.resource.namespace() == resource.namespace());
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}

async fn update_configmap(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    // Merge patch onto existing resource (handles strategic merge patch null = delete semantics)
    let existing = state.store.get_by_kind("ConfigMap").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::ConfigMap(cm) = t.resource { serde_json::to_value(cm).ok() } else { None });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut cm: ConfigMap = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid ConfigMap: {}", e)))?;
    if cm.metadata.namespace.is_none() { cm.metadata.namespace = Some(namespace); }
    if cm.metadata.name.is_none() { cm.metadata.name = Some(name); }
    let resource = AnyResource::ConfigMap(cm);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}

async fn delete_configmap(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("ConfigMap").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("configmap \"{}/{}\" not found", namespace, name)))
}

// ── Secrets ───────────────────────────────────────────────────────────────────

async fn list_secrets_all(State(state): State<AppState>) -> Json<serde_json::Value> {
    list_secrets_in_ns(&state, None).await
}

async fn list_secrets(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<serde_json::Value> {
    list_secrets_in_ns(&state, Some(namespace)).await
}

async fn list_secrets_in_ns(state: &AppState, namespace: Option<String>) -> Json<serde_json::Value> {
    let trackers = state.store.get_by_kind("Secret").await;
    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .filter_map(|t| serde_json::to_value(&t.resource).ok())
        .collect();
    Json(serde_json::json!({
        "kind": "SecretList", "apiVersion": "v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}

async fn get_secret(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Secret").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("secret \"{}/{}\" not found", namespace, name)))
}

async fn create_secret(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut sec: Secret = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Secret: {}", e)))?;
    if sec.metadata.namespace.is_none() {
        sec.metadata.namespace = Some(namespace);
    }
    if sec.metadata.uid.is_none() {
        sec.metadata.uid = Some(uuid::Uuid::new_v4().to_string());
    }
    if sec.metadata.creation_timestamp.is_none() {
        sec.metadata.creation_timestamp = Some(now_time());
    }
    let resource = AnyResource::Secret(sec);
    let already_exists = state.store.get_by_kind("Secret").await.iter()
        .any(|t| t.resource.name() == resource.name() && t.resource.namespace() == resource.namespace());
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}

async fn update_secret(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = state.store.get_by_kind("Secret").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::Secret(sec) = t.resource { serde_json::to_value(sec).ok() } else { None });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut sec: Secret = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Secret: {}", e)))?;
    if sec.metadata.namespace.is_none() { sec.metadata.namespace = Some(namespace); }
    if sec.metadata.name.is_none() { sec.metadata.name = Some(name); }
    let resource = AnyResource::Secret(sec);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}

async fn delete_secret(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Secret").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("secret \"{}/{}\" not found", namespace, name)))
}

fn labels_match(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    for (key, value) in selector {
        if labels.get(key) != Some(value) {
            return false;
        }
    }
    true
}

// ── Error type ────────────────────────────────────────────────────────────────

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn not_found(msg: String) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: msg }
    }
    pub fn bad_request(msg: String) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: msg }
    }
    pub fn method_not_allowed(msg: String) -> Self {
        Self { status: StatusCode::METHOD_NOT_ALLOWED, message: msg }
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
            409 => "Conflict",
            500 => "InternalError",
            503 => "ServiceUnavailable",
            _ => "Unknown",
        };
        let body = Status {
            status: Some("Failure".into()),
            message: Some(self.message),
            reason: Some(reason.into()),
            code: Some(self.status.as_u16() as i32),
            ..Default::default()
        };
        (self.status, Json(body)).into_response()
    }
}
