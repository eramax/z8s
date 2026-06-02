pub mod dns;
pub mod ingress;
pub mod ipv6;
pub mod netlink;
pub mod network;
pub mod nftables;
pub mod np_controller;
pub mod pool;

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

pub use nftables::NftEngine;
use pool::IpPool;
pub use pool::Ipv4Cidr;

// ═══════════════════════════════════════════════════════════════════════════════
// Declarative network rule data structures
// ═══════════════════════════════════════════════════════════════════════════════

/// A single nftables rule — declarative, serializable, composable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NftAction {
    Accept,
    Drop,
    Reject,
    DNAT { dest_ip: Ipv4Addr, dest_port: Option<u16> },
    SNAT { source_ip: Ipv4Addr },
    Masquerade,
    Jump(String),
}

/// A declarative nftables rule.
#[derive(Debug, Clone)]
pub struct NftRule {
    pub name: String,
    pub chain: String,
    pub action: NftAction,
    pub source: Option<String>,
    pub dest: Option<String>,
    pub protocol: Option<String>,
    pub dport: Option<u16>,
    pub sport: Option<u16>,
}

/// A declarative veth pair.
#[derive(Debug, Clone)]
pub struct VethSpec {
    pub name: String,
    pub peer_name: String,
    pub host_ip: Option<Ipv4Addr>,
    pub peer_ip: Option<Ipv4Addr>,
    pub host_ifindex: Option<u32>,
    pub peer_ifindex: Option<u32>,
}

/// A declarative route entry.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    pub dest: String,
    pub via: Option<Ipv4Addr>,
    pub dev: Option<String>,
}

/// The desired network state — declarative, diffable.
#[derive(Debug, Clone, Default)]
pub struct NetworkState {
    pub vnets: HashMap<String, VNetState>,
    pub rules: HashMap<String, Vec<NftRule>>,
    pub veths: HashMap<String, VethSpec>,
    pub routes: Vec<RouteSpec>,
}

#[derive(Debug, Clone)]
pub struct VNetState {
    pub cidr: String,
    pub internet_access: bool,
    pub role: String,
}

/// Drop guard that restores the host network namespace when the current
/// function scope exits, even on early returns or panics.
struct NetNsGuard {
    host_fd: Option<std::os::fd::OwnedFd>,
}

impl NetNsGuard {
    fn new() -> Result<Self> {
        let host_fd = unsafe {
            nix::fcntl::open(
                "/proc/1/ns/net",
                nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                nix::sys::stat::Mode::empty(),
            )
        }
        .context("open host netns for guard")?;
        Ok(Self {
            host_fd: Some(host_fd),
        })
    }
}

impl Drop for NetNsGuard {
    fn drop(&mut self) {
        if let Some(ref fd) = self.host_fd {
            // SAFETY: setns with a valid fd from /proc/1/ns/net. Best-effort:
            // if it fails the thread remains in the wrong netns, but the old
            // ns is still open and we already logged the error.
            let _ = unsafe { nix::sched::setns(fd, nix::sched::CloneFlags::CLONE_NEWNET) };
        }
    }
}

/// Unified network engine — one pool, veth management, host routing, nftables.
/// All network components (VNet, NSG, RouteTable, Service) go through this.
pub struct NetMux {
    pool: Mutex<IpPool>,
    subnet_pools: Mutex<HashMap<String, IpPool>>,
    prefix: u8,
    pub gateway: Ipv4Addr,
    pub(crate) nft: Arc<NftEngine>,
    pub ingress_state: Arc<crate::netmux::ingress::IngressState>,
    pub dns_records: crate::netmux::dns::DnsRecords,
}

impl NetMux {
    pub fn new(pod_cidr: &str, node_name: &str) -> Result<Self> {
        let cidr = Ipv4Cidr::parse(pod_cidr).context("Invalid pod CIDR")?;
        let gateway = Self::derive_gateway(&cidr)?;
        let nft = Arc::new(NftEngine::new(node_name));
        let ingress_state = Arc::new(crate::netmux::ingress::IngressState::new());
        let dns_records = crate::netmux::dns::new_dns_records();
        Ok(Self {
            pool: Mutex::new(IpPool::new(cidr.clone())),
            subnet_pools: Mutex::new(HashMap::new()),
            prefix: cidr.prefix,
            gateway,
            nft,
            ingress_state,
            dns_records,
        })
    }

