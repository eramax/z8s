use async_trait::async_trait;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::store::{AnyResource, ResourceState, ResourceTracker};
use crate::store::StoreBackend;
use crate::components::{Component, ReconcileContext, ResourceCategory};
use crate::types::{Deployment, Pod, OwnerReference};

pub fn labels_match(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    for (key, value) in selector {
        if labels.get(key) != Some(value) {
            return false;
        }
    }
    true
}

pub fn pod_owned_by_deployment(pod: &Pod, deploy_name: &str) -> bool {
    if let Some(refs) = &pod.metadata.owner_references {
        if refs.iter().any(|r| {
            r.controller == Some(true) && r.kind == "Deployment" && r.name == deploy_name
        }) {
            return true;
        }
    }
    pod.metadata
        .name
        .as_deref()
        .map_or(false, |n| n.starts_with(&format!("{deploy_name}-pod-")))
}

pub fn create_pod_from_template(deploy: &Deployment, name: &str) -> Result<Pod> {
    let spec = deploy.spec.as_ref().context("Deployment has no spec")?;
    let template = &spec.template;

    let mut pod = Pod::default();
    pod.api_version = "v1".into();
    pod.kind = "Pod".into();
    pod.metadata = template.metadata.clone().unwrap_or_default();
    pod.metadata.name = Some(name.to_string());
    let deploy_name = deploy.metadata.name.as_deref().unwrap_or("deployment");
    pod.metadata.owner_references = Some(vec![OwnerReference {
        api_version: "apps/v1".into(),
        kind: "Deployment".into(),
        name: deploy_name.to_string(),
        controller: Some(true),
        block_owner_deletion: Some(true),
        uid: deploy.metadata.uid.clone().unwrap_or_default(),
    }]);

    let mut labels = pod.metadata.labels.clone().unwrap_or_default();
    labels.extend(spec.selector.match_labels.clone().unwrap_or_default());
    pod.metadata.labels = Some(labels);

    pod.spec = template.spec.clone();
    Ok(pod)
}

pub struct DeploymentResource {
    pub store: Arc<dyn StoreBackend>,
}

impl DeploymentResource {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for DeploymentResource {
    fn kind(&self) -> &'static str {
        "Deployment"
    }

    fn category(&self) -> ResourceCategory {
        ResourceCategory::Compute
    }

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        self.reconcile_impl(ctx, tracker).await
    }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let Some(tracker) = ctx.store.get(&resource.uid()).await {
            self.reconcile_impl(ctx, &tracker).await?;
        }
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, _resource: &AnyResource) -> Result<()> {
        Ok(())
    }
}

impl DeploymentResource {
    async fn reconcile_impl(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        let AnyResource::Deployment(deploy) = &tracker.resource else {
            return Ok(());
        };

        let spec = deploy.spec.as_ref().context("Deployment has no spec")?;
        let name = deploy.metadata.name.as_deref().unwrap_or("unknown");
        let namespace = deploy.metadata.namespace.as_deref().unwrap_or("default");
        let replicas = spec.replicas.unwrap_or(1) as usize;
        let match_labels = spec.selector.match_labels.clone().unwrap_or_default();

        let all_pods = self.store.get_by_kind("Pod").await;
        let mut matching_pods: Vec<String> = all_pods
            .iter()
            .filter(|t| {
                if let AnyResource::Pod(pod) = &t.resource {
                    let pod_labels = pod.metadata.labels.clone().unwrap_or_default();
                    pod_owned_by_deployment(pod, name)
                        && labels_match(&match_labels, &pod_labels)
                        && pod.metadata.namespace.as_deref() == Some(namespace)
                } else {
                    false
                }
            })
            .map(|t| t.resource.name().to_string())
            .collect();

        let mut created_ids = Vec::new();
        while matching_pods.len() + created_ids.len() < replicas {
            let pod_name = format!(
                "{}-pod-{}",
                name,
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("x")
            );

            let mut pod = create_pod_from_template(deploy, &pod_name)?;
            pod.metadata.namespace = Some(namespace.to_string());

            let resource = AnyResource::Pod(pod);
            tracing::info!("Creating pod {} for deployment {}", pod_name, name);
            self.store.apply(resource.clone()).await?;

            if let Err(e) = ctx.process_tracker.start_pod(&resource).await {
                tracing::error!("Failed to start pod {}: {}", pod_name, e);
                self.store
                    .update_state(&resource.uid(), ResourceState::Failed(e.to_string()))
                    .await;
                break;
            }

            created_ids.push(pod_name);
        }

        for id in created_ids {
            matching_pods.push(id);
        }

        let expected_count = matching_pods.len().saturating_sub(replicas);
        for _ in 0..expected_count {
            if let Some(excess_name) = matching_pods.pop() {
                let all_pods = self.store.get_by_kind("Pod").await;
                if let Some(remove) = all_pods.iter().find(|t| t.resource.name() == excess_name) {
                    tracing::info!(
                        "Removing excess pod {} for deployment {}",
                        excess_name,
                        name
                    );
                    ctx.process_tracker.stop_pod(&remove.resource).await;
                    self.store.delete(&remove.resource).await.ok();
                }
            }
        }

        Ok(())
    }
}
