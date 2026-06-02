//! Pure network planner (N1+) — desired state from store snapshot (no IO).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::netmux::state::RuleKey;
use crate::netmux::{NftAction, NftRule};
use crate::store::{AnyResource, StoreSnapshot};
use crate::types::{IntOrString, NsgRule, Service};

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

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedDnsRecord {
    pub hostname: String,
    pub ip: Ipv4Addr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedNetworkPolicy {
    pub namespace: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedPodRoute {
    pub pod_ip: Ipv4Addr,
    pub via: Ipv4Addr,
    pub remote_node: String,
}

/// Desired network intents (dry-run output of the planner).
#[derive(Debug, Clone, Default)]
pub struct PlannedNetwork {
    pub generation: u64,
    pub dnat_rules: Vec<PlannedDnat>,
    pub dns_records: Vec<PlannedDnsRecord>,
    pub nsg_rules: Vec<NftRule>,
    pub network_policies: Vec<PlannedNetworkPolicy>,
    pub remote_routes: Vec<PlannedPodRoute>,
    pub local_pod_count: usize,
    pub service_count: usize,
}

pub struct NetworkPlanner {
    pub node_name: String,
    pub cluster_domain: String,
    pub gateway: Ipv4Addr,
    pub peers: Vec<(String, String)>,
}

impl NetworkPlanner {
    pub fn new(node_name: impl Into<String>) -> Self {
        let cfg = crate::config::get();
        Self {
            node_name: node_name.into(),
            cluster_domain: cfg.cluster_domain.clone(),
            gateway: Ipv4Addr::new(10, 42, 0, 1),
            peers: cfg.peers.clone(),
        }
    }

    pub fn with_gateway(mut self, gw: Ipv4Addr) -> Self {
        self.gateway = gw;
        self
    }

    /// Build desired state for **this node** only (no IO).
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

        plan_services(&mut out, &services, &local_pods);
        plan_dns_from_services(&mut out, &services, &self.cluster_domain);
        plan_dns_from_ingress(&mut out, snap, self.gateway);
        out.nsg_rules = plan_nsg_rules(snap);
        out.network_policies = plan_network_policies(snap);
        out.remote_routes = plan_remote_pod_routes(snap, &self.node_name, &self.peers);

        out
    }
}

fn plan_services(
    out: &mut PlannedNetwork,
    services: &[&Service],
    local_pods: &[(&crate::types::Pod, BTreeMap<String, String>)],
) {
    for svc in services {
        let Some(spec) = &svc.spec else { continue };
        let selector = spec.selector.as_ref().cloned().unwrap_or_default();
        if selector.is_empty() {
            continue;
        }
        let ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let name = svc.metadata.name.as_deref().unwrap_or("");
        if !selector_matches_any_local_pod(&selector, ns, local_pods) {
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
}

fn plan_dns_from_services(out: &mut PlannedNetwork, services: &[&Service], cluster_domain: &str) {
    for svc in services {
        let Some(spec) = &svc.spec else { continue };
        if spec.type_.as_deref() == Some("ExternalName") {
            continue;
        }
        let ip_str = spec.cluster_ip.as_deref().unwrap_or("");
        if ip_str.is_empty() || ip_str == "None" {
            continue;
        }
        let Ok(ip) = ip_str.parse::<Ipv4Addr>() else {
            continue;
        };
        let ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let name = svc.metadata.name.as_deref().unwrap_or("");
        push_dns(out, format!("{name}.{ns}.svc.{cluster_domain}"), ip);
        push_dns(out, format!("{name}.{ns}.svc"), ip);
    }
}

fn plan_dns_from_ingress(out: &mut PlannedNetwork, snap: &StoreSnapshot, gateway: Ipv4Addr) {
    for t in snap.by_kind("Ingress") {
        let AnyResource::Ingress(ing) = &t.resource else {
            continue;
        };
        if let Some(spec) = &ing.spec {
            if let Some(rules) = &spec.rules {
                for rule in rules {
                    if let Some(host) = &rule.host {
                        if !host.is_empty() {
                            push_dns(out, host.clone(), gateway);
                        }
                    }
                }
            }
        }
    }
}

fn push_dns(out: &mut PlannedNetwork, hostname: String, ip: Ipv4Addr) {
    if !out.dns_records.iter().any(|r| r.hostname == hostname) {
        out.dns_records.push(PlannedDnsRecord { hostname, ip });
    }
}

fn plan_nsg_rules(snap: &StoreSnapshot) -> Vec<NftRule> {
    let mut merged: Vec<(u32, NftRule)> = Vec::new();
    for t in snap.by_kind("NSG") {
        let AnyResource::Nsg(nsg) = &t.resource else {
            continue;
        };
        let mut sorted = nsg.spec.rules.clone();
        sorted.sort_by_key(|r| r.priority);
        for rule in sorted {
            merged.extend(nsg_rule_entries(&rule));
        }
    }
    merged.sort_by_key(|(p, _)| *p);
    merged.into_iter().map(|(_, r)| r).collect()
}

fn nsg_rule_entries(rule: &NsgRule) -> Vec<(u32, NftRule)> {
    let mut out = Vec::new();
    let action = match rule.action.as_str() {
        "deny" => NftAction::Drop,
        "allow" => NftAction::Accept,
        _ => return out,
    };
    for src in &rule.srcCIDRs {
        for dst in &rule.dstCIDRs {
            out.push((
                rule.priority,
                NftRule {
                    name: rule.name.clone(),
                    chain: "nsg-rules".into(),
                    action: action.clone(),
                    source: Some(src.clone()),
                    dest: Some(dst.clone()),
                    protocol: if rule.protocol.is_empty() {
                        None
                    } else {
                        Some(rule.protocol.clone())
                    },
                    dport: None,
                    sport: None,
                },
            ));
        }
    }
    out
}

fn plan_network_policies(snap: &StoreSnapshot) -> Vec<PlannedNetworkPolicy> {
    snap.by_kind("NetworkPolicy")
        .into_iter()
        .filter_map(|t| {
            let AnyResource::NetworkPolicy(np) = &t.resource else {
                return None;
            };
            Some(PlannedNetworkPolicy {
                namespace: np.metadata.namespace.as_deref().unwrap_or("default").into(),
                name: np.metadata.name.as_deref().unwrap_or("unknown").into(),
            })
        })
        .collect()
}

fn plan_remote_pod_routes(
    snap: &StoreSnapshot,
    local_node: &str,
    peers: &[(String, String)],
) -> Vec<PlannedPodRoute> {
    let peer_gateways: BTreeMap<String, Ipv4Addr> = peers
        .iter()
        .filter_map(|(name, ip)| ip.parse::<Ipv4Addr>().ok().map(|a| (name.clone(), a)))
        .collect();

    let mut routes = Vec::new();
    for t in snap.by_kind("Pod") {
        let AnyResource::Pod(pod) = &t.resource else {
            continue;
        };
        let remote_node = match pod.assigned_node.as_deref() {
            Some(n) if n != local_node && !n.is_empty() => n,
            _ => continue,
        };
        let via = match peer_gateways.get(remote_node) {
            Some(ip) => *ip,
            None => continue,
        };
        let pod_ip = pod
            .status
            .as_ref()
            .and_then(|s| s.pod_ip.as_deref())
            .and_then(|s| s.parse::<Ipv4Addr>().ok());
        let Some(pod_ip) = pod_ip else {
            continue;
        };
        routes.push(PlannedPodRoute {
            pod_ip,
            via,
            remote_node: remote_node.to_string(),
        });
    }
    routes
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
    use crate::types::{ObjectMeta, Pod, PodSpec, PodStatus, ServicePort, ServiceSpec};

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

    fn remote_pod_with_ip(name: &str, node: &str, ip: &str) -> ResourceTracker {
        ResourceTracker::new(AnyResource::Pod(Pod {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: Some(PodSpec::default()),
            assigned_node: Some(node.into()),
            status: Some(PodStatus {
                pod_ip: Some(ip.into()),
                ..Default::default()
            }),
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

    #[test]
    fn planner_emits_service_dns() {
        let snap = StoreSnapshot::from_trackers(vec![svc_clusterip(
            "web",
            "default",
            "web",
            "10.96.0.10",
            80i32,
        )]);
        let mut planner = NetworkPlanner::new("node-a");
        planner.cluster_domain = "cluster.local".into();
        let plan = planner.plan(&snap);
        assert!(plan.dns_records.iter().any(|r| {
            r.hostname == "web.default.svc.cluster.local" && r.ip.to_string() == "10.96.0.10"
        }));
    }

    #[test]
    fn planner_emits_remote_pod_route() {
        let snap = StoreSnapshot::from_trackers(vec![remote_pod_with_ip(
            "remote",
            "node-b",
            "10.42.1.5",
        )]);
        let mut planner = NetworkPlanner::new("node-a");
        planner.peers = vec![("node-b".into(), "192.168.1.2".into())];
        let plan = planner.plan(&snap);
        assert_eq!(plan.remote_routes.len(), 1);
        assert_eq!(
            plan.remote_routes[0].pod_ip,
            "10.42.1.5".parse::<Ipv4Addr>().unwrap()
        );
        assert_eq!(
            plan.remote_routes[0].via,
            "192.168.1.2".parse::<Ipv4Addr>().unwrap()
        );
    }
}
