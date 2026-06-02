//! ServiceAccount token issuance and pod secret mounts (R4).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tracing::debug;

use crate::cri::spec::ResolvedVolume;
use crate::cri::volumes;
use crate::types::Pod;

pub const SA_CONTAINER_PATH: &str = "/var/run/secrets/z8s.io/serviceaccount";

#[derive(Debug, Clone)]
pub struct SaIdentity {
    pub namespace: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ServiceAccountMount {
    pub host_dir: String,
}

pub struct TokenRegistry {
    by_token: RwLock<HashMap<String, SaIdentity>>,
    by_pod: RwLock<HashMap<String, String>>,
}

impl TokenRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            by_token: RwLock::new(HashMap::new()),
            by_pod: RwLock::new(HashMap::new()),
        })
    }

    pub async fn resolve_user(&self, bearer_token: &str) -> Option<String> {
        let map = self.by_token.read().await;
        map.get(bearer_token)
            .map(|id| format!("system:serviceaccount:{}:{}", id.namespace, id.name))
    }

    /// Issue a fresh token for a pod, materialize secret files, return mount info.
    pub async fn prepare_pod_mount(
        &self,
        pod: &Pod,
        pod_uid: &str,
    ) -> Result<Option<ServiceAccountMount>> {
        let spec = match pod.spec.as_ref() {
            Some(s) => s,
            None => return Ok(None),
        };
        if spec.automount_service_account_token == Some(false) {
            return Ok(None);
        }
        let namespace = pod.metadata.namespace.as_deref().unwrap_or("default");
        let sa_name = spec
            .service_account_name
            .as_deref()
            .or(spec.service_account.as_deref())
            .unwrap_or("default");

        self.revoke_for_pod(pod_uid).await;

        let token = random_token();
        let host_dir = host_dir_for_pod(pod_uid);
        materialize_secret_dir(&host_dir, &token, namespace, sa_name)?;

        {
            let mut by_pod = self.by_pod.write().await;
            by_pod.insert(pod_uid.to_string(), token.clone());
        }
        {
            let mut by_token = self.by_token.write().await;
            by_token.insert(
                token,
                SaIdentity {
                    namespace: namespace.to_string(),
                    name: sa_name.to_string(),
                },
            );
        }

        debug!(
            "Issued ServiceAccount token for pod {} (ns={}, sa={})",
            pod_uid, namespace, sa_name
        );
        Ok(Some(ServiceAccountMount { host_dir }))
    }

    pub async fn revoke_for_pod(&self, pod_uid: &str) {
        let token = {
            let mut by_pod = self.by_pod.write().await;
            by_pod.remove(pod_uid)
        };
        if let Some(token) = token {
            self.by_token.write().await.remove(&token);
        }
        let dir = host_dir_for_pod(pod_uid);
        if Path::new(&dir).exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

pub fn host_dir_for_pod(pod_uid: &str) -> String {
    let safe_uid = pod_uid.replace('/', "_");
    format!(
        "{}/serviceaccounts/{}",
        volumes::base_dir(),
        safe_uid
    )
}

pub fn append_service_account_volumes(
    volumes: &mut Vec<ResolvedVolume>,
    mount: &ServiceAccountMount,
) {
    let base = mount.host_dir.clone();
    for (file, container_path) in [
        ("token", format!("{SA_CONTAINER_PATH}/token")),
        ("namespace", format!("{SA_CONTAINER_PATH}/namespace")),
        ("name", format!("{SA_CONTAINER_PATH}/name")),
    ] {
        volumes.push(ResolvedVolume {
            host_path: format!("{base}/{file}"),
            container_path,
            read_only: true,
        });
    }
}

fn materialize_secret_dir(dir: &str, token: &str, namespace: &str, name: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create SA dir {dir}"))?;
    use std::os::unix::fs::PermissionsExt;
    for (file, contents) in [
        ("token", token),
        ("namespace", namespace),
        ("name", name),
    ] {
        let path = Path::new(dir).join(file);
        std::fs::write(&path, contents)
            .with_context(|| format!("write SA file {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).ok();
    }
    Ok(())
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("getrandom");
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(buf)
}
