//! Multi-document apply authorization (R6).

use crate::store::{AnyResource, StoreBackend};
use crate::types::parse_manifest_yaml;

use super::{api_group_for_resource, authorize, AuthzRequest};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyAuthzFailure {
    pub kind: String,
    pub name: String,
    pub namespace: String,
    pub verb: String,
}

/// Parse request body as one or more manifests (YAML multi-doc or JSON value/array).
pub fn parse_apply_body(bytes: &[u8]) -> anyhow::Result<Vec<AnyResource>> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        let trimmed = text.trim();
        if !trimmed.is_empty()
            && (trimmed.contains("apiVersion:") || trimmed.starts_with("---"))
        {
            return parse_manifest_yaml(text);
        }
    }
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
        return json_value_to_resources(v);
    }
    if let Ok(y) = serde_yaml::from_slice::<serde_yaml::Value>(bytes) {
        return json_value_to_resources(serde_json::to_value(y)?);
    }
    anyhow::bail!("invalid apply body: expected YAML or JSON")
}

fn json_value_to_resources(v: serde_json::Value) -> anyhow::Result<Vec<AnyResource>> {
    match v {
        serde_json::Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                out.push(json_object_to_resource(item)?);
            }
            Ok(out)
        }
        obj @ serde_json::Value::Object(_) => Ok(vec![json_object_to_resource(obj)?]),
        _ => anyhow::bail!("expected JSON object or array"),
    }
}

fn json_object_to_resource(v: serde_json::Value) -> anyhow::Result<AnyResource> {
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing kind"))?
        .to_string();
    AnyResource::from_json_value(v, &kind)
}

pub fn rbac_plural(resource: &AnyResource) -> String {
    crate::api::catalog::by_kind(resource.kind())
        .map(|e| e.plural.to_string())
        .unwrap_or_else(|| {
            let lower = resource.kind().to_ascii_lowercase();
            if lower.ends_with('s') {
                lower
            } else {
                format!("{lower}s")
            }
        })
}

pub fn api_group_from_resource(resource: &AnyResource) -> String {
    let api_version = match resource {
        AnyResource::Pod(p) => &p.api_version,
        AnyResource::Deployment(d) => &d.api_version,
        AnyResource::Service(s) => &s.api_version,
        AnyResource::ConfigMap(c) => &c.api_version,
        AnyResource::Secret(s) => &s.api_version,
        AnyResource::Role(r) => &r.api_version,
        AnyResource::RoleBinding(r) => &r.api_version,
        AnyResource::ClusterRole(r) => &r.api_version,
        AnyResource::ClusterRoleBinding(r) => &r.api_version,
        AnyResource::ServiceAccount(s) => &s.api_version,
        AnyResource::PersistentVolume(p) => &p.api_version,
        AnyResource::PersistentVolumeClaim(p) => &p.api_version,
        AnyResource::Namespace(n) => &n.api_version,
        AnyResource::Node(n) => &n.api_version,
        AnyResource::Ingress(i) => &i.api_version,
        AnyResource::NetworkPolicy(n) => &n.api_version,
        AnyResource::VNet(v) => &v.api_version,
        AnyResource::Subnet(s) => &s.api_version,
        AnyResource::Nsg(n) => &n.api_version,
        AnyResource::RouteTable(r) => &r.api_version,
        AnyResource::StorageClass(s) => &s.api_version,
        AnyResource::Endpoints(e) => &e.api_version,
        AnyResource::EndpointSlice(e) => &e.api_version,
        AnyResource::Event(e) => &e.api_version,
    };
    if api_version == "v1" {
        return String::new();
    }
    api_version
        .split_once('/')
        .map(|(g, _)| g.to_string())
        .unwrap_or_default()
}

pub async fn apply_verb_for(store: &dyn StoreBackend, resource: &AnyResource) -> &'static str {
    if store.get(&resource.uid()).await.is_some() {
        "patch"
    } else {
        "create"
    }
}

/// Authorize every document; returns failures (empty = all allowed).
pub async fn authorize_apply_documents(
    store: &dyn StoreBackend,
    user: &str,
    resources: &[AnyResource],
) -> Vec<ApplyAuthzFailure> {
    let mut failures = Vec::new();
    for resource in resources {
        let verb = apply_verb_for(store, resource).await;
        let plural = rbac_plural(resource);
        let api_group = api_group_from_resource(resource);
        let api_group = if api_group.is_empty() {
            api_group_for_resource(&plural).to_string()
        } else {
            api_group
        };
        let req = AuthzRequest {
            user,
            namespace: resource.namespace(),
            resource: &plural,
            verb,
            api_group: &api_group,
            name: Some(resource.name()),
        };
        if !authorize(store, &req).await {
            failures.push(ApplyAuthzFailure {
                kind: resource.kind().to_string(),
                name: resource.name().to_string(),
                namespace: resource.namespace().to_string(),
                verb: verb.to_string(),
            });
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryBackend;
    use crate::types::{ObjectMeta, Role, RoleBinding, RoleRef, Subject};
    use std::sync::Arc;

    #[tokio::test]
    async fn apply_authz_denies_without_binding() {
        let store: Arc<dyn StoreBackend> = Arc::new(MemoryBackend::new());
        let role = Role {
            api_version: "rbac.authorization.k8s.io/v1".into(),
            kind: "Role".into(),
            metadata: ObjectMeta {
                name: Some("viewer".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            rules: vec![crate::types::PolicyRule {
                api_groups: vec!["".into()],
                resources: vec!["pods".into()],
                verbs: vec!["get".into()],
                resource_names: vec![],
            }],
        };
        store
            .apply(AnyResource::Role(role))
            .await
            .unwrap();
        let rb = RoleBinding {
            api_version: "rbac.authorization.k8s.io/v1".into(),
            kind: "RoleBinding".into(),
            metadata: ObjectMeta {
                name: Some("viewer-binding".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            subjects: vec![Subject {
                kind: "ServiceAccount".into(),
                namespace: "default".into(),
                name: "deploy-bot".into(),
            }],
            role_ref: RoleRef {
                api_group: "rbac.authorization.k8s.io".into(),
                kind: "Role".into(),
                name: "viewer".into(),
            },
        };
        store
            .apply(AnyResource::RoleBinding(rb))
            .await
            .unwrap();

        let cm_yaml = r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: denied-cm
  namespace: default
"#;
        let docs = parse_apply_body(cm_yaml.as_bytes()).unwrap();
        let denied = authorize_apply_documents(
            store.as_ref(),
            "system:serviceaccount:default:deploy-bot",
            &docs,
        )
        .await;
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].verb, "create");
    }
}
