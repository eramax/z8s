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
use std::sync::Arc;
use tokio::signal;
use tokio::signal::unix::{SignalKind, signal as unix_signal};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    crate::config::init();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_thread_ids(true)
        .init();

    let cfg = crate::config::get();
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

    let store: Arc<dyn StoreBackend> = if let Some(ref data_dir) = cfg.data_dir {
        match crate::store::RedbBackend::open(data_dir) {
            Ok(db) => {
                info!("Using redb database at {}", data_dir);
                Arc::new(db)
            }
            Err(e) => {
                warn!("Failed to open redb at {}: {}. Falling back to in-memory.", data_dir, e);
                Arc::new(MemoryBackend::new())
            }
        }
    } else {
        Arc::new(MemoryBackend::new())
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

    // Gossip state (shared between WS server and clients)
    let gossip_state = if cfg!(feature = "gossip") || !cfg.peers.is_empty() {
        let state = Arc::new(tokio::sync::Mutex::new(
            crate::store::gossip::GossipState::new(cfg.node_name.clone(), store.clone())
        ));

        // Connect to each peer
        for (name, addr) in &cfg.peers {
            let url = format!("ws://{}/ws/gossip", addr);
            let st = state.clone();
            let n = name.clone();
            tokio::spawn(async move {
                crate::store::ws::run_gossip_client(n, url, st).await;
            });

            // Anti-entropy per peer
            let ae_state = state.clone();
            let ae_name = name.clone();
            tokio::spawn(async move {
                crate::store::anti_entropy::run_anti_entropy(ae_name, ae_state).await;
            });
        }

        Some(state)
    } else {
        None
    };

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

    info!("z8s shutdown complete.");
    Ok(())
}
