pub mod dns;
pub mod port_publish;
pub mod service_proxy;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Endpoints, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::BTreeMap;

/// The port the z8s DNS server is listening on (53 or 5353). Set once at startup.
static DNS_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

pub fn set_dns_port(port: u16) {
    DNS_PORT.set(port).ok();
}

pub fn dns_port() -> Option<u16> {
    DNS_PORT.get().copied()
}

#[derive(Debug, Clone)]
pub struct ServiceEndpoint {
    pub host: String,
    pub port: u16,
}

#[async_trait]
pub trait NetworkEngine: Send + Sync {
    fn dns_port(&self) -> Option<u16>;

    async fn sync_service(&self, svc: &Service);
    async fn remove_service(&self, ns: &str, name: &str);
    async fn sync_services_for_labels(&self, ns: &str, labels: &BTreeMap<String, String>);
    async fn compute_endpoints(&self, svc: &Service) -> Endpoints;
    async fn compute_endpointslices(&self, svc: &Service) -> Vec<EndpointSlice>;
}

#[async_trait]
pub trait PodResolver: Send + Sync {
    async fn is_pod_alive(&self, pod_name: &str) -> bool;
    async fn backend_connect_port(&self, pod_name: &str, container_port: u16) -> u16;
}
