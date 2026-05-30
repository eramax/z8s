use crate::cri::cgroup::CgroupManager;
use crate::cri::health::{HealthChecker, HealthStatus, ProbeAction, ProbeConfig};
use crate::cri::image::ImageManager;
use crate::cri::rootfs;
use anyhow::{Context, Result};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use async_trait::async_trait;
use crate::cri::spec::ContainerSpec;
use crate::cri::RuntimeProvider;




fn raise_nproc_limit() {
    use nix::sys::resource::{getrlimit, setrlimit, Resource};
    if let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NPROC) {
        let target: u64 = 65535;
        if soft < target {
            let new_hard = hard.max(target);
            let _ = setrlimit(Resource::RLIMIT_NPROC, target, new_hard);
        }
    }
}




// old attach_port_publish and launch_pasta_for_pid removed (retired to retired/network/)

struct StdPipes {
    stdout_r: std::os::fd::OwnedFd, stdout_w: std::os::fd::OwnedFd,
    stderr_r: std::os::fd::OwnedFd, stderr_w: std::os::fd::OwnedFd,
    sync_r: std::os::fd::OwnedFd, sync_w: std::os::fd::OwnedFd,
    ack_r: std::os::fd::OwnedFd, ack_w: std::os::fd::OwnedFd,
}

fn create_std_pipes() -> anyhow::Result<StdPipes> {
    let p1 = nix::unistd::pipe().context("stdout pipe")?;
    let p2 = nix::unistd::pipe().context("stderr pipe")?;
    let p3 = nix::unistd::pipe().context("sync pipe")?;
    let p4 = nix::unistd::pipe().context("ack pipe")?;
    Ok(StdPipes { stdout_r: p1.0, stdout_w: p1.1, stderr_r: p2.0, stderr_w: p2.1,
                  sync_r: p3.0, sync_w: p3.1, ack_r: p4.0, ack_w: p4.1 })
}

struct ContainerSpawnCtx<'a> {
    entrypoint: &'a str,
    cmd_args: &'a [String],
    env_vars: &'a [(String, String)],
    rootfs_path: &'a str,
    container_id: &'a str,
    pod_uid: &'a str,
    image: &'a str,
    container_name: &'a str,
    volumes: Vec<crate::cri::volumes::ResolvedVolume>,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
    isolate_net: bool,
    privileged: bool,
    extra_caps: Vec<String>,
    published_ports: Vec<u16>,
    working_dir: Option<String>,
    probes: Vec<ProbeConfig>,
    subnet: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContainerInstance {
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub pid: Option<u32>,
    pub rootfs: String,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub env_vars: Vec<(String, String)>,
    /// container_port → 127.0.0.1 host port (pod network namespace publish)
    pub published_ports: std::collections::HashMap<u16, u16>,
    /// Pod has its own network namespace (declared containerPorts).
    pub isolated_net: bool,
    /// Pod IP allocated from the pool (NetMux).
    pub pod_ip: Option<std::net::Ipv4Addr>,
    /// Host veth ifindex for this pod (NetMux).
    pub host_veth_ifindex: Option<u32>,
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
    pub store: Arc<crate::types::ResourceStore>,
}



