use crate::store::AnyResource;
use crate::store::StoreBackend;
use crate::scheduler::process::ProcessTracker;
use crate::types::{Service, IntOrString};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::netmux::NetMux;


pub struct NetworkManager {
    pub store: Arc<dyn StoreBackend>,
    process_tracker: Arc<ProcessTracker>,
    proxies: Mutex<HashSet<String>>,
    counter: Arc<AtomicUsize>,
    netmux: Arc<NetMux>,
}

impl NetworkManager {
    pub fn new(store: Arc<dyn StoreBackend>, process_tracker: Arc<ProcessTracker>, netmux: Arc<NetMux>) -> Self {
        Self {
            store,
            process_tracker,
            proxies: Mutex::new(HashSet::new()),
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
            debug!("sync_service_proxies {}: empty selector, skipping", key);
            return; // headless or external-name services — no proxy
        }
        debug!("sync_service_proxies {}: selector={:?}", key, selector);

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
                proxies.remove(&port_key);
                let listen_addr = format!("{}:{}", cluster_ip, svc_port.port);
                let svc_port_num = svc_port.port as u16;
                // Resolve container port: targetPort can be an integer or a port name
                let container_port = match &target_port {
                    IntOrString::Int(n) => *n as u16,
                    IntOrString::String(name) => {
                        // Look up the named port from pod container specs
                        Self::resolve_named_port(self.store.as_ref(), &svc_ns, name).await.unwrap_or(svc_port_num)
                    }
                };

                // Resolve backend pods matching the selector
                let backends = Self::resolve_backend_pods(self.store.as_ref(), &self.process_tracker, &selector, &svc_ns, container_port).await;
                debug!("resolve_backend_pods for {}: found {} backends", key, backends.len());
                let cluster_ip_addr: std::net::Ipv4Addr = cluster_ip.parse().unwrap_or_else(|_| {
                    debug!("ClusterIP for {} is empty, using default", key);
                    std::net::Ipv4Addr::new(10, 96, 0, 1)
                });
                debug!("Service {} → ClusterIP {} — adding DNAT with {} backends", key, listen_addr, backends.len());
                if let Err(e) = self.netmux.nft.add_dnat(cluster_ip_addr, svc_port_num, &backends).await {
                    tracing::error!("add_dnat failed for {}: {:?}", key, e);
                }
                proxies.insert(port_key);
            }

            // NodePort via nftables DNAT
            if svc_type == "NodePort" || svc_type == "LoadBalancer"  {
                if let Some(node_port) = svc_port.node_port.map(|p| p as u16) {
                    let port_key = format!("{}:nodeport:{}", key, node_port);
                    proxies.remove(&port_key);
                    let listen_addr = format!("0.0.0.0:{}", node_port);
                    let container_port = match &target_port {
                        IntOrString::Int(n) => *n as u16,
                        IntOrString::String(name) => {
                            Self::resolve_named_port(self.store.as_ref(), &svc_ns, name).await.unwrap_or(svc_port.port as u16)
                        }
                    };
                    let backends = Self::resolve_backend_pods(self.store.as_ref(), &self.process_tracker, &selector, &svc_ns, container_port).await;
                    let cluster_ip_addr: std::net::Ipv4Addr = cluster_ip.parse().unwrap_or_else(|_| {
                        debug!("ClusterIP for {} is empty, using default", key);
                        std::net::Ipv4Addr::new(10, 96, 0, 1)
                    });
                    debug!("Service {} → NodePort {} — adding DNAT with {} backends", key, listen_addr, backends.len());
                    if let Err(e) = self.netmux.nft.add_dnat(cluster_ip_addr, svc_port.port as u16, &backends).await {
                        tracing::error!("add_dnat failed for {}: {:?}", key, e);
                    }
                    tracing::info!("calling add_nodeport_dnat for {}: node_port={}, backends={}", key, node_port, backends.len());
                    if let Err(e) = self.netmux.nft.add_nodeport_dnat(node_port, &backends).await {
                        tracing::error!("add_nodeport_dnat failed for {}: {:?}", key, e);
                    }
                    proxies.insert(port_key);
                }
            }
        }
    }

    /// Resolve backend pod IPs matching a service selector.
    /// Uses a cached pod list by namespace to avoid O(N×M) iteration.
    async fn resolve_backend_pods(
        store: &dyn StoreBackend,
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
    async fn resolve_named_port(store: &dyn StoreBackend, ns: &str, name: &str) -> Option<u16> {
        let pods = store.get_by_kind("Pod").await;
        for t in &pods {
            if t.resource.namespace() != ns { continue; }
            let containers = crate::store::extract_containers(&t.resource);
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
        let keys: Vec<_> = proxies.iter()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            proxies.remove(&key);
            info!("Stopped service proxy for {}", key);
            // Extract ClusterIP and port from store
                let store_prefix = format!("{}/{}", ns, name);
                if key.starts_with(&store_prefix) {
                    let trackers = self.store.get_by_kind("Service").await;
                    for t in &trackers {
                        if t.resource.namespace() == ns && t.resource.name() == name {
                            if let crate::store::AnyResource::Service(svc) = &t.resource {
                                if let Some(spec) = &svc.spec {
                                    if let Some(cip) = &spec.cluster_ip {
                                        if let Ok(ip) = cip.parse::<std::net::Ipv4Addr>() {
                                            if let Some(port_str) = key.rsplit(':').next() {
                                                if let Ok(port) = port_str.parse::<u16>() {
                                                    if let Err(e) = self.netmux.nft.remove_dnat(ip, port).await {
                                                        tracing::warn!("remove_dnat failed for {}: {}", key, e);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
            }
        }
    }
}

#[async_trait]
impl crate::netmux::network::NetworkEngine for NetworkManager {
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

}

use async_trait::async_trait;
use anyhow::Result;
use crate::store::ResourceTracker;
use crate::components::{Component, ReconcileContext, ResourceCategory};

pub struct ServiceResource {
    pub store: Arc<dyn StoreBackend>,
    pub network: Arc<NetworkManager>,
}

impl ServiceResource {
    pub fn new(store: Arc<dyn StoreBackend>, network: Arc<NetworkManager>) -> Self {
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

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        if let AnyResource::Service(svc) = &tracker.resource {
            self.network.sync_service(svc).await;
        }
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
