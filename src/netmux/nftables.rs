use std::net::Ipv4Addr;
use anyhow::{Context, Result};
use tracing::{debug, info};
use ipnetwork::IpNetwork;
use rustables::{
    Batch, Table, Chain, Rule, ProtocolFamily, ChainType, ChainPolicy, Hook, HookClass,
};
use rustables::error::QueryError;
use rustables::expr::{
    Immediate, Nat, NatType,
    Meta, MetaType, Register, Cmp, CmpOp, VerdictKind,
    HighLevelPayload, NetworkHeaderField, IPv4HeaderField,
    TCPHeaderField, TransportHeaderField,
};

pub const NAT_TABLE: &str = "z8s_nat";
pub const FILTER_TABLE: &str = "z8s_filter";

const HOOK_PRIO_NAT: i32 = -100;
const HOOK_PRIO_FILTER: i32 = 0;

fn chain_name(ip: Ipv4Addr, port: u16) -> String {
    format!("svc-{:08x}-{:04x}", u32::from_be_bytes(ip.octets()), port)
}

pub struct NftEngine {
    /// Serializes all nftables batch sends — held only during spawn_blocking.
    writer: tokio::sync::Mutex<()>,
    jump_track: tokio::sync::Mutex<Vec<(Ipv4Addr, u16)>>,
    nodeport_jump_track: tokio::sync::Mutex<Vec<u16>>,
}

impl NftEngine {
    pub fn new() -> Self {
        Self {
            writer: tokio::sync::Mutex::new(()),
            jump_track: tokio::sync::Mutex::new(Vec::new()),
            nodeport_jump_track: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Send an nftables batch via spawn_blocking so the async runtime isn't blocked.
    /// Only ENOENT (2) on delete is silently suppressed — expected when deleting non-existent chains.
    async fn send(&self, batch: Batch) -> Result<()> {
        let _lock = self.writer.lock().await;
        tokio::task::spawn_blocking(move || {
            match batch.send() {
                Ok(()) => Ok(()),
                Err(e) => {
                    if let QueryError::NetlinkError(ref err) = e {
                        let code = err.error.abs();
                        if code == 2 || code == 16 || code == 17 {
                            return Ok(()); // ENOENT/EBUSY/EEXIST — safe to ignore
                        }
                        return Err(anyhow::anyhow!("nftables error {} ({})", code, code_name(code)));
                    }
                    Err(anyhow::anyhow!("{:?}", e))
                }
            }
        }).await.context("spawn_blocking")?
    }

    // ── Init ───────────────────────────────────────────────────────

    pub async fn init(&self, pod_cidr: &str) -> Result<()> {
        // NOTE: Only touches our own tables (z8s_nat, z8s_filter).
        // Never flush the entire ruleset — that would destroy k3s's kube-* tables
        // and require a reboot to recover (see incident.md).
        for tbl in [NAT_TABLE, FILTER_TABLE] {
            let t = Table::new(ProtocolFamily::Ipv4).with_name(tbl);
            let mut d = Batch::new(); d.add(&t, rustables::MsgType::Del); self.send(d).await?;
            let mut a = Batch::new(); a.add(&t, rustables::MsgType::Add); self.send(a).await?;
        }
        self.jump_track.lock().await.clear();
        self.nodeport_jump_track.lock().await.clear();

        let nat = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let mut nb = Batch::new();
        for (n, h) in [("prerouting", HookClass::PreRouting), ("postrouting", HookClass::PostRouting), ("output", HookClass::Out)] {
            nb.add(&Chain::new(&nat).with_name(n).with_type(ChainType::Nat).with_hook(Hook::new(h, HOOK_PRIO_NAT)).with_policy(ChainPolicy::Accept), rustables::MsgType::Add);
        }
        self.send(nb).await?;

        let filter = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let mut fb = Batch::new();
        fb.add(&Chain::new(&filter).with_name("forward").with_type(ChainType::Filter).with_hook(Hook::new(HookClass::Forward, HOOK_PRIO_FILTER)).with_policy(ChainPolicy::Accept), rustables::MsgType::Add);
        fb.add(&Chain::new(&filter).with_name("input").with_type(ChainType::Filter).with_hook(Hook::new(HookClass::In, HOOK_PRIO_FILTER)).with_policy(ChainPolicy::Accept), rustables::MsgType::Add);
        fb.add(&Chain::new(&filter).with_name("output").with_type(ChainType::Filter).with_hook(Hook::new(HookClass::Out, HOOK_PRIO_FILTER)).with_policy(ChainPolicy::Accept), rustables::MsgType::Add);
        let forward = Chain::new(&filter).with_name("forward");
        fb.add(&Rule::new(&forward)?.established()?.accept(), rustables::MsgType::Add);
        self.send(fb).await?;

        let mut nsg = Batch::new();
        nsg.add(&Chain::new(&filter).with_name("nsg-rules"), rustables::MsgType::Add);
        let mut jmp = Rule::new(&forward).map_err(|e| anyhow::anyhow!("{:?}", e))?;
        jmp.add_expr(Immediate::new_verdict(VerdictKind::Jump { chain: "nsg-rules".to_string() }));
        nsg.add(&jmp, rustables::MsgType::Add);
        self.send(nsg).await?;
        info!("nftables initialized");
        Ok(())
    }

    // ── SNAT ──────────────────────────────────────────────────────

    pub async fn add_snat(&self, _vnet_name: &str, vnet_cidr: &str) -> Result<()> {
        let cidr: IpNetwork = vnet_cidr.parse()?;
        let mut b = Batch::new();
        b.add(&Rule::new(&Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE)).with_name("postrouting"))?.snetwork(cidr)?.masquerade(), rustables::MsgType::Add);
        self.send(b).await?;
        debug!("SNAT added for {}", vnet_cidr);
        Ok(())
    }

