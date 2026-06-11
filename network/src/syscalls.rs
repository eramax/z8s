//! # Nftables Netlink Syscalls
//!
//! Encodes nftables netlink messages and sends them to the kernel.
//!
//! ## Why hand-rolled (mostly)
//!
//! We do not depend on `rustables` (third-party) or `libc`. We use
//! [`neli`] for the netlink socket plumbing (open, bind, send, recv)
//! because rustix 1.1.4 does not publicly export `SocketAddrNetlink`.
//! The nftables attribute encoding (NLA layout, nested attrs, padding,
//! nfgenmsg, nlmsg_flags) is hand-rolled in [`NlaBuf`] and [`encode_op`].
//!
//! ## Wire format
//!
//! Every message is `nlmsghdr(16) | nfgenmsg(4) | attrs`, where:
//!
//! - `nlmsghdr.nlmsg_type = (NFNL_SUBSYS_NFTABLES << 8) | NFT_MSG_*`
//! - `nlmsghdr.nlmsg_flags` is computed by [`nlmsg_flags_for`]
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

use std::os::fd::{AsRawFd, OwnedFd};

use nix::sys::socket::{
    self, AddressFamily, MsgFlags, NetlinkAddr, SockFlag, SockProtocol, SockType,
};

use crate::model::*;

// ═══════════════════════════════════════════════════════════════════════════
// Netlink constants
// ═══════════════════════════════════════════════════════════════════════════

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
const NFT_MSG_NEWOBJ: u16 = 12;
const NFT_MSG_DELOBJ: u16 = 13;

// Nftables attribute types
const NFTA_TABLE_NAME: u16 = 1;
const NFTA_TABLE_FLAGS: u16 = 2;
const NFTA_TABLE_USERDATA: u16 = 6;
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
const NFTA_OBJ_NAME: u16 = 1;
const NFTA_OBJ_TABLE: u16 = 2;
const NFTA_OBJ_TYPE: u16 = 3;
const NFTA_OBJ_DATA: u16 = 4;
const NFTA_COUNTER_BYTES: u16 = 1;
const NFTA_COUNTER_PACKETS: u16 = 2;

// NLA flags
const NLA_F_NESTED: u16 = 0x8000;

// Generic list-element / expression wrappers.
// A rule's NFTA_RULE_EXPRESSIONS holds a series of NFTA_LIST_ELEM, each
// carrying NFTA_EXPR_NAME ("cmp", "payload", …) + NFTA_EXPR_DATA (nested).
const NFTA_LIST_ELEM: u16 = 1;
const NFTA_EXPR_NAME: u16 = 1;
const NFTA_EXPR_DATA: u16 = 2;

// Typed register data: NFTA_DATA_VALUE for raw bytes, NFTA_DATA_VERDICT for
// a verdict (accept/drop/jump/…).
const NFTA_DATA_VALUE: u16 = 1;
const NFTA_DATA_VERDICT: u16 = 2;
const NFTA_VERDICT_CODE: u16 = 1;
const NFTA_VERDICT_CHAIN: u16 = 2;

// Verdict codes (stored big-endian). Negative nf-tables verdicts wrap around.
const NF_DROP: u32 = 0;
const NF_ACCEPT: u32 = 1;
const NFT_JUMP: u32 = 0xFFFF_FFFD; // -3
const NFT_GOTO: u32 = 0xFFFF_FFFE; // -2
const NFT_RETURN: u32 = 0xFFFF_FFFB; // -5

