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
    plan_dns_firewall(&mut filter);
    plan_vnets(snap, &mut state, &mut filter);
    plan_network_policies(snap, &pods, &mut state, &filter.name);
    plan_catch_all(&pods, cfg, &mut filter);
    plan_remote_routes(&pods, cfg, &mut state);
    plan_route_tables(snap, &mut state);
    plan_subnets(snap, &mut state);
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

/// The node filter table with forward, input, and output hook chains.
/// The forward chain starts with an established/related rule for stateful
/// firewalling, so return traffic from outbound connections is accepted
/// without needing explicit per-service rules.
fn base_filter_table(cfg: &PlanConfig) -> NftTable {
    NftTable::new(cfg.filter_table(), NftFamily::Ip)
        .with_chain(
            NftChain::base(
                "forward",
                NftChainKind::Filter,
                NftHook::Forward,
                0,
                NftPolicy::Accept,
            )
            .with_rule(established_related_rule()),
        )
        .with_chain(NftChain::base(
            "input",
            NftChainKind::Filter,
            NftHook::Input,
            0,
            NftPolicy::Accept,
        ))
        .with_chain(NftChain::base(
            "output",
            NftChainKind::Filter,
            NftHook::Output,
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
// DNS firewall — allow DNS (UDP/TCP 53) into the INPUT chain
// ═══════════════════════════════════════════════════════════════════════════

fn plan_dns_firewall(filter: &mut NftTable) {
    // UDP DNS
    let udp_dns = NftRule::from_exprs({
        let mut exprs = Vec::new();
        exprs.extend(match_l4proto(PROTO_UDP));
        exprs.extend(match_dport(53));
        exprs.push(NftExpr::Accept);
        exprs
    })
    .with_comment("dns-udp-accept");

    // TCP DNS
    let tcp_dns = NftRule::from_exprs({
        let mut exprs = Vec::new();
        exprs.extend(match_l4proto(PROTO_TCP));
        exprs.extend(match_dport(53));
        exprs.push(NftExpr::Accept);
        exprs
    })
    .with_comment("dns-tcp-accept");

    push_rule(filter, "input", udp_dns);
    push_rule(filter, "input", tcp_dns);
}

// ═══════════════════════════════════════════════════════════════════════════
// NSG — allow/deny in a dedicated nsg-rules chain
// ═══════════════════════════════════════════════════════════════════════════

fn plan_nsgs(snap: &StoreSnapshot, filter: &mut NftTable) {
    let mut nsgs = records_of_kind(snap, "Nsg");
    nsgs.sort_by_key(|r| r.name().to_string());

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
    // Only create the nsg-rules chain if there are actual rules.
    if rules.is_empty() {
        return;
    }
    // Create the nsg-rules chain and populate it.
    let mut nsg_chain = NftChain::regular("nsg-rules", NftChainKind::Filter);
    rules.sort_by_key(|(p, _)| *p);
    nsg_chain.rules = rules.into_iter().map(|(_, r)| r).collect();
    // Whitelist semantics: default-deny at the end of the chain.
    nsg_chain.rules.push(
        NftRule::drop().with_comment("nsg-default-deny"),
    );
    filter.chains.insert("nsg-rules".into(), nsg_chain);
    // Add a jump from the forward chain to nsg-rules (after established).
    if let Some(forward) = filter.chains.get_mut("forward") {
        forward.rules.push(
            NftRule::jump("nsg-rules").with_comment("jump-to-nsg"),
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
// NetworkPolicy — pod-selector IP sets + set-matching rules
// ═══════════════════════════════════════════════════════════════════════════

fn plan_network_policies(
    snap: &StoreSnapshot,
    pods: &[&ResourceRecord],
    state: &mut NetmuxState,
    _filter_table_name: &str,
) {
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

        let set_name = format!("np_{}_{}", sanitize(ns), sanitize(name));
        let mut set = NftSet::ipv4(&set_name);
        for (ip, _) in resolve_backends(pods, ns, &selector) {
            set = set.with_ipv4(ip);
        }
        // Add the set to the filter table.
        if let Some(t) = state.tables.get_mut(&key) {
            t.sets.insert(set.name.clone(), set);
        }
        // Add a set-matching rule in the nsg-rules chain: if source IP is in
        // the set, accept the packet. This implements ingress NetworkPolicy.
        if let Some(t) = state.tables.get_mut(&key)
            && let Some(nsg_chain) = t.chains.get_mut("nsg-rules")
        {
            let lookup_rule = NftRule::from_exprs(vec![
                NftExpr::Meta {
                    kind: META_L4PROTO,
                    op: CMP_EQ,
                    value: PROTO_TCP as u32,
                },
                NftExpr::Cmp {
                    sreg: 1,
                    op: CMP_EQ,
                    data: vec![PROTO_TCP],
                },
                NftExpr::Lookup {
                    set: set_name.clone(),
                    sreg: 1,
                },
                NftExpr::Accept,
            ])
            .with_comment(format!("np-{name}-ingress"));
            nsg_chain.rules.push(lookup_rule);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Catch-all — accept pod-to-pod traffic
// ═══════════════════════════════════════════════════════════════════════════

/// Add a catch-all chain that accepts traffic destined to the pod CIDR.
/// This is appended after the nsg-rules jump so that pods can communicate
/// with each other even without explicit NSG rules.
fn plan_catch_all(pods: &[&ResourceRecord], cfg: &PlanConfig, filter: &mut NftTable) {
    // Only add if there are pods (meaning pod CIDR traffic exists).
    if pods.is_empty() {
        return;
    }
    // Create the catch-all chain with a single rule: accept if dst is pod CIDR.
    let mut exprs = match_cidr(16, &cfg.pod_cidr);
    exprs.push(NftExpr::Accept);
    let catch_all = NftChain::regular("catch-all", NftChainKind::Filter)
        .with_rule(NftRule::from_exprs(exprs).with_comment("pod-cidr-accept"));
    filter.chains.insert("catch-all".into(), catch_all);
    // Jump from forward to catch-all (after nsg-rules).
    if let Some(forward) = filter.chains.get_mut("forward") {
        forward.rules.push(
            NftRule::jump("catch-all").with_comment("jump-to-catch-all"),
        );
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
    // Kubernetes API server DNS record.
    // The API server is typically at the first IP in the service CIDR.
    let api_server_ip = cfg.service_cidr.gateway();
    state.dns_records.insert(
        format!("kubernetes.default.svc.{}", cfg.cluster_domain),
        api_server_ip,
    );
    state
        .dns_records
        .insert("kubernetes.default.svc".into(), api_server_ip);

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
// RouteTable — custom route resources
// ═══════════════════════════════════════════════════════════════════════════

fn plan_route_tables(snap: &StoreSnapshot, state: &mut NetmuxState) {
    for rec in records_of_kind(snap, "RouteTable") {
        let AnyResource::RouteTable(rt) = &rec.spec else {
            continue;
        };
        for route in &rt.spec.routes {
            // Parse dest as "IP/prefix" or just "IP" (defaults to /32).
            let (ip_str, prefix) = if let Some((ip_s, pfx_s)) = route.dest.split_once('/') {
                (ip_s, pfx_s.parse::<u8>().unwrap_or(32))
            } else {
                (route.dest.as_str(), 32)
            };
            let dest = match ip_str.parse::<Ipv4Addr>() {
                Ok(ip) => ip,
                Err(_) => continue,
            };
            let gateway = route.via.as_deref().and_then(|s| s.parse::<Ipv4Addr>().ok());
            let route_spec = RouteSpec {
                dest,
                prefix,
                gateway,
                oif: None,
            };
            state.routes.insert(route_spec.key(), route_spec);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Subnet — named subnet pool registration
// ═══════════════════════════════════════════════════════════════════════════

fn plan_subnets(snap: &StoreSnapshot, state: &mut NetmuxState) {
    for rec in records_of_kind(snap, "Subnet") {
        let AnyResource::Subnet(sub) = &rec.spec else {
            continue;
        };
        let name = rec.name().to_string();
        if let Some(cidr) = Ipv4Cidr::parse(&sub.spec.cidr) {
            state
                .ip_pools
                .entry(format!("subnet-{}", sanitize(&name)))
                .or_insert_with(|| IpPool::new(cidr));
        }
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

    #[test]
    fn plan_dns_firewall_adds_input_rules() {
        let snap = StoreSnapshot::from_records(vec![]);
        let state = plan(&snap, &cfg());
        let filter = state.table(NftFamily::Ip, "z8s_filter_node-a").unwrap();
        let input = &filter.chains["input"];

        // Should have exactly 2 DNS rules: UDP 53 and TCP 53.
        let dns_rules: Vec<_> = input
            .rules
            .iter()
            .filter(|r| {
                r.comment.as_deref() == Some("dns-udp-accept")
                    || r.comment.as_deref() == Some("dns-tcp-accept")
            })
            .collect();
        assert_eq!(dns_rules.len(), 2, "expected 2 DNS firewall rules, got {}", input.rules.len());

        // Verify UDP rule has port 53 match + accept.
        let udp = input.rules.iter().find(|r| r.comment.as_deref() == Some("dns-udp-accept")).unwrap();
        assert!(udp.exprs.iter().any(|e| matches!(e, NftExpr::Accept)));
        assert!(udp.exprs.iter().any(|e| matches!(e, NftExpr::Meta { kind: META_L4PROTO, .. })));

        // Verify TCP rule has port 53 match + accept.
        let tcp = input.rules.iter().find(|r| r.comment.as_deref() == Some("dns-tcp-accept")).unwrap();
        assert!(tcp.exprs.iter().any(|e| matches!(e, NftExpr::Accept)));
        assert!(tcp.exprs.iter().any(|e| matches!(e, NftExpr::Meta { kind: META_L4PROTO, .. })));
    }
}
