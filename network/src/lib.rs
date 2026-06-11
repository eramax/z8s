//! # Network Module
//!
//! All network functionality in z8s lives in this crate:
//!
//! - [`NetMux`] — the unified network engine (IPAM + nftables + DNS + Ingress)
//! - [`NetworkEngine`] — trait that controllers/services use
//! - [`PodResolver`] — trait for backend resolution
//!
//! ## Module Structure
//!
//! - [`ipam`] — IP address pool, CIDR parsing
//! - [`ipv6`] — IPv6 pool for public IP assignment
//! - [`netlink`] — raw RTNETLINK operations
//! - [`veth`] — veth pair management
//! - [`nft`] — nftables engine (SNAT, DNAT, forward rules, sets)
//! - [`dns`] — in-cluster DNS server with caching
//! - [`ingress`] — L7 HTTP ingress listener
//! - [`np_controller`] — NetworkPolicy controller
//! - [`planner`] — pure desired-state planner
//! - [`sync`] — reconciler that applies planner output
//! - [`state`] — stable identifiers for idempotent ops
//! - [`rule`] — declarative nftables rule types
//!
//! ## Usage
//!
//! ```rust,ignore
//! use network::NetMux;
//!
//! let netmux = NetMux::new("10.42.0.0/20", "node-1")?;
//! netmux.init_nft("10.42.0.0/20").await?;
//! let (pod_ip, host_idx, peer_idx) = netmux.attach_pod("uid-1", None, None)?;
//! ```

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

use z8s_core::types::Service;

pub use crate::dns::DnsState;
pub use crate::ingress::IngressState;
pub use crate::ipam::{IpPool, Ipv4Cidr};
pub use crate::ipv6::Ipv6Pool;
pub use crate::nft::NftEngine;
pub use crate::np_controller::NetworkPolicyController;
pub use crate::planner::{NetworkPlanner, PlannedNetwork};
pub use crate::rule::{NftAction, NftRule};
pub use crate::state::{RuleKey, TableId, CHAIN_LAYOUT};
pub use crate::sync::reconcile_network;
pub use crate::veth::{
    add_default_route, add_pod_host_route, assign_gateway, assign_ip, bring_up_veth,
    clean_orphan_veths, create_pod_veth, del_pod_host_route, delete_veth, open_pod_netns,
    peer_name_from_uid, resolve_zeth_ifindex_in_pod_netns, veth_name_from_uid, NetNsGuard,
};

pub mod dns;
pub mod ingress;
pub mod ipam;
pub mod ipv6;
pub mod netlink;
pub mod nft;
pub mod np_controller;
pub mod planner;
pub mod rule;
pub mod state;
pub mod sync;
pub mod veth;

// ── NetworkEngine trait ────────────────────────────────────────────────

/// A service backend endpoint (host:port).
#[derive(Debug, Clone)]
pub struct ServiceEndpoint {
    pub host: String,
    pub port: u16,
}

/// Network engine — implemented by NetMux.
/// Controllers and services use this trait to interact with the network.
#[async_trait::async_trait]
pub trait NetworkEngine: Send + Sync {
    /// Optional DNS port (None = default 53).
    fn dns_port(&self) -> Option<u16> {
        None
    }

    /// Sync a service — install/remove DNAT and DNS records.
    async fn sync_service(&self, svc: &Service) -> Result<()>;

    /// Remove a service's network state.
    async fn remove_service(&self, ns: &str, name: &str) -> Result<()>;

    /// Sync services matching a label selector.
    async fn sync_services_for_labels(
        &self,
        ns: &str,
        labels: &std::collections::BTreeMap<String, String>,
    ) -> Result<()>;
}

/// Trait for resolving a pod name to its IP — used by ingress to forward requests.
#[async_trait::async_trait]
pub trait PodResolver: Send + Sync {
    /// Check if a pod is alive (running and ready).
    async fn is_pod_alive(&self, pod_name: &str) -> bool;

    /// Get the host port to connect to a pod's container port.
    async fn backend_connect_port(&self, pod_name: &str, container_port: u16) -> u16;
}

// ── NetMux ─────────────────────────────────────────────────────────────

/// Unified network engine — one pool, veth management, host routing, nftables.
/// All network components (VNet, NSG, RouteTable, Service) go through this.
pub struct NetMux {
    pool: Mutex<IpPool>,
    subnet_pools: Mutex<HashMap<String, IpPool>>,
    prefix: u8,
    pub gateway: Ipv4Addr,
    pub(crate) nft: Arc<NftEngine>,
    pub ingress_state: Arc<IngressState>,
    pub dns_state: DnsState,
}

impl NetMux {
    /// Create a new NetMux with the given pod CIDR and node name.
    pub fn new(pod_cidr: &str, node_name: &str) -> Result<Self> {
        let cidr = Ipv4Cidr::parse(pod_cidr).context("Invalid pod CIDR")?;
        let gateway = Ipv4Cidr::gateway_for(&cidr);
        let nft = Arc::new(NftEngine::new(node_name));
        let ingress_state = Arc::new(IngressState::new());
        let dns_state = DnsState::new();
        Ok(Self {
            pool: Mutex::new(IpPool::new(cidr.clone())),
            subnet_pools: Mutex::new(HashMap::new()),
            prefix: cidr.prefix,
            gateway,
            nft,
            ingress_state,
            dns_state,
        })
    }

