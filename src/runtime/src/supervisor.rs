//! # Container Supervisor — Orchestrates Container Lifecycle
//!
//! Manages the running state of containers: spawn, stop, restart, log collection.
//! Unlike the old `ProcessSupervisor`, this uses composition over inheritance:
//! small focused types collaborate through the `RuntimeProvider` trait.
//!
//! ## Architecture
//!
//! The supervisor holds:
//! - `running: HashMap<String, RunningContainer>` — live containers
//! - `image_manager` — OCI image operations
//! - `cgroup_manager` — resource limits
//!
//! Spawn flow:
//! 1. Prepare rootfs (image pull + cache/copy)
//! 2. Create cgroup + apply resource limits
//! 3. Fork (double-fork for root, single-fork for userns)
//! 4. Parent: add to cgroup, spawn log tasks, build RunningContainer
//!
//! ## Functional Patterns
//!
//! - Environment merging is a pure function
//! - Rootfs preparation is a pure function
//! - Spawn steps compose via iterator chains

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use z8s_core::sys;
use super::cgroup::CgroupManager;
use super::health::{HealthChecker, HealthStatus, ProbeConfig};
use super::image::ImageManager;
use super::rootfs::{self, RootfsIsolation};
use super::spec::{ContainerConfig, ContainerSpec, ResolvedVolume};

// ── Types ──────────────────────────────────────────────────────────────────

/// Snapshot of a running container's state.
#[derive(Debug, Clone)]
pub struct ContainerInstance {
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub pid: Option<u32>,
    pub rootfs: String,
    pub started_at: Option<String>,
    pub env_vars: Vec<(String, String)>,
    pub published_ports: HashMap<u16, u16>,
    pub isolated_net: bool,
    pub pod_ip: Option<Ipv4Addr>,
    pub host_veth_ifindex: Option<u32>,
    pub run_as_user: Option<u32>,
    pub run_as_group: Option<u32>,
}

/// A container process with its state.
pub struct RunningContainer {
    pub instance: ContainerInstance,
    pub restart_count: u32,
    pub log_buffer: Arc<Mutex<Vec<String>>>,
    pub ready: Arc<AtomicBool>,
    pub healthy: Arc<Mutex<bool>>,
}

/// The main orchestrator for container lifecycle.
pub struct ContainerSupervisor {
    running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    image_manager: Arc<ImageManager>,
    cgroup_manager: Arc<CgroupManager>,
    restart_counts: Arc<Mutex<HashMap<String, u32>>>,
}

impl ContainerSupervisor {
    pub fn new(image_manager: Arc<ImageManager>, cgroup_manager: Arc<CgroupManager>) -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
            image_manager,
            cgroup_manager,
            restart_counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ── Pod Lifecycle ──────────────────────────────────────────────────────

    /// Start all containers in a pod.
    pub async fn start_pod_from_spec(&self, spec: &ContainerSpec) -> Result<()> {
        let pod_uid = &spec.pod_uid;
        let pod_name = &spec.pod_name;

        rootfs::cleanup_emptydir(pod_uid);

        if self.has_running_containers(pod_name).await {
            return Ok(());
        }

        let placeholders = self.insert_placeholders(spec).await;

        if let Err(e) = self.cgroup_manager.create_pod_cgroup(pod_uid) {
            self.remove_placeholders(&placeholders).await;
            return Err(e);
        }

        super::cgroup::apply_limits(&self.cgroup_manager, pod_uid, &spec.containers);

        match self.spawn_all_containers(spec, &placeholders).await {
            Ok(prepared) => {
                let mut running = self.running.lock().await;
                for (cid, rc) in prepared {
                    running.insert(cid, rc);
                }
                Ok(())
            }
            Err(e) => {
                self.remove_placeholders(&placeholders).await;
                Err(e)
            }
        }
    }

