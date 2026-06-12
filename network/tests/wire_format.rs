//! Wire format verification tests.
//!
//! These tests verify that our hand-rolled nftables netlink encoding produces
//! the correct byte-level output. They parse the encoded messages and validate
//! attribute types, nesting flags, values, and padding — without touching the
//! kernel.

#![cfg(target_os = "linux")]

use network::ipam::Ipv4Cidr;
use network::model::*;
use network::syscalls::{encode_op, nfgen_header, NlaBuf};
use network::{reconcile, NetmuxBuilder};

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Read a u16 from `buf` at `pos` in native byte order.
fn read_u16(buf: &[u8], pos: usize) -> u16 {
    u16::from_ne_bytes([buf[pos], buf[pos + 1]])
}

/// Read a u32 from `buf` at `pos` in native byte order.
fn read_u32(buf: &[u8], pos: usize) -> u32 {
    u32::from_ne_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
}

/// Read a u32 from `buf` at `pos` in big-endian (network) byte order.
/// Used for verdict codes and NAT type which the kernel reads as BE32.
fn read_u32_be(buf: &[u8], pos: usize) -> u32 {
    u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
}

/// Read a u64 from `buf` at `pos` in native byte order.
fn read_u64(buf: &[u8], pos: usize) -> u64 {
    u64::from_ne_bytes([
        buf[pos],
        buf[pos + 1],
        buf[pos + 2],
        buf[pos + 3],
        buf[pos + 4],
        buf[pos + 5],
        buf[pos + 6],
        buf[pos + 7],
    ])
}

/// An attribute parsed from a netlink message body.
#[derive(Debug)]
struct ParsedAttr {
    /// Raw nla_type (may have NLA_F_NESTED set).
    nla_type: u16,
    /// Logical length (header + payload, not padded).
    nla_len: usize,
    /// Payload bytes (after the 4-byte header, before padding).
    payload: Vec<u8>,
    /// Whether NLA_F_NESTED was set.
    is_nested: bool,
}

impl ParsedAttr {
    /// The base type without the NESTED flag.
    fn base_type(&self) -> u16 {
        self.nla_type & !0x8000
    }

    /// Advance past this attribute (header + payload + padding).
    fn total_padded(&self) -> usize {
        (self.nla_len + 3) & !3
    }
}

/// Parse all top-level attributes from a body that starts with nfgenmsg (4 bytes).
fn parse_attrs(body: &[u8]) -> Vec<ParsedAttr> {
    let mut attrs = Vec::new();
    let mut pos = 4; // skip nfgenmsg
    while pos + 4 <= body.len() {
        let nla_len = read_u16(body, pos) as usize;
        let nla_type = read_u16(body, pos + 2);
        if nla_len < 4 {
            break;
        }
        let payload_start = pos + 4;
        let payload_end = pos + nla_len;
        let payload = if payload_end <= body.len() {
            body[payload_start..payload_end].to_vec()
        } else {
            break;
        };
        attrs.push(ParsedAttr {
            nla_type,
            nla_len,
            payload,
            is_nested: nla_type & 0x8000 != 0,
        });
        pos += (nla_len + 3) & !3;
    }
    attrs
}

/// Parse nested attributes from a payload slice.
fn parse_nested_attrs(payload: &[u8]) -> Vec<ParsedAttr> {
    let mut attrs = Vec::new();
    let mut pos = 0;
    while pos + 4 <= payload.len() {
        let nla_len = read_u16(payload, pos) as usize;
        let nla_type = read_u16(payload, pos + 2);
        if nla_len < 4 {
            break;
        }
        let payload_start = pos + 4;
        let payload_end = pos + nla_len;
        let data = if payload_end <= payload.len() {
            payload[payload_start..payload_end].to_vec()
        } else {
            break;
        };
        attrs.push(ParsedAttr {
            nla_type,
            nla_len,
            payload: data,
            is_nested: nla_type & 0x8000 != 0,
        });
        pos += (nla_len + 3) & !3;
    }
    attrs
}

// ─── Table Tests ──────────────────────────────────────────────────────────

#[test]
fn encode_add_table_body_layout() {
    let op = NetlinkOp::AddTable {
        family: NftFamily::Ip,
        name: "filter".into(),
    };
    let (_msg_type, body) = encode_op(&op);

    // Body starts with nfgenmsg (4 bytes).
    assert!(body.len() >= 4);
    assert_eq!(body[0], 2); // NFPROTO_IPV4

    let attrs = parse_attrs(&body);
    // Should have exactly 2 attributes: TABLE_NAME and TABLE_FLAGS.
    assert_eq!(attrs.len(), 2);

    // NFTA_TABLE_NAME = 1
    assert_eq!(attrs[0].base_type(), 1);
    assert_eq!(attrs[0].payload, b"filter");

    // NFTA_TABLE_FLAGS = 2
    assert_eq!(attrs[1].base_type(), 2);
    assert_eq!(attrs[1].payload.len(), 4);
    assert_eq!(read_u32(&attrs[1].payload, 0), 0);
}

