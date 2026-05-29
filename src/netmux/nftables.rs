use std::net::Ipv4Addr;
use std::sync::Mutex;
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
    writer: Mutex<()>,
}

impl NftEngine {
    pub fn new() -> Self {
        Self { writer: Mutex::new(()) }
    }

    fn send_batch(batch: Batch) -> Result<()> {
        match batch.send() {
            Ok(()) => Ok(()),
            Err(e) => {
                if let QueryError::NetlinkError(ref err) = e {
                    let code = err.error.abs();
                    if code == 2 || code == 17 || code == 25 || code == 16 {
                        return Ok(());
                    }
                }
                Err(e.into())
            }
        }
    }

    pub fn init(&self, pod_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");

        // Delete stale table (clears all rules from previous runs)
        {
            let mut batch = Batch::new();
            let stale = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
            batch.add(&stale, rustables::MsgType::Del);
            Self::send_batch(batch)?;
        }

        // Create nat table with baseline chains
        {
            let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);
            let mut batch = Batch::new();
            batch.add(&nat_table, rustables::MsgType::Add);

            let prerouting = Chain::new(&nat_table)
                .with_name("prerouting")
                .with_type(ChainType::Nat)
                .with_hook(Hook::new(HookClass::PreRouting, HOOK_PRIO_NAT))
                .with_policy(ChainPolicy::Accept);
            batch.add(&prerouting, rustables::MsgType::Add);

            let postrouting = Chain::new(&nat_table)
                .with_name("postrouting")
                .with_type(ChainType::Nat)
                .with_hook(Hook::new(HookClass::PostRouting, HOOK_PRIO_NAT))
                .with_policy(ChainPolicy::Accept);
            batch.add(&postrouting, rustables::MsgType::Add);

            let output = Chain::new(&nat_table)
                .with_name("output")
                .with_type(ChainType::Nat)
                .with_hook(Hook::new(HookClass::Out, HOOK_PRIO_NAT))
                .with_policy(ChainPolicy::Accept);
            batch.add(&output, rustables::MsgType::Add);

            Self::send_batch(batch)?;
        }

        // Create filter table with baseline chains
        {
            let mut batch = Batch::new();
            let filter_table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
            batch.add(&filter_table, rustables::MsgType::Add);

            let forward = Chain::new(&filter_table)
                .with_name("forward")
                .with_type(ChainType::Filter)
                .with_hook(Hook::new(HookClass::Forward, HOOK_PRIO_FILTER))
                .with_policy(ChainPolicy::Drop);
            batch.add(&forward, rustables::MsgType::Add);

            let input = Chain::new(&filter_table)
                .with_name("input")
                .with_type(ChainType::Filter)
                .with_hook(Hook::new(HookClass::In, HOOK_PRIO_FILTER))
                .with_policy(ChainPolicy::Accept);
            batch.add(&input, rustables::MsgType::Add);

            let output = Chain::new(&filter_table)
                .with_name("output")
                .with_type(ChainType::Filter)
                .with_hook(Hook::new(HookClass::Out, HOOK_PRIO_FILTER))
                .with_policy(ChainPolicy::Accept);
            batch.add(&output, rustables::MsgType::Add);

            let ct_rule = Rule::new(&forward)?
                .established()?
                .accept();
            batch.add(&ct_rule, rustables::MsgType::Add);

            let pod_net: IpNetwork = pod_cidr.parse().context("Invalid pod CIDR in nftables init")?;
            let inter_pod_rule = Rule::new(&forward)?
                .snetwork(pod_net)?
                .dnetwork(pod_net)?
                .accept();
            batch.add(&inter_pod_rule, rustables::MsgType::Add);

            // Allow host-to-pod traffic (needed for NodePort DNAT from host)
            let host_to_pod = Rule::new(&forward)?
                .dnetwork(pod_net)?
                .accept();
            batch.add(&host_to_pod, rustables::MsgType::Add);

            Self::send_batch(batch)?;
        }

