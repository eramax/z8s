//! # Network Module
//!
//! Unified, declarative network engine for z8s.
//!
//! - [`Netmux`] — the engine facade: holds the current state, applies ops,
//!   computes diffs from desired state
//! - [`NetmuxState`] — the declarative model (tables, chains, rules, sets,
//!   pods, routes, DNS records)
//! - [`reconcile`] — pure function: `desired`, `current` → `Vec<NetlinkOp>`
//! - [`NetmuxBuilder`] — convenience builder for DB-driven state assembly
//!
//! ## Design
//!
//! - **No external dependencies** for nftables encoding — we talk to the
//!   kernel via rustix netlink, encoding nftables netlink payloads by hand
//!   in [`crate::syscalls`]
//! - **Functional core, imperative shell** — `reconcile` is pure;
//!   `Netmux::apply` is the only side-effecting call
//! - **Declarative reconciliation** — callers patch a `NetmuxState`, the
//!   engine emits the minimal set of netlink ops to reach it
//! - **Automatic cleanup** — when resources are removed from the desired
//!   state, the next reconcile tick produces `Del*` ops automatically
//!
//! ## Module structure
//!
//! - [`ipam`] — IPv4/IPv6 pools (pure)
//! - [`model`] — declarative types (`NftTable`, `NftChain`, `NftRule`,
//!   `NftSet`, `NftExpr`, `NftFamily`, …, `NetmuxState`, `NetlinkOp`)
//! - [`syscalls`] — nftables netlink encoding, `NlSocket`
//! - [`engine`] — `Netmux` engine, `reconcile`, `NetmuxBuilder`

// ═══════════════════════════════════════════════════════════════════════════
// Modules
// ═══════════════════════════════════════════════════════════════════════════

pub mod dns;
pub mod engine;
pub mod ipam;
pub mod model;
pub mod plan;
pub mod rtnetlink;
pub mod nftables;

// ═══════════════════════════════════════════════════════════════════════════
// Re-exports — the public API
// ═══════════════════════════════════════════════════════════════════════════

pub use dns::{DnsConfig, DnsServer, DnsZone};
pub use engine::{reconcile, Netmux, NetmuxBuilder};
pub use ipam::{IpPool, Ipv4Cidr, Ipv6Pool};
pub use model::{
    NetlinkOp, NetmuxState, NftChain, NftChainKind, NftCounter, NftExpr, NftFamily, NftHook,
    NftPolicy, NftRule, NftSet, NftTable, NsgAction, NsgRule, PodNetwork, RouteSpec,
    ServicePortSpec, ServiceSpec, VNetSpec, VethPair,
};
pub use plan::{plan, PlanConfig};
pub use rtnetlink::{host_veth_name, peer_veth_name, RouteSocket};
pub use nftables::{encode_op, nfgen_header, send_batch, NlSocket, NlaBuf};

// ═══════════════════════════════════════════════════════════════════════════
// NetworkEngine trait — controllers/services depend on this
// ═══════════════════════════════════════════════════════════════════════════

use std::sync::Arc;

/// Trait that controllers and services use to drive the network engine.
///
/// The engine itself is the only implementation; the trait exists so the
/// network module is decoupled from its callers.
#[async_trait::async_trait]
pub trait NetworkEngine: Send + Sync {
    /// Reconcile the current state with the desired state. Returns the
    /// number of ops applied.
    async fn reconcile(&self, desired: &NetmuxState) -> anyhow::Result<usize>;

    /// Get a snapshot of the current state.
    fn current(&self) -> NetmuxState;

    /// Inject a pre-known state (for restart, dry-run, or test setup).
    fn seed(&self, state: NetmuxState);
}