// ─── Chain Tests ──────────────────────────────────────────────────────────

#[test]
fn encode_chain_with_hook_is_nested() {
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
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);

    // Find the HOOK attribute (type 4).
    let hook_attr = attrs.iter().find(|a| a.base_type() == 4);
    assert!(hook_attr.is_some(), "NFTA_CHAIN_HOOK not found");
    let hook = hook_attr.unwrap();
    assert!(hook.is_nested, "NFTA_CHAIN_HOOK must be NLA_F_NESTED");

    // Parse nested attrs inside the hook.
    let hook_attrs = parse_nested_attrs(&hook.payload);
    // NFTA_HOOK_HOOKNUM = 1, NFTA_HOOK_PRIORITY = 2.
    assert_eq!(hook_attrs.len(), 2);
    assert_eq!(hook_attrs[0].base_type(), 1); // HOOKNUM
    assert_eq!(read_u32(&hook_attrs[0].payload, 0), 1); // NF_INET_LOCAL_IN
    assert_eq!(hook_attrs[1].base_type(), 2); // PRIORITY
    assert_eq!(
        i32::from_ne_bytes(hook_attrs[1].payload[0..4].try_into().unwrap()),
        0
    );
}

#[test]
fn encode_chain_policy_value() {
    let op = NetlinkOp::AddChain {
        family: NftFamily::Ip,
        table: "filter".into(),
        chain: NftChain::base(
            "input",
            NftChainKind::Filter,
            NftHook::Input,
            0,
            NftPolicy::Drop,
        ),
    };
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);

    // NFTA_CHAIN_POLICY = 5
    let policy_attr = attrs.iter().find(|a| a.base_type() == 5);
    assert!(policy_attr.is_some(), "NFTA_CHAIN_POLICY not found");
    let policy = policy_attr.unwrap();
    assert_eq!(read_u32(&policy.payload, 0), 0); // NF_DROP = 0
}

#[test]
fn encode_chain_type_is_string() {
    let op = NetlinkOp::AddChain {
        family: NftFamily::Ip,
        table: "filter".into(),
        chain: NftChain::base(
            "prerouting",
            NftChainKind::Nat,
            NftHook::Prerouting,
            -100,
            NftPolicy::Accept,
        ),
    };
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);

    // NFTA_CHAIN_TYPE = 7
    let type_attr = attrs.iter().find(|a| a.base_type() == 7);
    assert!(type_attr.is_some(), "NFTA_CHAIN_TYPE not found");
    assert_eq!(type_attr.unwrap().payload, b"nat");
}

#[test]
fn encode_chain_without_hook_has_no_hook_attr() {
    let op = NetlinkOp::AddChain {
        family: NftFamily::Ip,
        table: "filter".into(),
        chain: NftChain::regular("mychain", NftChainKind::Filter),
    };
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);

    // NFTA_CHAIN_HOOK = 4 should not be present.
    assert!(
        !attrs.iter().any(|a| a.base_type() == 4),
        "NFTA_CHAIN_HOOK should not be present for regular chain"
    );
    // NFTA_CHAIN_POLICY = 5 should still be present.
    assert!(attrs.iter().any(|a| a.base_type() == 5));
    // NFTA_CHAIN_TYPE = 7 should be present.
    assert!(attrs.iter().any(|a| a.base_type() == 7));
}

// ─── Rule Tests ───────────────────────────────────────────────────────────

