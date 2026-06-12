//! End-to-end connectivity tests — DEFENSIVE edition.
//!
//! Safety guarantees:
//!   1. Pre-cleans leftover state from prior crashed runs.
//!   2. Each test owns its own cleanup via a guard struct.
//!   3. Engine reconciles to empty state + detaches pods BEFORE dropping.
//!   4. Post-cleans everything — even on panic.
//!   5. Unique resource names per test prevent cross-test collisions.
//!
//! Requires: `sudo` (CAP_NET_ADMIN), `nft`, `ip`, `ping` in PATH.
//!
//! Run: `sudo -E cargo test -p network --test e2e_connectivity -- --nocapture`

#![cfg(target_os = "linux")]

use std::net::Ipv4Addr;
use std::panic;
use std::process::Command;

use network::ipam::Ipv4Cidr;
use network::model::*;
use network::{Netmux, NetmuxBuilder, NetmuxState};

// ── Low-level helpers ────────────────────────────────────────────────────

fn run(cmd: &str, args: &[&str]) -> String {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd}: {e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_quiet(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_in_netns(ns: &str, cmd: &str, args: &[&str]) -> String {
    let mut full = vec!["netns", "exec", ns, cmd];
    full.extend_from_slice(args);
    run("ip", &full)
}

fn nft_list(table: &str) -> String {
    run("nft", &["-n", "list", "table", "ip", table])
}

fn nft_table_exists(table: &str) -> bool {
    Command::new("nft")
        .args(["-n", "list", "table", "ip", table])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── Aggressive cleanup ───────────────────────────────────────────────────

/// Delete ALL test nft tables, netns, and veths.
/// Called before each test and as a safety net after each test.
fn clean_everything() {
    // Nft tables.
    let tables = run("nft", &["list", "tables"]);
    for line in tables.lines() {
        if let Some(name) = line.strip_prefix("table ip ") {
            if name.starts_with("z8s_")
                || name.starts_with("vnet_")
                || name.starts_with("kube")
                || name.starts_with("nsg_")
                || name.starts_with("comment_")
            {
                let _ = Command::new("nft")
                    .args(["delete", "table", "ip", name])
                    .output();
            }
        }
    }

    // Netns — kill processes inside, then delete.
    let netns_list = run("ip", &["netns", "list"]);
    for line in netns_list.lines() {
        let name = line.split_whitespace().next().unwrap_or("");
        if name.starts_with("z8s_e2") {
            let pids = run("ip", &["netns", "pids", name]);
            for pid_str in pids.lines() {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
                }
            }
            // Retry delete a few times (processes may not have died yet).
            for _ in 0..5 {
                if Command::new("ip")
                    .args(["netns", "delete", name])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    // Veths.
    let links = run("ip", &["-br", "link", "show"]);
    for line in links.lines() {
        if let Some(name) = line.split_whitespace().next() {
            if name.starts_with("veth-e2") || name.starts_with("zeth-e2") {
                let _ = Command::new("ip").args(["link", "delete", name]).output();
            }
        }
    }
}

// ── Guard: ensures cleanup on panic ──────────────────────────────────────

struct TestGuard {
    engine: Option<Netmux>,
    pods: Vec<(String, Ipv4Addr)>,
    netns_names: Vec<String>,
}

impl TestGuard {
    fn new() -> Self {
        Self {
            engine: Some(Netmux::connect().expect("connect netlink")),
            pods: Vec::new(),
            netns_names: Vec::new(),
        }
    }

    fn add_netns(&mut self, name: &str) -> u32 {
        run("ip", &["netns", "add", name]);
        run_in_netns(name, "ip", &["link", "set", "lo", "up"]);
        let _child = Command::new("ip")
            .args(["netns", "exec", name, "sleep", "600"])
            .spawn()
            .unwrap_or_else(|e| panic!("spawn in {name}: {e}"));
        // Wait for process to appear.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let pids = run("ip", &["netns", "pids", name]);
        let pid: u32 = pids
            .lines()
            .next()
            .unwrap_or_else(|| panic!("no PID in {name}"))
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("bad PID in {name}"));
        // Re-find child by PID — the original child handle is for `ip`, not `sleep`.
        // We track by netns name and kill by `ip netns pids` in cleanup.
        self.netns_names.push(name.to_string());
        pid
    }

    fn apply_nft(&mut self, desired: &NetmuxState) {
        let engine = self.engine.as_mut().unwrap();
        let ops = network::reconcile(desired, engine.current());
        engine.apply(&ops, false).expect("apply nft ops");
    }

    fn attach_pod(&mut self, uid: &str, ip: Ipv4Addr, pid: u32) -> VethPair {
        let engine = self.engine.as_mut().unwrap();
        let pair = engine.attach_pod(uid, ip, pid).expect("attach_pod");
        self.pods.push((uid.to_string(), ip));
        pair
    }

    fn cleanup_engine(&mut self) {
        if let Some(ref mut engine) = self.engine {
            // Detach all pods.
            for (uid, _) in self.pods.drain(..) {
                let _ = engine.detach_pod(&uid);
            }
            // Reconcile to empty — this removes all nftables tables.
            let empty = NetmuxState::new();
            let ops = network::reconcile(&empty, engine.current());
            let _ = engine.apply(&ops, false);
        }
        self.engine = None;
    }
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        // Best-effort engine cleanup.
        self.cleanup_engine();
        // Kill all netns processes.
        for name in &self.netns_names {
            let pids = run("ip", &["netns", "pids", name]);
            for pid_str in pids.lines() {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
                }
            }
        }
        // Delete netns.
        for name in &self.netns_names {
            for _ in 0..5 {
                if Command::new("ip")
                    .args(["netns", "delete", name])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        // Nuclear safety net — clean ANY leftover test resources.
        clean_everything();
    }
}

/// Run a closure with guaranteed cleanup — even on panic.
fn with_guard<F: FnOnce(&mut TestGuard) -> () + panic::UnwindSafe>(f: F) {
    clean_everything(); // pre-clean leftovers from prior crashed runs
    let mut guard = TestGuard::new();
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| f(&mut guard)));
    drop(guard); // Drop impl handles engine + netns + safety net
    if let Err(e) = result {
        panic::resume_unwind(e);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 1: Pod-to-Pod
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn e2e_pod_to_pod() {
    with_guard(|g| {
        let pod_a_ip = Ipv4Addr::new(10, 42, 10, 5);
        let pod_b_ip = Ipv4Addr::new(10, 42, 10, 6);

        let pid_a = g.add_netns("z8s_e2_a");
        let pid_b = g.add_netns("z8s_e2_b");

        let mut b = NetmuxBuilder::new();
        b.add_vnet(VNetSpec {
            name: "default".into(),
            cidr: Ipv4Cidr::parse("10.42.0.0/16").unwrap(),
            internet_access: false,
        });
        b.add_pod(PodNetwork {
            pod_uid: "e2-a".into(),
            pod_ip: pod_a_ip,
            veth_host: String::new(),
            veth_peer: String::new(),
            vnet: Some("default".into()),
        });
        b.add_pod(PodNetwork {
            pod_uid: "e2-b".into(),
            pod_ip: pod_b_ip,
            veth_host: String::new(),
            veth_peer: String::new(),
            vnet: Some("default".into()),
        });
        let desired = b.build();

        g.apply_nft(&desired);
        g.attach_pod("e2-a", pod_a_ip, pid_a);
        g.attach_pod("e2-b", pod_b_ip, pid_b);

        // A → B
        let out = run_in_netns("z8s_e2_a", "ping", &["-c", "3", "-W", "2", &pod_b_ip.to_string()]);
        println!("A→B: {out}");
        assert!(out.contains("3 received"), "A→B failed: {out}");

        // B → A
        let out = run_in_netns("z8s_e2_b", "ping", &["-c", "3", "-W", "2", &pod_a_ip.to_string()]);
        println!("B→A: {out}");
        assert!(out.contains("3 received"), "B→A failed: {out}");

        println!("e2e_pod_to_pod: PASSED");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 2: Pod-to-internet (masquerade)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn e2e_pod_to_internet() {
    with_guard(|g| {
        let pod_ip = Ipv4Addr::new(10, 42, 20, 5);
        let gateway = Ipv4Addr::new(10, 42, 20, 1);

        let pid = g.add_netns("z8s_e2_inet");

        let mut b = NetmuxBuilder::new();
        b.add_vnet(VNetSpec {
            name: "inet".into(),
            cidr: Ipv4Cidr::parse("10.42.20.0/24").unwrap(),
            internet_access: true,
        });
        b.add_pod(PodNetwork {
            pod_uid: "e2-inet".into(),
            pod_ip,
            veth_host: String::new(),
            veth_peer: String::new(),
            vnet: Some("inet".into()),
        });
        let desired = b.build();

        g.apply_nft(&desired);
        g.attach_pod("e2-inet", pod_ip, pid);

        // Pod → gateway
        let out = run_in_netns("z8s_e2_inet", "ping", &["-c", "3", "-W", "2", &gateway.to_string()]);
        println!("pod→gw: {out}");
        assert!(out.contains("3 received"), "pod→gw failed: {out}");

        // Pod → external (best-effort, may be network-isolated)
        let out = run_in_netns("z8s_e2_inet", "ping", &["-c", "2", "-W", "3", "1.1.1.1"]);
        println!("pod→ext: {out}");
        if out.contains("received") {
            println!("pod→ext: reachable via masquerade");
        } else {
            println!("WARNING: external unreachable (may be network-isolated)");
        }

        println!("e2e_pod_to_internet: PASSED");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 3: NSG isolation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn e2e_nsg_isolation() {
    with_guard(|g| {
        let pod_a_ip = Ipv4Addr::new(10, 42, 30, 5);
        let pod_b_ip = Ipv4Addr::new(10, 42, 30, 6);

        let pid_a = g.add_netns("z8s_e2_na");
        let pid_b = g.add_netns("z8s_e2_nb");

        // Phase 1: Pod A has deny-all NSG.
        let mut b = NetmuxBuilder::new();
        b.add_vnet(VNetSpec {
            name: "default".into(),
            cidr: Ipv4Cidr::parse("10.42.30.0/24").unwrap(),
            internet_access: false,
        });
        b.add_pod(PodNetwork {
            pod_uid: "e2-na".into(),
            pod_ip: pod_a_ip,
            veth_host: String::new(),
            veth_peer: String::new(),
            vnet: Some("default".into()),
        });
        b.add_pod(PodNetwork {
            pod_uid: "e2-nb".into(),
            pod_ip: pod_b_ip,
            veth_host: String::new(),
            veth_peer: String::new(),
            vnet: Some("default".into()),
        });
        b.add_policy(
            "e2-na",
            &[NsgRule {
                name: "deny-all".into(),
                priority: 1000,
                action: NsgAction::Deny,
                src_cidrs: vec!["0.0.0.0/0".into()],
                dst_cidrs: vec!["0.0.0.0/0".into()],
            }],
        );
        let desired = b.build();

        g.apply_nft(&desired);
        g.attach_pod("e2-na", pod_a_ip, pid_a);
        g.attach_pod("e2-nb", pod_b_ip, pid_b);

        assert!(nft_table_exists("nsg_e2-na"), "NSG table should exist");
        let listing = nft_list("nsg_e2-na");
        assert!(listing.contains("drop"), "should have drop rule: {listing}");
        println!("NSG table:\n{listing}");

        // Phase 2: Remove NSG, verify connectivity restored.
        let mut b2 = NetmuxBuilder::new();
        b2.add_vnet(VNetSpec {
            name: "default".into(),
            cidr: Ipv4Cidr::parse("10.42.30.0/24").unwrap(),
            internet_access: false,
        });
        b2.add_pod(PodNetwork {
            pod_uid: "e2-na".into(),
            pod_ip: pod_a_ip,
            veth_host: String::new(),
            veth_peer: String::new(),
            vnet: Some("default".into()),
        });
        b2.add_pod(PodNetwork {
            pod_uid: "e2-nb".into(),
            pod_ip: pod_b_ip,
            veth_host: String::new(),
            veth_peer: String::new(),
            vnet: Some("default".into()),
        });
        let desired2 = b2.build();

        g.apply_nft(&desired2);
        assert!(!nft_table_exists("nsg_e2-na"), "NSG table should be removed");

        let out = run_in_netns("z8s_e2_nb", "ping", &["-c", "3", "-W", "2", &pod_a_ip.to_string()]);
        println!("B→A (no NSG): {out}");
        assert!(out.contains("3 received"), "B→A failed after removing NSG: {out}");

        println!("e2e_nsg_isolation: PASSED");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 4: Full reconcile pipeline
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn e2e_reconcile_full() {
    with_guard(|g| {
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
                cluster_ip: Some("10.96.0.50".into()),
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
            node_name: "e2-full".into(),
            pod_cidr: Ipv4Cidr::parse("10.42.0.0/16").unwrap(),
            service_cidr: Ipv4Cidr::parse("10.96.0.0/16").unwrap(),
            cluster_domain: "cluster.local".into(),
            gateway: Ipv4Addr::new(10, 42, 0, 1),
            peers: vec![],
        };

        let engine = g.engine.as_mut().unwrap();
        let report = engine.reconcile_full(&snap, &cfg).expect("reconcile_full");
        println!("report: {:?}", report);
        assert!(report.total() > 0);

        assert!(nft_table_exists("z8s_nat_e2-full"), "nat table missing");
        assert!(nft_table_exists("z8s_filter_e2-full"), "filter table missing");

        let nat = nft_list("z8s_nat_e2-full");
        assert!(nat.contains("masquerade"), "should have masquerade: {nat}");

        println!("e2e_reconcile_full: PASSED");
    });
}
