mod api;
mod builder;
mod container;
mod controller;
mod init;
mod manifest;
mod server;
mod supervisor;

use crate::api::types::ResourceStore;
use crate::container::image::ImageManager;
use crate::controller::DeploymentController;
use crate::init::InitHandler;
use crate::manifest::watcher::ManifestWatcher;
use crate::supervisor::cgroup::CgroupManager;
use crate::supervisor::health::HealthChecker;
use crate::supervisor::process::ProcessSupervisor;
use anyhow::Result;
use std::sync::Arc;
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_thread_ids(true)
        .init();

    info!("z8s v{} starting...", env!("CARGO_PKG_VERSION"));

    let pid = std::process::id();
    if pid == 1 {
        info!("Running as PID 1 (init process)");
    } else {
        warn!(
            "Not running as PID 1 (pid={}). Some init features will be unavailable.",
            pid
        );
    }

    let store = Arc::new(ResourceStore::new());

    let cgroup_manager = Arc::new(CgroupManager::new().unwrap_or_else(|e| {
        warn!("Cgroups not available: {}. Running without resource limits.", e);
        std::fs::create_dir_all("/sys/fs/cgroup/z8s").ok();
        CgroupManager::new().expect("Failed to create cgroup manager even with fallback")
    }));

    let image_manager = Arc::new(ImageManager::new().unwrap_or_else(|e| {
        warn!("Image manager init failed: {}. Running without image pulling.", e);
        ImageManager::new().expect("Failed to create image manager")
    }));

    let health_checker = Arc::new(HealthChecker::new());

    let supervisor = Arc::new(ProcessSupervisor::new(
        image_manager,
        cgroup_manager,
        health_checker,
        store.clone(),
    ));

    let watcher = Arc::new(ManifestWatcher::new(store.clone()));

    let controller =
        Arc::new(DeploymentController::new(store.clone(), supervisor.clone()));

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let init_handler = InitHandler::new().unwrap_or_else(|e| {
        error!("Failed to create init handler: {}", e);
        panic!("Init handler required");
    });

    if let Err(e) = watcher.load_existing().await {
        error!("Failed to load existing manifests: {}", e);
    }

    supervisor.reconcile().await;
    controller.reconcile_deployments().await;

    let watcher_clone = watcher.clone();
    tokio::spawn(async move {
        if let Err(e) = watcher_clone.start_watching().await {
            error!("Manifest watcher failed: {}", e);
        }
    });

    let controller_clone = controller.clone();
    tokio::spawn(async move {
        controller_clone.run().await;
    });

    let supervisor_clone = supervisor.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(10));
        loop {
            ticker.tick().await;
            supervisor_clone.reconcile().await;
        }
    });

    let store_clone = store.clone();
    tokio::spawn(async move {
        server::run_server(store_clone).await;
    });

    info!("z8s is ready. Watching /etc/z8s/manifests/ for manifests.");
    if pid == 1 {
        init_handler.run(&shutdown_tx).await.ok();
    } else {
        signal::ctrl_c().await?;
        info!("Received Ctrl+C, shutting down...");
        let _ = shutdown_tx.send(true);
    }

    let _ = shutdown_rx.changed().await;
    info!("Shutting down all services...");

    let resources = store.get_all().await;
    for tracker in &resources {
        supervisor.stop_pod(&tracker.resource).await;
    }

    info!("z8s shutdown complete.");
    Ok(())
}
