//! # Declarative Network Model
//!
//! All network state is expressed as immutable data structures that the
//! engine diffs and applies to the kernel. This is the single source of
//! truth for what the network should look like at any given tick.
//!
//! ## Functional Design
//!
//! - All types are `Clone + PartialEq + Debug` — trivially diffable
//! - The engine holds a `NetmuxState` (current) and a planner produces
//!   a `NetmuxState` (desired). A pure function in [`crate::engine`]
//!   computes the diff.
//! - No `Arc<Mutex>` in the data model — concurrency is the engine's
//!   concern, not the model's
//!
//! ## What the model covers
//!
//! - **Tables** — nftables tables per address family
//! - **Chains** — base, regular, and hook chains (prerouting, forward, etc.)
//! - **Rules** — composed of expressions (match, verdict, NAT, counter, lookup)
//! - **Sets** — named sets of keys (IPv4/IPv6 addresses) for fast membership
//! - **Counters** — packet/byte counters attached to rules or used standalone
//! - **VNet** — virtual network with isolation policies
//! - **NSG** — network security group rules
//! - **Service** — DNAT rules for ClusterIP/NodePort/LoadBalancer

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::ipam::{IpPool, Ipv4Cidr, Ipv6Pool};

// ═══════════════════════════════════════════════════════════════════════════
// Nftables family / chain / hook enums
// ═══════════════════════════════════════════════════════════════════════════

/// Netfilter address family.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NftFamily {
    /// IPv4 only.
    #[default]
    Ip,
    /// IPv6 only.
    Ip6,
    /// Both IPv4 and IPv6 (inet).
    Inet,
    /// Netdev (ingress).
    Netdev,
}

impl NftFamily {
    /// u8 value used in nfgenmsg.
    pub fn as_u8(self) -> u8 {
        match self {
            NftFamily::Ip => 2,
            NftFamily::Ip6 => 10,
            NftFamily::Inet => 1,
            NftFamily::Netdev => 5,
        }
    }

    /// Table-name suffix (RFC convention).
    pub fn suffix(self) -> &'static str {
        match self {
            NftFamily::Ip => "ip",
            NftFamily::Ip6 => "ip6",
            NftFamily::Inet => "inet",
            NftFamily::Netdev => "netdev",
        }
    }
}

/// Nftables chain kind (determines what hooks it can be attached to).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NftChainKind {
    /// Filter chains — accept/drop decisions.
    Filter,
    /// NAT chains — address translation.
    Nat,
    /// Route chains — early packet marking.
    Route,
}

impl NftChainKind {
    /// String value for NFTA_CHAIN_TYPE attribute.
    pub fn as_str(self) -> &'static str {
        match self {
            NftChainKind::Filter => "filter",
            NftChainKind::Nat => "nat",
            NftChainKind::Route => "route",
        }
    }
}

/// Netfilter hook point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NftHook {
    Prerouting,
    Input,
    Forward,
    Output,
    Postrouting,
    Ingress,
    Egress,
}

impl NftHook {
    /// u32 value used in NFTA_CHAIN_HOOKNUM.
    pub fn as_u32(self) -> u32 {
        match self {
            NftHook::Prerouting => 0,
            NftHook::Input => 1,
            NftHook::Forward => 2,
            NftHook::Output => 3,
            NftHook::Postrouting => 4,
            NftHook::Ingress => 5,
            NftHook::Egress => 6,
        }
    }

    /// Hook name (for display).
    pub fn name(self) -> &'static str {
        match self {
            NftHook::Prerouting => "prerouting",
            NftHook::Input => "input",
            NftHook::Forward => "forward",
            NftHook::Output => "output",
            NftHook::Postrouting => "postrouting",
            NftHook::Ingress => "ingress",
            NftHook::Egress => "egress",
        }
    }
}

/// Default chain policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NftPolicy {
    Accept,
    Drop,
}

