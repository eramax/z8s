use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};
use tracing::info;

use crate::store::ops::StoreOp;
use crate::store::snapshot::StoreSnapshot;
use crate::store::{AnyResource, ResourceState, ResourceTracker};

use super::backend::StoreBackend;

const RESOURCES: TableDefinition<&str, &[u8]> = TableDefinition::new("resources");
const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");
const LEASES: TableDefinition<&str, &[u8]> = TableDefinition::new("leases");
const EVENTS: TableDefinition<&str, &[u8]> = TableDefinition::new("events");

pub struct RedbBackend {
    db: Arc<Database>,
}

impl RedbBackend {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        std::fs::create_dir_all(path)?;
        let db_path = path.join("z8s.redb");
        info!("Opening redb database at {}", db_path.display());
        let db = if db_path.exists() {
            Database::open(&db_path)?
        } else {
            Database::create(&db_path)?
        };
        {
            let write_txn = db.begin_write()?;
            write_txn.open_table(RESOURCES)?;
            write_txn.open_table(NODES)?;
            write_txn.open_table(LEASES)?;
            write_txn.open_table(EVENTS)?;
            write_txn.commit()?;
        }
        Ok(Self { db: Arc::new(db) })
    }

    pub fn open_at(dir: impl AsRef<Path>, filename: &str) -> anyhow::Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let db_path = dir.join(filename);
        info!("Opening redb database at {}", db_path.display());
        let db = if db_path.exists() {
            Database::open(&db_path)?
        } else {
            Database::create(&db_path)?
        };
        {
            let write_txn = db.begin_write()?;
            write_txn.open_table(RESOURCES)?;
            write_txn.open_table(NODES)?;
            write_txn.open_table(LEASES)?;
            write_txn.open_table(EVENTS)?;
            write_txn.commit()?;
        }
        Ok(Self { db: Arc::new(db) })
    }

    pub fn write_lease_epoch(lease: &mut crate::types::LeaseRecord, holder: &str) {
        lease.epoch += 1;
        lease.holder = holder.to_string();
        lease.acquired_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        lease.expires_at_ms = lease.acquired_at_ms + 30_000;
    }
}

