use crate::cri::RuntimeProvider;
use crate::cri::cgroup::CgroupManager;
use crate::cri::health::ProbeConfig;
use crate::cri::image::ImageManager;
use crate::cri::rootfs;
use crate::cri::spec::ContainerSpec;
use anyhow::{Context, Result};
use async_trait::async_trait;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info, warn};


use crate::cri::spawn::{ContainerSpawnCtx, SpawnPipeline, SpawnState, StdPipes, create_std_pipes};

#[derive(Debug, Clone)]
pub struct ContainerInstance {
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub pid: Option<u32>,
    pub rootfs: String,
    pub started_at: Option<String>,
    pub env_vars: Vec<(String, String)>,
    /// container_port → 127.0.0.1 host port (pod network namespace publish)
    pub published_ports: std::collections::HashMap<u16, u16>,
    /// Pod has its own network namespace (declared containerPorts).
    pub isolated_net: bool,
    /// Pod IP allocated from the pool (NetMux).
    pub pod_ip: Option<std::net::Ipv4Addr>,
    /// Host veth ifindex for this pod (NetMux).
    pub host_veth_ifindex: Option<u32>,
    pub run_as_user: Option<u32>,
    pub run_as_group: Option<u32>,
}

#[derive(Debug)]
pub struct RunningContainer {
    pub child: Option<Child>,
    pub instance: ContainerInstance,
    pub restart_count: u32,
    pub log_buffer: Arc<Mutex<Vec<String>>>,
    pub ready: Arc<AtomicBool>,
    pub healthy: Arc<Mutex<bool>>,
}

pub struct ProcessSupervisor {
    pub running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    pub image_manager: Arc<ImageManager>,
    pub cgroup_manager: Arc<CgroupManager>,
    pub restart_counts: Arc<Mutex<HashMap<String, u32>>>,
    pub netmux: Arc<crate::netmux::NetMux>,
}

