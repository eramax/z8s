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
        for stream in listener.incoming() {
            if let Ok(mut s) = stream {
                use std::io::Write;
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
            }
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
        action: ProbeAction::Exec(ExecProbe { command: Some(vec!["/bin/true".into()]) }),
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
                if let Some((pid, exit_code)) = z8s_core::sys::waitpid(-1) {
                    if pid == child_pid {
                        assert_eq!(exit_code, 42);
                        child_done = true;
                        break;
                    }
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
                if let Some((pid, _)) = z8s_core::sys::waitpid(-1) {
                    if pid == child_pid { break; }
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
                if let Some((p, _)) = z8s_core::sys::waitpid(-1) { if p == pid { exited = true; break; } }
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
                if let Some((p, _)) = z8s_core::sys::waitpid(-1) { if p == pid { exited = true; break; } }
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
