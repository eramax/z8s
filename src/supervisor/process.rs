use crate::api::types::{
    extract_containers, parse_quantity_bytes, parse_quantity_cpu, ResourceState, ResourceStore,
};
use crate::api::AnyResource;
use crate::container::image::ImageManager;
use crate::container::rootfs;
use crate::supervisor::cgroup::CgroupManager;
use crate::supervisor::health::{HealthChecker, HealthStatus, ProbeAction, ProbeConfig};
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{ConfigMap, Container, Pod, PodSecurityContext, Secret};
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

fn resolve_run_as_user(pod_sc: Option<&PodSecurityContext>, container: &Container) -> Option<u32> {
    container
        .security_context
        .as_ref()
        .and_then(|sc| sc.run_as_user)
        .or_else(|| pod_sc.and_then(|sc| sc.run_as_user))
        .map(|u| u as u32)
}


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

fn declared_container_ports(container: &Container) -> Vec<u16> {
    container
        .ports
        .as_ref()
        .map(|ps| ps.iter().map(|p| p.container_port as u16).collect())
        .unwrap_or_default()
}

fn use_isolated_network(container: &Container) -> bool {
    !declared_container_ports(container).is_empty()
}

fn merge_publish_ports(container: &Container, isolated_net: bool) -> Vec<u16> {
    if !isolated_net {
        return Vec::new();
    }
    declared_container_ports(container)
}

fn attach_port_publish(
    pid: u32,
    ports: &[u16],
) -> (std::collections::HashMap<u16, u16>, Option<crate::network::port_publish::PortPublish>) {
    if ports.is_empty() {
        return (std::collections::HashMap::new(), None);
    }
    let publish = crate::network::port_publish::publish_ports(pid, ports);
    (publish.map.clone(), Some(publish))
}

fn resolve_run_as_group(pod_sc: Option<&PodSecurityContext>, container: &Container) -> Option<u32> {
    container
        .security_context
        .as_ref()
        .and_then(|sc| sc.run_as_group)
        .or_else(|| pod_sc.and_then(|sc| sc.run_as_group))
        .map(|g| g as u32)
}

struct ContainerSpawnCtx<'a> {
    entrypoint: &'a str,
    cmd_args: &'a [String],
    env_vars: &'a [(String, String)],
    rootfs_path: &'a str,
    container_id: &'a str,
    pod_uid: &'a str,
    image: &'a str,
    container: &'a Container,
    volumes: Vec<crate::container::volumes::ResolvedVolume>,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
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
    pub ready: Arc<Mutex<bool>>,
    pub healthy: Arc<Mutex<bool>>,
    port_publish: Option<crate::network::port_publish::PortPublish>,
}

pub struct ProcessSupervisor {
    pub running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    pub image_manager: Arc<ImageManager>,
    pub cgroup_manager: Arc<CgroupManager>,
    pub store: Arc<ResourceStore>,
    restart_counts: Arc<Mutex<HashMap<String, u32>>>,
    network: Arc<std::sync::Mutex<Option<Arc<crate::network::NetworkManager>>>>,
}

