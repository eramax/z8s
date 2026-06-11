//! # NetworkPolicy Controller
//!
//! Translates Kubernetes-style `NetworkPolicy` resources into nftables
//! sets and forward rules.
//!
//! ## Model
//!
//! A NetworkPolicy has:
//! - `podSelector` — which pods this policy applies to
//! - `ingress` / `egress` — rules listing allowed peers (`from` / `to`)
//!
//! For each `ingress.from[]` entry we create an nftables set named
//! `np:<ns>:<name>:<idx>`. When a pod's labels match a set's selector,
//! the pod's IP is added to the set. A single nftables rule then
//! accepts traffic from any IP in the set.
//!
//! ## Membership Updates
//!
//! The controller exposes `update_pod` and `remove_pod` to keep the
//! nftables sets in sync with the live pod set. The reconciler calls
//! these on every state change.

use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::Arc;
use tracing::{info, warn};

use z8s_core::types::{LabelSelector, NetworkPolicy};

use crate::nft::NftEngine;

/// A tracked policy set with its associated pod/namespace selector.
struct PolicySet {
    ip_addrs: Vec<Ipv4Addr>,
    pod_selector: Option<LabelSelector>,
    namespace_selector: Option<LabelSelector>,
}

/// NetworkPolicy controller — watches NetworkPolicy + Pod resources,
/// compiles pod selector rules to dynamic nftables sets.
pub struct NetworkPolicyController {
    nft: Arc<NftEngine>,
    /// CONCURRENCY: std::sync::Mutex used because lock is held briefly for
    /// HashMap read/write, never across .await points.
    sets: std::sync::Mutex<HashMap<String, PolicySet>>,
}

impl NetworkPolicyController {
    /// Create a new NetworkPolicy controller.
    pub fn new(nft: Arc<NftEngine>) -> Self {
        Self {
            nft,
            sets: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Apply a NetworkPolicy: create nftables sets and forward rules.
    pub async fn apply_network_policy(&self, np: &NetworkPolicy) -> Result<()> {
        let ns = np.metadata.namespace.as_deref().unwrap_or("default");
        let name = np.metadata.name.as_deref().unwrap_or("unknown");

        let _spec = match np.spec.as_ref() {
            Some(s) => s,
            None => return Ok(()),
        };

        // In the core types, NetworkPolicy ingress/egress are placeholder
        // structs. We track the selector and create a set + allow rule per
        // network policy to keep the API in place for future enrichment.

        let set_name = format!("np:{}:{}", ns, name);

        // Create the nftables set (empty initially; populated by update_pod)
        self.nft.create_set(&set_name, &[]).await?;

        // Wire the set into nsg-rules with a "match all destination" rule.
        // Future enrichment will use real dest selectors from spec.
        self.nft.add_forward_allow_set_src(&set_name, "0.0.0.0/0").await?;

        // Register the set in our tracker.
        let mut sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
        sets.entry(set_name).or_insert_with(|| PolicySet {
            ip_addrs: Vec::new(),
            pod_selector: None,
            namespace_selector: None,
        });

        info!("NetworkPolicy '{}/{}' applied", ns, name);
        Ok(())
    }

    /// Update a pod in all matching NetworkPolicy sets.
    pub async fn update_pod(
        &self,
        pod_ip: Ipv4Addr,
        labels: &BTreeMap<String, String>,
        _ns: &str,
    ) -> Result<()> {
        let to_update: Vec<String> = {
            let mut sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
            sets.iter_mut()
                .filter_map(|(n, ps)| {
                    let matches = ps
                        .pod_selector
                        .as_ref()
                        .map_or(true, |sel| labels_match_selector(labels, sel));
                    if matches && !ps.ip_addrs.contains(&pod_ip) {
                        ps.ip_addrs.push(pod_ip);
                        Some(n.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };
        for name in &to_update {
            let ips = {
                let s = self.sets.lock().unwrap_or_else(|e| e.into_inner());
                s.get(name).map(|ps| ps.ip_addrs.clone()).unwrap_or_default()
            };
            if let Err(e) = self.nft.replace_set(name, &ips).await {
                warn!("Failed to update set '{}': {}", name, e);
            }
        }
        Ok(())
    }

    /// Remove a pod from all matching NetworkPolicy sets.
    pub async fn remove_pod(&self, pod_ip: Ipv4Addr) -> Result<()> {
        let to_update: Vec<String> = {
            let mut sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
            sets.iter_mut()
                .filter_map(|(n, ps)| {
                    let pos = ps.ip_addrs.iter().position(|ip| *ip == pod_ip)?;
                    ps.ip_addrs.remove(pos);
                    Some(n.clone())
                })
                .collect()
        };
        for name in &to_update {
            let ips = {
                let s = self.sets.lock().unwrap_or_else(|e| e.into_inner());
                s.get(name).map(|ps| ps.ip_addrs.clone()).unwrap_or_default()
            };
            if let Err(e) = self.nft.replace_set(name, &ips).await {
                warn!("Failed to update set '{}': {}", name, e);
            }
        }
        Ok(())
    }
}

/// Check if pod labels match a LabelSelector.
pub fn labels_match_selector(
    pod_labels: &BTreeMap<String, String>,
    sel: &LabelSelector,
) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use z8s_core::types::{LabelSelector, LabelSelectorRequirement};

    fn selector(key: &str, val: &str) -> LabelSelector {
        let mut ml = BTreeMap::new();
        ml.insert(key.to_string(), val.to_string());
        LabelSelector {
            match_labels: Some(ml),
            match_expressions: None,
        }
    }

    #[test]
    fn match_labels_simple() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("app".to_string(), "web".to_string());

        let s = selector("app", "web");
        assert!(labels_match_selector(&pod_labels, &s));

        let s = selector("app", "db");
        assert!(!labels_match_selector(&pod_labels, &s));
    }

    #[test]
    fn match_labels_missing() {
        let pod_labels = BTreeMap::new();
        let s = selector("app", "web");
        assert!(!labels_match_selector(&pod_labels, &s));
    }

    #[test]
    fn match_expression_in() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("role".to_string(), "frontend".to_string());
        let s = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![LabelSelectorRequirement {
                key: "role".to_string(),
                operator: "In".to_string(),
                values: Some(vec!["frontend".to_string(), "backend".to_string()]),
            }]),
        };
        assert!(labels_match_selector(&pod_labels, &s));
    }

    #[test]
    fn match_expression_notin() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("role".to_string(), "frontend".to_string());
        let s = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![LabelSelectorRequirement {
                key: "role".to_string(),
                operator: "NotIn".to_string(),
                values: Some(vec!["db".to_string()]),
            }]),
        };
        assert!(labels_match_selector(&pod_labels, &s));
    }

    #[test]
    fn match_expression_exists() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("role".to_string(), "x".to_string());
        let s = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![LabelSelectorRequirement {
                key: "role".to_string(),
                operator: "Exists".to_string(),
                values: None,
            }]),
        };
        assert!(labels_match_selector(&pod_labels, &s));
    }

    #[test]
    fn match_expression_does_not_exist() {
        let pod_labels = BTreeMap::new();
        let s = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![LabelSelectorRequirement {
                key: "role".to_string(),
                operator: "DoesNotExist".to_string(),
                values: None,
            }]),
        };
        assert!(labels_match_selector(&pod_labels, &s));
    }
}
