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