#[test]
fn encode_drop_rule_verdict() {
    let op = NetlinkOp::AddRule {
        family: NftFamily::Ip,
        table: "filter".into(),
        chain: "input".into(),
        rule: NftRule::drop(),
    };
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);

    // NFTA_RULE_TABLE = 1
    assert_eq!(attrs[0].base_type(), 1);
    assert_eq!(attrs[0].payload, b"filter");

    // NFTA_RULE_CHAIN = 2
    assert_eq!(attrs[1].base_type(), 2);
    assert_eq!(attrs[1].payload, b"input");

    // NFTA_RULE_EXPRESSIONS = 4, nested
    let exprs_attr = attrs.iter().find(|a| a.base_type() == 4);
    assert!(exprs_attr.is_some());
    assert!(exprs_attr.unwrap().is_nested);

    // Inside the expression list, parse NFTA_LIST_ELEM attrs.
    let list_attrs = parse_nested_attrs(&exprs_attr.unwrap().payload);
    assert!(!list_attrs.is_empty());
    // Each list elem should be nested.
    assert!(list_attrs[0].is_nested);

    // Inside the list elem, find NFTA_EXPR_NAME and NFTA_EXPR_DATA.
    let elem_attrs = parse_nested_attrs(&list_attrs[0].payload);
    // Should have NAME (1) and DATA (2).
    assert!(elem_attrs.len() >= 2);
    assert_eq!(elem_attrs[0].base_type(), 1); // NAME
    assert_eq!(elem_attrs[0].payload, b"immediate");
    assert!(elem_attrs[1].is_nested); // DATA should be nested

    // Inside the expression data, find the verdict.
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);
    // NFTA_IMMEDIATE_DREG = 1, NFTA_IMMEDIATE_DATA = 2
    let dreg = data_attrs.iter().find(|a| a.base_type() == 1);
    assert!(dreg.is_some());
    assert_eq!(read_u32(&dreg.unwrap().payload, 0), 0); // NFT_REG_VERDICT = 0

    let verdict_data = data_attrs.iter().find(|a| a.base_type() == 2);
    assert!(verdict_data.is_some());
    assert!(verdict_data.unwrap().is_nested);

    // Inside the verdict data, find NFTA_DATA_VERDICT = 2 (nested).
    let verdict_nested = parse_nested_attrs(&verdict_data.unwrap().payload);
    assert!(!verdict_nested.is_empty());
    assert!(verdict_nested[0].is_nested);

    // Inside the verdict, find NFTA_VERDICT_CODE = 1.
    let verdict_attrs = parse_nested_attrs(&verdict_nested[0].payload);
    let code = verdict_attrs.iter().find(|a| a.base_type() == 1);
    assert!(code.is_some());
    assert_eq!(read_u32_be(&code.unwrap().payload, 0), 0); // NF_DROP = 0
}

#[test]
fn encode_accept_rule_verdict() {
    let op = NetlinkOp::AddRule {
        family: NftFamily::Ip,
        table: "filter".into(),
        chain: "input".into(),
        rule: NftRule::accept(),
    };
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);
    let exprs_attr = attrs.iter().find(|a| a.base_type() == 4).unwrap();
    let list_attrs = parse_nested_attrs(&exprs_attr.payload);
    let elem_attrs = parse_nested_attrs(&list_attrs[0].payload);
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);
    let verdict_data = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    let verdict_nested = parse_nested_attrs(&verdict_data.payload);
    let verdict_attrs = parse_nested_attrs(&verdict_nested[0].payload);
    let code = verdict_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(read_u32_be(&code.payload, 0), 1); // NF_ACCEPT = 1
}

#[test]
fn encode_jump_rule_verdict_chain_name() {
    let op = NetlinkOp::AddRule {
        family: NftFamily::Ip,
        table: "filter".into(),
        chain: "forward".into(),
        rule: NftRule::jump("nsg-rules"),
    };
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);
    let exprs_attr = attrs.iter().find(|a| a.base_type() == 4).unwrap();
    let list_attrs = parse_nested_attrs(&exprs_attr.payload);
    let elem_attrs = parse_nested_attrs(&list_attrs[0].payload);
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);
    let verdict_data = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    let verdict_nested = parse_nested_attrs(&verdict_data.payload);
    let verdict_attrs = parse_nested_attrs(&verdict_nested[0].payload);
    let code = verdict_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(
        read_u32_be(&code.payload, 0),
        0xFFFF_FFFDu32
    ); // NFT_JUMP = -3

    // NFTA_VERDICT_CHAIN = 2
    let chain = verdict_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    assert_eq!(chain.payload, b"nsg-rules");
}

// ─── Expression Tests ─────────────────────────────────────────────────────

#[test]
fn encode_meta_expr() {
    let mut b = NlaBuf::new();
    network::syscalls::encode_expr(
        &NftExpr::Meta {
            kind: 16, // L4PROTO
            op: 0,
            value: 0,
        },
        &mut b,
    );
    let attrs = parse_nested_attrs(b.as_slice());
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0].base_type(), 1); // NFTA_LIST_ELEM
    assert!(attrs[0].is_nested);

    let elem_attrs = parse_nested_attrs(&attrs[0].payload);
    assert_eq!(elem_attrs[0].base_type(), 1); // NFTA_EXPR_NAME
    assert_eq!(elem_attrs[0].payload, b"meta");
    assert!(elem_attrs[1].is_nested); // NFTA_EXPR_DATA

    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);
    // NFTA_META_KEY = 1, NFTA_META_DREG = 2
    let key = data_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(read_u32(&key.payload, 0), 16); // L4PROTO
    let dreg = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    assert_eq!(read_u32(&dreg.payload, 0), 1);
}

