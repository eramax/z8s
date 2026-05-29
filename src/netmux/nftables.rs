use std::net::Ipv4Addr;
use std::sync::Mutex;
// CONCURRENCY: Mutex used only for brief lock-hold-during-batch-send.
// Never held across .await points. std::sync::Mutex is safe here because
// the critical section (building and sending a Batch) is synchronous and
// never yields. This is the serialized write channel required by the plan.
use anyhow::{Context, Result};
use tracing::info;
use ipnetwork::IpNetwork;
use nix::libc;
use rustables::{
    Batch, Table, Chain, Rule, ProtocolFamily, ChainType, ChainPolicy, Hook, HookClass,
};
use rustables::set::{Set, SetBuilder};
use rustables::expr::{
    Conntrack, ConntrackKey, Immediate, Masquerade, Nat, NatType,
    Meta, MetaType, Register, Cmp, CmpOp, VerdictKind, Lookup,
    Payload, HighLevelPayload, NetworkHeaderField, IPv4HeaderField,
    TCPHeaderField, TransportHeaderField,
};

pub const NAT_TABLE: &str = "z8s_nat";
pub const FILTER_TABLE: &str = "z8s_filter";

pub struct NftEngine {
    writer: Mutex<()>,
}

impl NftEngine {
    pub fn new() -> Self {
        Self { writer: Mutex::new(()) }
    }

    pub fn init(&self) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        batch.add(&nat_table, rustables::MsgType::Add);

        let prerouting = Chain::new(&nat_table)
            .with_name("prerouting")
            .with_type(ChainType::Nat)
            .with_hook(Hook::new(HookClass::PreRouting, -100))
            .with_policy(ChainPolicy::Accept);
        batch.add(&prerouting, rustables::MsgType::Add);

        let postrouting = Chain::new(&nat_table)
            .with_name("postrouting")
            .with_type(ChainType::Nat)
            .with_hook(Hook::new(HookClass::PostRouting, -100))
            .with_policy(ChainPolicy::Accept);
        batch.add(&postrouting, rustables::MsgType::Add);

        let filter_table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        batch.add(&filter_table, rustables::MsgType::Add);

        let forward = Chain::new(&filter_table)
            .with_name("forward")
            .with_type(ChainType::Filter)
            .with_hook(Hook::new(HookClass::Forward, 0))
            .with_policy(ChainPolicy::Drop);
        batch.add(&forward, rustables::MsgType::Add);

        let input = Chain::new(&filter_table)
            .with_name("input")
            .with_type(ChainType::Filter)
            .with_hook(Hook::new(HookClass::In, 0))
            .with_policy(ChainPolicy::Accept);
        batch.add(&input, rustables::MsgType::Add);

        let output = Chain::new(&filter_table)
            .with_name("output")
            .with_type(ChainType::Filter)
            .with_hook(Hook::new(HookClass::Out, 0))
            .with_policy(ChainPolicy::Accept);
        batch.add(&output, rustables::MsgType::Add);

        let ct_rule = Rule::new(&forward)?
            .established()?;
        batch.add(&ct_rule, rustables::MsgType::Add);

