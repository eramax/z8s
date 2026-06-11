//! # Network Planner
//!
//! Pure desired-state computation from a store snapshot. The planner
//! reads the snapshot and produces a `PlannedNetwork` describing all
//! the network intents the node should be applying — DNAT rules, DNS
//! records, NSG rules, routes, etc.
//!
//! ## Why Pure?
//!
//! The planner is intentionally side-effect free. It takes a snapshot
//! and returns a plan. The reconciler (`sync.rs`) takes that plan and
//! applies it to nftables/netlink. This separation means:
//!
//! - The planner is trivially testable (no mocks, no netlink needed)
//! - The reconciler can be re-run idempotently
//! - Diffing `PlannedNetwork` against `applied` is straightforward
//!
//! ## Plan Structure
//!
//! ```text
//! PlannedNetwork {
//!   generation,           // monotonic
//!   dnat_rules,           // ClusterIP / NodePort DNAT intents
//!   dns_records,          // hostname → IP
//!   nsg_rules,            // NSG allow/deny rules
//!   network_policies,     // (ns, name) references
//!   remote_routes,        // routes to pods on other nodes
//!   local_pod_count,
//!   service_count,
//! }
//! ```

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use z8s_core::store::StoreSnapshot;
use z8s_core::types::{AnyResource, NsgRule, Service};

use crate::state::RuleKey;

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
    pub target_port: u16,
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

/// Desired network intents for this node (dry-run output of the planner).
#[derive(Debug, Clone, Default)]
pub struct PlannedNetwork {
    pub generation: u64,
    pub dnat_rules: Vec<PlannedDnat>,
    pub dns_records: Vec<PlannedDnsRecord>,
    pub nsg_rules: Vec<crate::rule::NftRule>,
    pub network_policies: Vec<PlannedNetworkPolicy>,
    pub remote_routes: Vec<PlannedPodRoute>,
    pub local_pod_count: usize,
    pub service_count: usize,
}

/// Network planner — pure functional, no IO.
pub struct NetworkPlanner {
    pub node_name: String,
    pub cluster_domain: String,
    pub gateway: Ipv4Addr,
    pub peers: Vec<(String, String)>,
}

impl NetworkPlanner {
    pub fn new(node_name: impl Into<String>) -> Self {
        Self {
            node_name: node_name.into(),
            cluster_domain: "cluster.local".to_string(),
            gateway: Ipv4Addr::new(10, 42, 0, 1),
            peers: Vec::new(),
        }
    }

    pub fn with_cluster_domain(mut self, d: impl Into<String>) -> Self {
        self.cluster_domain = d.into();
        self
    }

    pub fn with_gateway(mut self, gw: Ipv4Addr) -> Self {
        self.gateway = gw;
        self
    }

    pub fn with_peers(mut self, peers: Vec<(String, String)>) -> Self {
        self.peers = peers;
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
            .filter_map(|t| match &t.spec {
                AnyResource::Service(s) => Some(s),
                _ => None,
            })
            .collect();
        out.service_count = services.len();

        plan_services(&mut out, &services, &local_pods);
        plan_dns_from_services(&mut out, &services, &self.cluster_domain);
        plan_dns_in_cluster_api(&mut out, &self.cluster_domain);
        out.nsg_rules = plan_nsg_rules(snap);
        out.network_policies = plan_network_policies(snap);
        out.remote_routes = plan_remote_pod_routes(snap, &self.node_name, &self.peers);

        out
    }
}