impl ProcessSupervisor {
    pub fn new(
        image_manager: Arc<ImageManager>,
        cgroup_manager: Arc<CgroupManager>,
        netmux: Arc<crate::netmux::NetMux>,
    ) -> Self {
        let base = if rootfs::is_root() {
            "/var/lib/z8s".to_string()
        } else {
            format!(
                "{}/.local/share/z8s",
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
            )
        };
        std::fs::create_dir_all(format!("{}/containers", base)).ok();
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
            image_manager,
            cgroup_manager,
            restart_counts: Arc::new(Mutex::new(HashMap::new())),
            netmux,
        }
    }

    // ── ContainerSpec-based spawn pipeline (§7.5) ──────────────────────────

    async fn spawn_container_from_config(
        &self,
        cfg: &crate::cri::spec::ContainerConfig,
        rootfs_path: &str,
        pod_uid: &str,
        subnet: Option<String>,
    ) -> Result<RunningContainer> {
        let resolved =
            crate::cri::spawn::entrypoint::resolve_entrypoint(cfg, rootfs_path);
        let entrypoint = resolved.entrypoint;
        let cmd_args = resolved.args;
        let working_dir = resolved.working_dir;
        let image = &cfg.image;
        let is_native = cfg.is_native;
        let env_vars = &cfg.env;
        let volumes = &cfg.volumes;
        let run_as_user = cfg.run_as_user;
        let run_as_group = cfg.run_as_group;
        let isolate_net = cfg.isolated_net;
        let privileged = cfg.privileged;
        let extra_caps = cfg.extra_capabilities.clone();
        let container_id = &cfg.container_id;
        let published_ports_data = cfg.published_ports.values().copied().collect::<Vec<u16>>();

        let mut child_cmd = if is_native {
            let mut c = Command::new(&entrypoint);
            c.args(&cmd_args);
            c
        } else {
            let rootfs_owned = rootfs_path.to_string();

            let ctx = ContainerSpawnCtx {
                entrypoint: &entrypoint,
                cmd_args: &cmd_args,
                env_vars,
                rootfs_path: &rootfs_owned,
                container_id,
                pod_uid,
                image,
                container_name: &cfg.container_name,
                volumes: volumes.clone(),
                run_as_user,
                run_as_group,
                isolate_net,
                privileged,
                cap_profile: cfg.cap_profile.as_deref(),
                is_native: cfg.is_native,
                extra_caps: extra_caps.clone(),
                working_dir: working_dir.clone(),
                probes: cfg.probes.clone(),
                subnet,
            };
            return SpawnPipeline::isolated_default()
                .finish_isolated(self, SpawnState::new(ctx))
                .await;
        };

        child_cmd
            .env_clear()
            .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = child_cmd
            .spawn()
            .context("Failed to spawn container process")?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Child process exited before PID was read"))?;
        info!("Container {} started with PID {}", container_id, pid);

        self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

        self.build_running_container(
            child,
            container_id,
            rootfs_path,
            image,
            &cfg.container_name,
            env_vars,
            isolate_net,
            &published_ports_data,
            &cfg.probes,
        )
        .await
    }

    pub async fn start_pod_from_spec(&self, spec: &crate::cri::spec::ContainerSpec) -> Result<()> {
        let pod_uid = &spec.pod_uid;
        let pod_name = &spec.pod_name;

        crate::cri::volumes::cleanup_emptydir(pod_uid);

        if self.check_duplicate_start(pod_name).await {
            return Ok(());
        }

        let placeholders = self.insert_placeholders(spec).await;

        info!(
            "Starting pod {} ({} container(s))",
            pod_name,
            spec.containers.len()
        );

        if let Err(e) = self.cgroup_manager.create_pod_cgroup(pod_uid) {
            Self::remove_placeholders(&self.running, &placeholders).await;
            return Err(e);
        }

        self.apply_resource_limits(pod_uid, spec);
        let prepared = self.spawn_all_containers(spec, &placeholders).await?;

        {
            let mut running = self.running.lock().await;
            for (cid, rc) in prepared {
                running.insert(cid, rc);
            }
        }
        Ok(())
    }

    async fn check_duplicate_start(&self, pod_name: &str) -> bool {
        let running = self.running.lock().await;
        running
            .keys()
            .any(|cid| cid.starts_with(&format!("{}-", pod_name)))
    }

    async fn insert_placeholders(&self, spec: &crate::cri::spec::ContainerSpec) -> Vec<String> {
        let mut running = self.running.lock().await;
        let ids: Vec<String> = spec
            .containers
            .iter()
            .map(|c| c.container_id.clone())
            .collect();
        for cfg in &spec.containers {
            let cid = &cfg.container_id;
            if !running.contains_key(cid.as_str()) {
                running.insert(
                    cid.clone(),
                    RunningContainer {
                        child: None,
                        instance: ContainerInstance {
                            container_id: cid.clone(),
                            container_name: cfg.container_name.clone(),
                            image: cfg.image.clone(),
                            pid: None,
                            rootfs: String::new(),
                            started_at: None,
                            env_vars: Vec::new(),
                            published_ports: std::collections::HashMap::new(),
                            isolated_net: false,
                            pod_ip: None,
                            host_veth_ifindex: None,
                            run_as_user: None,
                            run_as_group: None,
                        },
                        restart_count: 0,
                        log_buffer: Arc::new(Mutex::new(Vec::new())),
                        ready: Arc::new(AtomicBool::new(false)),
                        healthy: Arc::new(Mutex::new(true)),
                    },
                );
            }
        }
        ids
    }

    fn apply_resource_limits(&self, pod_uid: &str, spec: &crate::cri::spec::ContainerSpec) {
        for cfg in &spec.containers {
            if let Some(limit) = cfg.memory_limit_bytes {
                if limit > 0 {
                    self.cgroup_manager.set_memory_limit(pod_uid, limit).ok();
                }
            }
            if let Some(low) = cfg.memory_low_bytes {
                if low > 0 {
                    self.cgroup_manager.set_memory_low(pod_uid, low).ok();
                }
            }
            if let Some(quota) = cfg.cpu_quota {
                if let Some(period) = cfg.cpu_period {
                    if quota > 0 && period > 0 {
                        self.cgroup_manager
                            .set_cpu_limit(pod_uid, quota, period)
                            .ok();
                    }
                }
            }
        }
    }

    async fn remove_placeholders(
        running: &Arc<Mutex<HashMap<String, RunningContainer>>>,
        placeholders: &[String],
    ) {
        let mut r = running.lock().await;
        for cid in placeholders {
            r.remove(cid.as_str());
        }
    }

    async fn spawn_all_containers(
        &self,
        spec: &crate::cri::spec::ContainerSpec,
        placeholders: &[String],
    ) -> Result<Vec<(String, RunningContainer)>> {
        let mut prepared: Vec<(String, RunningContainer)> = Vec::new();
        for cfg in &spec.containers {
            let rootfs_path = self.prepare_rootfs(spec, cfg).await?;
            let rc = match self
                .spawn_container_from_config(cfg, &rootfs_path, &spec.pod_uid, spec.subnet.clone())
                .await
            {
                Ok(rc) => rc,
                Err(e) => {
                    Self::remove_placeholders(&self.running, placeholders).await;
                    Self::cleanup_orphan_containers(&prepared).await;
                    return Err(e.context(format!(
                        "Failed to spawn container {}/{}",
                        spec.pod_name, cfg.container_name
                    )));
                }
            };
            prepared.push((cfg.container_id.clone(), rc));
        }
        Ok(prepared)
    }

    async fn prepare_rootfs(
        &self,
        spec: &crate::cri::spec::ContainerSpec,
        cfg: &crate::cri::spec::ContainerConfig,
    ) -> Result<String> {
        if cfg.is_native {
            info!(
                "Native process {}/{} (no OCI image)",
                spec.pod_name, cfg.container_name
            );
            return Ok(String::new());
        }
        self.image_manager
            .unpack_image(&cfg.image, &cfg.container_id)
            .await
            .context(format!(
                "Failed to prepare image {} for {}",
                cfg.image, cfg.container_name
            ))
    }

    async fn cleanup_orphan_containers(prepared: &[(String, RunningContainer)]) {
        for (cid, rc) in prepared {
            if let Some(pid) = rc.instance.pid {
                info!("Cleaning up orphan container {} (PID {})", cid, pid);
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
        }
    }

    async fn build_running_container(
        &self,
        mut child: Child,
        container_id: &str,
        rootfs_path: &str,
        image: &str,
        container_name: &str,
        env_vars: &[(String, String)],
        isolate_net: bool,
        published_ports: &[u16],
        probes: &[ProbeConfig],
    ) -> Result<RunningContainer> {
        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Child process exited before PID was read"))?;

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

        let instance = Self::build_container_instance(
            container_id,
            container_name,
            image,
            pid,
            rootfs_path,
            env_vars.to_vec(),
            isolate_net,
            None,
            None,
            None,
            None,
        );

        Ok(Self::build_running_from_instance(
            instance, log_buffer, probes,
        ))
    }

    /// Port the service proxy should dial on 127.0.0.1 for this pod.
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

    pub async fn stop_container(&self, container_id: &str) {
        let rootfs = {
            let mut running = self.running.lock().await;
            let Some(rc) = running.remove(container_id) else {
                return;
            };
            if let Some(pid) = rc.instance.pid {
                info!("Stopping container {} (PID {})", container_id, pid);
                // Container is PID 1 in its own PID namespace. SIGTERM from the ancestor
                // namespace is blocked unless the process registered a handler, so use
                // SIGKILL directly (always delivered to PID 1 from ancestor ns).
                // Kill the entire process group (container is session leader via setsid())
                let pgid = nix::unistd::Pid::from_raw(-(pid as i32));
                let _ = kill(pgid, Signal::SIGKILL);
                // Also kill the main PID in case the pgid kill missed it
                let _ = kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGKILL);
            }
            rc.instance.rootfs.clone()
        };
        if rootfs.ends_with("/merged") {
            let _ = tokio::task::spawn_blocking(move || {
                crate::cri::image::ImageManager::unmount_overlay(&rootfs);
            })
            .await;
        }
    }

    pub async fn stop_pod_from_spec(&self, spec: &crate::cri::spec::ContainerSpec) {
        for cfg in &spec.containers {
            {
                let running = self.running.lock().await;
                if let Some(rc) = running.get(&cfg.container_id) {
                    if let (Some(ip), Some(ifindex)) =
                        (rc.instance.pod_ip, rc.instance.host_veth_ifindex)
                    {
                        if let Err(e) = self.netmux.detach_pod(&spec.pod_uid, &ip, ifindex) {
                            warn!("NetMux detach failed for {}: {}", spec.pod_uid, e);
                        }
                    }
                }
            }
            self.stop_container(&cfg.container_id).await;
            self.restart_counts.lock().await.remove(&cfg.container_id);
        }
        // Run filesystem operations in spawn_blocking: on overlayfs in container
        // environments, statx/lookup can enter D-state. If that happens on the async
        // thread, the entire runtime stalls (timeout wrappers can't fire).
        // spawn_blocking isolates D-state to a dedicated thread — the async thread
        // stays responsive and the timeout will fire after 3s, detaching the stuck thread.
        let cg = self.cgroup_manager.clone();
        let uid = spec.pod_uid.clone();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::task::spawn_blocking(move || {
                cg.remove_cgroup(&uid).ok();
                crate::cri::volumes::cleanup_emptydir(&uid);
            }),
        )
        .await;
    }

    pub fn is_pid_alive(pid: u32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }

    pub async fn is_pod_alive(&self, pod_name: &str) -> bool {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        running.iter().any(|(cid, rc)| {
            cid.starts_with(&prefix) && rc.instance.pid.map(Self::is_pid_alive).unwrap_or(false)
        })
    }

    pub async fn is_pod_ready(&self, pod_name: &str) -> bool {
        if !self.is_pod_alive(pod_name).await {
            return false;
        }
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        for (cid, rc) in running.iter() {
            if cid.starts_with(&prefix) && rc.ready.load(Ordering::SeqCst) {
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

    /// Returns restart count per container name for a given pod (strips the pod-name prefix).
    pub async fn pod_restart_counts(
        &self,
        pod_name: &str,
    ) -> std::collections::HashMap<String, u32> {
        let prefix = format!("{}-", pod_name);
        self.restart_counts
            .lock()
            .await
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, &v)| {
                let cname = k.strip_prefix(&prefix).unwrap_or(k.as_str()).to_string();
                (cname, v)
            })
            .collect()
    }
}