    /// Stop all containers in a pod.
    pub async fn stop_pod_from_spec(&self, spec: &ContainerSpec) {
        for cfg in &spec.containers {
            self.stop_container(&cfg.container_id).await;
            self.restart_counts.lock().await.remove(&cfg.container_id);
        }
        let cg = self.cgroup_manager.clone();
        let uid = spec.pod_uid.clone();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::task::spawn_blocking(move || {
                cg.remove_cgroup(&uid).ok();
                rootfs::cleanup_emptydir(&uid);
            }),
        )
        .await;
    }

    /// Stop a single container.
    pub async fn stop_container(&self, container_id: &str) {
        let rootfs = {
            let mut running = self.running.lock().await;
            let Some(rc) = running.remove(container_id) else { return };
            if let Some(pid) = rc.instance.pid {
                info!("Stopping container {} (PID {})", container_id, pid);
                let pgid = -(pid as i32);
                sys::kill(pgid, rustix::process::Signal::KILL).ok();
                sys::kill(pid as i32, rustix::process::Signal::KILL).ok();
            }
            rc.instance.rootfs.clone()
        };
        if rootfs.ends_with("/merged") {
            let _ = tokio::task::spawn_blocking(move || ImageManager::unmount_overlay(&rootfs)).await;
        }
    }

    // ── Query ──────────────────────────────────────────────────────────────

    pub async fn is_pod_alive(&self, pod_name: &str) -> bool {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        running.iter().any(|(cid, rc)| {
            cid.starts_with(&prefix) && rc.instance.pid.map(is_pid_alive).unwrap_or(false)
        })
    }

    pub async fn is_pod_ready(&self, pod_name: &str) -> bool {
        if !self.is_pod_alive(pod_name).await {
            return false;
        }
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        running.iter().any(|(cid, rc)| {
            cid.starts_with(&prefix) && rc.ready.load(Ordering::SeqCst)
        })
    }

    pub async fn backend_connect_port(&self, pod_name: &str, container_port: u16) -> u16 {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        for (cid, rc) in running.iter() {
            if cid.starts_with(&prefix) {
                if rc.instance.isolated_net {
                    return rc.instance.published_ports.get(&container_port).copied().unwrap_or(container_port);
                }
                return container_port;
            }
        }
        container_port
    }

    pub async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String> {
        let container_id = format!("{}-{}", pod_name, container_name);
        let running = self.running.lock().await;
        if let Some(rc) = running.get(&container_id) {
            rc.log_buffer.lock().await.clone()
        } else {
            Vec::new()
        }
    }

    pub async fn pod_restart_counts(&self, pod_name: &str) -> HashMap<String, u32> {
        let prefix = format!("{}-", pod_name);
        self.restart_counts.lock().await.iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, &v)| (k.strip_prefix(&prefix).unwrap_or(k).to_string(), v))
            .collect()
    }

    pub async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String> {
        self.image_manager.unpack_image(image_ref, container_id).await
    }

    pub fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String> {
        self.cgroup_manager.create_pod_cgroup(pod_uid)
    }

    pub fn remove_cgroup(&self, pod_uid: &str) -> Result<()> {
        self.cgroup_manager.remove_cgroup(pod_uid)
    }

    // ── Internal ───────────────────────────────────────────────────────────

    async fn has_running_containers(&self, pod_name: &str) -> bool {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        running.keys().any(|cid| cid.starts_with(&prefix))
    }

    async fn insert_placeholders(&self, spec: &ContainerSpec) -> Vec<String> {
        let mut running = self.running.lock().await;
        let ids: Vec<String> = spec.containers.iter().map(|c| c.container_id.clone()).collect();
        for cfg in &spec.containers {
            let cid = &cfg.container_id;
            if !running.contains_key(cid.as_str()) {
                running.insert(cid.clone(), RunningContainer {
                    instance: ContainerInstance {
                        container_id: cid.clone(),
                        container_name: cfg.container_name.clone(),
                        image: cfg.image.clone(),
                        pid: None,
                        rootfs: String::new(),
                        started_at: None,
                        env_vars: Vec::new(),
                        published_ports: HashMap::new(),
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
                });
            }
        }
        ids
    }

    async fn remove_placeholders(&self, placeholders: &[String]) {
        let mut r = self.running.lock().await;
        for cid in placeholders {
            r.remove(cid.as_str());
        }
    }

    async fn spawn_all_containers(
        &self,
        spec: &ContainerSpec,
        placeholders: &[String],
    ) -> Result<Vec<(String, RunningContainer)>> {
        let mut prepared = Vec::new();
        for cfg in &spec.containers {
            let rootfs_path = self.prepare_rootfs(spec, cfg).await?;
            match self.spawn_container(cfg, &rootfs_path, &spec.pod_uid, spec.subnet.as_deref()).await {
                Ok(rc) => prepared.push((cfg.container_id.clone(), rc)),
                Err(e) => {
                    self.remove_placeholders(placeholders).await;
                    self.cleanup_orphans(&prepared).await;
                    return Err(e.context(format!("Failed to spawn {}/{}", spec.pod_name, cfg.container_name)));
                }
            };
        }
        Ok(prepared)
    }

    async fn prepare_rootfs(&self, spec: &ContainerSpec, cfg: &ContainerConfig) -> Result<String> {
        if cfg.is_native {
            info!("Native process {}/{} (no OCI image)", spec.pod_name, cfg.container_name);
            return Ok(String::new());
        }
        self.image_manager.unpack_image(&cfg.image, &cfg.container_id).await
            .context(format!("Failed to prepare image {} for {}", cfg.image, cfg.container_name))
    }

    async fn spawn_container(
        &self,
        cfg: &ContainerConfig,
        rootfs_path: &str,
        pod_uid: &str,
        subnet: Option<&str>,
    ) -> Result<RunningContainer> {
        if cfg.is_native {
            return self.spawn_native(cfg).await;
        }
        self.spawn_isolated(cfg, rootfs_path, pod_uid, subnet).await
    }

    async fn spawn_native(&self, cfg: &ContainerConfig) -> Result<RunningContainer> {
        let mut child_cmd = tokio::process::Command::new(&cfg.entrypoint);
        child_cmd.args(&cfg.args);
        child_cmd
            .env_clear()
            .envs(cfg.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let child = child_cmd.spawn().context("Failed to spawn native container")?;
        let pid = child.id().context("Child exited before PID read")?;
        info!("Native container {} started with PID {}", cfg.container_id, pid);
        self.cgroup_manager.add_pid_to_cgroup(cfg.container_id.split('-').next().unwrap_or(""), pid)?;
        self.build_running_container(child, cfg, "", pid).await
    }

    async fn spawn_isolated(
        &self,
        cfg: &ContainerConfig,
        rootfs_path: &str,
        pod_uid: &str,
        _subnet: Option<&str>,
    ) -> Result<RunningContainer> {
        // Prepare rootfs
        rootfs::prepare_rootfs(rootfs_path)?;

        // Create pipes before fork so both parent and child can access them
        let sync_flags = rustix::pipe::PipeFlags::CLOEXEC;
        let (sync_r, sync_w) = sys::pipe2(sync_flags)?;
        let (ack_r, ack_w) = sys::pipe2(sync_flags)?;
        let (stdout_r, stdout_w) = sys::pipe2(sync_flags)?;
        let (stderr_r, stderr_w) = sys::pipe2(sync_flags)?;

        let entrypoint = cfg.entrypoint.clone();
        let args = cfg.args.clone();
        let env = merge_env(&cfg.env, rootfs_path);
        let rootfs_owned = rootfs_path.to_string();
        let container_id = cfg.container_id.clone();
        let hostname = cfg.container_id.rsplit_once('-').map_or(cfg.container_id.as_str(), |(pod, _)| pod);
        let volumes = cfg.volumes.clone();
        let isolate_net = cfg.isolated_net;
        let run_as_user = cfg.run_as_user;
        let run_as_group = cfg.run_as_group;
        let privileged = cfg.privileged;
        let extra_caps = cfg.extra_capabilities.clone();
        let cap_profile = cfg.cap_profile.clone();
        let is_native = cfg.is_native;
        let working_dir = cfg.working_dir.clone();
        let probes = cfg.probes.clone();

        match sys::fork() {
            Ok(sys::ForkResult::Child) => {
                // ── Child process ──────────────────────────────────────
                drop(sync_r);
                drop(ack_w);
                let _ = sys::setsid();

                // Determine isolation strategy
                if rootfs::is_root() {
                    // Root: double-fork with PID namespace
                    self.spawn_root_ns_child(
                        &entrypoint, &args, &env, &rootfs_owned, hostname,
                        &volumes, isolate_net, run_as_user, run_as_group,
                        privileged, &extra_caps, cap_profile.as_deref(), is_native,
                        &working_dir, sync_w, ack_r, stdout_w, stderr_w,
                    );
                } else {
                    // Rootless: user namespace + chroot
                    self.spawn_userns_child(
                        &entrypoint, &args, &env, &rootfs_owned, hostname,
                        &volumes, isolate_net, run_as_user, run_as_group,
                        privileged, &extra_caps, cap_profile.as_deref(), is_native,
                        &working_dir, sync_w, ack_r, stdout_w, stderr_w,
                    );
                }
                unreachable!("child should have exec'd");
            }
            Ok(sys::ForkResult::Parent(child_pid)) => {
                // ── Parent process ─────────────────────────────────────
                drop(sync_w);
                drop(ack_r);

                // Wait for child to signal namespace setup
                let mut sync_buf = [0u8; 1];
                let n = sys::read_fd(&sync_r, &mut sync_buf).context("Failed to read sync")?;
                anyhow::ensure!(n == 1 && sync_buf[0] == b'S', "Child died before namespace setup");

                // Write userns maps if needed
                if !rootfs::is_root() {
                    write_userns_maps(child_pid as i32, run_as_user, run_as_group)?;
                }

                // Signal child to continue
                sys::write_fd(&ack_w, b"A").ok();
                drop(sync_r);
                drop(ack_w);

                info!("Container {} started with PID {}", container_id, child_pid);
                self.cgroup_manager.add_pid_to_cgroup(pod_uid, child_pid)?;

                // Build log reader handles
                let log_buffer = spawn_log_tasks(stdout_r, stderr_r);

                let instance = ContainerInstance {
                    container_id: container_id.clone(),
                    container_name: cfg.container_name.clone(),
                    image: cfg.image.clone(),
                    pid: Some(child_pid),
                    rootfs: rootfs_owned,
                    started_at: Some(z8s_core::types::helpers::now_rfc3339()),
                    env_vars: env.clone(),
                    published_ports: cfg.published_ports.clone(),
                    isolated_net: cfg.isolated_net,
                    pod_ip: None,
                    host_veth_ifindex: None,
                    run_as_user,
                    run_as_group,
                };

                let (ready, healthy) = spawn_container_probes(&probes, &container_id, &cfg.published_ports);
                Ok(RunningContainer {
                    instance,
                    restart_count: 0,
                    log_buffer,
                    ready,
                    healthy,
                })
            }
            Err(e) => Err(anyhow::anyhow!("Failed to fork: {}", e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_root_ns_child(
        &self,
        entrypoint: &str, args: &[String], env: &[(String, String)],
        rootfs_path: &str, hostname: &str, volumes: &[ResolvedVolume],
        isolate_net: bool, run_as_user: Option<u32>, run_as_group: Option<u32>,
        privileged: bool, extra_caps: &[String], cap_profile: Option<&str>,
        _is_native: bool, working_dir: &Option<String>,
        sync_w: OwnedFd, ack_r: OwnedFd,
        stdout_w: OwnedFd, stderr_w: OwnedFd,
    ) {
        if let Err(e) = rootfs::unshare_container_ns(isolate_net, hostname, true) {
            error!("z8s: namespace setup failed: {}", e);
            std::process::exit(1);
        }

        // Fork grandchild into new PID namespace
        match sys::fork() {
            Ok(sys::ForkResult::Child) => {
                // Grandchild: setup rootfs + exec
                drop(sync_w);
                drop(ack_r);
                setup_child_pipes(stdout_w, stderr_w);

                let isolation = match rootfs::setup_container_rootfs(rootfs_path, volumes) {
                    Ok(i) => i,
                    Err(e) => {
                        error!("z8s: rootfs setup failed: {}", e);
                        std::process::exit(1);
                    }
                };

                child_setup_privileges(run_as_group, run_as_user, working_dir, privileged, extra_caps, isolation, skip_landlock(privileged, cap_profile));
                let (exec_path, prog_args) = argv_for_isolation(entrypoint, args, rootfs_path, isolation);
                execvpe_container(&exec_path, &prog_args, env, rootfs_path, isolation);
            }
            Ok(sys::ForkResult::Parent(_grandchild_pid)) => {
                // Intermediate child: write grandchild PID, exit
                std::process::exit(0);
            }
            Err(e) => {
                error!("z8s: second fork failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_userns_child(
        &self,
        entrypoint: &str, args: &[String], env: &[(String, String)],
        rootfs_path: &str, hostname: &str, volumes: &[ResolvedVolume],
        isolate_net: bool, run_as_user: Option<u32>, run_as_group: Option<u32>,
        privileged: bool, extra_caps: &[String], cap_profile: Option<&str>,
        _is_native: bool, working_dir: &Option<String>,
        sync_w: OwnedFd, ack_r: OwnedFd,
        stdout_w: OwnedFd, stderr_w: OwnedFd,
    ) {
        let isolation = match rootfs::child_enter_ns_fork(rootfs_path, sync_w, ack_r, volumes, isolate_net, hostname) {
            Ok(i) => i,
            Err(e) => {
                error!("z8s: namespace setup failed: {}", e);
                std::process::exit(1);
            }
        };

        setup_child_pipes(stdout_w, stderr_w);
        child_setup_privileges(run_as_group, run_as_user, working_dir, privileged, extra_caps, isolation, skip_landlock(privileged, cap_profile));
        let (exec_path, prog_args) = argv_for_isolation(entrypoint, args, rootfs_path, isolation);
        execvpe_container(&exec_path, &prog_args, env, rootfs_path, isolation);
    }

    async fn cleanup_orphans(&self, prepared: &[(String, RunningContainer)]) {
        for (cid, rc) in prepared {
            if let Some(pid) = rc.instance.pid {
                info!("Cleaning up orphan {} (PID {})", cid, pid);
                sys::kill(pid as i32, rustix::process::Signal::TERM).ok();
            }
        }
    }

    async fn build_running_container(
        &self,
        mut child: tokio::process::Child,
        cfg: &ContainerConfig,
        rootfs_path: &str,
        pid: u32,
    ) -> Result<RunningContainer> {
        let log_buffer = Arc::new(Mutex::new(Vec::<String>::new()));
        if let Some(stdout) = child.stdout.take() {
            let buf = log_buffer.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut log = buf.lock().await;
                    log.push(format!("[stdout] {}", line));
                    if log.len() > 1000 { log.remove(0); }
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
                    if log.len() > 1000 { log.remove(0); }
                }
            });
        }

        let instance = ContainerInstance {
            container_id: cfg.container_id.clone(),
            container_name: cfg.container_name.clone(),
            image: cfg.image.clone(),
            pid: Some(pid),
            rootfs: rootfs_path.to_string(),
            started_at: Some(z8s_core::types::helpers::now_rfc3339()),
            env_vars: cfg.env.clone(),
            published_ports: cfg.published_ports.clone(),
            isolated_net: cfg.isolated_net,
            pod_ip: None,
            host_veth_ifindex: None,
            run_as_user: cfg.run_as_user,
            run_as_group: cfg.run_as_group,
        };

        let (ready, healthy) = spawn_container_probes(&cfg.probes, &cfg.container_id, &cfg.published_ports);
        Ok(RunningContainer {
            instance,
            restart_count: 0,
            log_buffer,
            ready,
            healthy,
        })
    }
}

// ── Pure Functions ─────────────────────────────────────────────────────────

/// Merge spec env vars with OCI image env vars. Spec takes precedence.
pub fn merge_env(env_vars: &[(String, String)], rootfs: &str) -> Vec<(String, String)> {
    let oci_env = super::image::read_image_config(rootfs)
        .env
        .unwrap_or_default();
    let mut result: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (k, v) in env_vars {
        if seen.insert(k.clone()) {
            result.push((k.clone(), v.clone()));
        }
    }
    for entry in &oci_env {
        if let Some(eq) = entry.find('=') {
            let key = entry[..eq].to_string();
            let val = entry[eq + 1..].to_string();
            if seen.insert(key.clone()) {
                result.push((key, val));
            }
        }
    }
    result
}

/// Check if a PID is alive.
/// Sends a harmless signal — if ESRCH, process doesn't exist.
pub fn is_pid_alive(pid: u32) -> bool {
    match rustix::process::Pid::from_raw(pid as i32) {
        Some(p) => {
            // Any signal works: ESRCH = not found, EPERM = exists but no perms, Ok = exists
    !matches!(rustix::process::kill_process(p, rustix::process::Signal::CONT), Err(rustix::io::Errno::SRCH))
        }
        None => false,
    }
}

/// Write userns UID/GID maps for a child process.
fn write_userns_maps(child_pid: i32, run_as_user: Option<u32>, run_as_group: Option<u32>) -> Result<()> {
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();

    // Try newuidmap/newgidmap first
    if try_newid_maps(child_pid, uid, gid).is_ok() {
        info!("Wrote userns maps via newuidmap/newgidmap for pid {}", child_pid);
        return Ok(());
    }

    // Try direct subuid/subgid ranges
    if try_write_subid_maps_direct(child_pid, uid, gid).is_ok() {
        info!("Wrote userns maps via /proc uid_map for pid {}", child_pid);
        return Ok(());
    }

    // Fallback: single UID mapping
    sys::write_setgroups(child_pid, "deny")?;
    let map_uid = run_as_user.unwrap_or(0);
    let map_gid = run_as_group.unwrap_or(map_uid);
    sys::write_uid_map(child_pid, &format!("{} {} 1\n", map_uid, uid))?;
    sys::write_gid_map(child_pid, &format!("{} {} 1\n", map_gid, gid))?;
    info!("Wrote single UID/GID map for pid {}", child_pid);
    Ok(())
}

fn try_newid_maps(child_pid: i32, uid: u32, gid: u32) -> Result<()> {
    let uid_args = build_idmap_args(child_pid, uid, "/etc/subuid")?;
    let gid_args = build_idmap_args(child_pid, gid, "/etc/subgid")?;
    let ok = std::process::Command::new(idmap_bin("newuidmap")).args(&uid_args).status()?.success();
    anyhow::ensure!(ok, "newuidmap failed");
    let ok = std::process::Command::new(idmap_bin("newgidmap")).args(&gid_args).status()?.success();
    anyhow::ensure!(ok, "newgidmap failed");
    Ok(())
}

fn try_write_subid_maps_direct(child_pid: i32, uid: u32, gid: u32) -> Result<()> {
    let (uid_start, uid_count) = read_subid("/etc/subuid", uid)
        .ok_or_else(|| anyhow::anyhow!("no subuid entry for uid {}", uid))?;
    let (gid_start, gid_count) = read_subid("/etc/subgid", gid)
        .ok_or_else(|| anyhow::anyhow!("no subgid entry for gid {}", gid))?;
    sys::write_setgroups(child_pid, "deny")?;
    sys::write_uid_map(child_pid, &format!("0 {} 1\n1 {} {}\n", uid, uid_start, uid_count))?;
    sys::write_gid_map(child_pid, &format!("0 {} 1\n1 {} {}\n", gid, gid_start, gid_count))?;
    Ok(())
}

fn build_idmap_args(child_pid: i32, host_id: u32, subid_file: &str) -> Result<Vec<String>> {
    let mut args = vec![child_pid.to_string(), "0".to_string(), host_id.to_string(), "1".to_string()];
    if let Some((start, count)) = read_subid(subid_file, host_id) {
        args.extend(["1".to_string(), start.to_string(), count.to_string()]);
    }
    Ok(args)
}

fn read_subid(path: &str, host_id: u32) -> Option<(u64, u64)> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let mut parts = line.splitn(3, ':');
        let id_field = parts.next()?;
        let start: u64 = parts.next()?.parse().ok()?;
        let count: u64 = parts.next()?.parse().ok()?;
        if id_field.parse::<u32>().is_ok_and(|id| id == host_id) {
            return Some((start, count));
        }
    }
    None
}

fn idmap_bin(name: &str) -> String {
    for path in [format!("/usr/bin/{name}"), format!("/bin/{name}")] {
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    name.to_string()
}

/// Spawn stdout/stderr log collection tasks.
fn spawn_log_tasks(stdout_r: OwnedFd, stderr_r: OwnedFd) -> Arc<Mutex<Vec<String>>> {
    let log_buffer = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let buf = log_buffer.clone();
        let file = tokio::fs::File::from_std(std::fs::File::from(stdout_r));
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(file).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut log = buf.lock().await;
                log.push(format!("[stdout] {}", line));
                if log.len() > 1000 { log.remove(0); }
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
                if log.len() > 1000 { log.remove(0); }
            }
        });
    }
    log_buffer
}

