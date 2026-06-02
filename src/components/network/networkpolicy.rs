use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;
use crate::store::{AnyResource, ResourceTracker};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

pub struct NetworkPolicyResource {
    pub netmux: Arc<NetMux>,
}

impl NetworkPolicyResource {
    pub fn new(netmux: Arc<NetMux>) -> Self {
        Self { netmux }
    }
}

#[async_trait]
impl Component for NetworkPolicyResource {
    fn kind(&self) -> &'static str {
        "NetworkPolicy"
    }

    fn category(&self) -> ResourceCategory {
        ResourceCategory::Network
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        if let AnyResource::NetworkPolicy(np) = &tracker.resource {
            let npc =
                crate::netmux::np_controller::NetworkPolicyController::new(self.netmux.clone());
            npc.apply_network_policy(np).await?;
            info!(
                "NetworkPolicy '{}/{}' applied",
                np.metadata.namespace.as_deref().unwrap_or("default"),
                np.metadata.name.as_deref().unwrap_or("?")
            );
        }
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, _resource: &AnyResource) -> Result<()> {
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!(
            "NetworkPolicy removed: {} {}",
            resource.namespace(),
            resource.name()
        );
        Ok(())
    }
}
