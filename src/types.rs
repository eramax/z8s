use anyhow::{Context, Result};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{
    ConfigMap, Container, PersistentVolume, PersistentVolumeClaim, Pod, Secret, Service,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::{Deserialize, Serialize};

// ── ResourceTracker ──────────────────────────────────────────────────────────

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

// ── AnyResource ──────────────────────────────────────────────────────────────

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
    VNet(crate::netmux::crds::VNet),
    Subnet(crate::netmux::crds::Subnet),
    Nsg(crate::netmux::crds::Nsg),
    RouteTable(crate::netmux::crds::RouteTable),
    Ingress(k8s_openapi::api::networking::v1::Ingress),
    NetworkPolicy(k8s_openapi::api::networking::v1::NetworkPolicy),
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
            AnyResource::VNet(r) => &r.metadata,
            AnyResource::Subnet(r) => &r.metadata,
            AnyResource::Nsg(r) => &r.metadata,
            AnyResource::RouteTable(r) => &r.metadata,
            AnyResource::Ingress(r) => &r.metadata,
            AnyResource::NetworkPolicy(r) => &r.metadata,
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
            AnyResource::VNet(r) => &mut r.metadata,
            AnyResource::Subnet(r) => &mut r.metadata,
            AnyResource::Nsg(r) => &mut r.metadata,
            AnyResource::RouteTable(r) => &mut r.metadata,
            AnyResource::Ingress(r) => &mut r.metadata,
            AnyResource::NetworkPolicy(r) => &mut r.metadata,
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
            AnyResource::VNet(_) => "VNet",
            AnyResource::Subnet(_) => "Subnet",
            AnyResource::Nsg(_) => "NSG",
            AnyResource::RouteTable(_) => "RouteTable",
            AnyResource::Ingress(_) => "Ingress",
            AnyResource::NetworkPolicy(_) => "NetworkPolicy",
        }
    }

    pub fn name(&self) -> &str {
        self.metadata().name.as_deref().unwrap_or("<unnamed>")
    }

    pub fn namespace(&self) -> &str {
        match self {
            AnyResource::PersistentVolume(_) => "",
            _ => self.metadata().namespace.as_deref().unwrap_or("default"),
        }
    }

    pub fn uid(&self) -> String {
        format!("{}/{}/{}", self.kind(), self.namespace(), self.name())
    }
}

// ── YAML parsing ────────────────────────────────────────────────────────────

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
            "VNet" => AnyResource::VNet(
                serde_yaml::from_value(value).context("Failed to parse VNet")?,
            ),
            "Subnet" => AnyResource::Subnet(
                serde_yaml::from_value(value).context("Failed to parse Subnet")?,
            ),
            "NSG" => AnyResource::Nsg(
                serde_yaml::from_value(value).context("Failed to parse NSG")?,
            ),
            "RouteTable" => AnyResource::RouteTable(
                serde_yaml::from_value(value).context("Failed to parse RouteTable")?,
            ),
            "Ingress" => AnyResource::Ingress(
                serde_yaml::from_value(value).context("Failed to parse Ingress")?,
            ),
            "NetworkPolicy" => AnyResource::NetworkPolicy(
                serde_yaml::from_value(value).context("Failed to parse NetworkPolicy")?,
            ),
            _ => anyhow::bail!("Unsupported resource kind: {}", kind),
        };

        resources.push(resource);
    }

    Ok(resources)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

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

