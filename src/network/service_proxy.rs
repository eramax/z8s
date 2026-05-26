use crate::api::types::ResourceStore;
use crate::supervisor::process::ProcessSupervisor;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

pub async fn run_proxy_addr(
    listen_addr: &str,
    selector: BTreeMap<String, String>,
    target_port: IntOrString,
    store: Arc<ResourceStore>,
    supervisor: Arc<ProcessSupervisor>,
    counter: Arc<AtomicUsize>,
    svc_name: &str,
    svc_ns: &str,
) {
    let listener = match TcpListener::bind(listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            warn!("Service proxy {}/{}: failed to bind {}: {}", svc_ns, svc_name, listen_addr, e);
            return;
        }
    };
    info!("Service proxy {}/{} listening on {}", svc_ns, svc_name, listen_addr);

    loop {
        match listener.accept().await {
            Ok((client, _peer)) => {
                let store = store.clone();
                let supervisor = supervisor.clone();
                let counter = counter.clone();
                let selector = selector.clone();
                let target_port = target_port.clone();
                let svc_ns = svc_ns.to_string();
                let svc_name = svc_name.to_string();
                tokio::spawn(async move {
                    handle_connection(client, selector, target_port, store, supervisor, counter, &svc_ns, &svc_name).await;
                });
            }
            Err(e) => {
                warn!("Service proxy accept error: {}", e);
                break;
            }
        }
    }
}

pub async fn run_proxy(
    listen_port: u16,
    selector: BTreeMap<String, String>,
    target_port: IntOrString,
    store: Arc<ResourceStore>,
    supervisor: Arc<ProcessSupervisor>,
    counter: Arc<AtomicUsize>,
    svc_name: &str,
    svc_ns: &str,
) {
    let addr = format!("0.0.0.0:{}", listen_port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            warn!("Service proxy {}/{}: failed to bind {}: {}", svc_ns, svc_name, addr, e);
            return;
        }
    };
    info!("Service proxy {}/{} listening on {}", svc_ns, svc_name, addr);

    loop {
        match listener.accept().await {
            Ok((client, peer)) => {
                info!("Service proxy {}/{}: new connection from {}", svc_ns, svc_name, peer);
                let store = store.clone();
                let supervisor = supervisor.clone();
                let counter = counter.clone();
                let selector = selector.clone();
                let target_port = target_port.clone();
                let svc_ns = svc_ns.to_string();
                let svc_name = svc_name.to_string();
                tokio::spawn(async move {
                    handle_connection(client, selector, target_port, store, supervisor, counter, &svc_ns, &svc_name).await;
                });
            }
            Err(e) => {
                warn!("Service proxy accept error: {}", e);
                break;
            }
        }
    }
}

async fn handle_connection(
    mut client: TcpStream,
    selector: BTreeMap<String, String>,
    target_port: IntOrString,
    store: Arc<ResourceStore>,
    supervisor: Arc<ProcessSupervisor>,
    counter: Arc<AtomicUsize>,
    svc_ns: &str,
    svc_name: &str,
) {
    let endpoints = find_endpoints(&selector, &target_port, &store, &supervisor, svc_ns).await;
    if endpoints.is_empty() {
        warn!("Service {}/{}: no endpoints available", svc_ns, svc_name);
        return;
    }

    let idx = counter.fetch_add(1, Ordering::Relaxed) % endpoints.len();
    let endpoint = &endpoints[idx];

    let mut backend = match TcpStream::connect(format!("{}:{}", endpoint.host, endpoint.port)).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Service {}/{}: failed to connect to {}:{}: {}", svc_ns, svc_name, endpoint.host, endpoint.port, e);
            return;
        }
    };

    info!("Service {}/{}: proxying to {}:{}", svc_ns, svc_name, endpoint.host, endpoint.port);
    tokio::io::copy_bidirectional(&mut client, &mut backend).await.ok();
}

async fn find_endpoints(
    selector: &BTreeMap<String, String>,
    target_port: &IntOrString,
    store: &ResourceStore,
    supervisor: &ProcessSupervisor,
    svc_ns: &str,
) -> Vec<super::ServiceEndpoint> {
    let pod_trackers = store.get_by_kind("Pod").await;
    let mut endpoints = Vec::new();

    for t in &pod_trackers {
        if let crate::api::AnyResource::Pod(pod) = &t.resource {
            if pod.metadata.namespace.as_deref().unwrap_or("default") != svc_ns {
                continue;
            }

            let pod_labels: BTreeMap<String, String> = pod.metadata.labels.clone().unwrap_or_default();
            if !selector.iter().all(|(k, v)| pod_labels.get(k) == Some(v)) {
                continue;
            }

            let pod_name = pod.metadata.name.as_deref().unwrap_or_default();
            if !supervisor.is_pod_ready(pod_name).await {
                continue;
            }

            // Resolve the target port
            let port = resolve_container_port(pod, target_port);
            if let Some(port) = port {
                endpoints.push(super::ServiceEndpoint {
                    host: "127.0.0.1".to_string(),
                    port,
                });
            }
        }
    }

    endpoints
}

fn resolve_container_port(
    pod: &k8s_openapi::api::core::v1::Pod,
    target_port: &IntOrString,
) -> Option<u16> {
    let spec = pod.spec.as_ref()?;
    match target_port {
        IntOrString::Int(p) => Some(*p as u16),
        IntOrString::String(name) => {
            for container in &spec.containers {
                if let Some(ports) = &container.ports {
                    for cp in ports {
                        if cp.name.as_deref() == Some(name.as_str()) {
                            return Some(cp.container_port as u16);
                        }
                    }
                }
            }
            None
        }
    }
}
