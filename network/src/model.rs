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
    /// Bitwise transform: `dreg = (sreg & mask) ^ xor`. Used to mask an
    /// address down to its network prefix before a [`NftExpr::Cmp`] so a
    /// single rule can match a whole CIDR.
    Bitwise {
        /// Source register.
        sreg: u32,
        /// Destination register.
        dreg: u32,
        /// Length in bytes (4 for IPv4).
        len: u32,
        /// Mask bytes (the prefix mask).
        mask: Vec<u8>,
        /// XOR bytes (usually all-zero).
        xor: Vec<u8>,
    },
    /// Pseudo-random or incrementing number generator into a register.
    /// Used to spread connections across service backends (load balancing):
    /// `dreg = (random % modulus) + offset`.
    Numgen {
        /// Destination register.
        dreg: u32,
        /// Modulus (number of backends).
        modulus: u32,
        /// Offset added to the result.
        offset: u32,
    },
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
    /// Conntrack state match. Loads connection tracking state into a register
    /// so a subsequent `Cmp` can test against the bitmask.
    Conntrack {
        /// Destination register.
        dreg: u32,
        /// Conntrack key: 3=STATE, 7=DIRECTION, 5=STATUS.
        key: u32,
    },
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

    /// Build a rule directly from a list of expressions.
    pub fn from_exprs(exprs: Vec<NftExpr>) -> Self {
        Self {
            handle: None,
            exprs,
            comment: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Rule builders — compose expression lists for the common intents
// ═══════════════════════════════════════════════════════════════════════════
//
// These are pure functions that turn high-level intent (match this CIDR, DNAT
// to that backend, masquerade this pod range) into the ordered expression
// list nftables expects. Keeping them here means the planner stays readable
// and the wire encoding stays in `syscalls`.

/// nftables payload base: link-layer header.
pub const PAYLOAD_LL: u32 = 0;
/// nftables payload base: network (IP) header.
pub const PAYLOAD_NETWORK: u32 = 1;
/// nftables payload base: transport (TCP/UDP) header.
pub const PAYLOAD_TRANSPORT: u32 = 2;

/// Comparison op: equal.
pub const CMP_EQ: u32 = 0;

/// `meta` key for the layer-4 protocol byte.
pub const META_L4PROTO: u32 = 16;

/// DNAT nat type.
pub const NAT_DNAT: u32 = 1;
/// SNAT nat type.
pub const NAT_SNAT: u32 = 0;

/// IP protocol number for TCP.
pub const PROTO_TCP: u8 = 6;
/// IP protocol number for UDP.
pub const PROTO_UDP: u8 = 17;

/// Conntrack key: connection state bitmask.
pub const CT_STATE: u32 = 3;
/// Conntrack state: established.
pub const CT_STATE_ESTABLISHED: u32 = 1 << 1; // 2
/// Conntrack state: related.
pub const CT_STATE_RELATED: u32 = 1 << 2; // 4
/// Conntrack state: established + related bitmask.
pub const CT_STATE_ESTABLISHED_RELATED: u32 = CT_STATE_ESTABLISHED | CT_STATE_RELATED; // 6

/// Compute the 4-byte network mask for an IPv4 prefix length.
pub fn prefix_mask_v4(prefix: u8) -> [u8; 4] {
    let mask: u32 = if prefix == 0 {
        0
    } else if prefix >= 32 {
        !0
    } else {
        !0u32 << (32 - prefix)
    };
    mask.to_be_bytes()
}

/// Map a protocol string ("TCP"/"UDP") to its IP protocol number.
pub fn proto_number(proto: &str) -> u8 {
    match proto.to_ascii_uppercase().as_str() {
        "UDP" => PROTO_UDP,
        _ => PROTO_TCP,
    }
}

/// Expressions that match an IPv4 address field (`offset` 12=src, 16=dst)
/// against a CIDR. Returns an empty vec for a `/0` (match-all) CIDR.
pub fn match_cidr(offset: u32, cidr: &Ipv4Cidr) -> Vec<NftExpr> {
    if cidr.prefix == 0 {
        return vec![];
    }
    let mut out = vec![NftExpr::Payload {
        dreg: 1,
        base: PAYLOAD_NETWORK,
        offset,
        len: 4,
    }];
    if cidr.prefix < 32 {
        out.push(NftExpr::Bitwise {
            sreg: 1,
            dreg: 1,
            len: 4,
            mask: prefix_mask_v4(cidr.prefix).to_vec(),
            xor: vec![0u8; 4],
        });
    }
    out.push(NftExpr::Cmp {
        sreg: 1,
        op: CMP_EQ,
        data: cidr.network.octets().to_vec(),
    });
    out
}

/// Expressions matching the layer-4 protocol (TCP/UDP).
pub fn match_l4proto(proto: u8) -> Vec<NftExpr> {
    vec![
        NftExpr::Meta {
            kind: META_L4PROTO,
            op: CMP_EQ,
            value: proto as u32,
        },
        NftExpr::Cmp {
            sreg: 1,
            op: CMP_EQ,
            data: vec![proto],
        },
    ]
}

/// Expressions matching the transport destination port.
pub fn match_dport(port: u16) -> Vec<NftExpr> {
    vec![
        NftExpr::Payload {
            dreg: 1,
            base: PAYLOAD_TRANSPORT,
            offset: 2,
            len: 2,
        },
        NftExpr::Cmp {
            sreg: 1,
            op: CMP_EQ,
            data: port.to_be_bytes().to_vec(),
        },
    ]
}

/// Expressions performing a DNAT to `ip:port`. Loads the address into reg 1
/// and the port into reg 2, then issues the NAT verdict.
pub fn dnat_to(ip: Ipv4Addr, port: u16) -> Vec<NftExpr> {
    vec![
        NftExpr::Immediate {
            dreg: 1,
            data: ip.octets().to_vec(),
        },
        NftExpr::Immediate {
            dreg: 2,
            data: port.to_be_bytes().to_vec(),
        },
        NftExpr::Nat {
            nat_type: NAT_DNAT,
            sreg_addr: 1,
            sreg_port: 2,
        },
    ]
}

/// Build a ClusterIP DNAT rule for one backend. When `lb = Some((idx, total))`
/// and `total > 1`, a `numgen` selector spreads connections across backends.
pub fn clusterip_dnat_rule(
    cluster_ip: Ipv4Addr,
    proto: u8,
    port: u16,
    backend_ip: Ipv4Addr,
    backend_port: u16,
    lb: Option<(u32, u32)>,
) -> NftRule {
    let mut exprs = match_cidr(16, &Ipv4Cidr::new(cluster_ip, 32));
    exprs.extend(match_l4proto(proto));
    exprs.extend(match_dport(port));
    if let Some((idx, total)) = lb
        && total > 1 {
            exprs.push(NftExpr::Numgen {
                dreg: 9,
                modulus: total,
                offset: 0,
            });
            exprs.push(NftExpr::Cmp {
                sreg: 9,
                op: CMP_EQ,
                data: idx.to_le_bytes().to_vec(),
            });
        }
    exprs.extend(dnat_to(backend_ip, backend_port));
    NftRule::from_exprs(exprs)
}

/// Build a NodePort DNAT rule (matches only protocol + node port).
pub fn nodeport_dnat_rule(
    proto: u8,
    node_port: u16,
    backend_ip: Ipv4Addr,
    backend_port: u16,
    lb: Option<(u32, u32)>,
) -> NftRule {
    let mut exprs = match_l4proto(proto);
    exprs.extend(match_dport(node_port));
    if let Some((idx, total)) = lb
        && total > 1 {
            exprs.push(NftExpr::Numgen {
                dreg: 9,
                modulus: total,
                offset: 0,
            });
            exprs.push(NftExpr::Cmp {
                sreg: 9,
                op: CMP_EQ,
                data: idx.to_le_bytes().to_vec(),
            });
        }
    exprs.extend(dnat_to(backend_ip, backend_port));
    NftRule::from_exprs(exprs)
}

/// Build a masquerade rule for traffic leaving the given source CIDR.
pub fn masquerade_rule(src: &Ipv4Cidr) -> NftRule {
    let mut exprs = match_cidr(12, src);
    exprs.push(NftExpr::Masquerade);
    NftRule::from_exprs(exprs)
}

/// Build an NSG-style filter rule: match optional src/dst CIDRs and an
/// optional protocol+port, then accept or drop.
pub fn nsg_filter_rule(
    accept: bool,
    src: Option<&Ipv4Cidr>,
    dst: Option<&Ipv4Cidr>,
    proto: Option<u8>,
    dport: Option<u16>,
) -> NftRule {
    let mut exprs = Vec::new();
    if let Some(s) = src {
        exprs.extend(match_cidr(12, s));
    }
    if let Some(d) = dst {
        exprs.extend(match_cidr(16, d));
    }
    if let Some(p) = proto {
        exprs.extend(match_l4proto(p));
    }
    if let Some(dp) = dport {
        exprs.extend(match_dport(dp));
    }
    exprs.push(if accept { NftExpr::Accept } else { NftExpr::Drop });
    NftRule::from_exprs(exprs)
}

/// Build a rule that matches established and related connections.
/// This is critical for stateful firewalling: return traffic from
/// outbound connections (and their related connections) is accepted
/// without explicit per-service rules.
///
/// Wire format (matching rustables `Rule::established()`):
/// 1. `ct` expression: load conntrack state into reg1
/// 2. `bitwise` expression: mask reg1 with ESTABLISHED bitmask, xor 0
/// 3. `cmp` expression: compare reg1 != 0
pub fn established_related_rule() -> NftRule {
    NftRule::from_exprs(vec![
        NftExpr::Conntrack {
            dreg: 1,
            key: CT_STATE,
        },
        NftExpr::Bitwise {
            sreg: 1,
            dreg: 1,
            len: 4,
            mask: CT_STATE_ESTABLISHED.to_le_bytes().to_vec(),
            xor: vec![0u8; 4],
        },
        NftExpr::Cmp {
            sreg: 1,
            op: 1, // NEQ (not equal)
            data: 0u32.to_be_bytes().to_vec(),
        },
        NftExpr::Accept,
    ])
    .with_comment("established")
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

/// A declarative IPv4 route. Local pod `/32` routes are applied imperatively
/// by `attach_pod`; the routes tracked in [`NetmuxState`] are the
/// reconcile-managed ones (remote pod routes via a peer gateway, static
/// RouteTable entries).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteSpec {
    /// Destination network.
    pub dest: Ipv4Addr,
    /// Destination prefix length.
    pub prefix: u8,
    /// Next-hop gateway, if any.
    pub gateway: Option<Ipv4Addr>,
    /// Output interface index, if pinned.
    pub oif: Option<u32>,
}

impl RouteSpec {
    /// A host route to a single address via a gateway (remote pod route).
    pub fn host_via(dest: Ipv4Addr, gateway: Ipv4Addr) -> Self {
        Self {
            dest,
            prefix: 32,
            gateway: Some(gateway),
            oif: None,
        }
    }

    /// The map key: destination network + prefix.
    pub fn key(&self) -> (Ipv4Addr, u8) {
        (self.dest, self.prefix)
    }
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
    /// Reconcile-managed routes keyed by `(dest, prefix)` (remote pod routes,
    /// static routes). Local pod `/32` routes are owned by `attach_pod`.
    pub routes: BTreeMap<(Ipv4Addr, u8), RouteSpec>,
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
            routes: desired.routes.clone(),
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
    /// Install an IPv4 route (RTNETLINK, not nftables).
    AddRoute { route: RouteSpec },
    /// Remove an IPv4 route (RTNETLINK, not nftables).
    DelRoute { route: RouteSpec },
}

impl NetlinkOp {
    /// Whether this op is an RTNETLINK route op (vs. an nftables op).
    pub fn is_route(&self) -> bool {
        matches!(self, NetlinkOp::AddRoute { .. } | NetlinkOp::DelRoute { .. })
    }
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