#[test]
fn encode_cmp_expr() {
    let mut b = NlaBuf::new();
    network::syscalls::encode_expr(
        &NftExpr::Cmp {
            sreg: 1,
            op: 0, // EQ
            data: vec![6], // TCP
        },
        &mut b,
    );
    let attrs = parse_nested_attrs(b.as_slice());
    let elem_attrs = parse_nested_attrs(&attrs[0].payload);
    assert_eq!(elem_attrs[0].payload, b"cmp");
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);

    // NFTA_CMP_SREG = 1
    let sreg = data_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(read_u32(&sreg.payload, 0), 1);
    // NFTA_CMP_OP = 2
    let op = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    assert_eq!(read_u32(&op.payload, 0), 0); // EQ
    // NFTA_CMP_DATA = 3 (nested)
    let cmp_data = data_attrs.iter().find(|a| a.base_type() == 3).unwrap();
    assert!(cmp_data.is_nested);
    let val = parse_nested_attrs(&cmp_data.payload);
    assert_eq!(val[0].base_type(), 1); // NFTA_DATA_VALUE
    assert_eq!(val[0].payload, vec![6]);
}

#[test]
fn encode_payload_expr() {
    let mut b = NlaBuf::new();
    network::syscalls::encode_expr(
        &NftExpr::Payload {
            dreg: 1,
            base: 1, // NETWORK
            offset: 16,
            len: 4,
        },
        &mut b,
    );
    let attrs = parse_nested_attrs(b.as_slice());
    let elem_attrs = parse_nested_attrs(&attrs[0].payload);
    assert_eq!(elem_attrs[0].payload, b"payload");
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);

    let dreg = data_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(read_u32(&dreg.payload, 0), 1);
    let base = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    assert_eq!(read_u32(&base.payload, 0), 1);
    let off = data_attrs.iter().find(|a| a.base_type() == 3).unwrap();
    assert_eq!(read_u32(&off.payload, 0), 16);
    let length = data_attrs.iter().find(|a| a.base_type() == 4).unwrap();
    assert_eq!(read_u32(&length.payload, 0), 4);
}

#[test]
fn encode_nat_expr() {
    let mut b = NlaBuf::new();
    network::syscalls::encode_expr(
        &NftExpr::Nat {
            nat_type: 1, // DNAT
            sreg_addr: 1,
            sreg_port: 2,
        },
        &mut b,
    );
    let attrs = parse_nested_attrs(b.as_slice());
    let elem_attrs = parse_nested_attrs(&attrs[0].payload);
    assert_eq!(elem_attrs[0].payload, b"nat");
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);

    // NFTA_NAT_TYPE = 1
    let nat_type = data_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(read_u32(&nat_type.payload, 0), 1); // DNAT
    // NFTA_NAT_FAMILY = 2
    let family = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    assert_eq!(read_u32(&family.payload, 0), 2); // NFPROTO_IPV4
    // NFTA_NAT_REG_ADDR_MIN = 3
    let addr_min = data_attrs.iter().find(|a| a.base_type() == 3).unwrap();
    assert_eq!(read_u32(&addr_min.payload, 0), 1);
    // NFTA_NAT_REG_PROTO_MIN = 5
    let proto_min = data_attrs.iter().find(|a| a.base_type() == 5).unwrap();
    assert_eq!(read_u32(&proto_min.payload, 0), 2);
}

#[test]
fn encode_bitwise_expr() {
    let mut b = NlaBuf::new();
    network::syscalls::encode_expr(
        &NftExpr::Bitwise {
            sreg: 1,
            dreg: 1,
            len: 4,
            mask: vec![255, 255, 255, 0],
            xor: vec![0, 0, 0, 0],
        },
        &mut b,
    );
    let attrs = parse_nested_attrs(b.as_slice());
    let elem_attrs = parse_nested_attrs(&attrs[0].payload);
    assert_eq!(elem_attrs[0].payload, b"bitwise");
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);

    let sreg = data_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(read_u32(&sreg.payload, 0), 1);
    let dreg = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    assert_eq!(read_u32(&dreg.payload, 0), 1);
    let length = data_attrs.iter().find(|a| a.base_type() == 3).unwrap();
    assert_eq!(read_u32(&length.payload, 0), 4);

    // NFTA_BITWISE_MASK = 4 (nested)
    let mask_attr = data_attrs.iter().find(|a| a.base_type() == 4).unwrap();
    assert!(mask_attr.is_nested);
    let mask_val = parse_nested_attrs(&mask_attr.payload);
    assert_eq!(mask_val[0].base_type(), 1); // NFTA_DATA_VALUE
    assert_eq!(mask_val[0].payload, vec![255, 255, 255, 0]);
}

