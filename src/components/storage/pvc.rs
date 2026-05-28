use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::api::types::{AnyResource, ResourceStore, ResourceTracker};
use crate::components::{Component, ReconcileContext, ResourceCategory};
use k8s_openapi::api::core::v1::{ObjectReference, PersistentVolume, PersistentVolumeClaimStatus};

pub struct PvcResource {
    pub store: Arc<ResourceStore>,
}

impl PvcResource {
    pub fn new(store: Arc<ResourceStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for PvcResource {
    fn kind(&self) -> &'static str {
        "PersistentVolumeClaim"
    }

    fn category(&self) -> ResourceCategory {
        ResourceCategory::Storage
    }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        let pvc = match resource {
            AnyResource::PersistentVolumeClaim(p) => p.clone(),
            _ => return Ok(()),
        };
        if pvc.spec.as_ref().and_then(|s| s.volume_name.as_ref()).is_some() {
            return Ok(());
        }

        let has_class = pvc.spec.as_ref()
            .and_then(|s| s.storage_class_name.as_ref())
            .is_some();

        if has_class {
            ctx.vol.provision_for_pvc(&pvc).await?;
            return Ok(());
        }

        let pv_trackers = self.store.get_by_kind("PersistentVolume").await;
        let pv = find_matching_pv(&pvc, &pv_trackers);
        let Some(pv) = pv else { return Ok(()) };

        let pv_name = pv.metadata.name.as_deref().unwrap_or("").to_string();
        let pvc_namespace = pvc.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let pvc_name = pvc.metadata.name.as_deref().unwrap_or("").to_string();

        let mut updated_pv = pv;
        updated_pv.spec.as_mut().unwrap().claim_ref = Some(ObjectReference {
            kind: Some("PersistentVolumeClaim".to_string()),
            name: Some(pvc_name.clone()),
            namespace: Some(pvc_namespace.clone()),
            ..Default::default()
        });
        updated_pv.status = Some(k8s_openapi::api::core::v1::PersistentVolumeStatus {
            phase: Some("Bound".to_string()),
            ..Default::default()
        });

        let mut updated_pvc = pvc.clone();
        updated_pvc.spec.as_mut().unwrap().volume_name = Some(pv_name.clone());
        updated_pvc.status = Some(PersistentVolumeClaimStatus {
            phase: Some("Bound".to_string()),
            capacity: updated_pv.spec.as_ref().and_then(|s| s.capacity.clone()),
            access_modes: updated_pv.spec.as_ref().and_then(|s| s.access_modes.clone()),
            ..Default::default()
        });

        self.store.apply(AnyResource::PersistentVolume(updated_pv)).await?;
        self.store.apply(AnyResource::PersistentVolumeClaim(updated_pvc)).await?;
        Ok(())
    }

    async fn on_delete(&self, _ctx: &ReconcileContext, _resource: &AnyResource) -> Result<()> {
        Ok(())
    }
}

fn find_matching_pv(pvc: &k8s_openapi::api::core::v1::PersistentVolumeClaim, pvs: &[ResourceTracker]) -> Option<PersistentVolume> {
    let pvc_spec = pvc.spec.as_ref()?;
    let req_storage = pvc_spec.resources.as_ref()
        .and_then(|r| r.requests.as_ref())
        .and_then(|m| m.get("storage"))
        .map(|q| crate::api::types::parse_quantity_bytes(q))
        .unwrap_or(0);

    for t in pvs {
        let pv = match &t.resource {
            AnyResource::PersistentVolume(p) => p.clone(),
            _ => continue,
        };
        if pv.spec.as_ref().and_then(|s| s.claim_ref.as_ref()).is_some() {
            continue;
        }
        if pv.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Bound") {
            continue;
        }
        let pv_capacity = pv.spec.as_ref()
            .and_then(|s| s.capacity.as_ref())
            .and_then(|m| m.get("storage"))
            .map(|q| crate::api::types::parse_quantity_bytes(q))
            .unwrap_or(0);
        if pv_capacity < req_storage {
            continue;
        }
        let pv_modes: Vec<&str> = pv.spec.as_ref()
            .and_then(|s| s.access_modes.as_ref())
            .map(|m| m.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let pvc_modes: Vec<&str> = pvc_spec.access_modes.as_ref()
            .map(|m| m.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        if !pvc_modes.is_empty() && !pvc_modes.iter().all(|m| pv_modes.contains(m)) {
            continue;
        }
        return Some(pv);
    }
    None
}
