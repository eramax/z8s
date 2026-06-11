//! # L7 Ingress Listener
//!
//! A simple L7 HTTP ingress that routes by `Host:` header to a backend
//! service. This is intentionally minimal — no TLS termination, no path
//! matching, no rate limiting. Use the L4 services for the heavy lifting.
//!
//! ## Flow
//!
//! ```text
//! Client → :80 (ingress listener)
//!              ↓ peek Host header
//!         lookup (Host) → (Service, Port)
//!              ↓
//!         resolve Service → ClusterIP
//!              ↓
//!         tcp connect to ClusterIP:Port
//!              ↓
//!         bidirectional copy
//! ```
//!
//! ## State
//!
//! `IngressState` is shared with the network reconciler:
//! - Reconciler populates it from Ingress resources
//! - Listener reads it on every request

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt as _;
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use z8s_core::types::{ResourceRecord, Resource, Service, ServiceSpec};

/// L7 ingress state — host → (service_name, port).
pub struct IngressState {
    pub routes: std::sync::RwLock<HashMap<String, HashMap<String, (String, u16)>>>,
}

impl IngressState {
    pub fn new() -> Self {
        Self {
            routes: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Replace routes for an Ingress UID.
    pub fn set_routes(&self, uid: &str, routes: HashMap<String, (String, u16)>) {
        self.routes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(uid.to_string(), routes);
    }

    /// Remove routes for an Ingress UID.
    pub fn remove_routes(&self, uid: &str) {
        self.routes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(uid);
    }

    /// Find a route for the given host. Returns (service_name, port).
    pub fn lookup(&self, host: &str) -> Option<(String, u16)> {
        let routes = self.routes.read().unwrap_or_else(|e| e.into_inner());
        routes
            .values()
            .find_map(|m| m.get(host).or_else(|| m.get("*")).cloned())
    }
}

impl Default for IngressState {
    fn default() -> Self {
        Self::new()
    }
}

/// Start the L7 HTTP listener. Returns immediately; spawns a task per request.
pub async fn start_http(state: Arc<IngressState>, store: Arc<dyn z8s_core::store::StoreBackend>) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], 80));
    let listener = TcpListener::bind(addr).await?;
    info!("Ingress: listening on {}", addr);
    loop {
        let (client, peer) = listener.accept().await?;
        let s = state.clone();
        let st = store.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(client, &s, st.as_ref()).await {
                warn!("Ingress conn from {}: {}", peer, e);
            }
        });
    }
}

/// Apply an Ingress resource to the ingress state.
pub fn apply_ingress(state: &IngressState, ingress_record: &ResourceRecord) -> Result<()> {
    let uid = ingress_record.uid();
    let mut routes_by_host = HashMap::new();
    if let z8s_core::types::AnyResource::Ingress(ingress) = &ingress_record.spec {
        // TODO: implement when core types are enriched (IngressSpec is currently empty).
        // For now, use uid as a placeholder host.
        let _ = &ingress.spec;
        let host = uid;
        let port = 80;
        routes_by_host.insert(host.to_string(), (uid.to_string(), port));
    }
    state.set_routes(uid, routes_by_host);
    Ok(())
}

/// Remove an Ingress resource from the ingress state.
pub fn remove_ingress(state: &IngressState, ingress_record: &ResourceRecord) -> Result<()> {
    state.remove_routes(ingress_record.uid());
    Ok(())
}

async fn handle_connection(
    mut client: TcpStream,
    state: &IngressState,
    store: &dyn z8s_core::store::StoreBackend,
) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = client.peek(&mut buf).await.context("peek")?;
    if n == 0 {
        return Ok(());
    }
    let host = extract_host(&buf[..n]).unwrap_or("").to_string();
    let backend = state.lookup(&host);
    let (backend_host, backend_port) = match backend {
        Some((ref svc, p)) => resolve_endpoint(store, svc, p).await,
        None => return Ok(()),
    };
    let mut backend_conn = TcpStream::connect(format!("{}:{}", backend_host, backend_port))
        .await
        .context("connect backend")?;
    tokio::io::copy_bidirectional(&mut client, &mut backend_conn).await?;
    Ok(())
}

fn extract_host(buf: &[u8]) -> Option<&str> {
    let s = std::str::from_utf8(buf).ok()?;
    for line in s.lines() {
        if line.to_ascii_lowercase().starts_with("host:") {
            let val = line[5..].trim();
            return Some(val.rfind(':').map(|i| &val[..i]).unwrap_or(val));
        }
    }
    None
}

async fn resolve_endpoint(
    store: &dyn z8s_core::store::StoreBackend,
    service_name: &str,
    port: u16,
) -> (String, u16) {
    let trackers = store.get_by_kind("Service").await;
    for t in &trackers {
        if let z8s_core::types::AnyResource::Service(svc) = &t.spec {
            if svc.name() == service_name {
                if let Some(cluster_ip) = svc
                    .spec
                    .as_ref()
                    .and_then(|s: &ServiceSpec| s.cluster_ip.as_deref())
                {
                    if !cluster_ip.is_empty() && cluster_ip != "None" {
                        return (cluster_ip.to_string(), port);
                    }
                }
            }
        }
    }
    ("127.0.0.1".to_string(), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_basic() {
        let req = b"GET / HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test\r\n\r\n";
        assert_eq!(extract_host(req), Some("example.com"));
    }

    #[test]
    fn extract_host_with_port() {
        let req = b"GET / HTTP/1.1\r\nHost: example.com:8080\r\n\r\n";
        assert_eq!(extract_host(req), Some("example.com"));
    }

    #[test]
    fn extract_host_missing() {
        let req = b"GET / HTTP/1.1\r\nUser-Agent: test\r\n\r\n";
        assert_eq!(extract_host(req), None);
    }

    #[test]
    fn state_set_lookup() {
        let state = IngressState::new();
        let mut routes = HashMap::new();
        routes.insert("example.com".to_string(), ("my-svc".to_string(), 8080));
        state.set_routes("ingress-uid-1", routes);

        let r = state.lookup("example.com");
        assert_eq!(r, Some(("my-svc".to_string(), 8080)));
        assert_eq!(state.lookup("unknown.com"), None);
    }

    #[test]
    fn state_wildcard_fallback() {
        let state = IngressState::new();
        let mut routes = HashMap::new();
        routes.insert("*".to_string(), ("default-svc".to_string(), 80));
        state.set_routes("ingress-uid-1", routes);

        assert_eq!(
            state.lookup("anything.com"),
            Some(("default-svc".to_string(), 80))
        );
    }

    #[test]
    fn state_remove_routes() {
        let state = IngressState::new();
        let mut routes = HashMap::new();
        routes.insert("example.com".to_string(), ("my-svc".to_string(), 8080));
        state.set_routes("ingress-uid-1", routes);
        state.remove_routes("ingress-uid-1");
        assert_eq!(state.lookup("example.com"), None);
    }
}

// Suppress unused warnings for unused import
#[allow(dead_code)]
fn _suppress_unused() {
    let _: Option<Service> = None;
}
