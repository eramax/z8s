//! # Nftables Netlink Syscalls
//!
//! Encodes nftables netlink messages and decodes kernel replies.
//!
//! ## Why hand-rolled
//!
//! We do not depend on `rustables` (third-party) or `libc`. We talk to the
//! kernel directly through `rustix`'s netlink bindings, encoding the
//! attribute payload (MPTCP policy / NLA layout) ourselves.
//!
//! ## Wire format
//!
//! Every message is `nlmsghdr(16) | nfgenmsg(4) | attrs`, where:
//!
//! - `nlmsghdr.nlmsg_type = (NFNL_SUBSYS_NFTABLES << 8) | NFT_MSG_*`
//! - `nfgenmsg` carries the address family
//!
//! Each attribute is:
//!
//! ```text
//! +--------+--------+--------+--------+--------+--------+
//! |  nla_len (u16)  |  nla_type (u16)  |  payload (nla_len - 4 bytes) |
//! +--------+--------+--------+--------+--------+--------+
//! ```
//!
//! Nested attributes (NLA_F_NESTED) carry a child stream of attrs.

use std::net::Ipv4Addr;
use std::num::NonZero;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

use rustix::net::{AddressFamily, Protocol, SocketFlags, SocketType};

use crate::model::*;

// ═══════════════════════════════════════════════════════════════════════════
// Netlink constants
// ═══════════════════════════════════════════════════════════════════════════

const NETLINK_NETFILTER: NonZero<u32> = NonZero::new(10).unwrap();
const NFNL_SUBSYS_NFTABLES: u16 = 10;

// Nftables netlink message types
const NFT_MSG_NEWTABLE: u16 = 0;
const NFT_MSG_DELTABLE: u16 = 1;
const NFT_MSG_NEWCHAIN: u16 = 2;
const NFT_MSG_DELCHAIN: u16 = 3;
const NFT_MSG_NEWRULE: u16 = 4;
const NFT_MSG_DELRULE: u16 = 5;
const NFT_MSG_NEWSET: u16 = 6;
const NFT_MSG_DELSET: u16 = 7;
const NFT_MSG_NEWSETELEM: u16 = 9;
const NFT_MSG_DELSETELEM: u16 = 10;
const NFT_MSG_NEWOBJ: u16 = 12;
const NFT_MSG_DELOBJ: u16 = 13;

// Nftables attribute types
const NFTA_TABLE_NAME: u16 = 1;
const NFTA_CHAIN_NAME: u16 = 1;
const NFTA_CHAIN_TABLE: u16 = 2;
const NFTA_CHAIN_HOOKNUM: u16 = 4;
const NFTA_CHAIN_PRIORITY: u16 = 5;
const NFTA_CHAIN_POLICY: u16 = 7;
const NFTA_CHAIN_TYPE: u16 = 6;
const NFTA_RULE_TABLE: u16 = 1;
const NFTA_RULE_CHAIN: u16 = 2;
const NFTA_RULE_EXPRESSIONS: u16 = 4;
const NFTA_RULE_HANDLE: u16 = 5;
const NFTA_SET_NAME: u16 = 1;
const NFTA_SET_TABLE: u16 = 2;
const NFTA_SET_KEY_TYPE: u16 = 3;
const NFTA_SET_KEY_LEN: u16 = 4;
const NFTA_SET_DATA_LEN: u16 = 6;
const NFTA_SET_ELEMENTS: u16 = 9;
const NFTA_SET_ELEM_KEY: u16 = 1;
const NFTA_SET_ELEM_DATA: u16 = 2;
const NFTA_OBJ_NAME: u16 = 1;
const NFTA_OBJ_TABLE: u16 = 2;
const NFTA_OBJ_TYPE: u16 = 3;
const NFTA_OBJ_DATA: u16 = 4;
const NFTA_COUNTER_BYTES: u16 = 1;
const NFTA_COUNTER_PACKETS: u16 = 2;

// NLA flags
const NLA_F_NESTED: u16 = 0x8000;

