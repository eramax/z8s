//! Cluster bootstrap objects (in-cluster API, defaults, TLS, admin SA).

pub mod admin_sa;
pub mod in_cluster_api;
pub mod rbac;
pub mod service_account;
pub mod tls;

pub use admin_sa::{ensure_admin_sa, read_admin_token};
pub use in_cluster_api::{
    api_backend_endpoint, ensure_kubernetes_service, kubernetes_cluster_ip,
};
pub use rbac::ensure_bootstrap_rbac;
pub use service_account::ensure_default_service_account;
pub use tls::ensure_tls_certs;
