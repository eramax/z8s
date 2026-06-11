//! Parent-side logic after container fork.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::cri::health::{HealthChecker, HealthStatus, ProbeAction, ProbeConfig};
use crate::cri::runtime::{ContainerInstance, ProcessSupervisor, RunningContainer};

impl ProcessSupervisor {
    pub(crate) fn merge_env(env_vars: &[(String, String)], rootfs: &str) -> Vec<(String, String)> {
        let oci_env = crate::cri::oci::read_image_config(rootfs)
            .env
            .unwrap_or_default();
        let mut env_owned: Vec<(String, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (k, v) in env_vars {
            if seen.insert(k.clone()) {
                env_owned.push((k.clone(), v.clone()));
            }
        }
        for entry in &oci_env {
            if let Some(eq) = entry.find('=') {
                let key = entry[..eq].to_string();
                let val = entry[eq + 1..].to_string();
                if seen.insert(key.clone()) {
                    env_owned.push((key, val));
                }
            }
        }
        env_owned
    }

    /// Common parent-side logic after fork: cgroup, log tasks, build RunningContainer.
    pub(crate) fn parent_post_fork(
        &self,
        child_pid: u32,
        container_id: &str,
        container_name: &str,
        image: &str,
        rootfs_path: &str,
        env_owned: Vec<(String, String)>,
        pod_uid: &str,
        isolate_net: bool,
        pod_ip: Option<std::net::Ipv4Addr>,
        host_veth_ifindex: Option<u32>,
        run_as_user: Option<u32>,
        run_as_group: Option<u32>,
        stdout_r: std::os::fd::OwnedFd,
        stderr_r: std::os::fd::OwnedFd,
        probes: &[ProbeConfig],
    ) -> Result<RunningContainer> {
        info!("Container {} started with PID {}", container_id, child_pid);
        self.cgroup_manager.add_pid_to_cgroup(pod_uid, child_pid)?;

        let log_buffer = Self::spawn_log_tasks(stdout_r, stderr_r);
        let instance = Self::build_container_instance(
            container_id,
            container_name,
            image,
            child_pid,
            rootfs_path,
            env_owned,
            isolate_net,
            pod_ip,
            host_veth_ifindex,
            run_as_user,
            run_as_group,
        );
        Ok(Self::build_running_from_instance(instance, log_buffer, probes))
    }

    pub(crate) fn handle_veth_netns(
        &self,
        pod_uid: &str,
        pid: u32,
        isolate_net: bool,
        sync_r: &std::os::fd::OwnedFd,
        ack_w: &std::os::fd::OwnedFd,
        subnet: Option<&str>,
    ) -> (Option<std::net::Ipv4Addr>, Option<u32>) {
        if !isolate_net {
            return (None, None);
        }
        let mut sync_buf = [0u8; 1];
        let n = nix::unistd::read(sync_r, &mut sync_buf).unwrap_or(0);
        if n > 0 && sync_buf[0] == b'S' {
            match self.netmux.attach_pod(pod_uid, Some(pid), subnet) {
                Ok((ip, host_idx, peer_idx)) => {
                    if let Err(e) = self.netmux.configure_pod_netns(pod_uid, &ip, pid, peer_idx) {
                        warn!("NetMux configure_pod_netns failed: {:#}", e);
                    }
                    nix::unistd::write(ack_w, b"A").ok();
                    return (Some(ip), Some(host_idx));
                }
                Err(e) => warn!("NetMux: failed to attach pod {}: {:?}", pod_uid, e),
            }
        }
        nix::unistd::write(ack_w, b"A").ok();
        (None, None)
    }

    pub(crate) fn spawn_log_tasks(
        stdout_r: std::os::fd::OwnedFd,
        stderr_r: std::os::fd::OwnedFd,
    ) -> Arc<Mutex<Vec<String>>> {
        let log_buffer = Arc::new(Mutex::new(Vec::<String>::new()));
        {
            let buf = log_buffer.clone();
            let file = tokio::fs::File::from_std(std::fs::File::from(stdout_r));
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(file).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut log = buf.lock().await;
                    log.push(format!("[stdout] {}", line));
                    if log.len() > 1000 {
                        log.remove(0);
                    }
                }
            });
        }
        {
            let buf = log_buffer.clone();
            let file = tokio::fs::File::from_std(std::fs::File::from(stderr_r));
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(file).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut log = buf.lock().await;
                    log.push(format!("[stderr] {}", line));
                    if log.len() > 1000 {
                        log.remove(0);
                    }
                }
            });
        }
        log_buffer
    }
    pub(crate) fn build_container_instance(
        container_id: &str,
        container_name: &str,
        image: &str,
        pid: u32,
        rootfs_path: &str,
        env_vars: Vec<(String, String)>,
        isolate_net: bool,
        pod_ip: Option<std::net::Ipv4Addr>,
        host_veth_ifindex: Option<u32>,
        run_as_user: Option<u32>,
        run_as_group: Option<u32>,
    ) -> ContainerInstance {
        ContainerInstance {
            container_id: container_id.to_string(),
            container_name: container_name.to_string(),
            image: image.to_string(),
            pid: Some(pid),
            rootfs: rootfs_path.to_string(),
            started_at: Some(crate::config::now_rfc3339()),
            env_vars,
            published_ports: std::collections::HashMap::new(),
            isolated_net: isolate_net,
            pod_ip,
            host_veth_ifindex,
            run_as_user,
            run_as_group,
        }
    }

    pub(crate) fn build_running_from_instance(
        instance: ContainerInstance,
        log_buffer: Arc<Mutex<Vec<String>>>,
        probes: &[ProbeConfig],
    ) -> RunningContainer {
        let (ready, healthy) = crate::cri::probe_runner::spawn_container_probes(
            probes,
            &instance.container_id,
            &instance.published_ports,
        );
        RunningContainer {
            child: None,
            instance,
            restart_count: 0,
            log_buffer,
            ready,
            healthy,
        }
    }
}
