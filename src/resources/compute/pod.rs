use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::api::types::{AnyResource, ResourceState, ResourceStore, ResourceTracker};
use crate::resources::{Component, ReconcileContext, ResourceCategory};
use crate::cri::runtime::ProcessSupervisor;

pub struct PodResource {
    pub supervisor: Arc<ProcessSupervisor>,
    pub store: Arc<ResourceStore>,
}

impl PodResource {
    pub fn new(supervisor: Arc<ProcessSupervisor>, store: Arc<ResourceStore>) -> Self {
        Self { supervisor, store }
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

    async fn reconcile(&self, _ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        if tracker.state == ResourceState::Pending {
            self.supervisor.start_pod(&tracker.resource).await?;
        }
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        self.supervisor.start_pod(resource).await
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        self.supervisor.stop_pod(resource).await;
        Ok(())
    }
}