// Expression attrs.
const NFTA_META_DREG: u16 = 2;
const NFTA_META_KEY: u16 = 1;
const NFTA_CMP_SREG: u16 = 1;
const NFTA_CMP_OP: u16 = 2;
const NFTA_CMP_DATA: u16 = 3;
const NFTA_PAYLOAD_DREG: u16 = 1;
const NFTA_PAYLOAD_BASE: u16 = 2;
const NFTA_PAYLOAD_OFFSET: u16 = 3;
const NFTA_PAYLOAD_LEN: u16 = 4;
const NFTA_IMMEDIATE_DREG: u16 = 1;
const NFTA_IMMEDIATE_DATA: u16 = 2;
const NFTA_LOOKUP_SET: u16 = 1;
const NFTA_LOOKUP_SREG: u16 = 2;
const NFTA_NAT_TYPE: u16 = 1;
const NFTA_NAT_FAMILY: u16 = 2;
const NFTA_NAT_REG_ADDR_MIN: u16 = 3;
const NFTA_NAT_REG_ADDR_MAX: u16 = 4;
const NFTA_NAT_REG_PROTO_MIN: u16 = 5;
const NFTA_NAT_REG_PROTO_MAX: u16 = 6;
const NFTA_BITWISE_SREG: u16 = 1;
const NFTA_BITWISE_DREG: u16 = 2;
const NFTA_BITWISE_LEN: u16 = 3;
const NFTA_BITWISE_MASK: u16 = 4;
const NFTA_BITWISE_XOR: u16 = 5;
const NFTA_NG_DREG: u16 = 1;
const NFTA_NG_MODULUS: u16 = 2;
const NFTA_NG_TYPE: u16 = 3;
const NFTA_NG_OFFSET: u16 = 4;

// Numgen type: pseudo-random.
const NFT_NG_RANDOM: u32 = 1;

const NFT_OBJECT_COUNTER: u32 = 1;
const NFT_REG_VERDICT: u32 = 0x00;

// Netlink message flags (for nlmsghdr.nlmsg_flags).
// NLM_F_REQUEST is required for all netlink requests.
// NLM_F_ACK requests an NLMSG_ERROR reply (errno 0 on success).
// NLM_F_CREATE | NLM_F_EXCL means "create new, fail if exists" (atomic).
// NLM_F_APPEND appends rules to the end of a chain.
const NLM_F_REQUEST: u16 = 0x01;
const NLM_F_ACK: u16 = 0x04;
const NLM_F_CREATE: u16 = 0x400;
const NLM_F_EXCL: u16 = 0x200;
const NLM_F_APPEND: u16 = 0x800;

// Batch envelope constants (nf_tables requires every op to be wrapped in
// a batch on modern kernels; individual messages get EINVAL).
// The BATCH_BEGIN/END nlmsg_type is just NFNL_MSG_BATCH_BEGIN/END (the
// nfnetlink subsys for batch control is 0, so no shift is needed).
const NFNL_MSG_BATCH_BEGIN: u16 = 1;
const NFNL_MSG_BATCH_END: u16 = 2;

