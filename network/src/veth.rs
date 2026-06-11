//! # Veth Pair Management
//!
//! Creates and manages veth pairs for connecting pods to the host network.
//! Each pod gets one veth pair: the host side stays in the host netns, the
//! peer side moves into the pod's netns (or is created there directly).
//!
//! ## Naming Convention
//!
//! Linux interface names are limited to 15 characters. We use `veth-<uid8>`
//! for the host side and `zeth-<uid8>` for the peer side, where `<uid8>` is
//! the last 8 hex digits of the pod UID. This gives us:
//! - Deterministic naming (we can find a pod's veth from its UID)
//! - 13 chars total, well under the 15-char limit
//! - Last 8 hex digits avoid collisions for UIDs with common prefixes
//!
//! ## Setup Sequence
//!
//! ```text
//! 1. create_veth_pair(host=veth-XXXX, peer=zeth-XXXX, peer_pid=pod_pid)
//! 2. set_link_up(host_ifindex)
//! 3. add_addr(host_ifindex, gateway/32)        — gateway on host veth
//! 4. add_route(pod_ip/32, dev=host_ifindex)    — host route to pod
//! 5. (in pod netns) add_addr(peer_ifindex, pod_ip/32)
//! 6. (in pod netns) set_link_up(peer_ifindex)
//! 7. (in pod netns) add_route(0.0.0.0/0 via gateway, dev=peer_ifindex)
//! ```

use anyhow::{Context, Result};
use std::net::Ipv4Addr;
use tracing::{info, warn};

use crate::netlink;

/// Veth naming: `veth-<uid8>` where `<uid8>` = last 8 hex chars of pod UID.
/// Linux interface name limit is 15 chars; `veth-XXXXXXXX` = 13 chars.
pub fn veth_name_from_uid(uid: &str) -> String {
    let hex: Vec<char> = uid.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let start = hex.len().saturating_sub(8);
    let name: String = hex[start..].iter().collect();
    format!("veth-{}", name)
}

/// Peer veth naming: `zeth-<uid8>` to avoid colliding with the pod's eth0.
pub fn peer_name_from_uid(uid: &str) -> String {
    let hex: Vec<char> = uid.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let start = hex.len().saturating_sub(8);
    let name: String = hex[start..].iter().collect();
    format!("zeth-{}", name)
}

/// Create a veth pair for a pod.
///
/// `peer_pid`: if Some, peer is created directly in that PID's netns.
///   In this case `peer_ifindex` is 0 (must be resolved inside pod netns).
///
/// Returns (host_ifname, peer_ifname, host_ifindex, peer_ifindex).
pub fn create_pod_veth(
    pod_uid: &str,
    peer_pid: Option<u32>,
) -> Result<(String, String, u32, u32)> {
    let host_name = veth_name_from_uid(pod_uid);
    let peer_name = peer_name_from_uid(pod_uid);

    let (host_idx, peer_idx) = netlink::create_veth_pair(&host_name, &peer_name, peer_pid)
        .context("create_veth_pair")?;
    info!(
        "Created veth pair: {} (idx {}) <-> {} (idx {})",
        host_name, host_idx, peer_name, peer_idx
    );
    Ok((host_name, peer_name, host_idx, peer_idx))
}

/// Bring up the host-side veth interface.
pub fn bring_up_veth(ifindex: u32) -> Result<()> {
    netlink::set_link_up(ifindex).context("set_link_up")
}

/// Assign the gateway IP to the host side of the veth (/32 to avoid cross-veth conflicts).
pub fn assign_gateway(gateway: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::add_addr(host_veth_ifindex, gateway, 32).context("assign_gateway")
}

/// Add a /32 host route for the pod IP via the host veth.
pub fn add_pod_host_route(pod_ip: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::add_route(pod_ip, 32, None, Some(host_veth_ifindex)).context("add_pod_route")?;
    info!("Host route: {} -> dev veth (ifindex {})", pod_ip, host_veth_ifindex);
    Ok(())
}

