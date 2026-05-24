use anyhow::{Context, Result};
use oci_distribution::client::{Client, ClientConfig, ImageLayer};
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use std::path::Path;
use tracing::{debug, info};

const ACCEPTED_LAYER_TYPES: &[&str] = &[
    "application/vnd.docker.image.rootfs.diff.tar.gzip",
    "application/vnd.docker.image.rootfs.diff.tar",
    "application/vnd.oci.image.layer.v1.tar",
    "application/vnd.oci.image.layer.v1.tar+gzip",
    "application/vnd.oci.image.layer.v1.tar+zstd",
];

const Z8S_IMAGE_CACHE: &str = "/var/lib/z8s/images";
const Z8S_ROOTFS: &str = "/var/lib/z8s/rootfs";

pub struct ImageManager {
    client: Client,
    cache_dir: String,
    rootfs_dir: String,
}

impl ImageManager {
    pub fn new() -> Result<Self> {
        let client = Client::new(ClientConfig::default());

        let cache_dir = Z8S_IMAGE_CACHE.to_string();
        let rootfs_dir = Z8S_ROOTFS.to_string();

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

    pub async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String> {
        let container_rootfs = format!("{}/{}", self.rootfs_dir, container_id);

        if container_rootfs.as_str() == "/" {
            anyhow::bail!("Refusing to unpack to root filesystem");
        }

        // Reuse cached rootfs if it exists for this image ref
        let meta_path = format!("{}/.z8s-image-ref", container_rootfs);
        if Path::new(&container_rootfs).exists() && Path::new(&meta_path).exists() {
            if let Ok(cached_ref) = std::fs::read_to_string(&meta_path) {
                if cached_ref.trim() == image_ref {
                    info!("Reusing cached rootfs for {} at {}", image_ref, container_rootfs);
                    return Ok(container_rootfs);
                }
            }
        }

        let reference: Reference = image_ref.parse().context("Invalid image reference")?;
        let auth = RegistryAuth::Anonymous;

        info!("Pulling and unpacking image: {} -> {}", image_ref, container_id);
        info!("Connecting to registry for {}", image_ref);
        info!("Pulling {:?} with auth {:?}", &reference, &auth);
        let image_data = match self.client.pull(&reference, &auth, ACCEPTED_LAYER_TYPES.to_vec()).await {
            Ok(data) => data,
            Err(e) => {
                anyhow::bail!(
                    "Failed to pull image '{}': {:#}. Check network connectivity, image name, and registry auth.",
                    image_ref, e
                );
            }
        };

        if Path::new(&container_rootfs).exists() {
            std::fs::remove_dir_all(&container_rootfs)?;
        }
        std::fs::create_dir_all(&container_rootfs)?;

        let layers = &image_data.layers;
        info!("Unpacking {} layers for {}", layers.len(), image_ref);

        for (i, layer) in layers.iter().enumerate() {
            self.unpack_layer(layer, &container_rootfs, i)?;
        }

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

    pub fn rootfs_path(&self, container_id: &str) -> String {
        format!("{}/{}", self.rootfs_dir, container_id)
    }
}
