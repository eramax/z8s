use crate::api::types::{extract_containers, ResourceState, ResourceStore};
use crate::api::AnyResource;
use crate::container::image::ImageManager;
use crate::supervisor::cgroup::CgroupManager;
use crate::supervisor::health::HealthChecker;
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Container;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
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
    pub rootfs: String,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug)]
pub struct RunningContainer {
    pub child: Option<Child>,
    pub instance: ContainerInstance,
    pub restart_count: u32,
    pub log_buffer: Arc<Mutex<Vec<String>>>,
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
        let data_dir = "/var/lib/z8s/containers".to_string();
        std::fs::create_dir_all(&data_dir).ok();

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
            warn!("No containers found in resource {}", resource.name());
            return Ok(());
        }

        let pod_uid = resource.uid();
        let pod_name = resource.name().to_string();

        info!(
            "Starting pod: {} ({} container(s))",
            pod_name,
            containers.len()
        );

        self.cgroup_manager.create_pod_cgroup(&pod_uid)?;
        self.set_resource_limits(&pod_uid, &containers)?;

        let mut running = self.running.lock().await;

        for container in &containers {
            let container_name = container.name.clone();
            let container_id = format!("{}-{}", pod_name, container_name);

            let image_ref = container
                .image
                .clone()
                .unwrap_or_else(|| "docker.io/library/alpine:latest".to_string());

            let rootfs = self
                .image_manager
                .unpack_image(&image_ref, &container_id)
                .await
                .context(format!(
                    "Failed to prepare image {} for container {}",
                    image_ref, container_name
                ))?;

            info!("Starting container {}/{}", pod_name, container_name);

            let running_container = self
                .spawn_container(container, &container_id, &rootfs, &pod_uid)
                .await?;

            running.insert(container_id.clone(), running_container);
        }

        drop(running);

        self.store
            .update_state(&pod_uid, ResourceState::Running)
            .await;

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

        let cmd = if !command.is_empty() {
            command
        } else {
            vec!["/bin/sh".to_string()]
        };

        let entrypoint = cmd[0].clone();
        let cmd_args: Vec<&str> = if !args.is_empty() {
            cmd.iter()
                .skip(1)
                .chain(args.iter())
                .map(|s| s.as_str())
                .collect()
        } else {
            cmd.iter().skip(1).map(|s| s.as_str()).collect()
        };

        let env_vars: Vec<(&str, &str)> = container
            .env
            .as_ref()
            .map(|env| {
                env.iter()
                    .map(|e| {
                        let name: &str = e.name.as_str();
                        let value: &str = e.value.as_deref().unwrap_or("");
                        (name, value)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut child = Command::new(&entrypoint)
            .args(&cmd_args)
            .env_clear()
            .envs(env_vars)
            .current_dir(rootfs)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("Failed to spawn container process")?;

        let pid = child.id().expect("No PID for spawned process");
        info!("Container {} started with PID {}", container_id, pid);

        self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

        let log_buffer = Arc::new(Mutex::new(Vec::new()));

        // Capture stdout
        if let Some(stdout) = child.stdout.take() {
            let buf = log_buffer.clone();
            let cid = container_id.to_string();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let mut log = buf.lock().await;
                    log.push(format!("[stdout] {}", line));
                    if log.len() > 1000 { log.remove(0); }
                }
            });
        }

        // Capture stderr
        if let Some(stderr) = child.stderr.take() {
            let buf = log_buffer.clone();
            let cid = container_id.to_string();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let mut log = buf.lock().await;
                    log.push(format!("[stderr] {}", line));
                    if log.len() > 1000 { log.remove(0); }
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

        Ok(RunningContainer {
            child: Some(child),
            instance,
            restart_count: 0,
            log_buffer,
        })
    }

    fn set_resource_limits(&self, pod_uid: &str, containers: &[Container]) -> Result<()> {
        for container in containers {
            if let Some(resources) = &container.resources {
                if let Some(limits) = &resources.limits {
                    if let Some(memory) = limits.get("memory") {
                        let bytes = parse_quantity(memory);
                        if bytes > 0 {
                            self.cgroup_manager.set_memory_limit(pod_uid, bytes as i64)?;
                        }
                    }

                    if let Some(cpu) = limits.get("cpu") {
                        let (quota, period) = parse_cpu_quantity(cpu);
                        self.cgroup_manager.set_cpu_limit(pod_uid, quota, period)?;
                    }
                }

                if let Some(requests) = &resources.requests {
                    if let Some(memory) = requests.get("memory") {
                        let bytes = parse_quantity(memory);
                        if bytes > 0 {
                            self.cgroup_manager.set_memory_low(pod_uid, bytes as i64)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn restart_container(&self, container_id: &str) {
        info!("Restarting container: {}", container_id);
        self.stop_container(container_id).await;
    }

    pub async fn stop_container(&self, container_id: &str) {
        let mut running = self.running.lock().await;
        if let Some(rc) = running.remove(container_id) {
            if let Some(pid) = rc.instance.pid {
                info!("Stopping container {} (PID {})", container_id, pid);
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
                // Brief grace period then force kill
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            }
        }
    }

    pub async fn stop_pod(&self, resource: &AnyResource) {
        let containers = extract_containers(resource);
        let pod_name = resource.name();

        for container in &containers {
            let container_id = format!("{}-{}", pod_name, container.name);
            self.stop_container(&container_id).await;
        }

        let pod_uid = resource.uid();
        self.cgroup_manager.remove_cgroup(&pod_uid).ok();
        self.store
            .update_state(&pod_uid, ResourceState::Terminated)
            .await;
    }

    pub async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String> {
        let container_id = format!("{}-{}", pod_name, container_name);
        let running = self.running.lock().await;
        if let Some(rc) = running.get(&container_id) {
            return rc.log_buffer.lock().await.clone();
        }
        Vec::new()
    }

    pub async fn reconcile(&self) {
        let resources = self.store.get_all().await;

        for tracker in &resources {
            let uid = tracker.resource.uid();
            let running = self.running.lock().await;

            match &tracker.resource {
                AnyResource::Pod(_) | AnyResource::Deployment(_) => {
                    let is_running = running
                        .values()
                        .any(|rc| rc.instance.container_id.starts_with(&uid));

                    if tracker.state == ResourceState::Pending && !is_running {
                        drop(running);
                        if let Err(e) = self.start_pod(&tracker.resource).await {
                            error!("Failed to start {}: {:?}", uid, e);
                            self.store
                                .update_state(&uid, ResourceState::Failed(e.to_string()))
                                .await;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn parse_quantity(q: &Quantity) -> u64 {
    let s = format!("{:?}", q);
    let s = s.trim();
    if let Some(rest) = s.strip_suffix("Ki") {
        rest.parse::<u64>().unwrap_or(0) * 1024
    } else if let Some(rest) = s.strip_suffix("Mi") {
        rest.parse::<u64>().unwrap_or(0) * 1024 * 1024
    } else if let Some(rest) = s.strip_suffix("Gi") {
        rest.parse::<u64>().unwrap_or(0) * 1024 * 1024 * 1024
    } else if let Some(rest) = s.strip_suffix("k") {
        rest.parse::<u64>().unwrap_or(0) * 1000
    } else if let Some(rest) = s.strip_suffix("M") {
        rest.parse::<u64>().unwrap_or(0) * 1000 * 1000
    } else if let Some(rest) = s.strip_suffix("G") {
        rest.parse::<u64>().unwrap_or(0) * 1000 * 1000 * 1000
    } else {
        s.parse::<u64>().unwrap_or(0)
    }
}

fn parse_cpu_quantity(q: &Quantity) -> (i64, i64) {
    let s = format!("{:?}", q);
    let s = s.trim();
    if let Some(rest) = s.strip_suffix('m') {
        let millicores = rest.parse::<i64>().unwrap_or(0);
        (millicores * 1000, 100_000)
    } else {
        let cores = s.parse::<f64>().unwrap_or(0.0);
        let quota = (cores * 100_000.0) as i64;
        (quota, 100_000)
    }
}
