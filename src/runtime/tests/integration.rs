//! # Real Integration Tests — Container Lifecycle with Real Images
//!
//! Tests pull actual container images and run real processes.
//! Requires network access for image pulls.
//!
//! Run: `cargo test --package runtime --test integration`
//! Ignored: `cargo test --package runtime --test integration -- --ignored`

use std::path::PathBuf;
use runtime::image::ImageManager;
use runtime::rootfs;
use runtime::supervisor;

fn test_base_dir() -> PathBuf {
    std::env::temp_dir().join("z8s_integration_test")
}

fn cleanup() {
    // Don't cleanup shared base dir - other tests may be using it
    // Each test cleans up its own subdirectory
}

// ── Image Pull + Unpack Tests ────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires network access"]
async fn test_pull_alpine() {
    cleanup();
    let mgr = ImageManager::new().unwrap();
    let container_id = "test-alpine-001";
    let rootfs_path = mgr.unpack_image("alpine:latest", container_id).await.unwrap();

    assert!(PathBuf::from(&rootfs_path).exists(), "rootfs should exist");
    // Alpine has busybox with symlinks
    assert!(
        PathBuf::from(&rootfs_path).join("bin/busybox").exists()
            || PathBuf::from(&rootfs_path).join("bin/sh").exists()
            || PathBuf::from(&rootfs_path).join("bin/ash").exists(),
        "alpine should have busybox or sh"
    );
    assert!(PathBuf::from(&rootfs_path).join("etc/alpine-release").exists(), "alpine should have /etc/alpine-release");

    println!("alpine rootfs: {}", rootfs_path);
    cleanup();
}

#[tokio::test]
#[ignore = "requires network access"]
async fn test_pull_ubuntu() {
    cleanup();
    let mgr = ImageManager::new().unwrap();
    let container_id = "test-ubuntu-001";
    let rootfs_path = mgr.unpack_image("ubuntu:latest", container_id).await.unwrap();

    assert!(PathBuf::from(&rootfs_path).exists(), "rootfs should exist");
    assert!(PathBuf::from(&rootfs_path).join("bin/sh").exists(), "ubuntu should have /bin/sh");
    assert!(PathBuf::from(&rootfs_path).join("etc/os-release").exists(), "ubuntu should have /etc/os-release");

    println!("ubuntu rootfs: {}", rootfs_path);
    cleanup();
}

#[tokio::test]
#[ignore = "requires network access"]
async fn test_pull_nginx() {
    cleanup();
    let mgr = ImageManager::new().unwrap();
    let container_id = "test-nginx-001";
    let rootfs_path = mgr.unpack_image("nginx:latest", container_id).await.unwrap();

    assert!(PathBuf::from(&rootfs_path).exists(), "rootfs should exist");
    // nginx image may have different paths
    let has_nginx = PathBuf::from(&rootfs_path).join("usr/sbin/nginx").exists()
        || PathBuf::from(&rootfs_path).join("sbin/nginx").exists()
        || PathBuf::from(&rootfs_path).join("etc/nginx").exists();
    assert!(has_nginx, "nginx image should contain nginx binary or config");

    println!("nginx rootfs: {}", rootfs_path);
    cleanup();
}

// ── Rootfs Preparation Tests ─────────────────────────────────────────────

#[test]
fn test_prepare_rootfs_creates_dirs() {
    let rootfs_dir = test_base_dir().join("prepare");
    let _ = std::fs::remove_dir_all(&rootfs_dir);
    std::fs::create_dir_all(&rootfs_dir).unwrap();

    // Verify dir exists before calling prepare_rootfs
    assert!(rootfs_dir.exists(), "rootfs_dir should exist before prepare_rootfs");

    let result = rootfs::prepare_rootfs(rootfs_dir.to_str().unwrap());
    if let Err(e) = &result {
        eprintln!("prepare_rootfs failed: {:?}", e);
        // Debug: check what dirs were created
        for entry in std::fs::read_dir(&rootfs_dir).unwrap().flatten() {
            eprintln!("  created: {:?}", entry.file_name());
        }
    }
    result.unwrap();

    // Check essential directories exist
    let dirs = ["proc", "dev", "tmp", "etc", "run"];
    for dir in &dirs {
        let path = rootfs_dir.join(dir);
        assert!(path.exists(), "directory {} should exist", dir);
    }

    // Check resolv.conf written
    let resolv_path = rootfs_dir.join("etc/resolv.conf");
    if resolv_path.exists() {
        let resolv = std::fs::read_to_string(&resolv_path).unwrap();
        assert!(resolv.contains("nameserver"));
    }

    cleanup();
}