    // ── DNAT helpers ──────────────────────────────────────────────

    fn add_dnat_rule(batch: &mut Batch, chain: &Chain, matches: &[(rustables::expr::Payload, Vec<u8>)], backend_ip: Ipv4Addr, backend_port: u16) -> Result<()> {
        let mut r = Rule::new(chain).map_err(|e| anyhow::anyhow!("{:?}", e))?;
        r.add_expr(Meta::new(MetaType::NfProto));
        r.add_expr(Cmp::new(CmpOp::Eq, [2]));
        for (pl, val) in matches { r.add_expr(pl.clone()); r.add_expr(Cmp::new(CmpOp::Eq, val.clone())); }
        r.add_expr(Immediate::new_data(backend_ip.octets().to_vec(), Register::Reg1));
        r.add_expr(Immediate::new_data(backend_port.to_be_bytes().to_vec(), Register::Reg2));
        r.add_expr(Nat { nat_type: Some(NatType::DNat), family: Some(ProtocolFamily::Ipv4), ip_register: Some(Register::Reg1), port_register: Some(Register::Reg2) });
        batch.add(&r, rustables::MsgType::Add);
        Ok(())
    }

    async fn update_dnat_chain(&self, svc: &str, backends: &[(Ipv4Addr, u16)], matches: &[(rustables::expr::Payload, Vec<u8>)]) -> Result<()> {
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let mut b = Batch::new();
        b.add(&Chain::new(&nat).with_name(svc), rustables::MsgType::Add);
        self.send(b).await?;
        for (ip, port) in backends {
            let mut b = Batch::new();
            Self::add_dnat_rule(&mut b, &Chain::new(&nat).with_name(svc), matches, *ip, *port)?;
            self.send(b).await?;
        }
        Ok(())
    }