impl NftPolicy {
    /// u32 verdict value (NFTA_CHAIN_POLICY).
    pub fn as_u32(self) -> u32 {
        match self {
            NftPolicy::Accept => 0x00000001, // NF_ACCEPT
            NftPolicy::Drop => 0x00000000,   // NF_DROP
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Rule expressions
// ═══════════════════════════════════════════════════════════════════════════

/// A single rule expression. Composed into a list inside an NftRule.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NftExpr {
    /// Match on packet meta (l4proto, etc.).
    Meta {
        /// NFT_META_* value.
        kind: u32,
        /// Comparison op (0=eq, 1=neq, 2=lt, 3=lte, 4=gt, 5=gte).
        op: u32,
        /// Value to compare against.
        value: u32,
    },
    /// Compare register against a value.
    Cmp {
        /// Source register.
        sreg: u32,
        /// Comparison op.
        op: u32,
        /// Value bytes.
        data: Vec<u8>,
    },
    /// Load bytes from a packet header into a register.
    Payload {
        /// Destination register.
        dreg: u32,
        /// Payload base: 0=network, 1=transport, 2=link.
        base: u32,
        /// Offset from the base.
        offset: u32,
        /// Number of bytes to load.
        len: u32,
    },
    /// Look up register value in a named set.
    Lookup {
        /// Set name.
        set: String,
        /// Source register.
        sreg: u32,
    },
    /// Load an immediate value into a register.
    Immediate {
        /// Destination register.
        dreg: u32,
        /// Value bytes.
        data: Vec<u8>,
    },
    /// NAT — DNAT or SNAT.
    Nat {
        /// 0=DNAT, 1=SNAT.
        nat_type: u32,
        /// Address register (or 0xFFFFFFFF for none).
        sreg_addr: u32,
        /// Port register (or 0xFFFFFFFF for none).
        sreg_port: u32,
    },
    /// Masquerade (SNAT with the outgoing interface address).
    Masquerade,
    /// Accept verdict.
    Accept,
    /// Drop verdict.
    Drop,
    /// Return from the current chain.
    Return,
    /// Jump to a named chain.
    Jump(String),
    /// Goto a named chain.
    Goto(String),
}

// ═══════════════════════════════════════════════════════════════════════════
// Rules
// ═══════════════════════════════════════════════════════════════════════════

/// A single nftables rule — ordered list of expressions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NftRule {
    /// Optional handle (kernel-assigned if None).
    pub handle: Option<u64>,
    /// Expressions in evaluation order.
    pub exprs: Vec<NftExpr>,
    /// Optional comment for debugging.
    pub comment: Option<String>,
}

impl NftRule {
    /// Create a simple accept rule.
    pub fn accept() -> Self {
        Self {
            handle: None,
            exprs: vec![NftExpr::Accept],
            comment: None,
        }
    }

    /// Create a simple drop rule.
    pub fn drop() -> Self {
        Self {
            handle: None,
            exprs: vec![NftExpr::Drop],
            comment: None,
        }
    }

    /// Create a jump rule.
    pub fn jump(chain: impl Into<String>) -> Self {
        Self {
            handle: None,
            exprs: vec![NftExpr::Jump(chain.into())],
            comment: None,
        }
    }

    /// Attach a comment to this rule.
    pub fn with_comment(mut self, c: impl Into<String>) -> Self {
        self.comment = Some(c.into());
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Chains
// ═══════════════════════════════════════════════════════════════════════════

/// A single nftables chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NftChain {
    pub name: String,
    pub kind: NftChainKind,
    /// None for regular chains (no hook); Some for base chains.
    pub hook: Option<NftHook>,
    /// Hook priority (lower runs first; negative for NAT).
    pub priority: i32,
    pub policy: NftPolicy,
    /// Rules in evaluation order.
    pub rules: Vec<NftRule>,
}

impl NftChain {
    /// Create a base (hook-attached) chain.
    pub fn base(
        name: impl Into<String>,
        kind: NftChainKind,
        hook: NftHook,
        priority: i32,
        policy: NftPolicy,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            hook: Some(hook),
            priority,
            policy,
            rules: vec![],
        }
    }

    /// Create a regular (non-hook) chain.
    pub fn regular(name: impl Into<String>, kind: NftChainKind) -> Self {
        Self {
            name: name.into(),
            kind,
            hook: None,
            priority: 0,
            policy: NftPolicy::Accept,
            rules: vec![],
        }
    }

