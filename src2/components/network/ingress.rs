use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::store::{AnyResource, ResourceTracker};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Ingress reconcile runs in `netmux::sync_network`.
pub struct IngressResource {
    pub store: Arc<dyn crate::store::StoreBackend>,
}

impl IngressResource {
    pub fn new(store: Arc<dyn crate::store::StoreBackend>, _netmux: Arc<crate::netmux::NetMux>) -> Self {
        Self { store }
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

    async fn on_apply(&self, _ctx: &ReconcileContext, _resource: &AnyResource) -> Result<()> {
        Ok(())
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Ingress(ing) = resource {
            crate::netmux::ingress::remove_ingress(&ctx.netmux.ingress_state, ing)?;
            if let Some(spec) = &ing.spec {
                if let Some(rules) = &spec.rules {
                    let mut records = ctx.netmux.dns_records.write().unwrap_or_else(|e| e.into_inner());
                    for rule in rules {
                        if let Some(host) = &rule.host {
                            records.remove(host);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
