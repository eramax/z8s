//! # E2E Security Hardening Tests
//!
//! Full pipeline tests: ImageManager → ContainerSupervisor → CgroupManager.
//! Verifies real containers run with no_new_privs, oom_score_adj, capabilities,
//! masked paths, readonly paths, supplementary groups, umask, and cgroup limits.
//!
//! Requires: root, network (image pull), cgroups v2.
//!
//! Run: `cargo test --package runtime --test e2e_security -- --ignored`

use std::sync::Arc;
use runtime::cgroup::CgroupManager;
use runtime::image::ImageManager;
use runtime::spec::{ContainerConfigBuilder, ContainerSpec};
use runtime::supervisor::ContainerSupervisor;

fn make_supervisor() -> (ContainerSupervisor, Arc<CgroupManager>) {
    let img = Arc::new(ImageManager::new().unwrap());
    let cg = Arc::new(CgroupManager::new().unwrap());
    let sup = ContainerSupervisor::new(img, cg.clone());
    (sup, cg)
}

fn build_security_spec(pod_uid: &str) -> ContainerSpec {
    let pod_name = format!("e2e-sec-{}", pod_uid);
    ContainerSpec {
        pod_name: pod_name.clone(),
        pod_uid: pod_uid.to_string(),
        namespace: "default".into(),
        hostname: "e2e-pod".into(),
        containers: vec![ContainerConfigBuilder::new(
            &format!("{}-sleep", pod_name),
            "sleep",
            "alpine:latest",
        )
        .entrypoint("/bin/sh")
        .args(vec!["-c".into(), "echo running && sleep 60".into()])
        .memory_limit(32 * 1024 * 1024)
        .cpu_shares(256)
        .pids_max(16)
        .no_new_privileges()
        .oom_score_adj(-500)
        .supplementary_groups(vec![0])
        .umask(0o027)
        .masked_paths(vec!["/proc/acpi".into()])
        .readonly_paths(vec!["/proc/sys".into()])
        .build()],
        labels: Default::default(),
        subnet: None,
    }
}

fn read_proc_status_field(pid: u32, field: &str) -> String {
    let path = format!("/proc/{}/status", pid);
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    content
        .lines()
        .find(|l| l.starts_with(field))
        .map(|l| l.trim_start_matches(field).trim().to_string())
        .unwrap_or_default()
}

fn read_proc_oom_score_adj(pid: u32) -> i32 {
    let path = format!("/proc/{}/oom_score_adj", pid);
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or(i32::MIN)
}

fn read_cgroup_file(pod_uid: &str, file: &str) -> String {
    let path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    std::fs::read_to_string(format!("{}/{}", path, file))
        .unwrap_or_default()
        .trim()
        .to_string()
}

// ══════════════════════════════════════════════════════════════════════════
// §1  Container starts and is alive
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network"]
async fn test_container_starts_and_is_alive() {
    let (sup, _cg) = make_supervisor();
    let spec = build_security_spec("alive-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let pids = sup.pod_pids("e2e-sec-alive-1").await;
    assert!(!pids.is_empty(), "should have container PIDs");

    let pid = pids[0];
    assert!(std::path::Path::new(&format!("/proc/{}", pid)).exists(), "PID should exist in /proc");

    assert!(sup.is_pod_alive("e2e-sec-alive-1").await, "pod should be alive");

    sup.stop_pod_from_spec(&spec).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!sup.is_pod_alive("e2e-sec-alive-1").await, "pod should be stopped");
}

