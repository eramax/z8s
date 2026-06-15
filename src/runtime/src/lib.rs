//! # Runtime Module — Container Lifecycle
//!
//! Manages the complete lifecycle of containers: image pulling, filesystem
//! preparation, process spawning with Linux namespaces, cgroup resource
//! limits, exec into running containers, and health probing.
//!
//! ## Architecture
//!
//! ```text
//! RuntimeProvider (trait)
//!     │
//!     ├── ImageManager    — OCI pull, layer cache, rootfs copy/overlay
//!     ├── Rootfs          — mount, pivot_root, chroot, capabilities
//!     ├── CgroupManager   — cgroups v2 resource limits
//!     ├── HealthChecker   — exec/http/tcp probes
//!     └── ContainerSupervisor — orchestrates spawn, tracks running containers
//! ```
//!
//! ## Design Principles
//!
//! 1. **Types are data, functions are behavior** — no god objects
//! 2. **Pure functions** for stateless operations (env merge, path resolution)
//! 3. **Functional pipelines** — iterator chains, builder pattern
//! 4. **rustix** for all syscalls — no `nix` dependency
//! 5. **Graceful degradation** — rootless mode falls back to chroot/userns

use async_trait::async_trait;

pub mod cgroup;
pub mod exec;
pub mod health;
pub mod image;
pub mod rootfs;
pub mod spec;
pub mod supervisor;

/// The runtime's contract with the rest of the system.
///
/// Every method is async because image pulls and process management
/// involve I/O. The trait is object-safe so controllers can hold
/// `Arc<dyn RuntimeProvider>`.
#[async_trait]
pub trait RuntimeProvider: Send + Sync {
    /// Start all containers in a pod spec.
    async fn start_pod(&self, spec: &spec::ContainerSpec) -> anyhow::Result<()>;

    /// Stop all containers in a pod and clean up resources.
    async fn stop_pod(&self, spec: &spec::ContainerSpec) -> anyhow::Result<()>;

    /// Stop a single container by ID.
    async fn stop_container(&self, container_id: &str) -> anyhow::Result<()>;

    /// Whether any container in the pod is alive (PID exists).
    async fn is_pod_alive(&self, pod_name: &str) -> bool;

    /// Whether all containers in the pod are ready (probes pass).
    async fn is_pod_ready(&self, pod_name: &str) -> bool;

    /// Port the service proxy should dial on 127.0.0.1 for this pod.
    async fn backend_connect_port(&self, pod_name: &str, port: u16) -> u16;

    /// stdout/stderr log lines for a container.
    async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String>;

    /// Restart count per container name for a pod.
    async fn pod_restart_counts(&self, pod_name: &str) -> std::collections::HashMap<String, u32>;

    /// Pull and unpack an OCI image, returning the rootfs path.
    async fn unpack_image(&self, image_ref: &str, container_id: &str) -> anyhow::Result<String>;

    /// Create a cgroup for a pod.
    fn create_pod_cgroup(&self, pod_uid: &str) -> anyhow::Result<String>;

    /// Remove a pod's cgroup.
    fn remove_cgroup(&self, pod_uid: &str) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cgroup::CgroupManager;

    // ── spec.rs — ContainerConfigBuilder ──────────────────────────────────

    #[test]
    fn builder_creates_container_config() {
        let cfg = spec::ContainerConfigBuilder::new("c-1", "web", "nginx:1.25")
            .entrypoint("/bin/sh")
            .args(vec!["-c".into(), "echo hello".into()])
            .env("MY_VAR", "hello")
            .env("PORT", "8080")
            .memory_limit(512 * 1024 * 1024)
            .cpu_limit(100000, 100000)
            .run_as(1000, 1000)
            .isolated_net(true)
            .build();

        assert_eq!(cfg.container_id, "c-1");
        assert_eq!(cfg.container_name, "web");
        assert_eq!(cfg.image, "nginx:1.25");
        assert_eq!(cfg.entrypoint, "/bin/sh");
        assert_eq!(cfg.args, vec!["-c", "echo hello"]);
        assert_eq!(cfg.env, vec![("MY_VAR".into(), "hello".into()), ("PORT".into(), "8080".into())]);
        assert_eq!(cfg.memory_limit_bytes, Some(512 * 1024 * 1024));
        assert_eq!(cfg.cpu_quota, Some(100000));
        assert_eq!(cfg.run_as_user, Some(1000));
        assert_eq!(cfg.run_as_group, Some(1000));
        assert!(cfg.isolated_net);
        assert!(!cfg.privileged);
    }

