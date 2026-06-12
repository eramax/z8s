//! # Netmux Engine
//!
//! The engine is the single entry point for translating a desired [`NetmuxState`]
//! into kernel-visible state (nftables rules, veth pairs, routes, DNS records).
//!
//! ## Design
//!
//! - **Pure diff** — [`reconcile`] takes `desired` and `current` and returns
//!   a list of [`NetlinkOp`]s. No IO, no side effects, fully testable.
//! - **Side-effecting apply** — [`Netmux`] holds an [`NlSocket`](crate::syscalls::NlSocket)
//!   and executes ops in order, tracking the last-applied `current` state for
//!   the next tick.
//! - **Auto-cleanup** — when a resource is removed from the DB and the desired
//!   state is patched, the next reconcile tick produces the `Del*` ops
//!   automatically (see `compute_diff`).
//!
//! ## Reusability
//!
//! A controller or controller-side reconcile loop can:
//!
//! ```ignore
//! let desired = build_desired_state(&store)?; // pure function over store
//! let ops = reconcile(&desired, &engine.current());
//! engine.apply(ops)?;
//! ```
//!
//! The desired state can be patched/updated at any time — the engine only
//! sees the new desired state and the previous current, and computes the
//! minimal op set.

use std::net::Ipv4Addr;

use anyhow::{anyhow, Context, Result};
use tracing::{debug, info};

use crate::model::*;
use crate::rtnetlink::RouteSocket;
use crate::syscalls::NlSocket;

// ═══════════════════════════════════════════════════════════════════════════
// Pure diff
// ═══════════════════════════════════════════════════════════════════════════

/// Compute the ops required to transform `current` into `desired`.
///
/// This is a **pure function** — no IO, no mutation. It is the single
/// place where DB-driven patches become netlink ops.
///
/// ## Cleanup
///
/// Any resource present in `current` but absent from `desired` is removed.
/// This is how deleted/stale resources get cleaned up — the controller
/// simply re-derives `desired` from the DB, and the diff picks up the
/// missing entries automatically.
pub fn reconcile(desired: &NetmuxState, current: &NetmuxState) -> Vec<NetlinkOp> {
    let mut ops = Vec::new();

    // ── Tables ─────────────────────────────────────────────────────
    for (key, want_table) in &desired.tables {
        match current.tables.get(key) {
            None => {
                ops.push(NetlinkOp::AddTable {
                    family: want_table.family,
                    name: want_table.name.clone(),
                });
                push_table_init(&mut ops, want_table);
            }
            Some(cur_table) => {
                push_table_diff(&mut ops, want_table, cur_table);
            }
        }
    }
    for (key, cur_table) in &current.tables {
        if !desired.tables.contains_key(key) {
            ops.push(NetlinkOp::DelTable {
                family: cur_table.family,
                name: cur_table.name.clone(),
            });
        }
    }

    // ── IP pools ───────────────────────────────────────────────────
    for (name, want_pool) in &desired.ip_pools {
        if !current.ip_pools.contains_key(name) {
            // Pools are declarative bookkeeping; nothing to apply
            // to the kernel, but we keep them in current to track
            // that the resource was seen.
            debug!(pool = %name, "new ip pool registered (no kernel op)");
            let _ = want_pool;
        }
    }
    for name in current.ip_pools.keys() {
        if !desired.ip_pools.contains_key(name) {
            debug!(pool = %name, "ip pool removed (no kernel op)");
        }
    }

    // ── IPv6 pools ─────────────────────────────────────────────────
    for name in desired.ip6_pools.keys() {
        if !current.ip6_pools.contains_key(name) {
            debug!(pool = %name, "new ip6 pool registered (no kernel op)");
        }
    }
    for name in current.ip6_pools.keys() {
        if !desired.ip6_pools.contains_key(name) {
            debug!(pool = %name, "ip6 pool removed (no kernel op)");
        }
    }

    // ── Pods ───────────────────────────────────────────────────────
    for (uid, want_pod) in &desired.pods {
        if !current.pods.contains_key(uid) {
            debug!(pod = %uid, ip = %want_pod.pod_ip, "new pod registered");
        }
    }
    for (uid, cur_pod) in &current.pods {
        if !desired.pods.contains_key(uid) {
            // Cleanup: kernel-level cleanup of the veth/route/DNS is
            // handled by the higher-level pod lifecycle. The engine
            // only owns the declarative state.
            debug!(pod = %uid, ip = %cur_pod.pod_ip, "pod removed from desired");
        }
    }

    // ── Veths ──────────────────────────────────────────────────────
    for (name, want_v) in &desired.veths {
        if !current.veths.contains_key(name) {
            debug!(veth = %name, "new veth registered (host ifindex={})", want_v.host_ifindex);
        }
    }
    for name in current.veths.keys() {
        if !desired.veths.contains_key(name) {
            debug!(veth = %name, "veth removed (cleanup expected by pod lifecycle)");
        }
    }

    // ── Routes ─────────────────────────────────────────────────────
    // Reconcile-managed routes (remote pod routes, static routes). Local
    // pod /32 routes are owned by `attach_pod` and never appear here.
    for (key, want) in &desired.routes {
        if current.routes.get(key) != Some(want) {
            ops.push(NetlinkOp::AddRoute { route: want.clone() });
        }
    }
    for (key, cur) in &current.routes {
        if !desired.routes.contains_key(key) {
            ops.push(NetlinkOp::DelRoute { route: cur.clone() });
        }
    }

    // ── DNS records ────────────────────────────────────────────────
    for (name, ip) in &desired.dns_records {
        if current.dns_records.get(name) != Some(ip) {
            debug!(hostname = %name, ip = %ip, "dns record add/update");
        }
    }
    for name in current.dns_records.keys() {
        if !desired.dns_records.contains_key(name) {
            debug!(hostname = %name, "dns record remove (cleanup)");
        }
    }

    ops
}

