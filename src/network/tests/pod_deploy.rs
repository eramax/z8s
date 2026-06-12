//! # Pod Deployment Integration Test
//!
//! Deploys 3 simulated pods using veth pairs + netns, assigns IPs from a
//! virtual network, and verifies that:
//! 1. veth NICs are established (visible via `ip link`)
//! 2. IPs are assigned to the pod-side interfaces
//! 3. The host-side veths are visible on the host
//! 4. Cleanup removes all interfaces and netns
//!
//! This test exercises the veth/netns path (NETLINK_ROUTE) independently
//! of the nftables path (NETLINK_NETFILTER). The user's original request
//! was to verify veth NICs are established when pods are deployed.
//!
//! Requires: `sudo` (CAP_NET_ADMIN), `ip` command in PATH.

#![cfg(target_os = "linux")]

use std::process::Command;

/// Run a shell command, panic with context on failure.
fn run(cmd: &str, args: &[&str]) -> String {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd}: {e}"));
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        panic!(
            "{cmd} {} failed (exit {:?}):\nstdout: {stdout}\nstderr: {stderr}",
            args.join(" "),
            out.status.code()
        );
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run a command, ignore failure (for cleanup).
fn run_quiet(cmd: &str, args: &[&str]) {
    let _ = Command::new(cmd).args(args).output();
}

/// Best-effort cleanup of all test resources.
fn cleanup(netns: &[&str], host_veths: &[&str]) {
    for ns in netns {
        run_quiet("ip", &["netns", "delete", ns]);
    }
    for v in host_veths {
        run_quiet("ip", &["link", "delete", v]);
    }
}

#[test]
fn deploy_three_pods_with_veth_and_netns() {
    // Pre-clean any stale state from prior failed runs.
    cleanup(
        &["z8s_test_pod1", "z8s_test_pod2", "z8s_test_pod3"],
        &[
            "veth_test_host1",
            "veth_test_host2",
            "veth_test_host3",
        ],
    );

    let pods = [
        ("z8s_test_pod1", "veth_test_host1", "10.244.1.2/24"),
        ("z8s_test_pod2", "veth_test_host2", "10.244.2.2/24"),
        ("z8s_test_pod3", "veth_test_host3", "10.244.3.2/24"),
    ];
    let _pod_veths = ["veth_test_pod1", "veth_test_pod2", "veth_test_pod3"];

    // 1. Create 3 network namespaces (simulating pod sandboxes).
    for (ns, _, _) in &pods {
        run("ip", &["netns", "add", ns]);
    }
    let netns_list = run("ip", &["netns", "list"]);
    for (ns, _, _) in &pods {
        assert!(
            netns_list.contains(ns),
            "netns {ns} not found after creation. Got: {netns_list}"
        );
    }

    // 2. Create 3 veth pairs (host ↔ pod).
    for (ns, host, _) in &pods {
        let pod_veth = format!("veth_test_{ns}").replace("z8s_test_", "");
        run(
            "ip",
            &["link", "add", host, "type", "veth", "peer", "name", &pod_veth],
        );
    }

    // 3. Move the pod-side veth into each pod's netns.
    for (ns, _, _) in &pods {
        let pod_veth = format!("veth_test_{ns}").replace("z8s_test_", "");
        run("ip", &["link", "set", &pod_veth, "netns", ns]);
    }

    // 4. Bring up the host-side veths.
    for (_, host, _) in &pods {
        run("ip", &["link", "set", host, "up"]);
    }

    // 5. Bring up the pod-side veths (inside each netns) and assign IPs.
    for (ns, _, ip) in &pods {
        let pod_veth = format!("veth_test_{ns}").replace("z8s_test_", "");
        run(
            "ip",
            &[
                "netns", "exec", ns, "ip", "link", "set", "lo", "up",
            ],
        );
        run(
            "ip",
            &[
                "netns", "exec", ns, "ip", "link", "set", &pod_veth, "up",
            ],
        );
        run(
            "ip",
            &[
                "netns", "exec", ns, "ip", "addr", "add", ip, "dev", &pod_veth,
            ],
        );
    }

    // ── Verification ───────────────────────────────────────────────────

    // Verify host-side veths are visible.
    let link_show = run("ip", &["-br", "link", "show"]);
    for (_, host, _) in &pods {
        assert!(
            link_show.contains(host),
            "host veth {host} not visible in `ip link`. Got:\n{link_show}"
        );
    }

    // Verify pod-side veths are visible inside their netns.
    for (ns, _, ip) in &pods {
        let pod_veth = format!("veth_test_{ns}").replace("z8s_test_", "");
        let pod_link = run("ip", &["netns", "exec", ns, "ip", "-br", "link", "show"]);
        assert!(
            pod_link.contains(&pod_veth),
            "pod veth {pod_veth} not visible in netns {ns}. Got:\n{pod_link}"
        );

        // Verify IP is assigned to the pod-side veth.
        let pod_addr = run("ip", &["netns", "exec", ns, "ip", "-br", "addr", "show"]);
        let ip_prefix = ip.split('/').next().unwrap();
        assert!(
            pod_addr.contains(ip_prefix),
            "IP {ip_prefix} not assigned to pod veth in netns {ns}. Got:\n{pod_addr}"
        );
    }

    // Verify veth NICs are established (state UP, not UNKNOWN/DOWN).
    for (ns, _, _) in &pods {
        let pod_veth = format!("veth_test_{ns}").replace("z8s_test_", "");
        let state = run("ip", &["netns", "exec", ns, "ip", "-br", "link", "show"]);
        // The line for our veth should show "UP" state.
        let line = state
            .lines()
            .find(|l| l.contains(&pod_veth))
            .unwrap_or_else(|| panic!("pod veth {pod_veth} not found in:\n{state}"));
        assert!(
            line.contains("UP"),
            "pod veth {pod_veth} is not UP. Line: {line}"
        );
    }

    // Verify host-side veths are also UP.
    for (_, host, _) in &pods {
        let line = link_show
            .lines()
            .find(|l| l.contains(host))
            .unwrap_or_else(|| panic!("host veth {host} not found in:\n{link_show}"));
        assert!(
            line.contains("UP"),
            "host veth {host} is not UP. Line: {line}"
        );
    }

    // ── Cleanup ────────────────────────────────────────────────────────
    let host_veths: Vec<&str> = pods.iter().map(|(_, h, _)| *h).collect();
    cleanup(&pods.iter().map(|(n, _, _)| *n).collect::<Vec<_>>(), &host_veths);

    // Verify cleanup.
    let link_after = run("ip", &["-br", "link", "show"]);
    for (_, host, _) in &pods {
        assert!(
            !link_after.contains(host),
            "host veth {host} not cleaned up. Got:\n{link_after}"
        );
    }
    let netns_after = run("ip", &["netns", "list"]);
    for (ns, _, _) in &pods {
        assert!(
            !netns_after.contains(ns),
            "netns {ns} not cleaned up. Got:\n{netns_after}"
        );
    }
}