/// Compute the nlmsg_flags for a given op.
pub fn nlmsg_flags_for(op: &NetlinkOp) -> u16 {
    let base = NLM_F_REQUEST | NLM_F_ACK;
    match op {
        NetlinkOp::AddTable { .. }
        | NetlinkOp::AddChain { .. }
        | NetlinkOp::AddSet { .. }
        | NetlinkOp::AddCounter { .. } => base | NLM_F_CREATE | NLM_F_EXCL,
        NetlinkOp::AddRule { .. } => base | NLM_F_CREATE | NLM_F_EXCL | NLM_F_APPEND,
        NetlinkOp::DelTable { .. }
        | NetlinkOp::DelChain { .. }
        | NetlinkOp::DelRule { .. }
        | NetlinkOp::DelSet { .. }
        | NetlinkOp::SetFlush { .. }
        | NetlinkOp::DelCounter { .. } => base,
        // Route ops are RTNETLINK, not nftables; they never reach this path.
        NetlinkOp::AddRoute { .. } | NetlinkOp::DelRoute { .. } => base,
    }
}

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
    /// The `nla_len` field is the logical length (header + payload, NUL-included),
    /// NOT the padded total — the kernel reads `nla_len - 4` bytes of payload
    /// then seeks to the next attr at `nla_len` rounded up to 4 bytes.
    pub fn put_str(&mut self, kind: u16, s: &str) {
        let bytes = s.as_bytes();
        let nla_len = 4 + bytes.len() + 1; // include NUL
        let pad = (4 - (nla_len % 4)) % 4;
        self.bytes.extend_from_slice(&(nla_len as u16).to_ne_bytes());
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(bytes);
        self.bytes.push(0); // NUL terminator
        if pad > 0 {
            self.bytes.resize(self.bytes.len() + pad, 0);
        }
    }

    /// Append raw bytes in NLA format (padded to 4-byte boundary).
    /// `nla_len` is the logical length (header + payload), not the padded total.
    pub fn put_slice(&mut self, kind: u16, data: &[u8]) {
        let nla_len = 4 + data.len();
        let pad = (4 - (nla_len % 4)) % 4;
        self.bytes.extend_from_slice(&(nla_len as u16).to_ne_bytes());
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(data);
        if pad > 0 {
            self.bytes.resize(self.bytes.len() + pad, 0);
        }
    }

    /// Append a nested attribute (`NLA_F_NESTED`). Padded to 4 bytes.
    /// `nla_len` is the logical length (header + payload, NOT padded total).
    pub fn put_nested<F>(&mut self, kind: u16, f: F)
    where
        F: FnOnce(&mut NlaBuf),
    {
        let header_pos = self.bytes.len();
        self.bytes.extend_from_slice(&0u16.to_ne_bytes()); // placeholder for length
        // NLA_F_NESTED goes in the TYPE field (high bit), not the length field.
        self.bytes.extend_from_slice(&(kind | NLA_F_NESTED).to_ne_bytes());
        let payload_start = self.bytes.len();
        f(self);
        let payload_len = self.bytes.len() - payload_start;
        let nla_len = 4 + payload_len; // logical length, not padded
        let pad = (4 - (nla_len % 4)) % 4;
        if pad > 0 {
            self.bytes.resize(self.bytes.len() + pad, 0);
        }
        self.bytes[header_pos..header_pos + 2].copy_from_slice(&(nla_len as u16).to_ne_bytes());
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

/// Emit one expression as an `NFTA_LIST_ELEM { NFTA_EXPR_NAME, NFTA_EXPR_DATA }`.
fn put_expr<F>(out: &mut NlaBuf, name: &str, f: F)
where
    F: FnOnce(&mut NlaBuf),
{
    out.put_nested(NFTA_LIST_ELEM, |elem| {
        elem.put_str(NFTA_EXPR_NAME, name);
        elem.put_nested(NFTA_EXPR_DATA, f);
    });
}

/// Emit a register data attribute wrapping raw bytes (`NFTA_DATA_VALUE`).
fn put_data_value(out: &mut NlaBuf, kind: u16, bytes: &[u8]) {
    out.put_nested(kind, |d| d.put_slice(NFTA_DATA_VALUE, bytes));
}

/// Emit a verdict data attribute (`NFTA_DATA_VERDICT`), optionally with a
/// target chain (for jump/goto).
fn put_verdict(out: &mut NlaBuf, kind: u16, code: u32, chain: Option<&str>) {
    out.put_nested(kind, |d| {
        d.put_nested(NFTA_DATA_VERDICT, |v| {
            v.put_slice(NFTA_VERDICT_CODE, &code.to_be_bytes());
            if let Some(c) = chain {
                v.put_str(NFTA_VERDICT_CHAIN, c);
            }
        });
    });
}

/// Encode a single rule expression into the rule's expression list.
pub fn encode_expr(expr: &NftExpr, out: &mut NlaBuf) {
    match expr {
        NftExpr::Meta { kind, .. } => put_expr(out, "meta", |d| {
            d.put_u32(NFTA_META_KEY, *kind);
            d.put_u32(NFTA_META_DREG, 1);
        }),
        NftExpr::Cmp { sreg, op, data } => put_expr(out, "cmp", |d| {
            d.put_u32(NFTA_CMP_SREG, *sreg);
            d.put_u32(NFTA_CMP_OP, *op);
            put_data_value(d, NFTA_CMP_DATA, data);
        }),
        NftExpr::Payload {
            dreg,
            base,
            offset,
            len,
        } => put_expr(out, "payload", |d| {
            d.put_u32(NFTA_PAYLOAD_DREG, *dreg);
            d.put_u32(NFTA_PAYLOAD_BASE, *base);
            d.put_u32(NFTA_PAYLOAD_OFFSET, *offset);
            d.put_u32(NFTA_PAYLOAD_LEN, *len);
        }),
        NftExpr::Lookup { set, sreg } => put_expr(out, "lookup", |d| {
            d.put_str(NFTA_LOOKUP_SET, set);
            d.put_u32(NFTA_LOOKUP_SREG, *sreg);
        }),
        NftExpr::Immediate { dreg, data } => put_expr(out, "immediate", |d| {
            d.put_u32(NFTA_IMMEDIATE_DREG, *dreg);
            put_data_value(d, NFTA_IMMEDIATE_DATA, data);
        }),
        NftExpr::Nat {
            nat_type,
            sreg_addr,
            sreg_port,
        } => put_expr(out, "nat", |d| {
            d.put_u32(NFTA_NAT_TYPE, *nat_type);
            d.put_u32(NFTA_NAT_FAMILY, NftFamily::Ip.as_u8() as u32);
            d.put_u32(NFTA_NAT_REG_ADDR_MIN, *sreg_addr);
            d.put_u32(NFTA_NAT_REG_ADDR_MAX, *sreg_addr);
            if *sreg_port != 0xFFFF_FFFF {
                d.put_u32(NFTA_NAT_REG_PROTO_MIN, *sreg_port);
                d.put_u32(NFTA_NAT_REG_PROTO_MAX, *sreg_port);
            }
        }),
        NftExpr::Masquerade => put_expr(out, "masq", |_d| {}),
        NftExpr::Bitwise {
            sreg,
            dreg,
            len,
            mask,
            xor,
        } => put_expr(out, "bitwise", |d| {
            d.put_u32(NFTA_BITWISE_SREG, *sreg);
            d.put_u32(NFTA_BITWISE_DREG, *dreg);
            d.put_u32(NFTA_BITWISE_LEN, *len);
            put_data_value(d, NFTA_BITWISE_MASK, mask);
            put_data_value(d, NFTA_BITWISE_XOR, xor);
        }),
        NftExpr::Numgen {
            dreg,
            modulus,
            offset,
        } => put_expr(out, "numgen", |d| {
            d.put_u32(NFTA_NG_DREG, *dreg);
            d.put_u32(NFTA_NG_MODULUS, *modulus);
            d.put_u32(NFTA_NG_TYPE, NFT_NG_RANDOM);
            d.put_u32(NFTA_NG_OFFSET, *offset);
        }),
        NftExpr::Accept => put_expr(out, "immediate", |d| {
            d.put_u32(NFTA_IMMEDIATE_DREG, NFT_REG_VERDICT);
            put_verdict(d, NFTA_IMMEDIATE_DATA, NF_ACCEPT, None);
        }),
        NftExpr::Drop => put_expr(out, "immediate", |d| {
            d.put_u32(NFTA_IMMEDIATE_DREG, NFT_REG_VERDICT);
            put_verdict(d, NFTA_IMMEDIATE_DATA, NF_DROP, None);
        }),
        NftExpr::Return => put_expr(out, "immediate", |d| {
            d.put_u32(NFTA_IMMEDIATE_DREG, NFT_REG_VERDICT);
            put_verdict(d, NFTA_IMMEDIATE_DATA, NFT_RETURN, None);
        }),
        NftExpr::Jump(chain) => put_expr(out, "immediate", |d| {
            d.put_u32(NFTA_IMMEDIATE_DREG, NFT_REG_VERDICT);
            put_verdict(d, NFTA_IMMEDIATE_DATA, NFT_JUMP, Some(chain));
        }),
        NftExpr::Goto(chain) => put_expr(out, "immediate", |d| {
            d.put_u32(NFTA_IMMEDIATE_DREG, NFT_REG_VERDICT);
            put_verdict(d, NFTA_IMMEDIATE_DATA, NFT_GOTO, Some(chain));
        }),
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
            // NFTA_TABLE_FLAGS is required by the kernel (value 0 = no flags).
            // Both nft CLI and rustables send this; without it the kernel
            // rejects the op with EINVAL.
            b.put_u32(NFTA_TABLE_FLAGS, 0);
            // NFTA_TABLE_USERDATA (16 bytes of zeros) — nft CLI sends this
            // for its own internal bookkeeping. The kernel accepts an empty
            // value, but sending it matches nft CLI's wire format exactly.
            b.put_slice(NFTA_TABLE_USERDATA, &[0u8; 16]);
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
                        list.put_nested(NFTA_LIST_ELEM, |item| {
                            item.put_nested(NFTA_SET_ELEM_KEY, |key| {
                                key.put_slice(NFTA_DATA_VALUE, el);
                            });
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
        NetlinkOp::AddRoute { .. } | NetlinkOp::DelRoute { .. } => {
            // Route ops belong to RTNETLINK (crate::rtnetlink), never the
            // netfilter socket. The engine dispatches them before this point.
            panic!("encode_op called with a route op: {op:?}")
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
// Netlink socket — uses nix (same as rustables) for open/bind/send/recv
// ═══════════════════════════════════════════════════════════════════════════

/// A netlink socket bound to the NETLINK_NETFILTER protocol.
/// Uses [`nix`] for open/bind/send/recv (same approach as the rustables
/// crate, which is a known-working nftables implementation).
pub struct NlSocket {
    sock: OwnedFd,
}

impl NlSocket {
    /// Open a netlink socket bound to the NETLINK_NETFILTER protocol.
    /// Requires CAP_NET_ADMIN.
    pub fn open() -> std::io::Result<Self> {
        let sock = socket::socket(
            AddressFamily::Netlink,
            SockType::Raw,
            SockFlag::empty(),
            SockProtocol::NetlinkNetFilter,
        )
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        // Bind to (pid=0, groups=0). pid=0 means "auto-assign" by the kernel.
        let addr = NetlinkAddr::new(0, 0);
        socket::bind(sock.as_raw_fd(), &addr)
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        Ok(Self { sock })
    }

    /// Send a nftables message wrapped in a batch begin/end envelope.
    /// Modern nftables kernels reject individual messages with EINVAL;
    /// every op must be inside NFNL_MSG_BATCH_BEGIN .. NFNL_MSG_BATCH_END.
    /// The single ACK is read after BATCH_END.
    pub fn send(&self, op: &NetlinkOp) -> std::io::Result<Vec<u8>> {
        let (msg_type, body) = encode_op(op);

        // ── Wire format note ──────────────────────────────────────────
        // nft CLI uses nlmsg_pid=0 in every message of the batch. The
        // kernel uses the *bound* pid (set by `bind()`) internally to
        // route replies, not the nlmsg_pid in the message body. The body
        // field is the source pid and should be 0 for userspace-originated
        // messages.
        //
        // BATCH_BEGIN and BATCH_END also carry a 4-byte nfgenmsg after the
        // nlmsghdr, with `res_id` set to the nftables subsys number (10).
        // nlmsg_seq is sequential: 0, 1, 2 for the three messages.
        //
        // The op's flags are NLM_F_REQUEST only (NLM_F_ACK, NLM_F_CREATE,
        // and NLM_F_EXCL are all stripped) — matching nft CLI exactly.
        // The kernel sends one NLMSG_ERROR after the batch commits,
        // regardless of NLM_F_ACK, because the batch is a transaction.

        // Build BATCH_BEGIN: nlmsghdr(16) + nfgenmsg(4) = 20 bytes.
        // Flags: NLM_F_REQUEST | NLM_F_ACK (rustables pattern — the kernel
        // sends an ACK for BATCH_BEGIN because it carries NLM_F_ACK).
        let begin_type = NFNL_MSG_BATCH_BEGIN;
        let mut begin = Vec::with_capacity(20);
        begin.extend_from_slice(&20u32.to_ne_bytes()); // nlmsg_len = 20
        begin.extend_from_slice(&begin_type.to_ne_bytes());
        begin.extend_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
        begin.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_seq = 0
        begin.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid = 0
        // nfgenmsg: family=AF_UNSPEC(0), version=0, res_id=10 (network byte order)
        begin.extend_from_slice(&[0u8, 0, 0, 10]);

        // Build the actual op message: nlmsghdr(16) + body.
        // Flags: NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK (rustables pattern).
        // The kernel sends an ACK for the op because it carries NLM_F_ACK.
        let op_flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK;
        let total_len = (16 + body.len()) as u32;
        let mut msg = Vec::with_capacity(16 + body.len());
        msg.extend_from_slice(&total_len.to_ne_bytes());
        msg.extend_from_slice(&nlmsg_type(msg_type).to_ne_bytes());
        msg.extend_from_slice(&op_flags.to_ne_bytes());
        msg.extend_from_slice(&1u32.to_ne_bytes()); // nlmsg_seq = 1
        msg.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid = 0
        msg.extend_from_slice(&body);
        let pad = (4 - (msg.len() % 4)) % 4;
        if pad > 0 {
            msg.resize(msg.len() + pad, 0);
        }

        // Build BATCH_END: nlmsghdr(16) + nfgenmsg(4) = 20 bytes.
        // Flags: NLM_F_REQUEST only (no NLM_F_ACK — rustables pattern).
        // The kernel commits the batch after BATCH_END and the transaction
        // is complete; no per-message ACK for BATCH_END.
        let end_type = NFNL_MSG_BATCH_END;
        let mut end = Vec::with_capacity(20);
        end.extend_from_slice(&20u32.to_ne_bytes()); // nlmsg_len = 20
        end.extend_from_slice(&end_type.to_ne_bytes());
        end.extend_from_slice(&NLM_F_REQUEST.to_ne_bytes());
        end.extend_from_slice(&2u32.to_ne_bytes()); // nlmsg_seq = 2
        end.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid = 0
        // nfgenmsg: family=AF_UNSPEC, version=0, res_id=10 (network byte order)
        end.extend_from_slice(&[0u8, 0, 0, 10]);

        // ── Critical: send all three messages in ONE sendmsg call ──────
        // The kernel's nfnetlink_rcv processes messages one at a time.
        // Three separate send() calls would be seen as three standalone
        // ops outside any batch — exactly the EINVAL case. nft CLI uses
        // a single sendmsg with one iovec containing all three messages
        // concatenated. We do the same: combine into one Vec and send once.
        let mut combined = Vec::with_capacity(begin.len() + msg.len() + end.len());
        combined.extend_from_slice(&begin);
        combined.extend_from_slice(&msg);
        combined.extend_from_slice(&end);

        let raw_fd = self.sock.as_raw_fd();
        // Use nix::sendto (same as rustables) — bare send() on a bound
        // netlink socket works, but sendto with an explicit NetlinkAddr
        // is what rustables does and is the most portable.
        let addr = NetlinkAddr::new(0, 0);
        socket::sendto(raw_fd, &combined, &addr, MsgFlags::empty())
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;

        // Drain ACKs: 2 expected (BATCH_BEGIN has NLM_F_ACK, op has NLM_F_ACK,
        // BATCH_END does NOT have NLM_F_ACK — matching rustables pattern).
        // Only interpret bytes 16-19 as errno if the reply is NLMSG_ERROR
        // (type 2); otherwise it's a notification or other message type.
        let mut last_reply = Vec::new();
        for _ in 0..2 {
            let mut reply = vec![0u8; 8192];
            let n = socket::recv(raw_fd, &mut reply, MsgFlags::empty())
                .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
            reply.truncate(n);
            if reply.len() >= 20 {
                // nlmsghdr layout: len(4) | type(2) | flags(2) | seq(4) | pid(4)
                let nlmsg_type = u16::from_ne_bytes([reply[4], reply[5]]);
                if nlmsg_type == 2 {
                    // NLMSG_ERROR: bytes 16-19 are the error code (i32)
                    let errno = i32::from_ne_bytes([reply[16], reply[17], reply[18], reply[19]]);
                    if errno != 0 {
                        return Err(std::io::Error::from_raw_os_error(-errno));
                    }
                }
            }
            last_reply = reply;
        }
        Ok(last_reply)
    }

    /// Raw fd.
    pub fn raw_fd(&self) -> i32 {
        self.sock.as_raw_fd()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

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
        assert_eq!(b.len(), 8);
        assert_eq!(&b.as_slice()[4..7], b"hi\0");
    }

    #[test]
    fn nlabuf_put_nested_updates_length() {
        let mut b = NlaBuf::new();
        b.put_nested(7, |inner| {
            inner.put_u32(1, 42);
        });
        assert_eq!(b.len(), 12);
        // Length field (bytes 0..2) is just the length, no flags.
        let len = u16::from_ne_bytes([b.as_slice()[0], b.as_slice()[1]]);
        assert_eq!(len, 12);
        // Type field (bytes 2..4) carries NLA_F_NESTED in the high bit.
        let kind = u16::from_ne_bytes([b.as_slice()[2], b.as_slice()[3]]);
        assert_eq!(kind & NLA_F_NESTED, NLA_F_NESTED);
        assert_eq!(kind & !NLA_F_NESTED, 7);
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
        // body = nfgenmsg(4) + NLA TABLE_NAME(12) + NLA TABLE_FLAGS(8) + NLA TABLE_USERDATA(20) = 44
        assert_eq!(body.len(), 44);
        assert_eq!(body[0], 2);
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
    fn encode_drop_rule_emits_immediate_verdict() {
        let op = NetlinkOp::AddRule {
            family: NftFamily::Ip,
            table: "filter".into(),
            chain: "input".into(),
            rule: NftRule::drop(),
        };
        let (msg_type, body) = encode_op(&op);
        assert_eq!(msg_type, NFT_MSG_NEWRULE);
        // A drop is an "immediate" expression writing the NF_DROP verdict.
        assert!(body.windows(b"immediate".len()).any(|w| w == b"immediate"));
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
    fn encode_meta_expr_emits_name() {
        let mut b = NlaBuf::new();
        encode_expr(
            &NftExpr::Meta {
                kind: 16,
                op: 0,
                value: 0,
            },
            &mut b,
        );
        assert!(b.as_slice().windows(b"meta".len()).any(|w| w == b"meta"));
    }

    #[test]
    fn encode_payload_expr_emits_name() {
        let mut b = NlaBuf::new();
        encode_expr(
            &NftExpr::Payload {
                dreg: 1,
                base: 1,
                offset: 16,
                len: 4,
            },
            &mut b,
        );
        assert!(b
            .as_slice()
            .windows(b"payload".len())
            .any(|w| w == b"payload"));
    }

    #[test]
    fn encode_bitwise_and_numgen() {
        let mut b = NlaBuf::new();
        encode_expr(
            &NftExpr::Bitwise {
                sreg: 1,
                dreg: 1,
                len: 4,
                mask: vec![255, 255, 255, 0],
                xor: vec![0, 0, 0, 0],
            },
            &mut b,
        );
        encode_expr(
            &NftExpr::Numgen {
                dreg: 9,
                modulus: 3,
                offset: 0,
            },
            &mut b,
        );
        assert!(b
            .as_slice()
            .windows(b"bitwise".len())
            .any(|w| w == b"bitwise"));
        assert!(b.as_slice().windows(b"numgen".len()).any(|w| w == b"numgen"));
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

    #[test]
    fn nlmsg_flags_for_add_ops() {
        let op = NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "t".into(),
        };
        let flags = nlmsg_flags_for(&op);
        assert!(flags & NLM_F_REQUEST != 0);
        assert!(flags & NLM_F_ACK != 0);
        assert!(flags & NLM_F_CREATE != 0);
        assert!(flags & NLM_F_EXCL != 0);
    }

    #[test]
    fn nlmsg_flags_for_del_ops() {
        let op = NetlinkOp::DelTable {
            family: NftFamily::Ip,
            name: "t".into(),
        };
        let flags = nlmsg_flags_for(&op);
        assert!(flags & NLM_F_REQUEST != 0);
        assert!(flags & NLM_F_ACK != 0);
        assert!(flags & NLM_F_CREATE == 0);
    }
}
