use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use anyhow::{Context, Result};
use tracing::{info, warn, debug};
use tokio::time::sleep;

use super::NetMux;

/// Per-node info tracked by each member of the cluster.
/// Used for cross-node routing and health monitoring.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub name: String,
    pub host_ip: Ipv4Addr,
    pub pod_cidr: Option<String>,
    pub last_seen: Instant,
}

/// Per-VNet bitmap tracking which /24 blocks are allocated.
#[derive(Debug, Clone)]
struct VNetBitmap {
    base: u32,
    size: u32,
    used: Vec<bool>,
}

/// Multi-node cluster state — manages peers, per-VNet /24 bitmaps,
/// join-handshake, heartbeat, and cross-node routes.
/// Plan §10: two-level IPAM (global /24 bitmap + local BTreeSet).
pub struct Cluster {
    name: String,
    host_ip: Ipv4Addr,
    assigned_cidr: String,
    join_token: String,
    api_port: u16,
    peers: Mutex<HashMap<String, NodeInfo>>,
    /// Per-VNet bitmaps: "vnet_name" -> VNetBitmap
    bitmaps: Mutex<HashMap<String, VNetBitmap>>,
}

impl Cluster {
    /// Create a new Cluster. Initializes the per-VNet /24 bitmap
    /// and populates seed peers from `--peers` flag.
    pub fn new(name: String, host_ip: Ipv4Addr, pod_cidr: &str, api_port: u16, peers: &[(String, String)]) -> Result<Self> {
        let cidr = super::pool::Ipv4Cidr::parse(pod_cidr).context("Invalid pod CIDR")?;
        let host_bits = 32 - cidr.prefix;
        let num_blocks = 1u32 << (host_bits - 8);

        let mut bitmaps = HashMap::new();
        bitmaps.insert("default".to_string(), VNetBitmap {
            base: cidr.network_u32(),
            size: num_blocks,
            used: {
                let mut b = vec![false; num_blocks as usize];
                b[0] = true; // reserve our own block
                b
            },
        });

        let base_network = cidr.network_u32();
        let our_block = format!("{}/24", Ipv4Addr::from(base_network));

        let mut peer_map = HashMap::new();
        for (peer_name, peer_ip) in peers {
            let ip: Ipv4Addr = peer_ip.parse()?;
            peer_map.insert(peer_name.clone(), NodeInfo {
                name: peer_name.clone(),
                host_ip: ip,
                pod_cidr: None,
                last_seen: Instant::now(),
            });
            info!("Cluster: known peer {} -> {}", peer_name, peer_ip);
        }

        Ok(Self {
            name,
            host_ip,
            assigned_cidr: our_block,
            join_token: "z8s-cluster-token".to_string(),
            api_port,
            peers: Mutex::new(peer_map),
            bitmaps: Mutex::new(bitmaps),
        })
    }

    pub fn handle_join(&self, node_name: &str, node_ip: Ipv4Addr, token: &str, vnet: &str) -> Result<String> {
        if token != self.join_token {
            anyhow::bail!("Invalid join token");
        }

        let mut bitmaps = self.bitmaps.lock().expect("lock poisoned");
        let bm = bitmaps.get_mut(vnet).ok_or_else(|| {
            anyhow::anyhow!("VNet '{}' not found in bitmap", vnet)
        })?;

        let block_idx = bm.used.iter().position(|b| !*b).ok_or_else(|| {
            anyhow::anyhow!("No free /24 blocks available in VNet '{}'", vnet)
        })?;
        bm.used[block_idx] = true;

        let block_ip = Ipv4Addr::from(bm.base + (block_idx as u32) * 256);
        let assigned_cidr = format!("{}/24", block_ip);

        let mut peers = self.peers.lock().expect("lock poisoned");
        peers.insert(node_name.to_string(), NodeInfo {
            name: node_name.to_string(),
            host_ip: node_ip,
            pod_cidr: Some(assigned_cidr.clone()),
            last_seen: Instant::now(),
        });

        info!("Cluster: node '{}' joined VNet '{}', assigned {}", node_name, vnet, assigned_cidr);
        Ok(assigned_cidr)
    }

    pub async fn announce_to_peers(&self) -> Result<()> {
        let peers = self.peers.lock().expect("lock poisoned").clone();
        let body = serde_json::json!({
            "node_name": self.name,
            "node_ip": self.host_ip.to_string(),
            "token": self.join_token,
            "vnet": "default",
        });
        let body_bytes = serde_json::to_vec(&body).context("serialize join body")?;

        for (_, peer) in &peers {
            let addr = format!("{}:{}", peer.host_ip, self.api_port);
            let request = format!(
                "POST /join HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                addr, body_bytes.len()
            );
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(mut stream) => {
                    use tokio::io::AsyncWriteExt;
                    let _ = stream.write_all(request.as_bytes()).await;
                    let _ = stream.write_all(&body_bytes).await;
                    let mut resp = Vec::new();
                    use tokio::io::AsyncReadExt;
                    let _ = stream.read_to_end(&mut resp).await;
                    if let Ok(text) = String::from_utf8(resp) {
                        debug!("Joined via {}: {}", peer.name, text.lines().last().unwrap_or(""));
                    }
                }
                Err(e) => debug!("Announce to {} failed: {}", peer.name, e),
            }
        }
        Ok(())
    }

    /// Periodic heartbeat loop — announces presence and detects dead peers.
    pub async fn heartbeat_loop(&self, netmux: &NetMux) -> Result<()> {
        loop {
            sleep(Duration::from_secs(30)).await;
            let _ = self.announce_to_peers().await;

            let mut peers = self.peers.lock().expect("lock poisoned");
            let dead: Vec<String> = peers.iter()
                .filter(|(_, n)| n.last_seen.elapsed() > Duration::from_secs(90))
                .map(|(name, _)| name.clone())
                .collect();
            for name in &dead {
                if let Some(node) = peers.remove(name) {
                    info!("Cluster: removing dead peer '{}' ({}), routes cleaned", name, node.host_ip);
                }
            }
            drop(peers);

            // Reinstall routes after peer changes
            let _ = self.install_routes(netmux).await;
        }
    }

    /// Install cross-node routes for all known peers.
    pub async fn install_routes(&self, netmux: &NetMux) -> Result<()> {
        let peers = self.peers.lock().expect("lock poisoned").clone();
        for (name, peer) in &peers {
            if let Some(ref cidr) = peer.pod_cidr {
                match netmux.add_subnet_route_raw(cidr, &peer.host_ip.to_string()) {
                    Ok(()) => info!("Route: {} via {} ({})", cidr, peer.host_ip, name),
                    Err(e) => warn!("Failed to install route to {}: {}", name, e),
                }
            }
        }
        Ok(())
    }

    pub fn update_peer_heartbeat(&self, name: &str) {
        if let Some(peer) = self.peers.lock().expect("lock poisoned").get_mut(name) {
            peer.last_seen = Instant::now();
        }
    }

    pub fn name(&self) -> &str { &self.name }
    pub fn host_ip(&self) -> Ipv4Addr { self.host_ip }
    pub fn assigned_cidr(&self) -> &str { &self.assigned_cidr }
    pub fn join_token(&self) -> &str { &self.join_token }
    pub fn peers(&self) -> HashMap<String, NodeInfo> { self.peers.lock().expect("lock poisoned").clone() }
}
