use crate::store::AnyResource;
use crate::store::StoreBackend;
use crate::types::Ingress;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

/// L7 ingress state — Host routes per Ingress UID
pub struct IngressState {
    pub routes: std::sync::RwLock<HashMap<String, HashMap<String, (String, u16)>>>,
}

impl IngressState {
    pub fn new() -> Self {
        Self {
            routes: std::sync::RwLock::new(HashMap::new()),
        }
    }
}

pub async fn start_http(state: Arc<IngressState>, store: Arc<dyn StoreBackend>) -> Result<()> {
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

pub fn apply_ingress(state: &IngressState, ingress: &Ingress) -> Result<()> {
    let uid = ingress.metadata.uid.as_deref().unwrap_or("");
    let spec = match ingress.spec.as_ref() {
        Some(s) => s,
        None => return Ok(()),
    };
    let mut routes_by_host = HashMap::new();
    if let Some(rules) = &spec.rules {
        for rule in rules {
            let host = rule.host.as_deref().unwrap_or("*");
            if let Some(http) = &rule.http {
                for path in &http.paths {
                    let port = path
                        .backend
                        .service
                        .as_ref()
                        .and_then(|svc| svc.port.as_ref())
                        .and_then(|p| p.number)
                        .unwrap_or(80) as u16;
                    let service_name = path
                        .backend
                        .service
                        .as_ref()
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    routes_by_host.insert(host.to_string(), (service_name, port));
                }
            }
        }
    }
    state
        .routes
        .write()
        .unwrap_or_else(|e| {
            tracing::warn!("rwlock poisoned");
            e.into_inner()
        })
        .insert(uid.to_string(), routes_by_host);
    Ok(())
}

pub fn remove_ingress(state: &IngressState, ingress: &Ingress) -> Result<()> {
    let uid = ingress.metadata.uid.as_deref().unwrap_or("");
    state
        .routes
        .write()
        .unwrap_or_else(|e| {
            tracing::warn!("rwlock poisoned");
            e.into_inner()
        })
        .remove(uid);
    Ok(())
}

async fn handle_connection(
    mut client: TcpStream,
    state: &IngressState,
    store: &dyn StoreBackend,
) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = client.peek(&mut buf).await.context("peek")?;
    if n == 0 {
        return Ok(());
    }

    let host = extract_host(&buf[..n]).unwrap_or("");
    let backend = {
        let routes = state.routes.read().unwrap_or_else(|e| {
            tracing::warn!("rwlock poisoned");
            e.into_inner()
        });
        routes
            .values()
            .find_map(|m| m.get(host).or_else(|| m.get("*")).cloned())
    };
    let (backend_host, backend_port) = match backend {
        Some((ref svc, p)) => resolve_endpoint(store, svc, p).await,
        None => return Ok(()),
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
            return Some(val.rfind(':').map(|i| &val[..i]).unwrap_or(val));
        }
    }
    None
}

async fn resolve_endpoint(
    store: &dyn StoreBackend,
    service_name: &str,
    port: u16,
) -> (String, u16) {
    for t in &store.get_by_kind("Service").await {
        if let AnyResource::Service(svc) = &t.resource {
            if svc.metadata.name.as_deref() == Some(service_name) {
                if let Some(ip) = svc.spec.as_ref().and_then(|s| s.cluster_ip.as_deref()) {
                    if !ip.is_empty() && ip != "None" {
                        return (ip.to_string(), port);
                    }
                }
            }
        }
    }
    ("127.0.0.1".to_string(), port)
}
