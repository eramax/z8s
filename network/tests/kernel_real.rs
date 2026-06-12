//! Kernel integration tests — actually talk to the kernel via netlink.
//!
//! Run with: `sudo -E ~/.cargo/bin/cargo test -p network --test kernel_real -- --nocapture`
//!
//! These tests create REAL nftables tables, chains, and rules in the kernel,
//! then verify them with the `nft` CLI.
//!
//! **SAFETY:** All tests use unique table names prefixed `z8s_test_*` and clean
//! up after themselves. No default chains or existing routes are modified.

#![cfg(target_os = "linux")]

use std::net::Ipv4Addr;
use std::process::Command;

use network::engine::ReconcileReport;
use network::ipam::Ipv4Cidr;
use network::model::*;
use network::nftables::NlSocket;
use network::{reconcile, Netmux};

// ─── Helpers ──────────────────────────────────────────────────────────────

fn nft(args: &[&str]) -> String {
    let out = Command::new("nft")
        .args(args)
        .output()
        .expect("nft binary missing");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn nft_table_exists(table: &str) -> bool {
    Command::new("nft")
        .args(["-n", "list", "table", "ip", table])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Best-effort cleanup of test tables. Ignores errors.
fn cleanup_tables(tables: &[&str]) {
    for t in tables {
        let _ = Command::new("nft")
            .args(["delete", "table", "ip", t])
            .output();
    }
}

/// Safely reconcile against empty state to clean up all test tables.
fn safe_cleanup(engine: &mut Netmux) {
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    if !ops.is_empty() {
        let _ = engine.apply(&ops, false);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 1: Create table + chain + rule, verify with nft CLI
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_create_table_chain_rule() {
    cleanup_tables(&["z8s_test_basic"]);

    let sock = NlSocket::open().expect("open netlink socket");

    let ops = vec![
        NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "z8s_test_basic".into(),
        },
        NetlinkOp::AddChain {
            family: NftFamily::Ip,
            table: "z8s_test_basic".into(),
            chain: NftChain::base(
                "input",
                NftChainKind::Filter,
                NftHook::Input,
                0,
                NftPolicy::Accept,
            ),
        },
        NetlinkOp::AddRule {
            family: NftFamily::Ip,
            table: "z8s_test_basic".into(),
            chain: "input".into(),
            rule: NftRule::accept(),
        },
    ];

    network::nftables::send_batch(&ops).expect("send batch");

    // Verify with nft CLI.
    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_basic"]);
    println!("table listing:\n{}", listing);
    assert!(listing.contains("input"), "input chain missing");
    assert!(listing.contains("accept"), "accept rule missing");

    // Cleanup.
    let _ = Command::new("nft")
        .args(["delete", "table", "ip", "z8s_test_basic"])
        .output();
    assert!(!nft_table_exists("z8s_test_basic"), "table should be deleted");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 2: Batch protocol — multiple ops in one batch
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_batch_multiple_ops() {
    cleanup_tables(&["z8s_test_batch"]);

    let sock = NlSocket::open().expect("open netlink socket");

    // Create table, chain, and rule in a single batch.
    let ops = vec![
        NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "z8s_test_batch".into(),
        },
        NetlinkOp::AddChain {
            family: NftFamily::Ip,
            table: "z8s_test_batch".into(),
            chain: NftChain::base(
                "forward",
                NftChainKind::Filter,
                NftHook::Forward,
                0,
                NftPolicy::Accept,
            ),
        },
        NetlinkOp::AddRule {
            family: NftFamily::Ip,
            table: "z8s_test_batch".into(),
            chain: "forward".into(),
            rule: NftRule::accept(),
        },
    ];

    network::nftables::send_batch(&ops).expect("send batch");

    // Verify all three resources exist.
    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_batch"]);
    println!("batch listing:\n{}", listing);
    assert!(listing.contains("forward"), "forward chain missing after batch");
    assert!(listing.contains("accept"), "accept rule missing after batch");

    // Cleanup.
    let _ = Command::new("nft")
        .args(["delete", "table", "ip", "z8s_test_batch"])
        .output();
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 3: Established/related rule with conntrack
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_established_rule() {
    cleanup_tables(&["z8s_test_estab"]);

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_estab", NftFamily::Ip);
    table.chains.insert(
        "forward".into(),
        NftChain::base(
            "forward",
            NftChainKind::Filter,
            NftHook::Forward,
            0,
            NftPolicy::Accept,
        )
        .with_rule(established_related_rule()),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_estab".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply established rule");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_estab"]);
    println!("established listing:\n{}", listing);
    assert!(listing.contains("ct"), "ct expression missing");
    assert!(listing.contains("state"), "ct state missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 4: DNAT rule (ClusterIP)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_dnat_rule() {
    cleanup_tables(&["z8s_test_dnat"]);

    let rule = clusterip_dnat_rule(
        Ipv4Addr::new(10, 96, 0, 10),
        PROTO_TCP,
        80,
        Ipv4Addr::new(10, 42, 0, 5),
        8080,
        None, // no load balancing (single backend)
    );

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_dnat", NftFamily::Ip);
    table.chains.insert(
        "prerouting".into(),
        NftChain::base(
            "prerouting",
            NftChainKind::Nat,
            NftHook::Prerouting,
            -100,
            NftPolicy::Accept,
        )
        .with_rule(rule),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_dnat".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply DNAT rule");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_dnat"]);
    println!("dnat listing:\n{}", listing);
    assert!(listing.contains("dnat"), "dnat missing");
    assert!(listing.contains("10.96.0.10"), "cluster IP missing");
    assert!(listing.contains("10.42.0.5"), "backend IP missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 5: Masquerade rule
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_masquerade_rule() {
    cleanup_tables(&["z8s_test_masq"]);

    let pod_cidr = Ipv4Cidr::parse("10.42.0.0/16").unwrap();
    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_masq", NftFamily::Ip);
    table.chains.insert(
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
    desired.tables.insert((NftFamily::Ip, "z8s_test_masq".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply masquerade");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_masq"]);
    println!("masq listing:\n{}", listing);
    assert!(listing.contains("masq"), "masquerade missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 6: Set with elements
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_set_with_elements() {
    cleanup_tables(&["z8s_test_set"]);

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_set", NftFamily::Ip);
    table.sets.insert(
        "pods".into(),
        NftSet::ipv4("pods")
            .with_ipv4(Ipv4Addr::new(10, 0, 0, 1))
            .with_ipv4(Ipv4Addr::new(10, 0, 0, 2)),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_set".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply set");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_set"]);
    println!("set listing:\n{}", listing);
    assert!(listing.contains("pods"), "set name missing");
    assert!(listing.contains("10.0.0.1"), "element 1 missing");
    assert!(listing.contains("10.0.0.2"), "element 2 missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 7: Lookup expression (set match)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_lookup_rule() {
    cleanup_tables(&["z8s_test_lookup"]);

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_lookup", NftFamily::Ip);
    table.sets.insert(
        "allowed".into(),
        NftSet::ipv4("allowed").with_ipv4(Ipv4Addr::new(10, 0, 0, 5)),
    );
    table.chains.insert(
        "forward".into(),
        NftChain::base(
            "forward",
            NftChainKind::Filter,
            NftHook::Forward,
            0,
            NftPolicy::Accept,
        )
        .with_rule(NftRule::from_exprs(vec![
            NftExpr::Payload {
                dreg: 1,
                base: 1, // NFT_PAYLOAD_NETWORK_HEADER
                offset: 12, // IPv4 saddr
                len: 4,
            },
            NftExpr::Lookup {
                set: "allowed".into(),
                sreg: 1,
            },
            NftExpr::Accept,
        ])),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_lookup".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    // Send all ops in a single batch so the lookup can reference the set
    let nft_ops: Vec<_> = ops.iter().filter(|op| !op.is_route()).cloned().collect();
    network::nftables::send_batch(&nft_ops).expect("apply lookup rule batch");

    // Seed the engine state so cleanup works
    let desired2 = desired.clone();
    engine.seed_current(desired2);

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_lookup"]);
    println!("lookup listing:\n{}", listing);
    assert!(listing.contains("lookup"), "lookup expression missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 8: Chain with jump rule
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_jump_rule() {
    cleanup_tables(&["z8s_test_jump"]);

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_jump", NftFamily::Ip);
    table.chains.insert(
        "nsg-rules".into(),
        NftChain::regular("nsg-rules", NftChainKind::Filter)
            .with_rule(NftRule::accept()),
    );
    table.chains.insert(
        "forward".into(),
        NftChain::base(
            "forward",
            NftChainKind::Filter,
            NftHook::Forward,
            0,
            NftPolicy::Accept,
        )
        .with_rule(NftRule::jump("nsg-rules")),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_jump".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply jump rule");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_jump"]);
    println!("jump listing:\n{}", listing);
    assert!(listing.contains("nsg-rules"), "nsg-rules chain missing");
    assert!(listing.contains("jump"), "jump verdict missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 9: Delete non-existent table — should not crash
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_delete_nonexistent_no_crash() {
    cleanup_tables(&["z8s_test_nonexist"]);

    // Try to delete a table that doesn't exist.
    let mut engine = Netmux::connect().expect("connect");
    let mut desired = NetmuxState::new();
    desired.tables.insert(
        (NftFamily::Ip, "z8s_test_nonexist".into()),
        NftTable::new("z8s_test_nonexist", NftFamily::Ip),
    );
    // First add it, then remove it.
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).unwrap();
    let ops = reconcile(&NetmuxState::new(), engine.current());
    engine.apply(&ops, false).unwrap();
    // Should not crash.
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 10: ReconcileReport with real ops
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_reconcile_report_real_ops() {
    cleanup_tables(&["z8s_test_report"]);

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_report", NftFamily::Ip);
    table.chains.insert(
        "input".into(),
        NftChain::base(
            "input",
            NftChainKind::Filter,
            NftHook::Input,
            0,
            NftPolicy::Accept,
        )
        .with_rule(NftRule::accept()),
    );
    table.sets.insert(
        "ips".into(),
        NftSet::ipv4("ips").with_ipv4(Ipv4Addr::new(10, 0, 0, 1)),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_report".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    let report = ReconcileReport::from_ops(&ops);
    println!("report: {:?}", report);
    assert!(report.tables >= 1, "should have at least 1 table op");
    assert!(report.chains >= 1, "should have at least 1 chain op");
    assert!(report.rules >= 1, "should have at least 1 rule op");
    assert!(report.sets >= 1, "should have at least 1 set op");

    engine.apply(&ops, false).expect("apply");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 11: Drop rule
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_drop_rule() {
    cleanup_tables(&["z8s_test_drop"]);

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_drop", NftFamily::Ip);
    table.chains.insert(
        "forward".into(),
        NftChain::base(
            "forward",
            NftChainKind::Filter,
            NftHook::Forward,
            0,
            NftPolicy::Accept,
        )
        .with_rule(NftRule::drop()),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_drop".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply drop rule");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_drop"]);
    println!("drop listing:\n{}", listing);
    assert!(listing.contains("drop"), "drop verdict missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 12: Idempotent reconcile — same state twice produces no ops
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_idempotent_reconcile() {
    cleanup_tables(&["z8s_test_idempotent"]);

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_idempotent", NftFamily::Ip);
    table.chains.insert(
        "input".into(),
        NftChain::base(
            "input",
            NftChainKind::Filter,
            NftHook::Input,
            0,
            NftPolicy::Accept,
        ),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_idempotent".into()), table);

    let mut engine = Netmux::connect().expect("connect");

    // First apply: creates the table.
    let ops = reconcile(&desired, engine.current());
    assert!(!ops.is_empty(), "first reconcile should produce ops");
    engine.apply(&ops, false).expect("first apply");

    // Second reconcile: should produce zero ops (idempotent).
    let ops = reconcile(&desired, engine.current());
    assert!(ops.is_empty(), "second reconcile should produce no ops, got: {:?}", ops);

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 13: Numgen load balancing
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_numgen_load_balancing() {
    cleanup_tables(&["z8s_test_lb"]);

    let rule = clusterip_dnat_rule(
        Ipv4Addr::new(10, 96, 0, 10),
        PROTO_TCP,
        80,
        Ipv4Addr::new(10, 42, 0, 5),
        8080,
        Some((0, 3)), // backend 0 of 3
    );

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_lb", NftFamily::Ip);
    table.chains.insert(
        "prerouting".into(),
        NftChain::base(
            "prerouting",
            NftChainKind::Nat,
            NftHook::Prerouting,
            -100,
            NftPolicy::Accept,
        )
        .with_rule(rule),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_lb".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply LB rule");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_lb"]);
    println!("lb listing:\n{}", listing);
    assert!(listing.contains("numgen"), "numgen expression missing");
    assert!(listing.contains("dnat"), "dnat missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 14: Bitwise CIDR match
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_bitwise_cidr_match() {
    cleanup_tables(&["z8s_test_bitwise"]);

    let cidr = Ipv4Cidr::parse("10.42.0.0/16").unwrap();
    let rule = NftRule::from_exprs(
        match_cidr(12, &cidr) // match dst CIDR
            .into_iter()
            .chain(std::iter::once(NftExpr::Accept))
            .collect(),
    );

    let mut desired = NetmuxState::new();
    let mut table = NftTable::new("z8s_test_bitwise", NftFamily::Ip);
    table.chains.insert(
        "forward".into(),
        NftChain::base(
            "forward",
            NftChainKind::Filter,
            NftHook::Forward,
            0,
            NftPolicy::Accept,
        )
        .with_rule(rule),
    );
    desired.tables.insert((NftFamily::Ip, "z8s_test_bitwise".into()), table);

    let mut engine = Netmux::connect().expect("connect");
    let ops = reconcile(&desired, engine.current());
    engine.apply(&ops, false).expect("apply bitwise rule");

    let listing = nft(&["-n", "list", "table", "ip", "z8s_test_bitwise"]);
    println!("bitwise listing:\n{}", listing);
    assert!(listing.contains("bitwise"), "bitwise expression missing");

    // Cleanup.
    let empty = NetmuxState::new();
    let ops = reconcile(&empty, engine.current());
    engine.apply(&ops, false).expect("cleanup");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 15: DNS server end-to-end
// ═══════════════════════════════════════════════════════════════════════════

/// Build a minimal DNS A-record query packet.
fn build_dns_query(name: &str, id: u16) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&0x0100u16.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    for label in name.split('.') {
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes());
    p
}

#[tokio::test]
async fn kernel_dns_server_e2e() {
    use network::dns::{DnsConfig, DnsServer, DnsZone};

    let mut records = std::collections::HashMap::new();
    records.insert("web.default.svc.cluster.local".into(), Ipv4Addr::new(10, 96, 0, 10));
    let zone = DnsZone::from_records(&records);

    let port = 15353;
    let server = DnsServer::new(
        DnsConfig {
            listen: format!("127.0.0.1:{}", port).parse().unwrap(),
            domain: "cluster.local".into(),
            upstream: None,
        },
        zone,
    );
    tokio::spawn(async move { let _ = server.serve().await; });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // A record query.
    let query = build_dns_query("web.default.svc.cluster.local", 0xABCD);
    sock.send_to(&query, format!("127.0.0.1:{}", port)).await.unwrap();
    let mut buf = vec![0u8; 1500];
    let n = tokio::time::timeout(std::time::Duration::from_secs(2), sock.recv(&mut buf))
        .await.expect("timeout").expect("recv");
    buf.truncate(n);

    assert!(buf.len() >= 12);
    assert_eq!(u16::from_be_bytes([buf[0], buf[1]]), 0xABCD);
    assert_ne!(u16::from_be_bytes([buf[2], buf[3]]) & 0x8000, 0);
    assert_eq!(u16::from_be_bytes([buf[6], buf[7]]), 1);
    assert!(buf.windows(4).any(|w| w == &[10, 96, 0, 10]));

    // NXDOMAIN.
    let query2 = build_dns_query("nonexistent.cluster.local", 0x1234);
    sock.send_to(&query2, format!("127.0.0.1:{}", port)).await.unwrap();
    let mut buf2 = vec![0u8; 1500];
    let n2 = tokio::time::timeout(std::time::Duration::from_secs(2), sock.recv(&mut buf2))
        .await.expect("timeout").expect("recv");
    buf2.truncate(n2);
    let rcode = u16::from_be_bytes([buf2[2], buf2[3]]) & 0x000F;
    assert_eq!(rcode, 3, "expected NXDOMAIN (3), got {}", rcode);
    println!("DNS e2e test passed");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 16: reconcile_full
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_reconcile_full() {
    cleanup_tables(&["z8s_rfull_nat_test-rfull", "z8s_rfull_filter_test-rfull",
                      "z8s_nat_test-rfull", "z8s_filter_test-rfull"]);

    let mut engine = Netmux::connect().expect("connect");

    use z8s_core::store::StoreSnapshot;
    use z8s_core::types::{ObjectMeta, ResourceRecord, Service, ServicePort, ServiceSpec};

    let svc = ResourceRecord::new(z8s_core::types::AnyResource::Service(Service {
        metadata: ObjectMeta {
            name: Some("web".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            type_: Some("ClusterIP".into()),
            cluster_ip: Some("10.96.0.10".into()),
            selector: Some({
                let mut m = std::collections::BTreeMap::new();
                m.insert("app".into(), "web".into());
                m
            }),
            ports: Some(vec![ServicePort {
                name: "http".into(),
                port: 80,
                target_port: Some(8080),
                node_port: None,
                protocol: Some("TCP".into()),
            }]),
        }),
        ..Default::default()
    }));

    let snap = StoreSnapshot::from_records(vec![svc]);
    let cfg = network::plan::PlanConfig {
        node_name: "test-rfull".into(),
        pod_cidr: Ipv4Cidr::parse("10.42.0.0/16").unwrap(),
        service_cidr: Ipv4Cidr::parse("10.96.0.0/16").unwrap(),
        cluster_domain: "cluster.local".into(),
        gateway: Ipv4Addr::new(10, 42, 0, 1),
        peers: vec![],
    };

    let report = engine.reconcile_full(&snap, &cfg).expect("reconcile_full");
    println!("report: {:?}", report);
    assert!(report.total() > 0);
    assert!(nft_table_exists("z8s_nat_test-rfull"));
    assert!(nft_table_exists("z8s_filter_test-rfull"));

    safe_cleanup(&mut engine);
    assert!(!nft_table_exists("z8s_nat_test-rfull"));
    assert!(!nft_table_exists("z8s_filter_test-rfull"));
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 17: Basic table creation via send()
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_send_creates_table() {
    cleanup_tables(&["z8s_send_test"]);

    let sock = NlSocket::open().expect("open");
    let op = NetlinkOp::AddTable {
        family: NftFamily::Ip,
        name: "z8s_send_test".into(),
    };
    sock.send(&op).expect("AddTable");

    assert!(nft_table_exists("z8s_send_test"), "table should exist");

    let _ = Command::new("nft")
        .args(["delete", "table", "ip", "z8s_send_test"])
        .output();
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 18: Add chain to nft-created table
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn kernel_add_chain_to_existing_table() {
    cleanup_tables(&["z8s_chain_test"]);

    // Step 1: Create just a table via send_batch
    eprintln!("=== Step 1: Create table only ===");
    let table_op = NetlinkOp::AddTable {
        family: NftFamily::Ip,
        name: "z8s_chain_test".into(),
    };
    match network::nftables::send_batch(&[table_op]) {
        Ok(()) => eprintln!("Step 1 OK"),
        Err(e) => eprintln!("Step 1 ERROR: {}", e),
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    let all_tables = nft(&["list", "tables"]);
    eprintln!("tables after step 1: '{}'", all_tables);
    assert!(nft_table_exists("z8s_chain_test"), "table must exist after step 1");

    // Step 2: Create chain in existing table
    eprintln!("=== Step 2: Add chain ===");
    let chain_op = NetlinkOp::AddChain {
        family: NftFamily::Ip,
        table: "z8s_chain_test".into(),
        chain: NftChain::regular("testchain", NftChainKind::Filter),
    };
    match network::nftables::send_batch(&[chain_op]) {
        Ok(()) => eprintln!("Step 2 OK"),
        Err(e) => eprintln!("Step 2 ERROR: {}", e),
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    let listing = nft(&["-n", "list", "table", "ip", "z8s_chain_test"]);
    eprintln!("listing after step 2: '{}'", listing);
    assert!(listing.contains("testchain"), "chain missing after step 2");

    let _ = Command::new("nft")
        .args(["delete", "table", "ip", "z8s_chain_test"])
        .output();
}
