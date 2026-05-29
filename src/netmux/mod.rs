pub mod pool;
pub mod veth;
pub mod routing;
pub mod netlink;
pub mod nftables;
pub mod crds;
pub mod vnet_controller;

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use tracing::{info, warn};

use pool::{IpPool, Ipv4Cidr};
pub use nftables::NftEngine;

/// Unified network engine — one pool, veth management, host routing, nftables.
pub struct NetMux {
    pool: Mutex<IpPool>,
    prefix: u8,
    gateway: Ipv4Addr,
    pub nft: NftEngine,
}

impl NetMux {
    pub fn new(pod_cidr: &str) -> Result<Self> {
        let cidr = Ipv4Cidr::parse(pod_cidr)
            .context("Invalid pod CIDR")?;
        let gateway = Self::derive_gateway(&cidr)?;
        let nft = NftEngine::new();
        Ok(Self {
            pool: Mutex::new(IpPool::new(cidr.clone())),
            prefix: cidr.prefix,
            gateway,
            nft,
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
        self.pool.lock().unwrap().allocate()
    }

    pub fn release_ip(&self, ip: Ipv4Addr) {
        self.pool.lock().unwrap().release(ip);
    }

    pub fn count_free(&self) -> usize {
        self.pool.lock().unwrap().count_free()
    }

    /// Attach a pod to the network: create veth, assign IP, add host route.
    /// Returns the allocated pod IP, host veth ifindex, and peer veth ifindex.
    pub fn attach_pod(&self, pod_uid: &str) -> Result<(Ipv4Addr, u32, u32)> {
        let pod_ip = self.allocate_ip()
            .context("No IPs available in pod CIDR")?;

        let (host_name, _peer_name, host_idx, peer_idx) =
            veth::create_pod_veth(pod_uid)
                .context("create_pod_veth")?;

        veth::bring_up_veth(host_idx)
            .context("bring_up_veth")?;

        veth::add_pod_host_route(&pod_ip, host_idx)
            .context("add_pod_host_route")?;

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
    pub fn configure_pod_netns(
        &self,
        pod_uid: &str,
        pod_ip: &Ipv4Addr,
        container_pid: u32,
        peer_ifindex: u32,
    ) -> Result<()> {
        veth::move_peer_to_netns(peer_ifindex, container_pid)
            .context("move_peer_to_netns")?;

        // Enter pod netns to assign IP and add default route
        let netns_path = format!("/proc/{}/ns/net", container_pid);
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
        veth::assign_ip(peer_ifindex, pod_ip, self.prefix)
            .context("assign_ip in pod netns")?;

        // Bring up peer inside pod netns
        netlink::set_link_up(peer_ifindex)
            .context("set_link_up peer in pod netns")?;

        // Add default route inside pod netns (via gateway)
        veth::add_default_route(peer_ifindex, &self.gateway)
            .context("add_default_route in pod netns")?;

        // Enter back to host netns
        let host_netns_fd = unsafe {
            nix::fcntl::open(
                "/proc/1/ns/net",
                nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
                nix::sys::stat::Mode::empty(),
            )
        }
        .context("open host netns")?;

        unsafe {
            nix::sched::setns(&host_netns_fd, nix::sched::CloneFlags::CLONE_NEWNET)
                .context("setns back to host netns")?;
        }

        Ok(())
    }

    /// Initialize nftables tables and chains.
    pub fn init_nftables(&self) -> Result<()> {
        self.nft.init()
    }

    /// Add MASQUERADE rule for pod internet access.
    pub fn add_snat(&self, pod_cidr: &str) -> Result<()> {
        self.nft.add_snat(pod_cidr)
    }

    /// Add DNAT rule for ClusterIP.
    pub fn add_dnat(&self, cluster_ip: Ipv4Addr, port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        self.nft.add_dnat(cluster_ip, port, backends)
    }

    /// Add forward allow rule between two CIDRs.
    pub fn add_forward_allow(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        self.nft.add_forward_allow(src_cidr, dst_cidr)
    }

    /// Add forward deny rule between two CIDRs.
    pub fn add_forward_deny(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        self.nft.add_forward_deny(src_cidr, dst_cidr)
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
}
