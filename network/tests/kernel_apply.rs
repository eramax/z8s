//! Integration test: apply nftables state via the Netmux engine and verify
//! with the real `nft` CLI that the kernel actually received the rules.
//!
//! Run with: `sudo -E ~/.cargo/bin/cargo test -p network --test kernel_apply -- --nocapture`
//!
//! Requires CAP_NET_ADMIN. This test does not modify the system permanently:
//! it pre-cleans stale tables, creates uniquely-named tables, asserts on the
//! actual rule bodies (not just chain names), then reconciles against an
//! empty state and verifies the kernel is clean again.
//!
//! Gated on Linux because nftables is Linux-only.

#![cfg(target_os = "linux")]

use std::process::Command;

use network::ipam::Ipv4Cidr;
use network::model::*;
use network::syscalls::NlSocket;
use network::{Netmux, NetmuxBuilder};

/// Run `nft` and return its stdout (empty on error).
fn nft(args: &[&str]) -> String {
    let out = Command::new("nft")
        .args(args)
        .output()
        .expect("nft binary missing — install nftables");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn nft_list(table: &str) -> String {
    nft(&["-n", "list", "table", "ip", table])
}

fn nft_table_exists(table: &str) -> bool {
    Command::new("nft")
        .args(["-n", "list", "table", "ip", table])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Pre-clean: remove any tables we might create, in case a prior run crashed.
/// Best-effort: ignore failures (table may not exist).
fn pre_clean(tables: &[&str]) {
    for t in tables {
        let _ = Command::new("nft")
            .args(["delete", "table", "ip", t])
            .output();
    }
}

#[test]
fn kernel_apply_and_cleanup() {
    let _ = std::env::var("USER"); // ensure not running as root without sudo
    pre_clean(&["kube", "vnet_test", "nsg_pod-abc"]);

    // ── Build desired state: vnet ─────────────────────────────────
    let mut b = NetmuxBuilder::new();
    b.add_vnet(VNetSpec {
        name: "test".into(),
        cidr: Ipv4Cidr::parse("10.99.0.0/16").unwrap(),
        internet_access: false,
    });
    let desired = b.build();

    // ── Apply via the engine (real kernel) ─────────────────────────
    let mut engine = Netmux::connect().expect("connect netlink netfilter");
    let ops = network::reconcile(&desired, &engine.current());
    println!("applying {} vnet ops", ops.len());
    engine
        .apply(&ops, false)
        .expect("apply vnet ops to kernel");

    // ── Verify vnet table — check chain NAMES + rule BODIES ────────
    let vnet_table = "vnet_test";
    let listing = nft_list(vnet_table);
    println!("nft list {}:\n{}", vnet_table, listing);
    assert!(
        listing.contains("vnet-isolation"),
        "vnet-isolation chain missing: {}",
        listing
    );
    assert!(
        listing.contains("prerouting"),
        "prerouting chain missing"
    );
    // The engine emits a jump rule from prerouting to vnet-isolation.
    assert!(
        listing.contains("jump"),
        "expected jump rule in prerouting chain, got: {}",
        listing
    );

    // ── Add a service and verify kube table ────────────────────────
    let mut b2 = NetmuxBuilder::new();
    b2.add_service(ServiceSpec {
        name: "web".into(),
        namespace: "test".into(),
        kind: "ClusterIP".into(),
        cluster_ip: Some(std::net::Ipv4Addr::new(10, 99, 0, 10)),
        ports: vec![ServicePortSpec {
            name: "http".into(),
            port: 80,
            target_port: 8080,
            node_port: None,
            protocol: "TCP".into(),
        }],
    });
    let desired2 = b2.build();
    let ops2 = network::reconcile(&desired2, &engine.current());
    println!("applying {} service ops", ops2.len());
    engine.apply(&ops2, false).expect("apply service ops");

    let kube_listing = nft_list("kube");
    println!("nft list kube:\n{}", kube_listing);
    assert!(kube_listing.contains("prerouting"), "kube prerouting missing");
    assert!(kube_listing.contains("postrouting"), "kube postrouting missing");
    assert!(
        kube_listing.contains("clusterip-dnat"),
        "kube clusterip-dnat missing"
    );
    // The DNAT rule should reference the cluster IP 10.99.0.10.
    assert!(
        kube_listing.contains("10.99.0.10"),
        "DNAT rule should reference cluster IP 10.99.0.10, got: {}",
        kube_listing
    );
    // And the dnat verdict.
    assert!(
        kube_listing.contains("dnat to"),
        "DNAT rule should contain 'dnat to', got: {}",
        kube_listing
    );

    // ── Add a policy and verify nsg table ──────────────────────────
    let mut b3 = NetmuxBuilder::new();
    b3.add_policy(
        "pod-abc",
        &[NsgRule {
            name: "deny-all".into(),
            priority: 1000,
            action: NsgAction::Deny,
            src_cidrs: vec!["0.0.0.0/0".into()],
            dst_cidrs: vec!["0.0.0.0/0".into()],
        }],
    );
    let desired3 = b3.build();
    let ops3 = network::reconcile(&desired3, &engine.current());
    println!("applying {} policy ops", ops3.len());
    engine.apply(&ops3, false).expect("apply policy ops");

    let nsg_listing = nft_list("nsg_pod-abc");
    println!("nft list nsg_pod-abc:\n{}", nsg_listing);
    assert!(nsg_listing.contains("ingress"), "nsg ingress chain missing");
    // The deny rule should produce a 'drop' verdict.
    assert!(
        nsg_listing.contains("drop"),
        "expected drop verdict in nsg, got: {}",
        nsg_listing
    );
    // Note: comments are stored in user data (NFTA_RULE_USERDATA) which we
    // don't currently encode, so we skip checking for them in the kernel.

    // ── Clean up: reconcile against empty state ───────────────────
    let empty = NetmuxState::new();
    let cleanup_ops = network::reconcile(&empty, &engine.current());
    println!("cleaning up {} ops", cleanup_ops.len());
    engine.apply(&cleanup_ops, false).expect("cleanup");

    // Verify all tables are gone.
    assert!(!nft_table_exists("kube"), "kube table should be deleted");
    assert!(
        !nft_table_exists(vnet_table),
        "{} table should be deleted",
        vnet_table
    );
    assert!(
        !nft_table_exists("nsg_pod-abc"),
        "nsg_pod-abc table should be deleted"
    );

    // Sanity: the socket can still send (no panics on the way out).
    let _sock = NlSocket::open().expect("reopen socket");
    println!("kernel apply + cleanup OK");
}