#[async_trait]
impl StoreBackend for RedbBackend {
    async fn apply(&self, resource: AnyResource) -> anyhow::Result<()> {
        let db = self.db.clone();
        let key = resource.uid();
        let bytes = serde_json::to_vec(&resource)?;
        tracing::debug!("DB apply: key={}, size={}", key, bytes.len());
        tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(RESOURCES)?;
                table.insert(key.as_str(), bytes.as_slice())?;
                // Verify by reading back
                if let Ok(v) = table.get(key.as_str()) {
                    tracing::debug!("DB verify: key={}, found={}", key, v.is_some());
                }
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
                if let Ok(resource) = serde_json::from_slice::<AnyResource>(bytes) {
                    result.push(ResourceTracker::new(resource));
                }
            }
            Ok(result)
        })
        .await
        .unwrap_or_else(|_| Ok(vec![]))
        .unwrap_or_default()
    }

    async fn get_by_kind(&self, kind: &str) -> Vec<ResourceTracker> {
        let db = self.db.clone();
        let prefix = format!("{}/", kind);
        let kind = kind.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<ResourceTracker>, anyhow::Error> {
            let read_txn = db.begin_read()?;
            let table = read_txn.open_table(RESOURCES)?;
            let mut result = Vec::new();
            let mut total = 0u64;
            for item in table.iter()? {
                total += 1;
                let (key, value) = item?;
                if key.value().starts_with(&prefix) {
                    let bytes = value.value();
                    if let Ok(resource) = serde_json::from_slice::<AnyResource>(bytes) {
                        result.push(ResourceTracker::new(resource));
                    } else {
                        tracing::warn!("DB deserialize fail for key={}", key.value());
                    }
                }
            }
            tracing::debug!(
                "DB get_by_kind({}): total_keys={}, matched={}",
                kind,
                total,
                result.len()
            );
            Ok(result)
        })
        .await
        .unwrap_or_else(|_| Ok(vec![]))
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
            let resource = serde_json::from_slice::<AnyResource>(bytes).ok()?;
            Some(ResourceTracker::new(resource))
        })
        .await
        .unwrap_or(None)
    }

    async fn update_state(&self, _uid: &str, _state: ResourceState) {
        // State is ephemeral — rebuilt from process tracker on restart.
    }

    async fn apply_batch(&self, ops: Vec<StoreOp>) -> anyhow::Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(RESOURCES)?;
                for op in ops {
                    match op {
                        StoreOp::Upsert(resource) => {
                            let key = resource.uid();
                            let bytes = serde_json::to_vec(&resource)?;
                            table.insert(key.as_str(), bytes.as_slice())?;
                        }
                        StoreOp::Delete(resource) => {
                            let key = resource.uid();
                            table.remove(key.as_str()).ok();
                        }
                    }
                }
            }
            write_txn.commit()?;
            Ok(())
        })
        .await?
    }

    async fn snapshot(&self) -> StoreSnapshot {
        StoreSnapshot::from_trackers(self.get_all().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreBackend;

    fn temp_db() -> RedbBackend {
        let dir = std::env::temp_dir().join(format!("z8s-test-{}", crate::config::random_id()));
        RedbBackend::open(&dir).unwrap()
    }

    #[tokio::test]
    async fn apply_and_get() {
        let db = temp_db();
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cm1\n  namespace: default\ndata:\n  key: val\n";
        let resource = crate::store::parse_manifest_yaml(yaml).unwrap().remove(0);
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
        let cm = crate::store::parse_manifest_yaml(
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n  namespace: ns1\n",
        )
        .unwrap()
        .remove(0);
        let secret = crate::store::parse_manifest_yaml(
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
        let yaml =
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: delme\n  namespace: default\n";
        let resource = crate::store::parse_manifest_yaml(yaml).unwrap().remove(0);
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
        let r1 = crate::store::parse_manifest_yaml(yaml1).unwrap().remove(0);
        let r2 = crate::store::parse_manifest_yaml(yaml2).unwrap().remove(0);
        db.apply(r1).await.unwrap();
        db.apply(r2).await.unwrap();
        let all = db.get_by_kind("ConfigMap").await;
        assert_eq!(all.len(), 1);
    }
}

// ── Node/lease operations (not part of StoreBackend trait) ────────────────────

impl RedbBackend {
    pub async fn read_node(&self, node_name: &str) -> Option<crate::types::NodeRecord> {
        let db = self.db.clone();
        let key = node_name.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db.begin_read().ok()?;
            let table = read_txn.open_table(NODES).ok()?;
            let value = table.get(key.as_str()).ok()??;
            serde_json::from_slice::<crate::types::NodeRecord>(value.value()).ok()
        })
        .await
        .unwrap_or(None)
    }

    pub async fn write_node(&self, record: &crate::types::NodeRecord) -> anyhow::Result<()> {
        let db = self.db.clone();
        let key = record.node_name.clone();
        let bytes = serde_json::to_vec(record)?;
        tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(NODES)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            write_txn.commit()?;
            Ok(())
        })
        .await?
    }

    pub async fn read_lease(&self) -> Option<crate::types::LeaseRecord> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let read_txn = db.begin_read().ok()?;
            let table = read_txn.open_table(LEASES).ok()?;
            let value = table.get("scheduler").ok()??;
            serde_json::from_slice::<crate::types::LeaseRecord>(value.value()).ok()
        })
        .await
        .unwrap_or(None)
    }

    pub async fn write_lease(&self, record: &crate::types::LeaseRecord) -> anyhow::Result<()> {
        let db = self.db.clone();
        let bytes = serde_json::to_vec(record)?;
        tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(LEASES)?;
                table.insert("scheduler", bytes.as_slice())?;
            }
            write_txn.commit()?;
            Ok(())
        })
        .await?
    }
}
