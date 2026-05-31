use async_trait::async_trait;
use crate::types::Service;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ServiceEndpoint {
    pub host: String,
    pub port: u16,
}

#[async_trait]
pub trait NetworkEngine: Send + Sync {
    fn dns_port(&self) -> Option<u16> { crate::config::dns_port() }
    async fn sync_service(&self, svc: &Service) -> anyhow::Result<()>;
    async fn remove_service(&self, ns: &str, name: &str) -> anyhow::Result<()>;
    async fn sync_services_for_labels(&self, ns: &str, labels: &BTreeMap<String, String>) -> anyhow::Result<()>;
    async fn compute_endpoints(&self, _svc: &Service) -> crate::types::Endpoints {
        crate::types::Endpoints::default()
    }
    async fn compute_endpointslices(&self, _svc: &Service) -> Vec<crate::types::EndpointSlice> {
        vec![]
    }
}

#[async_trait]
pub trait PodResolver: Send + Sync {
    async fn is_pod_alive(&self, pod_name: &str) -> bool;
    async fn backend_connect_port(&self, pod_name: &str, container_port: u16) -> u16;
}