/// Emit all the Add* ops for a freshly-created table.
fn push_table_init(ops: &mut Vec<NetlinkOp>, table: &NftTable) {
    for counter in table.counters.values() {
        ops.push(NetlinkOp::AddCounter {
            family: table.family,
            table: table.name.clone(),
            counter: counter.clone(),
        });
    }
    for set in table.sets.values() {
        ops.push(NetlinkOp::AddSet {
            family: table.family,
            table: table.name.clone(),
            set: set.clone(),
        });
    }
    // Chains are created after the table exists. Each chain is created
    // independently; rules are added after their chain is created.
    for chain in table.chains.values() {
        ops.push(NetlinkOp::AddChain {
            family: table.family,
            table: table.name.clone(),
            chain: chain.clone(),
        });
    }
    // Rules come after all chains are created, since a rule might reference
    // a chain via Jump/Goto.
    for chain in table.chains.values() {
        for rule in &chain.rules {
            ops.push(NetlinkOp::AddRule {
                family: table.family,
                table: table.name.clone(),
                chain: chain.name.clone(),
                rule: rule.clone(),
            });
        }
    }
}

/// Diff a table that exists in both current and desired.
fn push_table_diff(ops: &mut Vec<NetlinkOp>, want: &NftTable, cur: &NftTable) {
    // Counters
    for (name, want_c) in &want.counters {
        if !cur.counters.contains_key(name) {
            ops.push(NetlinkOp::AddCounter {
                family: want.family,
                table: want.name.clone(),
                counter: want_c.clone(),
            });
        }
    }
    for name in cur.counters.keys() {
        if !want.counters.contains_key(name) {
            ops.push(NetlinkOp::DelCounter {
                family: want.family,
                table: want.name.clone(),
                name: name.clone(),
            });
        }
    }
    // Sets
    for (name, want_set) in &want.sets {
        match cur.sets.get(name) {
            None => ops.push(NetlinkOp::AddSet {
                family: want.family,
                table: want.name.clone(),
                set: want_set.clone(),
            }),
            Some(cur_set) if want_set.elements != cur_set.elements => {
                // Replace set wholesale when elements differ
                ops.push(NetlinkOp::DelSet {
                    family: want.family,
                    table: want.name.clone(),
                    name: want_set.name.clone(),
                });
                ops.push(NetlinkOp::AddSet {
                    family: want.family,
                    table: want.name.clone(),
                    set: want_set.clone(),
                });
            }
            _ => {}
        }
    }
    for name in cur.sets.keys() {
        if !want.sets.contains_key(name) {
            ops.push(NetlinkOp::DelSet {
                family: want.family,
                table: want.name.clone(),
                name: name.clone(),
            });
        }
    }
    // Chains + rules
    for (name, want_chain) in &want.chains {
        match cur.chains.get(name) {
            None => {
                ops.push(NetlinkOp::AddChain {
                    family: want.family,
                    table: want.name.clone(),
                    chain: want_chain.clone(),
                });
                for rule in &want_chain.rules {
                    ops.push(NetlinkOp::AddRule {
                        family: want.family,
                        table: want.name.clone(),
                        chain: want_chain.name.clone(),
                        rule: rule.clone(),
                    });
                }
            }
            Some(cur_chain) => {
                push_chain_diff(ops, want.family, &want.name, want_chain, cur_chain);
            }
        }
    }
    for (name, cur_chain) in &cur.chains {
        if !want.chains.contains_key(name) {
            for rule in &cur_chain.rules {
                if let Some(h) = rule.handle {
                    ops.push(NetlinkOp::DelRule {
                        family: want.family,
                        table: want.name.clone(),
                        chain: cur_chain.name.clone(),
                        handle: h,
                    });
                }
            }
            ops.push(NetlinkOp::DelChain {
                family: want.family,
                table: want.name.clone(),
                name: cur_chain.name.clone(),
            });
        }
    }
}

