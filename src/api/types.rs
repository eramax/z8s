use anyhow::{Context, Result};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{
    ConfigMap, Container, PersistentVolume, PersistentVolumeClaim, Pod, Secret, Service,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnyResource {
    Pod(Pod),
    Deployment(Deployment),
    Service(Service),
    ConfigMap(ConfigMap),
    Secret(Secret),
    PersistentVolume(PersistentVolume),
    PersistentVolumeClaim(PersistentVolumeClaim),
}

impl AnyResource {
    pub fn metadata(&self) -> &ObjectMeta {
        match self {
            AnyResource::Pod(r) => &r.metadata,
            AnyResource::Deployment(r) => &r.metadata,
            AnyResource::Service(r) => &r.metadata,
            AnyResource::ConfigMap(r) => &r.metadata,
            AnyResource::Secret(r) => &r.metadata,
            AnyResource::PersistentVolume(r) => &r.metadata,
            AnyResource::PersistentVolumeClaim(r) => &r.metadata,
        }
    }

    pub fn metadata_mut(&mut self) -> &mut ObjectMeta {
        match self {
            AnyResource::Pod(r) => &mut r.metadata,
            AnyResource::Deployment(r) => &mut r.metadata,
            AnyResource::Service(r) => &mut r.metadata,
            AnyResource::ConfigMap(r) => &mut r.metadata,
            AnyResource::Secret(r) => &mut r.metadata,
            AnyResource::PersistentVolume(r) => &mut r.metadata,
            AnyResource::PersistentVolumeClaim(r) => &mut r.metadata,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            AnyResource::Pod(_) => "Pod",
            AnyResource::Deployment(_) => "Deployment",
            AnyResource::Service(_) => "Service",
            AnyResource::ConfigMap(_) => "ConfigMap",
            AnyResource::Secret(_) => "Secret",
            AnyResource::PersistentVolume(_) => "PersistentVolume",
            AnyResource::PersistentVolumeClaim(_) => "PersistentVolumeClaim",
        }
    }

    pub fn name(&self) -> &str {
        self.metadata().name.as_deref().unwrap_or("<unnamed>")
    }

    pub fn namespace(&self) -> &str {
        match self {
            // PersistentVolumes are cluster-scoped (no namespace)
            AnyResource::PersistentVolume(_) => "",
            _ => self.metadata().namespace.as_deref().unwrap_or("default"),
        }
    }

    pub fn uid(&self) -> String {
        format!("{}/{}/{}", self.kind(), self.namespace(), self.name())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceState {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Terminated,
}

#[derive(Debug, Clone)]
pub struct ResourceTracker {
    pub resource: AnyResource,
    pub state: ResourceState,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

impl ResourceTracker {
    pub fn new(resource: AnyResource) -> Self {
        Self {
            resource,
            state: ResourceState::Pending,
            last_updated: chrono::Utc::now(),
        }
    }
}

pub struct ResourceStore {
    resources: RwLock<HashMap<String, ResourceTracker>>,
}

impl ResourceStore {
    pub fn new() -> Self {
        Self {
            resources: RwLock::new(HashMap::new()),
        }
    }

    pub async fn apply(&self, resource: AnyResource) -> Result<()> {
        let uid = resource.uid();
        let mut store = self.resources.write().await;
        store.insert(uid, ResourceTracker::new(resource));
        Ok(())
    }

    pub async fn delete(&self, resource: &AnyResource) -> Result<()> {
        let uid = resource.uid();
        self.resources.write().await.remove(&uid);
        Ok(())
    }

    pub async fn get_all(&self) -> Vec<ResourceTracker> {
        self.resources.read().await.values().cloned().collect()
    }

    pub async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker> {
        self.resources
            .read()
            .await
            .values()
            .filter(|t| t.resource.kind() == kind)
            .cloned()
            .collect()
    }

    pub async fn get(&self, uid: &str) -> Option<ResourceTracker> {
        self.resources.read().await.get(uid).cloned()
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
            "PersistentVolume" => AnyResource::PersistentVolume(
                serde_yaml::from_value(value).context("Failed to parse PersistentVolume")?,
            ),
            "PersistentVolumeClaim" => AnyResource::PersistentVolumeClaim(
                serde_yaml::from_value(value).context("Failed to parse PersistentVolumeClaim")?,
            ),
            _ => anyhow::bail!("Unsupported resource kind: {}", kind),
        };

        resources.push(resource);
    }

    Ok(resources)
}

pub fn extract_containers(resource: &AnyResource) -> Vec<Container> {
    match resource {
        AnyResource::Pod(pod) => pod.spec.as_ref().map_or_else(Vec::new, |s| {
            let mut c = s.containers.clone();
            if let Some(init) = &s.init_containers {
                c.extend(init.clone());
            }
            c
        }),
        AnyResource::Deployment(deploy) => deploy
            .spec
            .as_ref()
            .and_then(|s| s.template.spec.as_ref())
            .map_or_else(Vec::new, |s| {
                let mut c = s.containers.clone();
                if let Some(init) = &s.init_containers {
                    c.extend(init.clone());
                }
                c
            }),
        _ => Vec::new(),
    }
}

/// Parse a k8s resource Quantity string into bytes.
pub fn parse_quantity_bytes(q: &Quantity) -> u64 {
    let s = q.0.trim();
    if let Some(rest) = s.strip_suffix("Ki") {
        rest.parse::<u64>().unwrap_or(0) * 1024
    } else if let Some(rest) = s.strip_suffix("Mi") {
        rest.parse::<u64>().unwrap_or(0) * 1024 * 1024
    } else if let Some(rest) = s.strip_suffix("Gi") {
        rest.parse::<u64>().unwrap_or(0) * 1024 * 1024 * 1024
    } else if let Some(rest) = s.strip_suffix('k') {
        rest.parse::<u64>().unwrap_or(0) * 1000
    } else if let Some(rest) = s.strip_suffix('M') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000
    } else if let Some(rest) = s.strip_suffix('G') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000_000
    } else {
        s.parse::<u64>().unwrap_or(0)
    }
}

/// Parse a k8s CPU Quantity into (quota_usec, period_usec) for cgroups cpu.max.
pub fn parse_quantity_cpu(q: &Quantity) -> (i64, i64) {
    let s = q.0.trim();
    if let Some(rest) = s.strip_suffix('m') {
        let millicores = rest.parse::<i64>().unwrap_or(0);
        (millicores * 100, 100_000)
    } else {
        let cores = s.parse::<f64>().unwrap_or(0.0);
        ((cores * 100_000.0) as i64, 100_000)
    }
}
