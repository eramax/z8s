mod api;
mod components;
mod config;
mod cri;
mod init;
mod manifest;
mod netmux;
mod scheduler;
mod storage;
mod store;
mod types;

use crate::store::{AnyResource, StoreBackend, MemoryBackend};
use crate::components::{ComponentRegistry, PipelineBuilder, ReconcileContext};
use crate::components::compute::deployment::DeploymentResource;
use crate::components::compute::pod::PodResource;
use crate::components::network::service::ServiceResource;
use crate::components::network::ingress::IngressResource;
use crate::components::network::networkpolicy::NetworkPolicyResource;
use crate::components::network::vnet::VNetResource;
use crate::components::network::subnet::SubnetResource;
use crate::components::network::nsg::NsgResource;
use crate::components::network::routetable::RouteTableResource;
use crate::components::storage::configmap::ConfigMapResource;
use crate::components::storage::pv::PvResource;
use crate::components::storage::pvc::PvcResource;
use crate::components::storage::secret::SecretResource;
use crate::cri::image::ImageManager;
use crate::cri::runtime::ContainerRuntime;
use crate::init::InitHandler;
use crate::manifest::watcher::ManifestWatcher;
use crate::components::network::service::NetworkManager;
use crate::scheduler::reconciler::Reconciler;
use crate::cri::cgroup::CgroupManager;
use crate::cri::runtime::ProcessSupervisor;
use crate::scheduler::process::ProcessTracker;
use crate::storage::ProvisionerDispatcher;
use anyhow::Result;
use nix::libc;
use std::sync::Arc;
use tokio::signal;
use tokio::signal::unix::{SignalKind, signal as unix_signal};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn lock_file_path(port: u16) -> String {
    format!("/tmp/z8s-{}.lock", port)
}

fn acquire_port_lock(port: u16) -> Result<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    let path = lock_file_path(port);
    let file = std::fs::File::create(&path)
        .map_err(|e| anyhow::anyhow!("Failed to create lock file {path}: {e}"))?;
    let ret = unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    if ret != 0 {
        anyhow::bail!("z8s is already running on port {port} (lock held). If stale, remove {path} and retry.");
    }
    Ok(file)
}

fn daemonize(port: u16, foreground: bool) -> Result<std::fs::File> {
    let _ = foreground;
    let lock = acquire_port_lock(port)?;
    // Write PID into the lock file so `z8s restart` can find us.
    use std::io::Write;
    let mut file = lock;
    file.set_len(0).ok();
    write!(file, "{}\n", std::process::id()).ok();
    file.flush().ok();
    Ok(file)
}

fn find_z8s_pid() -> Result<i32> {
    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => anyhow::bail!("Cannot access /proc"),
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(pid) = name_str.parse::<i32>() {
            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
                if comm.trim() == "z8s" {
                    return Ok(pid);
                }
            }
        }
    }
    anyhow::bail!("No z8s process found");
}

