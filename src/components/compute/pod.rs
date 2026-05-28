use async_trait::async_trait;
use anyhow::Result;

use crate::api::types::{ResourceState, ResourceTracker};
use crate::components::{Component, ReconcileContext};

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

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        if tracker.state == ResourceState::Pending {
            ctx.process_tracker.start_pod(&tracker.resource).await?;
        }
        Ok(())
    }
}
