use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::cri::RuntimeProvider;
use crate::cri::runtime::RunningContainer;
use crate::netmux::network::PodResolver;
use crate::store::StoreBackend;
use crate::store::{AnyResource, ResourceState};
use async_trait::async_trait;

pub struct ProcessTracker {
    pub running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    pub restart_counts: Arc<Mutex<HashMap<String, u32>>>,
    pub cri: Arc<dyn RuntimeProvider>,
    pub store: Arc<dyn StoreBackend>,
    pub broadcast_tx: tokio::sync::RwLock<Option<tokio::sync::mpsc::UnboundedSender<AnyResource>>>,
}

impl ProcessTracker {
    pub fn new(
        running: Arc<Mutex<HashMap<String, RunningContainer>>>,
        restart_counts: Arc<Mutex<HashMap<String, u32>>>,
        cri: Arc<dyn RuntimeProvider>,
        store: Arc<dyn StoreBackend>,
    ) -> Self {
        Self {
            running,
            restart_counts,
            cri,
            store,
            broadcast_tx: tokio::sync::RwLock::new(None),
        }
    }

    pub async fn set_broadcast_tx(&self, tx: tokio::sync::mpsc::UnboundedSender<AnyResource>) {
        *self.broadcast_tx.write().await = Some(tx);
    }

    pub async fn start_pod(&self, resource: &AnyResource) -> anyhow::Result<()> {
        let spec =
            crate::components::compute::spec_builder::build_spec(resource, self.store.as_ref())
                .await;
        self.cri.start_pod(&spec).await?;
        self.store
            .update_state(&resource.uid(), ResourceState::Running)
            .await;
        if let Some(mut t) = self.store.get(&resource.uid()).await {
            t.state = ResourceState::Running;
            if let Some(tx) = &*self.broadcast_tx.read().await {
                let _ = tx.send(t.resource);
            }
        }
        Ok(())
    }

    pub async fn stop_pod(&self, resource: &AnyResource) {
        let spec =
            crate::components::compute::spec_builder::build_spec(resource, self.store.as_ref())
                .await;
        let _ = self.cri.stop_pod(&spec).await;
        self.store
            .update_state(&resource.uid(), ResourceState::Terminated)
            .await;
        if let Some(mut t) = self.store.get(&resource.uid()).await {
            t.state = ResourceState::Terminated;
            if let Some(tx) = &*self.broadcast_tx.read().await {
                let _ = tx.send(t.resource);
            }
        }
    }

    pub async fn is_running(&self, pod_name: &str) -> bool {
        let prefix = format!("{}-", pod_name);
        self.running
            .lock()
            .await
            .keys()
            .any(|cid| cid.starts_with(&prefix))
    }

    pub async fn is_ready(&self, pod_name: &str) -> bool {
        self.is_running(pod_name).await
    }

    pub async fn get_logs(&self, pod_name: &str, container_name: &str) -> Vec<String> {
        let container_id = format!("{}-{}", pod_name, container_name);
        let running = self.running.lock().await;
        if let Some(rc) = running.get(&container_id) {
            return rc.log_buffer.lock().await.clone();
        }
        Vec::new()
    }

    pub async fn pod_restart_counts(&self, pod_name: &str) -> HashMap<String, u32> {
        self.restart_counts
            .lock()
            .await
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{}-", pod_name)))
            .map(|(k, v)| {
                (
                    k.trim_start_matches(&format!("{}-", pod_name)).to_string(),
                    *v,
                )
            })
            .collect()
    }

    pub async fn restart_count(&self, pod_name: &str) -> u32 {
        let counts = self.restart_counts.lock().await;
        counts
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{}-", pod_name)))
            .map(|(_, v)| *v)
            .sum()
    }

    /// Get all pod IPs (name → IP) for enrichment.
    pub async fn pod_ips(&self) -> Vec<(String, std::net::Ipv4Addr)> {
        let running = self.running.lock().await;
        let mut ips = Vec::new();
        for (cid, rc) in running.iter() {
            if let Some(ip) = rc.instance.pod_ip {
                // Extract pod name from container ID (pod-container)
                let pod_name = cid.rsplit_once('-').map_or(cid.as_str(), |(pod, _)| pod);
                ips.push((pod_name.to_string(), ip));
            }
        }
        ips
    }

    /// Get the pod IP for a given pod name.
    pub async fn pod_ip(&self, pod_name: &str) -> Option<std::net::Ipv4Addr> {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        for (cid, rc) in running.iter() {
            if cid.starts_with(&prefix) {
                if let Some(ip) = rc.instance.pod_ip {
                    return Some(ip);
                }
            }
        }
        None
    }

    pub async fn backend_connect_port(&self, pod_name: &str, container_port: u16) -> u16 {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        for (cid, rc) in running.iter() {
            if cid.starts_with(&prefix) {
                if rc.instance.isolated_net {
                    return rc
                        .instance
                        .published_ports
                        .get(&container_port)
                        .copied()
                        .unwrap_or(container_port);
                }
                return container_port;
            }
        }
        container_port
    }

