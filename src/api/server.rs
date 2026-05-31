pub use crate::api::types::{ResourceState, StoreBackend, MemoryBackend};
pub use crate::api::AnyResource;
pub use axum::extract::{Path, State};
pub use axum::http::{Method, StatusCode, Uri};
pub use axum::response::{IntoResponse, Json};
pub use axum::Router;
pub use crate::types::{
    APIGroup, APIGroupList, APIResource, APIResourceList, APIVersions, GroupVersionForDiscovery,
    ListMeta, ObjectMeta, Status,
    DeploymentCondition, DeploymentStatus,
    ConfigMap, ContainerState, ContainerStateRunning, ContainerStateTerminated, ContainerStateWaiting, ContainerStatus, DaemonEndpoint,
    Endpoints, Event, EventSource, HostIP, Ingress, Namespace, NamespaceStatus,
    NetworkPolicy, Node, NodeAddress,
    NodeCondition, NodeDaemonEndpoints, NodeSpec, NodeStatus, NodeSystemInfo,
    ObjectReference, PodCondition, PodIP, PodStatus, Secret, Service, ServiceStatus,
    EndpointSlice, Quantity, List, Scale, ScaleSpec, ScaleStatus,
    SelfSubjectAccessReview, SelfSubjectAccessReviewSpec, SubjectAccessReviewStatus,
};
pub use std::collections::{BTreeMap, HashMap};
pub use std::sync::Arc;
pub use tokio::sync::{Mutex, RwLock};
pub use tracing::info;

use crate::scheduler::process::ProcessTracker;
use crate::components::{ComponentRegistry, ReconcileContext};

impl axum::extract::FromRef<AppState> for crate::cri::exec::ExecState {
    fn from_ref(state: &AppState) -> Self {
        crate::cri::exec::ExecState(state.process_tracker.running.clone())
    }
}

// ── Core server infrastructure ───────────────────────────────────────────────

pub fn z8s_port() -> u16 {
    crate::config::get().api_port
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn StoreBackend>,
    pub process_tracker: Arc<ProcessTracker>,
    pub registry: Arc<ComponentRegistry>,
    pub ctx: Arc<ReconcileContext>,
    pub gossip_state: Option<Arc<tokio::sync::Mutex<crate::store::gossip::GossipState>>>,
}

impl AppState {
    pub async fn pod_ready(&self, name: &str) -> bool {
        self.process_tracker.is_ready(name).await
    }

    pub async fn pod_running(&self, name: &str) -> bool {
        self.process_tracker.is_running(name).await
    }

    pub async fn pod_logs(&self, pod: &str, container: &str) -> Vec<String> {
        self.process_tracker.get_logs(pod, container).await
    }

    pub async fn pod_restarts(&self, name: &str) -> HashMap<String, u32> {
        self.process_tracker.pod_restart_counts(name).await
    }

    pub async fn backend_port(&self, pod: &str, port: u16) -> u16 {
        self.process_tracker.backend_connect_port(pod, port).await
    }

    /// Apply a resource and broadcast to gossip peers
    pub async fn apply_and_broadcast(&self, resource: AnyResource) -> anyhow::Result<()> {
        self.store.apply(resource.clone()).await?;
        if let Some(ref gs) = self.gossip_state {
            gs.lock().await.broadcast_write(&resource).await;
        }
        Ok(())
    }
}

