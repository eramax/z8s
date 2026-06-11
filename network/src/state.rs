//! # Declarative Network State Identifiers
//!
//! Stable identities for idempotent nftables upsert/delete. Each rule has
//! a `RuleKey` that uniquely identifies it across the cluster, allowing
//! the reconciler to know exactly what to add or remove.
//!
//! ## Nft Table Layout
//!
//! ```text
//! table z8s_nat_{node}
//!   chain prerouting  (hook) -> jumps to svc-* / np-*
//!   chain output      (hook) -> jumps to svc-*
//!   chain postrouting (hook) -> masquerade for pod CIDR
//!   chain svc-{ip_hex}-{port} -> DNAT backends
//!   chain np-{port}           -> NodePort DNAT
//!
//! table z8s_filter_{node}
//!   chain forward     (hook) -> jump nsg-rules -> established -> policy
//!   chain nsg-rules           -> NSG allow/deny
//!   chain catch-all           -> default-allow for pod CIDR
//! ```
//!
//! ## Why Stable Keys?
//!
//! Without stable identifiers, the reconciler cannot tell which rules are
//! stale. With them, each reconciliation is `desired \ applied` →
//! delete missing, update changed, add new.

use std::net::Ipv4Addr;

/// Nft table owned by z8s on this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableId {
    Nat,
    Filter,
}

impl std::fmt::Display for TableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TableId::Nat => write!(f, "nat"),
            TableId::Filter => write!(f, "filter"),
        }
    }
}

impl TableId {
    pub fn name(self, node_name: &str) -> String {
        let suffix = node_name.replace('-', "_");
        match self {
            TableId::Nat => format!("z8s_nat_{}", suffix),
            TableId::Filter => format!("z8s_filter_{}", suffix),
        }
    }
}

/// Stable identity for idempotent nft upsert/delete.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleKey {
    pub table: TableId,
    pub chain: String,
    /// Logical handle, e.g. `svc-0a5f4001-0050` or `np-30080`.
    pub handle: String,
}

impl RuleKey {
    /// Create a NAT rule key for a given chain and handle.
    pub fn nat(chain: impl Into<String>, handle: impl Into<String>) -> Self {
        Self {
            table: TableId::Nat,
            chain: chain.into(),
            handle: handle.into(),
        }
    }

    /// Key for a ClusterIP DNAT chain.
    pub fn clusterip_dnat(cluster_ip: &str, port: u16) -> Self {
        let ip_hex = cluster_ip.replace('.', "");
        Self::nat(
            format!("svc-{}-{:04x}", ip_hex, port),
            format!("svc-{}-{:04x}", ip_hex, port),
        )
    }

    /// Key for a NodePort DNAT chain.
    pub fn nodeport_dnat(port: u16) -> Self {
        Self::nat(format!("np-{:04x}", port), format!("np-{:04x}", port))
    }
}

impl std::fmt::Display for RuleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.table, self.handle)
    }
}

/// Build a stable chain name from cluster IP and port.
pub fn chain_name(ip: Ipv4Addr, port: u16) -> String {
    format!("svc-{:08x}-{:04x}", u32::from_be_bytes(ip.octets()), port)
}

/// Build a stable chain name for a NodePort.
pub fn nodeport_chain_name(port: u16) -> String {
    format!("np-{:04x}", port)
}

/// Documented chain layout.
pub const CHAIN_LAYOUT: &str = r#"
table z8s_nat_{node}
  chain prerouting  (hook) -> jumps to svc-* / np-*
  chain output      (hook) -> jumps to svc-*
  chain postrouting (hook) -> masquerade for pod CIDR
  chain svc-{ip_hex}-{port} -> DNAT backends
  chain np-{port}           -> NodePort DNAT

table z8s_filter_{node}
  chain forward     (hook) -> jump nsg-rules -> established -> policy
  chain nsg-rules           -> NSG allow/deny
  chain catch-all           -> default-allow for pod CIDR
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_name_replaces_dashes() {
        assert_eq!(TableId::Nat.name("node-1"), "z8s_nat_node_1");
        assert_eq!(TableId::Filter.name("node-1"), "z8s_filter_node_1");
    }

    #[test]
    fn clusterip_dnat_key_stable() {
        let k1 = RuleKey::clusterip_dnat("10.96.0.10", 80);
        let k2 = RuleKey::clusterip_dnat("10.96.0.10", 80);
        assert_eq!(k1, k2);
        // IP "10.96.0.10" with dots removed = "1096010"
        assert_eq!(k1.handle, "svc-1096010-0050");
    }

    #[test]
    fn nodeport_dnat_key_stable() {
        let k = RuleKey::nodeport_dnat(30080);
        // 30080 in hex = 0x7580
        assert_eq!(k.handle, "np-7580");
    }

    #[test]
    fn chain_name_format() {
        let name = chain_name(Ipv4Addr::new(10, 96, 0, 10), 80);
        assert_eq!(name, "svc-0a60000a-0050");
    }
}
