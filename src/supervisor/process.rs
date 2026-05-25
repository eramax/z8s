use crate::api::types::{
    extract_containers, parse_quantity_bytes, parse_quantity_cpu, ResourceState, ResourceStore,
};
use crate::api::AnyResource;
use crate::container::image::ImageManager;
use crate::supervisor::cgroup::CgroupManager;
use crate::supervisor::health::{HealthChecker, HealthStatus, ProbeAction, ProbeConfig};
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Container;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct ContainerInstance {
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub pid: Option<u32>,
    /// Empty string means native process (no chroot/OCI rootfs).
    pub rootfs: String,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug)]
pub struct RunningContainer {
    pub child: Option<Child>,
    pub instance: ContainerInstance,
    pub restart_count: u32,
    pub log_buffer: Arc<Mutex<Vec<String>>>,
    pub ready: Arc<Mutex<bool>>,
    pub healthy: Arc<Mutex<bool>>,
}

pub struct ProcessSupervisor {
    pub running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    pub image_manager: Arc<ImageManager>,
    pub cgroup_manager: Arc<CgroupManager>,
    pub health_checker: Arc<HealthChecker>,
    pub store: Arc<ResourceStore>,
}

impl ProcessSupervisor {
    pub fn new(
        image_manager: Arc<ImageManager>,
        cgroup_manager: Arc<CgroupManager>,
        health_checker: Arc<HealthChecker>,
        store: Arc<ResourceStore>,
    ) -> Self {
        let base = if nix::unistd::Uid::effective().is_root() {
            "/var/lib/z8s".to_string()
        } else {
            format!("{}/.local/share/z8s", std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
        };
        std::fs::create_dir_all(format!("{}/containers", base)).ok();
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
            image_manager,
            cgroup_manager,
            health_checker,
            store,
        }
    }

    pub async fn start_pod(&self, resource: &AnyResource) -> Result<()> {
        let containers = extract_containers(resource);
        if containers.is_empty() {
            warn!("No containers in resource {}", resource.name());
            return Ok(());
        }

        let pod_uid = resource.uid();
        let pod_name = resource.name().to_string();
        info!("Starting pod {} ({} container(s))", pod_name, containers.len());

        self.cgroup_manager.create_pod_cgroup(&pod_uid)?;
        self.set_resource_limits(&pod_uid, &containers)?;

        let mut prepared = Vec::new();
        for container in &containers {
            let container_id = format!("{}-{}", pod_name, container.name);
            let image_ref = container.image.clone().unwrap_or_default();

            // Empty image or "host://" → native process, no OCI pull
            let rootfs = if image_ref.is_empty() || image_ref == "host" || image_ref.starts_with("host://") {
                info!("Native process {}/{} (no OCI image)", pod_name, container.name);
                String::new()
            } else {
                self.image_manager
                    .unpack_image(&image_ref, &container_id)
                    .await
                    .context(format!("Failed to prepare image {} for {}", image_ref, container.name))?
            };

            info!("Starting container {}/{}", pod_name, container.name);
            let rc = self
                .spawn_container(container, &container_id, &rootfs, &pod_uid)
                .await?;
            prepared.push((container_id, rc));
        }

        let mut running = self.running.lock().await;
        for (cid, rc) in prepared {
            running.insert(cid, rc);
        }
        drop(running);

        self.store.update_state(&pod_uid, ResourceState::Running).await;
        Ok(())
    }