    /// Add a rule to this chain.
    pub fn with_rule(mut self, rule: NftRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Add multiple rules.
    pub fn with_rules(mut self, rules: impl IntoIterator<Item = NftRule>) -> Self {
        self.rules.extend(rules);
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Sets and Counters
// ═══════════════════════════════════════════════════════════════════════════

/// A named nftables set of keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NftSet {
    pub name: String,
    /// Key type as a string descriptor ("ipv4_addr", "ipv6_addr", etc.).
    pub key_type: String,
    /// Key length in bytes.
    pub key_len: u32,
    /// Data length (0 for set, >0 for map).
    pub data_len: u32,
    /// Element keys (raw bytes, exactly `key_len` each).
    pub elements: Vec<Vec<u8>>,
}

impl NftSet {
    /// Create an IPv4-address set.
    pub fn ipv4(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            key_type: "ipv4_addr".into(),
            key_len: 4,
            data_len: 0,
            elements: vec![],
        }
    }

    /// Create an IPv6-address set.
    pub fn ipv6(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            key_type: "ipv6_addr".into(),
            key_len: 16,
            data_len: 0,
            elements: vec![],
        }
    }

    /// Add an IPv4 element.
    pub fn with_ipv4(mut self, ip: Ipv4Addr) -> Self {
        self.elements.push(ip.octets().to_vec());
        self
    }

    /// Add an IPv6 element.
    pub fn with_ipv6(mut self, ip: Ipv6Addr) -> Self {
        self.elements.push(ip.octets().to_vec());
        self
    }

    /// Add raw element bytes.
    pub fn with_element(mut self, bytes: Vec<u8>) -> Self {
        self.elements.push(bytes);
        self
    }
}

/// A named packet/byte counter object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NftCounter {
    pub name: String,
}

impl NftCounter {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tables
// ═══════════════════════════════════════════════════════════════════════════

/// A complete nftables table — chains, sets, counters, all in one struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct NftTable {
    pub name: String,
    pub family: NftFamily,
    /// Chains keyed by name.
    pub chains: BTreeMap<String, NftChain>,
    /// Sets keyed by name.
    pub sets: BTreeMap<String, NftSet>,
    /// Counters keyed by name.
    pub counters: BTreeMap<String, NftCounter>,
}

impl NftTable {
    /// Create a new empty table.
    pub fn new(name: impl Into<String>, family: NftFamily) -> Self {
        Self {
            name: name.into(),
            family,
            chains: BTreeMap::new(),
            sets: BTreeMap::new(),
            counters: BTreeMap::new(),
        }
    }

    /// Add a chain.
    pub fn with_chain(mut self, chain: NftChain) -> Self {
        self.chains.insert(chain.name.clone(), chain);
        self
    }

    /// Add a set.
    pub fn with_set(mut self, set: NftSet) -> Self {
        self.sets.insert(set.name.clone(), set);
        self
    }

    /// Add a counter.
    pub fn with_counter(mut self, counter: NftCounter) -> Self {
        self.counters.insert(counter.name.clone(), counter);
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// VNet / NSG / Service — high-level network policy
// ═══════════════════════════════════════════════════════════════════════════

/// A virtual network: CIDR + isolation + internet access.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VNetSpec {
    pub name: String,
    pub cidr: Ipv4Cidr,
    pub internet_access: bool,
}

/// An NSG rule: allow or deny traffic between CIDRs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NsgRule {
    pub name: String,
    pub priority: i32,
    /// "allow" or "deny".
    pub action: NsgAction,
    pub src_cidrs: Vec<String>,
    pub dst_cidrs: Vec<String>,
}

/// NSG rule action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NsgAction {
    Allow,
    Deny,
}

impl NsgAction {
    pub fn as_str(self) -> &'static str {
        match self {
            NsgAction::Allow => "allow",
            NsgAction::Deny => "deny",
        }
    }
}

/// A service exposure (ClusterIP / NodePort / LoadBalancer).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceSpec {
    pub name: String,
    pub namespace: String,
    /// "ClusterIP", "NodePort", "LoadBalancer", "ExternalName".
    pub kind: String,
    pub cluster_ip: Option<Ipv4Addr>,
    pub ports: Vec<ServicePortSpec>,
}

/// A single port on a service.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServicePortSpec {
    pub name: String,
    pub port: u16,
    pub target_port: u16,
    pub node_port: Option<u16>,
    pub protocol: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// Network state — the engine's view of the world
// ═══════════════════════════════════════════════════════════════════════════

/// A pod's IP and routing info (kept in the engine state).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PodNetwork {
    pub pod_uid: String,
    pub pod_ip: Ipv4Addr,
    pub veth_host: String,
    pub veth_peer: String,
    pub vnet: Option<String>,
}

/// A veth pair handle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VethPair {
    pub host_name: String,
    pub peer_name: String,
    pub host_ifindex: u32,
    pub peer_ifindex: u32,
}

