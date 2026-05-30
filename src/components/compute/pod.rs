use async_trait::async_trait;
use anyhow::Result;
use std::sync::OnceLock;
use tokio::sync::Semaphore;

use crate::types::{AnyResource, ResourceState, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};

fn pod_start_semaphore() -> &'static Semaphore {
    static SEM: OnceLock<Semaphore> = OnceLock::new();
    SEM.get_or_init(|| Semaphore::new(10))
}

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
            let permit = pod_start_semaphore().acquire().await;
            let ctx = ctx.clone();
            let resource = tracker.resource.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(e) = ctx.process_tracker.start_pod(&resource).await {
                    tracing::error!("Failed to start pod {}: {}", resource.name(), e);
                }
            });
        }
        if tracker.state == ResourceState::Running && ctx.process_tracker.is_ready(tracker.resource.name()).await {
            if let AnyResource::Pod(pod) = &tracker.resource {
                let labels = pod.metadata.labels.clone().unwrap_or_default();
                let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
                if let Err(e) = ctx.net.sync_services_for_labels(ns, &labels).await {
                    tracing::warn!("sync_services_for_labels failed: {}", e);
                }
                if let Some(ip) = ctx.process_tracker.pod_ip(tracker.resource.name()).await {
                    let npc = crate::netmux::np_controller::NetworkPolicyController::new(ctx.netmux.clone());
                    if let Err(e) = npc.update_pod(ip, &labels, ns).await {
                        tracing::warn!("NetworkPolicy update_pod failed: {}", e);
                    }
                }
            }
        }
        Ok(())
    }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        let _permit = pod_start_semaphore().acquire().await;
        ctx.process_tracker.start_pod(resource).await?;
        if let AnyResource::Pod(pod) = resource {
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
            let _ = ctx.net.sync_services_for_labels(ns, &labels).await;
        }
        Ok(())
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Pod(pod) = resource {
            if let Some(ip) = ctx.process_tracker.pod_ip(pod.metadata.name.as_deref().unwrap_or("")).await {
                let npc = crate::netmux::np_controller::NetworkPolicyController::new(ctx.netmux.clone());
                if let Err(e) = npc.remove_pod(ip).await {
                    tracing::warn!("NetworkPolicy remove_pod failed: {}", e);
                }
            }
        }
        ctx.process_tracker.stop_pod(resource).await;
        Ok(())
    }
}
