use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::store::ops::StoreOp;
use crate::store::snapshot::StoreSnapshot;
use crate::store::{AnyResource, ResourceState, ResourceTracker};

use super::backend::StoreBackend;

pub struct MemoryBackend {
    resources: RwLock<HashMap<String, ResourceTracker>>,
}

impl MemoryBackend {
    pub fn new() -> Self {
        Self {
            resources: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl StoreBackend for MemoryBackend {
    async fn apply(&self, resource: AnyResource) -> anyhow::Result<()> {
        let uid = resource.uid();
        let mut store = self.resources.write().await;
        let existing_state = store.get(&uid).map(|t| t.state.clone());
        let mut tracker = ResourceTracker::new(resource);
        if let Some(state) = existing_state {
            tracker.state = state;
        }
        store.insert(uid, tracker);
        Ok(())
    }

    async fn delete(&self, resource: &AnyResource) -> anyhow::Result<()> {
        let uid = resource.uid();
        self.resources.write().await.remove(&uid);
        Ok(())
    }

    async fn get_all(&self) -> Vec<ResourceTracker> {
        self.resources.read().await.values().cloned().collect()
    }

    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker> {
        self.resources
            .read()
            .await
            .values()
            .filter(|t| t.resource.kind() == kind)
            .cloned()
            .collect()
    }

    async fn get(&self, uid: &str) -> Option<ResourceTracker> {
        self.resources.read().await.get(uid).cloned()
    }

    async fn update_state(&self, uid: &str, state: ResourceState) {
        let mut store = self.resources.write().await;
        if let Some(tracker) = store.get_mut(uid) {
            tracker.state = state;
            tracker.last_updated = crate::config::now_rfc3339();
        }
    }

    async fn apply_batch(&self, ops: Vec<StoreOp>) -> anyhow::Result<()> {
        let mut store = self.resources.write().await;
        for op in ops {
            match op {
                StoreOp::Upsert(resource) => {
                    let uid = resource.uid();
                    let existing_state = store.get(&uid).map(|t| t.state.clone());
                    let mut tracker = ResourceTracker::new(resource);
                    if let Some(state) = existing_state {
                        tracker.state = state;
                    }
                    store.insert(uid, tracker);
                }
                StoreOp::Delete(resource) => {
                    store.remove(&resource.uid());
                }
            }
        }
        Ok(())
    }

    async fn snapshot(&self) -> StoreSnapshot {
        let store = self.resources.read().await;
        StoreSnapshot::from_trackers(store.values().cloned().collect())
    }
}
