//! Per-container spawn inputs (C1 pipeline context).

use crate::cri::health::ProbeConfig;
use crate::cri::rootfs;

/// Isolation strategy for container spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationStrategy {
    /// Root mode: double-fork with PID namespace, pivot_root
    RootNs,
    /// Non-root: user namespace + chroot
    UserNs,
}

pub fn isolation_strategy() -> IsolationStrategy {
    if rootfs::is_root() {
        IsolationStrategy::RootNs
    } else {
        IsolationStrategy::UserNs
    }
}

pub struct ContainerSpawnCtx<'a> {
    pub entrypoint: &'a str,
    pub cmd_args: &'a [String],
    pub env_vars: &'a [(String, String)],
    pub rootfs_path: &'a str,
    pub container_id: &'a str,
    pub pod_uid: &'a str,
    pub image: &'a str,
    pub container_name: &'a str,
    pub volumes: Vec<crate::cri::volumes::ResolvedVolume>,
    pub run_as_user: Option<u32>,
    pub run_as_group: Option<u32>,
    pub isolate_net: bool,
    pub privileged: bool,
    pub is_native: bool,
    pub extra_caps: Vec<String>,
    pub working_dir: Option<String>,
    pub probes: Vec<ProbeConfig>,
    pub subnet: Option<String>,
}
