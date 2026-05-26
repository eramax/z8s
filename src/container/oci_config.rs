//! OCI image Entrypoint/Cmd persisted at pull time and merged with Kubernetes overrides.

use k8s_openapi::api::core::v1::Container;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const OCI_CONFIG_FILE: &str = ".z8s-oci-config.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedImageConfig {
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
}

pub fn save_image_config(dir: &str, entrypoint: Option<Vec<String>>, cmd: Option<Vec<String>>) {
    let path = Path::new(dir).join(OCI_CONFIG_FILE);
    let saved = SavedImageConfig { entrypoint, cmd };
    if let Ok(json) = serde_json::to_string(&saved) {
        std::fs::write(path, json).ok();
    }
}

pub fn read_image_config(rootfs_path: &str) -> SavedImageConfig {
    let path = Path::new(rootfs_path).join(OCI_CONFIG_FILE);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<SavedImageConfig>(&raw) {
            if cfg.entrypoint.is_some() || cfg.cmd.is_some() {
                return cfg;
            }
        }
    }
    guess_image_config(rootfs_path)
}

/// Fallback when `.z8s-oci-config.json` is missing (images cached before OCI config was stored).
pub fn guess_image_config(rootfs_path: &str) -> SavedImageConfig {
    let root = Path::new(rootfs_path);
    for candidate in ["/whoami", "/http-echo", "/nginx", "/bin/sh"] {
        let rel = candidate.trim_start_matches('/');
        if root.join(rel).exists() {
            return SavedImageConfig {
                entrypoint: Some(vec![candidate.to_string()]),
                cmd: None,
            };
        }
    }
    SavedImageConfig::default()
}

/// Resolve argv per Kubernetes rules (command/args override image Entrypoint/Cmd).
pub fn resolve_argv(container: &Container, rootfs_path: &str) -> (String, Vec<String>) {
    let img = read_image_config(rootfs_path);
    let image_ep = img.entrypoint.unwrap_or_default();
    let image_cmd = img.cmd.unwrap_or_default();

    let k8s_cmd = container.command.clone().unwrap_or_default();
    let k8s_args = container.args.clone().unwrap_or_default();

    let (program, mut args) = match (k8s_cmd.is_empty(), k8s_args.is_empty()) {
        (true, true) => {
            let ep = if image_ep.is_empty() {
                vec!["/bin/sh".to_string()]
            } else {
                image_ep
            };
            let prog = ep[0].clone();
            let rest: Vec<String> = ep[1..]
                .iter()
                .chain(image_cmd.iter())
                .cloned()
                .collect();
            (prog, rest)
        }
        (false, true) => {
            let prog = k8s_cmd[0].clone();
            let rest = k8s_cmd[1..].to_vec();
            (prog, rest)
        }
        (true, false) => {
            let ep = if image_ep.is_empty() {
                vec!["/bin/sh".to_string()]
            } else {
                image_ep
            };
            let prog = ep[0].clone();
            let rest: Vec<String> = ep[1..].iter().chain(k8s_args.iter()).cloned().collect();
            (prog, rest)
        }
        (false, false) => {
            let prog = k8s_cmd[0].clone();
            let rest: Vec<String> = k8s_cmd[1..]
                .iter()
                .chain(k8s_args.iter())
                .cloned()
                .collect();
            (prog, rest)
        }
    };

    (program, args)
}
