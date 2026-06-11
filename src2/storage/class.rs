//! StorageClass resolution from the store (SC1).

use anyhow::{Context, Result};
use tracing::info;

use crate::store::{AnyResource, StoreBackend};
use crate::types::{ObjectMeta, StorageClass};

pub fn default_storage_classes() -> Vec<StorageClass> {
    let mut standard_anno = std::collections::BTreeMap::new();
    standard_anno.insert(
        "storageclass.kubernetes.io/is-default-class".into(),
        "true".into(),
    );
    vec![
        StorageClass {
            metadata: ObjectMeta {
                name: Some("standard".into()),
                uid: Some("storageclass-standard".into()),
                annotations: Some(standard_anno),
                ..Default::default()
            },
            provisioner: "z8s.io/loop".into(),
            reclaim_policy: Some("Delete".into()),
            volume_binding_mode: Some("Immediate".into()),
            ..Default::default()
        },
        StorageClass {
            metadata: ObjectMeta {
                name: Some("hostpath".into()),
                uid: Some("storageclass-hostpath".into()),
                ..Default::default()
            },
            provisioner: "z8s.io/hostpath".into(),
            reclaim_policy: Some("Delete".into()),
            volume_binding_mode: Some("Immediate".into()),
            ..Default::default()
        },
    ]
}

/// Seed built-in StorageClasses if missing (main / single-node bootstrap).
pub async fn seed_default_storage_classes(store: &dyn StoreBackend) -> Result<()> {
    for sc in default_storage_classes() {
        let name = sc.metadata.name.as_deref().context("StorageClass name")?;
        let uid = format!("StorageClass/{name}");
        if store.get(&uid).await.is_none() {
            store.apply(AnyResource::StorageClass(sc.clone())).await?;
            info!("Seeded StorageClass '{}'", name);
        }
    }
    Ok(())
}

pub async fn resolve_storage_class(
    store: &dyn StoreBackend,
    name: &str,
) -> Result<Option<StorageClass>> {
    let uid = format!("StorageClass/{name}");
    let Some(tracker) = store.get(&uid).await else {
        return Ok(None);
    };
    match tracker.resource {
        AnyResource::StorageClass(sc) => Ok(Some(sc)),
        _ => Ok(None),
    }
}

pub async fn default_storage_class_name(store: &dyn StoreBackend) -> Option<String> {
    for t in store.get_by_kind("StorageClass").await {
        if let AnyResource::StorageClass(sc) = &t.resource {
            let is_default = sc
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("storageclass.kubernetes.io/is-default-class"))
                .map(|v| v == "true")
                .unwrap_or(false);
            if is_default {
                return sc.metadata.name.clone();
            }
        }
    }
    None
}

pub fn volume_binding_immediate(class: &StorageClass) -> bool {
    class.volume_binding_mode.as_deref() != Some("WaitForFirstConsumer")
}

pub fn volume_binding_wait_for_consumer(class: &StorageClass) -> bool {
    class.volume_binding_mode.as_deref() == Some("WaitForFirstConsumer")
}