// ── ContainerRuntime wrapper (implements RuntimeProvider) ────────────────────

pub struct ContainerRuntime {
    pub supervisor: Arc<ProcessSupervisor>,
    pub cgroup_manager: Arc<CgroupManager>,
}

impl ContainerRuntime {
    pub fn new(supervisor: Arc<ProcessSupervisor>, cgroup_manager: Arc<CgroupManager>) -> Self {
        Self {
            supervisor,
            cgroup_manager,
        }
    }

    pub fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String> {
        self.cgroup_manager.create_pod_cgroup(pod_uid)
    }

    pub fn remove_cgroup(&self, pod_uid: &str) -> Result<()> {
        self.cgroup_manager.remove_cgroup(pod_uid)
    }
}

#[async_trait]
impl RuntimeProvider for ContainerRuntime {
    async fn start_pod(&self, spec: &ContainerSpec) -> Result<()> {
        info!(
            "CRI: starting pod {} (namespace={})",
            spec.pod_name, spec.namespace
        );
        self.supervisor.start_pod_from_spec(spec).await
    }

    async fn stop_pod(&self, spec: &ContainerSpec) -> Result<()> {
        info!("CRI: stopping pod {}", spec.pod_name);
        self.supervisor.stop_pod_from_spec(spec).await;
        Ok(())
    }

    async fn stop_container(&self, container_id: &str) -> Result<()> {
        self.supervisor.stop_container(container_id).await;
        Ok(())
    }

    async fn is_pod_alive(&self, pod_name: &str) -> bool {
        self.supervisor.is_pod_alive(pod_name).await
    }

    async fn is_pod_ready(&self, pod_name: &str) -> bool {
        self.supervisor.is_pod_ready(pod_name).await
    }

    async fn backend_connect_port(&self, pod_name: &str, port: u16) -> u16 {
        self.supervisor.backend_connect_port(pod_name, port).await
    }

    async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String> {
        self.supervisor
            .get_container_logs(pod_name, container_name)
            .await
    }

    async fn pod_restart_counts(&self, pod_name: &str) -> HashMap<String, u32> {
        self.supervisor.pod_restart_counts(pod_name).await
    }

    async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String> {
        self.supervisor
            .image_manager
            .unpack_image(image_ref, container_id)
            .await
    }

    fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String> {
        self.cgroup_manager.create_pod_cgroup(pod_uid)
    }

    fn remove_cgroup(&self, pod_uid: &str) -> Result<()> {
        self.cgroup_manager.remove_cgroup(pod_uid)
    }
}