/// Start probe loops for a container.
fn spawn_container_probes(
    probes: &[ProbeConfig],
    container_id: &str,
    port_map: &HashMap<u16, u16>,
) -> (Arc<AtomicBool>, Arc<Mutex<bool>>) {
    let ready = Arc::new(AtomicBool::new(probes.is_empty()));
    let healthy = Arc::new(Mutex::new(true));
    if probes.is_empty() {
        return (ready, healthy);
    }

    let p_ready = ready.clone();
    let p_healthy = healthy.clone();
    let cid = container_id.to_string();
    let probes_owned = probes.to_vec();
    let port_map = port_map.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut next_fire: Vec<(Instant, usize)> = probes_owned
            .iter()
            .enumerate()
            .map(|(i, p)| (Instant::now() + std::time::Duration::from_secs(p.initial_delay_seconds.max(0) as u64), i))
            .collect();

        loop {
            interval.tick().await;
            let now = Instant::now();
            let due: Vec<usize> = next_fire.iter().filter(|(t, _)| *t <= now).map(|(_, i)| *i).collect();
            if due.is_empty() { continue; }

            let mut all_ok = true;
            for i in due {
                let config = &probes_owned[i];
                let status = HealthChecker::run(config, &port_map).await;
                let ok = matches!(status, HealthStatus::Healthy);
                all_ok &= ok;
                if !ok { warn!("Probe {} for {} failed", i, cid); }
                if let Some(slot) = next_fire.iter_mut().find(|(_, idx)| *idx == i) {
                    slot.0 = now + std::time::Duration::from_secs(config.period_seconds.max(1) as u64);
                }
            }
            p_ready.store(all_ok, Ordering::SeqCst);
            *p_healthy.lock().await = all_ok;
        }
    });

    (ready, healthy)
}