fn push_chain_diff(
    ops: &mut Vec<NetlinkOp>,
    family: NftFamily,
    table: &str,
    want: &NftChain,
    cur: &NftChain,
) {
    let chain_shape_changed = want.kind != cur.kind
        || want.hook != cur.hook
        || want.priority != cur.priority
        || want.policy != cur.policy;
    let rules_changed = want.rules != cur.rules;

    if chain_shape_changed || rules_changed {
        // When the chain shape or rules change, we delete and recreate the
        // entire chain. This avoids the need to track kernel-assigned rule
        // handles — individual rule deletion is not supported without
        // knowing the handle the kernel assigned.
        ops.push(NetlinkOp::DelChain {
            family,
            table: table.to_string(),
            name: cur.name.clone(),
        });
        ops.push(NetlinkOp::AddChain {
            family,
            table: table.to_string(),
            chain: want.clone(),
        });
        for rule in &want.rules {
            ops.push(NetlinkOp::AddRule {
                family,
                table: table.to_string(),
                chain: want.name.clone(),
                rule: rule.clone(),
            });
        }
        return;
    }
    // Same shape and rules — no ops needed.
}

// ═══════════════════════════════════════════════════════════════════════════
// The Netmux engine — side-effecting apply
// ═══════════════════════════════════════════════════════════════════════════

/// The Netmux engine: holds a netlink socket and the current kernel-visible state.
///
/// `Netmux` is intentionally small. It owns:
/// - the netlink socket used to apply nftables ops
/// - the most recently applied `NetmuxState` (the "current" side of the next diff)
/// - the controller-side `NetworkEngine` trait implementation
pub struct Netmux {
    sock: Option<NlSocket>,
    route: Option<RouteSocket>,
    current: NetmuxState,
    /// Pod gateway address used for pod-side default routes / host veth IP.
    gateway: Ipv4Addr,
}

impl Netmux {
    /// Create an unconnected engine (no netlink socket yet). Useful for tests.
    pub fn unconnected() -> Self {
        Self {
            sock: None,
            route: None,
            current: NetmuxState::new(),
            gateway: Ipv4Addr::new(10, 42, 0, 1),
        }
    }

    /// Connect to the kernel netlink sockets (netfilter + route). Requires
    /// `CAP_NET_ADMIN`.
    pub fn connect() -> Result<Self> {
        let sock = NlSocket::open().context("opening netlink netfilter socket")?;
        let route = RouteSocket::open().context("opening netlink route socket")?;
        Ok(Self {
            sock: Some(sock),
            route: Some(route),
            current: NetmuxState::new(),
            gateway: Ipv4Addr::new(10, 42, 0, 1),
        })
    }

    /// Set the pod gateway address (defaults to `10.42.0.1`).
    pub fn with_gateway(mut self, gw: Ipv4Addr) -> Self {
        self.gateway = gw;
        self
    }

    /// The pod gateway address.
    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    /// Borrow the current applied state.
    pub fn current(&self) -> &NetmuxState {
        &self.current
    }

    /// Inject a pre-known `current` state (e.g. after a clean reboot, or in
    /// tests). Does not contact the kernel.
    pub fn seed_current(&mut self, state: NetmuxState) {
        self.current = state;
    }

    /// Apply a list of ops to the kernel and update the current state.
    ///
    /// `dry_run = true` only updates the in-memory `current` (skipping kernel
    /// IO). This is the default in tests.
    pub fn apply(&mut self, ops: &[NetlinkOp], dry_run: bool) -> Result<()> {
        for op in ops {
            if !dry_run {
                if op.is_route() {
                    let route = self
                        .route
                        .as_ref()
                        .ok_or_else(|| anyhow!("netlink route socket not connected"))?;
                    route
                        .apply(op)
                        .with_context(|| format!("applying route op: {:?}", op))?;
                } else {
                    let sock = self
                        .sock
                        .as_ref()
                        .ok_or_else(|| anyhow!("netlink netfilter socket not connected"))?;
                    sock.send(op)
                        .with_context(|| format!("sending op: {:?}", op))?;
                }
            }
            self.apply_one_to_state(op);
        }
        if !ops.is_empty() {
            self.current.generation = self.current.generation.wrapping_add(1);
        }
        Ok(())
    }

    /// High-level convenience: compute diff from `desired` and apply it.
    pub fn reconcile_and_apply(&mut self, desired: &NetmuxState, dry_run: bool) -> Result<usize> {
        let ops = reconcile(desired, &self.current);
        let n = ops.len();
        if n > 0 {
            info!(ops = n, "applying netlink ops");
        }
        self.apply(&ops, dry_run)?;
        Ok(n)
    }

