//! # Container Specification — Data Types for Container Configuration
//!
//! Pure data types describing what a container needs. No behavior, no side effects.
//! The supervisor reads these to decide how to spawn and manage containers.

use std::collections::{BTreeMap, HashMap};

use super::health::ProbeConfig;

/// Top-level specification for a pod's containers.
///
/// Built by the controller from a `ResourceRecord<Pod>` and passed to
/// `RuntimeProvider::start_pod`.
#[derive(Debug, Clone)]
pub struct ContainerSpec {
    pub pod_name: String,
    pub pod_uid: String,
    pub namespace: String,
    pub hostname: String,
    pub containers: Vec<ContainerConfig>,
    pub labels: BTreeMap<String, String>,
    pub subnet: Option<String>,
}

/// Configuration for a single container within a pod.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub entrypoint: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    pub env: Vec<(String, String)>,
    pub volumes: Vec<ResolvedVolume>,
    pub memory_limit_bytes: Option<i64>,
    pub memory_low_bytes: Option<i64>,
    pub cpu_quota: Option<i64>,
    pub cpu_period: Option<i64>,
    pub run_as_user: Option<u32>,
    pub run_as_group: Option<u32>,
    pub privileged: bool,
    pub cap_profile: Option<String>,
    pub extra_capabilities: Vec<String>,
    pub isolated_net: bool,
    pub published_ports: HashMap<u16, u16>,
    pub probes: Vec<ProbeConfig>,
    pub is_native: bool,
}

/// A resolved volume mount: host path → container path.
#[derive(Debug, Clone)]
pub struct ResolvedVolume {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

// ── Builder ────────────────────────────────────────────────────────────────

/// Fluent builder for `ContainerConfig`.
pub struct ContainerConfigBuilder {
    config: ContainerConfig,
}

impl ContainerConfigBuilder {
    pub fn new(container_id: &str, container_name: &str, image: &str) -> Self {
        Self {
            config: ContainerConfig {
                container_id: container_id.to_string(),
                container_name: container_name.to_string(),
                image: image.to_string(),
                entrypoint: String::new(),
                args: Vec::new(),
                working_dir: None,
                env: Vec::new(),
                volumes: Vec::new(),
                memory_limit_bytes: None,
                memory_low_bytes: None,
                cpu_quota: None,
                cpu_period: None,
                run_as_user: None,
                run_as_group: None,
                privileged: false,
                cap_profile: None,
                extra_capabilities: Vec::new(),
                isolated_net: false,
                published_ports: HashMap::new(),
                probes: Vec::new(),
                is_native: false,
            },
        }
    }

    pub fn entrypoint(mut self, ep: &str) -> Self {
        self.config.entrypoint = ep.to_string();
        self
    }

    pub fn args(mut self, args: Vec<String>) -> Self {
        self.config.args = args;
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.config.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn envs(mut self, vars: Vec<(String, String)>) -> Self {
        self.config.env = vars;
        self
    }

    pub fn working_dir(mut self, dir: &str) -> Self {
        self.config.working_dir = Some(dir.to_string());
        self
    }

    pub fn volumes(mut self, vols: Vec<ResolvedVolume>) -> Self {
        self.config.volumes = vols;
        self
    }

    pub fn memory_limit(mut self, bytes: i64) -> Self {
        self.config.memory_limit_bytes = Some(bytes);
        self
    }

    pub fn cpu_limit(mut self, quota: i64, period: i64) -> Self {
        self.config.cpu_quota = Some(quota);
        self.config.cpu_period = Some(period);
        self
    }

    pub fn run_as(mut self, uid: u32, gid: u32) -> Self {
        self.config.run_as_user = Some(uid);
        self.config.run_as_group = Some(gid);
        self
    }

    pub fn privileged(mut self, yes: bool) -> Self {
        self.config.privileged = yes;
        self
    }

    pub fn isolated_net(mut self, yes: bool) -> Self {
        self.config.isolated_net = yes;
        self
    }

    pub fn probes(mut self, probes: Vec<ProbeConfig>) -> Self {
        self.config.probes = probes;
        self
    }

    pub fn native(mut self) -> Self {
        self.config.is_native = true;
        self
    }

    pub fn build(self) -> ContainerConfig {
        self.config
    }
}
