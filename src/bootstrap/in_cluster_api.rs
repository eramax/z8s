//! In-cluster Kubernetes API service (R0): Service `kubernetes` + DNS.

use std::net::Ipv4Addr;

use anyhow::{Context, Result};
use tracing::info;

use crate::store::{AnyResource, StoreBackend};
use crate::types::{ObjectMeta, Service, ServicePort, ServiceSpec};

/// Well-known ClusterIP for the `kubernetes` Service (first host in default service CIDR).
pub fn kubernetes_cluster_ip() -> Ipv4Addr {
    let cfg = crate::config::get();
    let base = u32::from_be_bytes(cfg.service_cidr_base);
    Ipv4Addr::from(base | 1)
}

/// Where the API process listens (for DNAT backends).
pub fn api_backend_endpoint() -> (Ipv4Addr, u16) {
    let cfg = crate::config::get();
    let port = cfg.api_port;
    if cfg.peers.is_empty() {
        let ip = cfg
            .node_ip
            .parse::<Ipv4Addr>()
            .unwrap_or(Ipv4Addr::new(127, 0, 0, 1));
        return (ip, port);
    }
    for (name, ip_str) in &cfg.peers {
        if name == "main" {
            if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                return (ip, port);
            }
        }
    }
    if let Some((_, ip_str)) = cfg.peers.first() {
        if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
            return (ip, port);
        }
    }
    (
        cfg.node_ip
            .parse()
            .unwrap_or(Ipv4Addr::new(127, 0, 0, 1)),
        port,
    )
}

/// Create the cluster-local `kubernetes` Service if missing.
pub async fn ensure_kubernetes_service(store: &dyn StoreBackend) -> Result<()> {
    let uid = "Service/default/kubernetes";
    if store.get(uid).await.is_some() {
        return Ok(());
    }

    let cluster_ip = kubernetes_cluster_ip();
    let svc = Service {
        metadata: ObjectMeta {
            name: Some("kubernetes".into()),
            namespace: Some("default".into()),
            uid: Some("service-kubernetes".into()),
            labels: Some({
                let mut m = std::collections::BTreeMap::new();
                m.insert("component".into(), "apiserver".into());
                m.insert("provider".into(), "z8s".into());
                m
            }),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            type_: Some("ClusterIP".into()),
            cluster_ip: Some(cluster_ip.to_string()),
            ports: Some(vec![
                ServicePort {
                    name: Some("https".into()),
                    port: 443,
                    target_port: Some(crate::types::IntOrString::Int(
                        crate::config::get().api_port as i32,
                    )),
                    protocol: Some("TCP".into()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };

    store
        .apply(AnyResource::Service(svc))
        .await
        .context("apply kubernetes Service")?;
    info!(
        "Bootstrapped Service kubernetes/default ClusterIP {}",
        cluster_ip
    );
    Ok(())
}
