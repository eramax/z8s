use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::api::types::{AnyResource, ResourceStore, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};

pub struct PvcResource {
    pub store: Arc<ResourceStore>,
}

impl PvcResource {
    pub fn new(store: Arc<ResourceStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for PvcResource {
    fn kind(&self) -> &'static str {
        "PersistentVolumeClaim"
    }

    fn category(&self) -> ResourceCategory {
        ResourceCategory::Storage
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
