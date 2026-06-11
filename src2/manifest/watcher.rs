use crate::api::AnyResource;
use crate::store::ops::{StoreChange, StoreOp};
use crate::store::{parse_manifest_yaml, StoreBackend, StoreEventHub};
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};
use tracing::{error, info, warn};

pub struct ManifestWatcher {
    store: Arc<dyn StoreBackend>,
    store_events: StoreEventHub,
    scheduler_notify: Arc<Notify>,
    dir: String,
    processed: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl ManifestWatcher {
    pub fn new(
        store: Arc<dyn StoreBackend>,
        store_events: StoreEventHub,
        scheduler_notify: Arc<Notify>,
    ) -> Self {
        Self {
            store,
            store_events,
            scheduler_notify,
            dir: crate::config::get().manifests_dir.clone(),
            processed: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    pub async fn load_existing(&self) -> Result<()> {
        let dir = Path::new(&self.dir);
        if !dir.exists() {
            std::fs::create_dir_all(dir).context(format!(
                "Failed to create manifests directory: {}",
                self.dir
            ))?;
            info!("Created manifests directory: {}", self.dir);
            return Ok(());
        }
        self.load_dir_recursive(dir).await
    }

    /// Recursively load all YAML manifests from a directory tree (iterative BFS).
    async fn load_dir_recursive(&self, root: &Path) -> Result<()> {
        let yaml_paths = collect_yaml_paths(root);
        for path in yaml_paths {
            if let Err(e) = self.process_file(&path).await {
                error!("Failed to process {}: {}", path.display(), e);
            }
        }
        Ok(())
    }

    async fn process_file(&self, path: &Path) -> Result<()> {
        let content =
            std::fs::read_to_string(path).context(format!("Failed to read {}", path.display()))?;

        if content.trim().is_empty() {
            warn!("Empty manifest file: {}", path.display());
            return Ok(());
        }

        let resources = parse_manifest_yaml(&content)
            .context(format!("Failed to parse YAML in {}", path.display()))?;

        let mut applied = Vec::new();
        for resource in &resources {
            info!(
                "Loaded resource: {} '{}' in namespace '{}'",
                resource.kind(),
                resource.name(),
                resource.namespace()
            );
            let resource = set_default_namespace(resource);
            applied.push(resource);
        }

        if !applied.is_empty() {
            let ops: Vec<StoreOp> = applied.iter().cloned().map(StoreOp::Upsert).collect();
            self.store.apply_batch(ops).await?;
            for resource in applied {
                self.store_events
                    .emit_applied(resource, StoreChange::Created);
            }
            self.scheduler_notify.notify_one();
        }

        let mut processed = self.processed.write().await;
        processed.insert(path.to_string_lossy().to_string());
        drop(processed);

        Ok(())
    }

    pub async fn start_watching(self: Arc<Self>) -> Result<()> {
        let dir = self.dir.clone();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Event>>(256);

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                let tx = tx.clone();
                let _ = tx.blocking_send(res.map_err(|e| anyhow::anyhow!(e)));
            },
            Config::default(),
        )
        .context("Failed to create file watcher")?;

        watcher
            .watch(Path::new(&dir), RecursiveMode::Recursive)
            .context(format!("Failed to watch directory: {}", dir))?;

        info!("Watching manifests directory: {}", dir);

        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
                    ) {
                        for path in event.paths {
                            if is_yaml(&path) {
                                if let Err(e) = self.process_file(&path).await {
                                    error!("Failed to process {}: {}", path.display(), e);
                                }
                            }
                        }
                    }
                }
                Err(e) => error!("Watcher error: {}", e),
            }
        }
        Ok(())
    }
}

fn is_yaml(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "yaml" || e == "yml")
        .unwrap_or(false)
}

fn collect_yaml_paths(root: &Path) -> Vec<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let mut yaml_paths = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if is_yaml(&path) {
                yaml_paths.push(path);
            }
        }
    }
    yaml_paths
}

fn set_default_namespace(resource: &AnyResource) -> AnyResource {
    let mut r = resource.clone();
    let meta = r.metadata_mut();
    if meta.namespace.is_none() {
        meta.namespace = Some("default".into());
    }
    r
}
