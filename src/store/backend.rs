use async_trait::async_trait;

use crate::types::{AnyResource, ResourceState, ResourceTracker};

#[async_trait]
pub trait StoreBackend: Send + Sync {
    async fn apply(&self, resource: AnyResource) -> anyhow::Result<()>;
    async fn delete(&self, resource: &AnyResource) -> anyhow::Result<()>;
    async fn get_all(&self) -> Vec<ResourceTracker>;
    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker>;
    async fn get(&self, uid: &str) -> Option<ResourceTracker>;
    async fn update_state(&self, uid: &str, state: ResourceState);
}
