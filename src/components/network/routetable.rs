use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::netmux::{NftAction, NftRule, NetMux, RouteSpec};
use crate::store::{AnyResource, ResourceTracker};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{info, warn};

pub struct RouteTableResource {
    netmux: Arc<NetMux>,
}

impl RouteTableResource {
    pub fn new(netmux: Arc<NetMux>) -> Self {
        Self { netmux }
    }
}

#[async_trait]
impl Component for RouteTableResource {
    fn kind(&self) -> &'static str {
        "RouteTable"
    }
    fn category(&self) -> ResourceCategory {
        ResourceCategory::Network
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        if let AnyResource::RouteTable(rt) = &tracker.resource {
            let name = rt.metadata.name.as_deref().unwrap_or("unknown");
            self.apply_route_table(rt).await?;
        }
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::RouteTable(rt) = resource {
            self.apply_route_table(rt).await?;
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        info!("RouteTable removed: {} {}", resource.namespace(), resource.name());
        Ok(())
    }
}

impl RouteTableResource {
    async fn apply_route_table(&self, rt: &crate::types::RouteTable) -> Result<()> {
        let name = rt.metadata.name.as_deref().unwrap_or("unknown");
        let rules = &rt.spec.rules;

        if rules.is_empty() {
            info!("RouteTable '{}' has no rules", name);
            return Ok(());
        }

        // Convert RouteTable rules to nftables rules
        let mut nft_rules: Vec<NftRule> = Vec::new();

        for rule in rules {
            match rule.action.as_str() {
                "allow" | "accept" => {
                    nft_rules.push(NftRule {
                        name: rule.name.clone(),
                        chain: "forward".to_string(),
                        action: NftAction::Accept,
                        source: None,
                        dest: None,
                        protocol: rule.headers.iter()
                            .find(|h| h.name == "protocol")
                            .map(|h| h.value.clone()),
                        dport: None,
                        sport: None,
                    });
                }
                "deny" | "drop" => {
                    nft_rules.push(NftRule {
                        name: rule.name.clone(),
                        chain: "forward".to_string(),
                        action: NftAction::Drop,
                        source: None,
                        dest: None,
                        protocol: None,
                        dport: None,
                        sport: None,
                    });
                }
                _ => {
                    warn!("RouteTable '{}' rule '{}' has unknown action '{}'", name, rule.name, rule.action);
                }
            }
        }

        if !nft_rules.is_empty() {
            info!("RouteTable '{}' applying {} nftables rules", name, nft_rules.len());
            self.netmux.apply_rules(&nft_rules).await?;
        }

        Ok(())
    }
}