fn plan_services(
    out: &mut PlannedNetwork,
    services: &[&Service],
    local_pods: &[(&z8s_core::types::Pod, BTreeMap<String, String>)],
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

        let Some(ports) = spec.ports.as_ref() else { continue };
        for svc_port in ports {
            let target_port = svc_port.target_port.unwrap_or(svc_port.port);

            if let Some(cip) = cluster_ip {
                out.dnat_rules.push(PlannedDnat {
                    rule_key: RuleKey::clusterip_dnat(&cip.to_string(), svc_port.port),
                    service_ns: ns.to_string(),
                    service_name: name.to_string(),
                    listen_port: svc_port.port,
                    is_nodeport: false,
                    node_port: None,
                    cluster_ip: Some(cip),
                    target_port,
                });
            }

            if svc_type == "NodePort" || svc_type == "LoadBalancer" {
                if let Some(np) = svc_port.node_port {
                    out.dnat_rules.push(PlannedDnat {
                        rule_key: RuleKey::nodeport_dnat(np),
                        service_ns: ns.to_string(),
                        service_name: name.to_string(),
                        listen_port: svc_port.port,
                        is_nodeport: true,
                        node_port: Some(np),
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
        push_dns(out, format!("{}.{}.svc.{}", name, ns, cluster_domain), ip);
        push_dns(out, format!("{}.{}.svc", name, ns), ip);
    }
}

fn plan_dns_in_cluster_api(out: &mut PlannedNetwork, cluster_domain: &str) {
    // The well-known Kubernetes API service IP — by convention the
    // first address in the service CIDR.
    let ip = Ipv4Addr::new(10, 96, 0, 1);
    push_dns(out, format!("kubernetes.default.svc.{}", cluster_domain), ip);
    push_dns(out, "kubernetes.default.svc".to_string(), ip);
}

fn push_dns(out: &mut PlannedNetwork, hostname: String, ip: Ipv4Addr) {
    if !out.dns_records.iter().any(|r| r.hostname == hostname) {
        out.dns_records.push(PlannedDnsRecord { hostname, ip });
    }
}

fn plan_nsg_rules(snap: &StoreSnapshot) -> Vec<crate::rule::NftRule> {
    let mut merged: Vec<(u32, crate::rule::NftRule)> = Vec::new();
    for t in snap.by_kind("NSG") {
        let AnyResource::Nsg(nsg) = &t.spec else {
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

fn nsg_rule_entries(rule: &NsgRule) -> Vec<(u32, crate::rule::NftRule)> {
    let mut out = Vec::new();
    let action = match rule.action.as_str() {
        "deny" => crate::rule::NftAction::Drop,
        "allow" => crate::rule::NftAction::Accept,
        _ => return out,
    };
    for src in &rule.src_cidrs {
        for dst in &rule.dst_cidrs {
            out.push((
                rule.priority as u32,
                crate::rule::NftRule {
                    name: rule.name.clone(),
                    chain: "nsg-rules".into(),
                    action: action.clone(),
                    source: Some(src.clone()),
                    dest: Some(dst.clone()),
                    protocol: None,
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
            let AnyResource::NetworkPolicy(np) = &t.spec else {
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
        let AnyResource::Pod(_pod) = &t.spec else {
            continue;
        };
        // Pods don't carry their own assigned_node — that lives on the ResourceRecord.
        let remote_node = match t.assigned_node.as_deref() {
            Some(n) if n != local_node && !n.is_empty() => n,
            _ => continue,
        };
        let via = match peer_gateways.get(remote_node) {
            Some(ip) => *ip,
            None => continue,
        };
        // Pod IP lives on the record's status (set by the controller after IP allocation).
        let pod_ip = t
            .status
            .pod_ip
            .as_deref()
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
) -> Vec<(&'a z8s_core::types::Pod, BTreeMap<String, String>)> {
    snap.by_node(node_name)
        .into_iter()
        .filter_map(|t| {
            let AnyResource::Pod(pod) = &t.spec else {
                return None;
            };
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            Some((pod, labels))
        })
        .collect()
}

fn selector_matches_any_local_pod(
    selector: &BTreeMap<String, String>,
    ns: &str,
    local_pods: &[(&z8s_core::types::Pod, BTreeMap<String, String>)],
) -> bool {
    local_pods.iter().any(|(pod, labels)| {
        pod.metadata.namespace.as_deref().unwrap_or("default") == ns
            && selector.iter().all(|(k, v)| labels.get(k) == Some(v))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use z8s_core::types::{ObjectMeta, Pod, PodSpec, ServicePort, ServiceSpec};

    fn pod(name: &str, ns: &str, node: &str, app: &str) -> z8s_core::types::ResourceRecord {
        use z8s_core::types::{Phase, ResourceRecord, ResourceStatus};
        let mut labels = BTreeMap::new();
        labels.insert("app".into(), app.into());
        ResourceRecord {
            spec: AnyResource::Pod(Pod {
                api_version: "v1".into(),
                kind: "Pod".into(),
                metadata: ObjectMeta {
                    name: Some(name.into()),
                    namespace: Some(ns.into()),
                    labels: Some(labels),
                    uid: Some(format!("uid-{}-{}", name, node)),
                    ..Default::default()
                },
                spec: Some(PodSpec::default()),
            }),
            status: ResourceStatus::default(),
            generation: 1,
            observed_generation: 1,
            assigned_node: Some(node.into()),
            last_updated: 0,
        }
    }

    fn svc_clusterip(
        name: &str,
        ns: &str,
        app: &str,
        ip: &str,
        port: u16,
    ) -> z8s_core::types::ResourceRecord {
        let mut selector = BTreeMap::new();
        selector.insert("app".into(), app.into());
        z8s_core::types::ResourceRecord {
            spec: AnyResource::Service(Service {
                api_version: "v1".into(),
                kind: "Service".into(),
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
                        name: "http".to_string(),
                        port,
                        target_port: Some(port),
                        node_port: None,
                        protocol: None,
                    }]),
                }),
                ..Default::default()
            }),
            generation: 1,
            observed_generation: 1,
            status: Default::default(),
            assigned_node: None,
            last_updated: 0,
        }
    }

    #[test]
    fn planner_emits_clusterip_for_local_pod() {
        let snap = StoreSnapshot::from_records(vec![
            pod("p1", "default", "node-a", "web"),
            svc_clusterip("web", "default", "web", "10.96.0.10", 80),
        ]);
        let plan = NetworkPlanner::new("node-a").plan(&snap);
        assert_eq!(plan.local_pod_count, 1);
        assert_eq!(plan.dnat_rules.len(), 1);
        assert!(!plan.dnat_rules[0].is_nodeport);
        assert_eq!(plan.dnat_rules[0].cluster_ip, Some("10.96.0.10".parse().unwrap()));
    }

    #[test]
    fn planner_skips_service_without_local_backend() {
        let snap = StoreSnapshot::from_records(vec![
            pod("p1", "default", "node-b", "web"),
            svc_clusterip("web", "default", "web", "10.96.0.10", 80),
        ]);
        let plan = NetworkPlanner::new("node-a").plan(&snap);
        assert!(plan.dnat_rules.is_empty());
    }

    #[test]
    fn planner_emits_service_dns() {
        let snap = StoreSnapshot::from_records(vec![svc_clusterip(
            "web", "default", "web", "10.96.0.10", 80,
        )]);
        let plan = NetworkPlanner::new("node-a")
            .with_cluster_domain("cluster.local")
            .plan(&snap);
        assert!(plan.dns_records.iter().any(|r| {
            r.hostname == "web.default.svc.cluster.local" && r.ip.to_string() == "10.96.0.10"
        }));
    }

    #[test]
    fn planner_emits_kubernetes_api_dns() {
        let snap = StoreSnapshot::from_records(vec![]);
        let plan = NetworkPlanner::new("node-a")
            .with_cluster_domain("cluster.local")
            .plan(&snap);
        assert!(plan.dns_records.iter().any(|r| {
            r.hostname == "kubernetes.default.svc.cluster.local"
                && r.ip == Ipv4Addr::new(10, 96, 0, 1)
        }));
    }

    #[test]
    fn planner_emits_remote_pod_route() {
        let mut p = pod("remote", "default", "node-b", "x");
        p.status.pod_ip = Some("10.42.1.5".into());
        let snap = StoreSnapshot::from_records(vec![p]);
        let plan = NetworkPlanner::new("node-a")
            .with_peers(vec![("node-b".into(), "192.168.1.2".into())])
            .plan(&snap);
        assert_eq!(plan.remote_routes.len(), 1);
        assert_eq!(plan.remote_routes[0].pod_ip, "10.42.1.5".parse::<Ipv4Addr>().unwrap());
        assert_eq!(plan.remote_routes[0].via, "192.168.1.2".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn planner_no_services_means_no_dnat() {
        let snap = StoreSnapshot::from_records(vec![pod("p1", "default", "node-a", "web")]);
        let plan = NetworkPlanner::new("node-a").plan(&snap);
        assert!(plan.dnat_rules.is_empty());
        assert_eq!(plan.service_count, 0);
    }

    #[test]
    fn planner_external_name_service_skipped_for_dns() {
        // ExternalName service has no ClusterIP — should not be in DNS records
        let mut selector = BTreeMap::new();
        selector.insert("app".into(), "web".into());
        let rec = z8s_core::types::ResourceRecord {
            spec: AnyResource::Service(Service {
                metadata: ObjectMeta {
                    name: Some("web".into()),
                    namespace: Some("default".into()),
                    ..Default::default()
                },
                spec: Some(ServiceSpec {
                    selector: Some(selector),
                    cluster_ip: None,
                    type_: Some("ExternalName".into()),
                    ports: None,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            generation: 1,
            observed_generation: 1,
            status: Default::default(),
            assigned_node: None,
            last_updated: 0,
        };
        let snap = StoreSnapshot::from_records(vec![rec]);
        let plan = NetworkPlanner::new("node-a").plan(&snap);
        // No DNS records for ExternalName services
        assert!(!plan.dns_records.iter().any(|r| r.hostname.starts_with("web.")));
    }
}
