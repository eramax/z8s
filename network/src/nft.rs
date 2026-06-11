//! # Nftables Engine
//!
//! Manages the nftables tables and chains for pod networking.
//! All operations go through a single nft connection, with batches
//! serialized to avoid concurrent netlink writes.
//!
//! ## Tables
//!
//! - `z8s_nat_{node}` — NAT table with prerouting/output/postrouting hooks
//! - `z8s_filter_{node}` — Filter table with forward/input/output hooks
//!
//! ## Chains
//!
//! NAT table:
//! - `prerouting` (hook) → jumps to `svc-*` and `np-*` chains
//! - `output` (hook) → jumps to `svc-*` chains
//! - `postrouting` (hook) → masquerade for pod CIDR
//! - `svc-{ip_hex}-{port}` → DNAT backends
//! - `np-{port}` → NodePort DNAT
//!
//! Filter table:
//! - `forward` (hook) → `jump nsg-rules` → established → policy
//! - `input` (hook) → input policy
//! - `output` (hook) → output policy
//! - `nsg-rules` → NSG allow/deny rules
//! - `catch-all` → default-allow for pod CIDR
//!
//! ## Performance
//!
//! The `PERFORMANCE-PLAN.md` calls for **batched nftables** to combine
//! `add_snat` + `add_forward_allow` + `add_dnat` into a single
//! `Batch::send()` round-trip. We expose `add_snat`, `add_dnat`, etc.
//! as a sequence-friendly API; the user can opt to batch by using
//! `rustables::Batch` directly.

use anyhow::{anyhow, Context, Result};
use ipnetwork::IpNetwork;
use rustables::expr::{
    Cmp, CmpOp, HighLevelPayload, IPv4HeaderField, Immediate, Meta, MetaType, Nat, NatType,
    NetworkHeaderField, Register, TCPHeaderField, TransportHeaderField, VerdictKind,
};
use rustables::{
    Batch, Chain, ChainPolicy, ChainType, Hook, HookClass, ProtocolFamily, Rule, Table,
};
use std::net::Ipv4Addr;
use tracing::{debug, info, warn};

use crate::state::TableId;

/// Generate the NAT table name for a node.
pub fn nat_table_name(node_name: &str) -> String {
    TableId::Nat.name(node_name)
}

/// Generate the filter table name for a node.
pub fn filter_table_name(node_name: &str) -> String {
    TableId::Filter.name(node_name)
}

const HOOK_PRIO_NAT: i32 = -100;
const HOOK_PRIO_FILTER: i32 = 0;

/// Build a stable chain name from cluster IP and port.
pub fn svc_chain_name(ip: Ipv4Addr, port: u16) -> String {
    crate::state::chain_name(ip, port)
}

/// Build a stable chain name for a NodePort.
pub fn np_chain_name(port: u16) -> String {
    crate::state::nodeport_chain_name(port)
}

/// Nftables engine — owns the tables and chains for this node.
pub struct NftEngine {
    /// Serializes all batch sends. Held only inside `send()`.
    writer: tokio::sync::Mutex<()>,
    /// Track of ClusterIP DNAT chains we've created jumps for.
    clusterip_jumps: tokio::sync::Mutex<Vec<(Ipv4Addr, u16)>>,
    /// Track of NodePort DNAT chains we've created jumps for.
    nodeport_jumps: tokio::sync::Mutex<Vec<u16>>,
    nat_table: String,
    filter_table: String,
}

impl NftEngine {
    /// Create a new NftEngine for a node.
    pub fn new(node_name: &str) -> Self {
        Self {
            writer: tokio::sync::Mutex::new(()),
            clusterip_jumps: tokio::sync::Mutex::new(Vec::new()),
            nodeport_jumps: tokio::sync::Mutex::new(Vec::new()),
            nat_table: nat_table_name(node_name),
            filter_table: filter_table_name(node_name),
        }
    }