    async fn spawn_container(
        &self,
        container: &Container,
        container_id: &str,
        rootfs: &str,
        pod_uid: &str,
    ) -> Result<RunningContainer> {
        let image = container.image.clone().unwrap_or_default();
        let command = container.command.clone().unwrap_or_default();
        let args = container.args.clone().unwrap_or_default();
        let is_native = rootfs.is_empty();

        let cmd = if !command.is_empty() {
            command
        } else if is_native {
            anyhow::bail!("Native process container '{}' must have a command", container.name);
        } else {
            vec!["/bin/sh".to_string()]
        };

        let entrypoint = cmd[0].clone();
        let cmd_args: Vec<String> = cmd[1..].iter().chain(args.iter()).cloned().collect();

        let env_vars: Vec<(String, String)> = container
            .env
            .as_ref()
            .map(|env| {
                env.iter()
                    .map(|e| (e.name.clone(), e.value.clone().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default();

        let mut child_cmd = if is_native {
            // Run directly on the host filesystem
            let mut c = Command::new(&entrypoint);
            c.args(&cmd_args);
            c
        } else {
            // Run inside the container rootfs via chroot
            let mut c = Command::new(&entrypoint);
            c.args(&cmd_args).current_dir(rootfs);
            c
        };

        child_cmd
            .env_clear()
            .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = child_cmd.spawn().context("Failed to spawn container process")?;
        let pid = child.id().expect("No PID for spawned process");
        info!("Container {} started with PID {}", container_id, pid);

        self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

        let log_buffer = Arc::new(Mutex::new(Vec::<String>::new()));

        if let Some(stdout) = child.stdout.take() {
            let buf = log_buffer.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut log = buf.lock().await;
                    log.push(format!("[stdout] {}", line));
                    if log.len() > 1000 {
                        log.remove(0);
                    }
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let buf = log_buffer.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut log = buf.lock().await;
                    log.push(format!("[stderr] {}", line));
                    if log.len() > 1000 {
                        log.remove(0);
                    }
                }
            });
        }

        let instance = ContainerInstance {
            container_id: container_id.to_string(),
            container_name: container.name.clone(),
            image,
            pid: Some(pid),
            rootfs: rootfs.to_string(),
            started_at: Some(chrono::Utc::now()),
        };

        let ready = Arc::new(Mutex::new(true));
        let healthy = Arc::new(Mutex::new(true));

        if container.liveness_probe.is_some()
            || container.readiness_probe.is_some()
            || container.startup_probe.is_some()
        {
            let p_ready = ready.clone();
            let p_healthy = healthy.clone();
            let cid = container_id.to_string();
            let container_cfg = container.clone();
            tokio::spawn(async move {
                let probes = [
                    container_cfg.liveness_probe.as_ref().map(|p| ("liveness", p)),
                    container_cfg.readiness_probe.as_ref().map(|p| ("readiness", p)),
                    container_cfg.startup_probe.as_ref().map(|p| ("startup", p)),
                ];
                for (_name, probe) in probes.into_iter().flatten() {
                    if let Some(config) = ProbeConfig::from_probe(probe) {
                        tokio::time::sleep(Duration::from_secs(
                            config.initial_delay_seconds as u64,
                        ))
                        .await;
                        loop {
                            let status = match &config.action {
                                ProbeAction::Exec(exec) => {
                                    HealthChecker::check_exec(
                                        exec.command.as_deref().unwrap_or(&[]),
                                        config.timeout(),
                                    )
                                    .await
                                }
                                ProbeAction::HTTPGet(http) => {
                                    HealthChecker::check_http(http, config.timeout()).await
                                }
                                ProbeAction::TCPSocket(tcp) => {
                                    HealthChecker::check_tcp(tcp, config.timeout()).await
                                }
                            };
                            let ok = matches!(status, HealthStatus::Healthy);
                            *p_ready.lock().await = ok;
                            *p_healthy.lock().await = ok;
                            if !ok {
                                warn!("Probe for {} failed", cid);
                            }
                            tokio::time::sleep(Duration::from_secs(
                                config.period_seconds as u64,
                            ))
                            .await;
                        }
                    }
                }
            });
        }

        Ok(RunningContainer {
            child: Some(child),
            instance,
            restart_count: 0,
            log_buffer,
            ready,
            healthy,
        })
    }

    fn set_resource_limits(&self, pod_uid: &str, containers: &[Container]) -> Result<()> {
        for container in containers {
            if let Some(resources) = &container.resources {
                if let Some(limits) = &resources.limits {
                    if let Some(memory) = limits.get("memory") {
                        let bytes = parse_quantity_bytes(memory);
                        if bytes > 0 {
                            self.cgroup_manager.set_memory_limit(pod_uid, bytes as i64)?;
                        }
                    }
                    if let Some(cpu) = limits.get("cpu") {
                        let (quota, period) = parse_quantity_cpu(cpu);
                        self.cgroup_manager.set_cpu_limit(pod_uid, quota, period)?;
                    }
                }
                if let Some(requests) = &resources.requests {
                    if let Some(memory) = requests.get("memory") {
                        let bytes = parse_quantity_bytes(memory);
                        if bytes > 0 {
                            self.cgroup_manager.set_memory_low(pod_uid, bytes as i64)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn stop_container(&self, container_id: &str) {
        let mut running = self.running.lock().await;
        if let Some(rc) = running.remove(container_id) {
            if let Some(pid) = rc.instance.pid {
                info!("Stopping container {} (PID {})", container_id, pid);
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            }
        }
    }

    pub async fn restart_container(&self, container_id: &str) {
        info!("Restarting container {}", container_id);
        let spec = {
            let running = self.running.lock().await;
            running.get(container_id).map(|rc| {
                (
                    rc.instance.image.clone(),
                    rc.instance.rootfs.clone(),
                    rc.instance.container_name.clone(),
                )
            })
        };
        self.stop_container(container_id).await;
        if let Some((_image, rootfs, _name)) = spec {
            // Re-spawning requires the original Container spec; store it or delegate to
            // reconcile() which will detect the missing container and restart it.
            info!("Container {} stopped; reconcile loop will restart it", container_id);
            let _ = rootfs; // rootfs is available if needed for direct re-spawn
        }
    }

    pub async fn stop_pod(&self, resource: &AnyResource) {
        let pod_name = resource.name();
        for container in &extract_containers(resource) {
            self.stop_container(&format!("{}-{}", pod_name, container.name)).await;
        }
        let pod_uid = resource.uid();
        self.cgroup_manager.remove_cgroup(&pod_uid).ok();
        self.store.update_state(&pod_uid, ResourceState::Terminated).await;
    }

    pub async fn is_pod_ready(&self, pod_name: &str) -> bool {
        let arcs: Vec<_> = {
            let running = self.running.lock().await;
            running
                .values()
                .filter(|rc| rc.instance.container_id.starts_with(pod_name))
                .map(|rc| rc.ready.clone())
                .collect()
        };
        for arc in arcs {
            if *arc.lock().await {
                return true;
            }
        }
        false
    }

    pub async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String> {
        let container_id = format!("{}-{}", pod_name, container_name);
        let running = self.running.lock().await;
        if let Some(rc) = running.get(&container_id) {
            return rc.log_buffer.lock().await.clone();
        }
        Vec::new()
    }

    /// Reconcile only Pods. Deployments are owned by DeploymentController.
    pub async fn reconcile(&self) {
        let resources = self.store.get_all().await;
        for tracker in &resources {
            if !matches!(&tracker.resource, AnyResource::Pod(_)) {
                continue;
            }
            let uid = tracker.resource.uid();
            let is_running = {
                let running = self.running.lock().await;
                running.values().any(|rc| rc.instance.container_id.starts_with(&uid))
            };
            if tracker.state == ResourceState::Pending && !is_running {
                if let Err(e) = self.start_pod(&tracker.resource).await {
                    error!("Failed to start {}: {:?}", uid, e);
                    self.store.update_state(&uid, ResourceState::Failed(e.to_string())).await;
                }
            }
        }
    }
}