// ── Child-Side Helpers ────────────────────────────────────────────────────

fn setup_child_pipes(stdout_w: OwnedFd, stderr_w: OwnedFd) {
    let _ = sys::dup2_stdout(&stdout_w);
    let _ = sys::dup2_stderr(&stderr_w);
    drop(stdout_w);
    drop(stderr_w);
    if let Ok(fd) = sys::open("/dev/null", rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty()) {
        let _ = sys::dup2_stdin(&fd);
    }
}

fn child_setup_privileges(
    run_as_group: Option<u32>,
    run_as_user: Option<u32>,
    working_dir: &Option<String>,
    privileged: bool,
    extra_caps: &[String],
    isolation: RootfsIsolation,
    skip_landlock: bool,
) {
    if let Some(gid) = run_as_group {
        z8s_core::sys::setgid(gid).ok();
    }
    if let Some(uid) = run_as_user {
        z8s_core::sys::setuid(uid).ok();
    }
    if let Some(wd) = working_dir {
        sys::chdir(wd).ok();
    }
    rootfs::drop_capabilities(privileged, extra_caps);
    if isolation != RootfsIsolation::Degraded && !skip_landlock {
        rootfs::apply_landlock();
    }
}

fn argv_for_isolation(
    entrypoint: &str,
    args: &[String],
    rootfs_host_path: &str,
    isolation: RootfsIsolation,
) -> (String, Vec<String>) {
    if isolation == RootfsIsolation::Degraded {
        rootfs::build_container_argv(entrypoint, args, rootfs_host_path)
    } else {
        rootfs::build_container_argv_in_mount_ns(entrypoint, args, rootfs_host_path)
    }
}

