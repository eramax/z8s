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

/// Resource usage statistics from a live cgroup.
#[derive(Debug, Clone, Default)]
pub struct ResourceStats {
    pub memory_current_bytes: u64,
    pub memory_limit_bytes: Option<u64>,
    pub memory_swap_bytes: Option<u64>,
    pub cpu_usage_usec: u64,
    pub pids_current: u64,
    pub pids_limit: Option<u64>,
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

    /// Whether cgroups are available and enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
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

    /// Set CPU weight (1–10000, default 100). Maps to cpu.weight.
    pub fn set_cpu_shares(&self, pod_uid: &str, shares: u64) -> Result<()> {
        if !self.enabled || shares == 0 {
            return Ok(());
        }
        let clamped = shares.clamp(1, 10000);
        let path = format!("{}/{}/cpu.weight", self.base_path, sanitize(pod_uid));
        fs::write(&path, clamped.to_string())
            .context(format!("Failed to set cpu.weight at {}", path))?;
        Ok(())
    }

    /// Set memory + swap combined limit. -1 = unlimited swap.
    pub fn set_memory_swap(&self, pod_uid: &str, limit_bytes: i64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let path = format!("{}/{}/memory.swap.max", self.base_path, sanitize(pod_uid));
        let val = if limit_bytes < 0 { "max".to_string() } else { limit_bytes.to_string() };
        fs::write(&path, &val)
            .context(format!("Failed to set memory.swap.max at {}", path))?;
        Ok(())
    }

    /// Set max number of processes/threads. 0 or max = unlimited.
    pub fn set_pids_max(&self, pod_uid: &str, max: u64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let path = format!("{}/{}/pids.max", self.base_path, sanitize(pod_uid));
        let val = if max == 0 { "max".to_string() } else { max.to_string() };
        fs::write(&path, &val)
            .context(format!("Failed to set pids.max at {}", path))?;
        Ok(())
    }

    /// Pin to specific CPUs. Format: "0-3,6".
    pub fn set_cpuset_cpus(&self, pod_uid: &str, cpus: &str) -> Result<()> {
        if !self.enabled || cpus.is_empty() {
            return Ok(());
        }
        let path = format!("{}/{}/cpuset.cpus", self.base_path, sanitize(pod_uid));
        fs::write(&path, cpus)
            .context(format!("Failed to set cpuset.cpus at {}", path))?;
        Ok(())
    }

    /// Pin to specific memory nodes. Format: "0-1".
    pub fn set_cpuset_mems(&self, pod_uid: &str, mems: &str) -> Result<()> {
        if !self.enabled || mems.is_empty() {
            return Ok(());
        }
        let path = format!("{}/{}/cpuset.mems", self.base_path, sanitize(pod_uid));
        fs::write(&path, mems)
            .context(format!("Failed to set cpuset.mems at {}", path))?;
        Ok(())
    }

    /// Read live resource usage from a cgroup.
    pub fn resource_stats(&self, pod_uid: &str) -> ResourceStats {
        if !self.enabled {
            return ResourceStats::default();
        }
        let dir = format!("{}/{}", self.base_path, sanitize(pod_uid));
        let read_val = |file: &str| -> Option<String> {
            fs::read_to_string(format!("{}/{}", dir, file)).ok()
        };
        let parse_u64 = |s: Option<String>| -> Option<u64> {
            s.and_then(|v| v.trim().parse::<u64>().ok())
        };

        let mut stats = ResourceStats {
            memory_current_bytes: parse_u64(read_val("memory.current")).unwrap_or(0),
            memory_limit_bytes: read_val("memory.max").and_then(|v| {
                let t = v.trim();
                if t == "max" || t == "-1" { None } else { t.parse().ok() }
            }),
            memory_swap_bytes: read_val("memory.swap.max").and_then(|v| {
                let t = v.trim();
                if t == "max" || t == "-1" { None } else { t.parse().ok() }
            }),
            pids_current: parse_u64(read_val("pids.current")).unwrap_or(0),
            pids_limit: read_val("pids.max").and_then(|v| {
                let t = v.trim();
                if t == "max" { None } else { t.parse().ok() }
            }),
            ..ResourceStats::default()
        };

        // cpu.stat → usage_usec
        if let Some(cpu_stat) = read_val("cpu.stat") {
            for line in cpu_stat.lines() {
                if let Some(val) = line.strip_prefix("usage_usec ") {
                    stats.cpu_usage_usec = val.trim().parse().unwrap_or(0);
                    break;
                }
            }
        }

        stats
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
        if let Some(swap) = cfg.memory_swap_bytes {
            cgroup.set_memory_swap(pod_uid, swap).ok();
        }
        match (cfg.cpu_quota, cfg.cpu_period) {
            (Some(quota), Some(period)) if quota > 0 && period > 0 => {
                cgroup.set_cpu_limit(pod_uid, quota, period).ok();
            }
            _ => {}
        }
        if let Some(shares) = cfg.cpu_shares.filter(|&s| s > 0) {
            cgroup.set_cpu_shares(pod_uid, shares).ok();
        }
        if let Some(ref cpus) = cfg.cpuset_cpus {
            cgroup.set_cpuset_cpus(pod_uid, cpus).ok();
        }
        if let Some(ref mems) = cfg.cpuset_mems {
            cgroup.set_cpuset_mems(pod_uid, mems).ok();
        }
        if let Some(pids) = cfg.pids_max {
            cgroup.set_pids_max(pod_uid, pids).ok();
        }
    }
}