pub async fn build_app_state(
    store: Arc<dyn StoreBackend>,
    process_tracker: Arc<ProcessTracker>,
    registry: Arc<ComponentRegistry>,
    ctx: Arc<ReconcileContext>,
    gossip_state: Option<Arc<tokio::sync::Mutex<crate::store::gossip::GossipState>>>,
) -> AppState {
    // Log store state on startup
    let pods = store.get_by_kind("Pod").await.len();
    let svcs = store.get_by_kind("Service").await.len();
    let deploys = store.get_by_kind("Deployment").await.len();
    let ns_count = store.get_by_kind("Namespace").await.len();
    info!("Store state: {} namespaces, {} pods, {} services, {} deployments", ns_count, pods, svcs, deploys);

    // Ensure default namespace exists in the store
    let existing = store.get_by_kind("Namespace").await;
    if !existing.iter().any(|t| t.resource.name() == "default") {
        let ns = make_namespace("default", "ns-default");
        info!("Creating default namespace");
        store.apply(AnyResource::Namespace(ns)).await.ok();
    }
    AppState { store, process_tracker, registry, ctx, gossip_state }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::api::handlers::system::routes())
        .merge(crate::api::handlers::pod::routes())
        .merge(crate::api::handlers::deployment::routes())
        .merge(crate::api::handlers::namespace::routes())
        .merge(crate::api::handlers::node::routes())
        .merge(crate::api::handlers::event::routes())
        .merge(crate::api::handlers::configmap::routes())
        .merge(crate::api::handlers::secret::routes())
        .merge(crate::api::handlers::service::routes())
        .merge(crate::api::handlers::pv::routes())
        .merge(crate::api::handlers::pvc::routes())
        .merge(crate::api::handlers::endpoints::routes())
        .merge(crate::api::handlers::endpointslices::routes())
        .merge(crate::api::handlers::metrics::routes())
        .merge(crate::api::handlers::storage_class::routes())
        .merge(crate::api::handlers::ingress::routes())
        .merge(crate::api::handlers::networkpolicy::routes())
        .merge(crate::api::handlers::vnet::routes())
        .merge(crate::api::handlers::subnet::routes())
        .merge(crate::api::handlers::nsg::routes())
        .merge(crate::api::handlers::routetable::routes())
        .route("/ws/gossip", axum::routing::any(gossip_ws_handler))
        .fallback(fallback_handler)
        .with_state(state)
}

pub async fn gossip_ws_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl axum::response::IntoResponse {
    match state.gossip_state {
        Some(ref gs) => {
            let gs = gs.clone();
            ws.on_upgrade(move |socket| {
                crate::store::ws::handle_gossip_ws(socket, gs)
            })
            .into_response()
        }
        None => (axum::http::StatusCode::SERVICE_UNAVAILABLE, "gossip not configured").into_response(),
    }
}