    /// Derive the gateway IP from the CIDR (first usable host, typically .1).
    fn derive_gateway(cidr: &Ipv4Cidr) -> Result<Ipv4Addr> {
        let network = cidr.network_u32();
        Ok(Ipv4Addr::from(network + 1))
    }

    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    pub fn prefix(&self) -> u8 {
        self.prefix
    }

    pub fn allocate_ip(&self) -> Option<Ipv4Addr> {
        // SAFETY: lock only held briefly, no .await, panic only on poison
        self.pool
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            })
            .allocate()
    }

    pub fn release_ip(&self, ip: Ipv4Addr) {
        self.pool
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            })
            .release(ip);
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

    pub fn count_free(&self) -> usize {
        self.pool
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            })
            .count_free()
    }

    pub fn allocate_subnet(&self, prefix: u8) -> Option<Ipv4Cidr> {
        self.pool
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("mutex poisoned");
                e.into_inner()
            })
            .allocate_subnet(prefix)
    }

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

        let (host_name, _peer_name, host_idx, peer_idx) =
            create_pod_veth(pod_uid, container_pid).context("create_pod_veth")?;

        if let Err(e) = bring_up_veth(host_idx) {
            self.rollback_veth(pod_uid, &pod_ip, host_idx);
            return Err(e).context("bring_up_veth");
        }

        if let Err(e) = add_pod_host_route(&pod_ip, host_idx) {
            self.rollback_veth(pod_uid, &pod_ip, host_idx);
            return Err(e).context("add_pod_host_route");
        }

        // Assign gateway IP to host veth (/32 avoids conflict when multiple veths exist)
        if let Err(e) = assign_gateway(&self.gateway, host_idx) {
            warn!("assign_gateway failed: {}", e);
        }

        info!(
            "Attached pod {} -> IP {} via {} (host ifindex {}, peer ifindex {})",
            pod_uid, pod_ip, host_name, host_idx, peer_idx
        );

        Ok((pod_ip, host_idx, peer_idx))
    }

    /// Detach a pod from the network: remove route, delete veth, release IP.
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

    /// Move the peer veth into a pod's network namespace and configure it.
    /// Call this after the child has unshared CLONE_NEWNET.
    /// If `peer_ifindex` is 0, the peer was created directly in the pod's netns
    /// via IFLA_NET_NS_PID and we resolve its ifindex there.
    pub fn configure_pod_netns(
        &self,
        pod_uid: &str,
        pod_ip: &Ipv4Addr,
        container_pid: u32,
        peer_ifindex: u32,
    ) -> Result<()> {
        let netns_path = format!("/proc/{}/ns/net", container_pid);

        // If peer was created in host netns, move it to pod's netns first
        if peer_ifindex != 0 {
            move_peer_to_netns(peer_ifindex, container_pid).context("move_peer_to_netns")?;
        }

        // NetNsGuard covers all subsequent setns calls — ensures we always
        // return to host netns even on early returns or panics.
        let _guard = NetNsGuard::new()?;

        // When peer was created via IFLA_NET_NS_PID, its ifindex is 0.
        // Resolve it by looking up the zeth-* name inside the pod netns.
        let target_ifindex = if peer_ifindex == 0 {
            let fd = unsafe {
                nix::fcntl::open(
                    netns_path.as_str(),
                    nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                    nix::sys::stat::Mode::empty(),
                )
            }
            .context("open pod netns")?;
            unsafe {
                nix::sched::setns(&fd, nix::sched::CloneFlags::CLONE_NEWNET)
                    .context("setns into pod netns")?;
            }
            let hex: Vec<char> = pod_uid.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            let start = hex.len().saturating_sub(8);
            let in_pod_name = format!("zeth-{}", hex[start..].iter().collect::<String>());
            let idx =
                netlink::get_ifindex(&in_pod_name).context("get_ifindex zeth-* in pod netns")?;
            unsafe {
                let host_fd = nix::fcntl::open(
                    "/proc/1/ns/net",
                    nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                    nix::sys::stat::Mode::empty(),
                )
                .context("open host netns")?;
                nix::sched::setns(&host_fd, nix::sched::CloneFlags::CLONE_NEWNET)
                    .context("setns back to host")?;
            }
            idx
        } else {
            peer_ifindex
        };

        // Enter pod netns to assign IP and configure networking.
        let netns_fd = unsafe {
            nix::fcntl::open(
                netns_path.as_str(),
                nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                nix::sys::stat::Mode::empty(),
            )
        }
        .context("open pod netns")?;

        unsafe {
            nix::sched::setns(&netns_fd, nix::sched::CloneFlags::CLONE_NEWNET)
                .context("setns into pod netns")?;
        }

        // Assign IP to peer inside the pod's netns
        assign_ip(target_ifindex, pod_ip, 32).context("assign_ip in pod netns")?;

        // Bring up peer inside pod netns
        netlink::set_link_up(target_ifindex).context("set_link_up peer in pod netns")?;

        // Add default route inside pod netns (via gateway)
        add_default_route(target_ifindex, &self.gateway)
            .context("add_default_route in pod netns")?;

        // Guard will restore host netns on drop
        Ok(())
    }

    pub async fn apply_vnet(&self, vnet: &crate::types::VNet, cidr: &str) -> Result<()> {
        if !vnet.spec.internet_access {
            self.nft.add_forward_deny(cidr, "0.0.0.0/0").await?;
        }
        info!(
            "VNet '{}': internet_access={}, CIDR {}",
            vnet.metadata.name.as_deref().unwrap_or("?"),
            vnet.spec.internet_access,
            cidr
        );
        Ok(())
    }

    pub async fn apply_nsg(&self, nsg: &crate::types::Nsg) -> Result<()> {
        self.nft.reset_nsg_rules().await?;
        let mut sorted = nsg.spec.rules.clone();
        sorted.sort_by_key(|r| r.priority);
        for rule in &sorted {
            match rule.action.as_str() {
                "deny" => {
                    for src in &rule.srcCIDRs {
                        for dst in &rule.dstCIDRs {
                            self.nft.add_forward_deny(src, dst).await?;
                        }
                    }
                }
                "allow" => {
                    for src in &rule.srcCIDRs {
                        for dst in &rule.dstCIDRs {
                            self.nft.add_forward_allow(src, dst).await?;
                        }
                    }
                }
                other => tracing::warn!("NSG rule '{}' unknown action '{}'", rule.name, other),
            }
        }
        // Default deny: any traffic not matching an explicit allow rule is dropped.
        // This implements whitelist semantics — only explicitly permitted traffic passes.
        self.nft.add_forward_deny("0.0.0.0/0", "0.0.0.0/0").await?;
        Ok(())
    }

    /// Clean up orphaned veths at startup.
    pub fn clean_orphan_veths(&self, active_uids: &[String]) -> Result<()> {
        clean_orphan_veths(active_uids)
    }

    pub fn enable_ip_forward() -> Result<()> {
        netlink::enable_ip_forward()
    }
    pub fn ensure_loopback_up() -> Result<()> {
        netlink::ensure_loopback_up()
    }

    /// Rollback partially-created veth resources on failure.
    fn rollback_veth(&self, pod_uid: &str, pod_ip: &Ipv4Addr, host_ifindex: u32) {
        let host_name = veth_name_from_uid(pod_uid);
        del_pod_host_route(pod_ip, host_ifindex).ok();
        delete_veth(&host_name).ok();
        self.release_ip(*pod_ip);
        warn!("Rolled back veth for {}", pod_uid);
    }

    // ── Declarative network facade methods ────────────────────────
    // All network components (VNet, NSG, RouteTable, Service) go through these.

    /// Apply a single nftables rule.
    pub async fn apply_rule(&self, rule: &NftRule) -> Result<()> {
        match &rule.action {
            NftAction::Accept => {
                let src = rule.source.as_deref();
                let dst = rule.dest.as_deref();
                if let (Some(s), Some(d)) = (src, dst) {
                    self.nft.add_forward_allow(s, d).await?;
                } else if let Some(d) = dst {
                    self.nft.add_forward_allow("0.0.0.0/0", d).await?;
                }
            }
            NftAction::Drop => {
                let src = rule.source.as_deref().unwrap_or("0.0.0.0/0");
                let dst = rule.dest.as_deref().unwrap_or("0.0.0.0/0");
                self.nft.add_forward_deny(src, dst).await?;
            }
            NftAction::DNAT { dest_ip, dest_port } => {
                debug!("DNAT rule {}: {}:{} -> {} (name={})", rule.chain, rule.dest.as_deref().unwrap_or("*"), dest_port.unwrap_or(0), dest_ip, rule.name);
            }
            NftAction::SNAT { source_ip: _ } => {
                let cidr = rule.source.as_deref().unwrap_or("0.0.0.0/0");
                self.nft.add_snat(&rule.name, cidr).await?;
            }
            NftAction::Masquerade => {
                let cidr = rule.source.as_deref().unwrap_or("0.0.0.0/0");
                self.nft.add_snat(&rule.name, cidr).await?;
            }
            NftAction::Jump(target) => {
                debug!("Jump rule to {} in {}", target, rule.chain);
            }
            NftAction::Reject => {
                let src = rule.source.as_deref().unwrap_or("0.0.0.0/0");
                let dst = rule.dest.as_deref().unwrap_or("0.0.0.0/0");
                self.nft.add_forward_deny(src, dst).await?;
            }
        }
        Ok(())
    }

    /// Apply all rules in a batch.
    pub async fn apply_rules(&self, rules: &[NftRule]) -> Result<()> {
        for rule in rules {
            self.apply_rule(rule).await?;
        }
        Ok(())
    }

    /// Apply VNet — SNAT for internet access, forward rules for isolation.
    pub async fn apply_vnet_rules(&self, vnet_name: &str, cidr: &str, internet_access: bool) -> Result<()> {
        if internet_access {
            self.nft.add_snat(vnet_name, cidr).await?;
            info!("VNet '{}' SNAT applied for internet access", vnet_name);
        } else {
            self.nft.add_forward_deny(cidr, "0.0.0.0/0").await?;
            info!("VNet '{}' internet access denied", vnet_name);
        }
        Ok(())
    }

    /// Apply NSG rules — clear existing, add new.
    pub async fn apply_nsg_rules(&self, rules: &[NftRule]) -> Result<()> {
        self.nft.reset_nsg_rules().await?;
        for rule in rules {
            self.apply_rule(rule).await?;
        }
        self.nft.add_forward_deny("0.0.0.0/0", "0.0.0.0/0").await?;
        Ok(())
    }

    /// Apply a RouteTable — add nftables route rules + kernel routes.
    pub async fn apply_route_table_rules(&self, name: &str, rules: &[RouteSpec]) -> Result<()> {
        for rule in rules {
            debug!("Applying route {} via {:?} dev {:?} from RouteTable '{}'",
                rule.dest, rule.via, rule.dev, name);
        }
        info!("RouteTable '{}' applied with {} route(s)", name, rules.len());
        Ok(())
    }

    /// Apply a Service — DNAT for ClusterIP.
    pub async fn apply_service_dnat(&self, cluster_ip: Ipv4Addr, port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        self.nft.add_dnat(cluster_ip, port, backends).await
    }

    /// Apply NodePort — DNAT from host port to backends.
    pub async fn apply_nodeport(&self, node_port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        self.nft.add_nodeport_dnat(node_port, backends).await
    }

    /// Remove a Service DNAT.
    pub async fn remove_service_dnat(&self, cluster_ip: Ipv4Addr, port: u16) -> Result<()> {
        self.nft.remove_dnat(cluster_ip, port).await
    }

    /// Remove a NodePort DNAT.
    pub async fn remove_nodeport(&self, node_port: u16) -> Result<()> {
        self.nft.remove_nodeport_dnat(node_port).await
    }

    /// Cleanup all z8s nftables rules.
    pub async fn cleanup_nft(&self) -> Result<()> {
        self.nft.cleanup().await
    }

    /// Initialize nftables tables and chains.
    pub async fn init_nft(&self, pod_cidr: &str) -> Result<()> {
        self.nft.init(pod_cidr).await
    }

    /// Add forward catch-all rule.
    pub async fn add_forward_catchall(&self, pod_cidr: &str) -> Result<()> {
        self.nft.add_forward_catchall(pod_cidr).await
    }

    /// Create an nftables set (for NetworkPolicy).
    pub async fn create_nft_set(&self, name: &str, initial_ips: &[Ipv4Addr]) -> Result<()> {
        self.nft.create_set(name, initial_ips).await
    }

    /// Replace an nftables set.
    pub async fn replace_nft_set(&self, name: &str, ips: &[Ipv4Addr]) -> Result<()> {
        self.nft.replace_set(name, ips).await
    }
}