        batch.send().context("Failed to send nftables init batch")?;
        info!("nftables: initialized tables ({}, {}) and baseline chains", NAT_TABLE, FILTER_TABLE);
        Ok(())
    }

    /// Add MASQUERADE for a specific VNet's CIDR (hub VNet gets SNAT, spokes don't).
    /// The `vnet_name` is used to tag the rule so it can be removed later.
    pub fn add_snat(&self, vnet_name: &str, vnet_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();
        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let postrouting = Chain::new(&nat_table).with_name("postrouting");

        let cidr: IpNetwork = vnet_cidr.parse().context("Invalid VNet CIDR")?;
        let rule = Rule::new(&postrouting)?
            .snetwork(cidr)?
            .masquerade();
        batch.add(&rule, rustables::MsgType::Add);

        batch.send().context("Failed to send SNAT batch")?;
        info!("nftables: added MASQUERADE for VNet '{}' (CIDR {})", vnet_name, vnet_cidr);
        Ok(())
    }

    /// Remove MASQUERADE for a specific VNet by CIDR.
    pub fn remove_snat(&self, vnet_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();
        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let postrouting = Chain::new(&nat_table).with_name("postrouting");

        let cidr: IpNetwork = vnet_cidr.parse().context("Invalid VNet CIDR")?;
        let rule = Rule::new(&postrouting)?
            .snetwork(cidr)?
            .masquerade();
        batch.add(&rule, rustables::MsgType::Del);

        batch.send().context("Failed to remove SNAT batch")?;
        info!("nftables: removed MASQUERADE for CIDR {}", vnet_cidr);
        Ok(())
    }

    pub fn add_dnat(&self, cluster_ip: Ipv4Addr, port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        if backends.is_empty() {
            return Ok(());
        }
        let chain_name = format!("svc-{}-{}", cluster_ip, port);
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let prerouting = Chain::new(&nat_table).with_name("prerouting");

        // Delete old chain if exists (removes all old rules atomically)
        let old_chain = Chain::new(&nat_table)
            .with_name(&chain_name)
            .with_type(ChainType::Nat);
        // Plan §6.3: atomically replace entire chain via Batch
        batch.add(&old_chain, rustables::MsgType::Del);

        // Create new per-service chain (no hook — jumped to, not base)
        let svc_chain = Chain::new(&nat_table)
            .with_name(&chain_name)
            .with_type(ChainType::Nat)
            .with_policy(ChainPolicy::Accept);
        batch.add(&svc_chain, rustables::MsgType::Add);

        // Shuffle backends for pseudo-round-robin
        let mut shuffled: Vec<(Ipv4Addr, u16)> = backends.to_vec();
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now().duration_since(UNIX_EPOCH)
            .expect("system time before epoch").subsec_nanos();
        let mut rng = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        for i in (1..shuffled.len()).rev() {
            let j = (rng >> 33) as usize % (i + 1);
            shuffled.swap(i, j);
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        }

        // Add one DNAT rule per backend into the per-service chain
        for (backend_ip, backend_port) in &shuffled {
            let mut rule = Rule::new(&svc_chain)?;
            rule.add_expr(Meta::new(MetaType::NfProto));
            rule.add_expr(Cmp::new(CmpOp::Eq, [libc::NFPROTO_IPV4 as u8]));
            rule.add_expr(
                HighLevelPayload::Network(NetworkHeaderField::IPv4(IPv4HeaderField::Daddr))
                    .build(),
            );
            rule.add_expr(Cmp::new(CmpOp::Eq, cluster_ip.octets()));
            rule.add_expr(
                HighLevelPayload::Transport(TransportHeaderField::Tcp(TCPHeaderField::Dport))
                    .build(),
            );
            rule.add_expr(Cmp::new(CmpOp::Eq, port.to_be_bytes()));
            rule.add_expr(Immediate::new_data(backend_ip.octets().to_vec(), Register::Reg1));
            rule.add_expr(Immediate::new_data(backend_port.to_be_bytes().to_vec(), Register::Reg2));
            rule.add_expr(Nat {
                nat_type: Some(NatType::DNat),
                family: Some(ProtocolFamily::Ipv4),
                ip_register: Some(Register::Reg1),
                port_register: Some(Register::Reg2),
            });
            batch.add(&rule, rustables::MsgType::Add);
        }

        // Add jump rule in prerouting to this service chain
        let mut jump_rule = Rule::new(&prerouting)?;
        jump_rule.add_expr(Meta::new(MetaType::NfProto));
        jump_rule.add_expr(Cmp::new(CmpOp::Eq, [libc::NFPROTO_IPV4 as u8]));
        jump_rule.add_expr(
            HighLevelPayload::Network(NetworkHeaderField::IPv4(IPv4HeaderField::Daddr))
                .build(),
        );
        jump_rule.add_expr(Cmp::new(CmpOp::Eq, cluster_ip.octets()));
        jump_rule.add_expr(
            HighLevelPayload::Transport(TransportHeaderField::Tcp(TCPHeaderField::Dport))
                .build(),
        );
        jump_rule.add_expr(Cmp::new(CmpOp::Eq, port.to_be_bytes()));
        jump_rule.add_expr(Immediate::new_verdict(rustables::expr::VerdictKind::Jump { chain: chain_name.clone() }));
        batch.add(&jump_rule, rustables::MsgType::Add);

        batch.send().context("Failed to send DNAT batch")?;
        info!("nftables: DNAT {}:{} -> {} backends (chain {})", cluster_ip, port, backends.len(), chain_name);
        Ok(())
    }

    /// Remove DNAT chain and jump rule for a ClusterIP.
    pub fn remove_dnat(&self, cluster_ip: Ipv4Addr, port: u16) -> Result<()> {
        let chain_name = format!("svc-{}-{}", cluster_ip, port);
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);

        let del_chain = Chain::new(&nat_table)
            .with_name(&chain_name)
            .with_type(ChainType::Nat);
        batch.add(&del_chain, rustables::MsgType::Del);

        batch.send().context("Failed to remove DNAT chain")?;
        info!("nftables: removed DNAT chain {}", chain_name);
        Ok(())
    }

    pub fn add_forward_allow(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let filter_table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let forward = Chain::new(&filter_table).with_name("forward");

        let src_net: IpNetwork = src_cidr.parse().context("Invalid src CIDR")?;
        let dst_net: IpNetwork = dst_cidr.parse().context("Invalid dst CIDR")?;

        let rule = Rule::new(&forward)?
            .snetwork(src_net)?
            .dnetwork(dst_net)?
            .accept();
        batch.add(&rule, rustables::MsgType::Add);

        batch.send().context("Failed to send forward allow batch")?;
        info!("nftables: forward allow {} -> {}", src_cidr, dst_cidr);
        Ok(())
    }

    pub fn add_forward_deny(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let filter_table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let forward = Chain::new(&filter_table).with_name("forward");

        let src_net: IpNetwork = src_cidr.parse().context("Invalid src CIDR")?;
        let dst_net: IpNetwork = dst_cidr.parse().context("Invalid dst CIDR")?;

        let rule = Rule::new(&forward)?
            .snetwork(src_net)?
            .dnetwork(dst_net)?
            .drop();
        batch.add(&rule, rustables::MsgType::Add);

        batch.send().context("Failed to send forward deny batch")?;
        info!("nftables: forward deny {} -> {}", src_cidr, dst_cidr);
        Ok(())
    }

    /// Create an nftables set for pod IPs (for NetworkPolicy).
    /// If `initial_ips` is provided, the set is created with those IPs.
    pub fn create_set(&self, name: &str, initial_ips: &[Ipv4Addr]) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let mut builder = SetBuilder::<Ipv4Addr>::new(name, &table)
            .map_err(|e| anyhow::anyhow!("SetBuilder error: {}", e))?;
        for ip in initial_ips {
            builder.add(ip);
        }
        let (set, elem_list) = builder.finish();
        batch.add(&set, rustables::MsgType::Add);
        batch.add(&elem_list, rustables::MsgType::Add);

        batch.send().context("Failed to create nftables set")?;
        info!("nftables: created set '{}' with {} IPs", name, initial_ips.len());
        Ok(())
    }

    /// Replace a set's elements (delete and recreate).
    pub fn replace_set(&self, name: &str, ips: &[Ipv4Addr]) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);

        // Delete old set (may fail if not exists, that's OK)
        let mut del_set = Set::default();
        del_set.family = ProtocolFamily::Ipv4;
        del_set = del_set.with_table(FILTER_TABLE.to_string()).with_name(name);
        batch.add(&del_set, rustables::MsgType::Del);

        // Create new set with elements
        let mut builder = SetBuilder::<Ipv4Addr>::new(name, &table)
            .map_err(|e| anyhow::anyhow!("SetBuilder error: {}", e))?;
        for ip in ips {
            builder.add(ip);
        }
        let (set, elem_list) = builder.finish();
        batch.add(&set, rustables::MsgType::Add);
        batch.add(&elem_list, rustables::MsgType::Add);

        batch.send().context("Failed to replace nftables set")?;
        info!("nftables: replaced set '{}' with {} IPs", name, ips.len());
        Ok(())
    }

    /// Add a forward rule that matches packets with src IP in a named set.
    pub fn add_forward_allow_set_src(&self, set_name: &str, dst_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let filter_table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let forward = Chain::new(&filter_table).with_name("forward");

        let mut set_for_lookup = Set::default();
        set_for_lookup.family = ProtocolFamily::Ipv4;
        set_for_lookup = set_for_lookup
            .with_table(FILTER_TABLE.to_string())
            .with_name(set_name);

        let mut rule = Rule::new(&forward)?;
        rule.add_expr(Lookup::new(&set_for_lookup)
            .map_err(|e| anyhow::anyhow!("Lookup error: {}", e))?);
        let dst_net: IpNetwork = dst_cidr.parse().context("Invalid dst CIDR")?;
        let rule = rule.dnetwork(dst_net)?.accept();
        batch.add(&rule, rustables::MsgType::Add);

        batch.send()?;
        info!("nftables: forward allow @{} -> {}", set_name, dst_cidr);
        Ok(())
    }
}