pub async fn run_server(
    store: Arc<dyn StoreBackend>,
    process_tracker: Arc<ProcessTracker>,
    registry: Arc<ComponentRegistry>,
    ctx: Arc<ReconcileContext>,
    gossip_state: Option<Arc<tokio::sync::Mutex<crate::store::gossip::GossipState>>>,
) {
    let state = build_app_state(store, process_tracker, registry, ctx, gossip_state).await;
    let app = build_router(state);
    let addr = format!("0.0.0.0:{}", z8s_port());
    info!("Starting k8s API server on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await
        .unwrap_or_else(|e| panic!("Failed to bind to {} — port in use? ({})", addr, e));
    axum::serve(listener, app).await.unwrap();
}

// ── Shared helpers ───────────────────────────────────────────────────────────

pub fn now_time() -> crate::types::Time {
    crate::types::Time(chrono::Utc::now().to_rfc3339())
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn make_namespace(name: &str, uid: &str) -> Namespace {
    Namespace {
        api_version: "v1".into(),
        kind: "Namespace".into(),
        metadata: ObjectMeta {
            name: Some(name.into()),
            uid: Some(uid.into()),
            creation_timestamp: Some(now_time()),
            ..Default::default()
        },
        spec: None,
        status: Some(NamespaceStatus { phase: Some("Active".into()), ..Default::default() }),
    }
}

pub fn accepts_table(headers: &axum::http::HeaderMap) -> bool {
    headers.get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("as=Table"))
        .unwrap_or(false)
}

pub fn age_from_timestamp(ts: &str) -> String {
    if let Ok(created) = chrono::DateTime::parse_from_rfc3339(ts) {
        let secs = chrono::Utc::now().signed_duration_since(created).num_seconds().max(0);
        if secs < 60 { format!("{}s", secs) }
        else if secs < 3600 { format!("{}m", secs / 60) }
        else if secs < 86400 { format!("{}h", secs / 3600) }
        else { format!("{}d", secs / 86400) }
    } else { "<unknown>".into() }
}

pub fn make_table(columns: serde_json::Value, rows: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({ "kind": "Table", "apiVersion": "meta.k8s.io/v1", "columnDefinitions": columns, "rows": rows })
}

pub fn ok_status() -> Status {
    Status { status: Some("Success".into()), code: Some(200), ..Default::default() }
}

pub fn make_list_meta() -> ListMeta {
    ListMeta { resource_version: Some("1".into()), ..Default::default() }
}

// ── Enriched response views for custom resources ──────────────────────────

/// Build enriched JSON for a VNet item with computed fields (subnet/pod/service counts).
pub fn enrich_vnet(mut v: serde_json::Value, subnets: usize, pods: usize, svcs: usize) -> serde_json::Value {
    let spec = v.as_object_mut().and_then(|o| o.get_mut("spec")).and_then(|s| s.as_object_mut());
    if let Some(s) = spec {
        s.insert("subnetCount".into(), serde_json::json!(subnets));
        s.insert("podCount".into(), serde_json::json!(pods));
        s.insert("serviceCount".into(), serde_json::json!(svcs));
    }
    v
}

/// Build enriched JSON for a Subnet item with computed fields (pod/service counts).
pub fn enrich_subnet(mut v: serde_json::Value, pods: usize, svcs: usize) -> serde_json::Value {
    let spec = v.as_object_mut().and_then(|o| o.get_mut("spec")).and_then(|s| s.as_object_mut());
    if let Some(s) = spec {
        s.insert("podCount".into(), serde_json::json!(pods));
        s.insert("serviceCount".into(), serde_json::json!(svcs));
    }
    v
}

/// Build enriched JSON for an NSG item with computed fields.
pub fn enrich_nsg(mut v: serde_json::Value, rules: usize, allows: usize, denies: usize, targets: &str) -> serde_json::Value {
    let spec = v.as_object_mut().and_then(|o| o.get_mut("spec")).and_then(|s| s.as_object_mut());
    if let Some(s) = spec {
        s.insert("ruleCount".into(), serde_json::json!(rules));
        s.insert("allowCount".into(), serde_json::json!(allows));
        s.insert("denyCount".into(), serde_json::json!(denies));
        s.insert("targets".into(), serde_json::json!(targets));
    }
    v
}

/// Build enriched JSON for a RouteTable item with computed fields.
pub fn enrich_routetable(mut v: serde_json::Value, rules: usize, allows: usize, denies: usize, methods: &[String]) -> serde_json::Value {
    let spec = v.as_object_mut().and_then(|o| o.get_mut("spec")).and_then(|s| s.as_object_mut());
    if let Some(s) = spec {
        s.insert("ruleCount".into(), serde_json::json!(rules));
        s.insert("allowCount".into(), serde_json::json!(allows));
        s.insert("denyCount".into(), serde_json::json!(denies));
        s.insert("methods".into(), serde_json::json!(methods));
    }
    v
}

/// Build a kubectl-compatible Table response from column definitions and JSON items.
/// This enables `kubectl get` to show custom columns instead of just NAME/AGE.
pub fn build_table(items: &[serde_json::Value], cols: &[(&str, &str, &str)]) -> serde_json::Value {
    let mut col_defs = Vec::new();
    col_defs.push(serde_json::json!({"name":"Name","type":"string","format":"name","description":"Name","priority":0}));
    for (name, json_path, col_type) in cols {
        col_defs.push(serde_json::json!({"name":name,"type":col_type,"jsonPath":json_path,"description":name,"priority":0}));
    }
    col_defs.push(serde_json::json!({"name":"Age","type":"date","description":"Age","priority":0}));

    let now = std::time::SystemTime::now();
    let rows: Vec<serde_json::Value> = items.iter().map(|item| {
        let name = item["metadata"]["name"].as_str().unwrap_or("");
        let age = format_ts_relative(item["metadata"]["creationTimestamp"].as_str().unwrap_or(""), now);
        let mut cells = vec![serde_json::json!(name)];
        for (_, json_path, _) in cols {
            cells.push(extract_json_path(item, json_path));
        }
        cells.push(serde_json::json!(age));
        serde_json::json!({"cells": cells, "object": item})
    }).collect();

    serde_json::json!({
        "kind": "Table",
        "apiVersion": "meta.k8s.io/v1",
        "metadata": {},
        "columnDefinitions": col_defs,
        "rows": rows,
    })
}

/// Convert a RFC3339 timestamp to a human-readable relative age (e.g. "5m", "2h", "7d").
pub fn format_ts_relative(ts: &str, now: std::time::SystemTime) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        let delta = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
            .saturating_sub(dt.timestamp() as u64);
        if delta < 60 { return format!("{}s", delta); }
        if delta < 3600 { return format!("{}m", delta / 60); }
        if delta < 86400 { return format!("{}h", delta / 3600); }
        return format!("{}d", delta / 86400);
    }
    ts.to_string()
}

