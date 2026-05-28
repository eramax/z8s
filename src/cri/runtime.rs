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
use tracing::{info, warn};
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




fn attach_port_publish(
    pid: u32,
    ports: &[u16],
) -> (std::collections::HashMap<u16, u16>, Option<crate::cri::port_publish::PortPublish>) {
    if ports.is_empty() {
        return (std::collections::HashMap::new(), None);
    }
    let publish = crate::cri::port_publish::publish_ports(pid, ports);
    (publish.map.clone(), Some(publish))
}

fn launch_pasta_for_pid(pid: u32) {
    info!("Launching pasta for PID {}", pid);
    match std::process::Command::new("pasta")
        .arg("--quiet")
        .arg("-t")
        .arg("none")
        .arg("-u")
        .arg("none")
        .arg("-T")
        .arg("none")
        .arg("-U")
        .arg("none")
        .arg(pid.to_string())
        .status()
    {
        Ok(status) => {
            if status.success() {
                info!("Successfully configured pasta networking for PID {}", pid);
            } else {
                warn!("pasta command exited with non-zero status for PID {}", pid);
            }
        }
        Err(e) => {
            warn!("Failed to launch pasta for PID {}: {}", pid, e);
        }
    }
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
}

#[derive(Debug)]
pub struct RunningContainer {
    pub child: Option<Child>,
    pub instance: ContainerInstance,
    pub restart_count: u32,
    pub log_buffer: Arc<Mutex<Vec<String>>>,
    pub ready: Arc<AtomicBool>,
    pub healthy: Arc<Mutex<bool>>,
    port_publish: Option<crate::cri::port_publish::PortPublish>,
}

pub struct ProcessSupervisor {
    pub running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    pub image_manager: Arc<ImageManager>,
    pub cgroup_manager: Arc<CgroupManager>,
    pub restart_counts: Arc<Mutex<HashMap<String, u32>>>,
}