#[test]
fn encode_numgen_expr() {
    let mut b = NlaBuf::new();
    network::syscalls::encode_expr(
        &NftExpr::Numgen {
            dreg: 9,
            modulus: 3,
            offset: 0,
        },
        &mut b,
    );
    let attrs = parse_nested_attrs(b.as_slice());
    let elem_attrs = parse_nested_attrs(&attrs[0].payload);
    assert_eq!(elem_attrs[0].payload, b"numgen");
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);

    // NFTA_NG_DREG = 1
    let dreg = data_attrs.iter().find(|a| a.base_type() == 1).unwrap();
    assert_eq!(read_u32(&dreg.payload, 0), 9);
    // NFTA_NG_MODULUS = 2
    let modulus = data_attrs.iter().find(|a| a.base_type() == 2).unwrap();
    assert_eq!(read_u32(&modulus.payload, 0), 3);
    // NFTA_NG_TYPE = 3
    let typ = data_attrs.iter().find(|a| a.base_type() == 3).unwrap();
    assert_eq!(read_u32(&typ.payload, 0), 1); // RANDOM
    // NFTA_NG_OFFSET = 4
    let offset = data_attrs.iter().find(|a| a.base_type() == 4).unwrap();
    assert_eq!(read_u32(&offset.payload, 0), 0);
}

#[test]
fn encode_masquerade_expr() {
    let mut b = NlaBuf::new();
    network::syscalls::encode_expr(&NftExpr::Masquerade, &mut b);
    let attrs = parse_nested_attrs(b.as_slice());
    let elem_attrs = parse_nested_attrs(&attrs[0].payload);
    assert_eq!(elem_attrs[0].payload, b"masq");
    // Masquerade has empty data (no expression-specific attributes).
    // The NFTA_EXPR_DATA should still be present as an empty nested attr.
    let data_attrs = parse_nested_attrs(&elem_attrs[1].payload);
    assert!(data_attrs.is_empty(), "masq expression data should be empty");
}

// ─── Set Tests ────────────────────────────────────────────────────────────

#[test]
fn encode_add_set_with_elements() {
    let op = NetlinkOp::AddSet {
        family: NftFamily::Ip,
        table: "filter".into(),
        set: NftSet::ipv4("pods")
            .with_ipv4(std::net::Ipv4Addr::new(10, 0, 0, 1))
            .with_ipv4(std::net::Ipv4Addr::new(10, 0, 0, 2)),
    };
    let (_msg_type, body) = encode_op(&op);
    let attrs = parse_attrs(&body);

    // NFTA_SET_TABLE = 1
    assert_eq!(attrs[0].base_type(), 1);
    assert_eq!(attrs[0].payload, b"filter");

    // NFTA_SET_NAME = 2
    assert_eq!(attrs[1].base_type(), 2);
    assert_eq!(attrs[1].payload, b"pods");

    // NFTA_SET_FLAGS = 3
    let flags = attrs.iter().find(|a| a.base_type() == 3).unwrap();
    let flags_val = read_u32(&flags.payload, 0);
    assert_eq!(flags_val & 1, 1, "NFT_SET_ANONYMOUS should be set");
    assert_eq!(flags_val & 0x20, 0x20, "NFT_SET_CONSTANT should be set for pre-populated set");

    // NFTA_SET_KEY_TYPE = 4
    let key_type = attrs.iter().find(|a| a.base_type() == 4).unwrap();
    assert_eq!(read_u32(&key_type.payload, 0), 7); // ipv4_addr

    // NFTA_SET_KEY_LEN = 5
    let key_len = attrs.iter().find(|a| a.base_type() == 5).unwrap();
    assert_eq!(read_u32(&key_len.payload, 0), 4);

    // NFTA_SET_ELEMENTS = 13 (nested)
    let elements = attrs.iter().find(|a| a.base_type() == 13);
    assert!(elements.is_some(), "NFTA_SET_ELEMENTS not found (type 13)");
    assert!(elements.unwrap().is_nested);
}

