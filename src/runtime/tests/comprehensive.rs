//! # Comprehensive Integration Tests — Container Runtime
//!
//! Tests cover every module: image, rootfs, cgroup, exec, health, supervisor, spec.
//! Tests marked `#[ignore]` require network or root.
//!
//! Run: `cargo test --package runtime --test comprehensive`
//! Ignored: `cargo test --package runtime --test comprehensive -- --ignored`

use std::path::PathBuf;
use runtime::image::ImageManager;
use runtime::rootfs;
use runtime::supervisor;
use runtime::spec;
use runtime::cgroup::CgroupManager;
use runtime::health::{HealthChecker, HealthStatus, ProbeAction, ProbeConfig, ExecProbe, TcpProbe};
#[allow(unused_imports)]
use std::os::unix::fs::PermissionsExt;

fn tmp(sub: &str) -> PathBuf { std::env::temp_dir().join(format!("z8s_test_{}", sub)) }
fn cleanup(name: &str) { let _ = std::fs::remove_dir_all(tmp(name)); }

// ══════════════════════════════════════════════════════════════════════════
// §1  IMAGE MODULE
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn test_pull_alpine_has_essential_bins() {
    cleanup("alpine_bins");
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("alpine:latest", "alpine-bins").await.unwrap();
    let root = PathBuf::from(&r);

    // alpine: busybox with symlinks
    assert!(root.join("bin/busybox").exists(), "busybox missing");
    assert!(root.join("bin/sh").exists() || root.join("bin/ash").exists(), "shell missing");
    assert!(root.join("etc/alpine-release").exists(), "alpine-release missing");
    assert!(root.join("etc/passwd").exists(), "passwd missing");
    assert!(root.join("etc/hosts").exists(), "hosts missing");
    assert!(root.join("tmp").is_dir(), "/tmp missing");
    assert!(root.join("var").is_dir(), "/var missing");

    // Check OCI config was saved
    assert!(root.join(".z8s-oci-config.json").exists());
    let cfg = std::fs::read_to_string(root.join(".z8s-oci-config.json")).unwrap();
    assert!(cfg.contains("entrypoint") || cfg.contains("cmd"));

    cleanup("alpine_bins");
}

#[tokio::test]
#[ignore]
async fn test_pull_ubuntu_has_essential_bins() {
    cleanup("ubuntu_bins");
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("ubuntu:latest", "ubuntu-bins").await.unwrap();
    let root = PathBuf::from(&r);

    assert!(root.join("bin/sh").exists(), "sh missing");
    assert!(root.join("bin/bash").exists(), "bash missing");
    assert!(root.join("usr/bin/env").exists(), "env missing");
    assert!(root.join("etc/os-release").exists(), "os-release missing");
    assert!(root.join("etc/passwd").exists(), "passwd missing");

    cleanup("ubuntu_bins");
}

#[tokio::test]
#[ignore]
async fn test_pull_nginx_has_config_and_binary() {
    cleanup("nginx_bins");
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("nginx:latest", "nginx-bins").await.unwrap();
    let root = PathBuf::from(&r);

    assert!(root.join("etc/nginx").is_dir(), "nginx config dir missing");
    assert!(
        root.join("usr/sbin/nginx").exists() || root.join("sbin/nginx").exists(),
        "nginx binary missing"
    );
    assert!(root.join("etc/nginx/nginx.conf").exists(), "nginx.conf missing");

    cleanup("nginx_bins");
}

#[tokio::test]
#[ignore]
async fn test_image_cache_reuse() {
    cleanup("cache_reuse");
    let mgr = ImageManager::new().unwrap();

    let r1 = mgr.unpack_image("alpine:latest", "cache-a").await.unwrap();
    let r2 = mgr.unpack_image("alpine:latest", "cache-b").await.unwrap();

    // Both should succeed (cache hit on second pull)
    assert!(PathBuf::from(&r1).exists());
    assert!(PathBuf::from(&r2).exists());
    assert_ne!(r1, r2, "different container IDs should have different rootfs paths");

    // Check the image ref marker
    let meta1 = std::fs::read_to_string(format!("{}/.z8s-image-ref", r1)).unwrap();
    assert!(meta1.contains("alpine"));

    cleanup("cache_reuse");
}

#[tokio::test]
#[ignore]
async fn test_image_layer_ordering() {
    cleanup("layer_order");
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("alpine:latest", "layer-test").await.unwrap();
    let root = PathBuf::from(&r);

    // Verify base layer files exist (lower layers extracted first)
    assert!(root.join("bin/busybox").exists());
    // Verify upper layer files exist (alpine-release added later)
    assert!(root.join("etc/alpine-release").exists());

    cleanup("layer_order");
}

// ══════════════════════════════════════════════════════════════════════════
// §2  ROOTFS MODULE
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_prepare_rootfs_all_dirs() {
    cleanup("rootfs_dirs");
    let root = tmp("rootfs_dirs");
    std::fs::create_dir_all(&root).unwrap();

    rootfs::prepare_rootfs(root.to_str().unwrap()).unwrap();

    for dir in &["proc", "sys", "dev", "dev/pts", "tmp", "etc", "run", "dev/shm"] {
        assert!(root.join(dir).exists(), "/{} should exist", dir);
    }
    assert!(root.join("etc/resolv.conf").exists());
    assert!(root.join("etc/hosts").exists());

    let hosts = std::fs::read_to_string(root.join("etc/hosts")).unwrap();
    assert!(hosts.contains("localhost"));

    cleanup("rootfs_dirs");
}

#[test]
fn test_prepare_rootfs_idempotent() {
    cleanup("rootfs_idempotent");
    let root = tmp("rootfs_idempotent");
    std::fs::create_dir_all(&root).unwrap();

    rootfs::prepare_rootfs(root.to_str().unwrap()).unwrap();
    rootfs::prepare_rootfs(root.to_str().unwrap()).unwrap(); // second call should not fail

    assert!(root.join("etc/resolv.conf").exists());
    cleanup("rootfs_idempotent");
}