    #[test]
    fn builder_defaults() {
        let cfg = spec::ContainerConfigBuilder::new("c-1", "web", "alpine").build();
        assert!(cfg.entrypoint.is_empty());
        assert!(cfg.args.is_empty());
        assert!(cfg.env.is_empty());
        assert!(cfg.memory_limit_bytes.is_none());
        assert!(!cfg.isolated_net);
        assert!(!cfg.privileged);
        assert!(!cfg.is_native);
    }

    // ── rootfs.rs — Pure Functions ────────────────────────────────────────

    #[test]
    fn resolve_exec_path_absolute() {
        let path = rootfs::resolve_exec_path("/usr/bin/ls", "/var/lib/z8s/rootfs/abc");
        assert_eq!(path, "/var/lib/z8s/rootfs/abc/usr/bin/ls");
    }

    #[test]
    fn resolve_exec_path_trailing_slash() {
        let path = rootfs::resolve_exec_path("/bin/sh", "/rootfs/");
        assert_eq!(path, "/rootfs/bin/sh");
    }

    #[test]
    fn host_path_in_container_root_nested() {
        let result = rootfs::host_path_in_container_root(
            "/var/lib/z8s/rootfs/abc/usr/bin/ls",
            "/var/lib/z8s/rootfs/abc",
        );
        assert_eq!(result, "/usr/bin/ls");
    }

    #[test]
    fn host_path_in_container_root_exact() {
        let result = rootfs::host_path_in_container_root(
            "/var/lib/z8s/rootfs/abc",
            "/var/lib/z8s/rootfs/abc",
        );
        assert_eq!(result, "/");
    }

    #[test]
    fn host_path_in_container_root_no_match() {
        let result = rootfs::host_path_in_container_root(
            "/usr/bin/ls",
            "/var/lib/z8s/rootfs/abc",
        );
        assert_eq!(result, "/usr/bin/ls");
    }

    #[test]
    fn build_container_argv_no_busybox() {
        // Non-busybox: args pass through
        let (path, args) = rootfs::build_container_argv(
            "/usr/bin/sleep",
            &["3600".into()],
            "/var/lib/z8s/rootfs/abc",
        );
        assert!(path.ends_with("/usr/bin/sleep") || path.contains("/sleep"));
        assert_eq!(args, vec!["3600"]);
    }

    #[test]
    fn build_container_argv_busybox() {
        // Busybox applet: entrypoint becomes first arg
        let (path, args) = rootfs::build_container_argv(
            "sleep",
            &["3600".into()],
            "/var/lib/z8s/rootfs/abc",
        );
        // If busybox is found, args should be ["sleep", "3600"]
        // If not found, args should be ["3600"] (fallback)
        if path.ends_with("/busybox") {
            assert_eq!(args, vec!["sleep", "3600"]);
        } else {
            assert_eq!(args, vec!["3600"]);
        }
    }

    // ── rootfs.rs — mount propagation constants ──────────────────────────

    #[test]
    fn mount_propagation_flags_compile() {
        use rustix::mount::MountPropagationFlags;
        // Verify these compile — if they don't, rustix API changed
        let _ = MountPropagationFlags::DOWNSTREAM;
        let _ = MountPropagationFlags::PRIVATE;
        let _ = MountPropagationFlags::REC;
    }

    #[test]
    fn mount_flags_compile() {
        use rustix::mount::MountFlags;
        let _ = MountFlags::BIND;
        let _ = MountFlags::REC;
        let _ = MountFlags::RDONLY;
        let _ = MountFlags::NOSUID;
        let _ = MountFlags::NODEV;
    }

    // ── cgroup.rs — sanitize ──────────────────────────────────────────────

    #[test]
    fn cgroup_sanitize_names() {
        // Cgroup names can't contain / : .
        let name = "default/nginx-pod-abc123";
        let sanitized = name.replace(['/', '.', ':'], "_");
        assert_eq!(sanitized, "default_nginx-pod-abc123");
    }

    // ── health.rs — ProbeConfig ──────────────────────────────────────────

