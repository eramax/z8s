use std::net::Ipv4Addr;
use anyhow::{Context, Result};
use tracing::info;

use super::netlink;

/// Add a /32 host route via a specific interface.
pub fn add_host_route(pod_ip: &Ipv4Addr, ifindex: u32) -> Result<()> {
    netlink::add_route(pod_ip, 32, None, Some(ifindex))
        .context("add_host_route")?;
    Ok(())
}

/// Delete a /32 host route.
pub fn del_host_route(pod_ip: &Ipv4Addr, ifindex: u32) -> Result<()> {
    netlink::del_route(pod_ip, 32, None, Some(ifindex))
        .context("del_host_route")?;
    Ok(())
}

/// Add a route for a subnet via a gateway (cross-node routing).
pub fn add_subnet_route(subnet: &Ipv4Addr, prefix: u8, gateway: &Ipv4Addr) -> Result<()> {
    netlink::add_route(subnet, prefix, Some(gateway), None)
        .context("add_subnet_route")?;
    info!("Subnet route: {}/{} via {}", subnet, prefix, gateway);
    Ok(())
}

/// Add default route via gateway and interface.
pub fn add_default_route(gateway: &Ipv4Addr, ifindex: u32) -> Result<()> {
    netlink::add_route(&Ipv4Addr::UNSPECIFIED, 0, Some(gateway), Some(ifindex))
        .context("add_default_route")?;
    Ok(())
}

/// Delete default route via gateway and interface.
pub fn del_default_route(gateway: &Ipv4Addr, ifindex: u32) -> Result<()> {
    netlink::del_route(&Ipv4Addr::UNSPECIFIED, 0, Some(gateway), Some(ifindex))
        .context("del_default_route")?;
    Ok(())
}

/// Add default route for pod inside its netns (via gateway).
pub fn add_pod_default_route(peer_ifindex: u32, gateway: &Ipv4Addr) -> Result<()> {
    netlink::add_route(&Ipv4Addr::UNSPECIFIED, 0, Some(gateway), Some(peer_ifindex))
        .context("add_pod_default_route")?;
    info!("Pod default route: via {} (ifindex {})", gateway, peer_ifindex);
    Ok(())
}
