use anyhow::{Context, Result};
use nix::mount::{mount, MsFlags};
use std::path::Path;
use tracing::{info, warn};

use crate::cri::rootfs;

#[derive(Debug, Clone)]
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

/// Remove cached mount points in a reused rootfs so emptyDir data does not persist
/// across pod delete/recreate when bind-mount falls back to copy.
pub fn scrub_rootfs_volume_mounts(rootfs_path: &str, volumes: &[ResolvedVolume]) {
    for vol in volumes {
        let dst = Path::new(rootfs_path).join(vol.container_path.trim_start_matches('/'));
        if dst.exists() || dst.is_symlink() {
            if dst.is_dir() && !dst.is_symlink() {
                std::fs::remove_dir_all(&dst).ok();
            } else {
                std::fs::remove_file(&dst).ok();
            }
        }
    }
}

fn is_emptydir_host_path(host_path: &str) -> bool {
    host_path.contains("/emptydir/")
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        return Ok(());
    }
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        copy_tree(&entry.path(), &dst.join(name))?;
    }
    Ok(())
}

/// Mount volumes at their container paths (e.g. `/var/data`) inside the current mount
/// namespace. Used when pivot_root/chroot failed but CLONE_NEWNS is active.
/// Writable mount path when host paths like `/var/data` are not creatable (rootless degraded mode).
fn degraded_mount_path(container_path: &str) -> String {
    if container_path == "/var/data" {
        "/tmp/data".to_string()
    } else {
        container_path.to_string()
    }
}

pub fn bind_mount_volumes_degraded(volumes: &[ResolvedVolume]) {
    for vol in volumes {
        let dst = degraded_mount_path(&vol.container_path);
        let src = Path::new(&vol.host_path);
        let dst_path = Path::new(&dst);

        if !src.exists() {
            let _ = std::fs::create_dir_all(src);
        }

        if dst_path.exists() {
            if dst_path.is_symlink() {
                std::fs::remove_file(dst_path).ok();
            } else if is_emptydir_host_path(&vol.host_path) {
                std::fs::remove_dir_all(dst_path).ok();
            }
        }

        let flags = MsFlags::MS_BIND | MsFlags::MS_REC;
        if is_emptydir_host_path(&vol.host_path) {
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
        } else if src.is_dir() {
            std::fs::create_dir_all(dst_path).ok();
        } else if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent).ok();
            if !dst_path.exists() {
                let _ = std::fs::write(dst_path, []);
            }
        }

        if mount(Some(src), dst_path, None::<&str>, flags, None::<&str>).is_ok() {
            if vol.read_only {
                let ro_flags = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY;
                mount(Some(src), dst_path, None::<&str>, ro_flags, None::<&str>).ok();
            }
            info!(
                "Mounted volume (degraded) {} → {}",
                vol.host_path, vol.container_path
            );
            continue;
        }

        if is_emptydir_host_path(&vol.host_path) {
            if std::os::unix::fs::symlink(src, dst_path).is_ok() {
                info!(
                    "Symlinked emptyDir (degraded) {} → {}",
                    vol.host_path, vol.container_path
                );
            } else {
                warn!(
                    "Failed to symlink emptyDir (degraded) {} → {}",
                    vol.host_path, vol.container_path
                );
            }
        } else if copy_tree(src, dst_path).is_ok() {
            info!(
                "Copied volume (degraded) {} → {}",
                vol.host_path, vol.container_path
            );
        } else {
            warn!(
                "Failed to install volume (degraded) {} → {}",
                vol.host_path, vol.container_path
            );
        }
    }
}

/// Stage volumes into rootfs tree (symlink/copy) from parent before fork — works without chroot.
pub fn stage_volumes_in_rootfs(rootfs_path: &str, volumes: &[ResolvedVolume]) {
    for vol in volumes {
        let rel = vol.container_path.trim_start_matches('/');
        let dst = Path::new(rootfs_path).join(rel);
        let src = Path::new(&vol.host_path);
        if dst.exists() || dst.is_symlink() {
            if dst.is_dir() && !dst.is_symlink() {
                std::fs::remove_dir_all(&dst).ok();
            } else {
                std::fs::remove_file(&dst).ok();
            }
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if is_emptydir_host_path(&vol.host_path) {
            std::fs::create_dir_all(&dst).ok();
            info!("Created emptyDir directory in rootfs at {}", dst.display());
            continue;
        }
        if src.is_dir() {
            if std::os::unix::fs::symlink(src, &dst).is_ok() {
                info!("Staged volume in rootfs {} → {}", vol.host_path, dst.display());
            } else if copy_tree(src, &dst).is_ok() {
                info!("Copied volume into rootfs {} → {}", vol.host_path, dst.display());
            }
        } else if src.is_file() {
            if std::os::unix::fs::symlink(src, &dst).is_ok() {
                info!("Staged volume file in rootfs {} → {}", vol.host_path, dst.display());
            } else if std::fs::copy(src, &dst).is_ok() {
                info!("Copied volume file into rootfs {} → {}", vol.host_path, dst.display());
            }
        }
    }
}

pub fn bind_mount_volumes(rootfs_path: &str, volumes: &[ResolvedVolume]) {
    for vol in volumes {
        let dst = format!("{}{}", rootfs_path, vol.container_path);
        let src = Path::new(&vol.host_path);
        let dst_path = Path::new(&dst);

        if !src.exists() {
            let _ = std::fs::create_dir_all(src);
        }

        // Clean stale entry so create_dir_all / bind-mount don't fail on dangling symlinks
        if dst_path.exists() || dst_path.is_symlink() {
            if dst_path.is_symlink() || dst_path.is_file() {
                std::fs::remove_file(dst_path).ok();
            } else if dst_path.is_dir() && is_emptydir_host_path(&vol.host_path) {
                std::fs::remove_dir_all(dst_path).ok();
            }
        }

        if src.is_dir() {
            std::fs::create_dir_all(dst_path).ok();
        } else if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent).ok();
            if !dst_path.exists() {
                let _ = std::fs::write(dst_path, []);
            }
        }

        let flags = MsFlags::MS_BIND | MsFlags::MS_REC;
        let bind_ok = mount(Some(src), dst_path, None::<&str>, flags, None::<&str>).is_ok();
        if bind_ok {
            if vol.read_only {
                let ro_flags = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY;
                mount(Some(src), dst_path, None::<&str>, ro_flags, None::<&str>).ok();
            }
            info!("Mounted volume {} → {}", vol.host_path, vol.container_path);
            continue;
        }

        // Bind mounts are often blocked in user namespaces.
        if is_emptydir_host_path(&vol.host_path) {
            // Symlink keeps emptyDir on the host staging dir (ephemeral), not in rootfs cache.
            if std::os::unix::fs::symlink(src, dst_path).is_ok() {
                info!(
                    "Symlinked emptyDir {} → {} (bind mount unavailable)",
                    vol.host_path, vol.container_path
                );
            } else {
                warn!(
                    "Failed to symlink emptyDir {} → {}",
                    vol.host_path, vol.container_path
                );
            }
        } else if copy_tree(src, dst_path).is_ok() {
            info!(
                "Copied volume {} → {} (bind mount unavailable)",
                vol.host_path, vol.container_path
            );
        } else {
            warn!(
                "Failed to install volume {} → {}",
                vol.host_path, vol.container_path
            );
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