fn execvpe_container(
    exec_path: &str,
    prog_args: &[String],
    env_owned: &[(String, String)],
    rootfs_host_path: &str,
    isolation: RootfsIsolation,
) -> ! {
    let envp: Vec<std::ffi::CString> = env_owned
        .iter()
        .map(|(k, v)| std::ffi::CString::new(format!("{}={}", k, v)).expect("env cannot contain null"))
        .collect();

    let mut argv: Vec<std::ffi::CString> = std::iter::once(
        std::ffi::CString::new(exec_path).expect("exec path cannot contain null"),
    )
    .chain(prog_args.iter().map(|a| {
        std::ffi::CString::new(a.as_str()).expect("arg cannot contain null")
    }))
    .collect();

    if isolation == RootfsIsolation::Degraded {
        let (loader, args) = rootfs::wrap_dynamic_linker(exec_path, prog_args.to_vec(), rootfs_host_path);
        argv = std::iter::once(std::ffi::CString::new(loader).expect("loader cannot contain null"))
            .chain(args.into_iter().map(|a| std::ffi::CString::new(a).expect("arg cannot contain null")))
            .collect();
    }

    let cstr_argv: Vec<*const std::ffi::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
    let cstr_envp: Vec<*const std::ffi::c_char> = envp.iter().map(|c| c.as_ptr()).collect();

    let _ = z8s_core::sys::execve(&argv[0], &cstr_argv, &cstr_envp);
    error!("z8s: execve({}) failed", argv[0].to_str().unwrap_or("?"));
    std::process::exit(1);
}

