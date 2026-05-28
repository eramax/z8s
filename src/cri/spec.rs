use std::collections::{BTreeMap, HashMap};

use crate::cri::health::ProbeConfig;

#[derive(Debug, Clone)]
pub struct ContainerSpec {
    pub pod_name: String,
    pub pod_uid: String,
    pub namespace: String,
    pub hostname: String,
    pub containers: Vec<ContainerConfig>,
    pub cgroup_path: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub rootfs_path: String,
    pub is_native: bool,
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
    pub extra_capabilities: Vec<String>,
    pub isolated_net: bool,
    pub published_ports: HashMap<u16, u16>,
    pub probes: Vec<ProbeConfig>,
}

pub use crate::cri::volumes::ResolvedVolume;
