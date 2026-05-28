pub mod dns;
pub mod service_proxy;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Endpoints, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::BTreeMap;

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
