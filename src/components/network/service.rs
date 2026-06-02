use crate::store::AnyResource;
use crate::store::StoreBackend;
use crate::types::Service;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::netmux::NetMux;

/// Thin handle for API/reconcile context; all network IO goes through orchestrator `SyncNetwork`.
pub struct NetworkManager {
    pub store: Arc<dyn StoreBackend>,
    pub netmux: Arc<NetMux>,
}

impl NetworkManager {
    pub fn new(store: Arc<dyn StoreBackend>, netmux: Arc<NetMux>) -> Self {
        Self { store, netmux }
    }

    pub async fn remove_service(&self, ns: &str, name: &str) {
        crate::netmux::sync_network::remove_service(&self.netmux, &self.store, ns, name).await;
    }
}

#[async_trait::async_trait]
impl crate::netmux::network::NetworkEngine for NetworkManager {
    async fn sync_service(&self, _svc: &Service) -> anyhow::Result<()> {
        Ok(())
    }

    async fn remove_service(&self, ns: &str, name: &str) -> anyhow::Result<()> {
        NetworkManager::remove_service(self, ns, name).await;
        Ok(())
    }

    async fn sync_services_for_labels(
        &self,
        _ns: &str,
        _labels: &BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::store::ResourceTracker;
use anyhow::Result;
use async_trait::async_trait;

pub struct ServiceResource {
    pub store: Arc<dyn StoreBackend>,
}

impl ServiceResource {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for ServiceResource {
    fn kind(&self) -> &'static str {
        "Service"
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
        crate::netmux::sync_network::remove_service(
            &ctx.netmux,
            &ctx.store,
            resource.namespace(),
            resource.name(),
        )
        .await;
        Ok(())
    }
}
