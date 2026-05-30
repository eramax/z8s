use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;
use tracing::info;
use crate::types::{AnyResource, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};

pub struct SubnetResource;

impl SubnetResource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Component for SubnetResource {
    fn kind(&self) -> &'static str { "Subnet" }
    fn category(&self) -> ResourceCategory { ResourceCategory::Network }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("Subnet applied: {} {}", resource.namespace(), resource.name());
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("Subnet removed: {} {}", resource.namespace(), resource.name());
        Ok(())
    }
}