    /// Allocate the next available pod IP from the main pool.
    pub fn allocate_ip(&self) -> Option<Ipv4Addr> {
        self.pool
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            })
            .allocate()
    }

    /// Release an IP back to the pool.
    pub fn release_ip(&self, ip: Ipv4Addr) {
        self.pool
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            })
            .release(ip);

        // Also release from any subnet pool that contains the IP
        let mut pools = self.subnet_pools.lock().unwrap_or_else(|e| {
            tracing::warn!("mutex poisoned");
            e.into_inner()
        });
        for pool in pools.values_mut() {
            if pool.cidr().contains(&ip) {
                pool.release(ip);
                return;
            }
        }
    }

    /// Number of free IPs in the main pool.
    pub fn count_free(&self) -> usize {
        self.pool
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            })
            .count_free()
    }

    /// Register a subnet CIDR for a named subnet.
    pub fn register_subnet_cidr(&self, name: &str, cidr_str: &str) -> Result<()> {
        let cidr = Ipv4Cidr::parse(cidr_str).context("Invalid subnet CIDR")?;
        let mut pools = self.subnet_pools.lock().unwrap_or_else(|e| {
            tracing::warn!("mutex poisoned");
            e.into_inner()
        });
        if !pools.contains_key(name) {
            pools.insert(name.to_string(), IpPool::new(cidr));
            info!("Registered subnet '{}' with CIDR {}", name, cidr_str);
        }
        Ok(())
    }

    /// Attach a pod to the network: create veth, host route, gateway IP.
    pub fn attach_pod(
        &self,
        pod_uid: &str,
        container_pid: Option<u32>,
        subnet: Option<&str>,
    ) -> Result<(Ipv4Addr, u32, u32)> {
        let pod_ip = if let Some(subnet_name) = subnet {
            let mut pools = self.subnet_pools.lock().unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            });
            pools
                .get_mut(subnet_name)
                .and_then(|p| p.allocate())
                .context(format!("No IPs available in subnet '{}'", subnet_name))?
        } else {
            self.allocate_ip().context("No IPs available in pod CIDR")?
        };

        let (_host_name, _peer_name, host_idx, peer_idx) =
            create_pod_veth(pod_uid, container_pid).context("create_pod_veth")?;

        bring_up_veth(host_idx).context("bring_up_veth")?;
        add_pod_host_route(&pod_ip, host_idx).context("add_pod_host_route")?;

        // Assign gateway IP to host veth (/32 avoids conflict when multiple veths exist)
        if let Err(e) = assign_gateway(&self.gateway, host_idx) {
            warn!("assign_gateway failed: {}", e);
        }

        info!(
            "Attached pod {} -> IP {} (host ifindex {}, peer ifindex {})",
            pod_uid, pod_ip, host_idx, peer_idx
        );

        Ok((pod_ip, host_idx, peer_idx))
    }

    /// Detach a pod: remove host route, delete veth, release IP.
    pub fn detach_pod(&self, pod_uid: &str, pod_ip: &Ipv4Addr, host_ifindex: u32) -> Result<()> {
        let host_name = veth_name_from_uid(pod_uid);
        if let Err(e) = del_pod_host_route(pod_ip, host_ifindex) {
            warn!("Failed to delete host route for {}: {}", pod_ip, e);
        }
        if let Err(e) = delete_veth(&host_name) {
            warn!("Failed to delete veth {}: {}", host_name, e);
        }
        self.release_ip(*pod_ip);
        info!("Detached pod {} (IP {})", pod_uid, pod_ip);
        Ok(())
    }

    /// Move peer veth into a pod's netns and configure it.
    /// Caller is responsible for running this from a context where the child
    /// has unshared CLONE_NEWNET.
    pub fn configure_pod_netns(
        &self,
        _pod_uid: &str,
        pod_ip: &Ipv4Addr,
        container_pid: u32,
        peer_ifindex: u32,
    ) -> Result<()> {
        let netns_path = format!("/proc/{}/ns/net", container_pid);

        if peer_ifindex != 0 {
            crate::netlink::move_peer_to_netns(peer_ifindex, container_pid)
                .context("move_peer_to_netns")?;
        }

        let _guard = NetNsGuard::new()?;

        // Resolve the target ifindex. When peer was created in host netns, we
        // use the index we were given. When peer was created in the pod netns
        // via IFLA_NET_NS_PID, we look up the zeth-* name inside the pod netns.
        let target_ifindex = if peer_ifindex == 0 {
            // Enter pod netns to resolve the zeth-* ifindex
            let fd = crate::netlink::open_netns(&netns_path).context("open pod netns")?;
            z8s_core::syscall::setns(&fd, rustix::thread::LinkNameSpaceType::Network)
                .context("setns into pod netns")?;
            drop(fd);
            let idx = crate::veth::resolve_zeth_ifindex_in_pod_netns(_pod_uid)
                .context("resolve zeth ifindex in pod netns")?;
            // We are now inside the pod netns and will do all configuration here
            assign_ip(idx, pod_ip, 32).context("assign_ip in pod netns")?;
            crate::netlink::set_link_up(idx).context("set_link_up peer in pod netns")?;
            add_default_route(idx, &self.gateway).context("add_default_route in pod netns")?;
            // NetNsGuard will restore host netns on drop
            return Ok(());
        } else {
            peer_ifindex
        };

        // Enter pod netns to assign IP and configure networking.
        let netns_fd = crate::netlink::open_netns(&netns_path).context("open pod netns")?;
        z8s_core::syscall::setns(&netns_fd, rustix::thread::LinkNameSpaceType::Network)
            .context("setns into pod netns")?;
        drop(netns_fd);

        assign_ip(target_ifindex, pod_ip, 32).context("assign_ip in pod netns")?;
        crate::netlink::set_link_up(target_ifindex).context("set_link_up peer in pod netns")?;
        add_default_route(target_ifindex, &self.gateway).context("add_default_route in pod netns")?;

        // Guard will restore host netns on drop
        Ok(())
    }

    /// Initialize nftables tables and chains.
    pub async fn init_nft(&self, pod_cidr: &str) -> Result<()> {
        self.nft.init(pod_cidr).await?;
        // Best-effort local route for service CIDR
        let svc_base = Ipv4Addr::new(10, 96, 0, 0);
        if let Err(e) = crate::netlink::add_local_service_cidr(&svc_base, 12) {
            warn!("Service CIDR local route: {} (ClusterIP may be unreachable)", e);
        } else {
            info!("Service CIDR 10.96.0.0/12 routed locally for ClusterIP DNAT");
        }
        Ok(())
    }

    /// Clean up orphaned veths at startup.
    pub fn clean_orphan_veths(&self, active_uids: &[String]) -> Result<()> {
        clean_orphan_veths(active_uids)
    }

    /// Enable IP forwarding.
    pub fn enable_ip_forward() -> Result<()> {
        crate::netlink::enable_ip_forward()
    }

    /// Ensure loopback is up.
    pub fn ensure_loopback_up() -> Result<()> {
        crate::netlink::ensure_loopback_up()
    }

    /// Cleanup all z8s nftables rules.
    pub async fn cleanup_nft(&self) -> Result<()> {
        self.nft.cleanup().await
    }

    /// Get the gateway IP.
    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    /// Get the CIDR prefix length.
    pub fn prefix(&self) -> u8 {
        self.prefix
    }

    /// Get the nftables engine (for direct rule application).
    pub fn nft(&self) -> Arc<NftEngine> {
        self.nft.clone()
    }
}