#[test]
fn verify_veth_pairs_are_bidirectional() {
    // Clean up first.
    cleanup(
        &["z8s_bidir_pod"],
        &["veth_bidir_host"],
    );

    // Create one veth pair and verify the peer relationship.
    run(
        "ip",
        &[
            "netns", "add", "z8s_bidir_pod",
        ],
    );
    run(
        "ip",
        &[
            "link", "add", "veth_bidir_host", "type", "veth", "peer", "name",
            "veth_bidir_pod",
        ],
    );
    run(
        "ip",
        &["link", "set", "veth_bidir_pod", "netns", "z8s_bidir_pod"],
    );
    run("ip", &["link", "set", "veth_bidir_host", "up"]);
    run(
        "ip",
        &[
            "netns", "exec", "z8s_bidir_pod", "ip", "link", "set",
            "veth_bidir_pod", "up",
        ],
    );

    // Verify peer ifindex on both sides — the host veth's "link_index"
    // field should point to the pod veth's "ifindex" (veth peers share an
    // ifindex pair).
    let host_json = run("ip", &["-j", "link", "show", "veth_bidir_host"]);
    let host_peer = extract_json_u32(&host_json, "link_index")
        .expect("host veth should have link_index (peer) ifindex");

    let pod_json = run(
        "ip",
        &[
            "netns", "exec", "z8s_bidir_pod", "ip", "-j", "link", "show",
            "veth_bidir_pod",
        ],
    );
    let pod_ifindex_val = extract_json_u32(&pod_json, "ifindex")
        .expect("pod veth should have ifindex");
    assert_eq!(
        host_peer, pod_ifindex_val,
        "veth peer mismatch: host.link_index={host_peer} != pod.ifindex={pod_ifindex_val}"
    );

    // Cleanup.
    cleanup(&["z8s_bidir_pod"], &["veth_bidir_host"]);
}

/// Extract a u32 field from a single-object `ip -j` JSON output.
fn extract_json_u32(json: &str, field: &str) -> Option<u32> {
    let needle = format!("\"{field}\":");
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..];
    let trimmed = after.trim_start();
    let num_str: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    num_str.parse().ok()
}