#[test]
fn test_prepare_rootfs_nonexistent_fails() {
    let result = rootfs::prepare_rootfs("/nonexistent/path/xyz");
    assert!(result.is_err());
}

// ── Exec Path Resolution ─────────────────────────────────────────────────

#[test]
fn test_resolve_exec_path_absolute() {
    let path = rootfs::resolve_exec_path("/usr/bin/ls", "/var/lib/z8s/rootfs/abc");
    assert!(path.starts_with("/var/lib/z8s/rootfs/abc"));
    assert!(path.ends_with("/usr/bin/ls"));
}

#[test]
fn test_resolve_exec_path_relative() {
    let path = rootfs::resolve_exec_path("sh", "/var/lib/z8s/rootfs/abc");
    // Should find sh in one of the standard paths, or return the entrypoint as-is
    assert!(path.contains("sh") || path == "sh");
}

#[test]
fn test_host_path_in_container_root() {
    let result = rootfs::host_path_in_container_root(
        "/var/lib/z8s/rootfs/abc/usr/bin/ls",
        "/var/lib/z8s/rootfs/abc",
    );
    assert_eq!(result, "/usr/bin/ls");

    let result = rootfs::host_path_in_container_root(
        "/var/lib/z8s/rootfs/abc",
        "/var/lib/z8s/rootfs/abc",
    );
    assert_eq!(result, "/");
}

// ── Full Container Lifecycle with Real Image ──────────────────────────────