/// Delete a /32 host route for the pod IP.
pub fn del_pod_host_route(pod_ip: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::del_route(pod_ip, 32, None, Some(host_veth_ifindex)).context("del_pod_route")?;
    info!("Host route removed: {} -> dev veth (ifindex {})", pod_ip, host_veth_ifindex);
    Ok(())
}

/// Assign an IP to an interface (typically used in the pod's netns).
pub fn assign_ip(ifindex: u32, ip: &Ipv4Addr, prefix: u8) -> Result<()> {
    netlink::add_addr(ifindex, ip, prefix).context("add_addr")?;
    info!("Assigned {} to ifindex {}", ip, ifindex);
    Ok(())
}

/// Add default route via the gateway inside the pod's netns.
pub fn add_default_route(peer_ifindex: u32, gateway: &Ipv4Addr) -> Result<()> {
    netlink::add_route(
        &Ipv4Addr::UNSPECIFIED,
        0,
        Some(gateway),
        Some(peer_ifindex),
    )
    .context("add_default_route")?;
    info!(
        "Default route: 0.0.0.0/0 via {} dev ifindex {}",
        gateway, peer_ifindex
    );
    Ok(())
}

/// Delete a veth pair by host name. Silently ignores missing interfaces.
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

/// List all `veth-*` interfaces on the host. Returns (name, ifindex) pairs.
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

// ── Netns helpers ───────────────────────────────────────────────────────

/// Resolve the zeth-* ifindex by name, assuming the caller is already inside
/// the pod's netns. Must be called from a context where the current thread is
/// in the target netns.
pub fn resolve_zeth_ifindex_in_pod_netns(pod_uid: &str) -> Result<u32> {
    let peer_name = peer_name_from_uid(pod_uid);
    netlink::get_ifindex(&peer_name)
}

/// Open a pod's network namespace by PID. Caller is responsible for setns-ing into it.
pub fn open_pod_netns(pid: u32) -> Result<std::os::fd::OwnedFd> {
    let path = format!("/proc/{}/ns/net", pid);
    netlink::open_netns(&path).with_context(|| format!("open pod netns for pid {}", pid))
}

/// Open the host network namespace (pid 1). Used for restoring context.
pub fn open_host_netns() -> Result<std::os::fd::OwnedFd> {
    netlink::open_netns("/proc/1/ns/net").context("open host netns")
}

/// Drop guard that ensures we return to the host netns when the scope exits.
pub struct NetNsGuard {
    host_fd: Option<std::os::fd::OwnedFd>,
}

impl NetNsGuard {
    /// Create a new guard. Captures a fd to the host netns.
    pub fn new() -> Result<Self> {
        let host_fd = open_host_netns()?;
        Ok(Self {
            host_fd: Some(host_fd),
        })
    }
}

impl Drop for NetNsGuard {
    fn drop(&mut self) {
        if let Some(ref fd) = self.host_fd {
            // Best-effort restore to host netns
            let _ = z8s_core::syscall::setns(
                fd,
                rustix::thread::LinkNameSpaceType::Network,
            );
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn veth_name_15char_limit() {
        let name = veth_name_from_uid("a1b2c3d4-e5f6-7890-abcd-ef1234567890");
        assert_eq!(name, "veth-34567890");
        assert!(name.len() <= 15);
    }

    #[test]
    fn veth_name_uniqueness() {
        let name1 = veth_name_from_uid("Pod/default/nginx-deploy-pod-bdaf80a2");
        let name2 = veth_name_from_uid("Pod/default/nginx-deploy-pod-19d6b9c5");
        assert_ne!(name1, name2);
        assert_eq!(name1, "veth-bdaf80a2");
        assert_eq!(name2, "veth-19d6b9c5");
    }

    #[test]
    fn peer_name_format() {
        let name = peer_name_from_uid("Pod/default/nginx-bdaf80a2");
        assert_eq!(name, "zeth-bdaf80a2");
    }

    #[test]
    fn veth_name_handles_short_uids() {
        // UIDs shorter than 8 hex chars
        let name = veth_name_from_uid("abc");
        assert_eq!(name, "veth-abc");
    }
}
