//! # OCI Image Manager — Pull, Cache, and Unpack Container Images
//!
//! Manages the lifecycle of OCI container images:
//!
//! 1. **Pull** — download image layers from a registry
//! 2. **Cache** — store extracted layers in a shared cache
//! 3. **Unpack** — copy or overlay cache into per-container rootfs
//!
//! ## Caching Strategy
//!
//! - Shared cache: `<base>/images/<hash>` — deduplicates across containers
//! - Container rootfs: `<base>/rootfs/<container_id>` — per-container view
//! - OverlayFS: used when running as root (kernel support required)
//! - Copy fallback: used in rootless mode or when overlay fails
//!
//! ## Layer Order
//!
//! `oci-distribution` returns layers in arbitrary completion order.
//! We re-sort by manifest order before extraction to ensure base layers
//! come first (required for correct whiteout processing).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};

use z8s_core::sys;
use oci_distribution::client::{Client, ClientConfig, ImageLayer};
use oci_distribution::config::ConfigFile;
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use tracing::{debug, info};

use super::rootfs;

const ACCEPTED_LAYER_TYPES: &[&str] = &[
    "application/vnd.docker.image.rootfs.diff.tar.gzip",
    "application/vnd.docker.image.rootfs.diff.tar",
    "application/vnd.oci.image.layer.v1.tar",
    "application/vnd.oci.image.layer.v1.tar+gzip",
    "application/vnd.oci.image.layer.v1.tar+zstd",
];

pub const OCI_CONFIG_FILE: &str = ".z8s-oci-config.json";

// ── Types ──────────────────────────────────────────────────────────────────

/// Saved OCI image configuration (entrypoint, cmd, env).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SavedImageConfig {
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub env: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

// ── ImageManager ───────────────────────────────────────────────────────────

/// Manages OCI image pulling, caching, and rootfs preparation.
pub struct ImageManager {
    client: Client,
    cache_dir: String,
    rootfs_dir: String,
}

impl ImageManager {
    /// Create a new image manager with default paths.
    pub fn new() -> Result<Self> {
        let client = Client::new(ClientConfig::default());
        let base = base_dir();
        let cache_dir = format!("{}/images", base);
        let rootfs_dir = format!("{}/rootfs", base);
        std::fs::create_dir_all(&cache_dir).context("Failed to create image cache directory")?;
        std::fs::create_dir_all(&rootfs_dir).context("Failed to create rootfs directory")?;
        Ok(Self { client, cache_dir, rootfs_dir })
    }

    /// Create a stub manager for degraded mode.
    pub fn new_stub() -> Self {
        let base = base_dir();
        Self {
            client: Client::new(ClientConfig::default()),
            cache_dir: format!("{}/images", base),
            rootfs_dir: format!("{}/rootfs", base),
        }
    }

    /// Pull and unpack an OCI image, returning the container rootfs path.
    /// With overlay, the usable files are in `container_rootfs/merged/`.
    pub async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String> {
        let container_rootfs = format!("{}/{}", self.rootfs_dir, container_id);
        anyhow::ensure!(!container_rootfs.ends_with("/"), "Refusing to unpack to root");

        // Check if this container already has the rootfs cached
        let meta_path = format!("{}/.z8s-image-ref", container_rootfs);
        let merged = format!("{}/merged", container_rootfs);

        // If overlay merged dir exists and is actually mounted, reuse it
        if Path::new(&merged).exists()
            && is_overlay_mounted(&merged)
            && Path::new(&meta_path).exists()
            && std::fs::read_to_string(&meta_path)
                .ok()
                .is_some_and(|c| c.trim() == image_ref)
        {
            info!("Reusing overlay rootfs for {} at {}", image_ref, merged);
            return Ok(merged);
        }

        // Check shared image cache
        let cache_path = self.image_cache_path(image_ref);
        let cache_meta = format!("{}/.z8s-image-ref", cache_path);
        if Path::new(&cache_path).exists()
            && Path::new(&cache_meta).exists()
            && std::fs::read_to_string(&cache_meta)
                .ok()
                .is_some_and(|c| c.trim() == image_ref)
        {
            info!("Copying cached rootfs for {} to {}", image_ref, container_rootfs);
            return prepare_rootfs_from_cache(&cache_path, &container_rootfs, &meta_path, image_ref);
        }

        // Pull from registry
        let reference: Reference = image_ref.parse().context("Invalid image reference")?;
        info!("Pulling image: {} → {}", image_ref, container_id);
        let image_data = self.client
            .pull(&reference, &RegistryAuth::Anonymous, ACCEPTED_LAYER_TYPES.to_vec())
            .await
            .context(format!("Failed to pull image '{}'", image_ref))?;

        // Check cache again (concurrent pull may have populated it)
        if Path::new(&cache_meta).exists()
            && std::fs::read_to_string(&cache_meta)
                .ok()
                .is_some_and(|c| c.trim() == image_ref)
        {
            return prepare_rootfs_from_cache(&cache_path, &container_rootfs, &meta_path, image_ref);
        }

        // Extract to shared cache
        if Path::new(&cache_path).exists() {
            std::fs::remove_dir_all(&cache_path)?;
        }
        std::fs::create_dir_all(&cache_path)?;

        let ordered = reorder_layers(image_data.layers, &image_data.manifest);
        info!("Unpacking {} layers for {}", ordered.len(), image_ref);

        // Save OCI config from registry
        let config_file: ConfigFile = image_data.config
            .clone()
            .try_into()
            .context("Failed to parse OCI image config")?;
        let (ep, cmd, env, wd) = config_file.config
            .as_ref()
            .map(|c| (c.entrypoint.clone(), c.cmd.clone(), c.env.clone(), c.working_dir.clone()))
            .unwrap_or((None, None, None, None));
        save_image_config(&cache_path, ep, cmd, env, wd);

        for (i, layer) in ordered.iter().enumerate() {
            unpack_layer(layer, &cache_path, i)?;
        }
        std::fs::write(&cache_meta, image_ref)?;

        prepare_rootfs_from_cache(&cache_path, &container_rootfs, &meta_path, image_ref)
    }