#[tokio::test]
#[ignore = "requires network access"]
async fn test_alpine_container_lifecycle() {
    cleanup();
    let mgr = ImageManager::new().unwrap();
    let rootfs_path = mgr.unpack_image("alpine:latest", "lifecycle-test").await.unwrap();

    // Verify rootfs is usable
    assert!(PathBuf::from(&rootfs_path).join("bin/busybox").exists());
    assert!(PathBuf::from(&rootfs_path).join("etc/alpine-release").exists());

    // Fork and exec inside the container rootfs
    let rootfs_owned = rootfs_path.clone();
    let mut child_exited = false;

    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            let _ = z8s_core::sys::chroot(&rootfs_owned);
            let _ = z8s_core::sys::chdir("/");
            let program = std::ffi::CString::new("/bin/sh").unwrap();
            let a0 = std::ffi::CString::new("/bin/sh").unwrap();
            let a1 = std::ffi::CString::new("-c").unwrap();
            let a2 = std::ffi::CString::new("echo alpine-ok").unwrap();
            let argv: &[*const std::ffi::c_char] = &[a0.as_ptr(), a1.as_ptr(), a2.as_ptr(), std::ptr::null()];
            let envp: &[*const std::ffi::c_char] = &[std::ptr::null()];
            let _ = z8s_core::sys::execve(&program, argv, envp);
            std::process::exit(1);
        }
        z8s_core::sys::ForkResult::Parent(child_pid) => {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(5) {
                if let Some((pid, _exit_code)) = z8s_core::sys::waitpid(-1)
                    && pid == child_pid
                {
                    child_exited = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    assert!(child_exited, "child should have exited");
    cleanup();
}

#[tokio::test]
#[ignore = "requires network access"]
async fn test_ubuntu_container_lifecycle() {
    cleanup();
    let mgr = ImageManager::new().unwrap();
    let rootfs_path = mgr.unpack_image("ubuntu:latest", "ubuntu-lifecycle").await.unwrap();

    assert!(PathBuf::from(&rootfs_path).exists());
    assert!(PathBuf::from(&rootfs_path).join("bin/sh").exists());

    let rootfs_owned = rootfs_path.clone();
    let mut child_exited = false;

    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            let _ = z8s_core::sys::chroot(&rootfs_owned);
            let _ = z8s_core::sys::chdir("/");
            let program = std::ffi::CString::new("/bin/sh").unwrap();
            let a0 = std::ffi::CString::new("/bin/sh").unwrap();
            let a1 = std::ffi::CString::new("-c").unwrap();
            let a2 = std::ffi::CString::new("echo ubuntu-ok").unwrap();
            let argv: &[*const std::ffi::c_char] = &[a0.as_ptr(), a1.as_ptr(), a2.as_ptr(), std::ptr::null()];
            let envp: &[*const std::ffi::c_char] = &[std::ptr::null()];
            let _ = z8s_core::sys::execve(&program, argv, envp);
            std::process::exit(1);
        }
        z8s_core::sys::ForkResult::Parent(child_pid) => {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(5) {
                if let Some((pid, _)) = z8s_core::sys::waitpid(-1)
                    && pid == child_pid
                {
                    child_exited = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    assert!(child_exited, "child should have exited");
    cleanup();
}

#[tokio::test]
#[ignore = "requires network access"]
async fn test_nginx_image_pulls() {
    cleanup();
    let mgr = ImageManager::new().unwrap();
    let rootfs_path = mgr.unpack_image("nginx:latest", "nginx-lifecycle").await.unwrap();

    assert!(PathBuf::from(&rootfs_path).exists());
    // nginx should have its binary or config
    let has_content = PathBuf::from(&rootfs_path).join("etc/nginx").exists()
        || PathBuf::from(&rootfs_path).join("usr/sbin/nginx").exists()
        || PathBuf::from(&rootfs_path).join("sbin/nginx").exists();
    assert!(has_content, "nginx image should have config or binary");

    println!("nginx rootfs: {}", rootfs_path);
    cleanup();
}

// ── Environment Merge Tests ──────────────────────────────────────────────

#[test]
fn test_merge_env_with_real_rootfs() {
    // Create a mock rootfs with OCI config
    let rootfs_dir = test_base_dir().join("merge_test");
    let _ = std::fs::remove_dir_all(&rootfs_dir);
    std::fs::create_dir_all(&rootfs_dir).unwrap();

    let config = runtime::image::SavedImageConfig {
        entrypoint: Some(vec!["/bin/sh".into()]),
        env: Some(vec!["PATH=/usr/local/bin:/usr/bin:/bin".into(), "OCI_VAR=from-image".into()]),
        ..Default::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    std::fs::write(rootfs_dir.join(runtime::image::OCI_CONFIG_FILE), json).unwrap();

    // Merge spec env with OCI env
    let spec_env = vec![
        ("SPEC_VAR".into(), "from-spec".into()),
        ("OCI_VAR".into(), "overridden".into()), // Should override OCI
    ];

    let merged = supervisor::merge_env(&spec_env, rootfs_dir.to_str().unwrap());

    // Spec takes precedence
    let oci_var = merged.iter().find(|(k, _)| k == "OCI_VAR").unwrap();
    assert_eq!(oci_var.1, "overridden");

    // Spec-only var
    let spec_var = merged.iter().find(|(k, _)| k == "SPEC_VAR").unwrap();
    assert_eq!(spec_var.1, "from-spec");

    // OCI PATH should be included
    let path = merged.iter().find(|(k, _)| k == "PATH").unwrap();
    assert!(path.1.contains("/usr/local/bin"));

    cleanup();
}

// ── Cgroup Tests ─────────────────────────────────────────────────────────

#[test]
#[ignore = "requires root for cgroup creation"]
fn test_cgroup_lifecycle_with_container() {
    if !runtime::rootfs::is_root() {
        eprintln!("skipping: not root");
        return;
    }
    let cgroup = runtime::cgroup::CgroupManager::new().unwrap();
    let test_uid = "test-cgroup-container";

    let path = cgroup.create_pod_cgroup(test_uid).unwrap();
    assert!(PathBuf::from(&path).exists());

    // Set limits
    cgroup.set_memory_limit(test_uid, 256 * 1024 * 1024).unwrap();
    cgroup.set_cpu_limit(test_uid, 50000, 100000).unwrap();

    // Verify
    let mem = std::fs::read_to_string(format!("{}/memory.max", path)).unwrap();
    assert_eq!(mem.trim(), "268435456");

    let cpu = std::fs::read_to_string(format!("{}/cpu.max", path)).unwrap();
    assert_eq!(cpu.trim(), "50000 100000");

    cgroup.remove_cgroup(test_uid).unwrap();
}

// ── Probe Tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_exec_probe_ubuntu() {
    let status = runtime::health::HealthChecker::check_exec(
        &["/bin/true".into()],
        std::time::Duration::from_secs(5),
    ).await;
    assert_eq!(status, runtime::health::HealthStatus::Healthy);
}

#[tokio::test]
async fn test_exec_probe_fails() {
    let status = runtime::health::HealthChecker::check_exec(
        &["/bin/false".into()],
        std::time::Duration::from_secs(5),
    ).await;
    assert_eq!(status, runtime::health::HealthStatus::Unhealthy);
}

#[tokio::test]
async fn test_exec_probe_timeout() {
    let status = runtime::health::HealthChecker::check_exec(
        &["sleep".into(), "10".into()],
        std::time::Duration::from_millis(100),
    ).await;
    assert_eq!(status, runtime::health::HealthStatus::Unhealthy);
}

#[tokio::test]
async fn test_tcp_probe_refused() {
    let status = runtime::health::HealthChecker::check_tcp(
        &runtime::health::TcpProbe { host: None, port: 19999 },
        std::time::Duration::from_millis(200),
    ).await;
    assert_eq!(status, runtime::health::HealthStatus::Unhealthy);
}