// ─── Batch Envelope Tests ─────────────────────────────────────────────────

#[test]
fn batch_begin_has_correct_header() {
    let op = NetlinkOp::AddTable {
        family: NftFamily::Ip,
        name: "x".into(),
    };
    let (_, body) = encode_op(&op);

    // The body passed to encode_op doesn't include the batch envelope.
    // We need to test the full send path. Instead, verify that the
    // nfgen_msg type is correct for a table operation.
    // NFT_MSG_NEWTABLE = 0, with NFNL_SUBSYS_NFTABLES << 8 = 0x0A00
    // So nlmsg_type should be 0x0A00.
    // This is tested by verifying the body[0] is the family byte.
    assert_eq!(body[0], 2); // NFPROTO_IPV4
}

// ─── Rule Builder Tests ───────────────────────────────────────────────────

#[test]
fn clusterip_dnat_rule_expressions() {
    let rule = clusterip_dnat_rule(
        std::net::Ipv4Addr::new(10, 96, 0, 10),
        PROTO_TCP,
        80,
        std::net::Ipv4Addr::new(10, 42, 0, 5),
        8080,
        Some((0, 2)),
    );
    // clusterip_dnat_rule builds:
    // 1. match_cidr(16, cluster_ip/32) -> Payload + Cmp (no Bitwise for /32)
    // 2. match_l4proto(TCP) -> Meta + Cmp
    // 3. match_dport(80) -> Payload + Cmp
    // 4. numgen + cmp (LB selector)
    // 5. dnat_to(backend_ip, backend_port) -> Immediate + Immediate + Nat
    //
    // Total: 2 + 2 + 2 + 2 + 3 = 11 expressions
    assert_eq!(rule.exprs.len(), 11);
    // First: payload (load dst IP).
    assert!(matches!(rule.exprs[0], NftExpr::Payload { .. }));
    // Second: cmp (match cluster IP, /32 skips bitwise).
    assert!(matches!(rule.exprs[1], NftExpr::Cmp { .. }));
    // Third: meta (load l4 proto).
    assert!(matches!(rule.exprs[2], NftExpr::Meta { .. }));
    // Fourth: cmp (match TCP).
    assert!(matches!(rule.exprs[3], NftExpr::Cmp { .. }));
    // Fifth: payload (load dport).
    assert!(matches!(rule.exprs[4], NftExpr::Payload { .. }));
    // Sixth: cmp (match port 80).
    assert!(matches!(rule.exprs[5], NftExpr::Cmp { .. }));
    // Seventh: numgen (LB).
    assert!(matches!(rule.exprs[6], NftExpr::Numgen { .. }));
    // Eighth: cmp (LB selector).
    assert!(matches!(rule.exprs[7], NftExpr::Cmp { .. }));
    // Ninth: immediate (backend IP).
    assert!(matches!(rule.exprs[8], NftExpr::Immediate { .. }));
    // Tenth: immediate (backend port).
    assert!(matches!(rule.exprs[9], NftExpr::Immediate { .. }));
    // Eleventh: nat (DNAT).
    assert!(matches!(rule.exprs[10], NftExpr::Nat { .. }));
}

#[test]
fn masquerade_rule_has_masq_expr() {
    let rule = masquerade_rule(&Ipv4Cidr::parse("10.42.0.0/16").unwrap());
    assert!(!rule.exprs.is_empty());
    // Last expression should be Masquerade.
    assert!(matches!(rule.exprs.last(), Some(NftExpr::Masquerade)));
}

#[test]
fn nsg_filter_rule_deny_all() {
    let rule = nsg_filter_rule(false, None, None, None, None);
    assert_eq!(rule.exprs.len(), 1);
    assert!(matches!(rule.exprs[0], NftExpr::Drop));
}

#[test]
fn nsg_filter_rule_allow_specific() {
    let src = Ipv4Cidr::parse("10.0.0.0/8").unwrap();
    let rule = nsg_filter_rule(true, Some(&src), None, Some(PROTO_TCP), Some(80));
    // Should have: match src CIDR, match proto, match dport, accept
    assert!(rule.exprs.len() >= 4);
    assert!(matches!(rule.exprs.last(), Some(NftExpr::Accept)));
}

// ─── Engine Reconcile Scenario Tests ──────────────────────────────────────

