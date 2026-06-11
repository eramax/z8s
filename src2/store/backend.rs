use async_trait::async_trait;

use crate::store::ops::StoreOp;
use crate::store::snapshot::StoreSnapshot;
use crate::store::{AnyResource, ResourceState, ResourceTracker};

#[async_trait]
pub trait StoreBackend: Send + Sync {
    async fn apply(&self, resource: AnyResource) -> anyhow::Result<()>;
    async fn delete(&self, resource: &AnyResource) -> anyhow::Result<()>;
    async fn get_all(&self) -> Vec<ResourceTracker>;
    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker>;
    async fn get(&self, uid: &str) -> Option<ResourceTracker>;
    async fn update_state(&self, uid: &str, state: ResourceState);

    /// Apply multiple operations; backends may override with a single transaction.
    async fn apply_batch(&self, ops: Vec<StoreOp>) -> anyhow::Result<()> {
        for op in ops {
            match op {
                StoreOp::Upsert(r) => self.apply(r).await?,
                StoreOp::UpsertWithState(r, state) => {
                    self.apply(r.clone()).await?;
                    if let Some(s) = state {
                        self.update_state(&r.uid(), s).await;
                    }
                }
                StoreOp::Delete(r) => self.delete(&r).await?,
            }
        }
        Ok(())
    }

    /// One read pass for reconcile planners.
    async fn snapshot(&self) -> StoreSnapshot {
        StoreSnapshot::from_trackers(self.get_all().await)
    }
}
