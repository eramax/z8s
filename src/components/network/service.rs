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

use crate::netmux::NetMux;


struct RunningProxy {
    handle: JoinHandle<()>,
}

pub struct NetworkManager {
    pub store: Arc<ResourceStore>,
    pub process_tracker: Arc<ProcessTracker>,
    proxies: Mutex<HashMap<String, RunningProxy>>,
    counter: Arc<AtomicUsize>,
    netmux: Arc<NetMux>,
}

impl NetworkManager {
    pub fn new(store: Arc<ResourceStore>, process_tracker: Arc<ProcessTracker>, netmux: Arc<NetMux>) -> Self {
        Self {
            store,
            process_tracker,
            proxies: Mutex::new(HashMap::new()),
            counter: Arc::new(AtomicUsize::new(0)),
            netmux,
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
            tracing::info!("sync_service_proxies {}: empty selector, skipping", key);
            return; // headless or external-name services — no proxy
        }
        tracing::info!("sync_service_proxies {}: selector={:?}", key, selector);

        let svc_type = spec.type_.as_deref().unwrap_or("ClusterIP");
        let cluster_ip = spec.cluster_ip.as_deref().unwrap_or("").to_string();
        let ports = spec.ports.as_deref().unwrap_or(&[]);

        let mut proxies = self.proxies.lock().await;

        for svc_port in ports {
            let target_port = svc_port.target_port.clone()
                .unwrap_or_else(|| IntOrString::Int(svc_port.port));

            // Bind on ClusterIP:servicePort via nftables DNAT
            if !cluster_ip.is_empty() && cluster_ip != "None" {
                let port_key = format!("{}:clusterip:{}", key, svc_port.port);
                if let Some(old) = proxies.remove(&port_key) {
                    old.handle.abort();
                }
                let listen_addr = format!("{}:{}", cluster_ip, svc_port.port);
                let svc_port_num = svc_port.port as u16;
                // Resolve container port: targetPort can be an integer or a port name
                let container_port = match &target_port {
                    IntOrString::Int(n) => *n as u16,
                    IntOrString::String(name) => {
                        // Look up the named port from pod container specs
                        Self::resolve_named_port(&self.store, &svc_ns, name).await.unwrap_or(svc_port_num)
                    }
                };

                // Resolve backend pods matching the selector
                let backends = Self::resolve_backend_pods(&self.store, &self.process_tracker, &selector, &svc_ns, container_port).await;
                tracing::info!("resolve_backend_pods for {}: found {} backends", key, backends.len());
                let cluster_ip_addr: std::net::Ipv4Addr = cluster_ip.parse().unwrap_or(std::net::Ipv4Addr::new(10, 96, 0, 1));
                tracing::info!("Service {} → ClusterIP {} — adding DNAT with {} backends", key, listen_addr, backends.len());
                if let Err(e) = self.netmux.add_dnat(cluster_ip_addr, svc_port_num, &backends) {
                    tracing::error!("add_dnat failed for {}: {:?}", key, e);
                }
                proxies.insert(port_key, RunningProxy { handle: tokio::spawn(async { /* DNAT via nftables */ }) });
            }

            // NodePort via nftables DNAT
            if svc_type == "NodePort" || svc_type == "LoadBalancer"  {
                if let Some(node_port) = svc_port.node_port.map(|p| p as u16) {
                    let port_key = format!("{}:nodeport:{}", key, node_port);
                    if let Some(old) = proxies.remove(&port_key) {
                        old.handle.abort();
                    }
                    let listen_addr = format!("0.0.0.0:{}", node_port);
                    let container_port = match &target_port {
                        IntOrString::Int(n) => *n as u16,
                        IntOrString::String(name) => {
                            Self::resolve_named_port(&self.store, &svc_ns, name).await.unwrap_or(svc_port.port as u16)
                        }
                    };
                    let backends = Self::resolve_backend_pods(&self.store, &self.process_tracker, &selector, &svc_ns, container_port).await;
                    let cluster_ip_addr: std::net::Ipv4Addr = cluster_ip.parse().unwrap_or(std::net::Ipv4Addr::new(10, 96, 0, 1));
                    tracing::info!("Service {} → NodePort {} — adding DNAT with {} backends", key, listen_addr, backends.len());
                    if let Err(e) = self.netmux.add_dnat(cluster_ip_addr, svc_port.port as u16, &backends) {
                        tracing::error!("add_dnat failed for {}: {:?}", key, e);
                    }
                    proxies.insert(port_key, RunningProxy { handle: tokio::spawn(async { /* DNAT via nftables */ }) });
                }
            }
        }
    }

    /// Resolve backend pod IPs matching a service selector.
    async fn resolve_backend_pods(
        store: &ResourceStore,
        tracker: &ProcessTracker,
        selector: &BTreeMap<String, String>,
        ns: &str,
        port: u16,
    ) -> Vec<(std::net::Ipv4Addr, u16)> {
        let pod_trackers = store.get_by_kind("Pod").await;
        let mut backends = Vec::new();
        for t in &pod_trackers {
            if let AnyResource::Pod(pod) = &t.resource {
                if pod.metadata.namespace.as_deref().unwrap_or("default") != ns { continue; }
                let labels = pod.metadata.labels.clone().unwrap_or_default();
                if !selector.iter().all(|(k, v)| labels.get(k) == Some(v)) { continue; }
                if let Some(pod_ip) = tracker.pod_ip(pod.metadata.name.as_deref().unwrap_or("")).await {
                    backends.push((pod_ip, port));
                }
            }
        }
        backends
    }

    /// Resolve a named port from pod containers to a numeric port.
    async fn resolve_named_port(store: &ResourceStore, ns: &str, name: &str) -> Option<u16> {
        let pods = store.get_by_kind("Pod").await;
        for t in &pods {
            if t.resource.namespace() != ns { continue; }
            let containers = crate::types::extract_containers(&t.resource);
            for c in &containers {
                if let Some(ports) = &c.ports {
                    for p in ports {
                        if p.name.as_deref() == Some(name) {
                            return Some(p.container_port as u16);
                        }
                    }
                }
            }
        }
        // Fallback: try to parse as number (e.g., targetPort was set as "8080")
        name.parse::<u16>().ok()
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
                // Extract port from key format "ns/name:clusterip:port"
                if let Some(port_str) = key.rsplit(':').next() {
                    if let Ok(port) = port_str.parse::<u16>() {
                        // We don't have the ClusterIP from the key alone, remove all matching
                        // We'll use a shortcut: just remove DNAT for the known IPs
                        // The proper way would be to look up the service from the store
                    }
                }
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
            tracing::info!("ServiceResource::on_apply for {}", svc.metadata.name.as_deref().unwrap_or("?"));
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
