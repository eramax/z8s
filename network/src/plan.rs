//! # Planner — desired network state from the store
//!
//! [`plan`] is a **pure function**: it reads a [`StoreSnapshot`] of the cluster
//! (the DB resources) and produces a [`NetmuxState`] describing exactly what
//! this node's data plane should look like. It performs no IO. The engine then
//! diffs that desired state against what is currently applied and emits the
//! minimal set of kernel operations ([`crate::reconcile`]).
//!
//! This is the "patch the DB structs straight into the engine" path: a
//! controller calls `plan(&snapshot, &cfg)` then
//! `engine.reconcile_and_apply(&desired, false)`.
//!
//! ## What it produces (per node)
//!
//! - **`z8s_nat_{node}`** — ClusterIP / NodePort DNAT (prerouting + output),
//!   pod-CIDR masquerade (postrouting)
//! - **`z8s_filter_{node}`** — NSG allow/deny rules (forward)
//! - **VNet** isolation + per-VNet IP pools
//! - **NetworkPolicy** pod-selector IP sets
//! - **Remote pod routes** — `pod_ip/32 via peer_gateway` for pods on peers
//! - **DNS records** — `svc.ns.svc.<domain>` → ClusterIP, ingress host → gateway
//!
//! ## Safety
//!
//! Only ever touches tables named `z8s_*_{node}` plus per-resource
//! `vnet_*` / `nsg_*` tables — never foreign rulesets.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use z8s_core::store::StoreSnapshot;
use z8s_core::types::{AnyResource, ResourceRecord};

use crate::ipam::{IpPool, Ipv4Cidr};
use crate::model::*;

// ═══════════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════════

/// Inputs the planner needs beyond the store snapshot.
#[derive(Debug, Clone)]
pub struct PlanConfig {
    /// This node's name (used as the table-name suffix).
    pub node_name: String,
    /// Pod CIDR (the range pods are allocated from; masqueraded on egress).
    pub pod_cidr: Ipv4Cidr,
    /// Service CIDR (ClusterIP range; informational here).
    pub service_cidr: Ipv4Cidr,
    /// DNS cluster domain (e.g. `cluster.local`).
    pub cluster_domain: String,
    /// Pod gateway address.
    pub gateway: Ipv4Addr,
    /// Peer nodes: `(node_name, node_ip)`. Remote pod routes go via the IP.
    pub peers: Vec<(String, Ipv4Addr)>,
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self {
            node_name: "node".into(),
            pod_cidr: Ipv4Cidr::parse("10.42.0.0/16").expect("valid default pod cidr"),
            service_cidr: Ipv4Cidr::parse("10.96.0.0/16").expect("valid default service cidr"),
            cluster_domain: "cluster.local".into(),
            gateway: Ipv4Addr::new(10, 42, 0, 1),
            peers: Vec::new(),
        }
    }
}

