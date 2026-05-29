use std::net::Ipv4Addr;
use std::sync::Mutex;
use anyhow::{Context, Result};
use tracing::info;
use ipnetwork::IpNetwork;
use nix::libc;
use rustables::{
    Batch, Table, Chain, Rule, ProtocolFamily, ChainType, ChainPolicy, Hook, HookClass,
};
use rustables::expr::{
    Conntrack, ConntrackKey, Immediate, Masquerade, Nat, NatType,
    Meta, MetaType, Register, Cmp, CmpOp, VerdictKind,
    Payload, HighLevelPayload, NetworkHeaderField, IPv4HeaderField,
    TCPHeaderField, TransportHeaderField,
};

const NAT_TABLE: &str = "z8s_nat";
const FILTER_TABLE: &str = "z8s_filter";

pub struct NftEngine {
    writer: Mutex<()>,
}

impl NftEngine {
    pub fn new() -> Self {
        Self { writer: Mutex::new(()) }
    }

    pub fn init(&self) -> Result<()> {
        let _lock = self.writer.lock().unwrap();
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

    pub fn add_snat(&self, pod_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().unwrap();
        let mut batch = Batch::new();

        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let postrouting = Chain::new(&nat_table).with_name("postrouting");

        let cidr: IpNetwork = pod_cidr.parse().context("Invalid pod CIDR")?;
        let rule = Rule::new(&postrouting)?
            .snetwork(cidr)?
            .masquerade();
        batch.add(&rule, rustables::MsgType::Add);

        batch.send().context("Failed to send SNAT batch")?;
        info!("nftables: added MASQUERADE for {}", pod_cidr);
        Ok(())
    }

    pub fn add_dnat(&self, cluster_ip: Ipv4Addr, port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        if backends.is_empty() {
            return Ok(());
        }
        let _lock = self.writer.lock().unwrap();
        let mut batch = Batch::new();

        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
        let prerouting = Chain::new(&nat_table).with_name("prerouting");

        for (idx, (backend_ip, backend_port)) in backends.iter().enumerate() {
            let mut rule = Rule::new(&prerouting)?;
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

        batch.send().context("Failed to send DNAT batch")?;
        info!("nftables: DNAT {}:{} -> {} backend(s)", cluster_ip, port, backends.len());
        Ok(())
    }

    pub fn add_forward_allow(&self, src_cidr: &str, dst_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().unwrap();
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
        let _lock = self.writer.lock().unwrap();
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
}