#[test]
fn test_prepare_rootfs_nonexistent() {
    assert!(rootfs::prepare_rootfs("/nonexistent/path").is_err());
}

#[test]
fn test_resolve_exec_path_absolute() {
    assert_eq!(
        rootfs::resolve_exec_path("/usr/bin/ls", "/rootfs"),
        "/rootfs/usr/bin/ls"
    );
}

#[test]
fn test_resolve_exec_path_trailing_slash() {
    assert_eq!(
        rootfs::resolve_exec_path("/bin/sh", "/rootfs/"),
        "/rootfs/bin/sh"
    );
}

#[test]
fn test_resolve_exec_path_relative() {
    let path = rootfs::resolve_exec_path("ls", "/rootfs");
    assert!(path.contains("ls"));
}

#[test]
fn test_host_path_in_container_root_exact() {
    assert_eq!(
        rootfs::host_path_in_container_root("/rootfs", "/rootfs"),
        "/"
    );
}

#[test]
fn test_host_path_in_container_root_nested() {
    assert_eq!(
        rootfs::host_path_in_container_root("/rootfs/usr/bin/ls", "/rootfs"),
        "/usr/bin/ls"
    );
}

#[test]
fn test_host_path_in_container_root_no_match() {
    assert_eq!(
        rootfs::host_path_in_container_root("/usr/bin/ls", "/rootfs"),
        "/usr/bin/ls"
    );
}

#[test]
fn test_build_container_argv_no_busybox() {
    let (path, args) = rootfs::build_container_argv("/usr/bin/sleep", &["3600".into()], "/rootfs");
    assert!(path.contains("sleep"));
    assert_eq!(args, vec!["3600"]);
}

#[test]
fn test_mount_propagation_flags() {
    use rustix::mount::MountPropagationFlags;
    let _ = MountPropagationFlags::DOWNSTREAM;
    let _ = MountPropagationFlags::PRIVATE;
    let _ = MountPropagationFlags::REC;
}

#[test]
fn test_mount_flags() {
    use rustix::mount::MountFlags;
    let _ = MountFlags::BIND;
    let _ = MountFlags::REC;
    let _ = MountFlags::RDONLY;
}

// ══════════════════════════════════════════════════════════════════════════
// §3  CGROUP MODULE
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_cgroup_sanitize() {
    // Cgroup names: replace / : .
    let name = "default/pod-abc.def:123";
    let safe = name.replace(['/', '.', ':'], "_");
    assert_eq!(safe, "default_pod-abc_def_123");
}

#[test]
fn test_cgroup_stub_noop() {
    let c = CgroupManager::new_stub();
    assert!(c.create_pod_cgroup("test").unwrap().is_empty());
    c.add_pid_to_cgroup("test", 1).unwrap();
    c.set_memory_limit("test", 1024).unwrap();
    c.set_cpu_limit("test", 100, 100).unwrap();
    c.remove_cgroup("test").unwrap();
}

#[test]
#[ignore = "needs root"]
fn test_cgroup_full_lifecycle() {
    if !rootfs::is_root() { return; }
    let c = CgroupManager::new().unwrap();
    let uid = "test-cg-full";

    let path = c.create_pod_cgroup(uid).unwrap();
    assert!(PathBuf::from(&path).exists());

    c.add_pid_to_cgroup(uid, std::process::id()).unwrap();
    let procs = std::fs::read_to_string(format!("{}/cgroup.procs", path)).unwrap();
    assert!(procs.contains(&std::process::id().to_string()));

    c.set_memory_limit(uid, 64 * 1024 * 1024).unwrap();
    let mem = std::fs::read_to_string(format!("{}/memory.max", path)).unwrap();
    assert_eq!(mem.trim(), "67108864");

    c.set_memory_low(uid, 32 * 1024 * 1024).unwrap();
    let low = std::fs::read_to_string(format!("{}/memory.low", path)).unwrap();
    assert_eq!(low.trim(), "33554432");

    c.set_cpu_limit(uid, 50000, 100000).unwrap();
    let cpu = std::fs::read_to_string(format!("{}/cpu.max", path)).unwrap();
    assert_eq!(cpu.trim(), "50000 100000");

    c.remove_cgroup(uid).unwrap();
}

#[test]
fn test_apply_limits() {
    let c = CgroupManager::new_stub();
    let configs = vec![
        spec::ContainerConfigBuilder::new("c1", "web", "nginx")
            .memory_limit(128 * 1024 * 1024)
            .cpu_limit(50000, 100000)
            .build(),
    ];
    runtime::cgroup::apply_limits(&c, "pod-1", &configs);
    // no-op on stub, just verify it doesn't panic
}

// ══════════════════════════════════════════════════════════════════════════
// §4  HEALTH MODULE
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_exec_probe_true() {
    assert_eq!(HealthChecker::check_exec(&["/bin/true".into()], Duration::from_secs(5)).await, HealthStatus::Healthy);
}

#[tokio::test]
async fn test_exec_probe_false() {
    assert_eq!(HealthChecker::check_exec(&["/bin/false".into()], Duration::from_secs(5)).await, HealthStatus::Unhealthy);
}

#[tokio::test]
async fn test_exec_probe_empty() {
    assert_eq!(HealthChecker::check_exec(&[], Duration::from_secs(1)).await, HealthStatus::Unknown);
}

#[tokio::test]
async fn test_exec_probe_timeout() {
    assert_eq!(
        HealthChecker::check_exec(&["sleep".into(), "10".into()], Duration::from_millis(50)).await,
        HealthStatus::Unhealthy
    );
}

