use anyhow::Result;
use async_trait::async_trait;
use std::sync::OnceLock;
use tokio::sync::Semaphore;

use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::store::{AnyResource, ResourceState, ResourceTracker};

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
        let pod_name = tracker.resource.name().to_string();
        let already_running = ctx.process_tracker.is_running(&pod_name).await;

        if let AnyResource::Pod(pod) = &tracker.resource {
            let assigned = pod.assigned_node.as_deref().unwrap_or("");
            let local_node = crate::config::get().node_name.as_str();

            if assigned == local_node {
                if !already_running && tracker.state == ResourceState::Pending {
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
            }
        }

        Ok(())
    }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Pod(pod) = resource {
            let assigned = pod.assigned_node.as_deref().unwrap_or("");
            let local_node = crate::config::get().node_name.as_str();
            
            if assigned == local_node {
                let pod_name = pod.metadata.name.clone().unwrap_or_default();
                let already_running = ctx.process_tracker.is_running(&pod_name).await;
                if !already_running {
                    let ctx = ctx.clone();
                    let resource = resource.clone();
                    tokio::spawn(async move {
                        let _permit = pod_start_semaphore().acquire().await;
                        if let Err(e) = ctx.process_tracker.start_pod(&resource).await {
                            tracing::error!("Failed to start pod in on_apply: {}", e);
                        }
                    });
                }
            }
        }
        Ok(())
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Pod(pod) = resource {
            let assigned = pod.assigned_node.as_deref().unwrap_or("");
            let local_node = crate::config::get().node_name.as_str();

            if assigned == local_node {
                let ctx = ctx.clone();
                let resource = resource.clone();
                let name = pod.metadata.name.clone().unwrap_or_default();
                tokio::spawn(async move {
                    if let Some(ip) = ctx.process_tracker.pod_ip(&name).await {
                        let npc = crate::netmux::np_controller::NetworkPolicyController::new(
                            ctx.netmux.clone(),
                        );
                        if let Err(e) = npc.remove_pod(ip).await {
                            tracing::warn!("NetworkPolicy remove_pod failed: {}", e);
                        }
                    }
                    ctx.process_tracker.stop_pod(&resource).await;
                });
            }
        }
        Ok(())
    }
}
