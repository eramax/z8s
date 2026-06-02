//! Admission: defaults before store apply (A1).

use crate::api::server::now_time;
use crate::store::AnyResource;
use crate::types::NamespaceStatus;

/// Apply cross-cutting metadata defaults on create/update.
pub fn prepare_metadata(resource: &mut AnyResource, namespace: Option<&str>) {
    let meta = resource.metadata_mut();
    if meta.namespace.is_none() {
        if let Some(ns) = namespace {
            meta.namespace = Some(ns.to_string());
        }
    }
    if meta.uid.is_none() {
        meta.uid = Some(crate::config::random_id());
    }
    if meta.creation_timestamp.is_none() {
        meta.creation_timestamp = Some(now_time());
    }
}

/// Kind-specific fields after metadata defaults.
pub fn prepare_create(resource: &mut AnyResource) {
    if let AnyResource::Namespace(ns) = resource {
        if let Some(name) = ns.metadata.name.clone() {
            if ns.metadata.uid.is_none() {
                ns.metadata.uid = Some(format!("ns-{name}"));
            }
        }
        if ns.status.is_none() {
            ns.status = Some(NamespaceStatus {
                phase: Some("Active".into()),
                ..Default::default()
            });
        }
    }
    if let AnyResource::Deployment(d) = resource {
        crate::api::enrich::fill_deployment_metadata(d);
    }
    if let AnyResource::Pod(p) = resource {
        crate::components::compute::status::fill_pod_metadata(p);
    }
}

pub fn prepare_update(resource: &mut AnyResource) {
    if let AnyResource::Deployment(d) = resource {
        crate::api::enrich::fill_deployment_metadata(d);
    }
    if let AnyResource::Pod(p) = resource {
        crate::components::compute::status::fill_pod_metadata(p);
    }
}

/// Reject create when a cluster-scoped object with the same name already exists.
pub fn reject_duplicate_cluster_create(
    kind: &str,
    name: &str,
    exists: bool,
) -> Result<(), crate::api::server::ApiError> {
    if kind == "Namespace" && exists {
        return Err(crate::api::server::ApiError::bad_request(format!(
            "namespace \"{name}\" already exists"
        )));
    }
    Ok(())
}
