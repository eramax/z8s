pub mod pool;
pub mod veth;
pub mod routing;
pub mod netlink;
pub mod nftables;
pub mod crds;
pub mod vnet_controller;
pub mod np_controller;
pub mod ingress;
pub mod cluster;
pub mod dns;
pub mod network;

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use tracing::{info, warn};

pub use pool::Ipv4Cidr;
use pool::IpPool;
pub use nftables::NftEngine;

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
        }.context("open host netns for guard")?;
        Ok(Self { host_fd: Some(host_fd) })
    }

    #[allow(dead_code)]
    fn disarm(&mut self) {
        self.host_fd = None;
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
pub struct NetMux {
    // CONCURRENCY: std::sync::Mutex used for brief synchronous access only.
    // Lock held only during allocate/release, never across .await points.
    pool: Mutex<IpPool>,
    prefix: u8,
    gateway: Ipv4Addr,
    pub nft: NftEngine,
    pub ingress_state: Arc<crate::netmux::ingress::IngressState>,
}

impl NetMux {
    pub fn new(pod_cidr: &str) -> Result<Self> {
        let cidr = Ipv4Cidr::parse(pod_cidr)
            .context("Invalid pod CIDR")?;
        let gateway = Self::derive_gateway(&cidr)?;
        let nft = NftEngine::new();
        let ingress_state = Arc::new(crate::netmux::ingress::IngressState::new());
        Ok(Self {
            pool: Mutex::new(IpPool::new(cidr.clone())),
            prefix: cidr.prefix,
            gateway,
            nft,
            ingress_state,
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
        self.pool.lock().expect("lock poisoned").allocate()
    }

    pub fn release_ip(&self, ip: Ipv4Addr) {
        self.pool.lock().expect("lock poisoned").release(ip);
    }

    pub fn count_free(&self) -> usize {
        self.pool.lock().expect("lock poisoned").count_free()
    }

    pub fn allocate_subnet(&self, prefix: u8) -> Option<Ipv4Cidr> {
        self.pool.lock().expect("lock poisoned").allocate_subnet(prefix)
    }

    /// Attach a pod to the network: allocate IP, create veth pair, add host route.
    /// If `container_pid` is provided, the veth peer is created directly in the
    /// pod's network namespace with name "eth0".
    /// Returns the allocated pod IP, host veth ifindex, and peer veth ifindex.
    pub fn attach_pod(&self, pod_uid: &str, container_pid: Option<u32>) -> Result<(Ipv4Addr, u32, u32)> {
        let pod_ip = self.allocate_ip()
            .context("No IPs available in pod CIDR")?;

        let (host_name, _peer_name, host_idx, peer_idx) =
            veth::create_pod_veth(pod_uid, container_pid)
                .context("create_pod_veth")?;

        if let Err(e) = veth::bring_up_veth(host_idx) {
            self.rollback_veth(pod_uid, &pod_ip, host_idx);
            return Err(e).context("bring_up_veth");
        }

        if let Err(e) = veth::add_pod_host_route(&pod_ip, host_idx) {
            self.rollback_veth(pod_uid, &pod_ip, host_idx);
            return Err(e).context("add_pod_host_route");
        }

        // Assign gateway IP to host veth (/32 avoids conflict when multiple veths exist)
        if let Err(e) = veth::assign_gateway(&self.gateway, host_idx) {
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
        let host_name = veth::veth_name_from_uid(pod_uid);

        if let Err(e) = veth::del_pod_host_route(pod_ip, host_ifindex) {
            warn!("Failed to delete host route for {}: {}", pod_ip, e);
        }

        if let Err(e) = veth::delete_veth(&host_name) {
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
            veth::move_peer_to_netns(peer_ifindex, container_pid)
                .context("move_peer_to_netns")?;
        }

        // When peer was created via IFLA_NET_NS_PID, its ifindex is 0.
        // Resolve it by looking up the zeth-* name inside the pod netns.
        let target_ifindex = if peer_ifindex == 0 {
            let fd = unsafe {
                nix::fcntl::open(
                    netns_path.as_str(),
                    nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                    nix::sys::stat::Mode::empty(),
                )
            }.context("open pod netns")?;
            unsafe { nix::sched::setns(&fd, nix::sched::CloneFlags::CLONE_NEWNET)
                .context("setns into pod netns")?; }
            let hex: Vec<char> = pod_uid.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            let start = hex.len().saturating_sub(8);
            let in_pod_name = format!("zeth-{}", hex[start..].iter().collect::<String>());
            let idx = netlink::get_ifindex(&in_pod_name)
                .context("get_ifindex zeth-* in pod netns")?;
            unsafe {
                let host_fd = nix::fcntl::open(
                    "/proc/1/ns/net",
                    nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                    nix::sys::stat::Mode::empty(),
                ).context("open host netns")?;
                nix::sched::setns(&host_fd, nix::sched::CloneFlags::CLONE_NEWNET)
                    .context("setns back to host")?;
            }
            idx
        } else {
            peer_ifindex
        };

        // Enter pod netns to assign IP and configure networking.
        // NetNsGuard ensures we always return to the host netns on scope exit.
        let _guard = NetNsGuard::new()?;
        let netns_fd = unsafe {
            nix::fcntl::open(
                netns_path.as_str(),
                nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                nix::sys::stat::Mode::empty(),
            )
        }.context("open pod netns")?;

        unsafe {
            nix::sched::setns(&netns_fd, nix::sched::CloneFlags::CLONE_NEWNET)
                .context("setns into pod netns")?;
        }

        // Assign IP to peer inside the pod's netns
        veth::assign_ip(target_ifindex, pod_ip, 32)
            .context("assign_ip in pod netns")?;

        // Bring up peer inside pod netns
        netlink::set_link_up(target_ifindex)
            .context("set_link_up peer in pod netns")?;

        // Add default route inside pod netns (via gateway)
        veth::add_default_route(target_ifindex, &self.gateway)
            .context("add_default_route in pod netns")?;

        // Guard will restore host netns on drop
        Ok(())
    }

    /// Initialize nftables tables and chains.
    pub fn init_nftables(&self, pod_cidr: &str) -> Result<()> {
        self.nft.init(pod_cidr)
    }

    pub fn add_forward_catchall(&self, pod_cidr: &str) -> Result<()> {
        self.nft.add_forward_catchall(pod_cidr)
    }

    /// Add MASQUERADE rule for pod internet access (per-VNet).
    pub fn add_snat(&self, vnet_name: &str, vnet_cidr: &str) -> Result<()> {
        self.nft.add_snat(vnet_name, vnet_cidr)
    }

    /// Remove MASQUERADE rule for a VNet.
    pub fn remove_snat(&self, vnet_cidr: &str) -> Result<()> {
        self.nft.remove_snat(vnet_cidr)
    }

    /// Add DNAT rule for ClusterIP.
    pub fn add_dnat(&self, cluster_ip: Ipv4Addr, port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        self.nft.add_dnat(cluster_ip, port, backends)
    }

    /// Remove DNAT chain for a ClusterIP.
    pub fn remove_dnat(&self, cluster_ip: Ipv4Addr, port: u16) -> Result<()> {
        self.nft.remove_dnat(cluster_ip, port)
    }

    /// Add NodePort DNAT rule (matches on tcp dport, any dest IP).
    pub fn add_nodeport_dnat(&self, node_port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        self.nft.add_nodeport_dnat(node_port, backends)
    }

    pub fn remove_nodeport_dnat(&self, node_port: u16) -> Result<()> {
        self.nft.remove_nodeport_dnat(node_port)
    }

    /// Add forward allow rule between two CIDRs.
    pub fn add_forward_allow(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        self.nft.add_forward_allow(src_cidr, dst_cidr)
    }

    /// Add forward allow rule matching src IP from a named set.
    pub fn add_forward_allow_set_src(&self, set_name: &str, dst_cidr: &str) -> Result<()> {
        self.nft.add_forward_allow_set_src(set_name, dst_cidr)
    }

    /// Add forward deny rule between two CIDRs.
    pub fn add_forward_deny(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        self.nft.add_forward_deny(src_cidr, dst_cidr)
    }

    /// Add a subnet route via a gateway (for cross-node routing).
    pub fn add_subnet_route_raw(&self, dest_cidr: &str, gateway: &str) -> Result<()> {
        let (ip, prefix) = parse_cidr(dest_cidr)?;
        let gw: Ipv4Addr = gateway.parse().context("Invalid gateway IP")?;
        let route = routing::add_subnet_route(&ip, prefix, &gw);
        route.context("add_subnet_route_raw")
    }

    /// Clean up orphaned veths at startup.
    pub fn clean_orphan_veths(&self, active_uids: &[String]) -> Result<()> {
        veth::clean_orphan_veths(active_uids)
    }

    /// Enable ip_forward on the host.
    pub fn enable_ip_forward() -> Result<()> {
        netlink::enable_ip_forward()
    }

    /// Ensure loopback is up.
    pub fn ensure_loopback_up() -> Result<()> {
        netlink::ensure_loopback_up()
    }

    /// Rollback partially-created veth resources on failure.
    fn rollback_veth(&self, pod_uid: &str, pod_ip: &Ipv4Addr, host_ifindex: u32) {
        let host_name = veth::veth_name_from_uid(pod_uid);
        veth::del_pod_host_route(pod_ip, host_ifindex).ok();
        veth::delete_veth(&host_name).ok();
        self.release_ip(*pod_ip);
        warn!("Rolled back veth for {}", pod_uid);
    }
}

fn parse_cidr(s: &str) -> Result<(Ipv4Addr, u8)> {
    let (ip_str, prefix_str) = s.split_once('/').context("Missing '/' in CIDR")?;
    let prefix: u8 = prefix_str.parse().context("Invalid prefix")?;
    let ip: Ipv4Addr = ip_str.parse().context("Invalid IP")?;
    Ok((ip, prefix))
}
