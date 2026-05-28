use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{PersistentVolume, PersistentVolumeClaim};
use std::path::Path;
use tracing::info;

use super::StorageClass;

pub struct LoopProvisioner;

impl LoopProvisioner {
    pub async fn provision(&self, pv: &mut PersistentVolume, _pvc: &PersistentVolumeClaim, _class: &StorageClass) -> Result<()> {
        let host_path = pv.spec.as_ref()
            .and_then(|s| s.host_path.as_ref())
            .map(|h| h.path.as_str())
            .context("PV has no hostPath")?;
        let img_path = format!("{}.img", host_path);

        if Path::new(&host_path).exists() {
            info!("Loop PV {} already mounted at {}", pv.metadata.name.as_deref().unwrap_or("?"), host_path);
            return Ok(());
        }

        let capacity = pv.spec.as_ref()
            .and_then(|s| s.capacity.as_ref())
            .and_then(|m| m.get("storage"))
            .map(|q| crate::types::parse_quantity_bytes(q))
            .unwrap_or(0);

        if capacity == 0 {
            anyhow::bail!("loop provisioner requires non-zero capacity");
        }

        let parent = Path::new(&img_path).parent().unwrap();
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;

        info!("Creating sparse image {} ({} bytes)", img_path, capacity);
        run("truncate", &["-s", &capacity.to_string(), &img_path])?;

        info!("Formatting {} as ext4", img_path);
        run("mkfs.ext4", &["-F", &img_path])?;

        info!("Attaching loop device for {}", img_path);
        let loop_dev = String::from_utf8(
            run_with_output("losetup", &["-f", "--show", &img_path])?
        ).map_err(|e| anyhow::anyhow!("invalid losetup output: {}", e))?;
        let loop_dev = loop_dev.trim().to_string();
        if loop_dev.is_empty() {
            cleanup_file(&img_path);
            anyhow::bail!("losetup returned empty device");
        }

        std::fs::create_dir_all(host_path)
            .with_context(|| format!("create mount dir {}", host_path))?;

        info!("Mounting {} at {}", loop_dev, host_path);
        if let Err(e) = run("mount", &[&loop_dev, host_path]) {
            let _ = run("losetup", &["-d", &loop_dev]);
            cleanup_file(&img_path);
            anyhow::bail!("mount failed: {}", e);
        }

        pv.metadata.annotations.get_or_insert_with(Default::default)
            .insert("z8s.io/loop-device".into(), loop_dev);
        pv.metadata.annotations.get_or_insert_with(Default::default)
            .insert("z8s.io/image-path".into(), img_path);

        Ok(())
    }

    pub async fn deprovision(&self, pv: &PersistentVolume) -> Result<()> {
        let annotations = match pv.metadata.annotations.as_ref() {
            Some(a) => a,
            None => return Ok(()),
        };
        let host_path = pv.spec.as_ref()
            .and_then(|s| s.host_path.as_ref())
            .map(|h| h.path.as_str())
            .unwrap_or("");

        if let Some(loop_dev) = annotations.get("z8s.io/loop-device") {
            info!("Detaching loop device {} for PV {}", loop_dev, pv.metadata.name.as_deref().unwrap_or("?"));
            let _ = run("umount", &[host_path]);
            let _ = run("losetup", &["-d", loop_dev]);
        }

        if let Some(img_path) = annotations.get("z8s.io/image-path") {
            cleanup_file(img_path);
        }

        Ok(())
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("failed to execute {}", cmd))?;
    if !status.success() {
        anyhow::bail!("{} exited with {}", cmd, status);
    }
    Ok(())
}

fn run_with_output(cmd: &str, args: &[&str]) -> Result<Vec<u8>> {
    let output = std::process::Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {}", cmd))?;
    if !output.status.success() {
        anyhow::bail!("{} exited with {}", cmd, output.status);
    }
    Ok(output.stdout)
}

fn cleanup_file(path: &str) {
    let _ = std::fs::remove_file(path);
}