impl ProcessSupervisor {
    pub fn new(
        image_manager: Arc<ImageManager>,
        cgroup_manager: Arc<CgroupManager>,
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
        }
    }


    // ── ContainerSpec-based spawn pipeline (§7.5) ──────────────────────────

    async fn spawn_container_from_config(
        &self,
        cfg: &crate::cri::spec::ContainerConfig,
        rootfs_path: &str,
        pod_uid: &str,
    ) -> Result<RunningContainer> {
        let image = &cfg.image;
        let is_native = cfg.is_native;
        let entrypoint = &cfg.entrypoint;
        let cmd_args = &cfg.args;
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
            c.args(cmd_args);
            c
        } else {
            if !volumes.is_empty() {
                crate::cri::volumes::scrub_rootfs_volume_mounts(rootfs_path, volumes);
                crate::cri::volumes::stage_volumes_in_rootfs(rootfs_path, volumes);
            }
            rootfs::prepare_rootfs(rootfs_path)?;

            let rootfs_owned = rootfs_path.to_string();

            let ctx = ContainerSpawnCtx {
                entrypoint,
                cmd_args,
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
        let pid = child.id().expect("No PID for spawned process");
        info!("Container {} started with PID {}", container_id, pid);

        self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

        self.build_running_container(child, container_id, rootfs_path, image, &cfg.container_name, env_vars, isolate_net, &published_ports_data, &cfg.probes).await
    }

    pub async fn start_pod_from_spec(&self, spec: &crate::cri::spec::ContainerSpec) -> Result<()> {
        let pod_uid = &spec.pod_uid;
        let pod_name = &spec.pod_name;

        crate::cri::volumes::cleanup_emptydir(pod_uid);

        {
            let running = self.running.lock().await;
            if running.keys().any(|cid| cid.starts_with(&format!("{}-", pod_name))) {
                info!("Pod {} already running, skipping duplicate start", pod_name);
                return Ok(());
            }
        }

        info!("Starting pod {} ({} container(s))", pod_name, spec.containers.len());

        self.cgroup_manager.create_pod_cgroup(pod_uid)?;

        let mut prepared = Vec::new();
        for cfg in &spec.containers {
            let image_ref = &cfg.image;
            let rootfs_path = if cfg.is_native {
                info!("Native process {}/{} (no OCI image)", pod_name, cfg.container_name);
                String::new()
            } else {
                self.image_manager.unpack_image(image_ref, &cfg.container_id).await
                    .context(format!("Failed to prepare image {} for {}", image_ref, cfg.container_name))?
            };

            info!("Starting container {}/{}", pod_name, cfg.container_name);
            let rc = self.spawn_container_from_config(cfg, &rootfs_path, pod_uid).await?;
            prepared.push((cfg.container_id.clone(), rc));
        }

        let mut running = self.running.lock().await;
        for (cid, rc) in prepared {
            running.insert(cid, rc);
        }
        drop(running);

        Ok(())
    }






    async fn spawn_root_ns_container(
        &self,
        ctx: ContainerSpawnCtx<'_>,
        _service_ports: &[u16],
    ) -> Result<RunningContainer> {
        let ContainerSpawnCtx { entrypoint, cmd_args, env_vars, rootfs_path, container_id, pod_uid, image, container_name, volumes, run_as_user, run_as_group, isolate_net, privileged, extra_caps, published_ports } = ctx;
        let (stdout_r, stdout_w) = nix::unistd::pipe().context("Failed to create stdout pipe")?;
        let (stderr_r, stderr_w) = nix::unistd::pipe().context("Failed to create stderr pipe")?;

        let rootfs_owned = rootfs_path.to_string();
        let entrypoint_owned = entrypoint.to_string();
        let args_owned = cmd_args.to_vec();
        let env_owned = env_vars.to_vec();

        match unsafe { nix::unistd::fork() } {
            Ok(nix::unistd::ForkResult::Parent { child }) => {
                drop(stdout_w);
                drop(stderr_w);

                let pid = child.as_raw() as u32;
                info!("Container {} started with PID {} (root ns)", container_id, pid);
                self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

                if isolate_net {
                    launch_pasta_for_pid(pid);
                }

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
                };
                let (published_ports_map, port_publish) = attach_port_publish(pid, &published_ports);
                instance.published_ports = published_ports_map;

                Ok(RunningContainer {
                    child: None,
                    instance,
                    restart_count: 0,
                    log_buffer,
                    ready: Arc::new(AtomicBool::new(true)),
                    healthy: Arc::new(Mutex::new(true)),
                    port_publish,
                })
            }
            Ok(nix::unistd::ForkResult::Child) => {
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

                let pod_hostname = container_id.rsplit_once('-').map_or(container_id, |(pod, _)| pod);
                let isolation = match rootfs::child_enter_ns_root(&rootfs_owned, &volumes, isolate_net, pod_hostname)
                {
                    Ok(i) => i,
                    Err(e) => {
                        eprintln!("z8s: root namespace setup failed: {}", e);
                        std::process::exit(1);
                    }
                };

                if let Some(gid) = run_as_group {
                    let _ = nix::unistd::setgid(nix::unistd::Gid::from_raw(gid));
                }
                if let Some(uid) = run_as_user {
                    let _ = nix::unistd::setuid(nix::unistd::Uid::from_raw(uid));
                }

                raise_nproc_limit();
                rootfs::drop_capabilities(privileged, &extra_caps);
                if isolation != rootfs::RootfsIsolation::Degraded {
                    rootfs::apply_landlock();
                }
                rootfs::apply_seccomp(privileged);

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
            .map(|(k, v)| std::ffi::CString::new(format!("{}={}", k, v)).unwrap())
            .collect();

        let mut argv: Vec<std::ffi::CString> =
            vec![std::ffi::CString::new(exec_path).unwrap()];
        for a in prog_args {
            argv.push(std::ffi::CString::new(a.as_str()).unwrap());
        }

        if isolation == rootfs::RootfsIsolation::Degraded {
            let (loader, args) =
                rootfs::wrap_dynamic_linker(exec_path, prog_args.to_vec(), rootfs_host_path);
            argv = vec![std::ffi::CString::new(loader).unwrap()];
            for a in args {
                argv.push(std::ffi::CString::new(a).unwrap());
            }
        }

        let e = nix::unistd::execvpe(&argv[0], &argv, &envp).expect_err("execvpe returned unexpectedly");
        eprintln!("z8s: execvpe({}) failed: {}", argv[0].to_str().unwrap_or("?"), e);
        std::process::exit(1);
    }

    async fn spawn_userns_container(
        &self,
        ctx: ContainerSpawnCtx<'_>,
        _service_ports: &[u16],
    ) -> Result<RunningContainer> {
        let ContainerSpawnCtx { entrypoint, cmd_args, env_vars, rootfs_path, container_id, pod_uid, image, container_name, volumes, run_as_user, run_as_group, isolate_net, privileged, extra_caps, published_ports } = ctx;
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
        let env_owned: Vec<(String, String)> = env_vars.to_vec();

        match unsafe { nix::unistd::fork() } {
            Ok(nix::unistd::ForkResult::Parent { child }) => {
                drop(stdout_w);
                drop(stderr_w);
                drop(sync_w);
                drop(ack_r);

                let child_pid = child.as_raw();

                let mut sync_buf = [0u8; 1];
                nix::unistd::read(&sync_r, &mut sync_buf)
                    .context("Failed to read sync from child")?;
                drop(sync_r);

                rootfs::write_userns_maps(child_pid, run_as_user, run_as_group)?;

                nix::unistd::write(&ack_w, b"A").ok();
                drop(ack_w);

                info!("User namespace configured for child PID {}", child_pid);

                let pid = child_pid as u32;
                info!("Container {} started with PID {}", container_id, pid);

                self.cgroup_manager.add_pid_to_cgroup(pod_uid, pid)?;

                if isolate_net {
                    launch_pasta_for_pid(pid);
                }

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
                };
                let (published_ports_map, port_publish) = attach_port_publish(pid, &published_ports);
                instance.published_ports = published_ports_map;

                let ready = Arc::new(AtomicBool::new(true));
                let healthy = Arc::new(Mutex::new(true));

                Ok(RunningContainer {
                    child: None,
                    instance,
                    restart_count: 0,
                    log_buffer,
                    ready,
                    healthy,
                    port_publish,
                })
            }
            Ok(nix::unistd::ForkResult::Child) => {
                drop(stdout_r);
                drop(stderr_r);
                drop(sync_r);
                drop(ack_w);

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
                        eprintln!("z8s: namespace setup failed: {:#}", e);
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

                raise_nproc_limit();
                rootfs::drop_capabilities(privileged, &extra_caps);
                if isolation != rootfs::RootfsIsolation::Degraded {
                    rootfs::apply_landlock();
                }
                rootfs::apply_seccomp(privileged);

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
        let pid = child.id().expect("No PID for spawned process");

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
        };
        let (published_ports_map, port_publish) = attach_port_publish(pid, published_ports);
        instance.published_ports = published_ports_map;

        let ready = Arc::new(AtomicBool::new(true));
        let healthy = Arc::new(Mutex::new(true));

        if !probes.is_empty() {
            let p_ready = ready.clone();
            let p_healthy = healthy.clone();
            let cid = container_id.to_string();
            let probes_owned = probes.to_vec();
            tokio::spawn(async move {
                for config in &probes_owned {
                    tokio::time::sleep(Duration::from_secs(config.initial_delay_seconds as u64)).await;
                    loop {
                        let status = match &config.action {
                            ProbeAction::Exec(exec) => {
                                HealthChecker::check_exec(exec.command.as_deref().unwrap_or(&[]), config.timeout()).await
                            }
                            ProbeAction::HTTPGet(http) => {
                                HealthChecker::check_http(http, config.timeout()).await
                            }
                            ProbeAction::TCPSocket(tcp) => {
                                HealthChecker::check_tcp(tcp, config.timeout()).await
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
        }

        Ok(RunningContainer {
            child: Some(child),
            instance,
            restart_count: 0,
            log_buffer,
            ready,
            healthy,
            port_publish,
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
        if let Some(mut rc) = running.remove(container_id) {
            if let Some(mut pp) = rc.port_publish.take() {
                pp.stop();
            }
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
            info!("Container {} stopped; reconcile loop will restart it", container_id);
            let _ = rootfs;
        }
    }

    pub async fn stop_pod_from_spec(&self, spec: &crate::cri::spec::ContainerSpec) {
        for cfg in &spec.containers {
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
