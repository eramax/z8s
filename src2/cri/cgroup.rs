use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tracing::{debug, info, warn};

const CGROUP_DIR: &str = "/sys/fs/cgroup";

pub struct CgroupManager {
    base_path: String,
    enabled: bool,
}

impl CgroupManager {
    pub fn new() -> Result<Self> {
        let base_path = format!("{}/z8s", CGROUP_DIR);
        if !Path::new(CGROUP_DIR).exists() {
            warn!("cgroups v2 not found — running without resource limits");
            return Ok(Self {
                base_path,
                enabled: false,
            });
        }
        match fs::create_dir_all(&base_path) {
            Ok(_) => {
                let subtree = format!("{}/cgroup.subtree_control", base_path);
                for ctrl in &["+pids", "+memory", "+cpu"] {
                    if let Err(e) = fs::write(&subtree, ctrl) {
                        debug!(
                            "Could not enable cgroup controller {} ({}): {}",
                            ctrl, subtree, e
                        );
                    }
                }
                Ok(Self {
                    base_path,
                    enabled: true,
                })
            }
            Err(e) => {
                warn!(
                    "Cannot write cgroup dir (need root?): {} — running without resource limits",
                    e
                );
                Ok(Self {
                    base_path,
                    enabled: false,
                })
            }
        }
    }

    /// Create a stub (no-op) CgroupManager for degraded mode.
    pub fn new_stub() -> Self {
        Self {
            base_path: String::new(),
            enabled: false,
        }
    }

    pub fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String> {
        if !self.enabled {
            return Ok(String::new());
        }
        let cg_path = format!("{}/{}", self.base_path, sanitize_cgroup_name(pod_uid));
        match fs::create_dir_all(&cg_path) {
            Ok(_) => info!("Created cgroup: {}", cg_path),
            Err(e) => {
                warn!(
                    "Cannot create pod cgroup (no root?): {} — continuing without cgroup",
                    e
                );
                return Ok(String::new());
            }
        }
        // Lift the pids limit so multi-process daemons (nginx workers, etc.) can fork freely.
        let pids_path = format!("{}/pids.max", cg_path);
        if let Err(e) = fs::write(&pids_path, "max") {
            debug!("Could not set pids.max ({}): {} — continuing", pids_path, e);
        } else {
            debug!("Set pids.max=max for cgroup {}", cg_path);
        }
        Ok(cg_path)
    }

    pub fn set_memory_limit(&self, pod_uid: &str, limit_bytes: i64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let path = format!(
            "{}/{}/memory.max",
            self.base_path,
            sanitize_cgroup_name(pod_uid)
        );
        if limit_bytes > 0 {
            fs::write(&path, format!("{}", limit_bytes))
                .context(format!("Failed to set memory.max at {}", path))?;
            debug!("Set memory limit {} bytes for {}", limit_bytes, pod_uid);
        }
        Ok(())
    }

    pub fn set_cpu_limit(&self, pod_uid: &str, cpu_quota: i64, cpu_period: i64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if cpu_quota > 0 && cpu_period > 0 {
            let quota_path = format!(
                "{}/{}/cpu.max",
                self.base_path,
                sanitize_cgroup_name(pod_uid)
            );
            fs::write(&quota_path, format!("{} {}", cpu_quota, cpu_period))
                .context(format!("Failed to set cpu.max at {}", quota_path))?;
            debug!("Set CPU quota {}/{} for {}", cpu_quota, cpu_period, pod_uid);
        }
        Ok(())
    }

    pub fn add_pid_to_cgroup(&self, pod_uid: &str, pid: u32) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let path = format!(
            "{}/{}/cgroup.procs",
            self.base_path,
            sanitize_cgroup_name(pod_uid)
        );
        if let Err(e) = fs::write(&path, format!("{}", pid)) {
            warn!(
                "Cannot write pid {} to cgroup (no root?): {} — continuing without cgroup tracking",
                pid, e
            );
        } else {
            debug!("Added pid {} to cgroup {}", pid, pod_uid);
        }
        Ok(())
    }

    pub fn remove_cgroup(&self, pod_uid: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let cg_path = format!("{}/{}", self.base_path, sanitize_cgroup_name(pod_uid));

        // Use a subprocess to prevent the async runtime or z8s threads from
        // getting stuck in D-state on broken kernfs/overlayfs mounts.
        let _ = std::process::Command::new("rmdir").arg(&cg_path).spawn(); // ignore errors, fire and forget

        Ok(())
    }

    pub fn set_memory_low(&self, pod_uid: &str, limit_bytes: i64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if limit_bytes > 0 {
            let path = format!(
                "{}/{}/memory.low",
                self.base_path,
                sanitize_cgroup_name(pod_uid)
            );
            fs::write(&path, format!("{}", limit_bytes))
                .context(format!("Failed to set memory.low at {}", path))?;
        }
        Ok(())
    }
}

fn sanitize_cgroup_name(name: &str) -> String {
    name.replace('/', "_").replace('.', "_").replace(':', "_")
}
