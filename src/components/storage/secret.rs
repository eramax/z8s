use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::store::{AnyResource, ResourceTracker};
use crate::store::StoreBackend;
use crate::components::{Component, ReconcileContext, ResourceCategory};

pub struct SecretResource {
    pub store: Arc<dyn StoreBackend>,
}

impl SecretResource {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for SecretResource {
    fn kind(&self) -> &'static str {
        "Secret"
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