pub fn parse_quantity_bytes(q: &Quantity) -> u64 {
    let s = q.0.trim();
    if let Some(rest) = s.strip_suffix("Ti") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(4)
    } else if let Some(rest) = s.strip_suffix("Gi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(3)
    } else if let Some(rest) = s.strip_suffix("Mi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(2)
    } else if let Some(rest) = s.strip_suffix("Ki") {
        rest.parse::<u64>().unwrap_or(0) * 1024
    } else if let Some(rest) = s.strip_suffix("Pi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(5)
    } else if let Some(rest) = s.strip_suffix("Ei") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(6)
    } else if let Some(rest) = s.strip_suffix('T') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000_000_000
    } else if let Some(rest) = s.strip_suffix('G') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000_000
    } else if let Some(rest) = s.strip_suffix('M') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000
    } else if let Some(rest) = s.strip_suffix('k') {
        rest.parse::<u64>().unwrap_or(0) * 1000
    } else {
        s.parse::<u64>().unwrap_or(0)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_includes_namespace() {
        let yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: my-pod\n  namespace: production\nspec:\n  containers:\n  - name: c\n    image: alpine\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        let r = &resources[0];
        assert_eq!(r.uid(), "Pod/production/my-pod");
    }

    #[test]
    fn uid_defaults_namespace_to_default() {
        let yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: my-pod\nspec:\n  containers:\n  - name: c\n    image: alpine\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources[0].uid(), "Pod/default/my-pod");
    }

    #[test]
    fn same_name_different_namespaces_get_different_uids() {
        let pod_a = parse_manifest_yaml(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: app\n  namespace: ns-a\nspec:\n  containers:\n  - name: c\n    image: alpine\n",
        ).unwrap().remove(0);
        let pod_b = parse_manifest_yaml(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: app\n  namespace: ns-b\nspec:\n  containers:\n  - name: c\n    image: alpine\n",
        ).unwrap().remove(0);
        assert_ne!(pod_a.uid(), pod_b.uid());
    }

    #[test]
    fn pv_namespace_is_empty() {
        let yaml = "apiVersion: v1\nkind: PersistentVolume\nmetadata:\n  name: my-pv\nspec:\n  capacity:\n    storage: 1Gi\n  accessModes:\n  - ReadWriteOnce\n  hostPath:\n    path: /data\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources[0].namespace(), "");
        assert_eq!(resources[0].uid(), "PersistentVolume//my-pv");
    }

    #[tokio::test]
    async fn store_namespaced_resources_do_not_collide() {
        use crate::store::{MemoryBackend, StoreBackend};
        let store = MemoryBackend::new();

        let cm_a_yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: config\n  namespace: ns-a\ndata:\n  key: value-a\n";
        let cm_b_yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: config\n  namespace: ns-b\ndata:\n  key: value-b\n";

        let res_a = parse_manifest_yaml(cm_a_yaml).unwrap().remove(0);
        let res_b = parse_manifest_yaml(cm_b_yaml).unwrap().remove(0);

        store.apply(res_a).await.unwrap();
        store.apply(res_b).await.unwrap();

        let all = store.get_by_kind("ConfigMap").await;
        assert_eq!(all.len(), 2);

        let a = store.get("ConfigMap/ns-a/config").await;
        let b = store.get("ConfigMap/ns-b/config").await;
        assert!(a.is_some());
        assert!(b.is_some());

        if let Some(AnyResource::ConfigMap(cm)) = a.map(|t| t.resource) {
            assert_eq!(cm.data.unwrap().get("key").unwrap(), "value-a");
        }
        if let Some(AnyResource::ConfigMap(cm)) = b.map(|t| t.resource) {
            assert_eq!(cm.data.unwrap().get("key").unwrap(), "value-b");
        }
    }

    #[test]
    fn parse_multi_document_yaml() {
        let yaml = "\
apiVersion: v1
kind: ConfigMap
metadata:
  name: cm1
  namespace: default
---
apiVersion: v1
kind: Secret
metadata:
  name: sec1
  namespace: default
";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].kind(), "ConfigMap");
        assert_eq!(resources[1].kind(), "Secret");
    }

    #[test]
    fn parse_pv_and_pvc() {
        let yaml = "\
apiVersion: v1
kind: PersistentVolume
metadata:
  name: pv1
spec:
  capacity:
    storage: 5Gi
  accessModes:
  - ReadWriteOnce
  hostPath:
    path: /data/pv1
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: pvc1
  namespace: default
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources.len(), 2);
        assert!(matches!(&resources[0], AnyResource::PersistentVolume(_)));
        assert!(matches!(&resources[1], AnyResource::PersistentVolumeClaim(_)));
    }

    #[test]
    fn parse_unsupported_kind_returns_error() {
        let yaml = "apiVersion: v1\nkind: UnknownThing\nmetadata:\n  name: x\n";
        assert!(parse_manifest_yaml(yaml).is_err());
    }

    #[test]
    fn extract_containers_from_pod_includes_init() {
        let yaml = "\
apiVersion: v1
kind: Pod
metadata:
  name: p
spec:
  initContainers:
  - name: init
    image: busybox
  containers:
  - name: app
    image: alpine
";
        let resource = parse_manifest_yaml(yaml).unwrap().remove(0);
        let containers = extract_containers(&resource);
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].name, "app");
        assert_eq!(containers[1].name, "init");
    }

    #[test]
    fn extract_containers_from_non_pod_is_empty() {
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cm\n";
        let resource = parse_manifest_yaml(yaml).unwrap().remove(0);
        assert!(extract_containers(&resource).is_empty());
    }

    #[test]
    fn parse_quantity_ki() {
        assert_eq!(parse_quantity_bytes(&Quantity("128Ki".into())), 128 * 1024);
    }

    #[test]
    fn parse_quantity_mi() {
        assert_eq!(parse_quantity_bytes(&Quantity("256Mi".into())), 256 * 1024 * 1024);
    }

    #[test]
    fn parse_quantity_gi() {
        assert_eq!(parse_quantity_bytes(&Quantity("1Gi".into())), 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_quantity_plain_bytes() {
        assert_eq!(parse_quantity_bytes(&Quantity("4096".into())), 4096);
    }

    #[test]
    fn parse_cpu_millicores() {
        let (quota, period) = parse_quantity_cpu(&Quantity("500m".into()));
        assert_eq!(quota, 50_000);
        assert_eq!(period, 100_000);
    }

    #[test]
    fn parse_cpu_cores() {
        let (quota, period) = parse_quantity_cpu(&Quantity("2".into()));
        assert_eq!(quota, 200_000);
        assert_eq!(period, 100_000);
    }
}