    async fn add_jump_rules(&self, svc: &str, hooks: &[&str]) -> Result<()> {
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        for h in hooks {
            let mut b = Batch::new();
            let mut r = Rule::new(&Chain::new(&nat).with_name(*h)).map_err(|e| anyhow::anyhow!("{:?}", e))?;
            r.add_expr(Immediate::new_verdict(VerdictKind::Jump { chain: svc.to_string() }));
            b.add(&r, rustables::MsgType::Add);
            self.send(b).await?;
        }
        Ok(())
    }

    // ── ClusterIP DNAT ────────────────────────────────────────────

    pub async fn add_dnat(&self, cluster_ip: Ipv4Addr, port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        let svc = chain_name(cluster_ip, port);
        let matches = vec![
            (HighLevelPayload::Network(NetworkHeaderField::IPv4(IPv4HeaderField::Daddr)).build(), cluster_ip.octets().to_vec()),
            (HighLevelPayload::Transport(TransportHeaderField::Tcp(TCPHeaderField::Dport)).build(), port.to_be_bytes().to_vec()),
        ];
        self.update_dnat_chain(&svc, backends, &matches).await?;
        let mut track = self.jump_track.lock().await;
        if !track.iter().any(|(ip, p)| *ip == cluster_ip && *p == port) {
            track.push((cluster_ip, port));
            self.add_jump_rules(&svc, &["prerouting", "output"]).await?;
        }
        debug!("DNAT {}:{} -> {} backends", cluster_ip, port, backends.len());
        Ok(())
    }

    pub async fn remove_dnat(&self, cluster_ip: Ipv4Addr, port: u16) -> Result<()> {
        let svc = chain_name(cluster_ip, port);
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let mut b = Batch::new();
        b.add(&Chain::new(&nat).with_name(&svc), rustables::MsgType::Del);
        self.send(b).await?;
        self.jump_track.lock().await.retain(|(ip, p)| *ip != cluster_ip || *p != port);
        info!("Removed DNAT {}:{}", cluster_ip, port);
        Ok(())
    }

    // ── NodePort DNAT ─────────────────────────────────────────────

    pub async fn add_nodeport_dnat(&self, node_port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        if backends.is_empty() { return Ok(()); }
        let svc = format!("np-{:04x}", node_port);
        let matches = vec![
            (HighLevelPayload::Transport(TransportHeaderField::Tcp(TCPHeaderField::Dport)).build(), node_port.to_be_bytes().to_vec()),
        ];
        self.update_dnat_chain(&svc, backends, &matches).await?;
        let mut track = self.nodeport_jump_track.lock().await;
        if !track.contains(&node_port) {
            track.push(node_port);
            self.add_jump_rules(&svc, &["prerouting", "output"]).await?;
        }
        debug!("NodePort {} -> {} backends", node_port, backends.len());
        Ok(())
    }

    pub async fn remove_nodeport_dnat(&self, node_port: u16) -> Result<()> {
        let svc = format!("np-{:04x}", node_port);
        let nat = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let mut b = Batch::new();
        b.add(&Chain::new(&nat).with_name(&svc), rustables::MsgType::Del);
        self.send(b).await?;
        self.nodeport_jump_track.lock().await.retain(|p| *p != node_port);
        info!("Removed NodePort DNAT {}", node_port);
        Ok(())
    }

    // ── Forward chain NSG rules ────────────────────────────────────

