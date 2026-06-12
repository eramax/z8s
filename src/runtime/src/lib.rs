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
}

