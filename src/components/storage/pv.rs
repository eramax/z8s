use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::api::types::{ResourceStore, ResourceTracker};
use crate::components::{Component, ReconcileContext};

pub struct PvResource {
    pub store: Arc<ResourceStore>,
}

impl PvResource {
    pub fn new(store: Arc<ResourceStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for PvResource {
    fn kind(&self) -> &'static str {
        "PersistentVolume"
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }
}
