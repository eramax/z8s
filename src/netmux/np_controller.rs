use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use anyhow::Result;
use tracing::{info, warn};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;

use super::NetMux;

/// A tracked policy set with its associated pod/namespace selector.
struct PolicySet {
    ip_addrs: Vec<Ipv4Addr>,
    pod_selector: Option<LabelSelector>,
    namespace_selector: Option<LabelSelector>,
}

/// NetworkPolicy controller — watches NetworkPolicy + Pod resources,
/// compiles pod selector rules to dynamic nftables sets.
pub struct NetworkPolicyController {
    netmux: Arc<NetMux>,
    /// Tracked sets: "np:<ns>:<name>:<idx>" -> PolicySet
    /// CONCURRENCY: std::sync::Mutex used because lock is held briefly for
    /// HashMap read/write, never across .await points.
    sets: std::sync::Mutex<std::collections::HashMap<String, PolicySet>>,
}

impl NetworkPolicyController {
    /// Create a new NetworkPolicy controller.
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
                    let set_name = format!("np:{}:{}:{}", ns, name, idx);

                    if let Some(ps) = &peer.pod_selector {
                        self.create_policy_set(&set_name, ps)?;
                        // Fix Bug #4: use 0.0.0.0/0 as destination (match all dest IPs)
                        self.netmux.add_forward_allow_set_src(&set_name, "0.0.0.0/0")?;

                        let mut sets = self.sets.lock().expect("lock poisoned");
                        sets.entry(set_name).or_insert_with(|| PolicySet {
                            ip_addrs: Vec::new(),
                            pod_selector: Some(ps.clone()),
                            namespace_selector: None,
                        });
                    }

                    if let Some(ns_sel) = &peer.namespace_selector {
                        // namespaceSelector: allow from pods in matching namespaces
                        // For now, create a set with a placeholder name
                        let ns_set_name = format!("np:ns:{}:{}:{}", ns, name, idx);
                        self.netmux.nft.create_set(&ns_set_name, &[])?;
                        self.netmux.add_forward_allow_set_src(&ns_set_name, "0.0.0.0/0")?;

                        let mut sets = self.sets.lock().expect("lock poisoned");
                        sets.entry(ns_set_name).or_insert_with(|| PolicySet {
                            ip_addrs: Vec::new(),
                            pod_selector: None,
                            namespace_selector: Some(ns_sel.clone()),
                        });
                    }

                    if let Some(ip_block) = &peer.ip_block {
                        // ipBlock: allow/deny by CIDR
                        self.netmux.add_forward_allow(&ip_block.cidr, "0.0.0.0/0")?;
                        for except in ip_block.except.as_deref().unwrap_or(&[]) {
                            self.netmux.add_forward_deny(except, "0.0.0.0/0")?;
                        }
                    }
                }
            }
        }

        info!("NetworkPolicy '{}/{}' applied", ns, name);
        Ok(())
    }

    /// Create an nftables set for a podSelector.
    fn create_policy_set(&self, name: &str, _ps: &LabelSelector) -> Result<()> {
        self.netmux.nft.create_set(name, &[])?;
        Ok(())
    }

    /// Update a pod in all matching NetworkPolicy sets.
    pub fn update_pod(&self, pod_ip: Ipv4Addr, labels: &BTreeMap<String, String>, _ns: &str) -> Result<()> {
        let mut sets = self.sets.lock().expect("lock poisoned");
        for (set_name, policy_set) in sets.iter_mut() {
            // Check if pod labels match this set's selector
            let matches = match &policy_set.pod_selector {
                Some(sel) => labels_match_selector(labels, sel),
                None => true, // namespace-based sets match all pods in ns
            };
            if matches && !policy_set.ip_addrs.contains(&pod_ip) {
                policy_set.ip_addrs.push(pod_ip);
                if let Err(e) = self.netmux.nft.replace_set(set_name, &policy_set.ip_addrs) {
                    warn!("Failed to update set '{}': {}", set_name, e);
                }
            }
        }
        Ok(())
    }

    /// Remove a pod from all matching NetworkPolicy sets.
    pub fn remove_pod(&self, pod_ip: Ipv4Addr) -> Result<()> {
        let mut sets = self.sets.lock().expect("lock poisoned");
        for (set_name, policy_set) in sets.iter_mut() {
            if let Some(pos) = policy_set.ip_addrs.iter().position(|ip| *ip == pod_ip) {
                policy_set.ip_addrs.remove(pos);
                if let Err(e) = self.netmux.nft.replace_set(set_name, &policy_set.ip_addrs) {
                    warn!("Failed to update set '{}': {}", set_name, e);
                }
            }
        }
        Ok(())
    }
}

/// Check if pod labels match a LabelSelector.
fn labels_match_selector(pod_labels: &BTreeMap<String, String>, sel: &LabelSelector) -> bool {
    if let Some(ref match_labels) = sel.match_labels {
        for (k, v) in match_labels {
            if pod_labels.get(k) != Some(v) {
                return false;
            }
        }
    }
    if let Some(ref match_expressions) = sel.match_expressions {
        for expr in match_expressions {
            let pod_val = pod_labels.get(&expr.key);
            let matches = match expr.operator.as_str() {
                "In" => {
                    if let Some(ref values) = expr.values {
                        pod_val.is_some_and(|v| values.contains(v))
                    } else {
                        false
                    }
                }
                "NotIn" => {
                    if let Some(ref values) = expr.values {
                        pod_val.map_or(true, |v| !values.contains(v))
                    } else {
                        true
                    }
                }
                "Exists" => pod_val.is_some(),
                "DoesNotExist" => pod_val.is_none(),
                _ => false,
            };
            if !matches {
                return false;
            }
        }
    }
    true
}
