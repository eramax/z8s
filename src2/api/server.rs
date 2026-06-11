pub use crate::api::AnyResource;
pub use crate::api::types::{MemoryBackend, ResourceState, StoreBackend};
pub use crate::types::{
    APIGroup, APIGroupList, APIResource, APIResourceList, APIVersions, ConfigMap, ContainerState,
    ContainerStateRunning, ContainerStateTerminated, ContainerStateWaiting, ContainerStatus,
    DaemonEndpoint, DeploymentCondition, DeploymentStatus, EndpointSlice, Endpoints, Event,
    EventSource, GroupVersionForDiscovery, HostIP, Ingress, List, ListMeta, Namespace,
    NamespaceStatus, NetworkPolicy, Node, NodeAddress, NodeCondition, NodeDaemonEndpoints,
    NodeSpec, NodeStatus, NodeSystemInfo, ObjectMeta, ObjectReference, PodCondition, PodIP,
    PodStatus, Quantity, Scale, ScaleSpec, ScaleStatus, Secret, SelfSubjectAccessReview,
    SelfSubjectAccessReviewSpec, Service, ServiceStatus, Status, SubjectAccessReviewStatus,
};
pub use axum::Router;
pub use axum::extract::{Path, State};
pub use axum::http::{Method, StatusCode, Uri};
pub use axum::response::{IntoResponse, Json};
pub use std::collections::{BTreeMap, HashMap};
pub use std::sync::Arc;
pub use tokio::sync::{Mutex, RwLock};
pub use tracing::info;