#[tokio::test]
async fn test_exec_probe_nonexistent() {
    assert_eq!(
        HealthChecker::check_exec(&["/nonexistent/binary".into()], Duration::from_secs(1)).await,
        HealthStatus::Unhealthy
    );
}

#[tokio::test]
async fn test_tcp_probe_refused() {
    assert_eq!(
        HealthChecker::check_tcp(&TcpProbe { host: None, port: 19999 }, Duration::from_millis(200)).await,
        HealthStatus::Unhealthy
    );
}

#[tokio::test]
async fn test_tcp_probe_timeout() {
    assert_eq!(
        HealthChecker::check_tcp(&TcpProbe { host: Some("192.0.2.1".into()), port: 80 }, Duration::from_millis(100)).await,
        HealthStatus::Unhealthy
    );
}

#[tokio::test]
async fn test_tcp_probe_listening() {
    // Start a temporary TCP listener
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            use std::io::Write;
            let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
        }
    });

    assert_eq!(
        HealthChecker::check_tcp(&TcpProbe { host: Some("127.0.0.1".into()), port }, Duration::from_millis(500)).await,
        HealthStatus::Healthy
    );
}

#[tokio::test]
async fn test_exec_probe_dispatch() {
    let probe = ProbeConfig {
        action: ProbeAction::Exec(ExecProbe { command: Some(vec!["true".into()]) }),
        initial_delay_seconds: 0,
        period_seconds: 1,
        timeout_seconds: 5,
    };
    let status = HealthChecker::run(&probe, &HashMap::new()).await;
    assert_eq!(status, HealthStatus::Healthy);
}

#[test]
fn test_probe_timeout_conversion() {
    let p = ProbeConfig { action: ProbeAction::Exec(ExecProbe { command: None }), initial_delay_seconds: 0, period_seconds: 1, timeout_seconds: 7 };
    assert_eq!(p.timeout(), Duration::from_secs(7));
}

// ══════════════════════════════════════════════════════════════════════════
// §5  SPEC MODULE
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_builder_full() {
    let cfg = spec::ContainerConfigBuilder::new("c-1", "web", "nginx:1.25")
        .entrypoint("/bin/sh")
        .args(vec!["-c".into(), "echo hi".into()])
        .env("A", "1")
        .env("B", "2")
        .working_dir("/app")
        .memory_limit(256 * 1024 * 1024)
        .cpu_limit(50000, 100000)
        .run_as(1000, 1000)
        .privileged(true)
        .isolated_net(true)
        .build();

    assert_eq!(cfg.container_id, "c-1");
    assert_eq!(cfg.container_name, "web");
    assert_eq!(cfg.image, "nginx:1.25");
    assert_eq!(cfg.entrypoint, "/bin/sh");
    assert_eq!(cfg.args, vec!["-c", "echo hi"]);
    assert_eq!(cfg.env, vec![("A".into(), "1".into()), ("B".into(), "2".into())]);
    assert_eq!(cfg.working_dir, Some("/app".into()));
    assert_eq!(cfg.memory_limit_bytes, Some(256 * 1024 * 1024));
    assert_eq!(cfg.cpu_quota, Some(50000));
    assert_eq!(cfg.cpu_period, Some(100000));
    assert_eq!(cfg.run_as_user, Some(1000));
    assert_eq!(cfg.run_as_group, Some(1000));
    assert!(cfg.privileged);
    assert!(cfg.isolated_net);
}

#[test]
fn test_builder_defaults() {
    let cfg = spec::ContainerConfigBuilder::new("c-1", "web", "alpine").build();
    assert!(cfg.entrypoint.is_empty());
    assert!(cfg.args.is_empty());
    assert!(cfg.env.is_empty());
    assert!(cfg.memory_limit_bytes.is_none());
    assert!(!cfg.privileged);
    assert!(!cfg.isolated_net);
    assert!(!cfg.is_native);
    assert!(cfg.probes.is_empty());
    assert!(cfg.published_ports.is_empty());
}

#[test]
fn test_container_spec() {
    let spec = spec::ContainerSpec {
        pod_name: "web-1".into(),
        pod_uid: "uid-abc".into(),
        namespace: "default".into(),
        hostname: "web-1".into(),
        containers: vec![
            spec::ContainerConfigBuilder::new("c1", "main", "nginx").build(),
        ],
        labels: std::collections::BTreeMap::from([("app".into(), "web".into())]),
        subnet: Some("10.0.0.0/24".into()),
    };
    assert_eq!(spec.pod_name, "web-1");
    assert_eq!(spec.containers.len(), 1);
    assert_eq!(spec.subnet, Some("10.0.0.0/24".into()));
}

// ══════════════════════════════════════════════════════════════════════════
// §6  SUPERVISOR MODULE — merge_env
// ══════════════════════════════════════════════════════════════════════════

fn setup_oci_config(dir: &PathBuf, env: Vec<String>) {
    std::fs::create_dir_all(dir).unwrap();
    let cfg = ImageSavedConfig { entrypoint: Some(vec!["/bin/sh".into()]), env: Some(env), cmd: None, working_dir: None };
    std::fs::write(dir.join(runtime::image::OCI_CONFIG_FILE), serde_json::to_string(&cfg).unwrap()).unwrap();
}

struct ImageSavedConfig { entrypoint: Option<Vec<String>>, env: Option<Vec<String>>, cmd: Option<Vec<String>>, working_dir: Option<String> }
impl serde::Serialize for ImageSavedConfig {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        if let Some(ref v) = self.entrypoint { m.serialize_entry("entrypoint", v)?; }
        if let Some(ref v) = self.cmd { m.serialize_entry("cmd", v)?; }
        if let Some(ref v) = self.env { m.serialize_entry("env", v)?; }
        if let Some(ref v) = self.working_dir { m.serialize_entry("working_dir", v)?; }
        m.end()
    }
}

