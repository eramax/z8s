use async_trait::async_trait;
use anyhow::Result;

use crate::api::types::{AnyResource, ResourceState, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};

pub struct PodResource;

impl PodResource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Component for PodResource {
    fn kind(&self) -> &'static str {
        "Pod"
    }

    fn category(&self) -> ResourceCategory {
        ResourceCategory::Compute
    }

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        if tracker.state == ResourceState::Pending {
            ctx.process_tracker.start_pod(&tracker.resource).await?;
        }
        Ok(())
    }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        ctx.process_tracker.start_pod(resource).await?;
        Ok(())
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        ctx.process_tracker.stop_pod(resource).await;
        Ok(())
    }
}
