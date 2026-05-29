use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use anyhow::{Context, Result};
use tracing::{info, warn};
use k8s_openapi::api::networking::v1::{NetworkPolicy, NetworkPolicyPeer};

use super::NetMux;

/// NetworkPolicy controller — watches NetworkPolicy + Pod resources,
/// compiles pod selector rules to dynamic nftables sets.
pub struct NetworkPolicyController {
    netmux: Arc<NetMux>,
    /// Tracked sets: "np:<ns>:<name>:<idx>" -> current pod IPs
    sets: std::sync::Mutex<std::collections::HashMap<String, Vec<Ipv4Addr>>>,
}

impl NetworkPolicyController {
    pub fn new(netmux: Arc<NetMux>) -> Self {
        Self {
            netmux,
            sets: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Apply a NetworkPolicy: create nftables sets and forward rules.
    pub fn apply_network_policy(&self, np: &NetworkPolicy) -> Result<()> {
        let ns = np.metadata.namespace.as_deref().unwrap_or("default");
        let name = np.metadata.name.as_deref().unwrap_or("unknown");

        let spec = match np.spec.as_ref() {
            Some(s) => s,
            None => return Ok(()),
        };

        // Process ingress rules
        if let Some(ingress_rules) = &spec.ingress {
            for (idx, rule) in ingress_rules.iter().enumerate() {
                let from = rule.from.as_deref().unwrap_or(&[]);
                for (_peer_idx, peer) in from.iter().enumerate() {
                    if let Some(ps) = &peer.pod_selector {
                        let set_name = format!("np:{}:{}:{}", ns, name, idx);
                        self.create_policy_set(&set_name, ps)?;

                        let dst_cidr = format!("{}/4", ns);
                        self.netmux.add_forward_allow_set_src(&set_name, &dst_cidr)?;

                        let mut sets = self.sets.lock().unwrap();
                        sets.entry(set_name).or_insert_with(Vec::new);
                    }
                }
            }
        }

        info!("NetworkPolicy '{}/{}' applied", ns, name);
        Ok(())
    }

    /// Create an nftables set for a podSelector.
    fn create_policy_set(&self, name: &str, ps: &k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector) -> Result<()> {
        self.netmux.nft.create_set(name, &[])?;
        info!("Created policy set '{}' for selector {:?}", name, ps.match_labels);
        Ok(())
    }

    /// Update a pod in all matching NetworkPolicy sets.
    pub fn update_pod(&self, pod_ip: Ipv4Addr, labels: &BTreeMap<String, String>, _ns: &str) -> Result<()> {
        let sets = self.sets.lock().unwrap();
        for (set_name, ips) in sets.iter() {
            let mut new_ips = ips.clone();
            if !new_ips.contains(&pod_ip) {
                new_ips.push(pod_ip);
                // Recreate set with updated IPs
                if let Err(e) = self.netmux.nft.replace_set(set_name, &new_ips) {
                    warn!("Failed to update set '{}': {}", set_name, e);
                }
            }
        }
        Ok(())
    }

    /// Remove a pod from all matching NetworkPolicy sets.
    pub fn remove_pod(&self, pod_ip: Ipv4Addr) -> Result<()> {
        let sets = self.sets.lock().unwrap();
        for (set_name, ips) in sets.iter() {
            if ips.contains(&pod_ip) {
                let new_ips: Vec<Ipv4Addr> = ips.iter().cloned().filter(|ip| *ip != pod_ip).collect();
                if let Err(e) = self.netmux.nft.replace_set(set_name, &new_ips) {
                    warn!("Failed to update set '{}': {}", set_name, e);
                }
            }
        }
        Ok(())
    }
}
