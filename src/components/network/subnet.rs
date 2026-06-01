use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;
use crate::store::{AnyResource, ResourceTracker};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

pub struct SubnetResource {
    netmux: Arc<NetMux>,
}

impl SubnetResource {
    pub fn new(netmux: Arc<NetMux>) -> Self {
        Self { netmux }
    }
}

#[async_trait]
impl Component for SubnetResource {
    fn kind(&self) -> &'static str {
        "Subnet"
    }
    fn category(&self) -> ResourceCategory {
        ResourceCategory::Network
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Subnet(subnet) = resource {
            let name = subnet.metadata.name.as_deref().unwrap_or("unknown");
            self.netmux.register_subnet_cidr(name, &subnet.spec.cidr)?;
            info!(
                "Subnet '{}' registered with CIDR {}",
                name, &subnet.spec.cidr
            );
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!(
            "Subnet removed: {} {}",
            resource.namespace(),
            resource.name()
        );
        Ok(())
    }
}
