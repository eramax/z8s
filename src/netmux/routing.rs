use std::net::Ipv4Addr;
use anyhow::{Context, Result};
use tracing::info;

use super::netlink;

/// Add a route for a subnet via a gateway (cross-node routing).
pub fn add_subnet_route(subnet: &Ipv4Addr, prefix: u8, gateway: &Ipv4Addr) -> Result<()> {
    netlink::add_route(subnet, prefix, Some(gateway), None)
        .context("add_subnet_route")?;
    info!("Subnet route: {}/{} via {}", subnet, prefix, gateway);
    Ok(())
}