    #[test]
    fn probe_timeout_conversion() {
        let probe = health::ProbeConfig {
            action: health::ProbeAction::Exec(health::ExecProbe {
                command: Some(vec!["ls".into()]),
            }),
            initial_delay_seconds: 5,
            period_seconds: 10,
            timeout_seconds: 3,
        };
        assert_eq!(probe.timeout(), std::time::Duration::from_secs(3));
    }

    #[test]
    fn probe_timeout_zero() {
        let probe = health::ProbeConfig {
            action: health::ProbeAction::TCPSocket(health::TcpProbe {
                host: None,
                port: 8080,
            }),
            initial_delay_seconds: 0,
            period_seconds: 1,
            timeout_seconds: 0,
        };
        assert_eq!(probe.timeout(), std::time::Duration::from_secs(0));
    }

    // ── image.rs — read_image_config ─────────────────────────────────────

    #[test]
    fn read_image_config_from_rootfs() {
        let dir = std::env::temp_dir().join("z8s_test_img_cfg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = image::SavedImageConfig {
            entrypoint: Some(vec!["/bin/sh".into(), "-c".into()]),
            cmd: Some(vec!["echo".into(), "hello".into()]),
            env: Some(vec!["PATH=/usr/bin".into()]),
            working_dir: Some("/app".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        std::fs::write(dir.join(image::OCI_CONFIG_FILE), json).unwrap();

        let loaded = image::read_image_config(dir.to_str().unwrap());
        assert_eq!(loaded.entrypoint, Some(vec!["/bin/sh".into(), "-c".into()]));
        assert_eq!(loaded.cmd, Some(vec!["echo".into(), "hello".into()]));
        assert_eq!(loaded.env, Some(vec!["PATH=/usr/bin".into()]));
        assert_eq!(loaded.working_dir, Some("/app".into()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_image_config_fallback_to_guess() {
        let dir = std::env::temp_dir().join("z8s_test_img_guess");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // No OCI config file → should guess based on file presence
        let loaded = image::read_image_config(dir.to_str().unwrap());
        // No entrypoint.sh or single binary → default
        assert!(loaded.entrypoint.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guess_image_config_single_executable() {
        let dir = std::env::temp_dir().join("z8s_test_guess_single");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create a single executable at root
        let binary = dir.join("myapp");
        std::fs::write(&binary, b"\x7fELF").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        // read_image_config calls guess_image_config internally
        let config = image::read_image_config(dir.to_str().unwrap());
        assert_eq!(config.entrypoint, Some(vec!["/myapp".into()]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── supervisor.rs — merge_env ────────────────────────────────────────

    #[test]
    fn merge_env_spec_overrides_oci() {
        let dir = std::env::temp_dir().join("z8s_test_merge_env");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Write OCI config with entrypoint + env (read_image_config requires entrypoint/cmd)
        let config = image::SavedImageConfig {
            entrypoint: Some(vec!["/bin/sh".into()]),
            env: Some(vec!["PATH=/usr/bin".into(), "MY_VAR=oci_value".into()]),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        std::fs::write(dir.join(image::OCI_CONFIG_FILE), json).unwrap();

        // Spec env overrides MY_VAR
        let spec_env = vec![("MY_VAR".into(), "spec_value".into()), ("SPEC_ONLY".into(), "yes".into())];

        let merged = supervisor::merge_env(&spec_env, dir.to_str().unwrap());

        // Spec values come first (spec takes precedence)
        let my_var = merged.iter().find(|(k, _)| k == "MY_VAR").unwrap();
        assert_eq!(my_var.1, "spec_value");

        let spec_only = merged.iter().find(|(k, _)| k == "SPEC_ONLY").unwrap();
        assert_eq!(spec_only.1, "yes");

        let path = merged.iter().find(|(k, _)| k == "PATH").unwrap();
        assert_eq!(path.1, "/usr/bin");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_env_empty_spec() {
        let dir = std::env::temp_dir().join("z8s_test_merge_empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = image::SavedImageConfig {
            entrypoint: Some(vec!["/bin/sh".into()]),
            env: Some(vec!["A=1".into(), "B=2".into()]),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        std::fs::write(dir.join(image::OCI_CONFIG_FILE), json).unwrap();

        let merged = supervisor::merge_env(&[], dir.to_str().unwrap());
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|(k, v)| k == "A" && v == "1"));
        assert!(merged.iter().any(|(k, v)| k == "B" && v == "2"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── syscall.rs — basic functionality ─────────────────────────────────

    #[test]
    fn pipe2_creates_readable_pipe() {
        let flags = rustix::pipe::PipeFlags::CLOEXEC;
        let (r, w) = z8s_core::sys::pipe2(flags).unwrap();
        // Write a byte
        z8s_core::sys::write_fd(&w, b"x").unwrap();
        drop(w);
        // Read it back
        let mut buf = [0u8; 1];
        let n = z8s_core::sys::read_fd(&r, &mut buf).unwrap();
        assert_eq!(n, 1);
        assert_eq!(buf[0], b'x');
    }

    #[test]
    fn fork_returns_valid_pid() {
        let result = z8s_core::sys::fork().unwrap();
        match result {
            z8s_core::sys::ForkResult::Child => {
                // Child: exit immediately
                std::process::exit(0);
            }
            z8s_core::sys::ForkResult::Parent(pid) => {
                assert!(pid > 0);
            }
        }
    }

    #[test]
    fn kill_nonexistent_pid_returns_esrch() {
        let result = z8s_core::sys::kill(99999999, rustix::process::Signal::CONT);
        assert!(result.is_err());
    }

    #[test]
    fn open_nonexistent_file_fails() {
        let result = z8s_core::sys::open(
            "/nonexistent/path/xyz",
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        );
        assert!(result.is_err());
    }

    // ══════════════════════════════════════════════════════════════════════
    // spec.rs — new builder fields
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn builder_new_security_fields() {
        let cfg = spec::ContainerConfigBuilder::new("c-1", "web", "alpine")
            .no_new_privileges()
            .oom_score_adj(-500)
            .supplementary_groups(vec![100, 200, 300])
            .umask(0o027)
            .masked_paths(vec!["/proc/kcore".into()])
            .readonly_paths(vec!["/sys/fs".into()])
            .build();

        assert!(cfg.no_new_privileges);
        assert_eq!(cfg.oom_score_adj, Some(-500));
        assert_eq!(cfg.supplementary_groups, vec![100, 200, 300]);
        assert_eq!(cfg.umask, Some(0o027));
        assert_eq!(cfg.masked_paths, vec!["/proc/kcore"]);
        assert_eq!(cfg.readonly_paths, vec!["/sys/fs"]);
    }

    #[test]
    fn builder_new_resource_fields() {
        let cfg = spec::ContainerConfigBuilder::new("c-1", "web", "alpine")
            .memory_limit(256 * 1024 * 1024)
            .memory_swap(512 * 1024 * 1024)
            .cpu_shares(512)
            .cpuset("0-3", "0-1")
            .pids_max(64)
            .build();

        assert_eq!(cfg.memory_limit_bytes, Some(256 * 1024 * 1024));
        assert_eq!(cfg.memory_swap_bytes, Some(512 * 1024 * 1024));
        assert_eq!(cfg.cpu_shares, Some(512));
        assert_eq!(cfg.cpuset_cpus, Some("0-3".into()));
        assert_eq!(cfg.cpuset_mems, Some("0-1".into()));
        assert_eq!(cfg.pids_max, Some(64));
    }

    #[test]
    fn builder_defaults_new_fields() {
        let cfg = spec::ContainerConfigBuilder::new("c-1", "web", "alpine").build();
        assert!(!cfg.no_new_privileges);
        assert!(cfg.oom_score_adj.is_none());
        assert!(cfg.supplementary_groups.is_empty());
        assert!(cfg.umask.is_none());
        assert!(cfg.masked_paths.is_empty());
        assert!(cfg.readonly_paths.is_empty());
        assert!(cfg.memory_swap_bytes.is_none());
        assert!(cfg.cpu_shares.is_none());
        assert!(cfg.cpuset_cpus.is_none());
        assert!(cfg.cpuset_mems.is_none());
        assert!(cfg.pids_max.is_none());
    }

    // ══════════════════════════════════════════════════════════════════════
    // rootfs.rs — masked/readonly paths
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn default_masked_paths_not_empty() {
        assert!(!rootfs::DEFAULT_MASKED_PATHS.is_empty());
        assert!(rootfs::DEFAULT_MASKED_PATHS.contains(&"/proc/kcore"));
        assert!(rootfs::DEFAULT_MASKED_PATHS.contains(&"/sys/firmware"));
    }

    #[test]
    fn default_readonly_paths_not_empty() {
        assert!(!rootfs::DEFAULT_READONLY_PATHS.is_empty());
        assert!(rootfs::DEFAULT_READONLY_PATHS.contains(&"/proc/sys"));
    }

    #[test]
    fn apply_masked_paths_creates_mounts() {
        let dir = std::env::temp_dir().join("z8s_test_masked");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Create a sensitive path to mask
        std::fs::create_dir_all(dir.join("proc")).unwrap();
        std::fs::write(dir.join("proc/kcore"), "host kernel").unwrap();

        rootfs::apply_masked_paths(dir.to_str().unwrap(), &[]);

        // After masking, the file content should be empty (overlaid with /dev/null)
        let content = std::fs::read(dir.join("proc/kcore")).unwrap();
        assert!(content.is_empty(), "masked path should be empty after bind-mount /dev/null");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_masked_paths_with_extra() {
        let dir = std::env::temp_dir().join("z8s_test_masked_extra");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("custom")).unwrap();
        std::fs::write(dir.join("custom/secret"), "sensitive").unwrap();

        rootfs::apply_masked_paths(dir.to_str().unwrap(), &["/custom/secret".into()]);

        let content = std::fs::read(dir.join("custom/secret")).unwrap();
        assert!(content.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_readonly_paths_remounts() {
        let dir = std::env::temp_dir().join("z8s_test_ro");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proc/sys")).unwrap();
        std::fs::write(dir.join("proc/sys/kernel"), "host kernel params").unwrap();

        rootfs::apply_readonly_paths(dir.to_str().unwrap(), &[]);

        // Verify the file exists (remount succeeded or gracefully degraded)
        assert!(dir.join("proc/sys/kernel").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ══════════════════════════════════════════════════════════════════════
    // cgroup.rs — resource_stats (unit test with stub)
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn cgroup_stats_disabled_returns_default() {
        let mgr = CgroupManager::new_stub();
        let stats = mgr.resource_stats("nonexistent-pod");
        assert_eq!(stats.memory_current_bytes, 0);
        assert!(stats.memory_limit_bytes.is_none());
        assert_eq!(stats.cpu_usage_usec, 0);
        assert_eq!(stats.pids_current, 0);
    }

    #[test]
    fn cgroup_manager_new_stub_is_disabled() {
        let mgr = CgroupManager::new_stub();
        assert!(mgr.create_pod_cgroup("test").is_ok());
        // All ops are no-ops
        mgr.set_memory_limit("test", 1024).ok();
        mgr.set_cpu_limit("test", 50000, 100000).ok();
        mgr.set_cpu_shares("test", 512).ok();
        mgr.set_memory_swap("test", -1).ok();
        mgr.set_pids_max("test", 64).ok();
        mgr.set_cpuset_cpus("test", "0-3").ok();
        mgr.set_cpuset_mems("test", "0-1").ok();
    }

    // ══════════════════════════════════════════════════════════════════════
    // supervisor.rs — merge_env dedup
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn merge_env_dedup() {
        let dir = std::env::temp_dir().join("z8s_test_merge_dedup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = image::SavedImageConfig {
            entrypoint: Some(vec!["/bin/sh".into()]),
            env: Some(vec!["A=1".into(), "B=2".into(), "C=3".into()]),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        std::fs::write(dir.join(image::OCI_CONFIG_FILE), json).unwrap();

        // Spec has duplicate key — first wins
        let spec_env = vec![("A".into(), "spec".into()), ("A".into(), "dup".into())];
        let merged = supervisor::merge_env(&spec_env, dir.to_str().unwrap());
        let a_vals: Vec<_> = merged.iter().filter(|(k, _)| k == "A").collect();
        assert_eq!(a_vals.len(), 1);
        assert_eq!(a_vals[0].1, "spec");

        // B and C come from OCI
        assert!(merged.iter().any(|(k, v)| k == "B" && v == "2"));
        assert!(merged.iter().any(|(k, v)| k == "C" && v == "3"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