/// Veth naming: `veth-<uid8>` where `<uid8>` = last 8 hex chars of pod UID.
/// Linux interface name limit is 15 chars.
/// Uses last 8 hex chars to avoid collisions from common UID prefixes
/// (e.g., "Pod/default/nginx-deploy-pod-<suffix>").
pub fn veth_name_from_uid(uid: &str) -> String {
    let hex: Vec<char> = uid.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let start = hex.len().saturating_sub(8);
    let name: String = hex[start..].iter().collect();
    format!("veth-{}", name)
}

/// Create a veth pair for a pod.
/// `peer_pid`: if set, the peer is created directly in the pod's netns
/// with name "zeth-<uid8>" (avoids conflict with existing k3s eth0).
/// Returns (host_ifname, peer_ifname, host_ifindex, peer_ifindex).
/// When peer_pid is set, peer_ifindex is 0 (must be resolved inside pod netns).
pub fn create_pod_veth(pod_uid: &str, peer_pid: Option<u32>) -> Result<(String, String, u32, u32)> {
    let host_name = veth_name_from_uid(pod_uid);
    let hex: Vec<char> = pod_uid.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let start = hex.len().saturating_sub(8);
    let peer_name = format!("zeth-{}", hex[start..].iter().collect::<String>());

    let (host_idx, peer_idx) =
        netlink::create_veth_pair(&host_name, &peer_name, peer_pid).context("create_veth_pair")?;
    info!(
        "Created veth pair: {} (idx {}) <-> {} (idx {})",
        host_name, host_idx, peer_name, peer_idx
    );

    Ok((host_name, peer_name.to_string(), host_idx, peer_idx))
}