    pub async fn is_container_ready(&self, container_id: &str) -> bool {
        let running = self.running.lock().await;
        running
            .get(container_id)
            .map(|rc| rc.ready.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    fn is_pid_alive(pid: u32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }

    pub fn reap_zombies(&self) -> Vec<(u32, i32)> {
        let mut reaped = Vec::new();
        loop {
            match nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(-1),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG),
            ) {
                Ok(nix::sys::wait::WaitStatus::Exited(pid, status)) => {
                    info!("Reaped zombie child {} (exit code {})", pid, status);
                    reaped.push((pid.as_raw() as u32, status));
                }
                Ok(nix::sys::wait::WaitStatus::Signaled(pid, sig, _)) => {
                    info!("Reaped zombie child {} (signal {:?})", pid, sig);
                    reaped.push((pid.as_raw() as u32, -(sig as i32)));
                }
                Ok(nix::sys::wait::WaitStatus::StillAlive) => break,
                Err(nix::errno::Errno::ECHILD) => break,
                _ => break,
            }
        }
        reaped
    }

    pub async fn handle_exited_containers(
        &self,
        reaped: Vec<(u32, i32)>,
        store: &Arc<dyn StoreBackend>,
    ) {
        let reaped_map: HashMap<u32, i32> = reaped.into_iter().collect();
        let dead: Vec<(String, u32, i32)> = {
            let running = self.running.lock().await;
            running
                .values()
                .filter_map(|rc| {
                    let pid = rc.instance.pid?;
                    let code = *reaped_map.get(&pid)?;
                    Some((rc.instance.container_id.clone(), pid, code))
                })
                .collect()
        };
        for (container_id, pid, exit_code) in dead {
            let trackers = store.get_all().await;
            let pod_tracker = trackers.iter().find(|t| {
                if !matches!(&t.resource, AnyResource::Pod(_)) {
                    return false;
                }
                let pod_name = t.resource.name();
                container_id.starts_with(&format!("{}-", pod_name))
            });
            let restart_policy = pod_tracker
                .and_then(|t| {
                    if let AnyResource::Pod(pod) = &t.resource {
                        pod.spec.as_ref()?.restart_policy.clone()
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "Always".to_string());
            let pod_uid = pod_tracker.map(|t| t.resource.uid()).unwrap_or_default();
            // Kill any orphaned child processes from the container's process group
            // These survive the parent exit and can hold ports (e.g., postgres pg_ctl spawn)
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-(pid as i32)),
                nix::sys::signal::Signal::SIGKILL,
            );

            let should_restart = match restart_policy.as_str() {
                "Always" => true,
                "OnFailure" => exit_code != 0,
                "Never" => false,
                _ => true,
            };
            info!(
                "Container {} (PID {}) exited with code {}; restartPolicy={}, restart={}",
                container_id, pid, exit_code, restart_policy, should_restart
            );
            let log_lines = if let Some(rc) = self.running.lock().await.get(&container_id) {
                rc.log_buffer.lock().await.clone()
            } else {
                Vec::new()
            };
            if !log_lines.is_empty() {
                warn!("--- Container {} logs before exit: ---", container_id);
                for line in log_lines {
                    warn!("  {}", line);
                }
                warn!("---------------------------------------");
            }
            self.running.lock().await.remove(&container_id);
            if should_restart {
                let mut counts = self.restart_counts.lock().await;
                let count = counts.entry(container_id.clone()).or_insert(0);
                *count += 1;
                let restart_count = *count;
                drop(counts);
                if !pod_uid.is_empty() {
                    let delay_secs: u64 = if restart_count <= 1 {
                        0
                    } else {
                        std::cmp::min(10u64 << (restart_count - 2).min(5), 300)
                    };
                    let store = store.clone();
                    let uid = pod_uid.clone();
                    tokio::spawn(async move {
                        if delay_secs > 0 {
                            info!(
                                "CrashLoopBackOff: restarting {} in {}s (restart #{})",
                                uid, delay_secs, restart_count
                            );
                            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                        }
                        store.update_state(&uid, ResourceState::Pending).await;
                        if let Some(mut t) = store.get(&uid).await {
                            t.state = ResourceState::Pending;
                            // (We don't easily have access to self.broadcast_tx here inside the spawn, 
                            // but the next start_pod will broadcast Running)
                        }
                    });
                }
            } else {
                if !pod_uid.is_empty() {
                    if exit_code == 0 {
                        store.update_state(&pod_uid, ResourceState::Succeeded).await;
                        if let Some(mut t) = store.get(&pod_uid).await {
                            t.state = ResourceState::Succeeded;
                            if let Some(tx) = &*self.broadcast_tx.read().await {
                                let _ = tx.send(t.resource);
                            }
                        }
                    } else {
                        store
                            .update_state(
                                &pod_uid,
                                ResourceState::Failed(format!("exit code {}", exit_code)),
                            )
                            .await;
                        if let Some(mut t) = store.get(&pod_uid).await {
                            t.state = ResourceState::Failed(format!("exit code {}", exit_code));
                            if let Some(tx) = &*self.broadcast_tx.read().await {
                                let _ = tx.send(t.resource);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl PodResolver for ProcessTracker {
    async fn is_pod_alive(&self, pod_name: &str) -> bool {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        running.iter().any(|(cid, rc)| {
            cid.starts_with(&prefix)
                && rc
                    .instance
                    .pid
                    .map(|p| Self::is_pid_alive(p))
                    .unwrap_or(false)
        })
    }

    async fn backend_connect_port(&self, pod_name: &str, container_port: u16) -> u16 {
        self.backend_connect_port(pod_name, container_port).await
    }
}
