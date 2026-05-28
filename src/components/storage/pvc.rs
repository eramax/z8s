use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::api::types::{ResourceStore, ResourceTracker};
use crate::components::{Component, ReconcileContext};

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

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }
}
