use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;
use tracing::info;
use crate::types::{AnyResource, ResourceStore, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;

pub struct IngressResource {
    pub store: Arc<ResourceStore>,
    pub netmux: Arc<NetMux>,
}

impl IngressResource {
    pub fn new(store: Arc<ResourceStore>, netmux: Arc<NetMux>) -> Self {
        Self { store, netmux }
    }
}

#[async_trait]
impl Component for IngressResource {
    fn kind(&self) -> &'static str {
        "Ingress"
    }

    fn category(&self) -> ResourceCategory {
        ResourceCategory::Network
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Ingress(ing) = resource {
            let ctrl = crate::netmux::ingress::IngressController::new(
                self.store.clone(),
                self.netmux.ingress_state.clone(),
            );
            ctrl.apply_ingress(ing)?;
            info!("Ingress '{}/{}' applied",
                ing.metadata.namespace.as_deref().unwrap_or("default"),
                ing.metadata.name.as_deref().unwrap_or("?"));
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Ingress(ing) = resource {
            let ctrl = crate::netmux::ingress::IngressController::new(
                self.store.clone(),
                self.netmux.ingress_state.clone(),
            );
            ctrl.remove_ingress(ing)?;
            info!("Ingress '{}/{}' removed",
                ing.metadata.namespace.as_deref().unwrap_or("default"),
                ing.metadata.name.as_deref().unwrap_or("?"));
        }
        Ok(())
    }
}
