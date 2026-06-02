//! Bootstrap ClusterRoles and admin binding (R7).

use anyhow::{Context, Result};
use tracing::info;

use crate::store::{AnyResource, StoreBackend};
use crate::types::{
    ClusterRole, ClusterRoleBinding, ObjectMeta, PolicyRule, RoleRef, Subject,
};

fn wildcard_rule(verbs: &[&str]) -> PolicyRule {
    PolicyRule {
        api_groups: vec!["*".into()],
        resources: vec!["*".into()],
        resource_names: vec![],
        verbs: verbs.iter().map(|v| (*v).to_string()).collect(),
    }
}

async fn cluster_role_exists(store: &dyn StoreBackend, name: &str) -> bool {
    store
        .get_by_kind("ClusterRole")
        .await
        .iter()
        .any(|t| {
            matches!(&t.resource, AnyResource::ClusterRole(r) if r.metadata.name.as_deref() == Some(name))
        })
}

async fn cluster_role_binding_exists(store: &dyn StoreBackend, name: &str) -> bool {
    store
        .get_by_kind("ClusterRoleBinding")
        .await
        .iter()
        .any(|t| {
            matches!(
                &t.resource,
                AnyResource::ClusterRoleBinding(b) if b.metadata.name.as_deref() == Some(name)
            )
        })
}

/// Seed cluster-admin / view / edit and bind `User/admin` to cluster-admin.
pub async fn ensure_bootstrap_rbac(store: &dyn StoreBackend) -> Result<()> {
    if !cluster_role_exists(store, "cluster-admin").await {
        let role = ClusterRole {
            api_version: "rbac.authorization.k8s.io/v1".into(),
            kind: "ClusterRole".into(),
            metadata: ObjectMeta {
                name: Some("cluster-admin".into()),
                labels: Some({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("z8s.io/bootstrapped".into(), "true".into());
                    m
                }),
                ..Default::default()
            },
            rules: vec![wildcard_rule(&["*"])],
        };
        store
            .apply(AnyResource::ClusterRole(role))
            .await
            .context("apply cluster-admin ClusterRole")?;
        info!("Bootstrapped ClusterRole cluster-admin");
    }

    if !cluster_role_exists(store, "view").await {
        let role = ClusterRole {
            api_version: "rbac.authorization.k8s.io/v1".into(),
            kind: "ClusterRole".into(),
            metadata: ObjectMeta {
                name: Some("view".into()),
                ..Default::default()
            },
            rules: vec![wildcard_rule(&["get", "list", "watch"])],
        };
        store.apply(AnyResource::ClusterRole(role)).await?;
        info!("Bootstrapped ClusterRole view");
    }

    if !cluster_role_exists(store, "edit").await {
        let role = ClusterRole {
            api_version: "rbac.authorization.k8s.io/v1".into(),
            kind: "ClusterRole".into(),
            metadata: ObjectMeta {
                name: Some("edit".into()),
                ..Default::default()
            },
            rules: vec![
                wildcard_rule(&["get", "list", "watch"]),
                wildcard_rule(&["create", "update", "patch", "delete"]),
            ],
        };
        store.apply(AnyResource::ClusterRole(role)).await?;
        info!("Bootstrapped ClusterRole edit");
    }

    if !cluster_role_binding_exists(store, "cluster-admin").await {
        let crb = ClusterRoleBinding {
            api_version: "rbac.authorization.k8s.io/v1".into(),
            kind: "ClusterRoleBinding".into(),
            metadata: ObjectMeta {
                name: Some("cluster-admin".into()),
                ..Default::default()
            },
            subjects: vec![Subject {
                kind: "User".into(),
                namespace: String::new(),
                name: "admin".into(),
            }],
            role_ref: RoleRef {
                api_group: "rbac.authorization.k8s.io".into(),
                kind: "ClusterRole".into(),
                name: "cluster-admin".into(),
            },
        };
        store
            .apply(AnyResource::ClusterRoleBinding(crb))
            .await
            .context("apply cluster-admin ClusterRoleBinding")?;
        info!("Bootstrapped ClusterRoleBinding cluster-admin → User/admin");
    }

    Ok(())
}
