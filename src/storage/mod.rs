pub mod hostpath;
pub mod loop_prov;

use async_trait::async_trait;
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{PersistentVolume, PersistentVolumeClaim, PersistentVolumeClaimStatus};
use std::sync::Arc;
use tracing::info;

use crate::api::types::{AnyResource, ResourceStore};

#[derive(Clone, Debug)]
pub struct StorageClass {
    pub name: String,
    pub provisioner: &'static str,
}

impl StorageClass {
    pub fn builtin() -> Vec<Self> {
        vec![
            Self { name: "standard".into(), provisioner: "z8s.io/loop" },
            Self { name: "hostpath".into(), provisioner: "z8s.io/hostpath" },
        ]
    }

    pub fn by_name(name: &str) -> Option<Self> {
        Self::builtin().into_iter().find(|c| c.name == name)
    }
}

#[async_trait]
pub trait StorageProvisioner: Send + Sync {
    async fn provision_for_pvc(&self, pvc: &PersistentVolumeClaim) -> Result<()>;
    async fn deprovision_pv(&self, pv: &PersistentVolume) -> Result<()>;
}

pub struct ProvisionerDispatcher {
    store: Arc<ResourceStore>,
    loop_prov: loop_prov::LoopProvisioner,
    hostpath_prov: hostpath::HostPathProvisioner,
}

impl ProvisionerDispatcher {
    pub fn new(store: Arc<ResourceStore>) -> Self {
        Self {
            store,
            loop_prov: loop_prov::LoopProvisioner,
            hostpath_prov: hostpath::HostPathProvisioner,
        }
    }

    fn select(&self, class: &StorageClass) -> &dyn StorageProvisionerBackend {
        match class.provisioner {
            "z8s.io/loop" => &self.loop_prov,
            "z8s.io/hostpath" => &self.hostpath_prov,
            _ => &self.hostpath_prov,
        }
    }
}

#[async_trait]
impl StorageProvisioner for ProvisionerDispatcher {
    async fn provision_for_pvc(&self, pvc: &PersistentVolumeClaim) -> Result<()> {
        let spec = pvc.spec.as_ref().context("PVC has no spec")?;
        let class_name = match &spec.storage_class_name {
            Some(n) => n,
            None => return Ok(()),
        };
        if spec.volume_name.is_some() {
            return Ok(());
        }
        let class = StorageClass::by_name(class_name)
            .context(format!("unknown storage class '{}'", class_name))?;
        let pv_name = format!("pvc-{}", pvc.metadata.uid.as_deref().unwrap_or("unknown"));
        let host_path = format!("/var/lib/z8s/pv/{}", pv_name);
        let mut pv = PersistentVolume {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(pv_name.clone()),
                annotations: Some({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("z8s.io/provisioner".into(), class.provisioner.into());
                    m
                }),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PersistentVolumeSpec {
                capacity: spec.resources.as_ref()
                    .and_then(|r| r.requests.as_ref())
                    .cloned(),
                access_modes: spec.access_modes.clone(),
                claim_ref: Some(k8s_openapi::api::core::v1::ObjectReference {
                    kind: Some("PersistentVolumeClaim".into()),
                    name: pvc.metadata.name.clone(),
                    namespace: pvc.metadata.namespace.clone(),
                    ..Default::default()
                }),
                persistent_volume_reclaim_policy: Some("Delete".into()),
                host_path: Some(k8s_openapi::api::core::v1::HostPathVolumeSource {
                    path: host_path,
                    type_: None,
                }),
                ..Default::default()
            }),
            status: Some(k8s_openapi::api::core::v1::PersistentVolumeStatus {
                phase: Some("Available".into()),
                ..Default::default()
            }),
        };

        info!("Provisioning PV {} from storage class '{}'", pv_name, class_name);
        self.select(&class).do_provision(&mut pv, pvc, &class).await?;
        pv.status.as_mut().map(|s| s.phase = Some("Bound".into()));

        let pvc_name = pvc.metadata.name.clone();

        let mut updated_pvc = pvc.clone();
        updated_pvc.spec.as_mut().unwrap().volume_name = Some(pv_name);
        updated_pvc.status = Some(PersistentVolumeClaimStatus {
            phase: Some("Bound".into()),
            capacity: pv.spec.as_ref().and_then(|s| s.capacity.clone()),
            access_modes: pv.spec.as_ref().and_then(|s| s.access_modes.clone()),
            ..Default::default()
        });

        self.store.apply(AnyResource::PersistentVolume(pv)).await?;
        self.store.apply(AnyResource::PersistentVolumeClaim(updated_pvc)).await?;

        info!("Bound PVC {} to dynamically provisioned PV", pvc_name.as_deref().unwrap_or("?"));
        Ok(())
    }

    async fn deprovision_pv(&self, pv: &PersistentVolume) -> Result<()> {
        let prov = pv.metadata.annotations.as_ref()
            .and_then(|a| a.get("z8s.io/provisioner"))
            .map(|s| s.as_str())
            .unwrap_or("");
        let class = StorageClass::by_name(
            StorageClass::builtin().iter().find(|c| c.provisioner == prov).map(|c| c.name.as_str()).unwrap_or("hostpath")
        ).unwrap_or_else(|| StorageClass::builtin().into_iter().find(|c| c.name == "hostpath").unwrap());
        self.select(&class).do_deprovision(pv).await
    }
}

#[async_trait]
trait StorageProvisionerBackend: Send + Sync {
    async fn do_provision(&self, pv: &mut PersistentVolume, pvc: &PersistentVolumeClaim, class: &StorageClass) -> Result<()>;
    async fn do_deprovision(&self, pv: &PersistentVolume) -> Result<()>;
}

#[async_trait]
impl StorageProvisionerBackend for loop_prov::LoopProvisioner {
    async fn do_provision(&self, pv: &mut PersistentVolume, pvc: &PersistentVolumeClaim, class: &StorageClass) -> Result<()> {
        self.provision(pv, pvc, class).await
    }

    async fn do_deprovision(&self, pv: &PersistentVolume) -> Result<()> {
        self.deprovision(pv).await
    }
}

#[async_trait]
impl StorageProvisionerBackend for hostpath::HostPathProvisioner {
    async fn do_provision(&self, pv: &mut PersistentVolume, pvc: &PersistentVolumeClaim, class: &StorageClass) -> Result<()> {
        self.provision(pv, pvc, class).await
    }

    async fn do_deprovision(&self, pv: &PersistentVolume) -> Result<()> {
        self.deprovision(pv).await
    }
}