fn skip_landlock(privileged: bool, cap_profile: Option<&str>) -> bool {
    if privileged {
        return true;
    }
    matches!(cap_profile, Some("host-native") | Some("host-dhcp") | Some("host-sshd") | Some("privileged"))
}

// ── RuntimeProvider Implementation ─────────────────────────────────────────

#[async_trait::async_trait]
impl super::RuntimeProvider for ContainerSupervisor {
    async fn start_pod(&self, spec: &ContainerSpec) -> anyhow::Result<()> {
        self.start_pod_from_spec(spec).await
    }

    async fn stop_pod(&self, spec: &ContainerSpec) -> anyhow::Result<()> {
        self.stop_pod_from_spec(spec).await;
        Ok(())
    }

    async fn stop_container(&self, container_id: &str) -> anyhow::Result<()> {
        self.stop_container(container_id).await;
        Ok(())
    }

    async fn is_pod_alive(&self, pod_name: &str) -> bool {
        self.is_pod_alive(pod_name).await
    }

    async fn is_pod_ready(&self, pod_name: &str) -> bool {
        self.is_pod_ready(pod_name).await
    }

    async fn backend_connect_port(&self, pod_name: &str, port: u16) -> u16 {
        self.backend_connect_port(pod_name, port).await
    }

    async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String> {
        self.get_container_logs(pod_name, container_name).await
    }

    async fn pod_restart_counts(&self, pod_name: &str) -> std::collections::HashMap<String, u32> {
        self.pod_restart_counts(pod_name).await
    }

    async fn unpack_image(&self, image_ref: &str, container_id: &str) -> anyhow::Result<String> {
        self.unpack_image(image_ref, container_id).await
    }

    fn create_pod_cgroup(&self, pod_uid: &str) -> anyhow::Result<String> {
        self.create_pod_cgroup(pod_uid)
    }

    fn remove_cgroup(&self, pod_uid: &str) -> anyhow::Result<()> {
        self.remove_cgroup(pod_uid)
    }
}
