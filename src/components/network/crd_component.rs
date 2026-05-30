use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;
use tracing::info;
use crate::types::{AnyResource, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;

/// Generic watcher for a networking CRD kind.
/// Dispatches to the appropriate controller based on resource type.
pub struct CrdWatcher {
    netmux: Arc<NetMux>,
    kind_str: &'static str,
    store: Arc<crate::types::ResourceStore>,
}

impl CrdWatcher {
    pub fn new(netmux: Arc<NetMux>, store: Arc<crate::types::ResourceStore>, kind: &'static str) -> Self {
        Self { netmux, kind_str: kind, store }
    }
}

#[async_trait]
impl Component for CrdWatcher {
    fn kind(&self) -> &'static str { self.kind_str }
    fn category(&self) -> ResourceCategory { ResourceCategory::Network }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        match resource {
            AnyResource::Hub(_) => info!("Hub applied"),
            AnyResource::Spoke(_) => info!("Spoke applied"),
            _ => {}
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("Networking CRD deleted: {} {}", resource.kind(), resource.name());
        Ok(())
    }

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        self.on_apply(ctx, &tracker.resource).await
    }
}
