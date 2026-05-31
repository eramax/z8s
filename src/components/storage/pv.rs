use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::types::{AnyResource, ResourceTracker};
use crate::store::StoreBackend;
use crate::components::{Component, ReconcileContext, ResourceCategory};
use k8s_openapi::api::core::v1::{ObjectReference, PersistentVolumeClaimStatus};

pub struct PvResource {
    pub store: Arc<dyn StoreBackend>,
}

impl PvResource {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Component for PvResource {
    fn kind(&self) -> &'static str { "PersistentVolume" }

    fn category(&self) -> ResourceCategory { ResourceCategory::Storage }

    async fn reconcile(&self, _ctx: &ReconcileContext, _tracker: &ResourceTracker) -> Result<()> {
        Ok(())
    }

    async fn on_apply(&self, _ctx: &ReconcileContext, _resource: &AnyResource) -> Result<()> {
        let pv = match _resource {
            AnyResource::PersistentVolume(p) => p.clone(),
            _ => return Ok(()),
        };
        if pv.spec.as_ref().and_then(|s| s.claim_ref.as_ref()).is_some() {
            return Ok(());
        }
        let pv_name = match pv.metadata.name.as_deref() {
            Some(n) => n.to_string(),
            None => return Ok(()),
        };
        let pvc_trackers = self.store.get_by_kind("PersistentVolumeClaim").await;
        for t in &pvc_trackers {
            let pvc = match &t.resource {
                AnyResource::PersistentVolumeClaim(p) => p.clone(),
                _ => continue,
            };
            let pvc_spec = match pvc.spec.as_ref() {
                Some(s) => s,
                None => continue,
            };
            if pvc_spec.volume_name.is_some() {
                continue;
            }
            let Some(mut updated_pvc) = try_bind_pvc(&pv, &pvc) else { continue };

            let mut updated_pv = pv.clone();
            if let Some(s) = updated_pv.spec.as_mut() {
                s.claim_ref = Some(ObjectReference {
                    kind: Some("PersistentVolumeClaim".into()),
                    name: pvc.metadata.name.clone(),
                    namespace: pvc.metadata.namespace.clone(),
                    ..Default::default()
                });
            }
            updated_pv.status = Some(k8s_openapi::api::core::v1::PersistentVolumeStatus {
                phase: Some("Bound".into()),
                ..Default::default()
            });

            self.store.apply(AnyResource::PersistentVolume(updated_pv)).await?;
            self.store.apply(AnyResource::PersistentVolumeClaim(updated_pvc)).await?;
            return Ok(());
        }
        Ok(())
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        if let AnyResource::PersistentVolume(pv) = resource {
            ctx.vol.deprovision_pv(pv).await.ok();
        }
        Ok(())
    }
}

fn try_bind_pvc(pv: &k8s_openapi::api::core::v1::PersistentVolume, pvc: &k8s_openapi::api::core::v1::PersistentVolumeClaim) -> Option<k8s_openapi::api::core::v1::PersistentVolumeClaim> {
    let pv_spec = pv.spec.as_ref()?;
    let pvc_spec = pvc.spec.as_ref()?;

    let req_storage = pvc_spec.resources.as_ref()
        .and_then(|r| r.requests.as_ref())
        .and_then(|m| m.get("storage"))
        .map(|q| crate::types::parse_quantity_bytes(q))
        .unwrap_or(0);

    let pv_capacity = pv_spec.capacity.as_ref()
        .and_then(|m| m.get("storage"))
        .map(|q| crate::types::parse_quantity_bytes(q))
        .unwrap_or(0);

    if pv_capacity < req_storage { return None; }

    let pv_modes: Vec<&str> = pv_spec.access_modes.as_ref()
        .map(|m| m.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    let pvc_modes: Vec<&str> = pvc_spec.access_modes.as_ref()
        .map(|m| m.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    if !pvc_modes.is_empty() && !pvc_modes.iter().all(|m| pv_modes.contains(m)) { return None; }

    let pv_name = pv.metadata.name.as_deref()?;
    let mut updated_pvc = pvc.clone();
    let pvc_spec = updated_pvc.spec.as_mut()?;
    pvc_spec.volume_name = Some(pv_name.to_string());
    updated_pvc.status = Some(PersistentVolumeClaimStatus {
        phase: Some("Bound".into()),
        capacity: pv_spec.capacity.clone(),
        access_modes: pv_spec.access_modes.clone(),
        ..Default::default()
    });

    Some(updated_pvc)
}