    pub async fn add_forward_allow(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE)).with_name("nsg-rules");
        let mut b = Batch::new();
        b.add(&Rule::new(&nsg)?.snetwork(src_cidr.parse()?)?.dnetwork(dst_cidr.parse()?)?.accept(), rustables::MsgType::Add);
        self.send(b).await
    }

    pub async fn add_forward_deny(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE)).with_name("nsg-rules");
        let mut b = Batch::new();
        b.add(&Rule::new(&nsg)?.snetwork(src_cidr.parse()?)?.dnetwork(dst_cidr.parse()?)?.drop(), rustables::MsgType::Add);
        self.send(b).await
    }

    pub async fn add_forward_allow_set_src(&self, set_name: &str, dst_cidr: &str) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE)).with_name("nsg-rules");
        let mut b = Batch::new();
        let mut s = rustables::Set::default(); s.family = ProtocolFamily::Ipv4; s = s.with_table(FILTER_TABLE).with_name(set_name);
        let mut r = Rule::new(&nsg).map_err(|e| anyhow::anyhow!("{:?}", e))?;
        r.add_expr(rustables::expr::Lookup::new(&s).map_err(|e| anyhow::anyhow!("{:?}", e))?);
        b.add(&r.dnetwork(dst_cidr.parse()?)?.accept(), rustables::MsgType::Add);
        self.send(b).await
    }

    pub async fn reset_nsg_rules(&self) -> Result<()> {
        let nsg = Chain::new(&Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE)).with_name("nsg-rules");
        let mut d = Batch::new(); d.add(&nsg, rustables::MsgType::Del); self.send(d).await?;
        let mut a = Batch::new(); a.add(&nsg, rustables::MsgType::Add); self.send(a).await
    }

    // ── Catch-all chain ────────────────────────────────────────────

    pub async fn add_forward_catchall(&self, pod_cidr: &str) -> Result<()> {
        let filter = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let mut b = Batch::new();
        b.add(&Chain::new(&filter).with_name("catch-all"), rustables::MsgType::Add);
        b.add(&Rule::new(&Chain::new(&filter).with_name("catch-all"))?.dnetwork(pod_cidr.parse()?)?.accept(), rustables::MsgType::Add);
        let forward = Chain::new(&filter).with_name("forward");
        let mut jmp = Rule::new(&forward).map_err(|e| anyhow::anyhow!("{:?}", e))?;
        jmp.add_expr(Immediate::new_verdict(VerdictKind::Jump { chain: "catch-all".to_string() }));
        b.add(&jmp, rustables::MsgType::Add);
        self.send(b).await
    }

    // ── Sets (NetworkPolicy) ───────────────────────────────────────

    pub async fn create_set(&self, name: &str, initial_ips: &[Ipv4Addr]) -> Result<()> {
        let table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let mut builder = rustables::set::SetBuilder::<Ipv4Addr>::new(name, &table).map_err(|e| anyhow::anyhow!("{:?}", e))?;
        for ip in initial_ips { builder.add(ip); }
        let (set, elem) = builder.finish();
        let mut b = Batch::new(); b.add(&set, rustables::MsgType::Add); b.add(&elem, rustables::MsgType::Add);
        self.send(b).await
    }

    pub async fn replace_set(&self, name: &str, ips: &[Ipv4Addr]) -> Result<()> {
        self.create_set(name, ips).await
    }

    /// Remove all z8s nftables tables. Called during shutdown.
    pub async fn cleanup(&self) -> Result<()> {
        info!("Cleaning up nftables rules...");
        for tbl in [NAT_TABLE, FILTER_TABLE] {
            let t = Table::new(ProtocolFamily::Ipv4).with_name(tbl);
            let mut d = Batch::new();
            d.add(&t, rustables::MsgType::Del);
            if let Err(e) = self.send(d).await {
                debug!("Failed to delete table {}: {}", tbl, e);
            }
        }
        self.jump_track.lock().await.clear();
        self.nodeport_jump_track.lock().await.clear();
        info!("nftables cleanup complete");
        Ok(())
    }
}

fn code_name(c: i32) -> &'static str {
    match c { 1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 5 => "EIO",
              6 => "ENXIO", 11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES", 16 => "EBUSY",
              17 => "EEXIST", 22 => "EINVAL", 25 => "ENOTTY", 26 => "ETXTBSY",
              95 => "EOPNOTSUPP", 105 => "ENOBUFS", 114 => "EALREADY", _ => "UNKNOWN" }
}
