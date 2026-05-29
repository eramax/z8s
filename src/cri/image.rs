use anyhow::{Context, Result};
use oci_distribution::client::{Client, ClientConfig, ImageLayer};
use oci_distribution::config::ConfigFile;
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info};

use crate::cri::oci::{save_image_config, OCI_CONFIG_FILE};

const ACCEPTED_LAYER_TYPES: &[&str] = &[
    "application/vnd.docker.image.rootfs.diff.tar.gzip",
    "application/vnd.docker.image.rootfs.diff.tar",
    "application/vnd.oci.image.layer.v1.tar",
    "application/vnd.oci.image.layer.v1.tar+gzip",
    "application/vnd.oci.image.layer.v1.tar+zstd",
];

fn z8s_base_dir() -> String {
    if let Some(dir) = &crate::config::get().data_dir {
        return dir.clone();
    }
    if nix::unistd::Uid::effective().is_root() {
        "/var/lib/z8s".to_string()
    } else {
        format!("{}/.local/share/z8s", std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
    }
}

/// Guard that removes a lock file on drop.
struct LockGuard(String);
impl Drop for LockGuard {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

pub struct ImageManager {
    client: Client,
    cache_dir: String,
    rootfs_dir: String,
}

impl ImageManager {
    pub fn new() -> Result<Self> {
        let client = Client::new(ClientConfig::default());

        let base = z8s_base_dir();
        let cache_dir = format!("{}/images", base);
        let rootfs_dir = format!("{}/rootfs", base);

        std::fs::create_dir_all(&cache_dir)
            .context("Failed to create image cache directory")?;
        std::fs::create_dir_all(&rootfs_dir)
            .context("Failed to create rootfs directory")?;

        Ok(Self {
            client,
            cache_dir,
            rootfs_dir,
        })
    }

    fn image_cache_path(&self, image_ref: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        image_ref.hash(&mut hasher);
        format!("{}/{:016x}", self.cache_dir, hasher.finish())
    }

    fn copy_oci_config(cache_path: &str, container_rootfs: &str) {
        let src = Path::new(cache_path).join(OCI_CONFIG_FILE);
        let dst = Path::new(container_rootfs).join(OCI_CONFIG_FILE);
        if src.exists() {
            std::fs::copy(src, dst).ok();
        }
    }

    /// Recursively copy a directory tree without spawning an external process.
    /// Preserves permissions and symlinks; skips special files (devices/fifos)
    /// which require root to create and are not needed for container rootfs copies.
    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        if let Ok(m) = std::fs::symlink_metadata(src) {
            std::fs::set_permissions(dst, m.permissions()).ok();
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_child = entry.path();
            let dst_child = dst.join(entry.file_name());
            let meta = std::fs::symlink_metadata(&src_child)?;
            let ft = meta.file_type();
            if ft.is_symlink() {
                let target = std::fs::read_link(&src_child)?;
                if dst_child.exists() || std::fs::symlink_metadata(&dst_child).is_ok() {
                    if dst_child.is_dir() {
                        std::fs::remove_dir_all(&dst_child).ok();
                    } else {
                        std::fs::remove_file(&dst_child).ok();
                    }
                }
                std::os::unix::fs::symlink(&target, &dst_child)?;
            } else if ft.is_dir() {
                Self::copy_dir(&src_child, &dst_child)?;
            } else if ft.is_file() {
                std::fs::copy(&src_child, &dst_child)?;
                std::fs::set_permissions(&dst_child, meta.permissions()).ok();
            }
        }
        Ok(())
    }

    pub async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String> {
        let container_rootfs = format!("{}/{}", self.rootfs_dir, container_id);

        if container_rootfs.as_str() == "/" {
            anyhow::bail!("Refusing to unpack to root filesystem");
        }

        // Check if this exact container already has the rootfs cached
        let meta_path = format!("{}/.z8s-image-ref", container_rootfs);
        if Path::new(&container_rootfs).exists() && Path::new(&meta_path).exists() {
            if let Ok(cached_ref) = std::fs::read_to_string(&meta_path) {
                if cached_ref.trim() == image_ref {
                    info!("Reusing cached rootfs for {} at {}", image_ref, container_rootfs);
                    return Ok(container_rootfs);
                }
            }
        }

        // Check if we have a shared image cache for this ref
        let cache_path = self.image_cache_path(image_ref);
        let cache_meta = format!("{}/.z8s-image-ref", cache_path);
        if Path::new(&cache_path).exists() && Path::new(&cache_meta).exists() {
            if let Ok(cached_ref) = std::fs::read_to_string(&cache_meta) {
                if cached_ref.trim() == image_ref {
                    info!("Copying cached rootfs for {} to {}", image_ref, container_rootfs);
                    return Self::copy_cache_to_container(&cache_path, &container_rootfs, &meta_path, image_ref);
                }
            }
        }

        let reference: Reference = image_ref.parse().context("Invalid image reference")?;
        let auth = RegistryAuth::Anonymous;

        info!("Pulling and unpacking image: {} -> {}", image_ref, container_id);
        let image_data = match self.client.pull(&reference, &auth, ACCEPTED_LAYER_TYPES.to_vec()).await {
            Ok(data) => data,
            Err(e) => {
                anyhow::bail!(
                    "Failed to pull image '{}': {:#}. Check network connectivity, image name, and registry auth.",
                    image_ref, e
                );
            }
        };

        // Apply a file-based lock so concurrent pulls of the same image
        // don't both extract to cache_path simultaneously.
        let lock_path = format!("{cache_path}.lock");
        loop {
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = f.write_fmt(format_args!("{}", std::process::id()));
                    drop(f);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Another thread/process is extracting; wait and retry
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    // After acquiring lock, recheck cache (it may be ready now)
                    if Path::new(&cache_meta).exists() {
                        if let Ok(cached_ref) = std::fs::read_to_string(&cache_meta) {
                            if cached_ref.trim() == image_ref {
                                info!("Cache ready after waiting for concurrent pull of {}", image_ref);
                                let _ = std::fs::remove_file(&lock_path);
                                return Self::copy_cache_to_container(&cache_path, &container_rootfs, &meta_path, image_ref);
                            }
                        }
                    }
                    continue;
                }
                Err(e) => anyhow::bail!("Failed to create image lock {}: {}", lock_path, e),
            }
        }

        let _lock_guard = LockGuard(lock_path.clone());

        // Double-check after acquiring lock: another thread may have populated the cache
        // while we were waiting for the lock or pulling.
        if Path::new(&cache_meta).exists() {
            if let Ok(cached_ref) = std::fs::read_to_string(&cache_meta) {
                if cached_ref.trim() == image_ref {
                    info!("Cache populated by concurrent pull for {}", image_ref);
                    return Self::copy_cache_to_container(&cache_path, &container_rootfs, &meta_path, image_ref);
                }
            }
        }

        // Unpack to shared cache first
        if Path::new(&cache_path).exists() {
            std::fs::remove_dir_all(&cache_path)?;
        }
        std::fs::create_dir_all(&cache_path)?;

        // Fix layer order: oci_distribution's buffer_unordered collects layers
        // in arbitrary (completion) order, not manifest order. Restore order so
        // that base layers are extracted before dependent layers.
        let mut image_data = image_data;
        if let Some(manifest) = &image_data.manifest {
            let mut by_digest: std::collections::HashMap<String, ImageLayer> = std::collections::HashMap::new();
            for layer in image_data.layers {
                by_digest.insert(layer.sha256_digest(), layer);
            }
            let mut ordered = Vec::with_capacity(manifest.layers.len());
            for desc in &manifest.layers {
                if let Some(layer) = by_digest.remove(&desc.digest) {
                    ordered.push(layer);
                }
            }
            // Append any extra layers not in the manifest (should not happen)
            ordered.extend(by_digest.into_values());
            image_data.layers = ordered;
        }

        let layers = &image_data.layers;
        info!("Unpacking {} layers for {}", layers.len(), image_ref);

        // Save the OCI entrypoint/cmd/env from the registry config
        // (must be done before guess_image_config fallback below)
        let config_file: ConfigFile = image_data
            .config
            .clone()
            .try_into()
            .context("Failed to parse OCI image config")?;
        let (image_ep, image_cmd, image_env, image_wd) = config_file
            .config
            .as_ref()
            .map(|c| (c.entrypoint.clone(), c.cmd.clone(), c.env.clone(), c.working_dir.clone()))
            .unwrap_or((None, None, None, None));
        let has_ep = image_ep.as_ref().is_some_and(|ep| !ep.is_empty());
        let has_cmd = image_cmd.as_ref().is_some_and(|c| !c.is_empty());
        let needs_guess = !has_ep && !has_cmd;
        save_image_config(&cache_path, image_ep, image_cmd, image_env, image_wd);

        for (i, layer) in layers.iter().enumerate() {
            self.unpack_layer(layer, &cache_path, i)
                .with_context(|| format!("Failed to unpack layer {}/{} ({})", i + 1, layers.len(), layer.media_type))?;
        }
        // Backfill OCI config on the cache (guess if needed)
        let oci_cfg = Path::new(&cache_path).join(OCI_CONFIG_FILE);
        let needs_guess = !oci_cfg.exists()
            || {
                let cfg = crate::cri::oci::read_image_config(&cache_path);
                cfg.entrypoint.as_ref().is_none_or(|ep| ep.is_empty())
                    && cfg.cmd.as_ref().is_none_or(|c| c.is_empty())
            };
        if needs_guess {
            let guessed = crate::cri::oci::guess_image_config(&cache_path);
            save_image_config(
                &cache_path,
                guessed.entrypoint.clone(),
                guessed.cmd.clone(),
                guessed.env.clone(),
                None,
            );
        }
        std::fs::write(&cache_meta, image_ref)
            .context("Failed to write cache metadata")?;

        Self::copy_cache_to_container(&cache_path, &container_rootfs, &meta_path, image_ref)
            .context("Failed to copy image cache to container rootfs")
    }

    fn copy_cache_to_container(cache_path: &str, container_rootfs: &str, meta_path: &str, image_ref: &str) -> Result<String> {
        if Path::new(container_rootfs).exists() {
            std::fs::remove_dir_all(container_rootfs)?;
        }
        Self::copy_dir(Path::new(cache_path), Path::new(container_rootfs))?;
        Self::copy_oci_config(cache_path, container_rootfs);
        Self::backfill_oci_config(cache_path, container_rootfs);
        std::fs::write(meta_path, image_ref)?;
        Ok(container_rootfs.to_string())
    }

    fn backfill_oci_config(cache_path: &str, container_rootfs: &str) {
        let oci_cfg = Path::new(cache_path).join(OCI_CONFIG_FILE);
        let needs_guess = !oci_cfg.exists()
            || {
                let cfg = crate::cri::oci::read_image_config(cache_path);
                cfg.entrypoint.as_ref().is_none_or(|ep| ep.is_empty())
                    && cfg.cmd.as_ref().is_none_or(|c| c.is_empty())
            };
        if needs_guess {
            let guessed = crate::cri::oci::guess_image_config(cache_path);
            save_image_config(
                cache_path,
                guessed.entrypoint.clone(),
                guessed.cmd.clone(),
                guessed.env.clone(),
                None,
            );
            Self::copy_oci_config(cache_path, container_rootfs);
        }
    }

    fn unpack_layer(&self, layer: &ImageLayer, target: &str, index: usize) -> Result<()> {
        use flate2::read::GzDecoder;
        use std::io::Read;
        use tar::Archive;

        let layer_data = &layer.data;
        debug!("Unpacking layer {} ({} bytes)", index, layer_data.len());

        let mut decompressed: Box<dyn Read> = if layer_data.len() > 2
            && layer_data[0] == 0x1f
            && layer_data[1] == 0x8b
        {
            Box::new(GzDecoder::new(layer_data.as_slice()))
        } else {
            Box::new(layer_data.as_slice())
        };

        let mut archive = Archive::new(&mut decompressed);
        archive.set_overwrite(true);
        archive.set_preserve_mtime(true);
        archive.set_preserve_permissions(true);

        let target_path = Path::new(target);

        // Iterate entry-by-entry so we can handle OCI whiteout files.
        // Whiteout files encode layer deletions from the overlayfs model:
        //   .wh.<name>        — delete <name> (file or directory) from lower layers
        //   .wh..wh..opq      — opaque: delete all non-whiteout children in this dir
        for entry_result in archive.entries().context(format!("Failed to read layer {} archive", index))? {
            let mut entry = entry_result
                .context(format!("Corrupt entry in layer {}", index))?;

            let path = entry.path()
                .context("Entry has non-UTF8 path")?
                .into_owned();

            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            if filename == ".wh..wh..opq" {
                // Opaque whiteout: clear the directory (except other .wh. markers)
                let dir = path.parent().map(|p| target_path.join(p)).unwrap_or_else(|| target_path.to_path_buf());
                if let Ok(children) = std::fs::read_dir(&dir) {
                    for child in children.flatten() {
                        if !child.file_name().to_string_lossy().starts_with(".wh.") {
                            let cp = child.path();
                            if cp.is_dir() {
                                std::fs::remove_dir_all(&cp).ok();
                            } else {
                                std::fs::remove_file(&cp).ok();
                            }
                        }
                    }
                }
            } else if let Some(real_name) = filename.strip_prefix(".wh.") {
                // File/dir whiteout: delete the named path from lower layers
                let dir = path.parent().map(|p| target_path.join(p)).unwrap_or_else(|| target_path.to_path_buf());
                let real = dir.join(real_name);
                if real.is_dir() {
                    std::fs::remove_dir_all(&real).ok();
                } else {
                    std::fs::remove_file(&real).ok();
                }
            } else {
                // Normal entry: extract relative to target (strips leading '/' for safety)
                entry.unpack_in(target)
                    .with_context(|| format!("Failed to unpack {:?} in layer {}", path, index))?;
            }
        }

        Ok(())
    }

}
