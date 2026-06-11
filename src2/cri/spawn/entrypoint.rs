//! Resolve container entrypoint, args, and working directory from OCI image config + Pod spec.

use crate::cri::oci::{self, SavedImageConfig};
use crate::cri::spec::ContainerConfig;

pub struct ResolvedEntrypoint {
    pub entrypoint: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
}

/// Merge Kubernetes `command`/`args` with OCI image metadata on disk.
pub fn resolve_entrypoint(cfg: &ContainerConfig, rootfs_path: &str) -> ResolvedEntrypoint {
    let oci = if cfg.entrypoint.is_empty() || cfg.working_dir.is_none() {
        oci::read_image_config(rootfs_path)
    } else {
        SavedImageConfig::default()
    };
    let working_dir = cfg.working_dir.clone().or(oci.working_dir.clone());
    let (entrypoint, args) = if cfg.entrypoint.is_empty() {
        let oci_ep = oci.entrypoint.as_ref().and_then(|v| v.first()).cloned();
        let oci_cmd = oci.cmd.unwrap_or_default();
        match (oci_ep, cfg.args.is_empty()) {
            (Some(ep), true) => {
                let mut args = oci_cmd;
                (ep, args)
            }
            (Some(ep), false) => (ep, cfg.args.clone()),
            (None, _) if !oci_cmd.is_empty() => {
                let prog = oci_cmd[0].clone();
                let args: Vec<String> = oci_cmd[1..].to_vec();
                (prog, args)
            }
            (None, false) if !cfg.args.is_empty() => {
                (cfg.args[0].clone(), cfg.args[1..].to_vec())
            }
            (None, _) => (cfg.entrypoint.clone(), cfg.args.clone()),
        }
    } else {
        (cfg.entrypoint.clone(), cfg.args.clone())
    };
    ResolvedEntrypoint {
        entrypoint,
        args,
        working_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cri::oci::save_image_config;

    fn container_with_args(args: Vec<&str>) -> ContainerConfig {
        ContainerConfig {
            container_id: "p-c".into(),
            container_name: "c".into(),
            image: "img".into(),
            rootfs_path: String::new(),
            is_native: false,
            entrypoint: String::new(),
            args: args.into_iter().map(String::from).collect(),
            working_dir: None,
            env: vec![],
            volumes: vec![],
            memory_limit_bytes: None,
            memory_low_bytes: None,
            cpu_quota: None,
            cpu_period: None,
            run_as_user: None,
            run_as_group: None,
            privileged: false,
            cap_profile: None,
            extra_capabilities: vec![],
            isolated_net: true,
            published_ports: Default::default(),
            probes: vec![],
        }
    }

    fn test_rootfs_dir() -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "z8s-entrypoint-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        (dir.clone(), dir.to_string_lossy().into_owned())
    }

    #[test]
    fn uses_oci_entrypoint_when_command_empty() {
        let (_dir, path) = test_rootfs_dir();
        save_image_config(
            &path,
            Some(vec!["/bin/sh".into()]),
            Some(vec!["-c".into(), "echo hi".into()]),
            None,
            None,
        );
        let cfg = container_with_args(vec![]);
        let r = resolve_entrypoint(&cfg, &path);
        assert_eq!(r.entrypoint, "/bin/sh");
        assert_eq!(r.args, vec!["-c", "echo hi"]);
        let _ = std::fs::remove_dir_all(_dir);
    }

    #[test]
    fn pod_args_override_oci_cmd() {
        let (_dir, path) = test_rootfs_dir();
        save_image_config(
            &path,
            Some(vec!["/bin/sh".into()]),
            Some(vec!["ignored".into()]),
            None,
            None,
        );
        let cfg = container_with_args(vec!["--help"]);
        let r = resolve_entrypoint(&cfg, &path);
        assert_eq!(r.entrypoint, "/bin/sh");
        assert_eq!(r.args, vec!["--help"]);
        let _ = std::fs::remove_dir_all(_dir);
    }
}