    fn image_cache_path(&self, image_ref: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        image_ref.hash(&mut hasher);
        format!("{}/{:016x}", self.cache_dir, hasher.finish())
    }

    /// Unmount overlay rootfs.
    pub fn unmount_overlay(rootfs_path: &str) {
        if rootfs_path.ends_with("/merged") {
            sys::umount2(rootfs_path, rustix::mount::UnmountFlags::DETACH).ok();
            if let Some(parent) = Path::new(rootfs_path).parent() {
                sys::umount2(parent.to_str().unwrap(), rustix::mount::UnmountFlags::DETACH).ok();
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }
}

// ── Pure Functions ─────────────────────────────────────────────────────────

/// Check if a directory is an overlay mount point.
fn is_overlay_mounted(path: &str) -> bool {
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        mounts.lines().any(|line| {
            line.contains("overlay") && line.contains(path)
        })
    } else {
        false
    }
}

/// Base directory for z8s data. Always use /home/abb path for consistency.
fn base_dir() -> String {
    "/home/abb/.local/share/z8s".to_string()
}

/// Save OCI image config to disk.
pub fn save_image_config(
    dir: &str,
    entrypoint: Option<Vec<String>>,
    cmd: Option<Vec<String>>,
    env: Option<Vec<String>>,
    working_dir: Option<String>,
) {
    let path = Path::new(dir).join(OCI_CONFIG_FILE);
    let saved = SavedImageConfig { entrypoint, cmd, env, working_dir };
    if let Ok(json) = serde_json::to_string(&saved) {
        std::fs::write(path, json).ok();
    }
}

/// Read OCI image config from rootfs.
pub fn read_image_config(rootfs_path: &str) -> SavedImageConfig {
    let path = Path::new(rootfs_path).join(OCI_CONFIG_FILE);
    if let Some(cfg) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<SavedImageConfig>(&raw).ok())
    {
        let has_ep = cfg.entrypoint.as_ref().is_some_and(|ep| !ep.is_empty());
        let has_cmd = cfg.cmd.as_ref().is_some_and(|c| !c.is_empty());
        if has_ep || has_cmd {
            return cfg;
        }
    }
    guess_image_config(rootfs_path)
}

/// Fallback when OCI config was not stored.
fn guess_image_config(rootfs_path: &str) -> SavedImageConfig {
    use std::os::unix::fs::PermissionsExt;
    let root = Path::new(rootfs_path);
    for rel in ["docker-entrypoint.sh", "usr/local/bin/docker-entrypoint.sh", "bin/sh"] {
        let path = root.join(rel);
        if path.symlink_metadata().is_ok() {
            return SavedImageConfig {
                entrypoint: Some(vec![format!("/{}", rel.trim_start_matches('/'))]),
                ..Default::default()
            };
        }
    }
    // Single executable at root (minimal images)
    if let Ok(entries) = std::fs::read_dir(root) {
        let exes: Vec<String> = entries
            .flatten()
            .filter(|e| {
                let p = e.path();
                p.parent() == Some(root)
                    && (p.is_file() || p.is_symlink())
                    && e.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
            })
            .map(|e| format!("/{}", e.file_name().to_string_lossy()))
            .collect();
        if exes.len() == 1 {
            return SavedImageConfig { entrypoint: Some(exes), ..Default::default() };
        }
    }
    SavedImageConfig::default()
}

/// Reorder layers to match manifest order (base first).
fn reorder_layers(
    layers: Vec<ImageLayer>,
    manifest: &Option<oci_distribution::manifest::OciImageManifest>,
) -> Vec<ImageLayer> {
    let Some(manifest) = manifest else {
        return layers;
    };
    let mut by_digest: HashMap<String, ImageLayer> = layers
        .into_iter()
        .map(|l| (l.sha256_digest(), l))
        .collect();
    let mut ordered = Vec::with_capacity(manifest.layers.len());
    for desc in &manifest.layers {
        if let Some(layer) = by_digest.remove(&desc.digest) {
            ordered.push(layer);
        }
    }
    ordered.extend(by_digest.into_values());
    ordered
}