impl ProcessSupervisor {
    pub fn new(
        image_manager: Arc<ImageManager>,
        cgroup_manager: Arc<CgroupManager>,
        netmux: Arc<crate::netmux::NetMux>,
        store: Arc<crate::types::ResourceStore>,
    ) -> Self {
        let base = if rootfs::is_root() {
            "/var/lib/z8s".to_string()
        } else {
            format!("{}/.local/share/z8s", std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
        };
        std::fs::create_dir_all(format!("{}/containers", base)).ok();
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
            image_manager,
            cgroup_manager,
            restart_counts: Arc::new(Mutex::new(HashMap::new())),
            netmux,
            store,
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
        let oci = if cfg.entrypoint.is_empty() || cfg.working_dir.is_none() {
            crate::cri::oci::read_image_config(rootfs_path)
        } else {
            crate::cri::oci::SavedImageConfig::default()
        };
        let oci_wd = oci.working_dir.clone();
        let working_dir = cfg.working_dir.clone().or(oci_wd);
        let (entrypoint, cmd_args) = if cfg.entrypoint.is_empty() {
            let oci_ep = oci.entrypoint.as_ref().and_then(|v| v.first()).cloned();
            let oci_cmd = oci.cmd.unwrap_or_default();
            match (oci_ep, cfg.args.is_empty()) {
                (Some(ep), true) => {
                    let mut args = oci_cmd;
                    (ep, args)
                }
                (Some(ep), false) => {
                    (ep, cfg.args.clone())
                }
                (None, _) if !oci_cmd.is_empty() => {
                    let prog = oci_cmd[0].clone();
                    let args: Vec<String> = oci_cmd[1..].to_vec();
                    (prog, args)
                }
                (None, false) if !cfg.args.is_empty() => {
                    (cfg.args[0].clone(), cfg.args[1..].to_vec())
                }
                (None, _) => {
                    (cfg.entrypoint.clone(), cfg.args.clone())
                }
            }
        } else {
            (cfg.entrypoint.clone(), cfg.args.clone())
        };
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
            if !volumes.is_empty() {
                crate::cri::volumes::scrub_rootfs_volume_mounts(rootfs_path, volumes);
                crate::cri::volumes::stage_volumes_in_rootfs(rootfs_path, volumes);
            }
            rootfs::prepare_rootfs(rootfs_path)?;

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
                extra_caps: extra_caps.clone(),
                published_ports: published_ports_data.clone(),
                working_dir: working_dir.clone(),
                probes: cfg.probes.clone(),
                subnet,
            };
            if rootfs::is_root() {
                return self.spawn_root_ns_container(ctx, &published_ports_data).await;
            } else {
                return self.spawn_userns_container(ctx, &published_ports_data).await;
            }
        };

        child_cmd
            .env_clear()
            .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = child_cmd.spawn().context("Failed to spawn container process")?;
        let pid = child.id().ok_or_else(|| anyhow::anyhow!("Child process exited before PID was read"))?;
        info!("Container {} started with PID {}", container_id, pid);

        self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