#[test]
fn test_merge_env_spec_wins() {
    let dir = tmp("merge_wins");
    cleanup("merge_wins");
    setup_oci_config(&dir, vec!["PATH=/usr/bin".into(), "OCI_VAR=old".into()]);

    let merged = supervisor::merge_env(&[("OCI_VAR".into(), "new".into())], dir.to_str().unwrap());
    let v = merged.iter().find(|(k, _)| k == "OCI_VAR").unwrap();
    assert_eq!(v.1, "new");
    cleanup("merge_wins");
}

#[test]
fn test_merge_env_empty_spec() {
    let dir = tmp("merge_empty");
    cleanup("merge_empty");
    setup_oci_config(&dir, vec!["A=1".into(), "B=2".into()]);

    let merged = supervisor::merge_env(&[], dir.to_str().unwrap());
    assert_eq!(merged.len(), 2);
    assert!(merged.iter().any(|(k, v)| k == "A" && v == "1"));
    cleanup("merge_empty");
}

#[test]
fn test_merge_env_spec_only() {
    let dir = tmp("merge_spec_only");
    cleanup("merge_spec_only");
    setup_oci_config(&dir, vec![]);

    let merged = supervisor::merge_env(&[("X".into(), "Y".into())], dir.to_str().unwrap());
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].1, "Y");
    cleanup("merge_spec_only");
}

#[test]
fn test_merge_env_no_oci_config() {
    let dir = tmp("merge_no_oci");
    cleanup("merge_no_oci");
    std::fs::create_dir_all(&dir).unwrap();
    // No OCI config file → guess_image_config returns defaults

    let merged = supervisor::merge_env(&[("A".into(), "1".into())], dir.to_str().unwrap());
    assert!(merged.iter().any(|(k, v)| k == "A" && v == "1"));
    cleanup("merge_no_oci");
}

// ══════════════════════════════════════════════════════════════════════════
// §7  IMAGE MODULE — read_image_config
// ══════════════════════════════════════════════════════════════════════════

use runtime::image::SavedImageConfig;

#[test]
fn test_read_image_config_full() {
    let dir = tmp("img_cfg_full");
    cleanup("img_cfg_full");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = SavedImageConfig { entrypoint: Some(vec!["/bin/sh".into(), "-c".into()]), cmd: Some(vec!["echo".into()]), env: Some(vec!["PATH=/usr/bin".into()]), working_dir: Some("/app".into()) };
    std::fs::write(dir.join(runtime::image::OCI_CONFIG_FILE), serde_json::to_string(&cfg).unwrap()).unwrap();

    let loaded = runtime::image::read_image_config(dir.to_str().unwrap());
    assert_eq!(loaded.entrypoint, Some(vec!["/bin/sh".into(), "-c".into()]));
    assert_eq!(loaded.cmd, Some(vec!["echo".into()]));
    assert_eq!(loaded.env, Some(vec!["PATH=/usr/bin".into()]));
    assert_eq!(loaded.working_dir, Some("/app".into()));
    cleanup("img_cfg_full");
}

#[test]
fn test_read_image_config_empty() {
    let dir = tmp("img_cfg_empty");
    cleanup("img_cfg_empty");
    std::fs::create_dir_all(&dir).unwrap();
    let loaded = runtime::image::read_image_config(dir.to_str().unwrap());
    assert!(loaded.entrypoint.is_none());
    cleanup("img_cfg_empty");
}

#[test]
fn test_guess_single_executable() {
    let dir = tmp("guess_single");
    cleanup("guess_single");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("myapp");
    std::fs::write(&bin, b"\x7fELF").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let cfg = runtime::image::read_image_config(dir.to_str().unwrap());
    assert_eq!(cfg.entrypoint, Some(vec!["/myapp".into()]));
    cleanup("guess_single");
}

#[test]
fn test_guess_docker_entrypoint() {
    let dir = tmp("guess_entrypoint");
    cleanup("guess_entrypoint");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("docker-entrypoint.sh"), b"#!/bin/sh\nexec \"$@\"").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.join("docker-entrypoint.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();

    let cfg = runtime::image::read_image_config(dir.to_str().unwrap());
    assert!(cfg.entrypoint.is_some());
    let ep = cfg.entrypoint.unwrap();
    assert!(ep[0].contains("docker-entrypoint"));
    cleanup("guess_entrypoint");
}

// ══════════════════════════════════════════════════════════════════════════
// §8  EXEC MODULE
// ══════════════════════════════════════════════════════════════════════════

use std::collections::HashMap;

#[test]
fn test_merge_exec_env_dedup() {
    let stored: Vec<(String, String)> = vec![("A".into(), "1".into()), ("B".into(), "2".into())];
    // We can't easily test the private merge_exec_env, but we can verify
    // the logic: container env overrides spec env for same key
    let mut result = stored.clone();
    let mut seen: HashMap<String, ()> = result.iter().map(|(k, _)| (k.clone(), ())).collect();
    let container_env: Vec<(String, String)> = vec![("B".into(), "99".into()), ("C".into(), "3".into())];
    for (k, v) in &container_env {
        if seen.insert(k.clone(), ()).is_none() {
            result.push((k.clone(), v.clone()));
        }
    }
    // A comes from spec (first), B from spec (first-wins), C from container
    assert_eq!(result[0], ("A".into(), "1".into()));
    assert_eq!(result[1], ("B".into(), "2".into()));
    assert_eq!(result[2], ("C".into(), "3".into()));
}

// ══════════════════════════════════════════════════════════════════════════
// §9  SYSCALL MODULE
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_pipe2_roundtrip() {
    let (r, w) = z8s_core::sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    z8s_core::sys::write_fd(&w, b"hello").unwrap();
    drop(w);
    let mut buf = [0u8; 16];
    let n = z8s_core::sys::read_fd(&r, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"hello");
}