impl PlanConfig {
    /// The NAT table name for this node.
    pub fn nat_table(&self) -> String {
        format!("z8s_nat_{}", sanitize(&self.node_name))
    }
    /// The filter table name for this node.
    pub fn filter_table(&self) -> String {
        format!("z8s_filter_{}", sanitize(&self.node_name))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Build the desired [`NetmuxState`] for this node from the store snapshot.
pub fn plan(snap: &StoreSnapshot, cfg: &PlanConfig) -> NetmuxState {
    let mut state = NetmuxState::new();

    let mut nat = base_nat_table(cfg);
    let mut filter = base_filter_table(cfg);

    let pods = records_of_kind(snap, "Pod");

    plan_services(snap, &pods, cfg, &mut nat);
    plan_masquerade(cfg, &mut nat);
    plan_nsgs(snap, &mut filter);
    plan_vnets(snap, &mut state, &mut filter);
    plan_network_policies(snap, &pods, &mut state);
    plan_remote_routes(&pods, cfg, &mut state);
    plan_dns(snap, cfg, &mut state);

    state
        .tables
        .insert((NftFamily::Ip, nat.name.clone()), nat);
    state
        .tables
        .insert((NftFamily::Ip, filter.name.clone()), filter);

    // Pod CIDR pool (bookkeeping for callers that allocate through the state).
    state
        .ip_pools
        .entry("pods".to_string())
        .or_insert_with(|| IpPool::new(cfg.pod_cidr.clone()));

    state.generation = 1;
    state
}

// ═══════════════════════════════════════════════════════════════════════════
// Base tables
// ═══════════════════════════════════════════════════════════════════════════

/// The node NAT table with its prerouting/output/postrouting hook chains.
fn base_nat_table(cfg: &PlanConfig) -> NftTable {
    NftTable::new(cfg.nat_table(), NftFamily::Ip)
        .with_chain(NftChain::base(
            "prerouting",
            NftChainKind::Nat,
            NftHook::Prerouting,
            -100,
            NftPolicy::Accept,
        ))
        .with_chain(NftChain::base(
            "output",
            NftChainKind::Nat,
            NftHook::Output,
            -100,
            NftPolicy::Accept,
        ))
        .with_chain(NftChain::base(
            "postrouting",
            NftChainKind::Nat,
            NftHook::Postrouting,
            100,
            NftPolicy::Accept,
        ))
}

/// The node filter table with its forward hook chain.
fn base_filter_table(cfg: &PlanConfig) -> NftTable {
    NftTable::new(cfg.filter_table(), NftFamily::Ip).with_chain(NftChain::base(
        "forward",
        NftChainKind::Filter,
        NftHook::Forward,
        0,
        NftPolicy::Accept,
    ))
}

// ═══════════════════════════════════════════════════════════════════════════
// Services — ClusterIP / NodePort DNAT
// ═══════════════════════════════════════════════════════════════════════════

fn plan_services(
    snap: &StoreSnapshot,
    pods: &[&ResourceRecord],
    _cfg: &PlanConfig,
    nat: &mut NftTable,
) {
    let mut services = records_of_kind(snap, "Service");
    services.sort_by_key(|r| r.name().to_string());

    for rec in services {
        let AnyResource::Service(svc) = &rec.spec else {
            continue;
        };
        let Some(spec) = &svc.spec else { continue };
        let Some(selector) = &spec.selector else {
            continue;
        };
        if selector.is_empty() {
            continue;
        }
        let ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let svc_type = spec.type_.as_deref().unwrap_or("ClusterIP");
        let cluster_ip = spec
            .cluster_ip
            .as_deref()
            .filter(|c| !c.is_empty() && *c != "None")
            .and_then(|c| c.parse::<Ipv4Addr>().ok());

        let backends = resolve_backends(pods, ns, selector);
        if backends.is_empty() {
            continue;
        }

        for port in spec.ports.as_deref().unwrap_or(&[]) {
            let proto = proto_number(port.protocol.as_deref().unwrap_or("TCP"));
            let target = port.target_port.unwrap_or(port.port);
            let total = backends.len() as u32;

            for (idx, (ip, _)) in backends.iter().enumerate() {
                let lb = Some((idx as u32, total));
                if let Some(cip) = cluster_ip {
                    let rule = clusterip_dnat_rule(cip, proto, port.port, *ip, target, lb)
                        .with_comment(format!("{}/{}", svc.metadata.name.as_deref().unwrap_or(""), port.name));
                    push_rule(nat, "prerouting", rule.clone());
                    push_rule(nat, "output", rule);
                }
                if matches!(svc_type, "NodePort" | "LoadBalancer")
                    && let Some(np) = port.node_port {
                        let rule = nodeport_dnat_rule(proto, np, *ip, target, lb)
                            .with_comment(format!("nodeport {np}"));
                        push_rule(nat, "prerouting", rule.clone());
                        push_rule(nat, "output", rule);
                    }
            }
        }
    }
}

/// Resolve service backends cluster-wide: pods in `ns` whose labels satisfy
/// `selector` and that have a routable pod IP. Sorted by IP for determinism.
fn resolve_backends(
    pods: &[&ResourceRecord],
    ns: &str,
    selector: &BTreeMap<String, String>,
) -> Vec<(Ipv4Addr, u16)> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for rec in pods {
        let AnyResource::Pod(pod) = &rec.spec else {
            continue;
        };
        if pod.metadata.namespace.as_deref().unwrap_or("default") != ns {
            continue;
        }
        let labels = pod.metadata.labels.clone().unwrap_or_default();
        if !selector.iter().all(|(k, v)| labels.get(k) == Some(v)) {
            continue;
        }
        if let Some(ip) = rec
            .status
            .pod_ip
            .as_deref()
            .and_then(|s| s.parse::<Ipv4Addr>().ok())
        {
            out.push(ip);
        }
    }
    out.sort();
    out.dedup();
    // Port is filled in by the caller (target port); pair here for the shape.
    out.into_iter().map(|ip| (ip, 0u16)).collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Masquerade
// ═══════════════════════════════════════════════════════════════════════════

fn plan_masquerade(cfg: &PlanConfig, nat: &mut NftTable) {
    let rule = masquerade_rule(&cfg.pod_cidr).with_comment("pod-cidr-masquerade");
    push_rule(nat, "postrouting", rule);
}

// ═══════════════════════════════════════════════════════════════════════════
// NSG — allow/deny in the forward chain
// ═══════════════════════════════════════════════════════════════════════════

fn plan_nsgs(snap: &StoreSnapshot, filter: &mut NftTable) {
    let mut nsgs = records_of_kind(snap, "Nsg");
    nsgs.sort_by_key(|r| r.name().to_string());

    let mut any = false;
    let mut rules: Vec<(i32, NftRule)> = Vec::new();
    for rec in nsgs {
        let AnyResource::Nsg(nsg) = &rec.spec else {
            continue;
        };
        for r in &nsg.spec.rules {
            let accept = match r.action.as_str() {
                "allow" => true,
                "deny" => false,
                _ => continue,
            };
            any = true;
            for s in &r.src_cidrs {
                for d in &r.dst_cidrs {
                    let src = Ipv4Cidr::parse(s);
                    let dst = Ipv4Cidr::parse(d);
                    let rule = nsg_filter_rule(accept, src.as_ref(), dst.as_ref(), None, None)
                        .with_comment(r.name.clone());
                    rules.push((r.priority, rule));
                }
            }
        }
    }
    rules.sort_by_key(|(p, _)| *p);
    for (_, rule) in rules {
        push_rule(filter, "forward", rule);
    }
    // Whitelist semantics: once any NSG exists, drop everything not allowed.
    if any {
        push_rule(
            filter,
            "forward",
            NftRule::drop().with_comment("nsg-default-deny"),
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// VNet — isolation + pools
// ═══════════════════════════════════════════════════════════════════════════

fn plan_vnets(snap: &StoreSnapshot, state: &mut NetmuxState, filter: &mut NftTable) {
    for rec in records_of_kind(snap, "VNet") {
        let AnyResource::VNet(vnet) = &rec.spec else {
            continue;
        };
        let Some(cidr) = vnet.spec.cidr.as_deref().and_then(Ipv4Cidr::parse) else {
            continue;
        };
        let name = vnet.metadata.name.as_deref().unwrap_or("vnet");
        state
            .ip_pools
            .entry(format!("vnet-{name}"))
            .or_insert_with(|| IpPool::new(cidr.clone()));
        // No internet access → drop forward from this CIDR to anywhere.
        if !vnet.spec.internet_access {
            let any = Ipv4Cidr::parse("0.0.0.0/0");
            let rule = nsg_filter_rule(false, Some(&cidr), any.as_ref(), None, None)
                .with_comment(format!("vnet-{name}-no-internet"));
            push_rule(filter, "forward", rule);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// NetworkPolicy — pod-selector IP sets
// ═══════════════════════════════════════════════════════════════════════════

fn plan_network_policies(snap: &StoreSnapshot, pods: &[&ResourceRecord], state: &mut NetmuxState) {
    let nat_filter_key = state
        .tables
        .keys()
        .find(|(_, n)| n.starts_with("z8s_filter_"))
        .cloned();
    let Some(key) = nat_filter_key else { return };

    for rec in records_of_kind(snap, "NetworkPolicy") {
        let AnyResource::NetworkPolicy(np) = &rec.spec else {
            continue;
        };
        let Some(spec) = &np.spec else { continue };
        let ns = np.metadata.namespace.as_deref().unwrap_or("default");
        let name = np.metadata.name.as_deref().unwrap_or("policy");
        let selector = spec.pod_selector.clone().unwrap_or_default();

        let mut set = NftSet::ipv4(format!("np_{}_{}", sanitize(ns), sanitize(name)));
        for (ip, _) in resolve_backends(pods, ns, &selector) {
            set = set.with_ipv4(ip);
        }
        if let Some(t) = state.tables.get_mut(&key) {
            t.sets.insert(set.name.clone(), set);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Remote pod routes
// ═══════════════════════════════════════════════════════════════════════════

fn plan_remote_routes(pods: &[&ResourceRecord], cfg: &PlanConfig, state: &mut NetmuxState) {
    let peer_gw: BTreeMap<&str, Ipv4Addr> =
        cfg.peers.iter().map(|(n, ip)| (n.as_str(), *ip)).collect();

    for rec in pods {
        let node = match rec.assigned_node.as_deref() {
            Some(n) if n != cfg.node_name && !n.is_empty() => n,
            _ => continue,
        };
        let Some(gw) = peer_gw.get(node).copied() else {
            continue;
        };
        if let Some(ip) = rec
            .status
            .pod_ip
            .as_deref()
            .and_then(|s| s.parse::<Ipv4Addr>().ok())
        {
            let route = RouteSpec::host_via(ip, gw);
            state.routes.insert(route.key(), route);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DNS
// ═══════════════════════════════════════════════════════════════════════════

fn plan_dns(snap: &StoreSnapshot, cfg: &PlanConfig, state: &mut NetmuxState) {
    for rec in records_of_kind(snap, "Service") {
        let AnyResource::Service(svc) = &rec.spec else {
            continue;
        };
        let Some(spec) = &svc.spec else { continue };
        if spec.type_.as_deref() == Some("ExternalName") {
            continue;
        }
        let Some(ip) = spec
            .cluster_ip
            .as_deref()
            .filter(|c| !c.is_empty() && *c != "None")
            .and_then(|c| c.parse::<Ipv4Addr>().ok())
        else {
            continue;
        };
        let ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let name = svc.metadata.name.as_deref().unwrap_or("");
        state
            .dns_records
            .insert(format!("{name}.{ns}.svc.{}", cfg.cluster_domain), ip);
        state.dns_records.insert(format!("{name}.{ns}.svc"), ip);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// All records of a given kind in the snapshot.
fn records_of_kind<'a>(snap: &'a StoreSnapshot, kind: &str) -> Vec<&'a ResourceRecord> {
    snap.all().iter().filter(|r| r.kind() == kind).collect()
}

/// Append a rule to a named chain in the table (no-op if the chain is absent).
fn push_rule(table: &mut NftTable, chain: &str, rule: NftRule) {
    if let Some(c) = table.chains.get_mut(chain) {
        c.rules.push(rule);
    }
}

/// Replace characters that are invalid in nftables identifiers.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use z8s_core::types::{ObjectMeta, Pod, Service, ServicePort, ServiceSpec};

    fn cfg() -> PlanConfig {
        PlanConfig {
            node_name: "node-a".into(),
            ..Default::default()
        }
    }

    fn pod_record(name: &str, ns: &str, node: &str, app: &str, ip: Option<&str>) -> ResourceRecord {
        let mut labels = BTreeMap::new();
        labels.insert("app".to_string(), app.to_string());
        let pod = Pod {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some(ns.into()),
                uid: Some(format!("uid-{name}")),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(Default::default()),
            ..Default::default()
        };
        let mut rec = ResourceRecord::new(AnyResource::Pod(pod));
        rec.assigned_node = Some(node.into());
        rec.status.pod_ip = ip.map(|s| s.to_string());
        rec
    }

    fn svc_record(name: &str, ns: &str, app: &str, cip: &str, port: u16, tport: u16) -> ResourceRecord {
        let mut selector = BTreeMap::new();
        selector.insert("app".to_string(), app.to_string());
        let svc = Service {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some(ns.into()),
                uid: Some(format!("uid-{name}")),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                type_: Some("ClusterIP".into()),
                cluster_ip: Some(cip.into()),
                selector: Some(selector),
                ports: Some(vec![ServicePort {
                    name: "http".into(),
                    port,
                    target_port: Some(tport),
                    node_port: None,
                    protocol: Some("TCP".into()),
                }]),
            }),
            ..Default::default()
        };
        ResourceRecord::new(AnyResource::Service(svc))
    }

    #[test]
    fn plan_builds_node_tables() {
        let snap = StoreSnapshot::from_records(vec![]);
        let state = plan(&snap, &cfg());
        assert!(state
            .tables
            .contains_key(&(NftFamily::Ip, "z8s_nat_node-a".to_string())));
        assert!(state
            .tables
            .contains_key(&(NftFamily::Ip, "z8s_filter_node-a".to_string())));
    }

    #[test]
    fn plan_emits_clusterip_dnat_for_backend() {
        let snap = StoreSnapshot::from_records(vec![
            pod_record("p1", "default", "node-a", "web", Some("10.42.0.5")),
            svc_record("web", "default", "web", "10.96.0.10", 80, 8080),
        ]);
        let state = plan(&snap, &cfg());
        let nat = state.table(NftFamily::Ip, "z8s_nat_node-a").unwrap();
        let pre = &nat.chains["prerouting"];
        // One DNAT rule (single backend) referencing the cluster IP.
        assert!(pre.rules.iter().any(|r| r
            .exprs
            .iter()
            .any(|e| matches!(e, NftExpr::Nat { .. }))));
        // And a masquerade rule in postrouting.
        let post = &nat.chains["postrouting"];
        assert!(post
            .rules
            .iter()
            .any(|r| r.exprs.iter().any(|e| matches!(e, NftExpr::Masquerade))));
    }

    #[test]
    fn plan_skips_service_with_no_backends() {
        let snap = StoreSnapshot::from_records(vec![svc_record(
            "web", "default", "web", "10.96.0.10", 80, 8080,
        )]);
        let state = plan(&snap, &cfg());
        let nat = state.table(NftFamily::Ip, "z8s_nat_node-a").unwrap();
        let pre = &nat.chains["prerouting"];
        assert!(!pre
            .rules
            .iter()
            .any(|r| r.exprs.iter().any(|e| matches!(e, NftExpr::Nat { .. }))));
    }

    #[test]
    fn plan_emits_dns_record() {
        let snap = StoreSnapshot::from_records(vec![svc_record(
            "web", "default", "web", "10.96.0.10", 80, 8080,
        )]);
        let state = plan(&snap, &cfg());
        assert_eq!(
            state.dns_records.get("web.default.svc.cluster.local"),
            Some(&Ipv4Addr::new(10, 96, 0, 10))
        );
    }

    #[test]
    fn plan_emits_remote_pod_route() {
        let mut c = cfg();
        c.peers = vec![("node-b".into(), Ipv4Addr::new(192, 168, 1, 2))];
        let snap = StoreSnapshot::from_records(vec![pod_record(
            "remote", "default", "node-b", "web", Some("10.42.1.7"),
        )]);
        let state = plan(&snap, &c);
        let key = (Ipv4Addr::new(10, 42, 1, 7), 32u8);
        assert_eq!(
            state.routes.get(&key).map(|r| r.gateway),
            Some(Some(Ipv4Addr::new(192, 168, 1, 2)))
        );
    }

    #[test]
    fn plan_local_pod_has_no_remote_route() {
        let snap = StoreSnapshot::from_records(vec![pod_record(
            "local", "default", "node-a", "web", Some("10.42.0.5"),
        )]);
        let state = plan(&snap, &cfg());
        assert!(state.routes.is_empty());
    }
}