    /// Update the in-memory `current` to reflect an op without contacting the
    /// kernel. Keeps the diff function honest.
    fn apply_one_to_state(&mut self, op: &NetlinkOp) {
        match op {
            NetlinkOp::AddTable { family, name } => {
                let key = (*family, name.clone());
                self.current
                    .tables
                    .entry(key)
                    .or_insert_with(|| NftTable::new(name.clone(), *family));
            }
            NetlinkOp::DelTable { family, name } => {
                self.current.tables.remove(&(*family, name.clone()));
            }
            NetlinkOp::AddChain {
                family,
                table,
                chain,
            } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone())) {
                    t.chains.insert(chain.name.clone(), chain.clone());
                }
            }
            NetlinkOp::DelChain { family, table, name } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone())) {
                    t.chains.remove(name);
                }
            }
            NetlinkOp::AddRule {
                family,
                table,
                chain,
                rule,
            } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone()))
                    && let Some(c) = t.chains.get_mut(chain) {
                        c.rules.push(rule.clone());
                    }
            }
            NetlinkOp::DelRule {
                family,
                table,
                chain,
                handle,
            } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone()))
                    && let Some(c) = t.chains.get_mut(chain) {
                        c.rules.retain(|r| r.handle != Some(*handle));
                    }
            }
            NetlinkOp::AddSet { family, table, set } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone())) {
                    t.sets.insert(set.name.clone(), set.clone());
                }
            }
            NetlinkOp::DelSet { family, table, name } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone())) {
                    t.sets.remove(name);
                }
            }
            NetlinkOp::SetFlush { family, table, name } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone()))
                    && let Some(s) = t.sets.get_mut(name) {
                        s.elements.clear();
                    }
            }
            NetlinkOp::AddCounter { family, table, counter } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone())) {
                    t.counters.insert(counter.name.clone(), counter.clone());
                }
            }
            NetlinkOp::DelCounter { family, table, name } => {
                if let Some(t) = self.current.tables.get_mut(&(*family, table.clone())) {
                    t.counters.remove(name);
                }
            }
            NetlinkOp::AddRoute { route } => {
                self.current.routes.insert(route.key(), route.clone());
            }
            NetlinkOp::DelRoute { route } => {
                self.current.routes.remove(&route.key());
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Pod lifecycle (imperative CRI hot path)
    // ─────────────────────────────────────────────────────────────────────

    /// Attach a pod to the network: allocate a veth pair, give the host side
    /// the gateway IP and a `/32` route to the pod, then move the peer into
    /// the pod's netns and configure its address + default route.
    ///
    /// This is the synchronous CRI hot path. It does not run a full reconcile;
    /// it performs only the per-pod netlink work and records the pod in the
    /// engine's current state so later reconciles see it.
    pub fn attach_pod(
        &mut self,
        pod_uid: &str,
        pod_ip: Ipv4Addr,
        container_pid: u32,
    ) -> Result<VethPair> {
        let route = self
            .route
            .as_ref()
            .ok_or_else(|| anyhow!("netlink route socket not connected"))?;
        let pair = route
            .attach_pod(pod_uid, pod_ip, container_pid, self.gateway)
            .with_context(|| format!("attaching pod {pod_uid}"))?;
        self.current.pods.insert(
            pod_uid.to_string(),
            PodNetwork {
                pod_uid: pod_uid.to_string(),
                pod_ip,
                veth_host: pair.host_name.clone(),
                veth_peer: pair.peer_name.clone(),
                vnet: None,
            },
        );
        self.current.veths.insert(pair.host_name.clone(), pair.clone());
        Ok(pair)
    }

    /// Detach a pod: remove the host `/32` route and delete the veth pair.
    /// The peer interface is destroyed automatically with its host side.
    pub fn detach_pod(&mut self, pod_uid: &str) -> Result<()> {
        let route = self
            .route
            .as_ref()
            .ok_or_else(|| anyhow!("netlink route socket not connected"))?;
        if let Some(pod) = self.current.pods.remove(pod_uid) {
            route
                .detach_pod(&pod.veth_host, pod.pod_ip)
                .with_context(|| format!("detaching pod {pod_uid}"))?;
            self.current.veths.remove(&pod.veth_host);
        }
        Ok(())
    }

    /// Remove veth pairs whose pod UID is no longer active (crash recovery).
    pub fn clean_orphan_veths(&self, active_uids: &[String]) -> Result<usize> {
        let route = self
            .route
            .as_ref()
            .ok_or_else(|| anyhow!("netlink route socket not connected"))?;
        route.clean_orphan_veths(active_uids)
    }
}