use crate::components::{ComponentRegistry, ReconcileContext};
use crate::scheduler::process::ProcessTracker;

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
    pub store_events: crate::store::StoreEventHub,
    /// Wakes the orchestrator immediately on store writes and gossip assignments.
    pub reconciler_notify: Arc<tokio::sync::Notify>,
    /// Cluster redb for join-token auth on main (gossip inbound).
    pub join_db: Option<Arc<crate::store::RedbBackend>>,
    /// Main node: require Bearer join token on `/ws/gossip`.
    pub require_join_auth: bool,
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

    /// Delete a resource, emit a store event, wake the orchestrator, and gossip delete to peers.
    pub async fn delete_and_notify(&self, resource: &AnyResource) -> anyhow::Result<()> {
        self.store.delete(resource).await?;
        self.store_events.emit_deleted(resource.clone());
        self.reconciler_notify.notify_one();
        Ok(())
    }

    /// Apply a resource, emit a store event, wake the orchestrator, and gossip to peers.
    pub async fn apply_and_broadcast(&self, resource: AnyResource) -> anyhow::Result<()> {
        let change = if self.store.get(&resource.uid()).await.is_some() {
            crate::store::StoreChange::Updated
        } else {
            crate::store::StoreChange::Created
        };
        self.store.apply(resource.clone()).await?;
        self.store_events
            .emit_applied(resource.clone(), change);
        self.reconciler_notify.notify_one();
        if let Some(ref gs) = self.gossip_state {
            let mut g = gs.lock().await;
            if g.queue_write(&resource) {
                g.flush_batch().await;
            }
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
    store_events: crate::store::StoreEventHub,
    reconciler_notify: Arc<tokio::sync::Notify>,
    join_db: Option<Arc<crate::store::RedbBackend>>,
    require_join_auth: bool,
) -> AppState {
    // Log store state on startup
    let pods = store.get_by_kind("Pod").await.len();
    let svcs = store.get_by_kind("Service").await.len();
    let deploys = store.get_by_kind("Deployment").await.len();
    let ns_count = store.get_by_kind("Namespace").await.len();
    info!(
        "Store state: {} namespaces, {} pods, {} services, {} deployments",
        ns_count, pods, svcs, deploys
    );

    // Ensure default namespace exists in the store
    let existing = store.get_by_kind("Namespace").await;
    if !existing.iter().any(|t| t.resource.name() == "default") {
        let ns = make_namespace("default", "ns-default");
        info!("Creating default namespace");
        store.apply(AnyResource::Namespace(ns)).await.ok();
    }

    // Ensure default VNet exists — all pods without z8s.io/vnet annotation use it
    let vnets = store.get_by_kind("VNet").await;
    if !vnets.iter().any(|t| t.resource.name() == "default") {
        let vnet = crate::types::VNet {
            api_version: "z8s.io/v1".into(),
            kind: "VNet".into(),
            metadata: crate::types::ObjectMeta {
                name: Some("default".into()),
                annotations: Some({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("z8s.io/default".into(), "true".into());
                    m
                }),
                ..Default::default()
            },
            spec: crate::types::VNetSpec {
                cidr: Some(crate::config::get().pod_cidr.clone()),
                internet_access: true,
                role: "hub".into(),
            },
            status: None,
        };
        info!("Creating default VNet with CIDR {}", crate::config::get().pod_cidr);
        store.apply(AnyResource::VNet(vnet)).await.ok();
    }

    // Ensure default Subnet exists within the default VNet
    let subnets = store.get_by_kind("Subnet").await;
    if !subnets.iter().any(|t| t.resource.name() == "default") {
        let subnet = crate::types::Subnet {
            api_version: "z8s.io/v1".into(),
            kind: "Subnet".into(),
            metadata: crate::types::ObjectMeta {
                name: Some("default".into()),
                ..Default::default()
            },
            spec: crate::types::SubnetSpec {
                vnet: "default".into(),
                cidr: crate::config::get().pod_cidr.clone(),
            },
        };
        info!("Creating default Subnet with CIDR {}", crate::config::get().pod_cidr);
        store.apply(AnyResource::Subnet(subnet)).await.ok();
    }
    AppState {
        store,
        process_tracker,
        registry,
        ctx,
        gossip_state,
        store_events,
        reconciler_notify,
        join_db,
        require_join_auth,
    }
}

pub fn build_router(state: AppState) -> Router {
    let store_for_mw = state.store.clone();
    let tokens_for_mw = state.process_tracker.tokens.clone();
    Router::new()
        .merge(crate::api::handlers::system::routes())
        .merge(crate::api::handlers::metrics::routes())
        .merge(crate::api::catalog_routes::from_catalog())
        .merge(crate::api::subresource::routes())
        .merge(crate::api::handlers::apply::routes())
        .route("/ws/gossip", axum::routing::any(gossip_ws_handler))
        .fallback(fallback_handler)
        .layer(axum::middleware::from_fn(move |headers, req, next| {
            let store = store_for_mw.clone();
            let tokens = tokens_for_mw.clone();
            async move {
                crate::api::handlers::rbac::authorize_middleware_with_store(
                    headers, req, next, store, tokens,
                )
                .await
            }
        }))
        .with_state(state)
}

#[derive(serde::Deserialize, Default)]
pub struct GossipWsQuery {
    pub node_name: Option<String>,
}

pub async fn gossip_ws_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    headers: axum::http::HeaderMap,
    axum::extract::Query(query): axum::extract::Query<GossipWsQuery>,
    State(state): State<AppState>,
) -> impl axum::response::IntoResponse {
    // When join auth is required, validate the Bearer token.
    // Skip auth if no join_db is available (e.g. in-memory mode).
    if state.require_join_auth {
        if let Some(ref db) = state.join_db {
            match db
                .authenticate_join(&headers, query.node_name.as_deref())
                .await
            {
                Ok(id) => {
                    tracing::info!(
                        "Gossip join authenticated for node '{}'",
                        id.node_name
                    );
                }
                Err(e) => {
                    tracing::warn!("Gossip join auth failed: {}", e);
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        format!("join authentication failed: {}", e),
                    )
                        .into_response();
                }
            }
        }
    }
    match state.gossip_state {
        Some(ref gs) => {
            let gs = gs.clone();
            let notify = state.reconciler_notify.clone();
            let events = state.store_events.clone();
            ws.on_upgrade(move |socket| crate::store::ws::handle_gossip_ws(socket, gs, events, notify))
                .into_response()
        }
        None => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "gossip not configured",
        )
            .into_response(),
    }
}

pub async fn run_server(
    store: Arc<dyn StoreBackend>,
    process_tracker: Arc<ProcessTracker>,
    registry: Arc<ComponentRegistry>,
    ctx: Arc<ReconcileContext>,
    gossip_state: Option<Arc<tokio::sync::Mutex<crate::store::gossip::GossipState>>>,
    store_events: crate::store::StoreEventHub,
    reconciler_notify: Arc<tokio::sync::Notify>,
    join_db: Option<Arc<crate::store::RedbBackend>>,
    require_join_auth: bool,
) {
    let state = build_app_state(
        store,
        process_tracker,
        registry,
        ctx,
        gossip_state,
        store_events,
        reconciler_notify,
        join_db,
        require_join_auth,
    )
    .await;
    let app = build_router(state);

    let addr: std::net::SocketAddr = format!("0.0.0.0:{}", z8s_port())
        .parse()
        .expect("Invalid listen address");
    let socket = tokio::net::TcpSocket::new_v4().expect("Failed to create TCP socket");
    socket
        .set_reuseaddr(true)
        .expect("Failed to set SO_REUSEADDR");
    socket
        .bind(addr)
        .unwrap_or_else(|e| panic!("Failed to bind to {} — port in use? ({})", addr, e));
    let listener = socket.listen(1024).expect("Failed to listen");

    let cfg = crate::config::get();
    let tls_cert = cfg.tls_cert.as_deref()
        .or_else(|| crate::config::tls_cert_path());
    let tls_key = cfg.tls_key.as_deref()
        .or_else(|| crate::config::tls_key_path());
    if let (Some(cert_path), Some(key_path)) = (tls_cert, tls_key) {
        use tokio_rustls::rustls;
        info!("Starting TLS API server on {}", addr);
        let cert_file = std::fs::File::open(cert_path).expect("Cannot open TLS cert");
        let key_file = std::fs::File::open(key_path).expect("Cannot open TLS key");
        let mut cert_reader = std::io::BufReader::new(cert_file);
        let mut key_reader = std::io::BufReader::new(key_file);
        let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .expect("Invalid TLS cert PEM");
        let key = rustls_pemfile::private_key(&mut key_reader)
            .expect("Invalid TLS key PEM")
            .expect("No private key found in key file");
        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("Invalid TLS cert/key pair");
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let tls_acceptor =
            tokio_rustls::TlsAcceptor::from(Arc::new(config));

        let mut tls_listener = TlsListener {
            tcp: listener,
            tls_acceptor,
        };
        axum::serve(tls_listener, app).await.unwrap();
    } else {
        info!("Starting API server on {}", addr);
        axum::serve(listener, app).await.unwrap();
    }
}