    /// Send an nftables batch via spawn_blocking so the async runtime isn't blocked.
    /// Only ENOENT (2), EBUSY (16), and EEXIST (17) on delete are silently suppressed.
    pub async fn send(&self, batch: Batch) -> Result<()> {
        let _lock = self.writer.lock().await;
        tokio::task::spawn_blocking(move || {
            match batch.send() {
                Ok(()) => Ok(()),
                Err(e) => {
                    if let rustables::error::QueryError::NetlinkError(ref err) = e {
                        let code = err.error.abs();
                        if matches!(code, 2 | 16 | 17) {
                            return Ok(()); // ENOENT/EBUSY/EEXIST — safe to ignore
                        }
                        return Err(anyhow!("nftables error {} ({})", code, code_name(code)));
                    }
                    Err(anyhow!("{:?}", e))
                }
            }
        })
        .await
        .context("spawn_blocking")?
    }

    // ── Init ───────────────────────────────────────────────────────

    /// Initialize tables and chains. Idempotent — safe to call on every node start.
    pub async fn init(&self, pod_cidr: &str) -> Result<()> {
        // Delete & recreate our tables (NEVER flush the whole ruleset — incident.md)
        for tbl in [&self.nat_table, &self.filter_table] {
            let t = Table::new(ProtocolFamily::Ipv4).with_name(tbl);
            let mut d = Batch::new();
            d.add(&t, rustables::MsgType::Del);
            self.send(d).await.ok();
            let mut a = Batch::new();
            a.add(&t, rustables::MsgType::Add);
            self.send(a).await?;
        }
        self.clusterip_jumps.lock().await.clear();
        self.nodeport_jumps.lock().await.clear();

        // NAT table chains (hooks)
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(&self.nat_table);
        let mut nb = Batch::new();
        for (n, h) in [
            ("prerouting", HookClass::PreRouting),
            ("postrouting", HookClass::PostRouting),
            ("output", HookClass::Out),
        ] {
            nb.add(
                &Chain::new(&nat)
                    .with_name(n)
                    .with_type(ChainType::Nat)
                    .with_hook(Hook::new(h, HOOK_PRIO_NAT))
                    .with_policy(ChainPolicy::Accept),
                rustables::MsgType::Add,
            );
        }
        self.send(nb).await?;

        // Filter table chains (hooks)
        let filter = Table::new(ProtocolFamily::Ipv4).with_name(&self.filter_table);
        let mut fb = Batch::new();
        fb.add(
            &Chain::new(&filter)
                .with_name("forward")
                .with_type(ChainType::Filter)
                .with_hook(Hook::new(HookClass::Forward, HOOK_PRIO_FILTER))
                .with_policy(ChainPolicy::Accept),
            rustables::MsgType::Add,
        );
        fb.add(
            &Chain::new(&filter)
                .with_name("input")
                .with_type(ChainType::Filter)
                .with_hook(Hook::new(HookClass::In, HOOK_PRIO_FILTER))
                .with_policy(ChainPolicy::Accept),
            rustables::MsgType::Add,
        );
        fb.add(
            &Chain::new(&filter)
                .with_name("output")
                .with_type(ChainType::Filter)
                .with_hook(Hook::new(HookClass::Out, HOOK_PRIO_FILTER))
                .with_policy(ChainPolicy::Accept),
            rustables::MsgType::Add,
        );
        let forward = Chain::new(&filter).with_name("forward");
        fb.add(
            &Rule::new(&forward)?.established()?.accept(),
            rustables::MsgType::Add,
        );
        self.send(fb).await?;

        // NSG chain
        let mut nsg = Batch::new();
        nsg.add(
            &Chain::new(&filter).with_name("nsg-rules"),
            rustables::MsgType::Add,
        );
        let mut jmp = Rule::new(&forward).map_err(|e| anyhow!("{:?}", e))?;
        jmp.add_expr(Immediate::new_verdict(VerdictKind::Jump {
            chain: "nsg-rules".to_string(),
        }));
        nsg.add(&jmp, rustables::MsgType::Add);
        self.send(nsg).await?;

        // Add catch-all allow for pod CIDR
        self.add_forward_catchall(pod_cidr).await?;
        info!("nftables initialized for pod_cidr={}", pod_cidr);
        Ok(())
    }

    // ── SNAT ──────────────────────────────────────────────────────

