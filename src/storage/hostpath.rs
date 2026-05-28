use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{PersistentVolume, PersistentVolumeClaim};
use std::path::Path;
use tracing::info;

use super::StorageClass;

pub struct HostPathProvisioner;

impl HostPathProvisioner {
    pub async fn provision(&self, pv: &mut PersistentVolume, _pvc: &PersistentVolumeClaim, _class: &StorageClass) -> Result<()> {
        let host_path = pv.spec.as_ref()
            .and_then(|s| s.host_path.as_ref())
            .map(|h| h.path.as_str())
            .context("PV has no hostPath")?;

        if !Path::new(host_path).exists() {
            info!("Creating hostPath directory {}", host_path);
            std::fs::create_dir_all(host_path)
                .with_context(|| format!("create dir {}", host_path))?;
        }

        Ok(())
    }

    pub async fn deprovision(&self, pv: &PersistentVolume) -> Result<()> {
        let host_path = match pv.spec.as_ref().and_then(|s| s.host_path.as_ref()) {
            Some(h) => h.path.as_str(),
            None => return Ok(()),
        };
        if Path::new(host_path).exists() {
            info!("Removing hostPath directory {}", host_path);
            let _ = std::fs::remove_dir_all(host_path);
        }
        Ok(())
    }
}
