//! Cluster bootstrap objects (in-cluster API, defaults).

pub mod in_cluster_api;
pub mod rbac;
pub mod service_account;

pub use in_cluster_api::{
    api_backend_endpoint, ensure_kubernetes_service, kubernetes_cluster_ip,
};
pub use rbac::ensure_bootstrap_rbac;
pub use service_account::ensure_default_service_account;
