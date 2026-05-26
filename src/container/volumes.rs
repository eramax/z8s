use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Secret, Volume};
use nix::mount::{mount, MsFlags};
use std::path::Path;
use tracing::{info, warn};

use crate::container::rootfs;

pub struct ResolvedVolume {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

pub fn base_dir() -> String {
    if rootfs::is_root() {
        "/var/lib/z8s".to_string()
    } else {
        format!(
            "{}/.local/share/z8s",
            std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
        )
    }
}

pub fn prepare_volumes(
    pod: &Pod,
    container_name: &str,
    pod_uid: &str,
    get_configmap: &dyn Fn(&str, &str) -> Option<ConfigMap>,
    get_secret: &dyn Fn(&str, &str) -> Option<Secret>,
) -> Result<Vec<ResolvedVolume>> {
    let spec = match pod.spec.as_ref() {
        Some(s) => s,
        None => return Ok(vec![]),
    };

    let namespace = pod.metadata.namespace.as_deref().unwrap_or("default");
    let base = base_dir();

    let volume_map: std::collections::HashMap<&str, &Volume> = spec
        .volumes
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|v| (v.name.as_str(), v))
        .collect();

    let mounts = spec
        .containers
        .iter()
        .find(|c| c.name == container_name)
        .and_then(|c| c.volume_mounts.as_ref());

    let mounts = match mounts {
        Some(m) => m,
        None => return Ok(vec![]),
    };

    let mut resolved = Vec::new();
    for mount in mounts {
        let vol = match volume_map.get(mount.name.as_str()) {
            Some(v) => v,
            None => {
                warn!("Volume mount '{}' references unknown volume, skipping", mount.name);
                continue;
            }
        };

        match resolve_volume_source(vol, namespace, pod_uid, &base, get_configmap, get_secret) {
            Ok(Some((host_path, _))) => {
                resolved.push(ResolvedVolume {
                    host_path,
                    container_path: mount.mount_path.clone(),
                    read_only: mount.read_only.unwrap_or(false),
                });
            }
            Ok(None) => {}
            Err(e) => {
                warn!("Failed to resolve volume '{}': {:#}", mount.name, e);
            }
        }
    }

    Ok(resolved)
}

fn resolve_volume_source(
    vol: &Volume,
    namespace: &str,
    pod_uid: &str,
    base: &str,
    get_configmap: &dyn Fn(&str, &str) -> Option<ConfigMap>,
    get_secret: &dyn Fn(&str, &str) -> Option<Secret>,
) -> Result<Option<(String, bool)>> {
    if let Some(hp) = &vol.host_path {
        return Ok(Some((hp.path.clone(), false)));
    }

    if vol.empty_dir.is_some() {
        let safe_uid = pod_uid.replace('/', "_");
        let dir = format!("{}/emptydir/{}-{}", base, safe_uid, vol.name);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create emptydir at {}", dir))?;
        return Ok(Some((dir, false)));
    }

    if let Some(cm_src) = &vol.config_map {
        let cm_name = &cm_src.name;
        let dir = format!("{}/configmaps/{}/{}", base, namespace, cm_name);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create configmap dir {}", dir))?;
        if let Some(cm) = get_configmap(namespace, cm_name) {
            materialize_configmap(&cm, &dir)?;
        } else if cm_src.optional.unwrap_or(false) {
            warn!("ConfigMap {}/{} not found (optional, continuing)", namespace, cm_name);
        } else {
            warn!("ConfigMap {}/{} not found, volume will be empty", namespace, cm_name);
        }
        return Ok(Some((dir, false)));
    }

    if let Some(sec_src) = &vol.secret {
        if let Some(sec_name) = &sec_src.secret_name {
            let dir = format!("{}/secrets/{}/{}", base, namespace, sec_name);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create secret dir {}", dir))?;
            if let Some(sec) = get_secret(namespace, sec_name) {
                materialize_secret(&sec, &dir)?;
            } else if sec_src.optional.unwrap_or(false) {
                warn!("Secret {}/{} not found (optional, continuing)", namespace, sec_name);
            } else {
                warn!("Secret {}/{} not found, volume will be empty", namespace, sec_name);
            }
            return Ok(Some((dir, false)));
        }
    }

    warn!(
        "Volume '{}' has no supported source (hostPath/emptyDir/configMap/secret), skipping",
        vol.name
    );
    Ok(None)
}

pub fn materialize_configmap(cm: &ConfigMap, dir: &str) -> Result<()> {
    if let Some(data) = &cm.data {
        for (key, value) in data {
            let path = Path::new(dir).join(key);
            std::fs::write(&path, value)
                .with_context(|| format!("Failed to write configmap key '{}'", key))?;
        }
    }
    if let Some(binary_data) = &cm.binary_data {
        for (key, value) in binary_data {
            let path = Path::new(dir).join(key);
            std::fs::write(&path, &value.0)
                .with_context(|| format!("Failed to write configmap binary key '{}'", key))?;
        }
    }
    Ok(())
}

pub fn materialize_secret(sec: &Secret, dir: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(data) = &sec.data {
        for (key, value) in data {
            let path = Path::new(dir).join(key);
            std::fs::write(&path, &value.0)
                .with_context(|| format!("Failed to write secret key '{}'", key))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).ok();
        }
    }
    if let Some(string_data) = &sec.string_data {
        for (key, value) in string_data {
            let path = Path::new(dir).join(key);
            std::fs::write(&path, value)
                .with_context(|| format!("Failed to write secret string key '{}'", key))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).ok();
        }
    }
    Ok(())
}

pub fn bind_mount_volumes(rootfs_path: &str, volumes: &[ResolvedVolume]) {
    for vol in volumes {
        let dst = format!("{}{}", rootfs_path, vol.container_path);
        let src = Path::new(&vol.host_path);
        let dst_path = Path::new(&dst);

        if src.is_dir() {
            std::fs::create_dir_all(dst_path).ok();
        } else {
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(dst_path, b"").ok();
        }

        let flags = MsFlags::MS_BIND | MsFlags::MS_REC;
        match mount(Some(src), dst_path, None::<&str>, flags, None::<&str>) {
            Ok(()) => {
                if vol.read_only {
                    // Remount read-only (MS_BIND alone ignores MS_RDONLY on first mount)
                    let ro_flags = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY;
                    mount(Some(src), dst_path, None::<&str>, ro_flags, None::<&str>).ok();
                }
                info!("Mounted volume {} → {}", vol.host_path, vol.container_path);
            }
            Err(e) => {
                warn!("Failed to bind-mount {} → {}: {} — trying file copy fallback", vol.host_path, vol.container_path, e);
                if src.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(src) {
                        for entry in entries.flatten() {
                            let dst_file = dst_path.join(entry.file_name());
                            if entry.path().is_file() {
                                std::fs::copy(entry.path(), &dst_file).ok();
                            }
                        }
                        info!("Copied files {} → {}", vol.host_path, vol.container_path);
                    }
                }
            }
        }
    }
}

pub fn cleanup_emptydir(pod_uid: &str) {
    let base = base_dir();
    let emptydir_base = format!("{}/emptydir", base);
    let safe_uid = pod_uid.replace('/', "_");
    let prefix = format!("{}-", safe_uid);
    let Ok(entries) = std::fs::read_dir(&emptydir_base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) {
            std::fs::remove_dir_all(entry.path()).ok();
        }
    }
}
