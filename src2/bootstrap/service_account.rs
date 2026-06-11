//! Ensure default ServiceAccount exists in `default` namespace.

use crate::store::{AnyResource, StoreBackend};
use crate::types::{ObjectMeta, ServiceAccount};

pub async fn ensure_default_service_account(store: &dyn StoreBackend) -> anyhow::Result<()> {
    let trackers = store.get_by_kind("ServiceAccount").await;
    let exists = trackers.iter().any(|t| {
        let AnyResource::ServiceAccount(sa) = &t.resource else {
            return false;
        };
        sa.metadata.namespace.as_deref() == Some("default")
            && sa.metadata.name.as_deref() == Some("default")
    });
    if exists {
        return Ok(());
    }
    let sa = ServiceAccount {
        api_version: "v1".to_string(),
        kind: "ServiceAccount".to_string(),
        metadata: ObjectMeta {
            name: Some("default".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        secrets: None,
    };
    store.apply(AnyResource::ServiceAccount(sa)).await?;
    Ok(())
}