    /// Add SNAT (masquerade) for a VNet CIDR.
    pub async fn add_snat(&self, _vnet_name: &str, vnet_cidr: &str) -> Result<()> {
        let cidr: IpNetwork = vnet_cidr.parse()?;
        let mut b = Batch::new();
        b.add(
            &Rule::new(
                &Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(&self.nat_table))
                    .with_name("postrouting"),
            )?
            .snetwork(cidr)?
            .masquerade(),
            rustables::MsgType::Add,
        );
        self.send(b).await?;
        debug!("SNAT added for {}", vnet_cidr);
        Ok(())
    }

    // ── DNAT helpers ──────────────────────────────────────────────

    fn build_dnat_rule(
        chain: &Chain,
        matches: &[(rustables::expr::Payload, Vec<u8>)],
        backend_ip: Ipv4Addr,
        backend_port: u16,
    ) -> Result<Rule> {
        let mut r = Rule::new(chain).map_err(|e| anyhow!("{:?}", e))?;
        r.add_expr(Meta::new(MetaType::NfProto));
        r.add_expr(Cmp::new(CmpOp::Eq, [2]));
        for (pl, val) in matches {
            r.add_expr(pl.clone());
            r.add_expr(Cmp::new(CmpOp::Eq, val.clone()));
        }
        r.add_expr(Immediate::new_data(
            backend_ip.octets().to_vec(),
            Register::Reg1,
        ));
        r.add_expr(Immediate::new_data(
            backend_port.to_be_bytes().to_vec(),
            Register::Reg2,
        ));
        r.add_expr(Nat {
            nat_type: Some(NatType::DNat),
            family: Some(ProtocolFamily::Ipv4),
            ip_register: Some(Register::Reg1),
            port_register: Some(Register::Reg2),
        });
        Ok(r)
    }

    async fn update_dnat_chain(
        &self,
        svc: &str,
        backends: &[(Ipv4Addr, u16)],
        matches: &[(rustables::expr::Payload, Vec<u8>)],
    ) -> Result<()> {
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(&self.nat_table);
        let mut b = Batch::new();
        b.add(&Chain::new(&nat).with_name(svc), rustables::MsgType::Add);
        self.send(b).await?;
        for (ip, port) in backends {
            let mut b = Batch::new();
            let rule = Self::build_dnat_rule(&Chain::new(&nat).with_name(svc), matches, *ip, *port)?;
            b.add(&rule, rustables::MsgType::Add);
            self.send(b).await?;
        }
        Ok(())
    }

    async fn add_jump_rules(&self, svc: &str, hooks: &[&str]) -> Result<()> {
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(&self.nat_table);
        for h in hooks {
            let mut b = Batch::new();
            let mut r = Rule::new(&Chain::new(&nat).with_name(*h))
                .map_err(|e| anyhow!("{:?}", e))?;
            r.add_expr(Immediate::new_verdict(VerdictKind::Jump {
                chain: svc.to_string(),
            }));
            b.add(&r, rustables::MsgType::Add);
            self.send(b).await?;
        }
        Ok(())
    }

    // ── ClusterIP DNAT ────────────────────────────────────────────

    /// Add a ClusterIP DNAT rule: redirect traffic to `cluster_ip:port` to one of `backends`.
    pub async fn add_dnat(
        &self,
        cluster_ip: Ipv4Addr,
        port: u16,
        backends: &[(Ipv4Addr, u16)],
    ) -> Result<()> {
        if backends.is_empty() {
            return Ok(()); // nothing to DNAT
        }
        let svc = svc_chain_name(cluster_ip, port);
        let matches = vec![
            (
                HighLevelPayload::Network(NetworkHeaderField::IPv4(IPv4HeaderField::Daddr)).build(),
                cluster_ip.octets().to_vec(),
            ),
            (
                HighLevelPayload::Transport(TransportHeaderField::Tcp(TCPHeaderField::Dport))
                    .build(),
                port.to_be_bytes().to_vec(),
            ),
        ];
        self.update_dnat_chain(&svc, backends, &matches).await?;
        let mut track = self.clusterip_jumps.lock().await;
        if !track.iter().any(|(ip, p)| *ip == cluster_ip && *p == port) {
            track.push((cluster_ip, port));
            self.add_jump_rules(&svc, &["prerouting", "output"]).await?;
        }
        debug!("DNAT {}:{} -> {} backends", cluster_ip, port, backends.len());
        Ok(())
    }

    /// Remove a ClusterIP DNAT chain.
    pub async fn remove_dnat(&self, cluster_ip: Ipv4Addr, port: u16) -> Result<()> {
        let svc = svc_chain_name(cluster_ip, port);
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(&self.nat_table);
        let mut b = Batch::new();
        b.add(&Chain::new(&nat).with_name(&svc), rustables::MsgType::Del);
        self.send(b).await?;
        self.clusterip_jumps
            .lock()
            .await
            .retain(|(ip, p)| *ip != cluster_ip || *p != port);
        info!("Removed DNAT {}:{}", cluster_ip, port);
        Ok(())
    }

    // ── NodePort DNAT ─────────────────────────────────────────────

    /// Add a NodePort DNAT rule: redirect traffic to host port `node_port` to one of `backends`.
    pub async fn add_nodeport_dnat(
        &self,
        node_port: u16,
        backends: &[(Ipv4Addr, u16)],
    ) -> Result<()> {
        if backends.is_empty() {
            return Ok(());
        }
        let svc = np_chain_name(node_port);
        let matches = vec![(
            HighLevelPayload::Transport(TransportHeaderField::Tcp(TCPHeaderField::Dport)).build(),
            node_port.to_be_bytes().to_vec(),
        )];
        self.update_dnat_chain(&svc, backends, &matches).await?;
        let mut track = self.nodeport_jumps.lock().await;
        if !track.contains(&node_port) {
            track.push(node_port);
            self.add_jump_rules(&svc, &["prerouting", "output"]).await?;
        }
        debug!("NodePort {} -> {} backends", node_port, backends.len());
        Ok(())
    }

    /// Remove a NodePort DNAT chain.
    pub async fn remove_nodeport_dnat(&self, node_port: u16) -> Result<()> {
        let svc = np_chain_name(node_port);
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(&self.nat_table);
        let mut b = Batch::new();
        b.add(&Chain::new(&nat).with_name(&svc), rustables::MsgType::Del);
        self.send(b).await?;
        self.nodeport_jumps.lock().await.retain(|p| *p != node_port);
        info!("Removed NodePort DNAT {}", node_port);
        Ok(())
    }

    // ── Forward chain NSG rules ────────────────────────────────────

    /// Add an allow rule to the nsg-rules chain.
    pub async fn add_forward_allow(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(&self.filter_table))
            .with_name("nsg-rules");
        let mut b = Batch::new();
        b.add(
            &Rule::new(&nsg)?
                .snetwork(src_cidr.parse()?)?
                .dnetwork(dst_cidr.parse()?)?
                .accept(),
            rustables::MsgType::Add,
        );
        self.send(b).await
    }

    /// Add a deny rule to the nsg-rules chain.
    pub async fn add_forward_deny(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(&self.filter_table))
            .with_name("nsg-rules");
        let mut b = Batch::new();
        b.add(
            &Rule::new(&nsg)?
                .snetwork(src_cidr.parse()?)?
                .dnetwork(dst_cidr.parse()?)?
                .drop(),
            rustables::MsgType::Add,
        );
        self.send(b).await
    }

    /// Allow traffic from an nftables set of source IPs.
    pub async fn add_forward_allow_set_src(&self, set_name: &str, dst_cidr: &str) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(&self.filter_table))
            .with_name("nsg-rules");
        let mut b = Batch::new();
        let s = rustables::Set::default()
            .with_table(&self.filter_table)
            .with_name(set_name);
        let mut r = Rule::new(&nsg).map_err(|e| anyhow!("{:?}", e))?;
        r.add_expr(rustables::expr::Lookup::new(&s).map_err(|e| anyhow!("{:?}", e))?);
        b.add(
            &r.dnetwork(dst_cidr.parse()?)?.accept(),
            rustables::MsgType::Add,
        );
        self.send(b).await
    }

    /// Reset (delete and recreate) the nsg-rules chain.
    pub async fn reset_nsg_rules(&self) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(&self.filter_table))
            .with_name("nsg-rules");
        let mut d = Batch::new();
        d.add(&nsg, rustables::MsgType::Del);
        self.send(d).await?;
        let mut a = Batch::new();
        a.add(&nsg, rustables::MsgType::Add);
        self.send(a).await
    }

    // ── Catch-all chain ────────────────────────────────────────────

    /// Add a default-allow catch-all chain for the pod CIDR.
    pub async fn add_forward_catchall(&self, pod_cidr: &str) -> Result<()> {
        let filter = Table::new(ProtocolFamily::Ipv4).with_name(&self.filter_table);
        let mut b = Batch::new();
        b.add(
            &Chain::new(&filter).with_name("catch-all"),
            rustables::MsgType::Add,
        );
        b.add(
            &Rule::new(&Chain::new(&filter).with_name("catch-all"))?
                .dnetwork(pod_cidr.parse()?)?
                .accept(),
            rustables::MsgType::Add,
        );
        let forward = Chain::new(&filter).with_name("forward");
        let mut jmp = Rule::new(&forward).map_err(|e| anyhow!("{:?}", e))?;
        jmp.add_expr(Immediate::new_verdict(VerdictKind::Jump {
            chain: "catch-all".to_string(),
        }));
        b.add(&jmp, rustables::MsgType::Add);
        self.send(b).await
    }

    // ── Sets (NetworkPolicy) ───────────────────────────────────────

    /// Create an nftables set of IPv4 addresses (used by NetworkPolicy).
    pub async fn create_set(&self, name: &str, initial_ips: &[Ipv4Addr]) -> Result<()> {
        let table = Table::new(ProtocolFamily::Ipv4).with_name(&self.filter_table);
        let mut builder = rustables::set::SetBuilder::<Ipv4Addr>::new(name, &table)
            .map_err(|e| anyhow!("{:?}", e))?;
        for ip in initial_ips {
            builder.add(ip);
        }
        let (set, elem) = builder.finish();
        let mut b = Batch::new();
        b.add(&set, rustables::MsgType::Add);
        b.add(&elem, rustables::MsgType::Add);
        self.send(b).await
    }

    /// Replace the contents of a set with the given IPs (creates if missing).
    pub async fn replace_set(&self, name: &str, ips: &[Ipv4Addr]) -> Result<()> {
        // Try to delete first, ignoring errors
        let mut d = Batch::new();
        d.add(&rustables::Set::default().with_table(&self.filter_table).with_name(name), rustables::MsgType::Del);
        self.send(d).await.ok();
        self.create_set(name, ips).await
    }

    /// Remove all z8s nftables tables. Called during shutdown.
    pub async fn cleanup(&self) -> Result<()> {
        info!("Cleaning up nftables rules...");
        for tbl in [&self.nat_table, &self.filter_table] {
            let t = Table::new(ProtocolFamily::Ipv4).with_name(tbl);
            let mut d = Batch::new();
            d.add(&t, rustables::MsgType::Del);
            if let Err(e) = self.send(d).await {
                debug!("Failed to delete table {}: {}", tbl, e);
            }
        }
        self.clusterip_jumps.lock().await.clear();
        self.nodeport_jumps.lock().await.clear();
        info!("nftables cleanup complete");
        Ok(())
    }
}