/// Extract a value from a JSON object using a jsonPath expression (e.g. ".spec.cidr").
fn extract_json_path(obj: &serde_json::Value, path: &str) -> serde_json::Value {
    if !path.starts_with('.') { return serde_json::Value::Null; }
    let parts: Vec<&str> = path[1..].split('.').collect();
    let mut current = obj;
    for part in &parts {
        current = match current {
            serde_json::Value::Object(m) => m.get(*part).unwrap_or(&serde_json::Value::Null),
            _ => return serde_json::Value::Null,
        };
    }
    if let Some(s) = current.as_str() {
        // Try to parse as number for "integer" columns
        if let Ok(n) = s.parse::<i64>() { return serde_json::json!(n); }
    }
    current.clone()
}

pub fn json_merge_patch(base: &mut serde_json::Value, patch: &serde_json::Value) {
    if let (serde_json::Value::Object(b), serde_json::Value::Object(p)) = (base, patch) {
        for (k, v) in p {
            if v.is_null() { b.remove(k); }
            else if v.is_object() {
                let entry = b.entry(k.clone()).or_insert(serde_json::Value::Object(Default::default()));
                json_merge_patch(entry, v);
            } else { b.insert(k.clone(), v.clone()); }
        }
    }
}

pub fn parse_body(bytes: &axum::body::Bytes) -> Result<serde_json::Value, ApiError> {
    if let Some(v) = crate::api::proto::try_proto_to_json(bytes) { return Ok(v); }
    if let Ok(v) = serde_json::from_slice(bytes) { return Ok(v); }
    serde_yaml::from_slice::<serde_yaml::Value>(bytes)
        .ok().and_then(|y| serde_json::to_value(y).ok())
        .ok_or_else(|| ApiError::bad_request("invalid body: not valid JSON or YAML".to_string()))
}

pub fn detect_arch() -> String {
    #[cfg(target_arch = "x86_64")] { return "amd64".into(); }
    #[cfg(target_arch = "aarch64")] { return "arm64".into(); }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    { std::env::consts::ARCH.into() }
}

pub async fn fallback_handler(uri: Uri) -> impl IntoResponse {
    let status = Status {
        status: Some("Failure".into()),
        message: Some(format!("no route found for {}", uri.path())),
        reason: Some("NotFound".into()),
        code: Some(404),
        ..Default::default()
    };
    (StatusCode::NOT_FOUND, Json(status))
}

pub fn api_resource(
    name: &str, singular: &str, namespaced: bool, kind: &str,
    verbs: &[&str], short_names: &[&str], categories: &[&str],
) -> APIResource {
    APIResource {
        name: name.into(), singular_name: singular.into(), namespaced, kind: kind.into(),
        verbs: verbs.iter().map(|s| s.to_string()).collect(),
        short_names: if short_names.is_empty() { None } else { Some(short_names.iter().map(|s| s.to_string()).collect()) },
        categories: if categories.is_empty() { None } else { Some(categories.iter().map(|s| s.to_string()).collect()) },
        group: None, version: None, storage_version_hash: None,
    }
}

pub fn gvd(group_version: &str, version: &str) -> GroupVersionForDiscovery {
    GroupVersionForDiscovery { group_version: group_version.into(), version: version.into() }
}