// Expression types
const NFT_EXPR_META: u16 = 1;
const NFT_EXPR_CMP: u16 = 6;
const NFT_EXPR_PAYLOAD: u16 = 9;
const NFT_EXPR_NAT: u16 = 14;
const NFT_EXPR_IMMEDIATE: u16 = 17;
const NFT_EXPR_LOOKUP: u16 = 18;
const NFT_EXPR_MASQ: u16 = 19;
const NFT_EXPR_ACCEPT: u16 = 28;
const NFT_EXPR_DROP: u16 = 29;

// Expression attrs
const NFT_META_KEY: u16 = 1;
const NFT_META_DREG: u16 = 2;
const NFT_CMP_SREG: u16 = 1;
const NFT_CMP_OP: u16 = 2;
const NFT_CMP_DATA: u16 = 3;
const NFT_PAYLOAD_DREG: u16 = 1;
const NFT_PAYLOAD_BASE: u16 = 2;
const NFT_PAYLOAD_OFFSET: u16 = 3;
const NFT_PAYLOAD_LEN: u16 = 4;
const NFT_IMMEDIATE_DREG: u16 = 1;
const NFT_IMMEDIATE_DATA: u16 = 2;
const NFT_LOOKUP_SET: u16 = 1;
const NFT_LOOKUP_SREG: u16 = 2;
const NFT_NAT_TYPE: u16 = 1;
const NFT_NAT_FAMILY: u16 = 2;
const NFT_NAT_REG_ADDR_MIN: u16 = 3;
const NFT_NAT_REG_ADDR_MAX: u16 = 4;
const NFT_NAT_REG_PROTO_MIN: u16 = 5;
const NFT_NAT_REG_PROTO_MAX: u16 = 6;

// Meta keys
const NFT_META_PROTOCOL: u8 = 4;

// Nat types
const NFT_NAT_SNAT: u32 = 0;
const NFT_NAT_DNAT: u32 = 1;

const NFT_OBJECT_COUNTER: u32 = 1;
const NFT_REG_VERDICT: u32 = 0x00;
const NF_RETURN: u32 = 0x00000005;

// ═══════════════════════════════════════════════════════════════════════════
// Attribute encoding
// ═══════════════════════════════════════════════════════════════════════════

/// A builder for a netlink message body.
#[derive(Debug, Default, Clone)]
pub struct NlaBuf {
    bytes: Vec<u8>,
}

impl NlaBuf {
    /// New empty buffer.
    pub fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Is empty?
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Append a u8 in NLA format.
    pub fn put_u8(&mut self, kind: u16, val: u8) {
        self.put_slice(kind, &[val]);
    }

    /// Append a u16 in NLA format.
    pub fn put_u16(&mut self, kind: u16, val: u16) {
        self.put_slice(kind, &val.to_ne_bytes());
    }

    /// Append a u32 in NLA format.
    pub fn put_u32(&mut self, kind: u16, val: u32) {
        self.put_slice(kind, &val.to_ne_bytes());
    }

    /// Append a u64 in NLA format.
    pub fn put_u64(&mut self, kind: u16, val: u64) {
        self.put_slice(kind, &val.to_ne_bytes());
    }

    /// Append a C-string in NLA format (NUL-terminated, padded).
    pub fn put_str(&mut self, kind: u16, s: &str) {
        let bytes = s.as_bytes();
        let mut padded_len = 4 + bytes.len() + 1; // include NUL
        let pad = (4 - (padded_len % 4)) % 4;
        padded_len += pad;
        self.bytes.extend_from_slice(&(padded_len as u16).to_ne_bytes());
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(bytes);
        self.bytes.push(0); // NUL terminator
        if pad > 0 {
            self.bytes.resize(self.bytes.len() + pad, 0);
        }
    }

    /// Append raw bytes in NLA format (padded to 4-byte boundary).
    pub fn put_slice(&mut self, kind: u16, data: &[u8]) {
        let mut padded_len = 4 + data.len();
        let pad = (4 - (padded_len % 4)) % 4;
        padded_len += pad;
        self.bytes.extend_from_slice(&(padded_len as u16).to_ne_bytes());
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(data);
        if pad > 0 {
            self.bytes.resize(self.bytes.len() + pad, 0);
        }
    }