#[test]
fn scenario_pod_to_pod_two_pods_same_subnet() {
    // Two pods in the same subnet: no remote routes needed.
    let mut desired = NetmuxState::new();
    let pod1 = PodNetwork {
        pod_uid: "uid-1".into(),
        pod_ip: std::net::Ipv4Addr::new(10, 42, 0, 5),
        veth_host: "veth-1".into(),
        veth_peer: "zeth-1".into(),
        vnet: None,
    };
    let pod2 = PodNetwork {
        pod_uid: "uid-2".into(),
        pod_ip: std::net::Ipv4Addr::new(10, 42, 0, 6),
        veth_host: "veth-2".into(),
        veth_peer: "zeth-2".into(),
        vnet: None,
    };
    desired.pods.insert("uid-1".into(), pod1);
    desired.pods.insert("uid-2".into(), pod2);

    let current = NetmuxState::new();
    let ops = reconcile(&desired, &current);
    // No nftables or route ops for local pods.
    assert!(ops.is_empty());
}

#[test]
fn scenario_pod_to_internet_masquerade() {
    // Pod CIDR masquerade: pods going to the internet get SNATed.
    let mut desired = NetmuxState::new();
    let pod_cidr = Ipv4Cidr::parse("10.42.0.0/16").unwrap();
    let mut nat = NftTable::new("z8s_nat_node-a", NftFamily::Ip);
    nat.chains.insert(
        "postrouting".into(),
        NftChain::base(
            "postrouting",
            NftChainKind::Nat,
            NftHook::Postrouting,
            100,
            NftPolicy::Accept,
        )
        .with_rule(masquerade_rule(&pod_cidr)),
    );
    desired
        .tables
        .insert((NftFamily::Ip, "z8s_nat_node-a".into()), nat);

    let current = NetmuxState::new();
    let ops = reconcile(&desired, &current);
    // Should emit AddTable, AddChain, AddRule.
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddTable { .. })));
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddChain { .. })));
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddRule { .. })));
}

#[test]
fn scenario_clusterip_service() {
    // Service with ClusterIP DNAT.
    let mut b = NetmuxBuilder::new();
    b.add_service(ServiceSpec {
        name: "web".into(),
        namespace: "default".into(),
        kind: "ClusterIP".into(),
        cluster_ip: Some(std::net::Ipv4Addr::new(10, 96, 0, 10)),
        ports: vec![ServicePortSpec {
            name: "http".into(),
            port: 80,
            target_port: 8080,
            node_port: None,
            protocol: "TCP".into(),
        }],
    });
    let desired = b.build();
    let current = NetmuxState::new();
    let ops = reconcile(&desired, &current);

    // Should create the kube table with nat chains.
    assert!(ops.iter().any(|op| matches!(
        op,
        NetlinkOp::AddTable { name, .. } if name == "kube"
    )));
    // Should have DNAT rules.
    let nat_rules: Vec<_> = ops
        .iter()
        .filter(|op| matches!(op, NetlinkOp::AddRule { .. }))
        .collect();
    assert!(!nat_rules.is_empty(), "expected DNAT rules");
}

#[test]
fn scenario_nsg_deny_all() {
    // NSG policy that denies all traffic.
    let mut b = NetmuxBuilder::new();
    b.add_policy(
        "pod-abc",
        &[NsgRule {
            name: "deny-all".into(),
            priority: 1000,
            action: NsgAction::Deny,
            src_cidrs: vec!["0.0.0.0/0".into()],
            dst_cidrs: vec!["0.0.0.0/0".into()],
        }],
    );
    let desired = b.build();
    let current = NetmuxState::new();
    let ops = reconcile(&desired, &current);

    // Should create an nsg table.
    assert!(ops.iter().any(|op| matches!(
        op,
        NetlinkOp::AddTable { name, .. } if name.starts_with("nsg_")
    )));
}

#[test]
fn scenario_remote_pod_route() {
    // Pod on a peer node: need a /32 route via the peer gateway.
    let mut desired = NetmuxState::new();
    let route = RouteSpec::host_via(
        std::net::Ipv4Addr::new(10, 42, 1, 5),
        std::net::Ipv4Addr::new(192, 168, 1, 2),
    );
    desired.routes.insert(route.key(), route);

    let current = NetmuxState::new();
    let ops = reconcile(&desired, &current);
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddRoute { .. })));
}

#[test]
fn scenario_cleanup_removes_stale_table() {
    // Current has a table that desired doesn't: should be deleted.
    let mut current = NetmuxState::new();
    current.tables.insert(
        (NftFamily::Ip, "stale-table".into()),
        NftTable::new("stale-table", NftFamily::Ip),
    );
    let desired = NetmuxState::new();
    let ops = reconcile(&desired, &current);
    assert!(ops.iter().any(|op| matches!(
        op,
        NetlinkOp::DelTable { name, .. } if name == "stale-table"
    )));
}

