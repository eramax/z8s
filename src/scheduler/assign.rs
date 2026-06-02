//! Leader scheduling helpers (P1c): assign pods + WaitForFirstConsumer volume bind (SC3).

use std::sync::Arc;

use tracing::warn;

use crate::storage::{self, StorageProvisioner};
use crate::store::{AnyResource, StoreBackend};
use crate::types::Pod;

/// Provision PVCs referenced by a pod that use WaitForFirstConsumer on the chosen node.
pub async fn provision_wait_for_first_consumer(
    store: &Arc<dyn StoreBackend>,
    vol: &Arc<dyn StorageProvisioner>,
    pod: &Pod,
    node: &str,
) -> anyhow::Result<()> {
    let Some(spec) = &pod.spec else {
        return Ok(());
    };
    let Some(volumes) = &spec.volumes else {
        return Ok(());
    };
    for vol_entry in volumes {
        let Some(claim) = &vol_entry.persistent_volume_claim else {
            continue;
        };
        let claim_name = &claim.claim_name;
        let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
        let pvc_uid = format!("PersistentVolumeClaim/{ns}/{claim_name}");
        let Some(tracker) = store.get(&pvc_uid).await else {
            continue;
        };
        let AnyResource::PersistentVolumeClaim(pvc) = tracker.resource else {
            continue;
        };
        if pvc
            .spec
            .as_ref()
            .and_then(|s| s.volume_name.as_ref())
            .is_some()
        {
            continue;
        }
        let class_name = match pvc
            .spec
            .as_ref()
            .and_then(|s| s.storage_class_name.as_ref())
        {
            Some(n) => n.clone(),
            None => storage::class::default_storage_class_name(store.as_ref())
                .await
                .unwrap_or_default(),
        };
        if class_name.is_empty() {
            continue;
        }
        let Some(class) = storage::class::resolve_storage_class(store.as_ref(), &class_name).await?
        else {
            continue;
        };
        if !storage::class::volume_binding_wait_for_consumer(&class) {
            continue;
        }
        if let Err(e) = vol
            .provision_for_pvc_on_node(&pvc, &class, node)
            .await
        {
            warn!(
                "WaitForFirstConsumer provision {}/{} on {}: {}",
                ns, claim_name, node, e
            );
            return Err(e);
        }
    }
    Ok(())
}
