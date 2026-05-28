use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::api::types::{ResourceStore, ResourceTracker};
use crate::components::{Component, ReconcileContext};

pub struct SecretResource {
    pub store: Arc<ResourceStore>,
}

impl SecretResource {
    pub fn new(store: Arc<ResourceStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for SecretResource {
    fn kind(&self) -> &'static str {
        "Secret"
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }
}
