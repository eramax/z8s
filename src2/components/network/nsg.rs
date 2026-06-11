use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::store::{AnyResource, ResourceTracker};
use anyhow::Result;
use async_trait::async_trait;

/// NSG reconcile runs in `netmux::sync_network`.
pub struct NsgResource;

impl NsgResource {
    pub fn new(_netmux: std::sync::Arc<crate::netmux::NetMux>) -> Self {
        Self
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

    async fn on_apply(&self, _ctx: &ReconcileContext, _resource: &AnyResource) -> Result<()> {
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, _resource: &AnyResource) -> Result<()> {
        Ok(())
    }
}
