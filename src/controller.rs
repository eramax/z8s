use crate::api::types::{ResourceState, ResourceStore};
use crate::api::AnyResource;
use crate::supervisor::process::ProcessSupervisor;
use anyhow::{Context, Result};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::Pod;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{error, info};

pub struct DeploymentController {
    store: Arc<ResourceStore>,
    supervisor: Arc<ProcessSupervisor>,
}

impl DeploymentController {
    pub fn new(store: Arc<ResourceStore>, supervisor: Arc<ProcessSupervisor>) -> Self {
        Self { store, supervisor }
    }

    pub async fn reconcile_deployments(&self) {
        let deployments = self.store.get_by_kind("Deployment").await;

        for tracker in &deployments {
            if let AnyResource::Deployment(deploy) = &tracker.resource {
                if let Err(e) = self.reconcile_one(deploy).await {
                    error!(
                        "Failed to reconcile deployment {}/{}: {}",
                        deploy.metadata.namespace.as_deref().unwrap_or("default"),
                        deploy.metadata.name.as_deref().unwrap_or("unknown"),
                        e
                    );
                }
            }
        }
    }

    async fn reconcile_one(&self, deploy: &Deployment) -> Result<()> {
        let spec = deploy.spec.as_ref().context("Deployment has no spec")?;

        let name = deploy.metadata.name.as_deref().unwrap_or("unknown");
        let namespace = deploy.metadata.namespace.as_deref().unwrap_or("default");
        let replicas = spec.replicas.unwrap_or(1) as usize;

        let selector = &spec.selector;
        let match_labels = selector.match_labels.clone().unwrap_or_default();

        let all_pods = self.store.get_by_kind("Pod").await;
        let mut matching_pods: Vec<String> = all_pods
            .iter()
            .filter(|t| {
                if let AnyResource::Pod(pod) = &t.resource {
                    let pod_labels = pod.metadata.labels.clone().unwrap_or_default();
                    labels_match(&match_labels, &pod_labels)
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
            info!("Creating pod {} for deployment {}", pod_name, name);
            self.store.apply(resource.clone()).await?;

            if let Err(e) = self.supervisor.start_pod(&resource).await {
                error!("Failed to start pod {}: {}", pod_name, e);
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
                    info!(
                        "Removing excess pod {} for deployment {}",
                        excess_name, name
                    );
                    self.supervisor.stop_pod(&remove.resource).await;
                    self.store.delete(&remove.resource).await.ok();
                }
            }
        }

        Ok(())
    }

    pub async fn run(self: Arc<Self>) {
        let mut ticker = interval(Duration::from_secs(15));
        loop {
            ticker.tick().await;
            self.reconcile_deployments().await;
        }
    }
}

fn labels_match(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    for (key, value) in selector {
        if labels.get(key) != Some(value) {
            return false;
        }
    }
    true
}

fn create_pod_from_template(deploy: &Deployment, name: &str) -> Result<Pod> {
    let spec = deploy.spec.as_ref().context("Deployment has no spec")?;
    let template = &spec.template;

    let mut pod = Pod::default();
    pod.metadata = template.metadata.clone().unwrap_or_default();
    pod.metadata.name = Some(name.to_string());

    let mut labels = pod.metadata.labels.clone().unwrap_or_default();
    labels.extend(spec.selector.match_labels.clone().unwrap_or_default());
    pod.metadata.labels = Some(labels);

    pod.spec = template.spec.clone();
    Ok(pod)
}