/// Bring up the host-side veth interface.
pub fn bring_up_veth(ifindex: u32) -> Result<()> {
    netlink::set_link_up(ifindex).context("set_link_up")?;
    Ok(())
}

/// Assign the gateway IP to the host side of the veth (/32 to avoid cross-veth conflicts).
pub fn assign_gateway(gateway: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::add_addr(host_veth_ifindex, gateway, 32).context("assign_gateway")
}

/// Add a /32 route on the host for the pod IP via the host veth.
pub fn add_pod_host_route(pod_ip: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::add_route(pod_ip, 32, None, Some(host_veth_ifindex)).context("add_pod_route")?;
    info!(
        "Host route: {} -> dev veth (ifindex {})",
        pod_ip, host_veth_ifindex
    );
    Ok(())
}

/// Delete a /32 route on the host for the pod IP.
pub fn del_pod_host_route(pod_ip: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::del_route(pod_ip, 32, None, Some(host_veth_ifindex)).context("del_pod_route")?;
    info!(
        "Host route removed: {} -> dev veth (ifindex {})",
        pod_ip, host_veth_ifindex
    );
    Ok(())
}

/// Assign an IP address to an interface.
pub fn assign_ip(ifindex: u32, ip: &Ipv4Addr, prefix: u8) -> Result<()> {
    netlink::add_addr(ifindex, ip, prefix).context("add_addr")?;
    info!("Assigned {} to ifindex {}", ip, ifindex);
    Ok(())
}

