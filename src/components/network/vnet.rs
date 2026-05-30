use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;
use tracing::info;
use crate::types::{AnyResource, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;

pub struct VNetResource {
    pub netmux: Arc<NetMux>,
}

impl VNetResource {
    pub fn new(netmux: Arc<NetMux>) -> Self {
        Self { netmux }
    }
}

#[async_trait]
impl Component for VNetResource {
    fn kind(&self) -> &'static str { "VNet" }
    fn category(&self) -> ResourceCategory { ResourceCategory::Network }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::VNet(vnet) = resource {
            let cidr = vnet.spec.cidr.as_deref().unwrap_or("10.42.0.0/20");
            let vc = crate::netmux::vnet_controller::VNetController::new(self.netmux.clone());
            vc.apply_vnet(vnet, cidr)?;
            if vnet.spec.internet_access {
                self.netmux.add_snat(vnet.metadata.name.as_deref().unwrap_or("vnet"), cidr)?;
            }
            info!("VNet '{}' applied (CIDR {})", vnet.metadata.name.as_deref().unwrap_or("?"), cidr);
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("VNet removed: {} {}", resource.namespace(), resource.name());
        Ok(())
    }
}
