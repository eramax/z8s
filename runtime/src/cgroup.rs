//! # Cgroups v2 — Resource Limits for Containers
//!
//! Manages cgroups v2 hierarchy for pod resource isolation.
//! Each pod gets a cgroup under `/sys/fs/cgroup/z8s/<pod_uid>`.
//!
//! ## Features
//!
//! - Memory limits (`memory.max`, `memory.low`)
//! - CPU limits (`cpu.max`)
//! - PID limits (`pids.max` — set to unlimited for daemon pods)
//! - Process tracking (`cgroup.procs`)
//!
//! ## Graceful Degradation
//!
//! If cgroups are unavailable (non-root, containerized host), all operations
//! become no-ops. The container still runs, just without resource limits.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

const CGROUP_DIR: &str = "/sys/fs/cgroup";

/// Manages the z8s cgroup subtree.
pub struct CgroupManager {
    base_path: String,
    enabled: bool,
}

impl CgroupManager {
    /// Create a new cgroup manager. Enables pids, memory, and cpu controllers.
    pub fn new() -> Result<Self> {
        let base_path = format!("{}/z8s", CGROUP_DIR);
        if !Path::new(CGROUP_DIR).exists() {
            warn!("cgroups v2 not found — running without resource limits");
            return Ok(Self { base_path, enabled: false });
        }
        match fs::create_dir_all(&base_path) {
            Ok(()) => {
                let subtree = format!("{}/cgroup.subtree_control", base_path);
                for ctrl in ["+pids", "+memory", "+cpu"] {
                    if let Err(e) = fs::write(&subtree, ctrl) {
                        debug!("Could not enable controller {} ({}): {}", ctrl, subtree, e);
                    }
                }
                Ok(Self { base_path, enabled: true })
            }
            Err(e) => {
                warn!("Cannot create cgroup dir (need root?): {} — running without limits", e);
                Ok(Self { base_path, enabled: false })
            }
        }
    }

    /// Create a stub manager for degraded mode (all ops are no-ops).
    pub fn new_stub() -> Self {
        Self {
            base_path: String::new(),
            enabled: false,
        }
    }

    /// Create a cgroup for a pod. Returns the cgroup path.
    pub fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String> {
        if !self.enabled {
            return Ok(String::new());
        }
        let cg_path = format!("{}/{}", self.base_path, sanitize(pod_uid));
        match fs::create_dir_all(&cg_path) {
            Ok(()) => info!("Created cgroup: {}", cg_path),
            Err(e) => {
                warn!("Cannot create pod cgroup: {} — continuing without cgroup", e);
                return Ok(String::new());
            }
        }
        // Lift pids limit so multi-process daemons can fork freely.
        if let Err(e) = fs::write(format!("{}/pids.max", cg_path), "max") {
            debug!("Could not set pids.max: {}", e);
        }
        Ok(cg_path)
    }

    /// Set memory limit in bytes.
    pub fn set_memory_limit(&self, pod_uid: &str, limit_bytes: i64) -> Result<()> {
        if !self.enabled || limit_bytes <= 0 {
            return Ok(());
        }
        let path = format!("{}/{}/memory.max", self.base_path, sanitize(pod_uid));
        fs::write(&path, limit_bytes.to_string())
            .context(format!("Failed to set memory.max at {}", path))?;
        debug!("Set memory limit {} bytes for {}", limit_bytes, pod_uid);
        Ok(())
    }

    /// Set memory protection (soft limit) in bytes.
    pub fn set_memory_low(&self, pod_uid: &str, limit_bytes: i64) -> Result<()> {
        if !self.enabled || limit_bytes <= 0 {
            return Ok(());
        }
        let path = format!("{}/{}/memory.low", self.base_path, sanitize(pod_uid));
        fs::write(&path, limit_bytes.to_string())
            .context(format!("Failed to set memory.low at {}", path))?;
        Ok(())
    }

    /// Set CPU quota/period limit.
    pub fn set_cpu_limit(&self, pod_uid: &str, quota: i64, period: i64) -> Result<()> {
        if !self.enabled || quota <= 0 || period <= 0 {
            return Ok(());
        }
        let path = format!("{}/{}/cpu.max", self.base_path, sanitize(pod_uid));
        fs::write(&path, format!("{} {}", quota, period))
            .context(format!("Failed to set cpu.max at {}", path))?;
        debug!("Set CPU quota {}/{} for {}", quota, period, pod_uid);
        Ok(())
    }

    /// Add a process to a pod's cgroup.
    pub fn add_pid_to_cgroup(&self, pod_uid: &str, pid: u32) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let path = format!("{}/{}/cgroup.procs", self.base_path, sanitize(pod_uid));
        if let Err(e) = fs::write(&path, pid.to_string()) {
            warn!("Cannot write pid {} to cgroup: {} — continuing", pid, e);
        } else {
            debug!("Added pid {} to cgroup {}", pid, pod_uid);
        }
        Ok(())
    }

    /// Remove a pod's cgroup. Uses subprocess to avoid D-state.
    pub fn remove_cgroup(&self, pod_uid: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let cg_path = format!("{}/{}", self.base_path, sanitize(pod_uid));
        let _ = std::process::Command::new("rmdir").arg(&cg_path).spawn();
        Ok(())
    }
}

/// Sanitize a UID for use as a cgroup name.
fn sanitize(name: &str) -> String {
    name.replace(['/', '.', ':'], "_")
}

// ── Pure Functions ─────────────────────────────────────────────────────────

/// Apply resource limits from a container config to its pod cgroup.
pub fn apply_limits(
    cgroup: &CgroupManager,
    pod_uid: &str,
    configs: &[super::spec::ContainerConfig],
) {
    for cfg in configs {
        if let Some(limit) = cfg.memory_limit_bytes.filter(|&n| n > 0) {
            cgroup.set_memory_limit(pod_uid, limit).ok();
        }
        if let Some(low) = cfg.memory_low_bytes.filter(|&n| n > 0) {
            cgroup.set_memory_low(pod_uid, low).ok();
        }
        match (cfg.cpu_quota, cfg.cpu_period) {
            (Some(quota), Some(period)) if quota > 0 && period > 0 => {
                cgroup.set_cpu_limit(pod_uid, quota, period).ok();
            }
            _ => {}
        }
    }
}
