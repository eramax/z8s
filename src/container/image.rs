use anyhow::{Context, Result};
use oci_distribution::client::{Client, ClientConfig, ImageLayer};
use oci_distribution::config::ConfigFile;
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use std::path::Path;
use tracing::{debug, info};

use crate::container::oci_config::{save_image_config, OCI_CONFIG_FILE};

const ACCEPTED_LAYER_TYPES: &[&str] = &[
    "application/vnd.docker.image.rootfs.diff.tar.gzip",
    "application/vnd.docker.image.rootfs.diff.tar",
    "application/vnd.oci.image.layer.v1.tar",
    "application/vnd.oci.image.layer.v1.tar+gzip",
    "application/vnd.oci.image.layer.v1.tar+zstd",
];

fn z8s_base_dir() -> String {
    if nix::unistd::Uid::effective().is_root() {
        "/var/lib/z8s".to_string()
    } else {
        format!("{}/.local/share/z8s", std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
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

    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        let status = std::process::Command::new("cp")
            .arg("-a")
            .arg(src.as_os_str())
            .arg(dst.as_os_str())
            .status()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        if !status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("cp -a failed with {}", status),
            ));
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
                    if Path::new(&container_rootfs).exists() {
                        std::fs::remove_dir_all(&container_rootfs)?;
                    }
                    Self::copy_dir(Path::new(&cache_path), Path::new(&container_rootfs))?;
                    Self::copy_oci_config(&cache_path, &container_rootfs);
                    // Backfill OCI config for caches created before we stored Entrypoint/Cmd
                    if !Path::new(&cache_path).join(OCI_CONFIG_FILE).exists() {
                        let guessed = crate::container::oci_config::guess_image_config(&cache_path);
                        save_image_config(
                            &cache_path,
                            guessed.entrypoint.clone(),
                            guessed.cmd.clone(),
                        );
                        Self::copy_oci_config(&cache_path, &container_rootfs);
                    }
                    std::fs::write(&meta_path, image_ref)?;
                    return Ok(container_rootfs);
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

        // Unpack to shared cache first
        if Path::new(&cache_path).exists() {
            std::fs::remove_dir_all(&cache_path)?;
        }
        std::fs::create_dir_all(&cache_path)?;

        let layers = &image_data.layers;
        info!("Unpacking {} layers for {}", layers.len(), image_ref);
        let config_file: ConfigFile = image_data
            .config
            .clone()
            .try_into()
            .context("Failed to parse OCI image config")?;
        let (image_ep, image_cmd) = config_file
            .config
            .as_ref()
            .map(|c| (c.entrypoint.clone(), c.cmd.clone()))
            .unwrap_or((None, None));
        save_image_config(&cache_path, image_ep, image_cmd);

        for (i, layer) in layers.iter().enumerate() {
            self.unpack_layer(layer, &cache_path, i)?;
        }
        std::fs::write(&cache_meta, image_ref)?;

        // Copy from cache to container-specific path
        if Path::new(&container_rootfs).exists() {
            std::fs::remove_dir_all(&container_rootfs)?;
        }
        Self::copy_dir(Path::new(&cache_path), Path::new(&container_rootfs))?;
        Self::copy_oci_config(&cache_path, &container_rootfs);
        std::fs::write(&meta_path, image_ref)?;

        info!("Image {} unpacked to {}", image_ref, container_rootfs);
        Ok(container_rootfs)
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

        archive
            .unpack(target)
            .context(format!("Failed to unpack layer {}", index))?;

        Ok(())
    }

}
