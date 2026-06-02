//! Bootstrap admin ServiceAccount + ClusterRoleBinding (R7).
//!
//! Creates `admin` SA in `default` namespace and binds it to `cluster-admin`.
//! Persists the SA token to `<data_dir>/admin-token` for CLI retrieval.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::info;

use crate::store::{AnyResource, StoreBackend};
use crate::types::{ClusterRoleBinding, ObjectMeta, RoleRef, ServiceAccount, Subject};

const ADMIN_SA_NAME: &str = "admin";
const ADMIN_NS: &str = "default";

/// Ensure admin ServiceAccount + ClusterRoleBinding exist.
/// Returns the admin bearer token string.
pub async fn ensure_admin_sa(store: &dyn StoreBackend, data_dir: &str) -> Result<String> {
    let token_path = Path::new(data_dir).join("admin-token");

    // 1. Create admin ServiceAccount if missing
    let sa_exists = store.get_by_kind("ServiceAccount").await.iter().any(|t| {
        matches!(
            &t.resource,
            AnyResource::ServiceAccount(sa)
                if sa.metadata.namespace.as_deref() == Some(ADMIN_NS)
                    && sa.metadata.name.as_deref() == Some(ADMIN_SA_NAME)
        )
    });

    if !sa_exists {
        let sa = ServiceAccount {
            api_version: "v1".into(),
            kind: "ServiceAccount".into(),
            metadata: ObjectMeta {
                name: Some(ADMIN_SA_NAME.into()),
                namespace: Some(ADMIN_NS.into()),
                ..Default::default()
            },
            secrets: None,
        };
        store
            .apply(AnyResource::ServiceAccount(sa))
            .await
            .context("apply admin ServiceAccount")?;
        info!("Bootstrapped ServiceAccount {}/{}", ADMIN_NS, ADMIN_SA_NAME);
    }

    // 2. Create ClusterRoleBinding if missing
    let crb_exists = store.get_by_kind("ClusterRoleBinding").await.iter().any(|t| {
        matches!(
            &t.resource,
            AnyResource::ClusterRoleBinding(crb)
                if crb.metadata.name.as_deref() == Some("admin-cluster-admin")
        )
    });

    if !crb_exists {
        let crb = ClusterRoleBinding {
            api_version: "rbac.authorization.k8s.io/v1".into(),
            kind: "ClusterRoleBinding".into(),
            metadata: ObjectMeta {
                name: Some("admin-cluster-admin".into()),
                labels: Some({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("z8s.io/bootstrapped".into(), "true".into());
                    m
                }),
                ..Default::default()
            },
            subjects: vec![
                // Bind both the ServiceAccount identity and the "admin" user
                // (for token fallback in extract_user)
                Subject {
                    kind: "ServiceAccount".into(),
                    namespace: ADMIN_NS.into(),
                    name: ADMIN_SA_NAME.into(),
                },
                Subject {
                    kind: "User".into(),
                    namespace: String::new(),
                    name: ADMIN_SA_NAME.into(),
                },
            ],
            role_ref: RoleRef {
                api_group: "rbac.authorization.k8s.io".into(),
                kind: "ClusterRole".into(),
                name: "cluster-admin".into(),
            },
        };
        store
            .apply(AnyResource::ClusterRoleBinding(crb))
            .await
            .context("apply admin ClusterRoleBinding")?;
        info!(
            "Bootstrapped ClusterRoleBinding admin-cluster-admin → ServiceAccount {}/{}",
            ADMIN_NS, ADMIN_SA_NAME
        );
    }

    // 3. Load or generate persistent token
    let token = if token_path.exists() {
        let t = std::fs::read_to_string(&token_path)
            .context("read admin-token")?
            .trim()
            .to_string();
        t
    } else {
        let t = generate_token();
        std::fs::create_dir_all(data_dir).context("create data dir")?;
        std::fs::write(&token_path, format!("{}\n", t))
            .with_context(|| format!("write {}", token_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        info!("Admin token written to {}", token_path.display());
        t
    };

    Ok(token)
}

/// Read the admin token from disk (for `z8s set kubeconfig`).
pub fn read_admin_token(data_dir: &str) -> Option<String> {
    let token_path = Path::new(data_dir).join("admin-token");
    std::fs::read_to_string(&token_path)
        .ok()
        .map(|t| t.trim().to_string())
}

fn generate_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("getrandom");
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}
