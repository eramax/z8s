use std::net::Ipv4Addr;
use anyhow::{Context, Result};
use tracing::{info, warn};

use super::netlink;

/// Veth naming: `veth-<uid8>` where `<uid8>` = first 8 hex chars of pod UID.
/// Linux interface name limit is 15 chars.
pub fn veth_name_from_uid(uid: &str) -> String {
    let hex: String = uid.chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect();
    format!("veth-{}", hex)
}

/// Peer side of veth pair (inside pod netns). Capped at 15 chars (Linux IFNAMSIZ).
pub fn veth_peer_name(host_name: &str) -> String {
    let p = format!("{}-e", host_name);
    p[..p.len().min(15)].to_string()
}

/// Create a veth pair for a pod.
/// Returns (host_ifname, peer_ifname, host_ifindex, peer_ifindex).
pub fn create_pod_veth(pod_uid: &str) -> Result<(String, String, u32, u32)> {
    let host_name = veth_name_from_uid(pod_uid);
    let peer_name = veth_peer_name(&host_name);

    let (host_idx, peer_idx) = netlink::create_veth_pair(&host_name, &peer_name, None)
        .context("create_veth_pair")?;
    info!("Created veth pair: {} (idx {}) <-> {} (idx {})", host_name, host_idx, peer_name, peer_idx);

    Ok((host_name, peer_name, host_idx, peer_idx))
}

/// Bring up the host-side veth interface.
pub fn bring_up_veth(ifindex: u32) -> Result<()> {
    netlink::set_link_up(ifindex).context("set_link_up")?;
    Ok(())
}

/// Add a /32 route on the host for the pod IP via the host veth.
pub fn add_pod_host_route(pod_ip: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::add_route(pod_ip, 32, None, Some(host_veth_ifindex))
        .context("add_pod_route")?;
    info!("Host route: {} -> dev veth (ifindex {})", pod_ip, host_veth_ifindex);
    Ok(())
}

/// Delete a /32 route on the host for the pod IP.
pub fn del_pod_host_route(pod_ip: &Ipv4Addr, host_veth_ifindex: u32) -> Result<()> {
    netlink::del_route(pod_ip, 32, None, Some(host_veth_ifindex))
        .context("del_pod_route")?;
    info!("Host route removed: {} -> dev veth (ifindex {})", pod_ip, host_veth_ifindex);
    Ok(())
}

/// Assign an IP address to an interface.
pub fn assign_ip(ifindex: u32, ip: &Ipv4Addr, prefix: u8) -> Result<()> {
    netlink::add_addr(ifindex, ip, prefix).context("add_addr")?;
    info!("Assigned {} to ifindex {}", ip, ifindex);
    Ok(())
}

/// Add default route inside the pod netns (via the peer interface, typically 10.42.x.1).
pub fn add_default_route(peer_ifindex: u32, gateway: &Ipv4Addr) -> Result<()> {
    netlink::add_route(&Ipv4Addr::UNSPECIFIED, 0, Some(gateway), Some(peer_ifindex))
        .context("add_default_route")?;
    info!("Default route: 0.0.0.0/0 via {} dev ifindex {}", gateway, peer_ifindex);
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

/// Delete veth pair by host ifindex.
pub fn delete_veth_by_index(ifindex: u32) -> Result<()> {
    netlink::del_link(ifindex).context("del_link")?;
    info!("Deleted veth ifindex {}", ifindex);
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
    buf[offset..offset+ns_pid_attr.len()].copy_from_slice(&ns_pid_attr);

    netlink::send_nlmsg(fd, &buf)?;
    let resp = netlink::recv_nlmsg(fd)?;
    unsafe { nix::libc::close(fd); }

    if resp.len() >= 16 {
        let msg_type = u16::from_ne_bytes([resp[4], resp[5]]);
        if msg_type == netlink::NLMSG_ERROR {
            if resp.len() >= 20 {
                let err_code = i32::from_ne_bytes([resp[16], resp[17], resp[18], resp[19]]);
                if err_code != 0 {
                    return Err(anyhow::anyhow!("move_peer_to_netns: netlink error {}", err_code));
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
    content.trim().parse::<u32>().map_err(|e| anyhow::anyhow!("parse ifindex: {}", e))
}

/// Clean up orphaned veth-* interfaces (those with no matching pod).
/// This is called at startup to remove stale veths from crashes.
pub fn clean_orphan_veths(active_uids: &[String]) -> Result<()> {
    let veths = list_veth_interfaces()?;
    let active_names: Vec<String> = active_uids.iter().map(|uid| veth_name_from_uid(uid)).collect();

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