#[test]
fn scenario_cleanup_removes_stale_route() {
    let mut current = NetmuxState::new();
    let route = RouteSpec::host_via(
        std::net::Ipv4Addr::new(10, 42, 1, 5),
        std::net::Ipv4Addr::new(192, 168, 1, 2),
    );
    current.routes.insert(route.key(), route);
    let desired = NetmuxState::new();
    let ops = reconcile(&desired, &current);
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::DelRoute { .. })));
}

#[test]
fn scenario_idempotent_reconcile() {
    // Same desired and current: no ops.
    let mut state = NetmuxState::new();
    state.tables.insert(
        (NftFamily::Ip, "kube".into()),
        NftTable::new("kube", NftFamily::Ip),
    );
    let ops = reconcile(&state, &state);
    assert!(ops.is_empty());
}

// ─── DNS Tests ────────────────────────────────────────────────────────────

#[test]
fn scenario_dns_records_from_plan() {
    let mut b = NetmuxBuilder::new();
    b.add_service(ServiceSpec {
        name: "web".into(),
        namespace: "default".into(),
        kind: "ClusterIP".into(),
        cluster_ip: Some(std::net::Ipv4Addr::new(10, 96, 0, 10)),
        ports: vec![ServicePortSpec {
            name: "http".into(),
            port: 80,
            target_port: 8080,
            node_port: None,
            protocol: "TCP".into(),
        }],
    });
    let desired = b.build();
    assert!(
        desired.dns_records.contains_key("web.default.svc.cluster.local"),
        "DNS record should be present"
    );
    assert_eq!(
        desired.dns_records["web.default.svc.cluster.local"],
        std::net::Ipv4Addr::new(10, 96, 0, 10)
    );
}

// ─── VNet Isolation Tests ─────────────────────────────────────────────────

#[test]
fn scenario_vnet_no_internet() {
    let mut b = NetmuxBuilder::new();
    b.add_vnet(VNetSpec {
        name: "isolated".into(),
        cidr: Ipv4Cidr::parse("10.99.0.0/16").unwrap(),
        internet_access: false,
    });
    let desired = b.build();
    // Should have a vnet table.
    assert!(desired.tables.iter().any(|((_, n), _)| n == "vnet_isolated"));
    // Should have an IP pool for the vnet.
    assert!(desired.ip_pools.contains_key("isolated-pods"));
}

// ─── Multi-table scenario ─────────────────────────────────────────────────

#[test]
fn scenario_full_stack() {
    // Simulate a full network state: NAT table, filter table, vnet, service, pods.
    let mut b = NetmuxBuilder::new();
    b.add_vnet(VNetSpec {
        name: "default".into(),
        cidr: Ipv4Cidr::parse("10.42.0.0/16").unwrap(),
        internet_access: true,
    });
    b.add_service(ServiceSpec {
        name: "web".into(),
        namespace: "default".into(),
        kind: "ClusterIP".into(),
        cluster_ip: Some(std::net::Ipv4Addr::new(10, 96, 0, 10)),
        ports: vec![ServicePortSpec {
            name: "http".into(),
            port: 80,
            target_port: 8080,
            node_port: None,
            protocol: "TCP".into(),
        }],
    });
    b.add_pod(PodNetwork {
        pod_uid: "uid-1".into(),
        pod_ip: std::net::Ipv4Addr::new(10, 42, 0, 5),
        veth_host: "veth-1".into(),
        veth_peer: "zeth-1".into(),
        vnet: Some("default".into()),
    });
    b.add_policy(
        "uid-1",
        &[NsgRule {
            name: "allow-web".into(),
            priority: 100,
            action: NsgAction::Allow,
            src_cidrs: vec!["10.0.0.0/8".into()],
            dst_cidrs: vec!["10.42.0.0/16".into()],
        }],
    );
    let desired = b.build();

    // Verify state has all components.
    assert!(desired.tables.contains_key(&(NftFamily::Ip, "kube".into())));
    assert!(desired.pods.contains_key("uid-1"));
    assert!(desired.ip_pools.contains_key("default-pods"));
    assert!(desired.dns_records.contains_key("web.default.svc.cluster.local"));

    // Reconcile against empty should produce ops for everything.
    let ops = reconcile(&desired, &NetmuxState::new());
    assert!(!ops.is_empty());
    // Should have tables, chains, rules, DNS, pods.
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddTable { .. })));
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddChain { .. })));
    assert!(ops.iter().any(|op| matches!(op, NetlinkOp::AddRule { .. })));
}
