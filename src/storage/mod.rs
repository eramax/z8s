pub mod class;
pub mod hostpath;
pub mod loop_prov;

use crate::types::{
    HostPathVolumeSource, ObjectMeta, ObjectReference, PersistentVolume, PersistentVolumeClaim,
    PersistentVolumeClaimStatus, PersistentVolumeSpec, PersistentVolumeStatus,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

use crate::store::AnyResource;
use crate::store::StoreBackend;

pub use class::{
    default_storage_class_name, resolve_storage_class, seed_default_storage_classes,
    volume_binding_immediate, volume_binding_wait_for_consumer,
};

#[async_trait]
pub trait StorageProvisioner: Send + Sync {
    async fn provision_for_pvc(&self, pvc: &PersistentVolumeClaim) -> Result<()>;
    async fn provision_for_pvc_on_node(
        &self,
        pvc: &PersistentVolumeClaim,
        class: &crate::types::StorageClass,
        node: &str,
    ) -> Result<()>;
    async fn deprovision_pv(&self, pv: &PersistentVolume) -> Result<()>;
}

pub struct ProvisionerDispatcher {
    store: Arc<dyn StoreBackend>,
    loop_prov: loop_prov::LoopProvisioner,
    hostpath_prov: hostpath::HostPathProvisioner,
    inflight: Mutex<HashMap<String, ()>>,
}

impl ProvisionerDispatcher {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self {
            store,
            loop_prov: loop_prov::LoopProvisioner,
            hostpath_prov: hostpath::HostPathProvisioner,
            inflight: Mutex::new(HashMap::new()),
        }
    }

    fn select_provisioner(&self, provisioner: &str) -> Result<&dyn StorageProvisionerBackend> {
        match provisioner {
            "z8s.io/loop" => Ok(&self.loop_prov),
            "z8s.io/hostpath" => Ok(&self.hostpath_prov),
            p => anyhow::bail!("unknown provisioner '{}'", p),
        }
    }

    async fn do_provision(
        &self,
        pvc: &PersistentVolumeClaim,
        class: &crate::types::StorageClass,
        bind_node: Option<&str>,
    ) -> Result<()> {
        let spec = pvc.spec.as_ref().context("PVC has no spec")?;
        let class_name = class.metadata.name.as_deref().unwrap_or("unknown");
        let pvc_ns = pvc.metadata.namespace.as_deref().unwrap_or("default");
        let pvc_name = pvc.metadata.name.as_deref().unwrap_or("unknown");
        let pv_name = format!("pvc-{}--{}", pvc_ns, pvc_name);
        let host_path = format!("/var/lib/z8s/pv/{}", pv_name);
        let mut pv = PersistentVolume {
            api_version: "v1".into(),
            kind: "PersistentVolume".into(),
            metadata: ObjectMeta {
                name: Some(pv_name.clone()),
                uid: Some(crate::config::random_id()),
                annotations: Some({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("z8s.io/provisioner".into(), class.provisioner.clone());
                    if let Some(node) = bind_node {
                        m.insert("z8s.io/bind-node".into(), node.into());
                    }
                    m
                }),
                ..Default::default()
            },
            spec: Some({
                let mut pv_spec = PersistentVolumeSpec {
                    capacity: spec
                        .resources
                        .as_ref()
                        .and_then(|r| r.requests.as_ref())
                        .cloned(),
                    access_modes: spec.access_modes.clone(),
                    claim_ref: Some(ObjectReference {
                        kind: Some("PersistentVolumeClaim".into()),
                        name: pvc.metadata.name.clone(),
                        namespace: pvc.metadata.namespace.clone(),
                        ..Default::default()
                    }),
                    persistent_volume_reclaim_policy: class
                        .reclaim_policy
                        .clone()
                        .or(Some("Delete".into())),
                    host_path: Some(HostPathVolumeSource {
                        path: host_path,
                        type_: None,
                    }),
                    ..Default::default()
                };
                pv_spec
            }),
            status: Some(PersistentVolumeStatus {
                phase: Some("Available".into()),
                ..Default::default()
            }),
        };

        info!(
            "Provisioning PV {} from storage class '{}'",
            pv_name, class_name
        );
        self.select_provisioner(&class.provisioner)?
            .do_provision(&mut pv, pvc, class)
            .await?;
        if let Some(s) = pv.status.as_mut() {
            s.phase = Some("Bound".into());
        }

        let mut updated_pvc = pvc.clone();
        let Some(pvc_spec) = updated_pvc.spec.as_mut() else {
            anyhow::bail!("PVC spec missing")
        };
        pvc_spec.volume_name = Some(pv_name);
        updated_pvc.status = Some(PersistentVolumeClaimStatus {
            phase: Some("Bound".into()),
            capacity: pv.spec.as_ref().and_then(|s| s.capacity.clone()),
            access_modes: pv.spec.as_ref().and_then(|s| s.access_modes.clone()),
            ..Default::default()
        });

        self.store.apply(AnyResource::PersistentVolume(pv)).await?;
        self.store
            .apply(AnyResource::PersistentVolumeClaim(updated_pvc))
            .await?;

        info!("Bound PVC {} to dynamically provisioned PV", pvc_name);
        Ok(())
    }
}

