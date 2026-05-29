use crate::types::{AnyResource, ResourceStore};
use crate::scheduler::process::ProcessTracker;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;


struct RunningProxy {
    handle: JoinHandle<()>,
}

pub struct NetworkManager {
    pub store: Arc<ResourceStore>,
    pub process_tracker: Arc<ProcessTracker>,
    proxies: Mutex<HashMap<String, RunningProxy>>,
    counter: Arc<AtomicUsize>,
}

impl NetworkManager {
    pub fn new(store: Arc<ResourceStore>, process_tracker: Arc<ProcessTracker>) -> Self {
        Self {
            store,
            process_tracker,
            proxies: Mutex::new(HashMap::new()),
            counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Start or restart ClusterIP/NodePort proxies and reconcile pod port publish for backends.
    pub async fn sync_service(&self, svc: &Service) {
        self.sync_service_proxies(svc).await;
        
    }

    /// Bind/rebind proxies only (safe to call from `start_pod` without async recursion).
    pub async fn sync_service_proxies(&self, svc: &Service) {
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
                let resolver: Arc<dyn crate::netmux::network::PodResolver> = self.process_tracker.clone();
                let counter = self.counter.clone();
                let selector_c = selector.clone();
                let svc_name_c = svc_name.clone();
                let svc_ns_c = svc_ns.clone();
                let target_port_c = target_port.clone();
                let listen_addr_log = listen_addr.clone();
                // Proxy retired in favor of nftables DNAT (NetMux Phase 2)
                // Old: crate::net::service_proxy::run_proxy_addr_when_ready(...)
                info!("Service {} → ClusterIP {} — DNAT via nftables", key, listen_addr_log);
                proxies.insert(port_key, RunningProxy { handle: tokio::spawn(async { /* retired */ }) });
            }

            // Also bind NodePort for external access
            if svc_type == "NodePort" || svc_type == "LoadBalancer"  {
                if let Some(node_port) = svc_port.node_port.map(|p| p as u16) {
                    let listen_addr = format!("0.0.0.0:{}", node_port);
                    let port_key = format!("{}:nodeport:{}", key, node_port);
                    if let Some(old) = proxies.remove(&port_key) {
                        old.handle.abort();
                    }
                    let store = self.store.clone();
                    let resolver: Arc<dyn crate::netmux::network::PodResolver> = self.process_tracker.clone();
                    let counter = self.counter.clone();
                    let selector_c = selector.clone();
                    let svc_name_c = svc_name.clone();
                    let svc_ns_c = svc_ns.clone();
                    let target_port_c = target_port.clone();
                    let listen_addr_log = listen_addr.clone();
                    // Proxy retired in favor of nftables DNAT (NetMux Phase 2)
                    // Old: crate::net::service_proxy::run_proxy_addr(...)
                    info!("Service {} → NodePort {} — DNAT via nftables", key, listen_addr_log);
                    proxies.insert(port_key, RunningProxy { handle: tokio::spawn(async { /* retired */ }) });
                }
            }
        }
    }

