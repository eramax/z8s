use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};
use k8s_openapi::api::networking::v1::Ingress;

use crate::types::{AnyResource, ResourceStore};

pub struct IngressState {
    routes: std::sync::RwLock<HashMap<String, (String, u16)>>,
}

impl IngressState {
    pub fn new() -> Self {
        Self { routes: std::sync::RwLock::new(HashMap::new()) }
    }
}

/// L7 ingress — TCP-level proxy that routes by Host header.
pub struct IngressController {
    state: Arc<IngressState>,
    store: Arc<ResourceStore>,
}

impl IngressController {
    pub fn new(store: Arc<ResourceStore>, state: Arc<IngressState>) -> Self {
        Self { store, state }
    }

    pub async fn start_http(&self) -> Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], 80));
        let listener = TcpListener::bind(addr).await?;
        info!("Ingress: listening on {}", addr);
        let state = self.state.clone();
        let store = self.store.clone();

        loop {
            let (client, peer) = listener.accept().await?;
            let state = state.clone();
            let store = store.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(client, &state, &store).await {
                    warn!("Ingress conn from {}: {}", peer, e);
                }
            });
        }
    }

    pub fn apply_ingress(&self, ingress: &Ingress) -> Result<()> {
        let spec = match ingress.spec.as_ref() {
            Some(s) => s,
            None => return Ok(()),
        };
        let mut routes = self.state.routes.write().expect("lock poisoned");
        routes.clear();
        if let Some(rules) = &spec.rules {
            for rule in rules {
                let host = rule.host.as_deref().unwrap_or("*");
                if let Some(http) = &rule.http {
                    for path in &http.paths {
                        let port = path.backend.service.as_ref()
                            .and_then(|svc| svc.port.as_ref())
                            .and_then(|p| p.number)
                            .unwrap_or(80) as u16;
                        let service_name = path.backend.service.as_ref()
                            .map(|s| s.name.clone())
                            .unwrap_or_default();
                        routes.insert(host.to_string(), (service_name.clone(), port));
                        info!("Ingress: {} -> {}:{}", host, service_name, port);
                    }
                }
            }
        }
        Ok(())
    }
}

async fn handle_connection(
    mut client: TcpStream,
    state: &IngressState,
    store: &ResourceStore,
) -> Result<()> {
    // Read first 1KB to extract Host header (enough for most requests)
    let mut buf = vec![0u8; 1024];
    let n = client.peek(&mut buf).await.context("peek")?;
    if n == 0 {
        return Ok(());
    }

    let host = extract_host(&buf[..n]).unwrap_or("");
    let addr = {
        let routes = state.routes.read().expect("lock poisoned");
        routes.get(host).cloned()
            .or_else(|| routes.get("*").cloned())
    };

    let (backend_host, backend_port) = match addr {
        Some((ref svc, p)) => resolve_endpoint(store, svc, p).await,
        None => return Ok(()), // no route
    };

    let mut backend = TcpStream::connect(format!("{}:{}", backend_host, backend_port))
        .await
        .context("connect backend")?;

    tokio::io::copy_bidirectional(&mut client, &mut backend).await?;
    Ok(())
}

fn extract_host(buf: &[u8]) -> Option<&str> {
    let s = std::str::from_utf8(buf).ok()?;
    for line in s.lines() {
        if line.to_ascii_lowercase().starts_with("host:") {
            let val = line[5..].trim();
            return Some(val.split(':').next().unwrap_or(val));
        }
    }
    None
}

async fn resolve_endpoint(store: &ResourceStore, service_name: &str, port: u16) -> (String, u16) {
    let trackers = store.get_by_kind("Service").await;
    for t in &trackers {
        if let AnyResource::Service(svc) = &t.resource {
            if svc.metadata.name.as_deref() == Some(service_name) {
                let ip = svc.spec.as_ref()
                    .and_then(|s| s.cluster_ip.as_deref())
                    .unwrap_or("")
                    .to_string();
                if !ip.is_empty() && ip != "None" {
                    return (ip, port);
                }
            }
        }
    }
    ("127.0.0.1".to_string(), port)
}
