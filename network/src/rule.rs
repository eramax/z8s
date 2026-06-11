//! # Declarative Network Rule Types
//!
//! A `NftRule` is a single declarative nftables rule that can be
//! composed, serialized, and applied. Rules are pure data — no IO.
//!
//! ## Why Declarative?
//!
//! Imperative nft calls (add this, add that) are hard to reason about
//! and impossible to diff. With declarative rules, the planner can
//! compute the desired set, the reconciler can compute the diff, and
//! the user can see exactly what rules are in effect.
//!
//! ## Builder
//!
//! `NftAction` is the "what to do" — accept, drop, DNAT, SNAT, jump.
//! `NftRule` is the "what to match and what to do" — combines an
//! action with source, destination, protocol, and ports.

use std::net::Ipv4Addr;

/// What a rule does when it matches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NftAction {
    /// Accept the packet (allow it through).
    Accept,
    /// Drop the packet (silently).
    Drop,
    /// Reject the packet (send ICMP unreachable).
    Reject,
    /// Destination NAT — rewrite the destination IP:port.
    DNAT {
        dest_ip: Ipv4Addr,
        dest_port: Option<u16>,
    },
    /// Source NAT — rewrite the source IP.
    SNAT {
        source_ip: Ipv4Addr,
    },
    /// Masquerade (SNAT with the outgoing interface address).
    Masquerade,
    /// Jump to another chain.
    Jump(String),
}

impl NftAction {
    /// Short string for logging/metrics.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Drop => "drop",
            Self::Reject => "reject",
            Self::DNAT { .. } => "dnat",
            Self::SNAT { .. } => "snat",
            Self::Masquerade => "masquerade",
            Self::Jump(_) => "jump",
        }
    }
}

/// A single nftables rule — declarative, diffable, composable.
#[derive(Debug, Clone, PartialEq)]
pub struct NftRule {
    /// Human-readable name (e.g. "nsg-default-allow-internal").
    pub name: String,
    /// Target chain (e.g. "nsg-rules", "prerouting").
    pub chain: String,
    /// What to do when matched.
    pub action: NftAction,
    /// Source CIDR (e.g. "10.42.0.0/24").
    pub source: Option<String>,
    /// Destination CIDR (e.g. "0.0.0.0/0").
    pub dest: Option<String>,
    /// Protocol: "tcp", "udp", "icmp" (None = any).
    pub protocol: Option<String>,
    /// Destination port (None = any).
    pub dport: Option<u16>,
    /// Source port (None = any).
    pub sport: Option<u16>,
}

impl NftRule {
    /// Build a new rule with a name and chain. Defaults to Accept.
    pub fn new(name: impl Into<String>, chain: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            chain: chain.into(),
            action: NftAction::Accept,
            source: None,
            dest: None,
            protocol: None,
            dport: None,
            sport: None,
        }
    }

    /// Set the action.
    pub fn action(mut self, a: NftAction) -> Self {
        self.action = a;
        self
    }

    /// Set source CIDR.
    pub fn source(mut self, s: impl Into<String>) -> Self {
        self.source = Some(s.into());
        self
    }

    /// Set destination CIDR.
    pub fn dest(mut self, d: impl Into<String>) -> Self {
        self.dest = Some(d.into());
        self
    }

    /// Set protocol.
    pub fn protocol(mut self, p: impl Into<String>) -> Self {
        self.protocol = Some(p.into());
        self
    }

    /// Set destination port.
    pub fn dport(mut self, p: u16) -> Self {
        self.dport = Some(p);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nft_rule_builder() {
        let r = NftRule::new("allow-internal", "nsg-rules")
            .action(NftAction::Accept)
            .source("10.42.0.0/24")
            .dest("10.42.0.0/24")
            .protocol("tcp")
            .dport(80);
        assert_eq!(r.name, "allow-internal");
        assert_eq!(r.chain, "nsg-rules");
        assert_eq!(r.action, NftAction::Accept);
        assert_eq!(r.source, Some("10.42.0.0/24".into()));
        assert_eq!(r.dport, Some(80));
    }

    #[test]
    fn nft_action_tag() {
        assert_eq!(NftAction::Accept.tag(), "accept");
        assert_eq!(NftAction::Drop.tag(), "drop");
        assert_eq!(
            NftAction::DNAT {
                dest_ip: Ipv4Addr::new(10, 0, 0, 1),
                dest_port: Some(80)
            }
            .tag(),
            "dnat"
        );
    }
}