        info!("nftables: initialized tables and chains");
        Ok(())
    }

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

        batch.send()?;
        info!("nftables: added MASQUERADE for VNet '{}' (CIDR {})", vnet_name, vnet_cidr);
        Ok(())
    }

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

        batch.send()?;
        info!("nftables: removed MASQUERADE for CIDR {}", vnet_cidr);
        Ok(())
    }

    pub fn add_dnat(&self, cluster_ip: Ipv4Addr, port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        if backends.is_empty() {
            return Ok(());
        }
        let _lock = self.writer.lock().expect("lock poisoned");
        let svc = chain_name(cluster_ip, port);
        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);

        // Delete old per-service chain (ignore ENOENT/EBUSY)
        {
            let mut batch = Batch::new();
            let old_chain = Chain::new(&nat_table).with_name(&svc);
            batch.add(&old_chain, rustables::MsgType::Del);
            Self::send_batch(batch)?;
        }

        // Create per-service chain with DNAT rules
        {
            let mut batch = Batch::new();
            let chain = Chain::new(&nat_table).with_name(&svc);
            batch.add(&chain, rustables::MsgType::Add);

            for (backend_ip, backend_port) in backends {
                let mut rule = Rule::new(&chain).map_err(|e| anyhow::anyhow!("{:?}", e))?;
                rule.add_expr(Meta::new(MetaType::NfProto));
                rule.add_expr(Cmp::new(CmpOp::Eq, [2]));
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
            batch.send()?;
        }

        // Add jump rules to prerouting and output (appended, may accumulate)
        for hook in ["prerouting", "output"] {
            let mut batch = Batch::new();
            let hook_chain = Chain::new(&nat_table).with_name(hook);
            let mut rule = Rule::new(&hook_chain).map_err(|e| anyhow::anyhow!("{:?}", e))?;
            rule.add_expr(Immediate::new_verdict(VerdictKind::Jump { chain: svc.clone() }));
            batch.add(&rule, rustables::MsgType::Add);
            batch.send()?;
        }

        debug!("nftables: DNAT {}:{} -> {} backends", cluster_ip, port, backends.len());
        Ok(())
    }

    pub fn add_nodeport_dnat(&self, node_port: u16, backends: &[(Ipv4Addr, u16)]) -> Result<()> {
        if backends.is_empty() {
            return Ok(());
        }
        let _lock = self.writer.lock().expect("lock poisoned");
        let svc = format!("np-{:04x}", node_port);
        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);

        {
            let mut batch = Batch::new();
            let old_chain = Chain::new(&nat_table).with_name(&svc);
            batch.add(&old_chain, rustables::MsgType::Del);
            Self::send_batch(batch)?;
        }

        {
            let mut batch = Batch::new();
            let chain = Chain::new(&nat_table).with_name(&svc);
            batch.add(&chain, rustables::MsgType::Add);

            for (backend_ip, backend_port) in backends {
                let mut rule = Rule::new(&chain).map_err(|e| anyhow::anyhow!("{:?}", e))?;
                rule.add_expr(Meta::new(MetaType::NfProto));
                rule.add_expr(Cmp::new(CmpOp::Eq, [2]));
                rule.add_expr(
                    HighLevelPayload::Transport(TransportHeaderField::Tcp(TCPHeaderField::Dport))
                        .build(),
                );
                rule.add_expr(Cmp::new(CmpOp::Eq, node_port.to_be_bytes()));
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
            batch.send()?;
        }

        for hook in ["prerouting", "output"] {
            let mut batch = Batch::new();
            let hook_chain = Chain::new(&nat_table).with_name(hook);
            let mut rule = Rule::new(&hook_chain).map_err(|e| anyhow::anyhow!("{:?}", e))?;
            rule.add_expr(Immediate::new_verdict(VerdictKind::Jump { chain: svc.clone() }));
            batch.add(&rule, rustables::MsgType::Add);
            batch.send()?;
        }

        debug!("nftables: NodePort {} -> {} backends", node_port, backends.len());
        Ok(())
    }

    pub fn remove_dnat(&self, cluster_ip: Ipv4Addr, port: u16) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let svc = chain_name(cluster_ip, port);
        let nat_table = Table::new(ProtocolFamily::Ipv4).with_name(NAT_TABLE);

        // Delete per-service chain (jump rules to it remain but are harmless)
        {
            let mut batch = Batch::new();
            let chain = Chain::new(&nat_table).with_name(&svc);
            batch.add(&chain, rustables::MsgType::Del);
            Self::send_batch(batch)?;
        }

        info!("nftables: removed DNAT for {}:{}", cluster_ip, port);
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

        batch.send()?;
        debug!("nftables: forward allow {} -> {}", src_cidr, dst_cidr);
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

        batch.send()?;
        debug!("nftables: forward deny {} -> {}", src_cidr, dst_cidr);
        Ok(())
    }

    pub fn create_set(&self, name: &str, initial_ips: &[Ipv4Addr]) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let mut builder = rustables::set::SetBuilder::<Ipv4Addr>::new(name, &table)
            .map_err(|e| anyhow::anyhow!("SetBuilder error: {}", e))?;
        for ip in initial_ips {
            builder.add(ip);
        }
        let (set, elem_list) = builder.finish();
        batch.add(&set, rustables::MsgType::Add);
        batch.add(&elem_list, rustables::MsgType::Add);

        batch.send()?;
        info!("nftables: created set '{}' with {} IPs", name, initial_ips.len());
        Ok(())
    }

    pub fn replace_set(&self, name: &str, ips: &[Ipv4Addr]) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);

        let mut del_set = rustables::Set::default();
        del_set.family = ProtocolFamily::Ipv4;
        del_set = del_set.with_table(FILTER_TABLE.to_string()).with_name(name);
        batch.add(&del_set, rustables::MsgType::Del);

        let mut builder = rustables::set::SetBuilder::<Ipv4Addr>::new(name, &table)
            .map_err(|e| anyhow::anyhow!("SetBuilder error: {}", e))?;
        for ip in ips {
            builder.add(ip);
        }
        let (set, elem_list) = builder.finish();
        batch.add(&set, rustables::MsgType::Add);
        batch.add(&elem_list, rustables::MsgType::Add);

        batch.send()?;
        debug!("nftables: replaced set '{}' with {} IPs", name, ips.len());
        Ok(())
    }

    pub fn add_forward_allow_set_src(&self, set_name: &str, dst_cidr: &str) -> Result<()> {
        let _lock = self.writer.lock().expect("lock poisoned");
        let mut batch = Batch::new();

        let filter_table = Table::new(ProtocolFamily::Ipv4).with_name(FILTER_TABLE);
        let forward = Chain::new(&filter_table).with_name("forward");

        let mut set_for_lookup = rustables::Set::default();
        set_for_lookup.family = ProtocolFamily::Ipv4;
        set_for_lookup = set_for_lookup
            .with_table(FILTER_TABLE.to_string())
            .with_name(set_name);

        let mut rule = Rule::new(&forward)?;
        rule.add_expr(rustables::expr::Lookup::new(&set_for_lookup)
            .map_err(|e| anyhow::anyhow!("Lookup error: {}", e))?);
        let dst_net: IpNetwork = dst_cidr.parse().context("Invalid dst CIDR")?;
        let rule = rule.dnetwork(dst_net)?.accept();
        batch.add(&rule, rustables::MsgType::Add);

        batch.send()?;
        debug!("nftables: forward allow @{} -> {}", set_name, dst_cidr);
        Ok(())
    }
}