    /// Append a nested attribute (`NLA_F_NESTED`). Padded to 4 bytes.
    pub fn put_nested<F>(&mut self, kind: u16, f: F)
    where
        F: FnOnce(&mut NlaBuf),
    {
        let header_pos = self.bytes.len();
        self.bytes.extend_from_slice(&0u16.to_ne_bytes()); // placeholder for length
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        let payload_start = self.bytes.len();
        f(self);
        let payload_len = self.bytes.len() - payload_start;
        let total = 4 + payload_len;
        let padded = (total + 3) & !3;
        if padded > total {
            self.bytes.resize(self.bytes.len() + (padded - total), 0);
        }
        let len = (padded as u16) | NLA_F_NESTED;
        self.bytes[header_pos..header_pos + 2].copy_from_slice(&len.to_ne_bytes());
    }

    /// Consume the buffer into raw bytes.
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    /// Borrow the raw bytes.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// nfgenmsg — 4-byte header
// ═══════════════════════════════════════════════════════════════════════════

/// Build the 4-byte nfgenmsg header.
pub fn nfgen_header(family: NftFamily) -> [u8; 4] {
    [family.as_u8(), 0u8, 0, 0]
}

// ═══════════════════════════════════════════════════════════════════════════
// Expression encoders
// ═══════════════════════════════════════════════════════════════════════════

/// Encode a single rule expression into a NLA nested attribute.
pub fn encode_expr(expr: &NftExpr, out: &mut NlaBuf) {
    match expr {
        NftExpr::Meta { kind, .. } => {
            out.put_nested(NFT_EXPR_META, |inner| {
                inner.put_u32(NFT_META_KEY, *kind);
                inner.put_slice(NFT_META_DREG, &1u32.to_be_bytes());
            });
        }
        NftExpr::Cmp { sreg, op, data } => {
            out.put_nested(NFT_EXPR_CMP, |inner| {
                inner.put_u32(NFT_CMP_SREG, *sreg);
                inner.put_u32(NFT_CMP_OP, *op);
                inner.put_slice(NFT_CMP_DATA, data);
            });
        }
        NftExpr::Payload {
            dreg,
            base,
            offset,
            len,
        } => {
            out.put_nested(NFT_EXPR_PAYLOAD, |inner| {
                inner.put_u32(NFT_PAYLOAD_DREG, *dreg);
                inner.put_u32(NFT_PAYLOAD_BASE, *base);
                inner.put_u32(NFT_PAYLOAD_OFFSET, *offset);
                inner.put_u32(NFT_PAYLOAD_LEN, *len);
            });
        }
        NftExpr::Lookup { set, sreg } => {
            out.put_nested(NFT_EXPR_LOOKUP, |inner| {
                inner.put_str(NFT_LOOKUP_SET, set);
                inner.put_u32(NFT_LOOKUP_SREG, *sreg);
            });
        }
        NftExpr::Immediate { dreg, data } => {
            out.put_nested(NFT_EXPR_IMMEDIATE, |inner| {
                inner.put_u32(NFT_IMMEDIATE_DREG, *dreg);
                inner.put_slice(NFT_IMMEDIATE_DATA, data);
            });
        }
        NftExpr::Nat {
            nat_type,
            sreg_addr,
            sreg_port,
        } => {
            out.put_nested(NFT_EXPR_NAT, |inner| {
                inner.put_u32(NFT_NAT_TYPE, *nat_type);
                inner.put_u32(NFT_NAT_FAMILY, NftFamily::Ip.as_u8() as u32);
                inner.put_u32(NFT_NAT_REG_ADDR_MIN, *sreg_addr);
                inner.put_u32(NFT_NAT_REG_ADDR_MAX, *sreg_addr);
                inner.put_u32(NFT_NAT_REG_PROTO_MIN, *sreg_port);
                inner.put_u32(NFT_NAT_REG_PROTO_MAX, *sreg_port);
            });
        }
        NftExpr::Masquerade => {
            out.put_nested(NFT_EXPR_MASQ, |inner| {
                inner.put_u32(NFT_NAT_TYPE, NFT_NAT_SNAT);
                inner.put_u32(NFT_NAT_FAMILY, NftFamily::Ip.as_u8() as u32);
                inner.put_u32(NFT_NAT_REG_ADDR_MIN, 0);
                inner.put_u32(NFT_NAT_REG_ADDR_MAX, 0);
                inner.put_u32(NFT_NAT_REG_PROTO_MIN, 0);
                inner.put_u32(NFT_NAT_REG_PROTO_MAX, 0);
            });
        }
        NftExpr::Accept => {
            out.put_nested(NFT_EXPR_ACCEPT, |_| {});
        }
        NftExpr::Drop => {
            out.put_nested(NFT_EXPR_DROP, |_| {});
        }
        NftExpr::Return => {
            out.put_nested(NFT_EXPR_IMMEDIATE, |inner| {
                inner.put_u32(NFT_IMMEDIATE_DREG, NFT_REG_VERDICT);
                inner.put_u32(NFT_IMMEDIATE_DATA, NF_RETURN);
            });
        }
        NftExpr::Jump(chain) => {
            out.put_nested(NFT_EXPR_IMMEDIATE, |inner| {
                inner.put_u32(NFT_IMMEDIATE_DREG, NFT_REG_VERDICT);
                inner.put_str(NFT_IMMEDIATE_DATA, chain);
            });
        }
        NftExpr::Goto(chain) => {
            out.put_nested(NFT_EXPR_IMMEDIATE, |inner| {
                inner.put_u32(NFT_IMMEDIATE_DREG, NFT_REG_VERDICT);
                inner.put_str(NFT_IMMEDIATE_DATA, chain);
            });
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Op encoders
// ═══════════════════════════════════════════════════════════════════════════

/// Build the full netlink message: nlmsghdr + nfgenmsg + attrs.
/// Returns (nlmsg_type, body) where body is the bytes that follow the
/// 16-byte nlmsghdr (nfgenmsg + attrs).
pub fn encode_op(op: &NetlinkOp) -> (u16, Vec<u8>) {
    match op {
        NetlinkOp::AddTable { family, name } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_TABLE_NAME, name);
            (NFT_MSG_NEWTABLE, build_message(*family, b.finish()))
        }
        NetlinkOp::DelTable { family, name } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_TABLE_NAME, name);
            (NFT_MSG_DELTABLE, build_message(*family, b.finish()))
        }
        NetlinkOp::AddChain { family, table, chain } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_CHAIN_TABLE, table);
            b.put_str(NFTA_CHAIN_NAME, &chain.name);
            b.put_str(NFTA_CHAIN_TYPE, chain.kind.as_str());
            if let Some(hook) = chain.hook {
                b.put_u32(NFTA_CHAIN_HOOKNUM, hook.as_u32());
                b.put_u32(NFTA_CHAIN_PRIORITY, chain.priority as u32);
            }
            b.put_u32(NFTA_CHAIN_POLICY, chain.policy.as_u32());
            (NFT_MSG_NEWCHAIN, build_message(*family, b.finish()))
        }
        NetlinkOp::DelChain { family, table, name } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_CHAIN_TABLE, table);
            b.put_str(NFTA_CHAIN_NAME, name);
            (NFT_MSG_DELCHAIN, build_message(*family, b.finish()))
        }
        NetlinkOp::AddRule { family, table, chain, rule } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_RULE_TABLE, table);
            b.put_str(NFTA_RULE_CHAIN, chain);
            b.put_nested(NFTA_RULE_EXPRESSIONS, |list| {
                for expr in &rule.exprs {
                    encode_expr(expr, list);
                }
            });
            if let Some(h) = rule.handle {
                b.put_u64(NFTA_RULE_HANDLE, h);
            }
            (NFT_MSG_NEWRULE, build_message(*family, b.finish()))
        }
        NetlinkOp::DelRule { family, table, chain, handle } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_RULE_TABLE, table);
            b.put_str(NFTA_RULE_CHAIN, chain);
            b.put_u64(NFTA_RULE_HANDLE, *handle);
            (NFT_MSG_DELRULE, build_message(*family, b.finish()))
        }
        NetlinkOp::AddSet { family, table, set } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_SET_TABLE, table);
            b.put_str(NFTA_SET_NAME, &set.name);
            b.put_u32(NFTA_SET_KEY_TYPE, type_name_to_u32(&set.key_type));
            b.put_u32(NFTA_SET_KEY_LEN, set.key_len);
            if set.data_len > 0 {
                b.put_u32(NFTA_SET_DATA_LEN, set.data_len);
            }
            if !set.elements.is_empty() {
                b.put_nested(NFTA_SET_ELEMENTS, |list| {
                    for el in &set.elements {
                        list.put_nested(NFTA_SET_ELEM_KEY, |elem| {
                            elem.put_slice(NFTA_SET_ELEM_DATA, el);
                        });
                    }
                });
            }
            (NFT_MSG_NEWSET, build_message(*family, b.finish()))
        }
        NetlinkOp::DelSet { family, table, name } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_SET_TABLE, table);
            b.put_str(NFTA_SET_NAME, name);
            (NFT_MSG_DELSET, build_message(*family, b.finish()))
        }
        NetlinkOp::SetFlush { family, table, name } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_SET_TABLE, table);
            b.put_str(NFTA_SET_NAME, name);
            (NFT_MSG_DELSET, build_message(*family, b.finish()))
        }
        NetlinkOp::AddCounter { family, table, counter } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_OBJ_TABLE, table);
            b.put_str(NFTA_OBJ_NAME, &counter.name);
            b.put_u32(NFTA_OBJ_TYPE, NFT_OBJECT_COUNTER);
            b.put_nested(NFTA_OBJ_DATA, |data| {
                data.put_u64(NFTA_COUNTER_BYTES, 0);
                data.put_u64(NFTA_COUNTER_PACKETS, 0);
            });
            (NFT_MSG_NEWOBJ, build_message(*family, b.finish()))
        }
        NetlinkOp::DelCounter { family, table, name } => {
            let mut b = NlaBuf::new();
            b.put_str(NFTA_OBJ_TABLE, table);
            b.put_str(NFTA_OBJ_NAME, name);
            b.put_u32(NFTA_OBJ_TYPE, NFT_OBJECT_COUNTER);
            (NFT_MSG_DELOBJ, build_message(*family, b.finish()))
        }
    }
}