/// Prepare container rootfs from shared cache using overlayfs.
/// Each container gets an upper/work dir — writes go there, reads come from the shared cache.
/// No copy — overlayfs is CoW by design.
fn prepare_rootfs_from_cache(
    cache_path: &str,
    container_rootfs: &str,
    meta_path: &str,
    image_ref: &str,
) -> Result<String> {
    let merged = try_overlay_mount(cache_path, container_rootfs)?;
    copy_oci_config(cache_path, &merged);
    backfill_oci_config(cache_path, &merged);
    std::fs::write(meta_path, image_ref)?;
    Ok(merged)
}

/// Mount overlayfs: lower=shared cache, upper=per-container writes, work=kernel scratch.
/// The merged view is what the container sees — CoW, no full copy.
fn try_overlay_mount(cache_path: &str, container_rootfs: &str) -> Result<String> {
    let base = container_rootfs;
    let upper = format!("{}/upper", base);
    let work = format!("{}/work", base);
    let merged = format!("{}/merged", base);

    std::fs::create_dir_all(&upper).context("create upper dir")?;
    std::fs::create_dir_all(&work).context("create work dir")?;
    std::fs::create_dir_all(&merged).context("create merged dir")?;

    let opts = format!("lowerdir={},upperdir={},workdir={}", cache_path, upper, work);
    z8s_core::sys::mount(
        Some("overlay"),
        &merged,
        Some("overlay"),
        rustix::mount::MountFlags::empty(),
        Some(&opts),
    )
    .context("overlay mount failed — need root or unprivileged overlay support")?;

    info!("OverlayFS: {} + {} → {}", cache_path, upper, merged);
    Ok(merged)
}

fn copy_oci_config(src_dir: &str, dst_dir: &str) {
    let src = Path::new(src_dir).join(OCI_CONFIG_FILE);
    let dst = Path::new(dst_dir).join(OCI_CONFIG_FILE);
    if src.exists() {
        std::fs::copy(src, dst).ok();
    }
}

fn backfill_oci_config(cache_path: &str, container_rootfs: &str) {
    let oci_cfg = Path::new(cache_path).join(OCI_CONFIG_FILE);
    let needs_guess = !oci_cfg.exists() || {
        let cfg = read_image_config(cache_path);
        cfg.entrypoint.as_ref().is_none_or(|ep| ep.is_empty())
            && cfg.cmd.as_ref().is_none_or(|c| c.is_empty())
    };
    if needs_guess {
        let guessed = guess_image_config(cache_path);
        save_image_config(cache_path, guessed.entrypoint, guessed.cmd, guessed.env, None);
        copy_oci_config(cache_path, container_rootfs);
    }
}

/// Unpack a single OCI image layer (tar.gz or tar).
fn unpack_layer(layer: &ImageLayer, target: &str, index: usize) -> Result<()> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tar::Archive;

    let data = &layer.data;
    debug!("Unpacking layer {} ({} bytes)", index, data.len());

    let mut decompressed: Box<dyn Read> = if data.len() > 2 && data[0] == 0x1f && data[1] == 0x8b {
        Box::new(GzDecoder::new(data.as_slice()))
    } else {
        Box::new(data.as_slice())
    };

    let mut archive = Archive::new(&mut decompressed);
    archive.set_overwrite(true);

    let target_path = Path::new(target);

    for entry_result in archive.entries()
        .context(format!("Failed to read layer {} archive", index))?
    {
        let mut entry = entry_result.context(format!("Corrupt entry in layer {}", index))?;
        let path = entry.path().context("Entry has non-UTF8 path")?.into_owned();
        let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

        if filename == ".wh..wh..opq" {
            // Opaque whiteout: clear directory
            let dir = path.parent().map(|p| target_path.join(p)).unwrap_or_else(|| target_path.to_path_buf());
            if let Ok(children) = std::fs::read_dir(&dir) {
                for child in children.flatten() {
                    if !child.file_name().to_string_lossy().starts_with(".wh.") {
                        let cp = child.path();
                        if cp.is_dir() { std::fs::remove_dir_all(&cp).ok(); }
                        else { std::fs::remove_file(&cp).ok(); }
                    }
                }
            }
        } else if let Some(real_name) = filename.strip_prefix(".wh.") {
            // File whiteout: delete named path
            let dir = path.parent().map(|p| target_path.join(p)).unwrap_or_else(|| target_path.to_path_buf());
            let real = dir.join(real_name);
            if real.is_dir() { std::fs::remove_dir_all(&real).ok(); }
            else { std::fs::remove_file(&real).ok(); }
        } else {
            // Handle hardlinks: remove target if it exists to avoid "File exists" error
            if entry.header().entry_type().is_hard_link() {
                let dst = target_path.join(&path);
                if dst.exists() {
                    let _ = std::fs::remove_file(&dst);
                }
            }
            entry.unpack_in(target)
                .with_context(|| format!("Failed to unpack {:?} in layer {}", path, index))?;
        }
    }
    Ok(())
}
