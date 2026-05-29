use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;
use tracing::info;
use crate::types::{AnyResource, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;

/// Generic watcher for a networking CRD kind.
/// Dispatches to the appropriate controller based on resource type.
pub struct CrdWatcher {
    netmux: Arc<NetMux>,
    kind_str: &'static str,
    store: Arc<crate::types::ResourceStore>,
}

impl CrdWatcher {
    pub fn new(netmux: Arc<NetMux>, store: Arc<crate::types::ResourceStore>, kind: &'static str) -> Self {
        Self { netmux, kind_str: kind, store }
    }
}

#[async_trait]
impl Component for CrdWatcher {
    fn kind(&self) -> &'static str { self.kind_str }
    fn category(&self) -> ResourceCategory { ResourceCategory::Network }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        match resource {
            AnyResource::VNet(vnet) => {
                let cidr = vnet.spec.cidr.as_deref().unwrap_or("10.42.0.0/20");
                let vc = crate::netmux::vnet_controller::VNetController::new(self.netmux.clone());
                vc.apply_vnet(vnet, cidr)?;
                if vnet.spec.internet_access {
                    self.netmux.add_snat(vnet.metadata.name.as_deref().unwrap_or("vnet"), cidr)?;
                }
                info!("VNet '{}' applied (CIDR {})", vnet.metadata.name.as_deref().unwrap_or("?"), cidr);
            }
            AnyResource::Nsg(nsg) => {
                let vc = crate::netmux::vnet_controller::VNetController::new(self.netmux.clone());
                vc.apply_nsg(nsg)?;
                info!("NSG '{}' applied", nsg.metadata.name.as_deref().unwrap_or("?"));
            }
            AnyResource::NetworkPolicy(np) => {
                let npc = crate::netmux::np_controller::NetworkPolicyController::new(self.netmux.clone());
                npc.apply_network_policy(np)?;
                info!("NetworkPolicy '{}/{}' applied",
                    np.metadata.namespace.as_deref().unwrap_or("default"),
                    np.metadata.name.as_deref().unwrap_or("?"));
            }
            AnyResource::Hub(_) => info!("Hub applied"),
            AnyResource::Spoke(_) => info!("Spoke applied"),
            AnyResource::Subnet(_) => info!("Subnet applied"),
            AnyResource::RouteTable(_) => info!("RouteTable applied"),
            AnyResource::Ingress(ing) => {
                let state = std::sync::Arc::new(crate::netmux::ingress::IngressState::new());
                let ctrl = crate::netmux::ingress::IngressController::new(self.store.clone(), state);
                ctrl.apply_ingress(ing)?;
                info!("Ingress '{}/{}' applied",
                    ing.metadata.namespace.as_deref().unwrap_or("default"),
                    ing.metadata.name.as_deref().unwrap_or("?"));
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("Networking CRD deleted: {} {}", resource.kind(), resource.name());
        Ok(())
    }

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        self.on_apply(ctx, &tracker.resource).await
    }
}