fn restart_z8s(port: u16) -> Result<()> {
    let lock_path = lock_file_path(port);

    // Try to read PID from lock file.
    let lock_pid: Option<i32> = std::fs::read_to_string(&lock_path).ok()
        .and_then(|s| s.trim().parse().ok());

    let pid = if let Some(pid) = lock_pid {
        if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok() {
            pid
        } else {
            eprintln!("Warning: lock file has stale PID {pid}, searching for z8s process...");
            find_z8s_pid()?
        }
    } else {
        find_z8s_pid()?
    };

    if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM).is_err() {
        anyhow::bail!("Failed to signal z8s (PID {pid}). Is it running?");
    }
    eprintln!("Sent SIGTERM to z8s (PID {pid}), waiting for exit...");

    // Wait for the process to exit.
    for _ in 0..50 {
        if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_secs(1));

    let self_path = std::env::current_exe()
        .map_err(|_| anyhow::anyhow!("Cannot determine executable path"))?;
    std::process::Command::new(&self_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to start z8s: {e}"))?;
    eprintln!("z8s restarted.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let foreground = args.iter().any(|a| a == "--foreground" || a == "-f");

    // ── Commands that don't require config ───────────────────────────
    let cmd = args.get(1).map(|s| s.as_str());
    let self_port = args.iter().position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(6443);

    if cmd == Some("restart") {
        return restart_z8s(self_port);
    }

    if cmd == Some("node") {
        match args.get(2).map(|s| s.as_str()) {
            Some("start") => {
                let node_port = args.iter().position(|a| a == "--port")
                    .and_then(|i| args.get(i + 1))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(7443);
                let service_cidr = args.iter().position(|a| a == "--service-cidr")
                    .and_then(|i| args.get(i + 1)).map(|s| s.to_string());
                let pod_cidr = args.iter().position(|a| a == "--pod-cidr")
                    .and_then(|i| args.get(i + 1)).map(|s| s.to_string());
                let peer = format!("127.0.0.1:{}", self_port);
                let node_name = format!("node-{node_port}");
                let self_path = std::env::current_exe()
                    .map_err(|_| anyhow::anyhow!("Cannot determine executable path"))?;
                let mut cmd = std::process::Command::new(&self_path);
                cmd.arg("--port").arg(node_port.to_string())
                    .arg("--peers").arg(format!("{}={}", node_name, peer))
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .stdin(std::process::Stdio::null());
                if let Some(cidr) = service_cidr {
                    cmd.arg("--service-cidr").arg(cidr);
                }
                if let Some(cidr) = pod_cidr {
                    cmd.arg("--pod-cidr").arg(cidr);
                }
                cmd.spawn()
                    .map_err(|e| anyhow::anyhow!("Failed to start node: {e}"))?;
                eprintln!("Node started on port {}.", node_port);
                return Ok(());
            }
            _ => {
                anyhow::bail!("Usage: z8s node start --port <PORT> [--service-cidr <CIDR>] [--pod-cidr <CIDR>]");
            }
        }
    }

    crate::config::init();
    let cfg = crate::config::get();
    let port = cfg.api_port;

    // ── Join mode ──────────────────────────────────────────────────
    if args.get(1).map(|s| s.as_str()) == Some("join") {
        let url = args.get(2).expect("Usage: z8s join <ws-url> [--token <token>]");
        let token = if args.len() > 3 && args[3] == "--token" {
            args.get(4).map(|s| s.to_string())
        } else {
            cfg.join_token.clone()
        };
        info!("Joining cluster at {} as worker", url);
        join_cluster(url, token).await;
        return Ok(());
    }

    let _lock = daemonize(port, foreground)?;

    info!(
        "z8s v{} starting — port={}, service-cidr={}.{}.{}.{}/{}, domain={}, manifests={}",
        env!("CARGO_PKG_VERSION"),
        cfg.api_port,
        cfg.service_cidr_base[0], cfg.service_cidr_base[1],
        cfg.service_cidr_base[2], cfg.service_cidr_base[3],
        cfg.service_cidr_prefix,
        cfg.cluster_domain,
        cfg.manifests_dir,
    );

    let pid = std::process::id();
    if pid == 1 {
        info!("Running as PID 1 (init process)");
    } else {
        warn!(
            "Not running as PID 1 (pid={}). Some init features will be unavailable.",
            pid
        );
    }

    let apparmor_restricted = std::fs::read_to_string(
        "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
    )
    .ok()
    .and_then(|s| s.trim().parse::<u32>().ok())
    .unwrap_or(0)
        == 1;

    if apparmor_restricted && !crate::cri::rootfs::is_root() {
        warn!(
            "AppArmor restricts unprivileged user namespaces \
             (kernel.apparmor_restrict_unprivileged_userns=1). \
             Containers will run in degraded mode without filesystem isolation. \
             Run z8s as root, or install the AppArmor profile: \
             sudo apparmor_parser -r /etc/apparmor.d/z8s"
        );
    }

    let store: Arc<dyn StoreBackend>;
    let redb: Option<Arc<crate::store::RedbBackend>>;

    if let Some(ref db_path) = cfg.db_path {
        let path = std::path::Path::new(db_path);
        let parent = path.parent().unwrap_or(std::path::Path::new("/tmp"));
        match crate::store::RedbBackend::open_at(parent, path.file_name().unwrap().to_str().unwrap()) {
            Ok(db) => {
                info!("Using redb database at {}", db_path);
                let db = Arc::new(db);
                store = db.clone() as Arc<dyn StoreBackend>;
                redb = Some(db);
            }
            Err(e) => {
                warn!("Failed to open redb at {}: {}. Falling back to in-memory.", db_path, e);
                store = Arc::new(MemoryBackend::new());
                redb = None;
            }
        }
    } else if let Some(ref data_dir) = cfg.data_dir {
        match crate::store::RedbBackend::open(data_dir) {
            Ok(db) => {
                info!("Using redb database at {}", data_dir);
                let db = Arc::new(db);
                store = db.clone() as Arc<dyn StoreBackend>;
                redb = Some(db);
            }
            Err(e) => {
                warn!("Failed to open redb at {}: {}. Falling back to in-memory.", data_dir, e);
                store = Arc::new(MemoryBackend::new());
                redb = None;
            }
        }
    } else {
        store = Arc::new(MemoryBackend::new());
        redb = None;
    };

    let cgroup_manager = Arc::new(CgroupManager::new().unwrap_or_else(|e| {
        warn!("Cgroups not available: {}. Running without resource limits.", e);
        CgroupManager::new().expect("cgroup manager init failed twice")
    }));

    let image_manager = Arc::new(ImageManager::new().unwrap_or_else(|e| {
        warn!("Image manager init failed: {}. Running without image pulling.", e);
        ImageManager::new().expect("Failed to create image manager")
    }));

    let netmux = Arc::new(crate::netmux::NetMux::new(&cfg.pod_cidr).unwrap_or_else(|e| {
        panic!("Failed to create NetMux with pod CIDR {}: {}", cfg.pod_cidr, e);
    }));

    // Set DNS server IP for pods to reach the embedded DNS server via the veth gateway
    crate::config::set_dns_server(netmux.gateway().to_string());

    if let Err(e) = crate::netmux::NetMux::enable_ip_forward() {
        warn!("Failed to enable ip_forward: {} — pods may not reach the internet", e);
    }
    if let Err(e) = crate::netmux::NetMux::ensure_loopback_up() {
        warn!("Failed to bring up loopback: {}", e);
    }
    crate::netmux::netlink::harden_sysctl().ok();
    if let Err(e) = netmux.nft.init(&cfg.pod_cidr).await {
        panic!("nftables init failed: {} — nftables is required, refusing to start", e);
    }
    if let Err(e) = netmux.nft.add_forward_catchall(&cfg.pod_cidr).await {
        warn!("Failed to add forward catch-all: {}", e);
    }
    if let Err(e) = netmux.nft.add_snat("default", &cfg.pod_cidr).await {
        warn!("Failed to add SNAT: {} — pods may not reach internet", e);
    }

    // Clean orphan veths from previous runs
    if let Err(e) = netmux.clean_orphan_veths(&[]) {
        warn!("Failed to clean orphan veths: {}", e);
    }

    let supervisor = Arc::new(ProcessSupervisor::new(
        image_manager,
        cgroup_manager.clone(),
        netmux.clone(),
    ));

    let cri = Arc::new(ContainerRuntime::new(
        supervisor.clone(),
        cgroup_manager.clone(),
    ));

    let process_tracker = Arc::new(ProcessTracker {
        running: supervisor.running.clone(),
        restart_counts: supervisor.restart_counts.clone(),
        cri: cri.clone(),
        store: store.clone(),
    });

    let network = Arc::new(NetworkManager::new(store.clone(), process_tracker.clone(), netmux.clone()));

    if let Some(port) = crate::netmux::dns::run_dns(store.clone(), netmux.dns_records.clone()).await {
        crate::config::set_dns_port(port);
    }

    let watcher = Arc::new(ManifestWatcher::new(store.clone()));

    let pipeline = Arc::new(PipelineBuilder::new()

        .build());

    let provisioner = Arc::new(ProvisionerDispatcher::new(store.clone()));

    let ctx = Arc::new(ReconcileContext {
        store: store.clone(),
        pipeline: pipeline.clone(),
        cri: cri.clone(),
        net: network.clone() as Arc<dyn crate::netmux::network::NetworkEngine>,
        process_tracker: process_tracker.clone(),
        vol: provisioner.clone() as Arc<dyn crate::storage::StorageProvisioner>,
        netmux: netmux.clone(),
    });

    let mut registry = ComponentRegistry::new();
    registry.register(Box::new(PodResource::new()));
    registry.register(Box::new(DeploymentResource::new(store.clone())));
    registry.register(Box::new(ServiceResource::new(store.clone(), network.clone())));
    registry.register(Box::new(IngressResource::new(store.clone(), netmux.clone())));
    registry.register(Box::new(NetworkPolicyResource::new(netmux.clone())));
    registry.register(Box::new(VNetResource::new(netmux.clone())));
    registry.register(Box::new(SubnetResource::new(netmux.clone())));
    registry.register(Box::new(NsgResource::new(netmux.clone())));
    registry.register(Box::new(RouteTableResource::new()));
    registry.register(Box::new(ConfigMapResource::new(store.clone())));
    registry.register(Box::new(SecretResource::new(store.clone())));
    registry.register(Box::new(PvResource::new(store.clone())));
    registry.register(Box::new(PvcResource::new(store.clone())));
    let registry = Arc::new(registry);

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let init_handler = InitHandler::new().unwrap_or_else(|e| {
        error!("Failed to create init handler: {}", e);
        panic!("Init handler required");
    });

    if let Err(e) = watcher.load_existing().await {
        error!("Failed to load existing manifests: {}", e);
    }

    // Watcher
    let watcher_clone = watcher.clone();
    tokio::spawn(async move {
        if let Err(e) = watcher_clone.start_watching().await {
            error!("Manifest watcher failed: {}", e);
        }
    });

    // Unified reconciler: zombie reaping + component reconciliation
    let reconciler = Arc::new(Reconciler::new(
        registry.clone(),
        ctx.clone(),
        process_tracker.clone(),
    ));
    let rec = reconciler.clone();
    tokio::spawn(async move { rec.run().await });

    // Gossip state (always initialize for incoming connections)
    let gossip_state = {
        let mut gs = crate::store::gossip::GossipState::new(cfg.node_name.clone(), store.clone());
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(gs));

        // Connect to each peer
        for (name, addr) in &cfg.peers {
            let url = format!("ws://{}/ws/gossip", addr);
            let st = state.clone();
            let n = name.clone();

            // Create per-peer broadcast channel
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    let mut gs = st.lock().await;
                    gs.add_peer(tx);
                });
            });

            tokio::spawn(async move {
                crate::store::ws::run_gossip_client(n, url, st, rx).await;
            });

            // Anti-entropy per peer
            let ae_state = state.clone();
            let ae_name = name.clone();
            tokio::spawn(async move {
                crate::store::anti_entropy::run_anti_entropy(ae_name, ae_state).await;
            });
        }

        Some(state)
    };

    // Register this node as a Node resource (after gossip state is initialized)
    {
        let mut node = crate::api::handlers::node::make_local_node();
        if node.metadata.creation_timestamp.is_none() {
            node.metadata.creation_timestamp = Some(crate::types::Time::now());
        }
        store.apply(AnyResource::Node(node.clone())).await.ok();
        if let Some(ref gs) = gossip_state {
            gs.lock().await.broadcast_write(&AnyResource::Node(node)).await;
        }
    }

    // API server
    let store_clone = store.clone();
    let _s2 = supervisor.clone();
    let pt2 = process_tracker.clone();
    let reg2 = registry.clone();
    let ctx2 = ctx.clone();
    let gs = gossip_state.clone();
    tokio::spawn(async move {
        api::server::run_server(store_clone, pt2, reg2, ctx2, gs).await;
    });

    // Heartbeat + scheduler
    if let Some(ref db) = redb {
        let hb_db = db.clone();
        let hb_name = cfg.node_name.clone();
        let hb_ip = cfg.node_ip.clone();
        tokio::spawn(async move {
            crate::store::leases::run_heartbeat(hb_db, hb_name, hb_ip).await;
        });

        let sched_store = store.clone();
        let sched_name = cfg.node_name.clone();
        let sched_db = db.clone();
        tokio::spawn(async move {
            crate::scheduler::scheduler::run_scheduler(sched_store, sched_name, sched_db).await;
        });
    }

    // L7 ingress HTTP listener
    {
        let netmux = netmux.clone();
        let ing_store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::netmux::ingress::start_http(netmux.ingress_state.clone(), ing_store).await {
                error!("Ingress HTTP listener failed: {}", e);
            }
        });
    }

    // Initial reconcile
    let pt = process_tracker.clone();
    let s = store.clone();
    let rc = registry.clone();
    let cx = ctx.clone();
    tokio::spawn(async move {
        let reaped = pt.reap_zombies();
        if !reaped.is_empty() {
            pt.handle_exited_containers(reaped, &s).await;
        }
        rc.reconcile_all(&cx).await;
    });

    info!("z8s is ready. Watching /etc/z8s/manifests/ for manifests.");
    if pid == 1 {
        init_handler.run(&shutdown_tx).await.ok();
    } else {
        let mut sigterm = unix_signal(SignalKind::terminate())?;
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("Received Ctrl+C, shutting down...");
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down...");
            }
        }
        let _ = shutdown_tx.send(true);
    }

    let _ = shutdown_rx.changed().await;
    info!("Shutting down all services...");

    let resources = store.get_all().await;
    for tracker in &resources {
        let spec = crate::components::compute::spec_builder::build_spec(&tracker.resource, store.as_ref()).await;
        let _ = crate::cri::RuntimeProvider::stop_pod(cri.as_ref(), &spec).await;
    }

    let _ = std::fs::remove_file(lock_file_path(port));
    info!("z8s shutdown complete.");
    Ok(())
}

/// Connect to a cluster as a worker via WebSocket.
async fn join_cluster(url: &str, _token: Option<String>) {
    use std::time::Duration;
    use tokio::time::sleep;
    use tracing::warn;

    loop {
        match tokio_tungstenite::connect_async(url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to cluster at {}", url);
                let (mut _write, mut read) = ws_stream.split();

                // Listen for messages until disconnect
                use futures_util::StreamExt;
                loop {
                    match read.next().await {
                        Some(Ok(msg)) => {
                            if msg.is_close() {
                                info!("Server closed connection");
                                break;
                            }
                            // Worker would process incoming messages here
                        }
                        Some(Err(e)) => {
                            warn!("WebSocket error: {}", e);
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to {}: {}. Retrying in 5s...", url, e);
                sleep(Duration::from_secs(5)).await;
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}
