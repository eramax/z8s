//! Network reconcile orchestration (apply order + report).

/// Report from a single `reconcile_network` pass.
#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    pub dnat_rules: usize,
    pub dns_records: usize,
    pub nsg_rules: usize,
    pub network_policies: usize,
    pub remote_routes: usize,
}

/// Documented apply order for `sync_network` (see `docs/plan/04-netmux.md` §3.4).
pub const APPLY_ORDER: &[&str] = &[
    "subnet pools",
    "vnet / snat",
    "nsg filter rules",
    "route tables (stub)",
    "ingress l7",
    "network policy sets",
    "service dnat (planner)",
    "dns records (planner)",
    "remote pod routes (planner)",
];