// ── TLS Listener adapter for axum::serve ─────────────────────────────────────

struct TlsListener {
    tcp: tokio::net::TcpListener,
    tls_acceptor: tokio_rustls::TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;

    fn accept(&mut self) -> impl std::future::Future<Output = (Self::Io, Self::Addr)> + Send {
        async {
            loop {
                let (tcp_stream, peer_addr) = self.tcp.accept().await.expect("TCP accept failed");
                match self.tls_acceptor.accept(tcp_stream).await {
                    Ok(tls_stream) => return (tls_stream, peer_addr),
                    Err(e) => {
                        tracing::error!("TLS handshake error: {}", e);
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

// ── Shared helpers ───────────────────────────────────────────────────────────

pub fn now_time() -> crate::types::Time {
    crate::types::Time(crate::config::now_rfc3339())
}

pub fn now_rfc3339() -> String {
    crate::config::now_rfc3339()
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
        status: Some(NamespaceStatus {
            phase: Some("Active".into()),
            ..Default::default()
        }),
    }
}

pub fn accepts_table(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("as=Table"))
        .unwrap_or(false)
}

pub fn age_from_timestamp(ts: &str) -> String {
    if let Some(secs) = crate::config::parse_rfc3339_secs(ts) {
        crate::config::age_from_epoch_secs(secs)
    } else {
        "<unknown>".into()
    }
}

pub use crate::api::table::{build_table, format_ts_relative, make_table};

pub fn ok_status() -> Status {
    Status {
        status: Some("Success".into()),
        code: Some(200),
        ..Default::default()
    }
}

pub fn make_list_meta() -> ListMeta {
    ListMeta {
        resource_version: Some("1".into()),
        ..Default::default()
    }
}

pub fn json_merge_patch(base: &mut serde_json::Value, patch: &serde_json::Value) {
    if let (serde_json::Value::Object(b), serde_json::Value::Object(p)) = (base, patch) {
        for (k, v) in p {
            if v.is_null() {
                b.remove(k);
            } else if v.is_object() {
                let entry = b
                    .entry(k.clone())
                    .or_insert(serde_json::Value::Object(Default::default()));
                json_merge_patch(entry, v);
            } else {
                b.insert(k.clone(), v.clone());
            }
        }
    }
}

pub fn parse_body(bytes: &axum::body::Bytes) -> Result<serde_json::Value, ApiError> {
    if let Some(v) = crate::api::proto::try_proto_to_json(bytes) {
        return Ok(v);
    }
    if let Ok(v) = serde_json::from_slice(bytes) {
        return Ok(v);
    }
    serde_yaml::from_slice::<serde_yaml::Value>(bytes)
        .ok()
        .and_then(|y| serde_json::to_value(y).ok())
        .ok_or_else(|| ApiError::bad_request("invalid body: not valid JSON or YAML".to_string()))
}

pub fn detect_arch() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        return "amd64".into();
    }
    #[cfg(target_arch = "aarch64")]
    {
        return "arm64".into();
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        std::env::consts::ARCH.into()
    }
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

pub fn gvd(group_version: &str, version: &str) -> GroupVersionForDiscovery {
    GroupVersionForDiscovery {
        group_version: group_version.into(),
        version: version.into(),
    }
}

// ── Error type ───────────────────────────────────────────────────────────────

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn not_found(msg: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg,
        }
    }
    pub fn bad_request(msg: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg,
        }
    }
    pub fn method_not_allowed(msg: String) -> Self {
        Self {
            status: StatusCode::METHOD_NOT_ALLOWED,
            message: msg,
        }
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

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
