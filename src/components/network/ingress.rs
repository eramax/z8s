use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;
use tracing::info;
use crate::types::{AnyResource, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::NetMux;

pub struct IngressResource {
    pub store: Arc<dyn crate::store::StoreBackend>,
    pub netmux: Arc<NetMux>,
}

impl IngressResource {
    pub fn new(store: Arc<dyn crate::store::StoreBackend>, netmux: Arc<NetMux>) -> Self {
        Self { store, netmux }
    }
}

#[async_trait]
impl Component for IngressResource {
    fn kind(&self) -> &'static str { "Ingress" }
    fn category(&self) -> ResourceCategory { ResourceCategory::Network }
    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> { Ok(()) }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Ingress(ing) = resource {
            crate::netmux::ingress::apply_ingress(&self.netmux.ingress_state, ing)?;
            // Register DNS records for ingress hosts → gateway IP
            let gw = self.netmux.gateway;
            if let Some(spec) = &ing.spec {
                if let Some(rules) = &spec.rules {
                    let mut records = self.netmux.dns_records.write().unwrap_or_else(|e| { tracing::warn!("dns_records lock poisoned"); e.into_inner() });
                    for rule in rules {
                        if let Some(host) = &rule.host {
                            if !host.is_empty() {
                                records.insert(host.clone(), gw);
                                info!("DNS: {} → {} (ingress)", host, gw);
                            }
                        }
                    }
                }
            }
            info!("Ingress '{}/{}' applied", ing.metadata.namespace.as_deref().unwrap_or("default"), ing.metadata.name.as_deref().unwrap_or("?"));
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::Ingress(ing) = resource {
            crate::netmux::ingress::remove_ingress(&self.netmux.ingress_state, ing)?;
            // Remove DNS records for ingress hosts
            if let Some(spec) = &ing.spec {
                if let Some(rules) = &spec.rules {
                    let mut records = self.netmux.dns_records.write().unwrap_or_else(|e| { tracing::warn!("dns_records lock poisoned"); e.into_inner() });
                    for rule in rules {
                        if let Some(host) = &rule.host {
                            records.remove(host);
                            info!("DNS: {} removed (ingress deleted)", host);
                        }
                    }
                }
            }
            info!("Ingress '{}/{}' removed", ing.metadata.namespace.as_deref().unwrap_or("default"), ing.metadata.name.as_deref().unwrap_or("?"));
        }
        Ok(())
    }
}