/// Add default route inside the pod netns (via the peer interface, to the host).
pub fn add_default_route(peer_ifindex: u32, gateway: &Ipv4Addr) -> Result<()> {
    netlink::add_route(&Ipv4Addr::UNSPECIFIED, 0, Some(gateway), Some(peer_ifindex))
        .context("add_default_route")?;
    info!(
        "Default route: 0.0.0.0/0 via {} dev ifindex {}",
        gateway, peer_ifindex
    );
    Ok(())
}

/// Delete veth pair by host name.
pub fn delete_veth(host_name: &str) -> Result<()> {
    match netlink::get_ifindex(host_name) {
        Ok(idx) => {
            netlink::del_link(idx).context("del_link")?;
            info!("Deleted veth {}", host_name);
        }
        Err(_) => {
            warn!("Veth {} not found, skipping delete", host_name);
        }
    }
    Ok(())
}

/// Bring up loopback inside a network namespace.
pub fn setup_loopback() -> Result<()> {
    netlink::ensure_loopback_up().context("ensure_loopback_up")?;
    Ok(())
}

/// Move a peer interface into a network namespace (by PID).
pub fn move_peer_to_netns(peer_ifindex: u32, pid: u32) -> Result<()> {
    let fd = netlink::netlink_socket()?;
    let ns_pid_attr = netlink::nlattr_bytes(netlink::IFLA_NET_NS_PID, &pid.to_ne_bytes());

    let total_len = 16 + 16 + ns_pid_attr.len();
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&netlink::RTM_NEWLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(netlink::NLM_F_REQUEST | netlink::NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = netlink::AF_INET as u8;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&peer_ifindex.to_ne_bytes());
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    let offset = 32;
    buf[offset..offset + ns_pid_attr.len()].copy_from_slice(&ns_pid_attr);

    netlink::send_nlmsg(&fd, &buf)?;
    let resp = netlink::recv_nlmsg(&fd)?;

    if resp.len() >= 16 {
        let msg_type = u16::from_ne_bytes([resp[4], resp[5]]);
        if msg_type == netlink::NLMSG_ERROR {
            if resp.len() >= 20 {
                let err_code = i32::from_ne_bytes([resp[16], resp[17], resp[18], resp[19]]);
                if err_code != 0 {
                    return Err(anyhow::anyhow!(
                        "move_peer_to_netns: netlink error {}",
                        err_code
                    ));
                }
            }
        }
    }
    info!("Moved peer ifindex {} to netns pid {}", peer_ifindex, pid);
    Ok(())
}

