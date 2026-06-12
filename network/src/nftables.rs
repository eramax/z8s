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

use std::os::fd::AsRawFd;

use nix::sys::socket::{
    self, AddressFamily, MsgFlags, NetlinkAddr, SockFlag, SockProtocol, SockType,
};

use crate::model::*;

// ═══════════════════════════════════════════════════════════════════════════
// Netlink constants
// ═══════════════════════════════════════════════════════════════════════════

const NFNL_SUBSYS_NFTABLES: u16 = 10;

// Nftables netlink message types (matching kernel enum nf_tables_msg_types)
const NFT_MSG_NEWTABLE: u16 = 0;
const NFT_MSG_GETTABLE: u16 = 1;
const NFT_MSG_DELTABLE: u16 = 2;
const NFT_MSG_NEWCHAIN: u16 = 3;
const NFT_MSG_DELCHAIN: u16 = 5;
const NFT_MSG_NEWRULE: u16 = 6;
const NFT_MSG_DELRULE: u16 = 8;
const NFT_MSG_NEWSET: u16 = 9;
const NFT_MSG_DELSET: u16 = 11;
const NFT_MSG_NEWSETELEM: u16 = 12;
const NFT_MSG_NEWOBJ: u16 = 18;
const NFT_MSG_DELOBJ: u16 = 20;

// Nftables attribute types
const NFTA_TABLE_NAME: u16 = 1;
const NFTA_TABLE_FLAGS: u16 = 2;
const NFTA_CHAIN_TABLE: u16 = 1;
const NFTA_CHAIN_NAME: u16 = 3;
const NFTA_CHAIN_HOOK: u16 = 4;
const NFTA_CHAIN_POLICY: u16 = 5;
const NFTA_CHAIN_TYPE: u16 = 7;
// Hook sub-attributes (inside the nested NFTA_CHAIN_HOOK)
const NFTA_HOOK_HOOKNUM: u16 = 1;
const NFTA_HOOK_PRIORITY: u16 = 2;
const NFTA_RULE_TABLE: u16 = 1;
const NFTA_RULE_CHAIN: u16 = 2;
const NFTA_RULE_HANDLE: u16 = 3;
const NFTA_RULE_EXPRESSIONS: u16 = 4;
const NFTA_RULE_USERDATA: u16 = 7;
const NFTA_SET_TABLE: u16 = 1;
const NFTA_SET_NAME: u16 = 2;
const NFTA_SET_FLAGS: u16 = 3;
const NFTA_SET_KEY_TYPE: u16 = 4;
const NFTA_SET_KEY_LEN: u16 = 5;
const NFTA_SET_DATA_LEN: u16 = 7;
const NFTA_SET_ELEMENTS: u16 = 13;
const NFTA_SET_ELEM_KEY: u16 = 1;
const NFTA_SET_ID: u16 = 10;
const NFTA_OBJ_TABLE: u16 = 1;
const NFTA_OBJ_NAME: u16 = 2;
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
const NFT_GOTO: u32 = 0xFFFF_FFFC; // -4
const NFT_RETURN: u32 = 0xFFFF_FFFB; // -5

// Expression attrs.
const NFTA_META_DREG: u16 = 1;
const NFTA_META_KEY: u16 = 2;
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
const NFTA_LOOKUP_SET_ID: u16 = 4;
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
// Conntrack expression attributes
const NFTA_CT_DREG: u16 = 1;
const NFTA_CT_KEY: u16 = 2;

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
// From linux/netfilter/nfnetlink.h: NLMSG_MIN_TYPE=16, so BEGIN=16, END=17.
const NFNL_MSG_BATCH_BEGIN: u16 = 16;
const NFNL_MSG_BATCH_END: u16 = 17;

const NLMSG_ERROR_TYPE: u16 = 2;
const NLMSG_DONE_TYPE: u16 = 3;