#[async_trait]
impl StorageProvisioner for ProvisionerDispatcher {
    async fn provision_for_pvc(&self, pvc: &PersistentVolumeClaim) -> Result<()> {
        let spec = pvc.spec.as_ref().context("PVC has no spec")?;
        if spec.volume_name.is_some() {
            return Ok(());
        }
        let class_name = match &spec.storage_class_name {
            Some(n) => n.clone(),
            None => match default_storage_class_name(self.store.as_ref()).await {
                Some(n) => n,
                None => return Ok(()),
            },
        };
        let Some(class) = resolve_storage_class(self.store.as_ref(), &class_name).await? else {
            anyhow::bail!("StorageClass '{}' not found", class_name);
        };
        if !volume_binding_immediate(&class) {
            return Ok(());
        }
        self.provision_with_class(pvc, &class, None).await
    }

    async fn provision_for_pvc_on_node(
        &self,
        pvc: &PersistentVolumeClaim,
        class: &crate::types::StorageClass,
        node: &str,
    ) -> Result<()> {
        self.provision_with_class(pvc, class, Some(node)).await
    }

    async fn deprovision_pv(&self, pv: &PersistentVolume) -> Result<()> {
        let prov: &'static str = match pv
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get("z8s.io/provisioner"))
            .map(|s| s.as_str())
            .unwrap_or("")
        {
            "z8s.io/loop" => "z8s.io/loop",
            "z8s.io/hostpath" => "z8s.io/hostpath",
            _ => return Ok(()),
        };
        let backend: &dyn StorageProvisionerBackend = match prov {
            "z8s.io/loop" => &self.loop_prov,
            "z8s.io/hostpath" => &self.hostpath_prov,
            _ => return Ok(()),
        };
        backend.do_deprovision(pv).await
    }
}

impl ProvisionerDispatcher {
    async fn provision_with_class(
        &self,
        pvc: &PersistentVolumeClaim,
        class: &crate::types::StorageClass,
        bind_node: Option<&str>,
    ) -> Result<()> {
        let spec = pvc.spec.as_ref().context("PVC has no spec")?;
        if spec.volume_name.is_some() {
            return Ok(());
        }

        let uid = match &pvc.metadata.uid {
            Some(u) => u.clone(),
            None => anyhow::bail!("PVC has no uid"),
        };

        {
            let mut inflight = self.inflight.lock().await;
            if inflight.contains_key(&uid) {
                return Ok(());
            }
            inflight.insert(uid.clone(), ());
        }

        let pvc_uid = format!(
            "PersistentVolumeClaim/{}/{}",
            pvc.metadata.namespace.as_deref().unwrap_or("default"),
            pvc.metadata.name.as_deref().unwrap_or("unknown")
        );
        if let Some(tracker) = self.store.get(&pvc_uid).await {
            if let AnyResource::PersistentVolumeClaim(ref p) = tracker.resource {
                if p.spec
                    .as_ref()
                    .and_then(|s| s.volume_name.as_ref())
                    .is_some()
                {
                    self.inflight.lock().await.remove(&uid);
                    return Ok(());
                }
            }
        }

        let result = self.do_provision(pvc, class, bind_node).await;

        self.inflight.lock().await.remove(&uid);
        result
    }
}

#[async_trait]
trait StorageProvisionerBackend: Send + Sync {
    async fn do_provision(
        &self,
        pv: &mut PersistentVolume,
        pvc: &PersistentVolumeClaim,
        class: &crate::types::StorageClass,
    ) -> Result<()>;
    async fn do_deprovision(&self, pv: &PersistentVolume) -> Result<()>;
}

#[async_trait]
impl StorageProvisionerBackend for loop_prov::LoopProvisioner {
    async fn do_provision(
        &self,
        pv: &mut PersistentVolume,
        pvc: &PersistentVolumeClaim,
        class: &crate::types::StorageClass,
    ) -> Result<()> {
        self.provision(pv, pvc, class).await
    }

    async fn do_deprovision(&self, pv: &PersistentVolume) -> Result<()> {
        self.deprovision(pv).await
    }
}

#[async_trait]
impl StorageProvisionerBackend for hostpath::HostPathProvisioner {
    async fn do_provision(
        &self,
        pv: &mut PersistentVolume,
        pvc: &PersistentVolumeClaim,
        class: &crate::types::StorageClass,
    ) -> Result<()> {
        self.provision(pv, pvc, class).await
    }

    async fn do_deprovision(&self, pv: &PersistentVolume) -> Result<()> {
        self.deprovision(pv).await
    }
}
