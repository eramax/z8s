//! Cluster bootstrap objects (in-cluster API, defaults).

pub mod in_cluster_api;

pub use in_cluster_api::{
    api_backend_endpoint, ensure_kubernetes_service, kubernetes_cluster_ip,
};
