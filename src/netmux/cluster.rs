use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use tracing::{info, warn};

use super::NetMux;

/// Per-node info tracked by each member.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub name: String,
    pub host_ip: Ipv4Addr,
    /// /24 block assigned to this node (e.g. "10.42.0.0/24").
    pub pod_cidr: Option<String>,
    pub last_seen: std::time::Instant,
}

/// Multi-node cluster state.
pub struct Cluster {
    /// Our node name.
    name: String,
    /// Our host IP (used by peers to reach us).
    host_ip: Ipv4Addr,
    /// Our assigned /24 block from the pod CIDR.
    assigned_cidr: String,
    /// Token to validate join requests.
    join_token: String,
    /// Peer nodes: name -> NodeInfo
    peers: Mutex<HashMap<String, NodeInfo>>,
    /// Bitmap of allocated /24 blocks per VNet.
    bitmap: Mutex<Vec<bool>>,
    /// Base IP for the /24 bitmap (network addr of pod CIDR).
    bitmap_base: u32,
    /// Number of /24 blocks available.
    bitmap_size: u32,
}

impl Cluster {
    pub fn new(name: String, host_ip: Ipv4Addr, pod_cidr: &str, peers: &[(String, String)]) -> Result<Self> {
        let cidr = super::pool::Ipv4Cidr::parse(pod_cidr).context("Invalid pod CIDR")?;
        let host_bits = 32 - cidr.prefix;
        let num_blocks = 1u32 << (host_bits - 8); // /24 blocks within the CIDR
        let mut bitmap = vec![false; num_blocks as usize];

        // Reserve first /24 for ourselves
        bitmap[0] = true;
        let base_network = cidr.network_u32();
        let our_block = format!("{}/24", Ipv4Addr::from(base_network));

        let mut peer_map = HashMap::new();
        for (peer_name, peer_ip) in peers {
            let ip: Ipv4Addr = peer_ip.parse()?;
            peer_map.insert(
                peer_name.clone(),
                NodeInfo {
                    name: peer_name.clone(),
                    host_ip: ip,
                    pod_cidr: None,
                    last_seen: std::time::Instant::now(),
                },
            );
            info!("Cluster: known peer {} -> {}", peer_name, peer_ip);
        }

        Ok(Self {
            name,
            host_ip,
            assigned_cidr: our_block,
            join_token: "z8s-cluster-token".to_string(), // configurable later
            peers: Mutex::new(peer_map),
            bitmap: Mutex::new(bitmap),
            bitmap_base: base_network,
            bitmap_size: num_blocks,
        })
    }

    /// Handle a join request from a new node.
    pub fn handle_join(&self, node_name: &str, node_ip: Ipv4Addr, token: &str) -> Result<String> {
        if token != self.join_token {
            anyhow::bail!("Invalid join token");
        }

        let mut bitmap = self.bitmap.lock().unwrap();
        let block_idx = bitmap.iter().position(|b| !*b).ok_or_else(|| {
            anyhow::anyhow!("No free /24 blocks available")
        })?;
        bitmap[block_idx] = true;

        let block_ip = Ipv4Addr::from(self.bitmap_base + (block_idx as u32) * 256);
        let assigned_cidr = format!("{}/24", block_ip);

        let mut peers = self.peers.lock().unwrap();
        peers.insert(
            node_name.to_string(),
            NodeInfo {
                name: node_name.to_string(),
                host_ip: node_ip,
                pod_cidr: Some(assigned_cidr.clone()),
                last_seen: std::time::Instant::now(),
            },
        );

        info!("Cluster: node '{}' joined, assigned {}", node_name, assigned_cidr);
        Ok(assigned_cidr)
    }

    /// Announce ourselves to seed peers (called at startup).
    pub async fn announce_to_peers(&self) -> Result<()> {
        let peers = self.peers.lock().unwrap().clone();
        for (name, peer) in &peers {
            let url = format!("http://{}:6443/join", peer.host_ip);
            let client = reqwest::Client::new();
            let body = serde_json::json!({
                "node_name": self.name,
                "node_ip": self.host_ip.to_string(),
                "token": self.join_token,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if let Ok(assigned) = resp.text().await {
                        info!("Joined cluster via '{}': assigned {}", name, assigned);
                    }
                }
                Err(e) => {
                    warn!("Failed to announce to '{}' ({}): {}", name, peer.host_ip, e);
                }
            }
        }
        Ok(())
    }

    /// Install cross-node routes for all known peers.
    pub fn install_routes(&self, netmux: &NetMux) -> Result<()> {
        let peers = self.peers.lock().unwrap().clone();
        for (name, peer) in &peers {
            if let Some(ref cidr) = peer.pod_cidr {
                let peer_host = peer.host_ip;
                match netmux.add_subnet_route_raw(cidr, &peer_host.to_string()) {
                    Ok(()) => info!("Route: {} via {} ({})", cidr, peer_host, name),
                    Err(e) => warn!("Failed to install route to {}: {}", name, e),
                }
            }
        }
        Ok(())
    }

    pub fn name(&self) -> &str { &self.name }
    pub fn host_ip(&self) -> Ipv4Addr { self.host_ip }
    pub fn assigned_cidr(&self) -> &str { &self.assigned_cidr }
    pub fn join_token(&self) -> &str { &self.join_token }
    pub fn peers(&self) -> HashMap<String, NodeInfo> { self.peers.lock().unwrap().clone() }
}
