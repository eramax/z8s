use anyhow::{Context, Result};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Container, Pod, Secret, Service};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnyResource {
    Pod(Pod),
    Deployment(Deployment),
    Service(Service),
    ConfigMap(ConfigMap),
    Secret(Secret),
}

impl AnyResource {
    pub fn kind(&self) -> &'static str {
        match self {
            AnyResource::Pod(_) => "Pod",
            AnyResource::Deployment(_) => "Deployment",
            AnyResource::Service(_) => "Service",
            AnyResource::ConfigMap(_) => "ConfigMap",
            AnyResource::Secret(_) => "Secret",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            AnyResource::Pod(p) => p.metadata.name.as_deref().unwrap_or("<unnamed>"),
            AnyResource::Deployment(d) => d.metadata.name.as_deref().unwrap_or("<unnamed>"),
            AnyResource::Service(s) => s.metadata.name.as_deref().unwrap_or("<unnamed>"),
            AnyResource::ConfigMap(c) => c.metadata.name.as_deref().unwrap_or("<unnamed>"),
            AnyResource::Secret(s) => s.metadata.name.as_deref().unwrap_or("<unnamed>"),
        }
    }

    pub fn namespace(&self) -> &str {
        match self {
            AnyResource::Pod(p) => p.metadata.namespace.as_deref().unwrap_or("default"),
            AnyResource::Deployment(d) => d.metadata.namespace.as_deref().unwrap_or("default"),
            AnyResource::Service(s) => s.metadata.namespace.as_deref().unwrap_or("default"),
            AnyResource::ConfigMap(c) => c.metadata.namespace.as_deref().unwrap_or("default"),
            AnyResource::Secret(s) => s.metadata.namespace.as_deref().unwrap_or("default"),
        }
    }

    pub fn labels(&self) -> BTreeMap<String, String> {
        match self {
            AnyResource::Pod(p) => p.metadata.labels.clone().unwrap_or_default(),
            AnyResource::Deployment(d) => d.metadata.labels.clone().unwrap_or_default(),
            AnyResource::Service(s) => s.metadata.labels.clone().unwrap_or_default(),
            AnyResource::ConfigMap(c) => c.metadata.labels.clone().unwrap_or_default(),
            AnyResource::Secret(s) => s.metadata.labels.clone().unwrap_or_default(),
        }
    }

    pub fn uid(&self) -> String {
        format!("{}/{}", self.kind(), self.name())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceState {
    Pending,
    Running,
    Failed(String),
    Terminated,
}

#[derive(Debug, Clone)]
pub struct ResourceTracker {
    pub resource: AnyResource,
    pub state: ResourceState,
    pub children: Vec<String>,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

impl ResourceTracker {
    pub fn new(resource: AnyResource) -> Self {
        Self {
            resource,
            state: ResourceState::Pending,
            children: Vec::new(),
            last_updated: chrono::Utc::now(),
        }
    }
}

pub struct ResourceStore {
    resources: Arc<RwLock<HashMap<String, ResourceTracker>>>,
}

impl ResourceStore {
    pub fn new() -> Self {
        Self {
            resources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn apply(&self, resource: AnyResource) -> Result<()> {
        let uid = resource.uid();
        let mut store = self.resources.write().await;
        let tracker = ResourceTracker::new(resource);
        store.insert(uid, tracker);
        Ok(())
    }

    pub async fn delete(&self, resource: &AnyResource) -> Result<()> {
        let uid = resource.uid();
        let mut store = self.resources.write().await;
        store.remove(&uid);
        Ok(())
    }

    pub async fn get_all(&self) -> Vec<ResourceTracker> {
        let store = self.resources.read().await;
        store.values().cloned().collect()
    }

    pub async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker> {
        let store = self.resources.read().await;
        store
            .values()
            .filter(|t| t.resource.kind() == kind)
            .cloned()
            .collect()
    }

    pub async fn update_state(&self, uid: &str, state: ResourceState) {
        let mut store = self.resources.write().await;
        if let Some(tracker) = store.get_mut(uid) {
            tracker.state = state;
            tracker.last_updated = chrono::Utc::now();
        }
    }
}

pub fn parse_manifest_yaml(yaml: &str) -> Result<Vec<AnyResource>> {
    let mut resources = Vec::new();

    for doc in serde_yaml::Deserializer::from_str(yaml) {
        let value: serde_yaml::Value =
            serde_yaml::Value::deserialize(doc).context("Failed to parse YAML document")?;

        let kind = value
            .get("kind")
            .and_then(|k| k.as_str())
            .context("Missing 'kind' field in YAML")?;

        let resource = match kind {
            "Pod" => AnyResource::Pod(
                serde_yaml::from_value(value).context("Failed to parse Pod")?,
            ),
            "Deployment" => AnyResource::Deployment(
                serde_yaml::from_value(value).context("Failed to parse Deployment")?,
            ),
            "Service" => AnyResource::Service(
                serde_yaml::from_value(value).context("Failed to parse Service")?,
            ),
            "ConfigMap" => AnyResource::ConfigMap(
                serde_yaml::from_value(value).context("Failed to parse ConfigMap")?,
            ),
            "Secret" => AnyResource::Secret(
                serde_yaml::from_value(value).context("Failed to parse Secret")?,
            ),
            _ => anyhow::bail!("Unsupported resource kind: {}", kind),
        };

        resources.push(resource);
    }

    Ok(resources)
}

pub fn extract_containers(resource: &AnyResource) -> Vec<Container> {
    match resource {
        AnyResource::Pod(pod) => {
            if let Some(spec) = &pod.spec {
                let mut containers = spec.containers.clone();
                if let Some(init) = &spec.init_containers {
                    containers.extend(init.clone());
                }
                containers
            } else {
                Vec::new()
            }
        }
        AnyResource::Deployment(deploy) => {
            if let Some(spec) = &deploy.spec {
                if let Some(template) = &spec.template.spec {
                    let mut containers = template.containers.clone();
                    if let Some(init) = &template.init_containers {
                        containers.extend(init.clone());
                    }
                    containers
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}