// ── Error type ───────────────────────────────────────────────────────────────

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn not_found(msg: String) -> Self { Self { status: StatusCode::NOT_FOUND, message: msg } }
    pub fn bad_request(msg: String) -> Self { Self { status: StatusCode::BAD_REQUEST, message: msg } }
    pub fn method_not_allowed(msg: String) -> Self { Self { status: StatusCode::METHOD_NOT_ALLOWED, message: msg } }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let reason = match self.status.as_u16() {
            400 => "BadRequest", 401 => "Unauthorized", 403 => "Forbidden",
            404 => "NotFound", 405 => "MethodNotAllowed", 409 => "Conflict",
            500 => "InternalError", 503 => "ServiceUnavailable", _ => "Unknown",
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

// ── E2E tests ────────────────────────────────────────────────────────────────

pub static NODEPORT_COUNTER: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(30000);

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    pub async fn make_app() -> axum::Router {
        let store: Arc<dyn StoreBackend> = Arc::new(MemoryBackend::new());
        let cgroup = Arc::new(crate::cri::cgroup::CgroupManager::new()
            .unwrap_or_else(|_| crate::cri::cgroup::CgroupManager::new().unwrap()));
        let image = Arc::new(crate::cri::image::ImageManager::new()
            .unwrap_or_else(|_| crate::cri::image::ImageManager::new().unwrap()));
        let supervisor = Arc::new(crate::cri::runtime::ProcessSupervisor::new(image, cgroup.clone(), Arc::new(crate::netmux::NetMux::new(&crate::config::get().pod_cidr).unwrap())));
        let container_runtime = Arc::new(crate::cri::runtime::ContainerRuntime::new(supervisor.clone(), cgroup));
        let process_tracker = Arc::new(ProcessTracker {
            running: supervisor.running.clone(),
            restart_counts: supervisor.restart_counts.clone(),
            cri: container_runtime.clone(),
            store: store.clone(),
        });
        let test_netmux = Arc::new(crate::netmux::NetMux::new(&crate::config::get().pod_cidr).unwrap());
        let network = Arc::new(crate::components::network::service::NetworkManager::new(store.clone(), process_tracker.clone(), test_netmux.clone()));
        let pipeline = Arc::new(crate::components::ReconciliationPipeline::builder().build());
        let ctx = Arc::new(crate::components::ReconcileContext {
            store: store.clone(),
            pipeline: pipeline.clone(),
            cri: container_runtime.clone() as Arc<dyn crate::cri::RuntimeProvider>,
            net: network.clone() as Arc<dyn crate::netmux::network::NetworkEngine>,
            process_tracker: process_tracker.clone(),
            vol: Arc::new(crate::storage::ProvisionerDispatcher::new(store.clone())) as Arc<dyn crate::storage::StorageProvisioner>,
            netmux: test_netmux.clone(),
        });
        let registry = Arc::new(crate::components::ComponentRegistry::new());
        let gossip_state = None;
        let state = build_app_state(store, process_tracker, registry, ctx, gossip_state).await;
        build_router(state)
    }

    pub async fn make_app_with_store() -> (axum::Router, Arc<dyn StoreBackend>) {
        let store: Arc<dyn StoreBackend> = Arc::new(MemoryBackend::new());
        let cgroup = Arc::new(crate::cri::cgroup::CgroupManager::new()
            .unwrap_or_else(|_| crate::cri::cgroup::CgroupManager::new().unwrap()));
        let image = Arc::new(crate::cri::image::ImageManager::new()
            .unwrap_or_else(|_| crate::cri::image::ImageManager::new().unwrap()));
        let supervisor = Arc::new(crate::cri::runtime::ProcessSupervisor::new(image, cgroup.clone(), Arc::new(crate::netmux::NetMux::new(&crate::config::get().pod_cidr).unwrap())));
        let container_runtime = Arc::new(crate::cri::runtime::ContainerRuntime::new(supervisor.clone(), cgroup));
        let process_tracker = Arc::new(ProcessTracker {
            running: supervisor.running.clone(),
            restart_counts: supervisor.restart_counts.clone(),
            cri: container_runtime.clone(),
            store: store.clone(),
        });
        let test_netmux = Arc::new(crate::netmux::NetMux::new(&crate::config::get().pod_cidr).unwrap());
        let network = Arc::new(crate::components::network::service::NetworkManager::new(store.clone(), process_tracker.clone(), test_netmux.clone()));
        let pipeline = Arc::new(crate::components::ReconciliationPipeline::builder().build());
        let ctx = Arc::new(crate::components::ReconcileContext {
            store: store.clone(),
            pipeline: pipeline.clone(),
            cri: container_runtime.clone() as Arc<dyn crate::cri::RuntimeProvider>,
            net: network.clone() as Arc<dyn crate::netmux::network::NetworkEngine>,
            process_tracker: process_tracker.clone(),
            vol: Arc::new(crate::storage::ProvisionerDispatcher::new(store.clone())) as Arc<dyn crate::storage::StorageProvisioner>,
            netmux: test_netmux.clone(),
        });
        let registry = Arc::new(crate::components::ComponentRegistry::new());
        let gossip_state = None;
        let state = build_app_state(store.clone(), process_tracker, registry, ctx, gossip_state).await;
        (build_router(state), store)
    }



    pub fn json_body(body: &str) -> Body { Body::from(body.to_string()) }

    #[tokio::test]
    pub async fn healthz_returns_ok() {
        let app = make_app().await;
        let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    pub async fn readyz_returns_ok() {
        let app = make_app().await;
        let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    pub async fn version_returns_json() {
        let app = make_app().await;
        let resp = app.oneshot(Request::builder().uri("/version").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v.get("gitVersion").is_some());
    }

    #[tokio::test]
    pub async fn create_and_get_configmap() {
        let cm_json = r#"{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"test-cm","namespace":"default"},"data":{"key":"value"}}"#;
        let (app, _) = make_app_with_store().await;
        let create_resp = app.clone().oneshot(Request::builder()
            .method("POST").uri("/api/v1/namespaces/default/configmaps")
            .header("content-type", "application/json").body(json_body(cm_json)).unwrap()).await.unwrap();
        assert!(create_resp.status() == StatusCode::CREATED || create_resp.status() == StatusCode::OK);
        let get_resp = app.oneshot(Request::builder()
            .uri("/api/v1/namespaces/default/configmaps/test-cm").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(get_resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(get_resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["metadata"]["name"], "test-cm");
        assert_eq!(v["data"]["key"], "value");
    }

    #[tokio::test]
    pub async fn get_nonexistent_configmap_returns_404() {
        let app = make_app().await;
        let resp = app.oneshot(Request::builder()
            .uri("/api/v1/namespaces/default/configmaps/missing").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    pub async fn configmaps_in_different_namespaces_do_not_collide() {
        let (app, store) = make_app_with_store().await;
        let cm_a: ConfigMap = serde_json::from_value(serde_json::json!({
            "apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"shared","namespace":"ns-a"},"data":{"env":"production"}
        })).unwrap();
        let cm_b: ConfigMap = serde_json::from_value(serde_json::json!({
            "apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"shared","namespace":"ns-b"},"data":{"env":"staging"}
        })).unwrap();
        store.apply(AnyResource::ConfigMap(cm_a)).await.unwrap();
        store.apply(AnyResource::ConfigMap(cm_b)).await.unwrap();
        assert_eq!(store.get_by_kind("ConfigMap").await.len(), 2);
        let resp_a = app.clone().oneshot(Request::builder()
            .uri("/api/v1/namespaces/ns-a/configmaps/shared").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK);
        let body_a = axum::body::to_bytes(resp_a.into_body(), usize::MAX).await.unwrap();
        let v_a: serde_json::Value = serde_json::from_slice(&body_a).unwrap();
        assert_eq!(v_a["data"]["env"], "production");
        let resp_b = app.oneshot(Request::builder()
            .uri("/api/v1/namespaces/ns-b/configmaps/shared").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp_b.status(), StatusCode::OK);
        let body_b = axum::body::to_bytes(resp_b.into_body(), usize::MAX).await.unwrap();
        let v_b: serde_json::Value = serde_json::from_slice(&body_b).unwrap();
        assert_eq!(v_b["data"]["env"], "staging");
    }

    #[tokio::test]
    pub async fn label_selector_filters_pods() {
        let (app, store) = make_app_with_store().await;
        let pod_a: crate::types::Pod = serde_json::from_value(serde_json::json!({
            "apiVersion":"v1","kind":"Pod","metadata":{"name":"pod-a","namespace":"default","labels":{"app":"web"}},
            "spec":{"containers":[{"name":"c","image":"alpine"}]}
        })).unwrap();
        let pod_b: crate::types::Pod = serde_json::from_value(serde_json::json!({
            "apiVersion":"v1","kind":"Pod","metadata":{"name":"pod-b","namespace":"default","labels":{"app":"db"}},
            "spec":{"containers":[{"name":"c","image":"postgres"}]}
        })).unwrap();
        store.apply(AnyResource::Pod(pod_a)).await.unwrap();
        store.apply(AnyResource::Pod(pod_b)).await.unwrap();
        let resp = app.clone().oneshot(Request::builder()
            .uri("/api/v1/namespaces/default/pods").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
        let resp = app.oneshot(Request::builder()
            .uri("/api/v1/namespaces/default/pods?labelSelector=app%3Dweb").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["metadata"]["name"], "pod-a");
    }

    #[tokio::test]
    pub async fn create_service_defaults_target_port() {
        let app = make_app().await;
        let svc_json = r#"{"apiVersion":"v1","kind":"Service","metadata":{"name":"my-svc","namespace":"default"},"spec":{"selector":{"app":"web"},"ports":[{"port":80,"protocol":"TCP"}]}}"#;
        let resp = app.clone().oneshot(Request::builder()
            .method("POST").uri("/api/v1/namespaces/default/services")
            .header("content-type", "application/json").body(json_body(svc_json)).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!v["spec"]["ports"][0]["targetPort"].is_null());
    }

    #[tokio::test]
    pub async fn list_services_returns_service_list_kind() {
        let app = make_app().await;
        let resp = app.oneshot(Request::builder()
            .uri("/api/v1/namespaces/default/services").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["kind"], "ServiceList");
    }

    #[tokio::test]
    pub async fn create_and_list_pv() {
        let app = make_app().await;
        let pv_json = r#"{"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":"pv1"},"spec":{"capacity":{"storage":"5Gi"},"accessModes":["ReadWriteOnce"],"hostPath":{"path":"/data"}}}"#;
        let resp = app.clone().oneshot(Request::builder()
            .method("POST").uri("/api/v1/persistentvolumes")
            .header("content-type", "application/json").body(json_body(pv_json)).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let resp = app.oneshot(Request::builder()
            .uri("/api/v1/persistentvolumes").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["kind"], "PersistentVolumeList");
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    pub async fn create_and_list_pvc() {
        let app = make_app().await;
        let pvc_json = r#"{"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":"pvc1","namespace":"default"},"spec":{"accessModes":["ReadWriteOnce"],"resources":{"requests":{"storage":"1Gi"}}}}"#;
        let resp = app.clone().oneshot(Request::builder()
            .method("POST").uri("/api/v1/namespaces/default/persistentvolumeclaims")
            .header("content-type", "application/json").body(json_body(pvc_json)).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let resp = app.oneshot(Request::builder()
            .uri("/api/v1/namespaces/default/persistentvolumeclaims").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["kind"], "PersistentVolumeClaimList");
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    pub async fn list_pods_scoped_to_namespace() {
        let (app, store) = make_app_with_store().await;
        let pod_default: crate::types::Pod = serde_json::from_value(serde_json::json!({
            "apiVersion":"v1","kind":"Pod","metadata":{"name":"p1","namespace":"default"},"spec":{"containers":[{"name":"c","image":"alpine"}]}
        })).unwrap();
        let pod_other: crate::types::Pod = serde_json::from_value(serde_json::json!({
            "apiVersion":"v1","kind":"Pod","metadata":{"name":"p2","namespace":"other"},"spec":{"containers":[{"name":"c","image":"alpine"}]}
        })).unwrap();
        store.apply(AnyResource::Pod(pod_default)).await.unwrap();
        store.apply(AnyResource::Pod(pod_other)).await.unwrap();
        let resp = app.oneshot(Request::builder()
            .uri("/api/v1/namespaces/default/pods").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["metadata"]["name"], "p1");
    }

    #[tokio::test]
    pub async fn unknown_route_returns_404() {
        let app = make_app().await;
        let resp = app.oneshot(Request::builder()
            .uri("/not/a/real/path").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