#[async_trait::async_trait]
impl NetworkEngine for NetMux {
    async fn sync_service(&self, _svc: &Service) -> Result<()> {
        // The reconciler (`reconcile_network`) handles DNAT/DNS installation.
        // This method is a no-op kept for trait compatibility.
        Ok(())
    }

    async fn remove_service(&self, _ns: &str, _name: &str) -> Result<()> {
        // The reconciler (`reconcile_network`) handles DNAT/DNS removal
        // based on the store snapshot. This method is a no-op kept for
        // trait compatibility; controllers should call `reconcile_network`
        // with a fresh snapshot to remove stale service state.
        Ok(())
    }

    async fn sync_services_for_labels(
        &self,
        _ns: &str,
        _labels: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        Ok(())
    }
}

impl Ipv4Cidr {
    /// Compute the gateway IP for a CIDR (first usable host).
    pub fn gateway_for(cidr: &Ipv4Cidr) -> Ipv4Addr {
        cidr.gateway()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netmux_new_uses_first_ip_as_gateway() {
        let m = NetMux::new("10.42.0.0/20", "node-1").unwrap();
        assert_eq!(m.gateway(), Ipv4Addr::new(10, 42, 0, 1));
        assert_eq!(m.prefix(), 20);
    }

    #[test]
    fn netmux_allocate_release() {
        let m = NetMux::new("10.42.0.0/29", "node-1").unwrap();
        let ip1 = m.allocate_ip().unwrap();
        assert!(m.allocate_ip().is_some());
        m.release_ip(ip1);
        let _ = m.allocate_ip();
    }

    #[test]
    fn netmux_register_subnet() {
        let m = NetMux::new("10.42.0.0/16", "node-1").unwrap();
        m.register_subnet_cidr("db-subnet", "10.43.0.0/24").unwrap();
        // Same subnet twice is idempotent
        m.register_subnet_cidr("db-subnet", "10.43.0.0/24").unwrap();
    }

    #[test]
    fn netmux_invalid_cidr_errors() {
        assert!(NetMux::new("not-a-cidr", "node-1").is_err());
    }
}