impl Default for Netmux {
    fn default() -> Self {
        Self::unconnected()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Builder helpers — declarative state from DB patches
// ═══════════════════════════════════════════════════════════════════════════

/// A builder for [`NetmuxState`] that controllers / API layers can use to
/// declare what the network should look like from DB state.
#[derive(Debug, Default, Clone)]
pub struct NetmuxBuilder {
    state: NetmuxState,
}

impl NetmuxBuilder {
    /// Create an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a vnet (CIDR + isolation).
    pub fn add_vnet(&mut self, vnet: VNetSpec) -> &mut Self {
        let family = NftFamily::Ip;
        let table_name = format!("vnet_{}", sanitize(&vnet.name));
        let table = NftTable::new(&table_name, family)
            .with_chain(
                NftChain::base(
                    "prerouting",
                    NftChainKind::Filter,
                    NftHook::Prerouting,
                    -300,
                    NftPolicy::Accept,
                )
                .with_rule(NftRule::jump("vnet-isolation")),
            )
            .with_chain(NftChain::regular("vnet-isolation", NftChainKind::Filter));
        self.state
            .tables
            .insert((family, table_name), table);
        // Pod CIDR pool
        self.state
            .ip_pools
            .entry(format!("{}-pods", vnet.name))
            .or_insert_with(|| crate::ipam::IpPool::new(vnet.cidr.clone()));
        self
    }

    /// Register a service (ClusterIP, NodePort, etc.).
    ///
    /// Note: the service's CIDR and vnet are immutable after creation —
    /// if you need to change them, create a new service with a new name.
    pub fn add_service(&mut self, svc: ServiceSpec) -> &mut Self {
        // ClusterIP DNAT lives in a single shared "kube" table.
        let family = NftFamily::Ip;
        let kube_table_name = "kube";
        // Ensure required chains exist (idempotent, no overlapping borrows).
        {
            let entry = self
                .state
                .tables
                .entry((family, kube_table_name.to_string()))
                .or_insert_with(|| NftTable::new(kube_table_name, family));
            entry.chains.entry("prerouting".to_string()).or_insert_with(|| {
                NftChain::base(
                    "prerouting",
                    NftChainKind::Nat,
                    NftHook::Prerouting,
                    -100,
                    NftPolicy::Accept,
                )
            });
            entry.chains.entry("clusterip-dnat".to_string()).or_insert_with(|| {
                NftChain::regular("clusterip-dnat", NftChainKind::Nat)
            });
            entry.chains.entry("postrouting".to_string()).or_insert_with(|| {
                NftChain::base(
                    "postrouting",
                    NftChainKind::Nat,
                    NftHook::Postrouting,
                    100,
                    NftPolicy::Accept,
                )
            });
            entry
                .chains
                .entry("masquerade".to_string())
                .or_insert_with(|| NftChain::regular("masquerade", NftChainKind::Nat));
        }
        // Now add the per-service DNAT rules.
        if let Some(cluster_ip) = svc.cluster_ip {
            let entry = self
                .state
                .tables
                .get_mut(&(family, kube_table_name.to_string()))
                .expect("kube table just initialized");
            let dnat_chain = entry
                .chains
                .get_mut("clusterip-dnat")
                .expect("clusterip-dnat chain just initialized");
            for port in &svc.ports {
                let rule = NftRule {
                    handle: None,
                    exprs: vec![
                        NftExpr::Payload {
                            dreg: 1,
                            base: 0,
                            offset: 16,
                            len: 4,
                        },
                        NftExpr::Cmp {
                            sreg: 1,
                            op: 0,
                            data: cluster_ip.octets().to_vec(),
                        },
                        NftExpr::Immediate {
                            dreg: 2,
                            data: vec![],
                        },
                        NftExpr::Nat {
                            nat_type: 1, // DNAT
                            sreg_addr: 2,
                            sreg_port: 0xFFFFFFFF,
                        },
                    ],
                    comment: Some(format!("{}/{}", svc.name, port.name)),
                };
                dnat_chain.rules.push(rule);
            }
        }
        // DNS: first-write-wins to avoid clobbering an existing record.
        self.state.dns_records.entry(format!(
            "{}.{}.svc.cluster.local",
            svc.name, svc.namespace
        )).or_insert(svc.cluster_ip.unwrap_or(Ipv4Addr::UNSPECIFIED));
        self
    }

    /// Register a NetworkPolicy (NSG) for a pod.
    pub fn add_policy(&mut self, pod_uid: &str, rules: &[NsgRule]) -> &mut Self {
        let family = NftFamily::Ip;
        let table_name = format!("nsg_{}", sanitize(pod_uid));
        let mut table = NftTable::new(&table_name, family)
            .with_chain(NftChain::regular("ingress", NftChainKind::Filter));
        for (i, r) in rules.iter().enumerate() {
            let mut exprs = Vec::new();
            if r.action == NsgAction::Deny {
                exprs.push(NftExpr::Drop);
            } else {
                exprs.push(NftExpr::Accept);
            }
            table.chains.get_mut("ingress").unwrap().rules.push(
                NftRule {
                    handle: Some(i as u64 + 1),
                    exprs,
                    comment: Some(r.name.clone()),
                },
            );
        }
        self.state.tables.insert((family, table_name), table);
        self
    }

    /// Patch a pod's network info (IP + veth). Local pod connectivity (veth,
    /// `/32` route) is applied by `attach_pod`; this only records the pod in
    /// the declarative state.
    pub fn add_pod(&mut self, pod: PodNetwork) -> &mut Self {
        self.state.pods.insert(pod.pod_uid.clone(), pod);
        self
    }

    /// Add a reconcile-managed route (remote pod route or static route).
    pub fn add_route(&mut self, route: RouteSpec) -> &mut Self {
        self.state.routes.insert(route.key(), route);
        self
    }

    /// Consume the builder, returning the final state.
    pub fn build(self) -> NetmuxState {
        self.state
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipam::Ipv4Cidr;

    #[test]
    fn reconcile_no_change() {
        let mut s = NetmuxState::new();
        s.tables.insert(
            (NftFamily::Ip, "filter".to_string()),
            NftTable::new("filter", NftFamily::Ip),
        );
        let ops = reconcile(&s, &s);
        assert!(ops.is_empty());
    }

    #[test]
    fn reconcile_adds_table() {
        let mut desired = NetmuxState::new();
        desired.tables.insert(
            (NftFamily::Ip, "kube".to_string()),
            NftTable::new("kube", NftFamily::Ip),
        );
        let current = NetmuxState::new();
        let ops = reconcile(&desired, &current);
        assert!(ops.iter().any(|op| matches!(
            op,
            NetlinkOp::AddTable { name, .. } if name == "kube"
        )));
    }

    #[test]
    fn reconcile_removes_table() {
        let mut current = NetmuxState::new();
        current.tables.insert(
            (NftFamily::Ip, "stale".to_string()),
            NftTable::new("stale", NftFamily::Ip),
        );
        let mut desired = NetmuxState::new();
        let ops = reconcile(&desired, &current);
        assert!(ops.iter().any(|op| matches!(
            op,
            NetlinkOp::DelTable { name, .. } if name == "stale"
        )));
    }

    #[test]
    fn reconcile_adds_chain() {
        let mut desired = NetmuxState::new();
        let mut t = NftTable::new("filter", NftFamily::Ip);
        t.chains.insert(
            "input".to_string(),
            NftChain::base("input", NftChainKind::Filter, NftHook::Input, 0, NftPolicy::Accept),
        );
        desired.tables.insert((NftFamily::Ip, "filter".to_string()), t);
        let mut current = NetmuxState::new();
        current.tables.insert(
            (NftFamily::Ip, "filter".to_string()),
            NftTable::new("filter", NftFamily::Ip),
        );
        let ops = reconcile(&desired, &current);
        assert!(ops.iter().any(|op| matches!(
            op,
            NetlinkOp::AddChain { chain, .. } if chain.name == "input"
        )));
    }

    #[test]
    fn reconcile_removes_chain() {
        let mut desired = NetmuxState::new();
        desired.tables.insert(
            (NftFamily::Ip, "filter".to_string()),
            NftTable::new("filter", NftFamily::Ip),
        );
        let mut current = NetmuxState::new();
        let mut t = NftTable::new("filter", NftFamily::Ip);
        t.chains.insert(
            "stale".to_string(),
            NftChain::regular("stale", NftChainKind::Filter),
        );
        current.tables.insert((NftFamily::Ip, "filter".to_string()), t);
        let ops = reconcile(&desired, &current);
        assert!(ops.iter().any(|op| matches!(
            op,
            NetlinkOp::DelChain { name, .. } if name == "stale"
        )));
    }

    #[test]
    fn reconcile_set_element_diff() {
        let mut desired = NetmuxState::new();
        let mut t1 = NftTable::new("kube", NftFamily::Ip);
        t1.sets.insert(
            "pods".to_string(),
            NftSet::ipv4("pods").with_ipv4(Ipv4Addr::new(10, 0, 0, 1)),
        );
        desired.tables.insert((NftFamily::Ip, "kube".to_string()), t1);
        let mut current = NetmuxState::new();
        let mut t2 = NftTable::new("kube", NftFamily::Ip);
        t2.sets.insert(
            "pods".to_string(),
            NftSet::ipv4("pods").with_ipv4(Ipv4Addr::new(10, 0, 0, 2)),
        );
        current.tables.insert((NftFamily::Ip, "kube".to_string()), t2);
        let ops = reconcile(&desired, &current);
        // Expect delset + addset (replace)
        let del = ops.iter().any(|op| matches!(op, NetlinkOp::DelSet { name, .. } if name == "pods"));
        let add = ops.iter().any(|op| matches!(op, NetlinkOp::AddSet { set, .. } if set.name == "pods"));
        assert!(del, "expected DelSet, got {:?}", ops);
        assert!(add, "expected AddSet, got {:?}", ops);
    }

    #[test]
    fn reconcile_no_op_for_unchanged_set() {
        let mut desired = NetmuxState::new();
        let mut t1 = NftTable::new("kube", NftFamily::Ip);
        t1.sets.insert(
            "pods".to_string(),
            NftSet::ipv4("pods").with_ipv4(Ipv4Addr::new(10, 0, 0, 1)),
        );
        desired.tables.insert((NftFamily::Ip, "kube".to_string()), t1);
        let mut current = NetmuxState::new();
        let mut t2 = NftTable::new("kube", NftFamily::Ip);
        t2.sets.insert(
            "pods".to_string(),
            NftSet::ipv4("pods").with_ipv4(Ipv4Addr::new(10, 0, 0, 1)),
        );
        current.tables.insert((NftFamily::Ip, "kube".to_string()), t2);
        let ops = reconcile(&desired, &current);
        assert!(!ops.iter().any(|op| matches!(op, NetlinkOp::DelSet { .. })));
    }

    #[test]
    fn apply_dry_run_updates_current() {
        let mut engine = Netmux::unconnected();
        let op = NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "test".into(),
        };
        engine.apply(&[op], true).unwrap();
        assert!(engine
            .current()
            .tables
            .contains_key(&(NftFamily::Ip, "test".to_string())));
    }

    #[test]
    fn apply_without_socket_fails() {
        let mut engine = Netmux::unconnected();
        let op = NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "x".into(),
        };
        let res = engine.apply(&[op], false);
        assert!(res.is_err());
    }

    #[test]
    fn reconcile_and_apply_no_diff() {
        let mut engine = Netmux::unconnected();
        let state = NetmuxState::new();
        let n = engine.reconcile_and_apply(&state, true).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn reconcile_and_apply_with_diff() {
        let mut engine = Netmux::unconnected();
        let mut desired = NetmuxState::new();
        desired.tables.insert(
            (NftFamily::Ip, "kube".to_string()),
            NftTable::new("kube", NftFamily::Ip),
        );
        let n = engine.reconcile_and_apply(&desired, true).unwrap();
        assert!(n >= 1);
        assert!(engine
            .current()
            .tables
            .contains_key(&(NftFamily::Ip, "kube".to_string())));
    }

    #[test]
    fn seed_current_short_circuits_diff() {
        let mut engine = Netmux::unconnected();
        let mut state = NetmuxState::new();
        state.tables.insert(
            (NftFamily::Ip, "kube".to_string()),
            NftTable::new("kube", NftFamily::Ip),
        );
        engine.seed_current(state.clone());
        let n = engine.reconcile_and_apply(&state, true).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn builder_adds_vnet() {
        let mut b = NetmuxBuilder::new();
        b.add_vnet(VNetSpec {
            name: "default".into(),
            cidr: Ipv4Cidr::parse("10.42.0.0/16").unwrap(),
            internet_access: true,
        });
        let state = b.build();
        assert!(state.tables.iter().any(|((_, n), _)| n == "vnet_default"));
        assert!(state.ip_pools.contains_key("default-pods"));
    }

    #[test]
    fn builder_adds_service() {
        let mut b = NetmuxBuilder::new();
        b.add_service(ServiceSpec {
            name: "web".into(),
            namespace: "default".into(),
            kind: "ClusterIP".into(),
            cluster_ip: Some(Ipv4Addr::new(10, 96, 0, 10)),
            ports: vec![ServicePortSpec {
                name: "http".into(),
                port: 80,
                target_port: 8080,
                node_port: None,
                protocol: "TCP".into(),
            }],
        });
        let state = b.build();
        assert!(state.tables.contains_key(&(NftFamily::Ip, "kube".to_string())));
        let kube = state.table(NftFamily::Ip, "kube").unwrap();
        assert!(kube.chains.contains_key("prerouting"));
        assert!(kube.chains.contains_key("postrouting"));
        assert!(state
            .dns_records
            .contains_key("web.default.svc.cluster.local"));
    }

    #[test]
    fn builder_adds_policy() {
        let mut b = NetmuxBuilder::new();
        b.add_policy(
            "pod-123",
            &[NsgRule {
                name: "deny-all".into(),
                priority: 1000,
                action: NsgAction::Deny,
                src_cidrs: vec!["0.0.0.0/0".into()],
                dst_cidrs: vec!["0.0.0.0/0".into()],
            }],
        );
        let state = b.build();
        assert!(state.tables.contains_key(&(NftFamily::Ip, "nsg_pod-123".to_string())));
    }

    #[test]
    fn builder_adds_pod() {
        let mut b = NetmuxBuilder::new();
        b.add_pod(PodNetwork {
            pod_uid: "uid-1".into(),
            pod_ip: Ipv4Addr::new(10, 42, 0, 5),
            veth_host: "veth123".into(),
            veth_peer: "eth0".into(),
            vnet: Some("default".into()),
        });
        let state = b.build();
        assert!(state.pods.contains_key("uid-1"));
    }

    #[test]
    fn reconcile_adds_and_removes_route() {
        let mut desired = NetmuxState::new();
        let r = RouteSpec::host_via(Ipv4Addr::new(10, 42, 1, 5), Ipv4Addr::new(192, 168, 1, 2));
        desired.routes.insert(r.key(), r.clone());
        let ops = reconcile(&desired, &NetmuxState::new());
        assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddRoute { .. })));
        // And cleanup when it disappears from desired.
        let mut current = NetmuxState::new();
        current.routes.insert(r.key(), r);
        let ops = reconcile(&NetmuxState::new(), &current);
        assert!(ops.iter().any(|op| matches!(op, NetlinkOp::DelRoute { .. })));
    }

