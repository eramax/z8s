use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::store::StoreBackend;
use crate::store::{AnyResource, ResourceTracker};

pub struct ConfigMapResource {
    pub store: Arc<dyn StoreBackend>,
}

impl ConfigMapResource {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for ConfigMapResource {
    fn kind(&self) -> &'static str {
        "ConfigMap"
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
