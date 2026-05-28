use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::api::types::{ResourceStore, ResourceTracker};
use crate::components::{Component, ReconcileContext};

pub struct ConfigMapResource {
    pub store: Arc<ResourceStore>,
}

impl ConfigMapResource {
    pub fn new(store: Arc<ResourceStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for ConfigMapResource {
    fn kind(&self) -> &'static str {
        "ConfigMap"
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }
}