    #[test]
    fn sanitize_replaces_special_chars() {
        assert_eq!(sanitize("kube-system"), "kube-system");
        assert_eq!(sanitize("kube.system"), "kube_system");
        assert_eq!(sanitize("ns/name"), "ns_name");
    }

    #[test]
    fn reconcile_generation_increments() {
        let mut engine = Netmux::unconnected();
        let g0 = engine.current().generation;
        let mut desired = NetmuxState::new();
        desired.tables.insert(
            (NftFamily::Ip, "x".to_string()),
            NftTable::new("x", NftFamily::Ip),
        );
        engine.reconcile_and_apply(&desired, true).unwrap();
        assert!(engine.current().generation > g0);
    }

    #[test]
    fn apply_one_add_table_creates_entry() {
        let mut engine = Netmux::unconnected();
        let op = NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "t".into(),
        };
        engine.apply_one_to_state(&op);
        assert!(engine.current.tables.contains_key(&(NftFamily::Ip, "t".to_string())));
    }

    #[test]
    fn apply_one_del_table_removes_entry() {
        let mut engine = Netmux::unconnected();
        let op1 = NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "t".into(),
        };
        let op2 = NetlinkOp::DelTable {
            family: NftFamily::Ip,
            name: "t".into(),
        };
        engine.apply_one_to_state(&op1);
        engine.apply_one_to_state(&op2);
        assert!(!engine.current.tables.contains_key(&(NftFamily::Ip, "t".to_string())));
    }

    #[test]
    fn apply_one_chain_ops() {
        let mut engine = Netmux::unconnected();
        engine.apply_one_to_state(&NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "t".into(),
        });
        engine.apply_one_to_state(&NetlinkOp::AddChain {
            family: NftFamily::Ip,
            table: "t".into(),
            chain: NftChain::base("c", NftChainKind::Filter, NftHook::Input, 0, NftPolicy::Accept),
        });
        let op2 = NetlinkOp::DelChain {
            family: NftFamily::Ip,
            table: "t".into(),
            name: "c".into(),
        };
        engine.apply_one_to_state(&op2);
        let t = engine.current.tables.get(&(NftFamily::Ip, "t".to_string())).unwrap();
        assert!(!t.chains.contains_key("c"));
    }

    #[test]
    fn reconcile_chain_property_change_triggers_recreate() {
        let mut desired = NetmuxState::new();
        let mut t1 = NftTable::new("filter", NftFamily::Ip);
        t1.chains.insert(
            "input".to_string(),
            NftChain::base("input", NftChainKind::Filter, NftHook::Input, 0, NftPolicy::Drop),
        );
        desired.tables.insert((NftFamily::Ip, "filter".to_string()), t1);
        let mut current = NetmuxState::new();
        let mut t2 = NftTable::new("filter", NftFamily::Ip);
        t2.chains.insert(
            "input".to_string(),
            NftChain::base("input", NftChainKind::Filter, NftHook::Input, 0, NftPolicy::Accept),
        );
        current.tables.insert((NftFamily::Ip, "filter".to_string()), t2);
        let ops = reconcile(&desired, &current);
        assert!(ops.iter().any(|op| matches!(op, NetlinkOp::DelChain { .. })));
        assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddChain { .. })));
    }

    #[test]
    fn reconcile_pod_registered_then_removed() {
        let mut desired = NetmuxState::new();
        let mut current = NetmuxState::new();
        let pod = PodNetwork {
            pod_uid: "u1".into(),
            pod_ip: Ipv4Addr::new(10, 0, 0, 1),
            veth_host: "vh".into(),
            veth_peer: "vp".into(),
            vnet: None,
        };
        current.pods.insert("u1".into(), pod.clone());
        let ops = reconcile(&desired, &current);
        // The pod is in current only, so the diff should pick up the removal
        // for bookkeeping (no netlink op, just debug log)
        assert!(ops.is_empty());
    }

    #[test]
    fn reconcile_dns_record_added() {
        let mut desired = NetmuxState::new();
        desired
            .dns_records
            .insert("web".into(), Ipv4Addr::new(10, 96, 0, 10));
        let current = NetmuxState::new();
        let ops = reconcile(&desired, &current);
        // DNS records are bookkeeping — no netlink op, no panic
        assert!(ops.is_empty());
    }

    #[test]
    fn default_engine_is_unconnected() {
        let e = Netmux::default();
        assert!(e.sock.is_none());
    }
}
