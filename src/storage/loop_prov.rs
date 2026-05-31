use anyhow::{Context, Result};
use crate::types::{PersistentVolume, PersistentVolumeClaim};
use std::path::Path;
use tracing::{info, warn};

use super::StorageClass;

pub struct LoopProvisioner;

impl LoopProvisioner {
    pub async fn provision(&self, pv: &mut PersistentVolume, _pvc: &PersistentVolumeClaim, _class: &StorageClass) -> Result<()> {
        let host_path = match pv.spec.as_ref()
            .and_then(|s| s.host_path.as_ref())
            .map(|h| h.path.clone())
        {
            Some(p) => p,
            None => anyhow::bail!("PV has no hostPath"),
        };

        let img_path = format!("{}.img", host_path);

        if Path::new(&host_path).exists() {
            if is_mounted(&host_path) {
                info!("Loop PV {} already mounted at {}", pv.metadata.name.as_deref().unwrap_or("?"), host_path);
                return Ok(());
            }
            info!("Loop PV {} hostPath exists but not mounted, re-provisioning", pv.metadata.name.as_deref().unwrap_or("?"));
            let _ = std::fs::remove_dir_all(&host_path);
        }

        let capacity = pv.spec.as_ref()
            .and_then(|s| s.capacity.as_ref())
            .and_then(|m| m.get("storage"))
            .map(|q| crate::store::parse_quantity_bytes(q))
            .unwrap_or(0);

        if capacity == 0 {
            anyhow::bail!("loop provisioner requires non-zero capacity");
        }

        let parent = Path::new(&img_path).parent()
            .context("image path has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;

        std::fs::create_dir_all(&host_path)
            .with_context(|| format!("create mount dir {}", host_path))?;

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

        info!("Mounting {} at {}", loop_dev, host_path);
        if let Err(e) = run("mount", &[&loop_dev, &host_path]) {
            let _ = run("losetup", &["-d", &loop_dev]);
            cleanup_file(&img_path);
            anyhow::bail!("mount failed: {}", e);
        }

        let ann = pv.metadata.annotations.get_or_insert_with(Default::default);
        ann.insert("z8s.io/loop-device".into(), loop_dev);
        ann.insert("z8s.io/image-path".into(), img_path);

        if let Err(e) = run("chmod", &["0777", &host_path]) {
            warn!("chmod 0777 {} failed: {}", host_path, e);
        }

        Ok(())
    }

    pub async fn deprovision(&self, pv: &PersistentVolume) -> Result<()> {
        let annotations = match pv.metadata.annotations.as_ref() {
            Some(a) => a,
            None => return Ok(()),
        };
        let host_path = match pv.spec.as_ref()
            .and_then(|s| s.host_path.as_ref())
            .map(|h| h.path.as_str())
        {
            Some(p) => p,
            None => return Ok(()),
        };

        if host_path.is_empty() {
            return Ok(());
        }

        let img_path = annotations.get("z8s.io/image-path").cloned().unwrap_or_default();

        if let Some(loop_dev) = annotations.get("z8s.io/loop-device") {
            if is_mounted(host_path) {
                info!("Unmounting {} for PV {}", host_path, pv.metadata.name.as_deref().unwrap_or("?"));
                if let Err(e) = run("umount", &[host_path]) {
                    warn!("umount {} failed ({}), trying lazy umount", host_path, e);
                    let _ = run("umount", &["-l", host_path]);
                }
            }
            info!("Detaching loop device {} for PV {}", loop_dev, pv.metadata.name.as_deref().unwrap_or("?"));
            if let Err(e) = run("losetup", &["-d", loop_dev]) {
                warn!("losetup -d {} failed: {}", loop_dev, e);
            }
        }

        if !img_path.is_empty() && Path::new(&img_path).exists() {
            info!("Removing backing image {}", img_path);
            cleanup_file(&img_path);
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

fn is_mounted(path: &str) -> bool {
    let output = std::process::Command::new("mountpoint")
        .arg("-q")
        .arg(path)
        .status();
    matches!(output, Ok(s) if s.success())
}
