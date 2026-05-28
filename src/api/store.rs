use crate::api::AnyResource;
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceState {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Terminated,
}

#[derive(Debug, Clone)]
pub struct ResourceTracker {
    pub resource: AnyResource,
    pub state: ResourceState,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

impl ResourceTracker {
    pub fn new(resource: AnyResource) -> Self {
        Self {
            resource,
            state: ResourceState::Pending,
            last_updated: chrono::Utc::now(),
        }
    }
}

pub struct ResourceStore {
    resources: RwLock<HashMap<String, ResourceTracker>>,
}

impl ResourceStore {
    pub fn new() -> Self {
        Self {
            resources: RwLock::new(HashMap::new()),
        }
    }

    pub async fn apply(&self, resource: AnyResource) -> anyhow::Result<()> {
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

    pub async fn delete(&self, resource: &AnyResource) -> anyhow::Result<()> {
        let uid = resource.uid();
        self.resources.write().await.remove(&uid);
        Ok(())
    }

    pub async fn get_all(&self) -> Vec<ResourceTracker> {
        self.resources.read().await.values().cloned().collect()
    }

    pub async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker> {
        self.resources
            .read()
            .await
            .values()
            .filter(|t| t.resource.kind() == kind)
            .cloned()
            .collect()
    }

    pub async fn get(&self, uid: &str) -> Option<ResourceTracker> {
        self.resources.read().await.get(uid).cloned()
    }

    pub async fn update_state(&self, uid: &str, state: ResourceState) {
        let mut store = self.resources.write().await;
        if let Some(tracker) = store.get_mut(uid) {
            tracker.state = state;
            tracker.last_updated = chrono::Utc::now();
        }
    }
}
