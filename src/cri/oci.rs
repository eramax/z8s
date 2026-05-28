//! OCI image Entrypoint/Cmd persisted at pull time and merged with Kubernetes overrides.

use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub const OCI_CONFIG_FILE: &str = ".z8s-oci-config.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedImageConfig {
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub env: Option<Vec<String>>,
}

pub fn save_image_config(
    dir: &str,
    entrypoint: Option<Vec<String>>,
    cmd: Option<Vec<String>>,
    env: Option<Vec<String>>,
) {
    let path = Path::new(dir).join(OCI_CONFIG_FILE);
    let saved = SavedImageConfig { entrypoint, cmd, env };
    if let Ok(json) = serde_json::to_string(&saved) {
        std::fs::write(path, json).ok();
    }
}

pub fn read_image_config(rootfs_path: &str) -> SavedImageConfig {
    let path = Path::new(rootfs_path).join(OCI_CONFIG_FILE);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<SavedImageConfig>(&raw) {
            let has_ep = cfg
                .entrypoint
                .as_ref()
                .is_some_and(|ep| !ep.is_empty());
            let has_cmd = cfg.cmd.as_ref().is_some_and(|c| !c.is_empty());
            if has_ep || has_cmd {
                return cfg;
            }
        }
    }
    guess_image_config(rootfs_path)
}

/// Fallback when OCI Entrypoint/Cmd were not stored (legacy cache or empty image config).
pub fn guess_image_config(rootfs_path: &str) -> SavedImageConfig {
    let root = Path::new(rootfs_path);
    for rel in [
        "docker-entrypoint.sh",
        "usr/local/bin/docker-entrypoint.sh",
        "usr/bin/docker-entrypoint.sh",
        "entrypoint.sh",
        "bin/sh",
    ] {
        let path = root.join(rel);
        if path.is_file() {
            let ep = format!("/{}", rel.trim_start_matches('/'));
            return SavedImageConfig {
                entrypoint: Some(vec![ep]),
                cmd: None,
                env: None,
            };
        }
    }
    // Single executable at image root (common for minimal / statically linked images).
    if let Ok(entries) = std::fs::read_dir(root) {
        let mut exes: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.parent()? != root {
                    return None;
                }
                let meta = e.metadata().ok()?;
                if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
                    return None;
                }
                Some(format!("/{}", e.file_name().to_string_lossy()))
            })
            .collect();
        exes.sort();
        if exes.len() == 1 {
            return SavedImageConfig {
                entrypoint: Some(exes),
                cmd: None,
                env: None,
            };
        }
    }
    SavedImageConfig::default()
}