/// Compute the nlmsg_flags for a given op.
pub fn nlmsg_flags_for(op: &NetlinkOp) -> u16 {
    let base = NLM_F_REQUEST | NLM_F_ACK;
    match op {
        NetlinkOp::AddTable { .. } => base | NLM_F_CREATE,
        NetlinkOp::AddChain { .. } | NetlinkOp::AddSet { .. } | NetlinkOp::AddSetElements { .. } | NetlinkOp::AddCounter { .. } => {
            base | NLM_F_CREATE
        }
        NetlinkOp::AddRule { .. } => base | NLM_F_CREATE | NLM_F_APPEND,
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

    /// Append a u32 in NLA format. Netlink attribute values are big-endian.
    pub fn put_u32(&mut self, kind: u16, val: u32) {
        self.put_slice(kind, &val.to_be_bytes());
    }

    /// Append a u64 in NLA format. Netlink attribute values are big-endian.
    pub fn put_u64(&mut self, kind: u16, val: u64) {
        self.put_slice(kind, &val.to_be_bytes());
    }

    /// Append a C-string in NLA format (padded to 4-byte boundary).
    /// String is NUL-terminated (matching nft CLI wire format exactly).
    pub fn put_str(&mut self, kind: u16, s: &str) {
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0); // null-terminate
        let nla_len = 4 + bytes.len();
        let pad = (4 - (nla_len % 4)) % 4;
        self.bytes.extend_from_slice(&(nla_len as u16).to_ne_bytes());
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(&bytes);
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
            d.put_u32(NFTA_LOOKUP_SET_ID, 1);
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
        NftExpr::Conntrack { dreg, key } => put_expr(out, "ct", |d| {
            d.put_u32(NFTA_CT_DREG, *dreg);
            d.put_u32(NFTA_CT_KEY, *key);
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
            b.put_u32(NFTA_TABLE_FLAGS, 0);
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
            // Only send TYPE + HOOK + POLICY for base chains (with hook).
            // Regular chains get only TABLE + NAME (matching pelagos/nft CLI).
            if let Some(hook) = chain.hook {
                b.put_str(NFTA_CHAIN_TYPE, chain.kind.as_str());
                b.put_nested(NFTA_CHAIN_HOOK, |h| {
                    h.put_u32(NFTA_HOOK_HOOKNUM, hook.as_u32());
                    // Priority must be encoded as i32 (matching pelagos/nft CLI).
                    let prio = chain.priority;
                    h.put_slice(NFTA_HOOK_PRIORITY, &prio.to_be_bytes());
                });
                b.put_u32(NFTA_CHAIN_POLICY, chain.policy.as_u32());
            }
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
            if let Some(ref comment) = rule.comment {
                // libnftnl udata TLV format: {type: u8, len: u8, value[len]}
                // NFTNL_UDATA_RULE_COMMENT = 0, value includes NUL terminator.
                let mut ud = Vec::with_capacity(2 + comment.len() + 1);
                ud.push(0u8); // type = NFTNL_UDATA_RULE_COMMENT
                ud.push((comment.len() + 1) as u8); // len includes NUL
                ud.extend_from_slice(comment.as_bytes());
                ud.push(0u8); // NUL terminator
                b.put_slice(NFTA_RULE_USERDATA, &ud);
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
            b.put_u32(NFTA_SET_FLAGS, 0);
            b.put_u32(NFTA_SET_KEY_TYPE, type_name_to_u32(&set.key_type));
            b.put_u32(NFTA_SET_KEY_LEN, set.key_len);
            b.put_u32(NFTA_SET_ID, 1);
            if set.data_len > 0 {
                b.put_u32(NFTA_SET_DATA_LEN, set.data_len);
            }
            // Elements are handled as separate NEWSETELEM ops in send_batch.
            (NFT_MSG_NEWSET, build_message(*family, b.finish()))
        }
        NetlinkOp::AddSetElements { family, table, set_name, elements } => {
            // NFT_MSG_NEWSETELEM uses NFTA_SET_ELEM_LIST_* attributes (not NFTA_SET_*)
            let mut b = NlaBuf::new();
            b.put_str(1, table);   // NFTA_SET_ELEM_LIST_TABLE
            b.put_str(2, set_name); // NFTA_SET_ELEM_LIST_SET
            b.put_u32(4, 1);       // NFTA_SET_ELEM_LIST_SET_ID
            b.put_nested(3, |list| { // NFTA_SET_ELEM_LIST_ELEMENTS
                for el in elements {
                    list.put_nested(NFTA_LIST_ELEM, |item| {
                        item.put_nested(NFTA_SET_ELEM_KEY, |key| {
                            key.put_slice(NFTA_DATA_VALUE, el);
                        });
                    });
                }
            });
            (NFT_MSG_NEWSETELEM, build_message(*family, b.finish()))
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

/// Nftables set flags (bitmask).
const NFT_SET_ANONYMOUS: u32 = 1;
const NFT_SET_CONSTANT: u32 = 0x20;

// ═══════════════════════════════════════════════════════════════════════════
// Netlink socket — uses nix for open/bind, libc for sendmsg/recvmsg
// ═══════════════════════════════════════════════════════════════════════════

/// A netlink socket bound to the NETLINK_NETFILTER protocol.
pub struct NlSocket {
    sock: std::os::fd::OwnedFd,
}

impl NlSocket {
    /// Open a netlink socket bound to the NETLINK_NETFILTER protocol.
    pub fn open() -> std::io::Result<Self> {
        let sock = socket::socket(
            AddressFamily::Netlink,
            SockType::Raw,
            SockFlag::empty(),
            SockProtocol::NetlinkNetFilter,
        )
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        let addr = NetlinkAddr::new(0, 0);
        socket::bind(sock.as_raw_fd(), &addr)
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        Ok(Self { sock })
    }

    /// Send a nftables message wrapped in a batch begin/end envelope.
    /// Opens a fresh socket per call.  Matches pelagos/nft CLI exactly:
    /// - BATCH_BEGIN: NLM_F_REQUEST only (no ACK)
    /// - Op:          NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK
    /// - BATCH_END:   NLM_F_REQUEST only
    /// Uses libc::sendmsg/recvmsg with iovec.
    pub fn send(&self, op: &NetlinkOp) -> std::io::Result<Vec<u8>> {
        let fd = open_netlink_fd()?;
        let (msg_type, body) = encode_op(op);
        let seq = 1u32;

        // Build op: NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK (matching pelagos)
        let op_flags = match op {
            NetlinkOp::AddTable { .. }
            | NetlinkOp::AddChain { .. }
            | NetlinkOp::AddSet { .. }
            | NetlinkOp::AddCounter { .. } => NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK,
            NetlinkOp::AddRule { .. } => NLM_F_REQUEST | NLM_F_CREATE | NLM_F_APPEND | NLM_F_ACK,
            _ => NLM_F_REQUEST | NLM_F_ACK,
        };
        let op_msg = build_op_msg(msg_type, op_flags, &body, seq);

        // Assemble batch: BATCH_BEGIN + op + BATCH_END
        let mut batch = Vec::with_capacity(40 + op_msg.len());
        push_batch_ctrl(&mut batch, NFNL_MSG_BATCH_BEGIN, 0);
        batch.extend_from_slice(&op_msg);
        push_batch_ctrl(&mut batch, NFNL_MSG_BATCH_END, 2); // seq=2 = num_acks+1

        send_and_drain_acks(fd, &batch, 1)?; // 1 ACK expected
        Ok(vec![])
    }

    /// Raw fd.
    pub fn raw_fd(&self) -> i32 {
        self.sock.as_raw_fd()
    }
}

impl Drop for NlSocket {
    fn drop(&mut self) {
        // fd is closed by OwnedFd
    }
}

// ── Low-level batch helpers (matching pelagos nfnetlink.rs) ────────────────

fn open_netlink_fd() -> std::io::Result<std::os::fd::RawFd> {
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_NETFILTER,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    sa.nl_family = libc::AF_NETLINK as u16;
    let rc = unsafe {
        libc::bind(
            fd,
            &sa as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        )
    };
    if rc < 0 {
        unsafe { libc::close(fd) };
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

fn push_batch_ctrl(buf: &mut Vec<u8>, msg_type: u16, seq: u32) {
    let start = buf.len();
    buf.extend_from_slice(&0u32.to_ne_bytes()); // placeholder len
    buf.extend_from_slice(&msg_type.to_ne_bytes());
    buf.extend_from_slice(&NLM_F_REQUEST.to_ne_bytes());
    buf.extend_from_slice(&seq.to_ne_bytes());
    buf.extend_from_slice(&0u32.to_ne_bytes()); // pid
    buf.push(0); // nfgenmsg: family=AF_UNSPEC
    buf.push(0); // version
    buf.extend_from_slice(&10u16.to_ne_bytes()); // res_id=10
    let len = (buf.len() - start) as u32;
    buf[start..start + 4].copy_from_slice(&len.to_ne_bytes());
}

fn build_op_msg(msg_type: u16, flags: u16, body: &[u8], seq: u32) -> Vec<u8> {
    let mut msg = Vec::with_capacity(16 + body.len());
    msg.extend_from_slice(&((16 + body.len()) as u32).to_ne_bytes());
    msg.extend_from_slice(&nlmsg_type(msg_type).to_ne_bytes());
    msg.extend_from_slice(&flags.to_ne_bytes());
    msg.extend_from_slice(&seq.to_ne_bytes());
    msg.extend_from_slice(&0u32.to_ne_bytes()); // pid
    msg.extend_from_slice(body);
    msg
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Send a batch via libc::sendmsg and drain `num_ack` NLMSG_ERROR responses.
fn send_and_drain_acks(
    fd: std::os::fd::RawFd,
    batch: &[u8],
    num_ack: usize,
) -> std::io::Result<()> {
    // Send via sendmsg with single iovec (matching pelagos)
    let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    sa.nl_family = libc::AF_NETLINK as u16;
    let iov = libc::iovec {
        iov_base: batch.as_ptr() as *mut _,
        iov_len: batch.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_name = &sa as *const _ as *mut _;
    msg.msg_namelen = std::mem::size_of::<libc::sockaddr_nl>() as u32;
    msg.msg_iov = &iov as *const _ as *mut _;
    msg.msg_iovlen = 1;

    let sent = unsafe { libc::sendmsg(fd, &mut msg, 0) };
    if sent < 0 {
        let e = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }

    // Drain ACK responses (matching pelagos recvmsg loop)
    let mut recv_buf = vec![0u8; 32768];
    let mut remaining = num_ack;
    while remaining > 0 {
        let iov_recv = libc::iovec {
            iov_base: recv_buf.as_mut_ptr() as *mut _,
            iov_len: recv_buf.len(),
        };
        let mut rmsg: libc::msghdr = unsafe { std::mem::zeroed() };
        rmsg.msg_iov = &iov_recv as *const _ as *mut _;
        rmsg.msg_iovlen = 1;
        let n = unsafe { libc::recvmsg(fd, &mut rmsg, 0) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EAGAIN) || e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            unsafe { libc::close(fd) };
            return Err(e);
        }
        let n = n as usize;
        let mut offset = 0usize;
        while offset + 16 <= n {
            let msg_len =
                u32::from_ne_bytes(recv_buf[offset..offset + 4].try_into().unwrap()) as usize;
            let msg_type = u16::from_ne_bytes(recv_buf[offset + 4..offset + 6].try_into().unwrap());
            if msg_len < 16 || offset + msg_len > n {
                break;
            }
            if msg_type == NLMSG_ERROR_TYPE && offset + 20 <= n {
                let error =
                    i32::from_ne_bytes(recv_buf[offset + 16..offset + 20].try_into().unwrap());
                if error != 0 {
                    unsafe { libc::close(fd) };
                    return Err(std::io::Error::from_raw_os_error(-error));
                }
                remaining = remaining.saturating_sub(1);
            } else if msg_type == NLMSG_DONE_TYPE {
                remaining = 0;
                break;
            }
            offset += align4(msg_len);
        }
    }
    unsafe { libc::close(fd) };
    Ok(())
}

/// Send multiple nftables ops in a single batch using a fresh socket.
/// Matches pelagos: NLM_F_ACK on each op, drains ACKs, one sendmsg call.
pub fn send_batch(ops: &[NetlinkOp]) -> std::io::Result<()> {
    if ops.is_empty() {
        return Ok(());
    }
    let fd = open_netlink_fd()?;

    let mut batch = Vec::with_capacity(40 + ops.len() * 128);
    push_batch_ctrl(&mut batch, NFNL_MSG_BATCH_BEGIN, 0);

    let mut num_acks = 0usize;
    for (i, op) in ops.iter().enumerate() {
        let (msg_type, body) = encode_op(op);
        let op_flags = match op {
            NetlinkOp::AddTable { .. }
            | NetlinkOp::AddChain { .. }
            | NetlinkOp::AddSet { .. }
            | NetlinkOp::AddSetElements { .. }
            | NetlinkOp::AddCounter { .. } => NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK,
            NetlinkOp::AddRule { .. } => NLM_F_REQUEST | NLM_F_CREATE | NLM_F_APPEND | NLM_F_ACK,
            NetlinkOp::DelTable { .. }
            | NetlinkOp::DelChain { .. }
            | NetlinkOp::DelRule { .. }
            | NetlinkOp::DelSet { .. }
            | NetlinkOp::SetFlush { .. }
            | NetlinkOp::DelCounter { .. } => NLM_F_REQUEST | NLM_F_ACK,
            NetlinkOp::AddRoute { .. } | NetlinkOp::DelRoute { .. } => NLM_F_REQUEST,
        };
        let seq = (i + 1) as u32;
        let msg = build_op_msg(msg_type, op_flags, &body, seq);
        batch.extend_from_slice(&msg);
        if op_flags & NLM_F_ACK != 0 {
            num_acks += 1;
        }
    }

    push_batch_ctrl(&mut batch, NFNL_MSG_BATCH_END, (num_acks + 1) as u32);

    send_and_drain_acks(fd, &batch, num_acks)
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// Scan `body` (nfgenmsg + attrs) for a top-level nlattr with the given
    /// type. Returns `Some(nla_type)` if found, `None` otherwise.
    fn find_attr_type(body: &[u8], target_type: u16) -> Option<u16> {
        // body starts with nfgenmsg (4 bytes), then attributes.
        let mut pos = 4;
        while pos + 4 <= body.len() {
            let nla_len = u16::from_ne_bytes([body[pos], body[pos + 1]]) as usize;
            let nla_type = u16::from_ne_bytes([body[pos + 2], body[pos + 3]]);
            if nla_len < 4 {
                break;
            }
            let base_type = nla_type & !NLA_F_NESTED;
            if base_type == target_type {
                return Some(nla_type);
            }
            pos += (nla_len + 3) & !3; // 4-byte align
        }
        None
    }

    #[test]
    fn nlabuf_put_u32_padded() {
        let mut b = NlaBuf::new();
        b.put_u32(1, 0xdeadbeef);
        assert_eq!(b.len(), 8);
        assert_eq!(&b.as_slice()[0..2], &8u16.to_ne_bytes());
        assert_eq!(&b.as_slice()[2..4], &1u16.to_ne_bytes());
        assert_eq!(&b.as_slice()[4..8], &0xdeadbeefu32.to_be_bytes());
    }

    #[test]
    fn nlabuf_put_str_padded() {
        let mut b = NlaBuf::new();
        b.put_str(1, "hi");
        // "hi\0" = 3 bytes, nla_len = 4 + 3 = 7, padded to 8
        assert_eq!(b.len(), 8);
        assert_eq!(&b.as_slice()[4..6], b"hi");
        assert_eq!(b.as_slice()[6], 0); // NUL terminator
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
        // body = nfgenmsg(4) + NLA TABLE_NAME("filter\0" padded to 12) + NLA TABLE_FLAGS(8)
        // = 4 + 12 + 8 = 24
        assert_eq!(body.len(), 24);
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
        // The chain should have nested hook attribute (NFTA_CHAIN_HOOK)
        // which contains NFTA_HOOK_HOOKNUM and NFTA_HOOK_PRIORITY.
        // Verify the hook attribute is nested (high bit set in type field).
        let hook_attr_type = find_attr_type(&body, NFTA_CHAIN_HOOK);
        assert!(hook_attr_type.is_some(), "NFTA_CHAIN_HOOK not found in chain body");
        let hook_type = hook_attr_type.unwrap();
        assert_ne!(hook_type & NLA_F_NESTED, 0, "NFTA_CHAIN_HOOK should be nested");
    }

    #[test]
    fn encode_add_chain_without_hook() {
        // Regular (non-hook) chains should not have NFTA_CHAIN_HOOK
        let op = NetlinkOp::AddChain {
            family: NftFamily::Ip,
            table: "filter".into(),
            chain: NftChain::regular("mychain", NftChainKind::Filter),
        };
        let (msg_type, body) = encode_op(&op);
        assert_eq!(msg_type, NFT_MSG_NEWCHAIN);
        assert!(find_attr_type(&body, NFTA_CHAIN_HOOK).is_none());
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
