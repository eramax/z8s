//! Container rootfs preparation from shared image cache (C2 overlay path).

use anyhow::Result;

use crate::config;
use crate::cri::image::ImageManager;

/// Materialize a per-container rootfs from a shared image cache directory.
/// Uses OverlayFS when `--overlay-rootfs` is set and the kernel allows it;
/// otherwise copies the cache tree (default).
pub fn prepare_container_rootfs(
    cache_path: &str,
    container_rootfs: &str,
    meta_path: &str,
    image_ref: &str,
) -> Result<String> {
    if config::get().overlay_rootfs {
        ImageManager::mount_overlay_rootfs(cache_path, container_rootfs, meta_path, image_ref)
    } else {
        ImageManager::copy_cache_to_container(cache_path, container_rootfs, meta_path, image_ref)
    }
}
