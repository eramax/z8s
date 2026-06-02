//! Declarative network state keys and chain layout (N0).

/// Nft table owned by z8s on this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableId {
    Nat,
    Filter,
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
    pub fn nat(chain: impl Into<String>, handle: impl Into<String>) -> Self {
        Self {
            table: TableId::Nat,
            chain: chain.into(),
            handle: handle.into(),
        }
    }

    pub fn clusterip_dnat(cluster_ip: &str, port: u16) -> Self {
        let ip_hex = cluster_ip.replace('.', "");
        Self::nat(
            format!("svc-{ip_hex}-{port:04}"),
            format!("svc-{ip_hex}-{port:04}"),
        )
    }

    pub fn nodeport_dnat(port: u16) -> Self {
        Self::nat(format!("np-{port}"), format!("np-{port}"))
    }
}

/// Documented chain layout for `z8s_nat_{node}` / `z8s_filter_{node}`.
pub const CHAIN_LAYOUT: &str = r#"
table z8s_nat_{node}
  chain prerouting  (hook) -> jumps to svc-* / np-*
  chain output      (hook) -> jumps to svc-*
  chain postrouting (hook) -> masquerade for pod CIDR
  chain svc-{ip}-{port}     -> DNAT backends
  chain np-{port}           -> NodePort DNAT

table z8s_filter_{node}
  chain forward     (hook) -> jump nsg-rules -> established -> policy
  chain nsg-rules           -> NSG allow/deny
"#;