fn code_name(c: i32) -> &'static str {
    match c {
        1 => "EPERM",
        2 => "ENOENT",
        3 => "ESRCH",
        4 => "EINTR",
        5 => "EIO",
        6 => "ENXIO",
        11 => "EAGAIN",
        12 => "ENOMEM",
        13 => "EACCES",
        16 => "EBUSY",
        17 => "EEXIST",
        22 => "EINVAL",
        25 => "ENOTTY",
        95 => "EOPNOTSUPP",
        105 => "ENOBUFS",
        114 => "EALREADY",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names() {
        assert_eq!(nat_table_name("node-1"), "z8s_nat_node_1");
        assert_eq!(filter_table_name("node-1"), "z8s_filter_node_1");
    }

    #[test]
    fn svc_chain_name_format() {
        let name = svc_chain_name(Ipv4Addr::new(10, 96, 0, 10), 80);
        assert_eq!(name, "svc-0a60000a-0050");
    }

    #[test]
    fn np_chain_name_format() {
        // 30080 in hex = 0x7580
        assert_eq!(np_chain_name(30080), "np-7580");
    }

    #[test]
    fn code_name_known() {
        assert_eq!(code_name(2), "ENOENT");
        assert_eq!(code_name(17), "EEXIST");
        assert_eq!(code_name(999), "UNKNOWN");
    }
}