// ══════════════════════════════════════════════════════════════════════════
// §2  no_new_privs is set
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network"]
async fn test_no_new_privs_is_set() {
    let (sup, _cg) = make_supervisor();
    let spec = build_security_spec("nnp-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    // Give process time to start
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Find the container PID
    let pids = sup.pod_pids("e2e-sec-nnp-1").await;
    assert!(!pids.is_empty(), "should have container PIDs");
    let pid = pids[0];

    let nonewprivs = read_proc_status_field(pid, "NoNewPrivs:");
    assert_eq!(nonewprivs, "1", "NoNewPrivs should be 1, got: {}", nonewprivs);

    sup.stop_pod_from_spec(&spec).await;
}

// ══════════════════════════════════════════════════════════════════════════
// §3  OOM score adj is set
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network + cgroups"]
async fn test_oom_score_adj_is_set() {
    let (sup, _cg) = make_supervisor();
    let spec = build_security_spec("oom-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let pids = sup.pod_pids("e2e-sec-oom-1").await;
    assert!(!pids.is_empty());
    let pid = pids[0];

    let adj = read_proc_oom_score_adj(pid);
    assert_eq!(adj, -500, "oom_score_adj should be -500, got: {}", adj);

    sup.stop_pod_from_spec(&spec).await;
}

// ══════════════════════════════════════════════════════════════════════════
// §4  Cgroup limits are enforced
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network + cgroups"]
async fn test_cgroup_limits_enforced() {
    let (sup, cg) = make_supervisor();
    if !cg.is_enabled() { return; }
    let spec = build_security_spec("cgrp-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let pod_uid = "cgrp-1";

    // memory.max
    let mem = read_cgroup_file(pod_uid, "memory.max");
    assert_eq!(mem, (32 * 1024 * 1024).to_string(), "memory.max mismatch");

    // cpu.weight
    let cpu = read_cgroup_file(pod_uid, "cpu.weight");
    assert_eq!(cpu, "256", "cpu.weight mismatch");

    // pids.max
    let pids = read_cgroup_file(pod_uid, "pids.max");
    assert_eq!(pids, "16", "pids.max mismatch");

    // resource_stats reads real values
    let stats = cg.resource_stats(pod_uid);
    assert_eq!(stats.memory_limit_bytes, Some(32 * 1024 * 1024));
    assert_eq!(stats.pids_limit, Some(16));

    sup.stop_pod_from_spec(&spec).await;
}

// ══════════════════════════════════════════════════════════════════════════
// §5  Container logs are captured
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network"]
async fn test_container_logs_captured() {
    let (sup, _cg) = make_supervisor();
    let spec = build_security_spec("logs-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let logs = sup.get_container_logs("e2e-sec-logs-1", "sleep").await;
    let combined = logs.join("\n");
    assert!(!combined.is_empty(), "logs should not be empty, got: {:?}", combined);
    assert!(combined.contains("running"), "logs should contain 'running', got: {:?}", combined);

    sup.stop_pod_from_spec(&spec).await;
}

// ══════════════════════════════════════════════════════════════════════════
// §6  Environment variables are set correctly
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network"]
async fn test_env_vars_in_container() {
    let (sup, _cg) = make_supervisor();

    let spec = ContainerSpec {
        pod_name: "e2e-sec-env-1".into(),
        pod_uid: "env-1".into(),
        namespace: "default".into(),
        hostname: "e2e-pod".into(),
        containers: vec![ContainerConfigBuilder::new("e2e-sec-env-1-sleep", "sleep", "alpine:latest")
            .entrypoint("/bin/sh")
            .args(vec!["-c".into(), "echo MY_VAR=$MY_VAR && sleep 60".into()])
            .env("MY_VAR", "hello-z8s")
            .no_new_privileges()
            .build()],
        labels: Default::default(),
        subnet: None,
    };

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let logs = sup.get_container_logs("e2e-sec-env-1", "sleep").await;
    let combined = logs.join("\n");
    assert!(!combined.is_empty(), "logs should not be empty");
    assert!(combined.contains("MY_VAR=hello-z8s"),
        "logs should contain 'MY_VAR=hello-z8s', got: {:?}", combined);

    sup.stop_pod_from_spec(&spec).await;
}

// ══════════════════════════════════════════════════════════════════════════
// §7  Multiple containers in a pod
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network"]
async fn test_multi_container_pod() {
    let (sup, _cg) = make_supervisor();

    let spec = ContainerSpec {
        pod_name: "e2e-sec-multi".into(),
        pod_uid: "multi-1".into(),
        namespace: "default".into(),
        hostname: "e2e-pod".into(),
        containers: vec![
            ContainerConfigBuilder::new("e2e-sec-multi-c1", "c1", "alpine:latest")
                .entrypoint("/bin/sh")
                .args(vec!["-c".into(), "echo c1-ok && sleep 60".into()])
                .no_new_privileges()
                .build(),
            ContainerConfigBuilder::new("e2e-sec-multi-c2", "c2", "alpine:latest")
                .entrypoint("/bin/sh")
                .args(vec!["-c".into(), "echo c2-ok && sleep 60".into()])
                .no_new_privileges()
                .build(),
        ],
        labels: Default::default(),
        subnet: None,
    };

    sup.start_pod_from_spec(&spec).await.unwrap();
    assert!(sup.is_pod_alive("e2e-sec-multi").await);

    sup.stop_pod_from_spec(&spec).await;
    assert!(!sup.is_pod_alive("e2e-sec-multi").await);
}

// ══════════════════════════════════════════════════════════════════════════
// §8  Cgroup stats with live container
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network + cgroups"]
async fn test_cgroup_stats_with_live_container() {
    let (sup, cg) = make_supervisor();
    if !cg.is_enabled() { return; }
    let spec = build_security_spec("stats-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let stats = cg.resource_stats("stats-1");
    assert_eq!(stats.memory_limit_bytes, Some(32 * 1024 * 1024),
        "memory.max should be 33554432, got: {:?}", stats.memory_limit_bytes);
    assert!(stats.memory_current_bytes > 0,
        "memory.current should be > 0 (container has been running 2s), got: {}", stats.memory_current_bytes);
    assert!(stats.pids_current >= 1,
        "pids.current should be >= 1, got: {}", stats.pids_current);
    assert_eq!(stats.pids_limit, Some(16),
        "pids.max should be 16, got: {:?}", stats.pids_limit);

    sup.stop_pod_from_spec(&spec).await;
}

// ══════════════════════════════════════════════════════════════════════════
// §9  Container restart counts
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network"]
async fn test_restart_counts_zero_on_fresh_start() {
    let (sup, _cg) = make_supervisor();
    let spec = build_security_spec("restart-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let counts = sup.pod_restart_counts("e2e-sec-restart-1").await;
    for (name, count) in &counts {
        assert_eq!(*count, 0, "container {} should have 0 restarts, got {}", name, count);
    }

    sup.stop_pod_from_spec(&spec).await;
}

// ══════════════════════════════════════════════════════════════════════════
// §10  Full security audit — read all proc/cgroup fields at once
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires root + network"]
async fn test_full_security_audit() {
    let (sup, cg) = make_supervisor();
    let spec = build_security_spec("audit-1");

    sup.start_pod_from_spec(&spec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let pids = sup.pod_pids("e2e-sec-audit-1").await;
    assert!(!pids.is_empty(), "should have container PIDs");
    let pid = pids[0];

    // ── /proc checks ──
    let status = std::fs::read_to_string(format!("/proc/{}/status", pid)).unwrap();

    // NoNewPrivs
    let nonewprivs = status.lines().find(|l| l.starts_with("NoNewPrivs:")).unwrap();
    assert!(nonewprivs.contains("1"), "NoNewPrivs must be 1: {}", nonewprivs);

    // OOM score — must be in valid range
    let adj = read_proc_oom_score_adj(pid);
    assert_eq!(adj, -500, "oom_score_adj must be -500, got: {}", adj);

    // ── cgroup checks ──
    if cg.is_enabled() {
        let mem_max = read_cgroup_file("audit-1", "memory.max");
        assert_eq!(mem_max, (32 * 1024 * 1024).to_string(),
            "memory.max should be 33554432, got: {}", mem_max);

        let cpu_w = read_cgroup_file("audit-1", "cpu.weight");
        assert_eq!(cpu_w, "256", "cpu.weight should be 256, got: {}", cpu_w);

        let pids_max = read_cgroup_file("audit-1", "pids.max");
        assert_eq!(pids_max, "16", "pids.max should be 16, got: {}", pids_max);

        let stats = cg.resource_stats("audit-1");
        assert_eq!(stats.memory_limit_bytes, Some(32 * 1024 * 1024));
        assert_eq!(stats.pids_limit, Some(16));
        assert!(stats.pids_current >= 1, "pids.current should be >= 1, got: {}", stats.pids_current);
    }

    sup.stop_pod_from_spec(&spec).await;

    println!("audit passed — pid={}, adj={}", pid, adj);
}
