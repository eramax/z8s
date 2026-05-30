use async_trait::async_trait;
use anyhow::Result;
use tracing::info;
use crate::types::{AnyResource, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};

pub struct RouteTableResource;

impl RouteTableResource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Component for RouteTableResource {
    fn kind(&self) -> &'static str { "RouteTable" }
    fn category(&self) -> ResourceCategory { ResourceCategory::Network }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::RouteTable(rt) = resource {
            info!("RouteTable '{}' applied with {} route(s)",
                rt.metadata.name.as_deref().unwrap_or("?"),
                rt.spec.routes.len());
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("RouteTable removed: {} {}", resource.namespace(), resource.name());
        Ok(())
    }
}