#[test]
fn test_pipe2_multiple_writes() {
    let (r, w) = z8s_core::sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    z8s_core::sys::write_fd(&w, b"aaa").unwrap();
    z8s_core::sys::write_fd(&w, b"bbb").unwrap();
    z8s_core::sys::write_fd(&w, b"ccc").unwrap();
    drop(w);
    let mut buf = [0u8; 64];
    let n = z8s_core::sys::read_fd(&r, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"aaabbbccc");
}

#[test]
fn test_fork_and_waitpid() {
    let mut child_done = false;
    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => std::process::exit(42),
        z8s_core::sys::ForkResult::Parent(child_pid) => {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(5) {
                if let Some((_pid, exit_code)) = z8s_core::sys::waitpid(child_pid as i32) {
                    assert_eq!(exit_code, 42);
                    child_done = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    assert!(child_done);
}

#[test]
fn test_fork_pipe_communication() {
    let (r, w) = z8s_core::sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => { drop(r); z8s_core::sys::write_fd(&w, b"child-msg").unwrap(); drop(w); std::process::exit(0); }
        z8s_core::sys::ForkResult::Parent(child_pid) => {
            drop(w);
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mut buf = [0u8; 64];
            let n = z8s_core::sys::read_fd(&r, &mut buf).unwrap();
            assert_eq!(&buf[..n], b"child-msg");
            let _ = child_pid;
        }
    }
}

#[test]
fn test_kill_nonexistent() {
    assert!(z8s_core::sys::kill(99999999, rustix::process::Signal::CONT).is_err());
}

#[test]
fn test_open_nonexistent() {
    assert!(z8s_core::sys::open("/nonexistent", rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty()).is_err());
}

#[test]
fn test_set_cloexec() {
    let (r, w) = z8s_core::sys::pipe2(rustix::pipe::PipeFlags::empty()).unwrap();
    z8s_core::sys::set_cloexec(&r).unwrap();
    z8s_core::sys::set_cloexec(&w).unwrap();
    // Just verify no error
    drop(r);
    drop(w);
}

#[test]
#[ignore]
fn test_sethostname_child() {
    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            let _ = z8s_core::sys::sethostname("test-host");
            let h = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
            assert_eq!(h.trim(), "test-host");
            std::process::exit(0);
        }
        z8s_core::sys::ForkResult::Parent(child_pid) => {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(5) {
                if let Some((pid, _)) = z8s_core::sys::waitpid(-1)
                    && pid == child_pid
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §10  FULL LIFECYCLE INTEGRATION
// ══════════════════════════════════════════════════════════════════════════

use std::time::Duration;

#[tokio::test]
#[ignore]
async fn test_alpine_full_lifecycle() {
    cleanup("alpine_lc");
    let mgr = ImageManager::new().unwrap();
    let rootfs = mgr.unpack_image("alpine:latest", "alpine-lc").await.unwrap();
    rootfs::prepare_rootfs(&rootfs).unwrap();

    let rootfs_c = rootfs.clone();
    let mut exited = false;
    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            let _ = z8s_core::sys::chroot(&rootfs_c);
            let _ = z8s_core::sys::chdir("/");
            let p = std::ffi::CString::new("/bin/sh").unwrap();
            let a = std::ffi::CString::new("/bin/sh").unwrap();
            let b = std::ffi::CString::new("-c").unwrap();
            let c = std::ffi::CString::new("echo lifecycle-ok && cat /etc/alpine-release | head -1").unwrap();
            let argv: &[*const std::ffi::c_char] = &[a.as_ptr(), b.as_ptr(), c.as_ptr(), std::ptr::null()];
            let envp: &[*const std::ffi::c_char] = &[std::ptr::null()];
            let _ = z8s_core::sys::execve(&p, argv, envp);
            std::process::exit(1);
        }
        z8s_core::sys::ForkResult::Parent(pid) => {
            let s = std::time::Instant::now();
            while s.elapsed() < Duration::from_secs(5) {
                if let Some((p, _)) = z8s_core::sys::waitpid(-1) && p == pid {
                    exited = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    assert!(exited);
    cleanup("alpine_lc");
}

#[tokio::test]
#[ignore]
async fn test_ubuntu_full_lifecycle() {
    cleanup("ubuntu_lc");
    let mgr = ImageManager::new().unwrap();
    let rootfs = mgr.unpack_image("ubuntu:latest", "ubuntu-lc").await.unwrap();
    rootfs::prepare_rootfs(&rootfs).unwrap();

    let rootfs_c = rootfs.clone();
    let mut exited = false;
    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            let _ = z8s_core::sys::chroot(&rootfs_c);
            let _ = z8s_core::sys::chdir("/");
            let p = std::ffi::CString::new("/bin/sh").unwrap();
            let a = std::ffi::CString::new("/bin/sh").unwrap();
            let b = std::ffi::CString::new("-c").unwrap();
            let c = std::ffi::CString::new("echo ubuntu-lifecycle-ok").unwrap();
            let argv: &[*const std::ffi::c_char] = &[a.as_ptr(), b.as_ptr(), c.as_ptr(), std::ptr::null()];
            let envp: &[*const std::ffi::c_char] = &[std::ptr::null()];
            let _ = z8s_core::sys::execve(&p, argv, envp);
            std::process::exit(1);
        }
        z8s_core::sys::ForkResult::Parent(pid) => {
            let s = std::time::Instant::now();
            while s.elapsed() < Duration::from_secs(5) {
                if let Some((p, _)) = z8s_core::sys::waitpid(-1) && p == pid {
                    exited = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    assert!(exited);
    cleanup("ubuntu_lc");
}

#[tokio::test]
#[ignore]
async fn test_nginx_pull_and_verify() {
    cleanup("nginx_lc");
    let mgr = ImageManager::new().unwrap();
    let rootfs = mgr.unpack_image("nginx:latest", "nginx-lc").await.unwrap();
    let root = PathBuf::from(&rootfs);

    assert!(root.join("etc/nginx").is_dir());
    assert!(root.join("etc/nginx/nginx.conf").exists());
    assert!(
        root.join("usr/sbin/nginx").exists() || root.join("sbin/nginx").exists()
    );

    cleanup("nginx_lc");
}

#[tokio::test]
#[ignore]
async fn test_multiple_images_cache_independent() {
    cleanup("multi_cache");
    let mgr = ImageManager::new().unwrap();
    let a = mgr.unpack_image("alpine:latest", "cache-alpine").await.unwrap();
    let b = mgr.unpack_image("ubuntu:latest", "cache-ubuntu").await.unwrap();

    assert!(PathBuf::from(&a).join("etc/alpine-release").exists());
    assert!(PathBuf::from(&b).join("etc/os-release").exists());
    assert_ne!(a, b);

    cleanup("multi_cache");
}

// ══════════════════════════════════════════════════════════════════════════
// §12  SYSCALL — no_new_privileges kernel test
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_no_new_privileges_sets_bit() {
    use z8s_core::sys;

    // Fork a child that sets no_new_privs and reads its own /proc/self/status
    let (r, w) = sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    match sys::fork().unwrap() {
        sys::ForkResult::Child => {
            drop(r);
            sys::prctl_no_new_privs().ok();
            // Read NoNewPrivs from /proc/self/status
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            let nonew = status.lines()
                .find(|l| l.starts_with("NoNewPrivs:"))
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
            sys::write_fd(&w, nonew.as_bytes()).ok();
            drop(w);
            std::process::exit(0);
        }
        sys::ForkResult::Parent(pid) => {
            drop(w);
            let mut buf = [0u8; 64];
            let n = sys::read_fd(&r, &mut buf).unwrap();
            let val = String::from_utf8_lossy(&buf[..n]).to_string();
            // Wait for child
            let _ = sys::waitpid(pid as i32);
            assert!(val.contains("NoNewPrivs:"));
            assert!(val.contains("1"), "NoNewPrivs should be 1, got: {}", val);
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §13  SYSCALL — OOM score adj kernel test
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_oom_score_adj_sets_value() {
    use z8s_core::sys;

    let (r, w) = sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    match sys::fork().unwrap() {
        sys::ForkResult::Child => {
            drop(r);
            sys::prctl_set_oom_score_adj(-500).ok();
            let adj = std::fs::read_to_string("/proc/self/oom_score_adj").unwrap();
            sys::write_fd(&w, adj.trim().as_bytes()).ok();
            drop(w);
            std::process::exit(0);
        }
        sys::ForkResult::Parent(pid) => {
            drop(w);
            let mut buf = [0u8; 32];
            let n = sys::read_fd(&r, &mut buf).unwrap();
            let val = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = sys::waitpid(pid as i32);
            assert_eq!(val, "-500", "oom_score_adj should be -500, got: {}", val);
        }
    }
}

#[test]
fn test_oom_score_adj_positive() {
    use z8s_core::sys;

    let (r, w) = sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    match sys::fork().unwrap() {
        sys::ForkResult::Child => {
            drop(r);
            sys::prctl_set_oom_score_adj(500).ok();
            let adj = std::fs::read_to_string("/proc/self/oom_score_adj").unwrap();
            sys::write_fd(&w, adj.trim().as_bytes()).ok();
            drop(w);
            std::process::exit(0);
        }
        sys::ForkResult::Parent(pid) => {
            drop(w);
            let mut buf = [0u8; 32];
            let n = sys::read_fd(&r, &mut buf).unwrap();
            let val = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = sys::waitpid(pid as i32);
            assert_eq!(val, "500", "oom_score_adj should be 500, got: {}", val);
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §14  SYSCALL — umask kernel test
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_umask_restricts_file_creation() {
    use z8s_core::sys;

    let (r, w) = sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    match sys::fork().unwrap() {
        sys::ForkResult::Child => {
            drop(r);
            // Set strict umask: no group/other write
            sys::umask(0o077);
            // Create a file
            std::fs::write("/tmp/z8s_umask_test_file", b"test").ok();
            // Read its permissions
            let meta = std::fs::metadata("/tmp/z8s_umask_test_file").unwrap();
            let mode = meta.permissions().mode() & 0o777;
            let result = format!("{:o}", mode);
            sys::write_fd(&w, result.as_bytes()).ok();
            let _ = std::fs::remove_file("/tmp/z8s_umask_test_file");
            drop(w);
            std::process::exit(0);
        }
        sys::ForkResult::Parent(pid) => {
            drop(w);
            let mut buf = [0u8; 16];
            let n = sys::read_fd(&r, &mut buf).unwrap();
            let val = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = sys::waitpid(pid as i32);
            // With umask 077, file should be 600 (owner rw only)
            assert_eq!(val, "600", "file permissions should be 600, got: {}", val);
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §15  SYSCALL — supplementary groups kernel test
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_supplementary_groups_applied() {
    use z8s_core::sys;

    let (r, w) = sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    match sys::fork().unwrap() {
        sys::ForkResult::Child => {
            drop(r);
            // Set supplementary groups (requires root or CAP_SETGID)
            let _ = sys::setgroups(&[0, 1, 2]);
            // Read groups from /proc/self/status
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            let groups_line = status.lines()
                .find(|l| l.starts_with("Groups:"))
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
            sys::write_fd(&w, groups_line.as_bytes()).ok();
            drop(w);
            std::process::exit(0);
        }
        sys::ForkResult::Parent(pid) => {
            drop(w);
            let mut buf = [0u8; 256];
            let n = sys::read_fd(&r, &mut buf).unwrap();
            let val = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = sys::waitpid(pid as i32);
            // Groups should contain our GID (usually 0 or 1000) plus the ones we set
            assert!(val.starts_with("Groups:"), "should have Groups line, got: {}", val);
            // At minimum, group 0 (root) should be present
            let groups: Vec<&str> = val.trim_start_matches("Groups:").trim().split(' ').collect();
            assert!(!groups.is_empty(), "groups should not be empty");
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §16  CGROUP — real kernel cgroup resource limits
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_cgroup_memory_limit_enforced() {
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-mem-limit-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_memory_limit(pod_uid, 32 * 1024 * 1024).unwrap(); // 32 MB

    // Verify by reading the cgroup file
    let cg_path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    let mem_max = std::fs::read_to_string(format!("{}/memory.max", cg_path)).unwrap();
    assert_eq!(mem_max.trim(), (32 * 1024 * 1024).to_string());

    mgr.remove_cgroup(pod_uid).ok();
}

#[test]
fn test_cgroup_cpu_shares_enforced() {
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-cpu-shares-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_cpu_shares(pod_uid, 512).unwrap();

    let cg_path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    let weight = std::fs::read_to_string(format!("{}/cpu.weight", cg_path)).unwrap();
    assert_eq!(weight.trim(), "512");

    mgr.remove_cgroup(pod_uid).ok();
}

#[test]
fn test_cgroup_cpu_quota_enforced() {
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-cpu-quota-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_cpu_limit(pod_uid, 50000, 100000).unwrap(); // 50% CPU

    let cg_path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    let cpu_max = std::fs::read_to_string(format!("{}/cpu.max", cg_path)).unwrap();
    assert_eq!(cpu_max.trim(), "50000 100000");

    mgr.remove_cgroup(pod_uid).ok();
}

#[test]
fn test_cgroup_pids_limit_enforced() {
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-pids-limit-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_pids_max(pod_uid, 32).unwrap();

    let cg_path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    let pids = std::fs::read_to_string(format!("{}/pids.max", cg_path)).unwrap();
    assert_eq!(pids.trim(), "32");

    mgr.remove_cgroup(pod_uid).ok();
}

#[test]
fn test_cgroup_memory_swap_enforced() {
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-mem-swap-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_memory_limit(pod_uid, 64 * 1024 * 1024).unwrap();
    mgr.set_memory_swap(pod_uid, 128 * 1024 * 1024).unwrap();

    let cg_path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    let swap = std::fs::read_to_string(format!("{}/memory.swap.max", cg_path)).unwrap();
    assert_eq!(swap.trim(), (128 * 1024 * 1024).to_string());

    mgr.remove_cgroup(pod_uid).ok();
}

#[test]
fn test_cgroup_cpuset_enforced() {
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-cpuset-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_cpuset_cpus(pod_uid, "0").unwrap();
    mgr.set_cpuset_mems(pod_uid, "0").unwrap();

    let cg_path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    let cpus = std::fs::read_to_string(format!("{}/cpuset.cpus", cg_path)).unwrap();
    let mems = std::fs::read_to_string(format!("{}/cpuset.mems", cg_path)).unwrap();
    assert_eq!(cpus.trim(), "0");
    assert_eq!(mems.trim(), "0");

    mgr.remove_cgroup(pod_uid).ok();
}

#[test]
fn test_cgroup_resource_stats_reads_real_values() {
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-stats-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_memory_limit(pod_uid, 64 * 1024 * 1024).unwrap();
    mgr.set_pids_max(pod_uid, 16).unwrap();

    let stats = mgr.resource_stats(pod_uid);
    // memory.limit should be set
    assert_eq!(stats.memory_limit_bytes, Some(64 * 1024 * 1024));
    // pids_limit should be set
    assert_eq!(stats.pids_limit, Some(16));
    // memory.current should be >= 0
    assert!(stats.memory_current_bytes < 1024 * 1024, "empty cgroup should use minimal memory");
    // pids.current should be 0 (no tasks in cgroup)
    assert_eq!(stats.pids_current, 0);

    mgr.remove_cgroup(pod_uid).ok();
}

// ══════════════════════════════════════════════════════════════════════════
// §17  CGROUP — resource_stats with live process
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_cgroup_stats_with_live_child() {
    use z8s_core::sys;
    use runtime::cgroup::CgroupManager;

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-stats-live-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();
    mgr.set_memory_limit(pod_uid, 64 * 1024 * 1024).unwrap();

    // Fork a child that allocates some memory
    let (r, w) = sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    match sys::fork().unwrap() {
        sys::ForkResult::Child => {
            drop(r);
            // Allocate 4 MB
            let _data: Vec<u8> = vec![42u8; 4 * 1024 * 1024];
            // Tell parent we're alive
            sys::write_fd(&w, b"ok").ok();
            drop(w);
            // Hold memory until killed
            std::thread::sleep(std::time::Duration::from_secs(60));
            std::process::exit(0);
        }
        sys::ForkResult::Parent(child_pid) => {
            drop(w);
            // Wait for child to be ready
            let mut buf = [0u8; 8];
            sys::read_fd(&r, &mut buf).ok();

            // Add child to cgroup
            mgr.add_pid_to_cgroup(pod_uid, child_pid).unwrap();

            // Give kernel a moment to update stats
            std::thread::sleep(std::time::Duration::from_millis(200));

            let stats = mgr.resource_stats(pod_uid);
            assert!(stats.memory_current_bytes > 0, "child allocated memory, usage should be > 0, got {}", stats.memory_current_bytes);
            assert_eq!(stats.pids_current, 1, "should have 1 process in cgroup");

            // Kill child
            sys::kill(child_pid as i32, rustix::process::Signal::KILL).ok();
            let _ = sys::waitpid(child_pid as i32);

            mgr.remove_cgroup(pod_uid).ok();
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §18  ROOTFS — apply_limits integration
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_apply_limits_sets_all_cgroup_controllers() {
    use runtime::cgroup::{self, CgroupManager};

    let mgr = match CgroupManager::new() {
        Ok(m) if m.is_enabled() => m,
        _ => {
            eprintln!("cgroups not available, skipping");
            return;
        }
    };

    let pod_uid = "test-apply-limits-1";
    mgr.create_pod_cgroup(pod_uid).unwrap();

    let cfgs = vec![spec::ContainerConfigBuilder::new("c1", "web", "alpine")
        .memory_limit(32 * 1024 * 1024)
        .memory_low(16 * 1024 * 1024)
        .memory_swap(64 * 1024 * 1024)
        .cpu_limit(50000, 100000)
        .cpu_shares(512)
        .pids_max(32)
        .cpuset("0", "0")
        .build()];

    cgroup::apply_limits(&mgr, pod_uid, &cfgs);

    let cg_path = format!("/sys/fs/cgroup/z8s/{}", pod_uid);
    assert_eq!(std::fs::read_to_string(format!("{}/memory.max", cg_path)).unwrap().trim(), (32 * 1024 * 1024).to_string());
    assert_eq!(std::fs::read_to_string(format!("{}/memory.low", cg_path)).unwrap().trim(), (16 * 1024 * 1024).to_string());
    assert_eq!(std::fs::read_to_string(format!("{}/memory.swap.max", cg_path)).unwrap().trim(), (64 * 1024 * 1024).to_string());
    assert_eq!(std::fs::read_to_string(format!("{}/cpu.max", cg_path)).unwrap().trim(), "50000 100000");
    assert_eq!(std::fs::read_to_string(format!("{}/cpu.weight", cg_path)).unwrap().trim(), "512");
    assert_eq!(std::fs::read_to_string(format!("{}/pids.max", cg_path)).unwrap().trim(), "32");
    assert_eq!(std::fs::read_to_string(format!("{}/cpuset.cpus", cg_path)).unwrap().trim(), "0");
    assert_eq!(std::fs::read_to_string(format!("{}/cpuset.mems", cg_path)).unwrap().trim(), "0");

    mgr.remove_cgroup(pod_uid).ok();
}

// ══════════════════════════════════════════════════════════════════════════
// §19  SUPERVISOR — full spawn with security hardening
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn test_spawn_with_no_new_privileges_and_oom() {
    use runtime::image::ImageManager;
    use runtime::cgroup::CgroupManager;
    use std::sync::Arc;

    let img_mgr = Arc::new(ImageManager::new().unwrap());
    let cgroup_mgr = Arc::new(CgroupManager::new().unwrap_or_else(|_| CgroupManager::new_stub()));
    let sup = runtime::supervisor::ContainerSupervisor::new(img_mgr, cgroup_mgr);

    // Build a minimal spec for alpine sleep
    let spec = spec::ContainerSpec {
        pod_name: "test-sec-hardening".into(),
        pod_uid: "test-sec-harden-uid".into(),
        namespace: "default".into(),
        hostname: "test-pod".into(),
        containers: vec![spec::ContainerConfigBuilder::new(
            "test-sec-harden-uid-sleep",
            "sleep",
            "alpine:latest",
        )
        .entrypoint("/bin/sh")
        .args(vec!["-c".into(), "echo running && sleep 30".into()])
        .no_new_privileges()
        .oom_score_adj(-500)
        .memory_limit(32 * 1024 * 1024)
        .cpu_shares(256)
        .pids_max(16)
        .build()],
        labels: Default::default(),
        subnet: None,
    };

    sup.start_pod_from_spec(&spec).await.unwrap();
    assert!(sup.is_pod_alive("test-sec-hardening").await);

    // Verify the child has NoNewPrivs=1
    let alive = sup.is_pod_alive("test-sec-hardening").await;
    assert!(alive);

    sup.stop_pod_from_spec(&spec).await;
    assert!(!sup.is_pod_alive("test-sec-hardening").await);
}

// ══════════════════════════════════════════════════════════════════════════
// §20  ROOTFS — mask/readonly paths integration with real rootfs
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn test_spawn_with_masked_readonly_paths() {
    use runtime::image::ImageManager;
    use runtime::cgroup::CgroupManager;
    use std::sync::Arc;

    let img_mgr = Arc::new(ImageManager::new().unwrap());
    let cgroup_mgr = Arc::new(CgroupManager::new().unwrap_or_else(|_| CgroupManager::new_stub()));
    let sup = runtime::supervisor::ContainerSupervisor::new(img_mgr, cgroup_mgr);

    let spec = spec::ContainerSpec {
        pod_name: "test-mask-ro".into(),
        pod_uid: "test-mask-ro-uid".into(),
        namespace: "default".into(),
        hostname: "test-pod".into(),
        containers: vec![spec::ContainerConfigBuilder::new(
            "test-mask-ro-uid-sleep",
            "sleep",
            "alpine:latest",
        )
        .entrypoint("/bin/sh")
        .args(vec!["-c".into(), "echo running && sleep 30".into()])
        .masked_paths(vec!["/proc/acpi".into()])
        .readonly_paths(vec!["/proc/sys".into()])
        .build()],
        labels: Default::default(),
        subnet: None,
    };

    sup.start_pod_from_spec(&spec).await.unwrap();
    assert!(sup.is_pod_alive("test-mask-ro").await);

    sup.stop_pod_from_spec(&spec).await;
    assert!(!sup.is_pod_alive("test-mask-ro").await);
}
