use async_trait::async_trait;
use anyhow::Result;

use crate::types::{AnyResource, ResourceState, ResourceTracker};
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
        // When pod is running AND ready (has IP), sync services for DNAT
        if tracker.state == ResourceState::Running && ctx.process_tracker.is_ready(tracker.resource.name()).await {
            if let AnyResource::Pod(pod) = &tracker.resource {
                let labels = pod.metadata.labels.clone().unwrap_or_default();
                let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
                if let Err(e) = ctx.net.sync_services_for_labels(ns, &labels).await {
                    tracing::warn!("sync_services_for_labels failed: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        ctx.process_tracker.start_pod(resource).await?;
        if let AnyResource::Pod(pod) = resource {
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
            let _ = ctx.net.sync_services_for_labels(ns, &labels).await;
        }
        Ok(())
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        ctx.process_tracker.stop_pod(resource).await;
        Ok(())
    }
}