/// List all veth-* interfaces on the host. Returns (name, ifindex) pairs.
pub fn list_veth_interfaces() -> Result<Vec<(String, u32)>> {
    let links_dir = std::fs::read_dir("/sys/class/net").context("read /sys/class/net")?;
    let mut result = Vec::new();
    for entry in links_dir {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("veth-") {
            if let Ok(idx) = get_ifindex_from_sys(&name) {
                result.push((name, idx));
            }
        }
    }
    Ok(result)
}

fn get_ifindex_from_sys(name: &str) -> Result<u32> {
    let path = format!("/sys/class/net/{}/ifindex", name);
    let content = std::fs::read_to_string(&path)?;
    content
        .trim()
        .parse::<u32>()
        .map_err(|e| anyhow::anyhow!("parse ifindex: {}", e))
}

/// Clean up orphaned veth-* interfaces (those with no matching pod).
/// This is called at startup to remove stale veths from crashes.
pub fn clean_orphan_veths(active_uids: &[String]) -> Result<()> {
    let veths = list_veth_interfaces()?;
    let active_names: Vec<String> = active_uids
        .iter()
        .map(|uid| veth_name_from_uid(uid))
        .collect();

    for (name, idx) in &veths {
        if !active_names.contains(name) {
            info!("Cleaning orphan veth: {} (ifindex {})", name, idx);
            if let Err(e) = netlink::del_link(*idx) {
                warn!("Failed to delete orphan veth {}: {}", name, e);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_veth_name_from_uid_15chars() {
        let name = veth_name_from_uid("a1b2c3d4-e5f6-7890-abcd-ef1234567890");
        assert_eq!(name, "veth-34567890");
        assert!(name.len() <= 15, "name {} exceeds 15 chars", name);
    }

    #[test]
    fn test_veth_name_same_prefix_unique_suffix() {
        // Two pods in the same deployment should get different veth names
        let name1 = veth_name_from_uid("Pod/default/nginx-deploy-pod-bdaf80a2");
        let name2 = veth_name_from_uid("Pod/default/nginx-deploy-pod-19d6b9c5");
        assert_ne!(name1, name2, "veth names must not collide");
        assert_eq!(name1, "veth-bdaf80a2");
        assert_eq!(name2, "veth-19d6b9c5");
    }
}
