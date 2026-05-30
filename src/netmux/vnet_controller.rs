use std::sync::Arc;
use anyhow::Result;
use tracing::{info, warn};

use super::NetMux;
use super::crds::{Nsg, VNet};

/// VNet controller — watches VNet/Subnet/NSG CRDs and compiles to nftables.
pub struct VNetController {
    netmux: Arc<NetMux>,
}

impl VNetController {
    /// Create a new VNet controller.
    pub fn new(netmux: Arc<NetMux>) -> Self {
        Self { netmux }
    }

    /// Apply an NSG: compile its rules to nftables forward chain rules.
    pub fn apply_nsg(&self, nsg: &Nsg) -> Result<()> {
        let mut sorted_rules = nsg.spec.rules.clone();
        sorted_rules.sort_by_key(|r| r.priority);

        for rule in &sorted_rules {
            match rule.action.as_str() {
                "deny" => {
                    for src in &rule.src_cidrs {
                        for dst in &rule.dst_cidrs {
                            self.netmux.add_forward_deny(src, dst)?;
                        }
                    }
                }
                "allow" => {
                    for src in &rule.src_cidrs {
                        for dst in &rule.dst_cidrs {
                            self.netmux.add_forward_allow(src, dst)?;
                        }
                    }
                }
                other => {
                    warn!("NSG rule '{}' has unknown action '{}', skipping", rule.name, other);
                }
            }
            let nsg_name = nsg.metadata.name.as_deref().unwrap_or("unknown");
            info!("NSG '{}': applied rule '{}' {} -> {} ({})",
                nsg_name, rule.name,
                rule.src_cidrs.join(","), rule.dst_cidrs.join(","),
                rule.action);
        }
        Ok(())
    }

    /// Apply a VNet: ensure CIDR is allocated and nftables rules are set.
    pub fn apply_vnet(&self, vnet: &VNet, cidr: &str) -> Result<()> {
            let vnet_name = vnet.metadata.name.as_deref().unwrap_or("unknown");
            if !vnet.spec.internet_access {
                self.netmux.add_forward_deny(cidr, "0.0.0.0/0")?;
                info!("VNet '{}': internet access denied (spoke)", vnet_name);
            } else {
                info!("VNet '{}': internet access allowed (hub), CIDR {}", vnet_name, cidr);
            }
        Ok(())
    }

}