#[async_trait::async_trait]
impl NetworkEngine for Arc<tokio::sync::Mutex<Netmux>> {
    async fn reconcile(&self, desired: &NetmuxState) -> anyhow::Result<usize> {
        let mut g = self.lock().await;
        g.reconcile_and_apply(desired, true)
    }
    fn current(&self) -> NetmuxState {
        // Note: this is best-effort; callers that need a consistent snapshot
        // should hold the lock.
        if let Ok(g) = self.try_lock() {
            g.current().clone()
        } else {
            NetmuxState::new()
        }
    }
    fn seed(&self, state: NetmuxState) {
        if let Ok(mut g) = self.try_lock() {
            g.seed_current(state);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Pod lifecycle — high-level wrappers used by the CRI module
// ═══════════════════════════════════════════════════════════════════════════

impl Netmux {
    /// Add a pod to the current state (called when a pod is scheduled).
    pub fn add_pod_to_state(&mut self, pod: PodNetwork) {
        let mut cur = self.current().clone();
        cur.pods.insert(pod.pod_uid.clone(), pod);
        self.seed_current(cur);
    }

    /// Remove a pod and its associated state (called when a pod terminates).
    pub fn remove_pod_from_state(&mut self, pod_uid: &str) {
        let mut cur = self.current().clone();
        cur.pods.remove(pod_uid);
        self.seed_current(cur);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Sysctl helpers — IP forwarding, reverse path filtering, etc.
// ═══════════════════════════════════════════════════════════════════════════

/// Enable IPv4 forwarding. Required for pods to reach the internet and
/// for pod-to-pod routing across nodes. Writes to `/proc/sys/net/ipv4/ip_forward`.
pub fn enable_ip_forward() -> anyhow::Result<()> {
    std::fs::write("/proc/sys/net/ipv4/ip_forward", b"1\n")
        .context("failed to enable ip_forward")
}

/// Enable reverse path filtering on all interfaces. Prevents IP spoofing
/// by verifying that incoming packets arrive on the interface that would
/// be used for routing to their source address.
pub fn enable_rp_filter() -> anyhow::Result<()> {
    for param in &[
        "net/ipv4/conf/all/rp_filter",
        "net/ipv4/conf/default/rp_filter",
    ] {
        let path = format!("/proc/sys/{}", param);
        if let Err(e) = std::fs::write(&path, b"1\n") {
            tracing::warn!("failed to set {}: {}", path, e);
        }
    }
    Ok(())
}

/// Harden sysctl settings for a node. Called during network init.
pub fn harden_sysctl() -> anyhow::Result<()> {
    enable_ip_forward()?;
    enable_rp_filter()?;
    enable_arp_announce()?;
    Ok(())
}

/// Enable ARP announce on all interfaces. Prevents IP spoofing by restricting
/// ARP announcements to the interface's own address range.
pub fn enable_arp_announce() -> anyhow::Result<()> {
    for param in &[
        "net/ipv4/conf/all/arp_announce",
        "net/ipv4/conf/default/arp_announce",
    ] {
        let path = format!("/proc/sys/{}", param);
        if let Err(e) = std::fs::write(&path, b"2\n") {
            tracing::warn!("failed to set {}: {}", path, e);
        }
    }
    Ok(())
}

use anyhow::Context;

// ═══════════════════════════════════════════════════════════════════════════
// Tests — re-export sanity
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexports_resolve() {
        let _f: NftFamily = NftFamily::Ip;
        let _h: NftHook = NftHook::Input;
        let _k: NftChainKind = NftChainKind::Filter;
        let _p: NftPolicy = NftPolicy::Accept;
        let _e: NftExpr = NftExpr::Accept;
        let _: NftTable = NftTable::new("t", NftFamily::Ip);
        let _: NftChain = NftChain::regular("c", NftChainKind::Filter);
        let _: NftRule = NftRule::accept();
        let _: NftSet = NftSet::ipv4("s");
        let _: NftCounter = NftCounter::new("c");
        let _: Ipv4Cidr = Ipv4Cidr::parse("10.0.0.0/24").unwrap();
        // NlSocket::open requires CAP_NET_ADMIN; just verify the type compiles
        // by referencing its constructor signature.
        let _: fn() -> std::io::Result<NlSocket> = NlSocket::open;
    }

    #[test]
    fn build_state_with_pod() {
        let mut b = NetmuxBuilder::new();
        b.add_pod(PodNetwork {
            pod_uid: "u1".into(),
            pod_ip: std::net::Ipv4Addr::new(10, 0, 0, 1),
            veth_host: "vh".into(),
            veth_peer: "vp".into(),
            vnet: None,
        });
        let state = b.build();
        assert!(state.pods.contains_key("u1"));
    }

    #[test]
    fn add_and_remove_pod_in_engine() {
        let mut e = Netmux::unconnected();
        e.add_pod_to_state(PodNetwork {
            pod_uid: "u1".into(),
            pod_ip: std::net::Ipv4Addr::new(10, 0, 0, 1),
            veth_host: "vh".into(),
            veth_peer: "vp".into(),
            vnet: None,
        });
        assert!(e.current().pods.contains_key("u1"));
        e.remove_pod_from_state("u1");
        assert!(!e.current().pods.contains_key("u1"));
    }
}