        self.build_running_container(child, container_id, rootfs_path, image, &cfg.container_name, env_vars, isolate_net, &published_ports_data, &cfg.probes).await
    }

    pub async fn start_pod_from_spec(&self, spec: &crate::cri::spec::ContainerSpec) -> Result<()> {
        let pod_uid = &spec.pod_uid;
        let pod_name = &spec.pod_name;

        crate::cri::volumes::cleanup_emptydir(pod_uid);

        if self.check_duplicate_start(pod_name).await {
            return Ok(());
        }

        let placeholders = self.insert_placeholders(spec).await;

        info!("Starting pod {} ({} container(s))", pod_name, spec.containers.len());

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
        running.keys().any(|cid| cid.starts_with(&format!("{}-", pod_name)))
    }

    async fn insert_placeholders(&self, spec: &crate::cri::spec::ContainerSpec) -> Vec<String> {
        let mut running = self.running.lock().await;
        let ids: Vec<String> = spec.containers.iter().map(|c| c.container_id.clone()).collect();
        for cfg in &spec.containers {
            let cid = &cfg.container_id;
            if !running.contains_key(cid.as_str()) {
                running.insert(cid.clone(), RunningContainer {
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
                        self.cgroup_manager.set_cpu_limit(pod_uid, quota, period).ok();
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
            let rc = match self.spawn_container_from_config(cfg, &rootfs_path, &spec.pod_uid, spec.subnet.clone()).await {
                Ok(rc) => rc,
                Err(e) => {
                    Self::remove_placeholders(&self.running, placeholders).await;
                    Self::cleanup_orphan_containers(&prepared).await;
                    return Err(e.context(format!("Failed to spawn container {}/{}", spec.pod_name, cfg.container_name)));
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
            info!("Native process {}/{} (no OCI image)", spec.pod_name, cfg.container_name);
            return Ok(String::new());
        }
        self.image_manager.unpack_image(&cfg.image, &cfg.container_id).await
            .context(format!("Failed to prepare image {} for {}", cfg.image, cfg.container_name))
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

    fn spawn_probes(
        probes: &[ProbeConfig],
        container_id: &str,
        container_port_map: &std::collections::HashMap<u16, u16>,
    ) -> (Arc<AtomicBool>, Arc<Mutex<bool>>) {
        let ready = Arc::new(AtomicBool::new(true));
        let healthy = Arc::new(Mutex::new(true));
        if probes.is_empty() {
            return (ready, healthy);
        }
        let p_ready = ready.clone();
        let p_healthy = healthy.clone();
        let cid = container_id.to_string();
        let probes_owned = probes.to_vec();
        let port_map = container_port_map.clone();
        tokio::spawn(async move {
            for config in &probes_owned {
                tokio::time::sleep(Duration::from_secs(config.initial_delay_seconds as u64)).await;
                loop {
                    let status = match &config.action {
                        ProbeAction::Exec(exec) => {
                            HealthChecker::check_exec(exec.command.as_deref().unwrap_or(&[]), config.timeout()).await
                        }
                        ProbeAction::HTTPGet(http) => {
                            let mut h = http.clone();
                            if let Some(&host_port) = port_map.get(&h.port) {
                                h.port = host_port;
                            }
                            HealthChecker::check_http(&h, config.timeout()).await
                        }
                        ProbeAction::TCPSocket(tcp) => {
                            let mut t = tcp.clone();
                            if let Some(&host_port) = port_map.get(&t.port) {
                                t.port = host_port;
                            }
                            HealthChecker::check_tcp(&t, config.timeout()).await
                        }
                    };
                    let ok = matches!(status, HealthStatus::Healthy);
                    p_ready.store(ok, Ordering::SeqCst);
                    *p_healthy.lock().await = ok;
                    if !ok { warn!("Probe for {} failed", cid); }
                    tokio::time::sleep(Duration::from_secs(config.period_seconds as u64)).await;
                }
            }
        });
        (ready, healthy)
    }

    fn merge_env(env_vars: &[(String, String)], rootfs: &str) -> Vec<(String, String)> {
        let oci_env = crate::cri::oci::read_image_config(rootfs).env.unwrap_or_default();
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
                let val = entry[eq+1..].to_string();
                if seen.insert(key.clone()) {
                    env_owned.push((key, val));
                }
            }
        }
        env_owned
    }

    fn handle_veth_netns(
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

    fn spawn_log_tasks(
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

    fn setup_child_pipes(
        stdout_r: std::os::fd::OwnedFd,
        stderr_r: std::os::fd::OwnedFd,
        stdout_w: std::os::fd::OwnedFd,
        stderr_w: std::os::fd::OwnedFd,
    ) {
        drop(stdout_r);
        drop(stderr_r);
        nix::unistd::dup2_stdout(&stdout_w).ok();
        nix::unistd::dup2_stderr(&stderr_w).ok();
        drop(stdout_w);
        drop(stderr_w);
        if let Ok(fd) = nix::fcntl::open(
            "/dev/null",
            nix::fcntl::OFlag::O_RDONLY,
            nix::sys::stat::Mode::empty(),
        ) {
            let _ = nix::unistd::dup2_stdin(fd);
        }
    }

    async fn spawn_root_ns_container(
        &self,
        ctx: ContainerSpawnCtx<'_>,
        _service_ports: &[u16],
    ) -> Result<RunningContainer> {
        let ContainerSpawnCtx { entrypoint, cmd_args, env_vars, rootfs_path, container_id, pod_uid, image, container_name, volumes, run_as_user, run_as_group, isolate_net, privileged, extra_caps, published_ports, working_dir, probes, subnet } = ctx;
        let pipes = create_std_pipes()?;
        let StdPipes { stdout_r, stdout_w, stderr_r, stderr_w, sync_r, sync_w, ack_r, ack_w } = pipes;
        let rootfs_owned = rootfs_path.to_string();
        let entrypoint_owned = entrypoint.to_string();
        let args_owned = cmd_args.to_vec();
        let env_owned = Self::merge_env(env_vars, &rootfs_owned);

        match unsafe { nix::unistd::fork() } {
            Ok(nix::unistd::ForkResult::Parent { child }) => {
                drop(stdout_w);
                drop(stderr_w);
                drop(sync_w);
                drop(ack_r);
                let pid = child.as_raw() as u32;
                info!("Container {} started with PID {} (root ns)", container_id, pid);
                self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

                let (pod_ip, host_veth_ifindex) = self.handle_veth_netns(pod_uid, pid, isolate_net, &sync_r, &ack_w, subnet.as_deref());

                drop(sync_r);
                drop(ack_w);

                let log_buffer = Self::spawn_log_tasks(stdout_r, stderr_r);

                let instance = ContainerInstance {
                    container_id: container_id.to_string(),
                    container_name: container_name.to_string(),
                    image: image.to_string(),
                    pid: Some(pid),
                    rootfs: rootfs_path.to_string(),
                    started_at: Some(chrono::Utc::now()),
                    env_vars: env_owned,
                    published_ports: std::collections::HashMap::new(),
                    isolated_net: isolate_net,
                    pod_ip,
                    host_veth_ifindex,
                };

                let (ready, healthy) = Self::spawn_probes(&probes, container_id, &std::collections::HashMap::new());
                Ok(RunningContainer {
                    child: None,
                    instance,
                    restart_count: 0,
                    log_buffer,
                    ready,
                    healthy,
                })
            }
            Ok(nix::unistd::ForkResult::Child) => {
                Self::setup_child_pipes(stdout_r, stderr_r, stdout_w, stderr_w);
                let _ = nix::unistd::setsid();
                let pod_hostname = container_id.rsplit_once('-').map_or(container_id, |(pod, _)| pod);
                let isolation = match rootfs::child_enter_ns_root(&rootfs_owned, &volumes, isolate_net, pod_hostname) {
                    Ok(i) => i,
                    Err(e) => {
                        error!("z8s: root namespace setup failed: {}", e);
                        std::process::exit(1);
                    }
                };

                if isolate_net {
                    nix::unistd::write(&sync_w, b"S").ok();
                    let mut ack = [0u8; 1];
                    let _ = nix::unistd::read(&ack_r, &mut ack);
                }
                drop(sync_w);
                drop(ack_r);

                if let Some(gid) = run_as_group {
                    if let Err(e) = nix::unistd::setgid(nix::unistd::Gid::from_raw(gid)) {
                        warn!("setgid({}) failed: {}", gid, e);
                    }
                }
                if let Some(uid) = run_as_user {
                    if let Err(e) = nix::unistd::setuid(nix::unistd::Uid::from_raw(uid)) {
                        warn!("setuid({}) failed: {}", uid, e);
                    }
                }
                if let Some(wd) = &working_dir {
                    if let Err(e) = nix::unistd::chdir(std::path::Path::new(wd)) {
                        warn!("chdir({}) failed: {}", wd, e);
                    }
                }

                raise_nproc_limit();
                rootfs::drop_capabilities(privileged, &extra_caps);
                if isolation != rootfs::RootfsIsolation::Degraded {
                    rootfs::apply_landlock();
                }

                let (exec_path, prog_args) = Self::argv_for_isolation(
                    &entrypoint_owned,
                    &args_owned,
                    &rootfs_owned,
                    isolation,
                );
                Self::execvpe_container(&exec_path, &prog_args, &env_owned, &rootfs_owned, isolation);
            }
            Err(e) => {
                drop(stdout_r);
                drop(stdout_w);
                drop(stderr_r);
                drop(stderr_w);
                drop(sync_r);
                drop(sync_w);
                drop(ack_r);
                drop(ack_w);
                anyhow::bail!("Failed to fork: {}", e);
            }
        }
    }

    fn argv_for_isolation(
        entrypoint: &str,
        args: &[String],
        rootfs_host_path: &str,
        isolation: rootfs::RootfsIsolation,
    ) -> (String, Vec<String>) {
        if isolation == rootfs::RootfsIsolation::Degraded {
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
        isolation: rootfs::RootfsIsolation,
    ) -> ! {
        let envp: Vec<std::ffi::CString> = env_owned
            .iter()
            .map(|(k, v)| std::ffi::CString::new(format!("{}={}", k, v))
                .expect("env keys/values cannot contain null bytes"))
            .collect();

        let mut argv: Vec<std::ffi::CString> =
            vec![std::ffi::CString::new(exec_path)
                .expect("exec path cannot contain null bytes")];
        for a in prog_args {
            argv.push(std::ffi::CString::new(a.as_str())
                .expect("arg strings cannot contain null bytes"));
        }

        if isolation == rootfs::RootfsIsolation::Degraded {
            let (loader, args) =
                rootfs::wrap_dynamic_linker(exec_path, prog_args.to_vec(), rootfs_host_path);
            argv = vec![std::ffi::CString::new(loader)
                .expect("loader path cannot contain null bytes")];
            for a in args {
                argv.push(std::ffi::CString::new(a)
                    .expect("arg strings cannot contain null bytes"));
            }
        }

        let e = nix::unistd::execvpe(&argv[0], &argv, &envp).expect_err("execvpe returned unexpectedly");
        error!("z8s: execvpe({}) failed: {}", argv[0].to_str().unwrap_or("?"), e);
        std::process::exit(1);
    }

    async fn spawn_userns_container(
        &self,
        ctx: ContainerSpawnCtx<'_>,
        _service_ports: &[u16],
    ) -> Result<RunningContainer> {
        let ContainerSpawnCtx { entrypoint, cmd_args, env_vars, rootfs_path, container_id, pod_uid, image, container_name, volumes, run_as_user, run_as_group, isolate_net, privileged, extra_caps, published_ports, working_dir, probes, subnet } = ctx;
        let (stdout_r, stdout_w) = nix::unistd::pipe()
            .context("Failed to create stdout pipe")?;
        let (stderr_r, stderr_w) = nix::unistd::pipe()
            .context("Failed to create stderr pipe")?;
        let (sync_r, sync_w) = nix::unistd::pipe()
            .context("Failed to create sync pipe")?;
        let (ack_r, ack_w) = nix::unistd::pipe()
            .context("Failed to create ack pipe")?;

        let rootfs_owned = rootfs_path.to_string();
        let entrypoint_owned = entrypoint.to_string();
        let args_owned: Vec<String> = cmd_args.to_vec();

        // Merge OCI image env into container env (Pod-specified env takes precedence)
        let oci_env = crate::cri::oci::read_image_config(&rootfs_owned).env.unwrap_or_default();
        let mut env_owned: Vec<(String, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Pod env first (higher priority)
        for (k, v) in env_vars {
            if seen.insert(k.clone()) {
                env_owned.push((k.clone(), v.clone()));
            }
        }
        // OCI env second (fills gaps — lower priority)
        for entry in &oci_env {
            if let Some(eq) = entry.find('=') {
                let key = entry[..eq].to_string();
                let val = entry[eq+1..].to_string();
                if seen.insert(key.clone()) {
                    env_owned.push((key, val));
                }
            }
        }

        match unsafe { nix::unistd::fork() } {
            Ok(nix::unistd::ForkResult::Parent { child }) => {
                drop(stdout_w);
                drop(stderr_w);
                drop(sync_w);
                drop(ack_r);

                let child_pid = child.as_raw();

                let mut sync_buf = [0u8; 1];
                let n = nix::unistd::read(&sync_r, &mut sync_buf)
                    .context("Failed to read sync from child")?;
                if n == 0 || sync_buf[0] != b'S' {
                    anyhow::bail!("Child process died before completing namespace setup");
                }
                drop(sync_r);

                rootfs::write_userns_maps(child_pid, run_as_user, run_as_group)?;

                let mut pod_ip: Option<std::net::Ipv4Addr> = None;
                let mut host_veth_ifindex: Option<u32> = None;

                if isolate_net {
                    let pid = child_pid as u32;
                    match self.netmux.attach_pod(pod_uid, Some(pid), subnet.as_deref()) {
                        Ok((ip, host_idx, peer_idx)) => {
                            if let Err(e) = self.netmux.configure_pod_netns(pod_uid, &ip, pid, peer_idx) {
                        warn!("NetMux configure_pod_netns failed: {:#}", e);
                            }
                            pod_ip = Some(ip);
                            host_veth_ifindex = Some(host_idx);
                            info!("NetMux: pod {} -> IP {}", pod_uid, ip);
                        }
                        Err(e) => warn!("NetMux: failed to attach pod {}: {}", pod_uid, e),
                    }
                }

                nix::unistd::write(&ack_w, b"A").ok();
                drop(ack_w);

                info!("User namespace configured for child PID {}", child_pid);

                let pid = child_pid as u32;
                info!("Container {} started with PID {}", container_id, pid);

                self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

                let log_buffer = Arc::new(Mutex::new(Vec::<String>::new()));

                let stdout_file = std::fs::File::from(stdout_r);
                let stdout_async = tokio::io::BufReader::new(
                    tokio::fs::File::from_std(stdout_file),
                );

                {
                    let buf = log_buffer.clone();
                    tokio::spawn(async move {
                        let mut lines = stdout_async.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let mut log = buf.lock().await;
                            log.push(format!("[stdout] {}", line));
                            if log.len() > 1000 {
                                log.remove(0);
                            }
                        }
                    });
                }

                let stderr_file = std::fs::File::from(stderr_r);
                let stderr_async = tokio::io::BufReader::new(
                    tokio::fs::File::from_std(stderr_file),
                );

                {
                    let buf = log_buffer.clone();
                    tokio::spawn(async move {
                        let mut lines = stderr_async.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let mut log = buf.lock().await;
                            log.push(format!("[stderr] {}", line));
                            if log.len() > 1000 {
                                log.remove(0);
                            }
                        }
                    });
                }

                let mut instance = ContainerInstance {
                    container_id: container_id.to_string(),
                    container_name: container_name.to_string(),
                    image: image.to_string(),
                    pid: Some(pid),
                    rootfs: rootfs_path.to_string(),
                    started_at: Some(chrono::Utc::now()),
                    env_vars: env_owned.clone(),
                    published_ports: std::collections::HashMap::new(),
                    isolated_net: isolate_net,
                    pod_ip,
                    host_veth_ifindex,
                };
                instance.published_ports = std::collections::HashMap::new();

                let (ready, healthy) = Self::spawn_probes(&probes, container_id, &instance.published_ports);

                Ok(RunningContainer {
                    child: None,
                    instance,
                    restart_count: 0,
                    log_buffer,
                    ready,
                    healthy,
                })
            }
            Ok(nix::unistd::ForkResult::Child) => {
                drop(stdout_r);
                drop(stderr_r);
                drop(sync_r);
                drop(ack_w);

                let _ = nix::unistd::setsid();

                let pod_hostname = container_id.rsplit_once('-').map_or(container_id, |(pod, _)| pod);
                let isolation = match rootfs::child_enter_ns_fork(
                    &rootfs_owned,
                    sync_w,
                    ack_r,
                    &volumes,
                    isolate_net,
                    pod_hostname,
                ) {
                    Ok(i) => i,
                    Err(e) => {
                        error!("z8s: namespace setup failed: {:#}", e);
                        std::process::exit(1);
                    }
                };

                nix::unistd::dup2_stdout(&stdout_w).ok();
                nix::unistd::dup2_stderr(&stderr_w).ok();
                drop(stdout_w);
                drop(stderr_w);

                // Open /dev/null for stdin instead of close(0)
                // close(0) after pivot_root causes SIGABRT in userns because when a forked child
                // starts, glibc tries to open /dev/null for fd 0, but device bind-mounts only
                // existed if we pre-created the destination files
                if let Ok(fd) = nix::fcntl::open(
                    "/dev/null",
                    nix::fcntl::OFlag::O_RDONLY,
                    nix::sys::stat::Mode::empty(),
                ) {
                    let _ = nix::unistd::dup2_stdin(fd);
                }

                if let Some(gid) = run_as_group {
                    let _ = nix::unistd::setgid(nix::unistd::Gid::from_raw(gid));
                }
                if let Some(uid) = run_as_user {
                    let _ = nix::unistd::setuid(nix::unistd::Uid::from_raw(uid));
                }

                if let Some(wd) = &working_dir {
                    let _ = nix::unistd::chdir(std::path::Path::new(wd));
                }

                raise_nproc_limit();
                rootfs::drop_capabilities(privileged, &extra_caps);
                if isolation != rootfs::RootfsIsolation::Degraded {
                    rootfs::apply_landlock();
                }

                let (exec_path, prog_args) = Self::argv_for_isolation(
                    &entrypoint_owned,
                    &args_owned,
                    &rootfs_owned,
                    isolation,
                );
                Self::execvpe_container(&exec_path, &prog_args, &env_owned, &rootfs_owned, isolation);
            }
            Err(e) => {
                drop(stdout_r);
                drop(stdout_w);
                drop(stderr_r);
                drop(stderr_w);
                drop(sync_r);
                drop(sync_w);
                drop(ack_r);
                drop(ack_w);
                anyhow::bail!("Failed to fork: {}", e);
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
        let pid = child.id().ok_or_else(|| anyhow::anyhow!("Child process exited before PID was read"))?;

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

                let mut instance = ContainerInstance {
                    container_id: container_id.to_string(),
                    container_name: container_name.to_string(),
                    image: image.to_string(),
                    pid: Some(pid),
                    rootfs: rootfs_path.to_string(),
                    started_at: Some(chrono::Utc::now()),
                    env_vars: env_vars.to_vec(),
                    published_ports: std::collections::HashMap::new(),
                    isolated_net: isolate_net,
                    pod_ip: None,
                    host_veth_ifindex: None,
                };
                instance.published_ports = std::collections::HashMap::new();

                let (ready, healthy) = Self::spawn_probes(&probes, container_id, &instance.published_ports);

                Ok(RunningContainer {
                    child: None,
                    instance,
                    restart_count: 0,
            log_buffer,
            ready,
            healthy,
        })
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
        let mut running = self.running.lock().await;
        if let Some(rc) = running.remove(container_id) {
            if let Some(pid) = rc.instance.pid {
                info!("Stopping container {} (PID {})", container_id, pid);
                // Kill the entire process group (container is session leader via setsid())
                // Negative PID targets the process group, killing orphaned children
                let pgid = nix::unistd::Pid::from_raw(-(pid as i32));
                let _ = kill(pgid, Signal::SIGTERM);
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = kill(pgid, Signal::SIGKILL);
                // Also kill the main PID in case the pgid kill missed it
                let _ = kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGKILL);
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
            info!("Container {} stopped; reconcile loop will restart it", container_id);
            let _ = rootfs;
        }
    }

    pub async fn stop_pod_from_spec(&self, spec: &crate::cri::spec::ContainerSpec) {
        for cfg in &spec.containers {
            // Detach netmux networking before stopping the container
            {
                let running = self.running.lock().await;
                if let Some(rc) = running.get(&cfg.container_id) {
                    if let (Some(ip), Some(ifindex)) = (rc.instance.pod_ip, rc.instance.host_veth_ifindex) {
                        if let Err(e) = self.netmux.detach_pod(&spec.pod_uid, &ip, ifindex) {
                            warn!("NetMux detach failed for {}: {}", spec.pod_uid, e);
                        }
                    }
                }
            }
            self.stop_container(&cfg.container_id).await;
            self.restart_counts.lock().await.remove(&cfg.container_id);
        }
        self.cgroup_manager.remove_cgroup(&spec.pod_uid).ok();
        crate::cri::volumes::cleanup_emptydir(&spec.pod_uid);
    }

    pub async fn is_pod_running(&self, pod_name: &str) -> bool {
        let prefix = format!("{}-", pod_name);
        self.running.lock().await.keys().any(|cid| cid.starts_with(&prefix))
    }

    pub fn is_pid_alive(pid: u32) -> bool {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            None,
        )
        .is_ok()
    }

    pub async fn is_pod_alive(&self, pod_name: &str) -> bool {
        let prefix = format!("{}-", pod_name);
        let running = self.running.lock().await;
        running.iter().any(|(cid, rc)| {
            cid.starts_with(&prefix)
                && rc.instance.pid.map(Self::is_pid_alive).unwrap_or(false)
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

    pub async fn get_restart_count(&self, container_id: &str) -> u32 {
        self.restart_counts.lock().await.get(container_id).copied().unwrap_or(0)
    }

    /// Returns restart count per container name for a given pod (strips the pod-name prefix).
    pub async fn pod_restart_counts(&self, pod_name: &str) -> std::collections::HashMap<String, u32> {
        let prefix = format!("{}-", pod_name);
        self.restart_counts.lock().await.iter()
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
    pub fn new(
        supervisor: Arc<ProcessSupervisor>,
        cgroup_manager: Arc<CgroupManager>,
    ) -> Self {
        Self { supervisor, cgroup_manager }
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
        info!("CRI: starting pod {} (namespace={})", spec.pod_name, spec.namespace);
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
        self.supervisor.get_container_logs(pod_name, container_name).await
    }

    async fn pod_restart_counts(&self, pod_name: &str) -> HashMap<String, u32> {
        self.supervisor.pod_restart_counts(pod_name).await
    }

    async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String> {
        self.supervisor.image_manager.unpack_image(image_ref, container_id).await
    }

    fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String> {
        self.cgroup_manager.create_pod_cgroup(pod_uid)
    }

    fn remove_cgroup(&self, pod_uid: &str) -> Result<()> {
        self.cgroup_manager.remove_cgroup(pod_uid)
    }
}
