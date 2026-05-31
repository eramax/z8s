use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};
use tracing::info;

use crate::types::{AnyResource, ResourceState, ResourceTracker};

use super::backend::StoreBackend;

const RESOURCES: TableDefinition<&str, &[u8]> = TableDefinition::new("resources");

pub struct RedbBackend {
    db: Arc<Database>,
}

impl RedbBackend {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        std::fs::create_dir_all(path)?;
        let db_path = path.join("z8s.redb");
        info!("Opening redb database at {}", db_path.display());
        let db = Database::create(&db_path)?;
        {
            let write_txn = db.begin_write()?;
            write_txn.open_table(RESOURCES)?;
            write_txn.commit()?;
        }
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl StoreBackend for RedbBackend {
    async fn apply(&self, resource: AnyResource) -> anyhow::Result<()> {
        let db = self.db.clone();
        let key = resource.uid();
        let bytes = bincode::serialize(&resource)?;
        tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(RESOURCES)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            write_txn.commit()?;
            Ok(())
        })
        .await?
    }

    async fn delete(&self, resource: &AnyResource) -> anyhow::Result<()> {
        let db = self.db.clone();
        let key = resource.uid();
        tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(RESOURCES)?;
                table.remove(key.as_str()).ok();
            }
            write_txn.commit()?;
            Ok(())
        })
        .await?
    }

    async fn get_all(&self) -> Vec<ResourceTracker> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<ResourceTracker>, anyhow::Error> {
            let read_txn = db.begin_read()?;
            let table = read_txn.open_table(RESOURCES)?;
            let mut result = Vec::new();
            for item in table.iter()? {
                let (_key, value) = item?;
                let bytes = value.value();
                if let Ok(resource) = bincode::deserialize::<AnyResource>(bytes) {
                    result.push(ResourceTracker::new(resource));
                }
            }
            Ok(result)
        })
        .await
        .unwrap_or_else(|e| Ok(vec![]))
        .unwrap_or_default()
    }

    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker> {
        let db = self.db.clone();
        let prefix = format!("{}/", kind);
        tokio::task::spawn_blocking(move || -> Result<Vec<ResourceTracker>, anyhow::Error> {
            let read_txn = db.begin_read()?;
            let table = read_txn.open_table(RESOURCES)?;
            let mut result = Vec::new();
            for item in table.iter()? {
                let (key, value) = item?;
                if key.value().starts_with(&prefix) {
                    let bytes = value.value();
                    if let Ok(resource) = bincode::deserialize::<AnyResource>(bytes) {
                        result.push(ResourceTracker::new(resource));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreBackend;

    fn temp_db() -> RedbBackend {
        let dir = std::env::temp_dir().join(format!("z8s-test-{}", uuid::Uuid::new_v4()));
        RedbBackend::open(&dir).unwrap()
    }

    #[tokio::test]
    async fn apply_and_get() {
        let db = temp_db();
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cm1\n  namespace: default\ndata:\n  key: val\n";
        let resource = crate::types::parse_manifest_yaml(yaml).unwrap().remove(0);
        db.apply(resource).await.unwrap();
        let got = db.get("ConfigMap/default/cm1").await;
        assert!(got.is_some());
        if let Some(tracker) = got {
            assert_eq!(tracker.resource.name(), "cm1");
        }
    }

    #[tokio::test]
    async fn get_by_kind_prefix_scan() {
        let db = temp_db();
        let cm = crate::types::parse_manifest_yaml(
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n  namespace: ns1\n",
        )
        .unwrap()
        .remove(0);
        let secret = crate::types::parse_manifest_yaml(
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: s\n  namespace: ns1\n",
        )
        .unwrap()
        .remove(0);
        db.apply(cm).await.unwrap();
        db.apply(secret).await.unwrap();
        let cms = db.get_by_kind("ConfigMap").await;
        assert_eq!(cms.len(), 1);
        let secrets = db.get_by_kind("Secret").await;
        assert_eq!(secrets.len(), 1);
    }

    #[tokio::test]
    async fn delete_removes_entry() {
        let db = temp_db();
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: delme\n  namespace: default\n";
        let resource = crate::types::parse_manifest_yaml(yaml).unwrap().remove(0);
        db.apply(resource.clone()).await.unwrap();
        assert!(db.get("ConfigMap/default/delme").await.is_some());
        db.delete(&resource).await.unwrap();
        assert!(db.get("ConfigMap/default/delme").await.is_none());
    }

    #[tokio::test]
    async fn apply_overwrites_existing() {
        let db = temp_db();
        let yaml1 = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: ow\n  namespace: default\ndata:\n  k: v1\n";
        let yaml2 = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: ow\n  namespace: default\ndata:\n  k: v2\n";
        let r1 = crate::types::parse_manifest_yaml(yaml1).unwrap().remove(0);
        let r2 = crate::types::parse_manifest_yaml(yaml2).unwrap().remove(0);
        db.apply(r1).await.unwrap();
        db.apply(r2).await.unwrap();
        let all = db.get_by_kind("ConfigMap").await;
        assert_eq!(all.len(), 1);
    }
}
                }
            }
            Ok(result)
        })
        .await
        .unwrap_or_else(|e| Ok(vec![]))
        .unwrap_or_default()
    }

    async fn get(&self, uid: &str) -> Option<ResourceTracker> {
        let db = self.db.clone();
        let key = uid.to_string();
        tokio::task::spawn_blocking(move || -> Option<ResourceTracker> {
            let read_txn = db.begin_read().ok()?;
            let table = read_txn.open_table(RESOURCES).ok()?;
            let value = table.get(key.as_str()).ok()??;
            let bytes = value.value();
            let resource = bincode::deserialize::<AnyResource>(bytes).ok()?;
            Some(ResourceTracker::new(resource))
        })
        .await
        .unwrap_or(None)
    }

    async fn update_state(&self, _uid: &str, _state: ResourceState) {
        // State is ephemeral in MemoryBackend — for RedbBackend, state lives on the resource itself.
        // This will be implemented properly in Phase 1c when we have our own types with status fields.
        // For now, this is a no-op since the resource's status is updated via apply() from the worker.
    }
}
