pub mod dns;
pub mod port_publish;
pub mod service_proxy;

/// The port the z8s DNS server is listening on (53 or 5353). Set once at startup.
static DNS_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

pub fn set_dns_port(port: u16) {
    DNS_PORT.set(port).ok();
}

pub fn dns_port() -> Option<u16> {
    DNS_PORT.get().copied()
}

use crate::api::types::ResourceStore;
use crate::supervisor::process::ProcessSupervisor;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;

#[derive(Debug, Clone)]
pub struct ServiceEndpoint {
    pub host: String,
    pub port: u16,
}

struct RunningProxy {
    handle: JoinHandle<()>,
}

pub struct NetworkManager {
    pub store: Arc<ResourceStore>,
    pub supervisor: Arc<ProcessSupervisor>,
    proxies: Mutex<HashMap<String, RunningProxy>>,
    counter: Arc<AtomicUsize>,
}

impl NetworkManager {
    pub fn new(store: Arc<ResourceStore>, supervisor: Arc<ProcessSupervisor>) -> Self {
        Self {
            store,
            supervisor,
            proxies: Mutex::new(HashMap::new()),
            counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Start or restart the proxy for a service.
    pub async fn sync_service(&self, svc: &Service) {
        let svc_name = svc.metadata.name.as_deref().unwrap_or_default().to_string();
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let key = format!("{}/{}", svc_ns, svc_name);

        let spec = match &svc.spec {
            Some(s) => s,
            None => return,
        };

        let selector: BTreeMap<String, String> = spec.selector.clone().unwrap_or_default();
        if selector.is_empty() {
            return; // headless or external-name services — no proxy
        }

        let svc_type = spec.type_.as_deref().unwrap_or("ClusterIP");
        let cluster_ip = spec.cluster_ip.as_deref().unwrap_or("").to_string();
        let ports = spec.ports.as_deref().unwrap_or(&[]);

        let mut proxies = self.proxies.lock().await;

        for svc_port in ports {
            let target_port = svc_port.target_port.clone()
                .unwrap_or_else(|| IntOrString::Int(svc_port.port));

            // Bind on ClusterIP:servicePort so pods can reach the service directly
            if !cluster_ip.is_empty() && cluster_ip != "None" {
                let listen_addr = format!("{}:{}", cluster_ip, svc_port.port);
                let port_key = format!("{}:clusterip:{}", key, svc_port.port);
                if let Some(old) = proxies.remove(&port_key) {
                    old.handle.abort();
                }
                let store = self.store.clone();
                let supervisor = self.supervisor.clone();
                let counter = self.counter.clone();
                let selector_c = selector.clone();
                let svc_name_c = svc_name.clone();
                let svc_ns_c = svc_ns.clone();
                let target_port_c = target_port.clone();
                let listen_addr_log = listen_addr.clone();
                let handle = tokio::spawn(async move {
                    service_proxy::run_proxy_addr_when_ready(
                        &listen_addr,
                        selector_c,
                        target_port_c,
                        store,
                        supervisor,
                        counter,
                        &svc_name_c,
                        &svc_ns_c,
                    )
                    .await;
                });
                info!("Service proxy {} → ClusterIP {} (deferred until endpoints ready)", key, listen_addr_log);
                proxies.insert(port_key, RunningProxy { handle });
            }

            // Also bind NodePort for external access
            if (svc_type == "NodePort" || svc_type == "LoadBalancer") {
                if let Some(node_port) = svc_port.node_port.map(|p| p as u16) {
                    let listen_addr = format!("0.0.0.0:{}", node_port);
                    let port_key = format!("{}:nodeport:{}", key, node_port);
                    if let Some(old) = proxies.remove(&port_key) {
                        old.handle.abort();
                    }
                    let store = self.store.clone();
                    let supervisor = self.supervisor.clone();
                    let counter = self.counter.clone();
                    let selector_c = selector.clone();
                    let svc_name_c = svc_name.clone();
                    let svc_ns_c = svc_ns.clone();
                    let target_port_c = target_port.clone();
                    let listen_addr_log = listen_addr.clone();
                    let handle = tokio::spawn(async move {
                        service_proxy::run_proxy_addr(
                            &listen_addr,
                            selector_c,
                            target_port_c,
                            store,
                            supervisor,
                            counter,
                            &svc_name_c,
                            &svc_ns_c,
                        )
                        .await;
                    });
                    info!("Service proxy {} → NodePort {}", key, listen_addr_log);
                    proxies.insert(port_key, RunningProxy { handle });
                }
            }
        }
    }

    /// Re-bind service proxies when a pod becomes ready (pods often start after their Service).
    pub async fn sync_services_for_labels(&self, namespace: &str, pod_labels: &BTreeMap<String, String>) {
        let trackers = self.store.get_by_kind("Service").await;
        for t in &trackers {
            if let crate::api::AnyResource::Service(svc) = &t.resource {
                if svc.metadata.namespace.as_deref().unwrap_or("default") != namespace {
                    continue;
                }
                let selector = svc
                    .spec
                    .as_ref()
                    .and_then(|s| s.selector.as_ref())
                    .cloned()
                    .unwrap_or_default();
                if selector.is_empty() {
                    continue;
                }
                if selector.iter().all(|(k, v)| pod_labels.get(k) == Some(v)) {
                    self.sync_service(svc).await;
                }
            }
        }
    }

    pub async fn remove_service(&self, ns: &str, name: &str) {
        let prefix = format!("{}/{}", ns, name);
        let mut proxies = self.proxies.lock().await;
        let keys: Vec<_> = proxies.keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(p) = proxies.remove(&key) {
                p.handle.abort();
                info!("Stopped service proxy for {}", key);
            }
        }
    }

    /// Generate an Endpoints object for a service.
    pub async fn compute_endpoints(
        &self,
        svc: &Service,
    ) -> k8s_openapi::api::core::v1::Endpoints {
        use k8s_openapi::api::core::v1::{EndpointAddress, EndpointPort, EndpointSubset, Endpoints, ObjectReference};

        let svc_name = svc.metadata.name.as_deref().unwrap_or_default().to_string();
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let selector = svc.spec.as_ref()
            .and_then(|s| s.selector.as_ref())
            .cloned()
            .unwrap_or_default();

        let mut addresses = Vec::new();

        // Find pods matching the selector
        let pod_trackers = self.store.get_by_kind("Pod").await;
        for t in &pod_trackers {
            if let crate::api::AnyResource::Pod(pod) = &t.resource {
                if pod.metadata.namespace.as_deref().unwrap_or("default") != svc_ns {
                    continue;
                }
                let pod_labels: BTreeMap<String, String> = pod.metadata.labels.clone().unwrap_or_default();
                if !selector.iter().all(|(k, v)| pod_labels.get(k) == Some(v)) {
                    continue;
                }
                // Check if pod is running
                let pod_name = pod.metadata.name.as_deref().unwrap_or_default();
                let is_running = self.supervisor.is_pod_ready(pod_name).await;
                if !is_running {
                    continue;
                }
                addresses.push(EndpointAddress {
                    ip: "127.0.0.1".to_string(),
                    hostname: None,
                    node_name: Some("z8s-node".to_string()),
                    target_ref: Some(ObjectReference {
                        kind: Some("Pod".to_string()),
                        name: Some(pod_name.to_string()),
                        namespace: Some(svc_ns.clone()),
                        ..Default::default()
                    }),
                });
            }
        }

        let ports: Vec<EndpointPort> = svc.spec.as_ref()
            .and_then(|s| s.ports.as_ref())
            .unwrap_or(&vec![])
            .iter()
            .map(|p| EndpointPort {
                name: p.name.clone(),
                port: p.target_port.as_ref()
                    .and_then(|tp| match tp {
                        IntOrString::Int(i) => Some(*i),
                        IntOrString::String(_) => None,
                    })
                    .unwrap_or(p.port),
                protocol: p.protocol.clone(),
                ..Default::default()
            })
            .collect();

        let subsets = if addresses.is_empty() {
            vec![]
        } else {
            vec![EndpointSubset {
                addresses: Some(addresses),
                not_ready_addresses: None,
                ports: if ports.is_empty() { None } else { Some(ports) },
            }]
        };

        Endpoints {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(svc_name),
                namespace: Some(svc_ns),
                ..Default::default()
            },
            subsets: Some(subsets),
        }
    }
}