impl ProcessSupervisor {
    pub fn new(
        image_manager: Arc<ImageManager>,
        cgroup_manager: Arc<CgroupManager>,
        store: Arc<ResourceStore>,
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
            store,
            restart_counts: Arc::new(Mutex::new(HashMap::new())),
            network: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn set_network(&self, network: Arc<crate::network::NetworkManager>) {
        *self.network.lock().unwrap() = Some(network);
    }

    pub async fn start_pod(&self, resource: &AnyResource) -> Result<()> {
        let containers = extract_containers(resource);
        if containers.is_empty() {
            warn!("No containers in resource {}", resource.name());
            return Ok(());
        }

        let pod_uid = resource.uid();
        let pod_name = resource.name().to_string();
        // Guard against concurrent start by multiple reconcile rounds
        {
            let running = self.running.lock().await;
            if running.keys().any(|cid| cid.starts_with(&format!("{}-", pod_name))) {
                info!("Pod {} already running, skipping duplicate start", pod_name);
                return Ok(());
            }
        }

        info!("Starting pod {} ({} container(s))", pod_name, containers.len());

        self.cgroup_manager.create_pod_cgroup(&pod_uid)?;
        self.set_resource_limits(&pod_uid, &containers)?;

        let service_ports = if let AnyResource::Pod(pod) = resource {
            self.service_target_ports_for_pod(pod).await
        } else {
            Vec::new()
        };

        let mut prepared = Vec::new();
        for container in &containers {
            let container_id = format!("{}-{}", pod_name, container.name);
            let image_ref = container.image.clone().unwrap_or_default();

            let rootfs_path = if image_ref.is_empty() || image_ref == "host" || image_ref.starts_with("host://") {
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
                .spawn_container(resource, container, &container_id, &rootfs_path, &pod_uid, &service_ports)
                .await?;
            prepared.push((container_id, rc));
        }

        let mut running = self.running.lock().await;
        for (cid, rc) in prepared {
            running.insert(cid, rc);
        }
        drop(running);

        self.store.update_state(&pod_uid, ResourceState::Running).await;

        if let AnyResource::Pod(pod) = resource {
            let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            let net = self.network.lock().unwrap().clone();
            if let Some(net) = net {
                net.sync_services_for_labels(ns, &labels).await;
            }
        }

        Ok(())
    }

    async fn fetch_cms_and_secrets(
        &self,
    ) -> (HashMap<(String, String), ConfigMap>, HashMap<(String, String), Secret>) {
        let cms = self
            .store
            .get_by_kind("ConfigMap")
            .await
            .into_iter()
            .filter_map(|t| {
                if let AnyResource::ConfigMap(cm) = t.resource {
                    let ns = cm.metadata.namespace.clone().unwrap_or_default();
                    let name = cm.metadata.name.clone().unwrap_or_default();
                    Some(((ns, name), cm))
                } else {
                    None
                }
            })
            .collect();

        let secrets = self
            .store
            .get_by_kind("Secret")
            .await
            .into_iter()
            .filter_map(|t| {
                if let AnyResource::Secret(sec) = t.resource {
                    let ns = sec.metadata.namespace.clone().unwrap_or_default();
                    let name = sec.metadata.name.clone().unwrap_or_default();
                    Some(((ns, name), sec))
                } else {
                    None
                }
            })
            .collect();

        (cms, secrets)
    }

    async fn service_target_ports_for_pod(&self, pod: &Pod) -> Vec<u16> {
        use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
        let pod_ns = pod.metadata.namespace.as_deref().unwrap_or("default");
        let pod_labels = pod.metadata.labels.clone().unwrap_or_default();
        let mut ports = Vec::new();
        let trackers = self.store.get_by_kind("Service").await;
        for t in trackers {
            if let AnyResource::Service(svc) = &t.resource {
                if svc.metadata.namespace.as_deref().unwrap_or("default") != pod_ns {
                    continue;
                }
                let selector = svc
                    .spec
                    .as_ref()
                    .and_then(|s| s.selector.as_ref())
                    .cloned()
                    .unwrap_or_default();
                if !selector.iter().all(|(k, v)| pod_labels.get(k) == Some(v)) {
                    continue;
                }
                if let Some(svc_ports) = svc.spec.as_ref().and_then(|s| s.ports.as_ref()) {
                    for sp in svc_ports {
                        let tp = sp
                            .target_port
                            .clone()
                            .unwrap_or_else(|| IntOrString::Int(sp.port));
                        if let IntOrString::Int(p) = tp {
                            ports.push(p as u16);
                        }
                    }
                }
            }
        }
        ports
    }

    async fn resolve_service_env(&self, pod: &Pod) -> Vec<(String, String)> {
        let pod_ns = pod.metadata.namespace.as_deref().unwrap_or("default");
        let trackers = self.store.get_by_kind("Service").await;
        let mut vars = Vec::new();
        for t in trackers {
            if let AnyResource::Service(svc) = &t.resource {
                if svc.metadata.namespace.as_deref().unwrap_or("default") != pod_ns {
                    continue;
                }
                let cluster_ip = svc.spec.as_ref()
                    .and_then(|s| s.cluster_ip.as_deref())
                    .unwrap_or("None");
                if cluster_ip == "None" || cluster_ip.is_empty() {
                    continue;
                }
                let svc_name = svc.metadata.name.as_deref().unwrap_or_default();
                let prefix = svc_name.to_uppercase().replace('-', "_");
                vars.push((format!("{}_SERVICE_HOST", prefix), cluster_ip.to_string()));
                if let Some(ports) = svc.spec.as_ref().and_then(|s| s.ports.as_ref()) {
                    for port in ports {
                        let port_str = port.port.to_string();
                        vars.push((format!("{}_SERVICE_PORT", prefix), port_str.clone()));
                        if let Some(pname) = &port.name {
                            let pname_up = pname.to_uppercase().replace('-', "_");
                            vars.push((format!("{}_SERVICE_PORT_{}", prefix, pname_up), port_str));
                        }
                    }
                }
            }
        }
        vars
    }

    fn resolve_env_from(
        container: &Container,
        pod: &Pod,
        cms: &HashMap<(String, String), ConfigMap>,
        secrets: &HashMap<(String, String), Secret>,
    ) -> Vec<(String, String)> {
        let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
        let mut vars = Vec::new();
        for env_from in container.env_from.as_deref().unwrap_or(&[]) {
            let prefix = env_from.prefix.as_deref().unwrap_or("");
            if let Some(cm_ref) = &env_from.config_map_ref {
                if let Some(cm) = cms.get(&(ns.to_string(), cm_ref.name.clone())) {
                    for (k, v) in cm.data.as_ref().into_iter().flatten() {
                        vars.push((format!("{}{}", prefix, k), v.clone()));
                    }
                } else {
                    warn!("envFrom configMapRef '{}' not found in ns '{}'", cm_ref.name, ns);
                }
            }
            if let Some(sec_ref) = &env_from.secret_ref {
                if let Some(sec) = secrets.get(&(ns.to_string(), sec_ref.name.clone())) {
                    for (k, v) in sec.data.as_ref().into_iter().flatten() {
                        match std::str::from_utf8(&v.0) {
                            Ok(s) => vars.push((format!("{}{}", prefix, k), s.to_string())),
                            Err(_) => warn!("Secret key '{}' is not valid UTF-8, skipping", k),
                        }
                    }
                    for (k, v) in sec.string_data.as_ref().into_iter().flatten() {
                        vars.push((format!("{}{}", prefix, k), v.clone()));
                    }
                } else {
                    warn!("envFrom secretRef '{}' not found in ns '{}'", sec_ref.name, ns);
                }
            }
        }
        vars
    }

    async fn spawn_container(
        &self,
        resource: &AnyResource,
        container: &Container,
        container_id: &str,
        rootfs_path: &str,
        pod_uid: &str,
        service_ports: &[u16],
    ) -> Result<RunningContainer> {
        let image = container.image.clone().unwrap_or_default();
        let is_native = rootfs_path.is_empty();

        let (entrypoint, cmd_args) = if is_native {
            let command = container.command.clone().unwrap_or_default();
            if command.is_empty() {
                anyhow::bail!("Native process container '{}' must have a command", container.name);
            }
            let args = container.args.clone().unwrap_or_default();
            let ep = command[0].clone();
            let rest: Vec<String> = command[1..].iter().chain(args.iter()).cloned().collect();
            (ep, rest)
        } else {
            crate::container::oci_config::resolve_argv(container, rootfs_path)
        };

        let pod_sc = match resource {
            AnyResource::Pod(pod) => pod.spec.as_ref().and_then(|s| s.security_context.as_ref()),
            _ => None,
        };
        let run_as_user = resolve_run_as_user(pod_sc, container);
        let run_as_group = resolve_run_as_group(pod_sc, container);

        let mut env_vars: Vec<(String, String)> = container
            .env
            .as_ref()
            .map(|env| {
                env.iter()
                    .map(|e| (e.name.clone(), e.value.clone().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default();

        // Resolve volumes, envFrom, and service env vars for pod resources
        let volumes = if let AnyResource::Pod(pod) = resource {
            let (cms, secrets) = self.fetch_cms_and_secrets().await;
            env_vars.extend(Self::resolve_env_from(container, pod, &cms, &secrets));
            // Inject service env vars (K8s-style: SVCNAME_SERVICE_HOST, SVCNAME_SERVICE_PORT)
            env_vars.extend(self.resolve_service_env(pod).await);
            if !is_native {
                crate::container::volumes::prepare_volumes(
                    pod,
                    &container.name,
                    pod_uid,
                    &|ns, name| cms.get(&(ns.to_string(), name.to_string())).cloned(),
                    &|ns, name| secrets.get(&(ns.to_string(), name.to_string())).cloned(),
                )
                .unwrap_or_default()
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Inject HOME if not already set
        if !env_vars.iter().any(|(k, _)| k == "HOME") {
            let home = match run_as_user {
                Some(0) | None => "/root".to_string(),
                Some(_) => "/home/user".to_string(),
            };
            env_vars.push(("HOME".to_string(), home));
        }

        let mut child_cmd = if is_native {
            let mut c = Command::new(&entrypoint);
            c.args(&cmd_args);
            c
        } else {
            if !volumes.is_empty() {
                crate::container::volumes::scrub_rootfs_volume_mounts(rootfs_path, &volumes);
                crate::container::volumes::stage_volumes_in_rootfs(rootfs_path, &volumes);
            }
            rootfs::prepare_rootfs(rootfs_path)?;

            let rootfs_owned = rootfs_path.to_string();

            let ctx = ContainerSpawnCtx {
                entrypoint: &entrypoint,
                cmd_args: &cmd_args,
                env_vars: &env_vars,
                rootfs_path: &rootfs_owned,
                container_id,
                pod_uid,
                image: &image,
                container,
                volumes,
                run_as_user,
                run_as_group,
            };
            if rootfs::is_root() {
                return self.spawn_root_ns_container(ctx, service_ports).await;
            } else {
                return self.spawn_userns_container(ctx, service_ports).await;
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

        self.build_running_container(child, container_id, rootfs_path, &image, container, &env_vars, service_ports).await
    }

    async fn spawn_root_ns_container(
        &self,
        ctx: ContainerSpawnCtx<'_>,
        _service_ports: &[u16],
    ) -> Result<RunningContainer> {
        let ContainerSpawnCtx { entrypoint, cmd_args, env_vars, rootfs_path, container_id, pod_uid, image, container, volumes, run_as_user, run_as_group } = ctx;
        let isolate_net = use_isolated_network(container);
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
                    container_name: container.name.clone(),
                    image: image.to_string(),
                    pid: Some(pid),
                    rootfs: rootfs_path.to_string(),
                    started_at: Some(chrono::Utc::now()),
                    env_vars: env_owned.clone(),
                    published_ports: std::collections::HashMap::new(),
                    isolated_net: isolate_net,
                };
                let publish_ports = merge_publish_ports(container, isolate_net);
                let (published_ports, port_publish) =
                    attach_port_publish(pid, &publish_ports);
                instance.published_ports = published_ports;

                Ok(RunningContainer {
                    child: None,
                    instance,
                    restart_count: 0,
                    log_buffer,
                    ready: Arc::new(Mutex::new(true)),
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

                if let Err(e) =
                    rootfs::child_enter_ns_root(&rootfs_owned, &volumes, isolate_net)
                {
                    eprintln!("z8s: root namespace setup failed: {}", e);
                    std::process::exit(1);
                }

                if let Some(gid) = run_as_group {
                    let _ = nix::unistd::setgid(nix::unistd::Gid::from_raw(gid));
                }
                if let Some(uid) = run_as_user {
                    let _ = nix::unistd::setuid(nix::unistd::Uid::from_raw(uid));
                }

                raise_nproc_limit();

                let (exec_path, prog_args) =
                    rootfs::build_container_argv(&entrypoint_owned, &args_owned, &rootfs_owned);
                let mut argv: Vec<std::ffi::CString> =
                    vec![std::ffi::CString::new(exec_path.clone()).unwrap()];
                for a in &prog_args {
                    argv.push(std::ffi::CString::new(a.as_str()).unwrap());
                }
                let envp: Vec<std::ffi::CString> = env_owned.iter()
                    .map(|(k, v)| std::ffi::CString::new(format!("{}={}", k, v)).unwrap())
                    .collect();

                let e = nix::unistd::execvpe(&argv[0], &argv, &envp)
                    .expect_err("execvpe returned unexpectedly");
                eprintln!("z8s: execvpe({}) failed: {}", exec_path, e);
                std::process::exit(1);
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

    async fn spawn_userns_container(
        &self,
        ctx: ContainerSpawnCtx<'_>,
        _service_ports: &[u16],
    ) -> Result<RunningContainer> {
        let ContainerSpawnCtx { entrypoint, cmd_args, env_vars, rootfs_path, container_id, pod_uid, image, container, volumes, run_as_user, run_as_group } = ctx;
        let isolate_net = use_isolated_network(container);
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
                    container_name: container.name.clone(),
                    image: image.to_string(),
                    pid: Some(pid),
                    rootfs: rootfs_path.to_string(),
                    started_at: Some(chrono::Utc::now()),
                    env_vars: env_owned.clone(),
                    published_ports: std::collections::HashMap::new(),
                    isolated_net: isolate_net,
                };
                let publish_ports = merge_publish_ports(container, isolate_net);
                let (published_ports, port_publish) =
                    attach_port_publish(pid, &publish_ports);
                instance.published_ports = published_ports;

                let ready = Arc::new(Mutex::new(true));
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

                if let Err(e) = rootfs::child_enter_ns_fork(
                    &rootfs_owned,
                    sync_w,
                    ack_r,
                    &volumes,
                    isolate_net,
                ) {
                    eprintln!("z8s: namespace setup failed: {:#}", e);
                    std::process::exit(1);
                }

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

                let (exec_path, prog_args) =
                    rootfs::build_container_argv(&entrypoint_owned, &args_owned, &rootfs_owned);
                let mut argv: Vec<std::ffi::CString> =
                    vec![std::ffi::CString::new(exec_path.clone()).unwrap()];
                for a in &prog_args {
                    argv.push(std::ffi::CString::new(a.as_str()).unwrap());
                }
                let envp: Vec<std::ffi::CString> = env_owned.iter()
                    .map(|(k, v)| std::ffi::CString::new(format!("{}={}", k, v)).unwrap())
                    .collect();

                let e = nix::unistd::execvpe(&argv[0], &argv, &envp)
                    .expect_err("execvpe returned unexpectedly");
                eprintln!("z8s: execvpe({}) failed: {}", exec_path, e);
                std::process::exit(1);
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
        container: &Container,
        env_vars: &[(String, String)],
        _service_ports: &[u16],
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

        let isolate_net = use_isolated_network(container);
        let mut instance = ContainerInstance {
            container_id: container_id.to_string(),
            container_name: container.name.clone(),
            image: image.to_string(),
            pid: Some(pid),
            rootfs: rootfs_path.to_string(),
            started_at: Some(chrono::Utc::now()),
            env_vars: env_vars.to_vec(),
            published_ports: std::collections::HashMap::new(),
            isolated_net: isolate_net,
        };
        let publish_ports = merge_publish_ports(container, isolate_net);
        let (published_ports, port_publish) = attach_port_publish(pid, &publish_ports);
        instance.published_ports = published_ports;

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

    pub async fn stop_pod(&self, resource: &AnyResource) {
        let pod_name = resource.name();
        for container in &extract_containers(resource) {
            self.stop_container(&format!("{}-{}", pod_name, container.name)).await;
        }
        let pod_uid = resource.uid();
        self.cgroup_manager.remove_cgroup(&pod_uid).ok();
        self.store.update_state(&pod_uid, ResourceState::Terminated).await;
        crate::container::volumes::cleanup_emptydir(&pod_uid);
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
            if cid.starts_with(&prefix) && *rc.ready.lock().await {
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

    pub async fn reconcile(&self) {
        let reaped = self.reap_zombies();
        if !reaped.is_empty() {
            self.handle_exited_containers(reaped).await;
        }
        let resources = self.store.get_all().await;
        for tracker in &resources {
            if !matches!(&tracker.resource, AnyResource::Pod(_)) {
                continue;
            }
            let uid = tracker.resource.uid();
            let pod_name = tracker.resource.name().to_string();
            let is_running = {
                let running = self.running.lock().await;
                running.keys().any(|cid| cid.starts_with(&format!("{}-", pod_name)))
            };
            if tracker.state == ResourceState::Pending && !is_running {
                if let Err(e) = self.start_pod(&tracker.resource).await {
                    error!("Failed to start {}: {:?}", uid, e);
                    self.store.update_state(&uid, ResourceState::Failed(e.to_string())).await;
                }
            }
        }
    }

    async fn handle_exited_containers(&self, reaped: Vec<(u32, i32)>) {
        // Map pid → exit_code for quick lookup
        let reaped_map: HashMap<u32, i32> = reaped.into_iter().collect();

        // Find containers whose PID was reaped
        let dead: Vec<(String, u32, i32)> = {
            let running = self.running.lock().await;
            running.values()
                .filter_map(|rc| {
                    let pid = rc.instance.pid?;
                    let code = *reaped_map.get(&pid)?;
                    Some((rc.instance.container_id.clone(), pid, code))
                })
                .collect()
        };

        for (container_id, pid, exit_code) in dead {
            let trackers = self.store.get_all().await;
            // Find the pod whose name is a prefix of this container_id
            let pod_tracker = trackers.iter().find(|t| {
                if !matches!(&t.resource, AnyResource::Pod(_)) { return false; }
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

            // Remove dead container from running map
            self.running.lock().await.remove(&container_id);

            if should_restart {
                // Increment restart count and let reconcile loop restart via Pending state
                let mut counts = self.restart_counts.lock().await;
                let count = counts.entry(container_id.clone()).or_insert(0);
                *count += 1;
                drop(counts);

                if !pod_uid.is_empty() {
                    // Reset to Pending so reconcile restarts the pod
                    self.store.update_state(&pod_uid, ResourceState::Pending).await;
                }
            } else {
                // No restart — set terminal state
                if !pod_uid.is_empty() {
                    if exit_code == 0 {
                        self.store.update_state(&pod_uid, ResourceState::Succeeded).await;
                    } else {
                        self.store
                            .update_state(&pod_uid, ResourceState::Failed(format!("exit code {}", exit_code)))
                            .await;
                    }
                }
            }
        }
    }

    fn reap_zombies(&self) -> Vec<(u32, i32)> {
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
}
