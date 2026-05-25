use crate::api::types::{parse_manifest_yaml, ResourceStore};
use crate::api::AnyResource;
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

const DEFAULT_MANIFESTS_DIR: &str = "/etc/z8s/manifests";

pub struct ManifestWatcher {
    store: Arc<ResourceStore>,
    dir: String,
    processed: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl ManifestWatcher {
    pub fn new(store: Arc<ResourceStore>) -> Self {
        Self {
            store,
            dir: DEFAULT_MANIFESTS_DIR.to_string(),
            processed: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    pub fn with_dir(mut self, dir: &str) -> Self {
        self.dir = dir.to_string();
        self
    }

    pub async fn load_existing(&self) -> Result<()> {
        let dir = Path::new(&self.dir);
        if !dir.exists() {
            std::fs::create_dir_all(dir)
                .context(format!("Failed to create manifests directory: {}", self.dir))?;
            info!("Created manifests directory: {}", self.dir);
            return Ok(());
        }

        let mut entries = std::fs::read_dir(dir)
            .context(format!("Failed to read manifests directory: {}", self.dir))?;

        while let Some(entry) = entries.next().transpose()? {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "yaml" || e == "yml") {
                if let Err(e) = self.process_file(&path).await {
                    error!("Failed to process {}: {}", path.display(), e);
                }
            }
        }

        Ok(())
    }

    async fn process_file(&self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)
            .context(format!("Failed to read {}", path.display()))?;

        if content.trim().is_empty() {
            warn!("Empty manifest file: {}", path.display());
            return Ok(());
        }

        let resources = parse_manifest_yaml(&content)
            .context(format!("Failed to parse YAML in {}", path.display()))?;

        for resource in &resources {
            info!(
                "Loaded resource: {} '{}' in namespace '{}'",
                resource.kind(),
                resource.name(),
                resource.namespace()
            );

            // Set default namespace if not specified
            let resource = set_default_namespace(resource);

            self.store.apply(resource)
                .await?;
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
            .watch(Path::new(&dir), RecursiveMode::NonRecursive)
            .context(format!("Failed to watch directory: {}", dir))?;

        info!("Watching manifests directory: {}", dir);

        while let Some(event) = rx.recv().await {
            match event {
                Ok(event) => {
                    match event.kind {
                        EventKind::Create(_)
                        | EventKind::Modify(_) => {
                            for path in &event.paths {
                                if !path.extension().map_or(false, |e| e == "yaml" || e == "yml") {
                                    continue;
                                }
                                // Skip files already loaded during startup (spurious watcher events)
                                let processed = self.processed.read().await;
                                if processed.contains(&path.to_string_lossy().to_string()) {
                                    continue;
                                }
                                drop(processed);
                                info!("Detected change in: {}", path.display());
                                if let Err(e) = self.process_file(path).await {
                                    error!("Failed to process {}: {}", path.display(), e);
                                }
                            }
                        }
                        EventKind::Remove(_) => {
                            for path in &event.paths {
                                let processed = self.processed.read().await;
                                if processed.contains(&path.to_string_lossy().to_string()) {
                                    // Resource removal would be handled here
                                    info!("Manifest removed: {}", path.display());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    error!("File watch error: {}", e);
                }
            }
        }

        Ok(())
    }
}

fn set_default_namespace(resource: &AnyResource) -> AnyResource {
    let mut resource = resource.clone();
    resource
        .metadata_mut()
        .namespace
        .get_or_insert_with(|| "default".to_string());
    resource
}
