//! Pure network planner (N1) — builds desired DNAT/service intent from a store snapshot.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::netmux::state::RuleKey;
use crate::store::{AnyResource, StoreSnapshot};
use crate::types::{IntOrString, Service};

/// One ClusterIP or NodePort DNAT intent for this node.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedDnat {
    pub rule_key: RuleKey,
    pub service_ns: String,
    pub service_name: String,
    pub listen_port: u16,
    pub is_nodeport: bool,
    pub node_port: Option<u16>,
    pub cluster_ip: Option<Ipv4Addr>,
    pub target_port: IntOrString,
}

/// Desired network intents (dry-run output of the planner).
#[derive(Debug, Clone, Default)]
pub struct PlannedNetwork {
    pub generation: u64,
    pub dnat_rules: Vec<PlannedDnat>,
    pub local_pod_count: usize,
    pub service_count: usize,
}

pub struct NetworkPlanner {
    pub node_name: String,
}

impl NetworkPlanner {
    pub fn new(node_name: impl Into<String>) -> Self {
        Self {
            node_name: node_name.into(),
        }
    }

    /// Build desired DNAT rules for **this node** only (no IO).
    pub fn plan(&self, snap: &StoreSnapshot) -> PlannedNetwork {
        let mut out = PlannedNetwork {
            generation: 1,
            ..Default::default()
        };

        let local_pods = local_assigned_pods(snap, &self.node_name);
        out.local_pod_count = local_pods.len();

        let services: Vec<&Service> = snap
            .by_kind("Service")
            .into_iter()
            .filter_map(|t| match &t.resource {
                AnyResource::Service(s) => Some(s),
                _ => None,
            })
            .collect();
        out.service_count = services.len();

        for svc in services {
            let Some(spec) = &svc.spec else { continue };
            let selector = spec.selector.as_ref().cloned().unwrap_or_default();
            if selector.is_empty() {
                continue;
            }
            let ns = svc.metadata.namespace.as_deref().unwrap_or("default");
            let name = svc.metadata.name.as_deref().unwrap_or("");
            if !selector_matches_any_local_pod(&selector, ns, &local_pods) {
                continue;
            }

            let svc_type = spec.type_.as_deref().unwrap_or("ClusterIP");
            let cluster_ip = spec
                .cluster_ip
                .as_deref()
                .filter(|c| !c.is_empty() && *c != "None")
                .and_then(|c| c.parse::<Ipv4Addr>().ok());

            for svc_port in spec.ports.as_deref().unwrap_or(&[]) {
                let target_port = svc_port
                    .target_port
                    .clone()
                    .unwrap_or_else(|| IntOrString::Int(svc_port.port));

                if let Some(cip) = cluster_ip {
                    out.dnat_rules.push(PlannedDnat {
                        rule_key: RuleKey::clusterip_dnat(&cip.to_string(), svc_port.port as u16),
                        service_ns: ns.to_string(),
                        service_name: name.to_string(),
                        listen_port: svc_port.port as u16,
                        is_nodeport: false,
                        node_port: None,
                        cluster_ip: Some(cip),
                        target_port: target_port.clone(),
                    });
                }

                if svc_type == "NodePort" || svc_type == "LoadBalancer" {
                    if let Some(np) = svc_port.node_port {
                        out.dnat_rules.push(PlannedDnat {
                            rule_key: RuleKey::nodeport_dnat(np as u16),
                            service_ns: ns.to_string(),
                            service_name: name.to_string(),
                            listen_port: svc_port.port as u16,
                            is_nodeport: true,
                            node_port: Some(np as u16),
                            cluster_ip,
                            target_port,
                        });
                    }
                }
            }
        }

        out
    }
}

fn local_assigned_pods<'a>(
    snap: &'a StoreSnapshot,
    node_name: &str,
) -> Vec<(&'a crate::types::Pod, BTreeMap<String, String>)> {
    snap.by_kind("Pod")
        .into_iter()
        .filter_map(|t| {
            let AnyResource::Pod(pod) = &t.resource else {
                return None;
            };
            if pod.assigned_node.as_deref() != Some(node_name) {
                return None;
            }
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            Some((pod, labels))
        })
        .collect()
}

fn selector_matches_any_local_pod(
    selector: &BTreeMap<String, String>,
    ns: &str,
    local_pods: &[(&crate::types::Pod, BTreeMap<String, String>)],
) -> bool {
    local_pods.iter().any(|(pod, labels)| {
        pod.metadata.namespace.as_deref().unwrap_or("default") == ns
            && selector.iter().all(|(k, v)| labels.get(k) == Some(v))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ResourceTracker;
    use crate::types::{ObjectMeta, Pod, PodSpec, ServicePort, ServiceSpec};

    fn pod(name: &str, ns: &str, node: &str, app: &str) -> ResourceTracker {
        let mut labels = BTreeMap::new();
        labels.insert("app".into(), app.into());
        ResourceTracker::new(AnyResource::Pod(Pod {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some(ns.into()),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(PodSpec::default()),
            assigned_node: Some(node.into()),
            ..Default::default()
        }))
    }

    fn svc_clusterip(name: &str, ns: &str, app: &str, ip: &str, port: i32) -> ResourceTracker {
        let mut selector = BTreeMap::new();
        selector.insert("app".into(), app.into());
        ResourceTracker::new(AnyResource::Service(crate::types::Service {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some(ns.into()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                selector: Some(selector),
                cluster_ip: Some(ip.into()),
                type_: Some("ClusterIP".into()),
                ports: Some(vec![ServicePort {
                    port,
                    target_port: Some(IntOrString::Int(port)),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    #[test]
    fn planner_emits_clusterip_for_local_pod() {
        let snap = StoreSnapshot::from_trackers(vec![
            pod("p1", "default", "node-a", "web"),
            svc_clusterip("web", "default", "web", "10.96.0.10", 80i32),
        ]);
        let plan = NetworkPlanner::new("node-a").plan(&snap);
        assert_eq!(plan.local_pod_count, 1);
        assert_eq!(plan.dnat_rules.len(), 1);
        assert!(!plan.dnat_rules[0].is_nodeport);
        assert_eq!(
            plan.dnat_rules[0].cluster_ip,
            Some("10.96.0.10".parse().unwrap())
        );
    }

    #[test]
    fn planner_skips_service_without_local_backend() {
        let snap = StoreSnapshot::from_trackers(vec![
            pod("p1", "default", "node-b", "web"),
            svc_clusterip("web", "default", "web", "10.96.0.10", 80i32),
        ]);
        let plan = NetworkPlanner::new("node-a").plan(&snap);
        assert!(plan.dnat_rules.is_empty());
    }
}