/// The complete network state — what the engine thinks is currently in place.
#[derive(Debug, Clone, Default)]
pub struct NetmuxState {
    /// All nftables tables, keyed by `(family, name)`.
    pub tables: BTreeMap<(NftFamily, String), NftTable>,
    /// IP pools keyed by name (pod CIDR, service CIDR, subnets).
    pub ip_pools: BTreeMap<String, IpPool>,
    /// IPv6 pools keyed by name.
    pub ip6_pools: BTreeMap<String, Ipv6Pool>,
    /// Per-pod network info keyed by pod UID.
    pub pods: BTreeMap<String, PodNetwork>,
    /// Veth pairs keyed by host-side interface name.
    pub veths: BTreeMap<String, VethPair>,
    /// DNS records (hostname -> IPv4).
    pub dns_records: HashMap<String, Ipv4Addr>,
    /// Host routes to pod IPs (pod_ip -> host veth ifindex).
    pub host_routes: BTreeMap<Ipv4Addr, u32>,
    /// Generation counter (monotonic).
    pub generation: u64,
}

impl NetmuxState {
    /// Create an empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge desired state into this state (functional, returns a new state).
    pub fn merged(&self, desired: &NetmuxState) -> NetmuxState {
        NetmuxState {
            tables: desired.tables.clone(),
            ip_pools: desired.ip_pools.clone(),
            ip6_pools: desired.ip6_pools.clone(),
            pods: desired.pods.clone(),
            veths: desired.veths.clone(),
            dns_records: desired.dns_records.clone(),
            host_routes: desired.host_routes.clone(),
            generation: desired.generation,
        }
    }

    /// Find a table.
    pub fn table(&self, family: NftFamily, name: &str) -> Option<&NftTable> {
        self.tables.get(&(family, name.to_string()))
    }

    /// Find a pod's network info.
    pub fn pod(&self, pod_uid: &str) -> Option<&PodNetwork> {
        self.pods.get(pod_uid)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// NetlinkOp — what the engine executes to update kernel state
// ═══════════════════════════════════════════════════════════════════════════

/// A single nftables operation the engine must execute.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NetlinkOp {
    AddTable { family: NftFamily, name: String },
    DelTable { family: NftFamily, name: String },
    AddChain { family: NftFamily, table: String, chain: NftChain },
    DelChain { family: NftFamily, table: String, name: String },
    AddRule { family: NftFamily, table: String, chain: String, rule: NftRule },
    DelRule { family: NftFamily, table: String, chain: String, handle: u64 },
    AddSet { family: NftFamily, table: String, set: NftSet },
    DelSet { family: NftFamily, table: String, name: String },
    SetFlush { family: NftFamily, table: String, name: String },
    AddCounter { family: NftFamily, table: String, counter: NftCounter },
    DelCounter { family: NftFamily, table: String, name: String },
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_as_u8() {
        assert_eq!(NftFamily::Ip.as_u8(), 2);
        assert_eq!(NftFamily::Ip6.as_u8(), 10);
        assert_eq!(NftFamily::Inet.as_u8(), 1);
    }

    #[test]
    fn hook_names() {
        assert_eq!(NftHook::Prerouting.name(), "prerouting");
        assert_eq!(NftHook::Postrouting.as_u32(), 4);
    }

    #[test]
    fn chain_kind_str() {
        assert_eq!(NftChainKind::Filter.as_str(), "filter");
        assert_eq!(NftChainKind::Nat.as_str(), "nat");
    }

    #[test]
    fn nft_set_ipv4() {
        let s = NftSet::ipv4("myset").with_ipv4(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(s.key_len, 4);
        assert_eq!(s.elements.len(), 1);
    }

    #[test]
    fn nft_rule_accept() {
        let r = NftRule::accept();
        assert_eq!(r.exprs.len(), 1);
    }

    #[test]
    fn nft_table_with_chain() {
        let t = NftTable::new("filter", NftFamily::Ip)
            .with_chain(NftChain::base(
                "input",
                NftChainKind::Filter,
                NftHook::Input,
                0,
                NftPolicy::Accept,
            ));
        assert_eq!(t.chains.len(), 1);
    }

    #[test]
    fn netmux_state_merged() {
        let mut desired = NetmuxState::new();
        desired.generation = 42;
        let current = NetmuxState::new();
        let merged = current.merged(&desired);
        assert_eq!(merged.generation, 42);
    }
}
