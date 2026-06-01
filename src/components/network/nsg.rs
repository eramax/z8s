use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;
use crate::store::{AnyResource, ResourceTracker};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

pub struct NsgResource {
    pub netmux: Arc<NetMux>,
}

impl NsgResource {
    pub fn new(netmux: Arc<NetMux>) -> Self {
        Self { netmux }
    }
}

#[async_trait]
impl Component for NsgResource {
    fn kind(&self) -> &'static str {
        "NSG"
    }
    fn category(&self) -> ResourceCategory {
        ResourceCategory::Network
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Nsg(nsg) = resource {
            self.netmux.apply_nsg(nsg).await?;
            info!(
                "NSG '{}' applied",
                nsg.metadata.name.as_deref().unwrap_or("?")
            );
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("NSG removed: {} {}", resource.namespace(), resource.name());
        Ok(())
    }
}