    /// Re-bind service proxies when a pod becomes ready (pods often start after their Service).
    pub async fn sync_services_for_labels(&self, namespace: &str, pod_labels: &BTreeMap<String, String>) {
        let trackers = self.store.get_by_kind("Service").await;
        for t in &trackers {
            if let AnyResource::Service(svc) = &t.resource {
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
                    self.sync_service_proxies(svc).await;
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
            if let AnyResource::Pod(pod) = &t.resource {
                if pod.metadata.namespace.as_deref().unwrap_or("default") != svc_ns {
                    continue;
                }
                let pod_labels: BTreeMap<String, String> = pod.metadata.labels.clone().unwrap_or_default();
                if !selector.iter().all(|(k, v)| pod_labels.get(k) == Some(v)) {
                    continue;
                }
                // Check if pod is running
                let pod_name = pod.metadata.name.as_deref().unwrap_or_default();
                let is_running = self.process_tracker.is_ready(pod_name).await;
                if !is_running {
                    continue;
                }
                let pod_ip = pod.status.as_ref()
                    .and_then(|s| s.pod_ip.as_deref())
                    .unwrap_or("127.0.0.1");
                addresses.push(EndpointAddress {
                    ip: pod_ip.to_string(),
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

    /// Generate EndpointSlice objects for a service — one per pod with its published host port.
    pub async fn compute_endpointslices(
        &self,
        svc: &Service,
    ) -> Vec<k8s_openapi::api::discovery::v1::EndpointSlice> {
        use k8s_openapi::api::discovery::v1::{Endpoint as DiscoveryEndpoint, EndpointConditions, EndpointPort as DiscoveryEndpointPort, EndpointSlice};
        use k8s_openapi::api::core::v1::ObjectReference;

        let svc_name = svc.metadata.name.as_deref().unwrap_or_default().to_string();
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let selector = svc.spec.as_ref()
            .and_then(|s| s.selector.as_ref())
            .cloned()
            .unwrap_or_default();

        let mut slices = Vec::new();

        let pod_trackers = self.store.get_by_kind("Pod").await;
        for t in &pod_trackers {
            if let AnyResource::Pod(pod) = &t.resource {
                if pod.metadata.namespace.as_deref().unwrap_or("default") != svc_ns {
                    continue;
                }
                let pod_labels: BTreeMap<String, String> = pod.metadata.labels.clone().unwrap_or_default();
                if !selector.iter().all(|(k, v)| pod_labels.get(k) == Some(v)) {
                    continue;
                }
                let pod_name = pod.metadata.name.as_deref().unwrap_or_default();
                let is_ready = self.process_tracker.is_ready(pod_name).await;
                if !is_ready {
                    continue;
                }
                let pod_ip = pod.status.as_ref()
                    .and_then(|s| s.pod_ip.as_deref())
                    .unwrap_or("127.0.0.1");

                // Use the actual host-side connect port per pod
                let host_port = match svc.spec.as_ref().and_then(|s| s.ports.as_ref()).and_then(|ps| ps.first()) {
                    Some(p) => {
                        let cp = match p.target_port.as_ref().cloned().unwrap_or_else(|| IntOrString::Int(p.port)) {
                            IntOrString::Int(i) => i as u16,
                            IntOrString::String(_) => p.port as u16,
                        };
                        self.process_tracker.backend_connect_port(pod_name, cp).await
                    }
                    None => 80,
                };

                let slice_name = format!("{}-{}-z8s", svc_name, pod_name);
                slices.push(EndpointSlice {
                    address_type: "IPv4".to_string(),
                    endpoints: vec![DiscoveryEndpoint {
                        addresses: vec![pod_ip.to_string()],
                        conditions: Some(EndpointConditions {
                            ready: Some(true),
                            serving: Some(true),
                            terminating: Some(false),
                        }),
                        hostname: None,
                        node_name: Some("z8s-node".to_string()),
                        target_ref: Some(ObjectReference {
                            kind: Some("Pod".to_string()),
                            name: Some(pod_name.to_string()),
                            namespace: Some(svc_ns.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        name: Some(slice_name),
                        namespace: Some(svc_ns.clone()),
                        labels: Some(BTreeMap::from([
                            ("kubernetes.io/service-name".to_string(), svc_name.clone()),
                        ])),
                        ..Default::default()
                    },
                    ports: svc.spec.as_ref()
                        .and_then(|s| s.ports.as_ref())
                        .map(|ps| {
                            ps.iter().map(|p| DiscoveryEndpointPort {
                                name: Some(p.name.clone().unwrap_or_default()),
                                port: Some(host_port as i32),
                                protocol: Some(p.protocol.clone().unwrap_or_else(|| "TCP".to_string())),
                                ..Default::default()
                            }).collect()
                        }),
                });
            }
        }

        slices
    }
}

#[async_trait]
impl crate::netmux::network::NetworkEngine for NetworkManager {
    fn dns_port(&self) -> Option<u16> {
        crate::config::dns_port()
    }

    async fn sync_service(&self, svc: &Service) -> anyhow::Result<()> {
        NetworkManager::sync_service(self, svc).await;
        Ok(())
    }

    async fn remove_service(&self, ns: &str, name: &str) -> anyhow::Result<()> {
        NetworkManager::remove_service(self, ns, name).await;
        Ok(())
    }

    async fn sync_services_for_labels(&self, ns: &str, labels: &BTreeMap<String, String>) -> anyhow::Result<()> {
        NetworkManager::sync_services_for_labels(self, ns, labels).await;
        Ok(())
    }

    async fn compute_endpoints(&self, svc: &Service) -> k8s_openapi::api::core::v1::Endpoints {
        NetworkManager::compute_endpoints(self, svc).await
    }

    async fn compute_endpointslices(&self, svc: &Service) -> Vec<k8s_openapi::api::discovery::v1::EndpointSlice> {
        NetworkManager::compute_endpointslices(self, svc).await
    }
}

use async_trait::async_trait;
use anyhow::Result;
use crate::types::ResourceTracker;
use crate::components::{Component, ReconcileContext, ResourceCategory};

pub struct ServiceResource {
    pub store: Arc<ResourceStore>,
    pub network: Arc<NetworkManager>,
}

impl ServiceResource {
    pub fn new(store: Arc<ResourceStore>, network: Arc<NetworkManager>) -> Self {
        Self { store, network }
    }
}

#[async_trait]
impl Component for ServiceResource {
    fn kind(&self) -> &'static str {
        "Service"
    }

    fn category(&self) -> ResourceCategory {
        ResourceCategory::Network
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Service(svc) = resource {
            self.network.sync_service(svc).await;
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        let ns = resource.namespace();
        let name = resource.name();
        self.network.remove_service(ns, name).await;
        Ok(())
    }
}