/// Wrap the body with the 4-byte nfgenmsg header.
pub fn build_message(family: NftFamily, body: Vec<u8>) -> Vec<u8> {
    let mut msg = Vec::with_capacity(4 + body.len());
    msg.extend_from_slice(&nfgen_header(family));
    msg.extend(body);
    msg
}

/// Combine the nftables subsys and message type into a single nlmsg_type u16.
pub fn nlmsg_type(nft_msg: u16) -> u16 {
    (NFNL_SUBSYS_NFTABLES << 8) | nft_msg
}

/// Map a textual type descriptor to its NFT_DATA_* constant.
fn type_name_to_u32(name: &str) -> u32 {
    match name {
        "ipv4_addr" => 7,
        "ipv6_addr" => 8,
        "inet_proto" => 12,
        "inet_service" => 13,
        "mark" => 6,
        _ => 0,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Netlink socket
// ═══════════════════════════════════════════════════════════════════════════

/// A netlink socket bound to the NETLINK_NETFILTER protocol.
pub struct NlSocket {
    fd: OwnedFd,
}

impl NlSocket {
    /// Open a netlink socket bound to the NETLINK_NETFILTER protocol.
    /// Requires CAP_NET_ADMIN.
    pub fn open() -> std::io::Result<Self> {
        // rustix 1.1.4: socket_with(domain, type_, flags, protocol: Option<Protocol>)
        let fd = rustix::net::socket_with(
            AddressFamily::NETLINK,
            SocketType::RAW,
            SocketFlags::CLOEXEC,
            Some(Protocol::from_raw(NETLINK_NETFILTER)),
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("netlink open: {e}")))?;
        // The kernel auto-binds to pid=0 and groups=0, which is exactly what
        // we need to send messages to the NETLINK_NETFILTER subsystem.
        Ok(Self { fd })
    }

    /// Send a nftables message and return the raw reply bytes.
    pub fn send(&self, msg_type: u16, body: &[u8]) -> std::io::Result<Vec<u8>> {
        // Layout: nlmsghdr(16) + body (nfgenmsg(4) + attrs)
        let total_len = (16 + body.len()) as u32;
        let mut full = Vec::with_capacity(16 + body.len());
        full.extend_from_slice(&total_len.to_ne_bytes()); // nlmsg_len
        full.extend_from_slice(&nlmsg_type(msg_type).to_ne_bytes()); // nlmsg_type
        full.extend_from_slice(&0u16.to_ne_bytes()); // nlmsg_flags
        full.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_seq
        full.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid
        full.extend_from_slice(body);
        // Pad to 4-byte boundary
        let pad = (4 - (full.len() % 4)) % 4;
        if pad > 0 {
            full.resize(full.len() + pad, 0);
        }

        // For NETLINK, the destination is always the kernel (pid 0).
        // Use `send` rather than `sendto` since we don't need to specify
        // a destination address.
        rustix::net::send(self.fd.as_fd(), &full, rustix::net::SendFlags::empty())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("netlink send: {e}")))?;

        let mut reply = vec![0u8; 8192];
        // rustix 1.1.4 recv returns (Buf::Output, usize); for &mut [u8] the
        // first element is the count of bytes read.
        let (n, _flags) =
            rustix::net::recv(self.fd.as_fd(), &mut reply[..], rustix::net::RecvFlags::empty())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("netlink recv: {e}")))?;
        reply.truncate(n);
        Ok(reply)
    }

    /// Borrow the underlying fd.
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Raw fd.
    pub fn raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nlabuf_put_u32_padded() {
        let mut b = NlaBuf::new();
        b.put_u32(1, 0xdeadbeef);
        assert_eq!(b.len(), 8);
        assert_eq!(&b.as_slice()[0..2], &8u16.to_ne_bytes());
        assert_eq!(&b.as_slice()[2..4], &1u16.to_ne_bytes());
        assert_eq!(&b.as_slice()[4..8], &0xdeadbeefu32.to_ne_bytes());
    }

    #[test]
    fn nlabuf_put_str_nul_padded() {
        let mut b = NlaBuf::new();
        b.put_str(1, "hi");
        // header(4) + "hi"(2) + NUL(1) + pad(1) = 8
        assert_eq!(b.len(), 8);
        assert_eq!(&b.as_slice()[4..7], b"hi\0");
    }

    #[test]
    fn nlabuf_put_nested_updates_length() {
        let mut b = NlaBuf::new();
        b.put_nested(7, |inner| {
            inner.put_u32(1, 42);
        });
        // Outer nested header (4) + inner u32 (4 hdr + 4 data = 8) = 12
        assert_eq!(b.len(), 12);
        let len = u16::from_ne_bytes([b.as_slice()[0], b.as_slice()[1]]);
        assert_eq!(len & NLA_F_NESTED, NLA_F_NESTED);
        assert_eq!(len & !NLA_F_NESTED, 12);
    }

    #[test]
    fn nfgen_header_values() {
        let h = nfgen_header(NftFamily::Ip);
        assert_eq!(h[0], 2);
        let h6 = nfgen_header(NftFamily::Ip6);
        assert_eq!(h6[0], 10);
    }

    #[test]
    fn nlmsg_type_combine() {
        assert_eq!(nlmsg_type(NFT_MSG_NEWTABLE), (NFNL_SUBSYS_NFTABLES << 8));
    }

    #[test]
    fn encode_add_table() {
        let op = NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "filter".into(),
        };
        let (msg_type, body) = encode_op(&op);
        assert_eq!(msg_type, NFT_MSG_NEWTABLE);
        // nfgen(4) + nla header(4) + "filter\0"(7) + pad(1) = 16
        assert_eq!(body.len(), 16);
        assert_eq!(body[0], 2); // family = ip
    }

    #[test]
    fn encode_add_chain_with_hook() {
        let op = NetlinkOp::AddChain {
            family: NftFamily::Ip,
            table: "filter".into(),
            chain: NftChain::base(
                "input",
                NftChainKind::Filter,
                NftHook::Input,
                0,
                NftPolicy::Accept,
            ),
        };
        let (msg_type, body) = encode_op(&op);
        assert_eq!(msg_type, NFT_MSG_NEWCHAIN);
        assert!(body.len() > 32);
    }

    #[test]
    fn encode_add_set_with_elements() {
        let op = NetlinkOp::AddSet {
            family: NftFamily::Ip,
            table: "filter".into(),
            set: NftSet::ipv4("myset").with_ipv4(Ipv4Addr::new(10, 0, 0, 1)),
        };
        let (msg_type, body) = encode_op(&op);
        assert_eq!(msg_type, NFT_MSG_NEWSET);
        assert!(body.len() > 32);
    }

    #[test]
    fn encode_add_counter() {
        let op = NetlinkOp::AddCounter {
            family: NftFamily::Ip,
            table: "filter".into(),
            counter: NftCounter::new("c1"),
        };
        let (msg_type, body) = encode_op(&op);
        assert_eq!(msg_type, NFT_MSG_NEWOBJ);
        assert!(body.len() > 16);
    }

    #[test]
    fn encode_drop_rule_uses_nested_drop_expr() {
        let op = NetlinkOp::AddRule {
            family: NftFamily::Ip,
            table: "filter".into(),
            chain: "input".into(),
            rule: NftRule::drop(),
        };
        let (msg_type, body) = encode_op(&op);
        assert_eq!(msg_type, NFT_MSG_NEWRULE);
        let needle = NFT_EXPR_DROP.to_ne_bytes();
        assert!(body.windows(2).any(|w| w == needle));
    }

    #[test]
    fn encode_jump_rule() {
        let op = NetlinkOp::AddRule {
            family: NftFamily::Ip,
            table: "filter".into(),
            chain: "input".into(),
            rule: NftRule::jump("KUBE-SVC-ABC"),
        };
        let (_, body) = encode_op(&op);
        assert!(body.windows(12).any(|w| w == b"KUBE-SVC-ABC"));
    }

    #[test]
    fn type_name_to_u32_known() {
        assert_eq!(type_name_to_u32("ipv4_addr"), 7);
        assert_eq!(type_name_to_u32("ipv6_addr"), 8);
        assert_eq!(type_name_to_u32("unknown"), 0);
    }

    #[test]
    fn encode_meta_expr_emits_meta_key() {
        let mut b = NlaBuf::new();
        encode_expr(
            &NftExpr::Meta {
                kind: NFT_META_PROTOCOL as u32,
                op: 0,
                value: 0,
            },
            &mut b,
        );
        let needle = NFT_META_KEY.to_ne_bytes();
        assert!(b.as_slice().windows(2).any(|w| w == needle));
    }

    #[test]
    fn encode_payload_expr_emits_offset_len() {
        let mut b = NlaBuf::new();
        encode_expr(
            &NftExpr::Payload {
                dreg: 1,
                base: 0,
                offset: 9,
                len: 1,
            },
            &mut b,
        );
        let needle = NFT_PAYLOAD_OFFSET.to_ne_bytes();
        assert!(b.as_slice().windows(2).any(|w| w == needle));
    }

    #[test]
    fn nlabuf_put_slice_with_padding() {
        let mut b = NlaBuf::new();
        b.put_slice(1, &[0xab, 0xcd, 0xef]);
        assert_eq!(b.len(), 8);
        assert_eq!(b.as_slice()[4], 0xab);
        assert_eq!(b.as_slice()[5], 0xcd);
        assert_eq!(b.as_slice()[6], 0xef);
        assert_eq!(b.as_slice()[7], 0);
    }
}
