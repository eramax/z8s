//! Generic catalog CRUD (A2).

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::api::admission;
use crate::api::catalog::{self, ResourceEntry};
use crate::api::compat::{self, WireContext};
use crate::api::handlers::crd;
use crate::api::server::*;
use crate::store::AnyResource;

fn entry_for_plural(plural: &str) -> Result<&'static ResourceEntry, ApiError> {
    catalog::by_plural(plural).ok_or_else(|| {
        ApiError::not_found(format!("resource type \"{}\" not in catalog", plural))
    })
}

fn wire_from_request(req: &Request) -> WireContext {
    WireContext::from_path(req.uri().path())
}

/// `GET /api/v1/{plural}` — cluster list or all-namespaces list depending on scope.
pub async fn list_v1_plural(
    State(s): State<AppState>,
    Path(plural): Path<String>,
    req: Request,
) -> Result<axum::response::Response, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if crate::api::watch::is_watch_request(req.uri().query()) {
        let wire = wire_from_request(&req);
        return Ok(crate::api::watch::watch_list(s, entry, None, wire).await);
    }
    if entry.namespaced {
        list_namespaced_all(State(s), Path(plural), req).await
    } else {
        list_cluster(State(s), Path(plural), req).await
    }
}

pub async fn create_v1_plural(
    State(s): State<AppState>,
    Path(plural): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if entry.namespaced {
        return Err(ApiError::bad_request(format!(
            "{} requires a namespace in the path",
            entry.kind
        )));
    }
    create_cluster(State(s), Path(plural), raw).await
}

pub async fn list_cluster(
    State(s): State<AppState>,
    Path(plural): Path<String>,
    req: Request,
) -> Result<axum::response::Response, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if entry.namespaced {
        return Err(ApiError::bad_request(format!(
            "{} is namespaced",
            entry.kind
        )));
    }
    let wire = wire_from_request(&req);
    if crate::api::watch::is_watch_request(req.uri().query()) {
        return Ok(crate::api::watch::watch_list(s, entry, None, wire).await);
    }
    if entry.kind == "Node" {
        return Ok(crate::api::enrich::list_nodes(&s, req.headers(), &wire).await);
    }
    if entry.kind == "VNet" {
        return Ok(crate::api::enrich::list_vnets(&s, req.headers(), &wire).await);
    }
    let resp = crd::generic_list_wire(&s, entry.kind, entry.list_kind, &wire.list_api_version).await;
    Ok((StatusCode::OK, resp).into_response())
}

pub async fn list_namespaced(
    State(s): State<AppState>,
    Path((namespace, plural)): Path<(String, String)>,
    req: Request,
) -> Result<axum::response::Response, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if !entry.namespaced {
        return Err(ApiError::bad_request(format!(
            "{} is cluster-scoped",
            entry.kind
        )));
    }
    let wire = wire_from_request(&req);
    if crate::api::watch::is_watch_request(req.uri().query()) {
        return Ok(
            crate::api::watch::watch_list(s, entry, Some(namespace), wire).await,
        );
    }
    if entry.kind == "Deployment" {
        return Ok(crate::api::enrich::list_deployments(
            &s,
            Some(namespace.as_str()),
            req.headers(),
            &wire,
        )
        .await);
    }
    if entry.kind == "Service" {
        return Ok(crate::api::enrich::list_services(
            &s,
            Some(namespace.as_str()),
            req.headers(),
            &wire,
        )
        .await);
    }
    if entry.kind == "Pod" {
        let query = req.uri().query().unwrap_or("");
        return Ok(crate::api::enrich::list_pods(
            &s,
            Some(namespace.as_str()),
            query,
            req.headers(),
            &wire,
        )
        .await);
    }
    let resp = crd::generic_list_namespaced_wire(
        &s,
        entry.kind,
        entry.list_kind,
        Some(namespace.as_str()),
        &wire.list_api_version,
    )
    .await;
    Ok((StatusCode::OK, resp).into_response())
}

pub async fn list_namespaced_all(
    State(s): State<AppState>,
    Path(plural): Path<String>,
    req: Request,
) -> Result<axum::response::Response, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if !entry.namespaced {
        return Err(ApiError::bad_request(format!(
            "{} is cluster-scoped",
            entry.kind
        )));
    }
    let wire = wire_from_request(&req);
    if crate::api::watch::is_watch_request(req.uri().query()) {
        return Ok(crate::api::watch::watch_list(s, entry, None, wire).await);
    }
    if entry.kind == "Deployment" {
        return Ok(
            crate::api::enrich::list_deployments(&s, None, req.headers(), &wire).await,
        );
    }
    if entry.kind == "Service" {
        return Ok(crate::api::enrich::list_services(&s, None, req.headers(), &wire).await);
    }
    if entry.kind == "Pod" {
        let query = req.uri().query().unwrap_or("");
        return Ok(
            crate::api::enrich::list_pods(&s, None, query, req.headers(), &wire).await,
        );
    }
    let resp = crd::generic_list_namespaced_wire(
        &s,
        entry.kind,
        entry.list_kind,
        None,
        &wire.list_api_version,
    )
    .await;
    Ok((StatusCode::OK, resp).into_response())
}

pub async fn get_cluster(
    State(s): State<AppState>,
    Path((plural, name)): Path<(String, String)>,
    req: Request,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entry = entry_for_plural(&plural)?;
    let wire = wire_from_request(&req);
    if entry.kind == "Node" {
        return crate::api::enrich::get_node(&s, &name, &wire).await;
    }
    if entry.kind == "VNet" {
        return crate::api::enrich::get_vnet(&s, &name, &wire).await;
    }
    let value = crd::generic_get(&s, entry.kind, &name).await?.0;
    Ok(Json(compat::encode_resource_value(value, &wire)))
}

pub async fn get_namespaced(
    State(s): State<AppState>,
    Path((namespace, plural, name)): Path<(String, String, String)>,
    req: Request,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entry = entry_for_plural(&plural)?;
    let wire = wire_from_request(&req);
    if entry.kind == "Deployment" {
        return crate::api::enrich::get_deployment(&s, &namespace, &name, &wire).await;
    }
    if entry.kind == "Pod" {
        return crate::api::enrich::get_pod(&s, &namespace, &name, &wire).await;
    }
    let value = crd::generic_get_namespaced(&s, entry.kind, &namespace, &name)
        .await?
        .0;
    Ok(Json(compat::encode_resource_value(value, &wire)))
}

pub async fn create_cluster(
    State(s): State<AppState>,
    Path(plural): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let entry = entry_for_plural(&plural)?;
    let body = parse_body(&raw)?;
    let mut resource = AnyResource::from_json_value(body, entry.kind)
        .map_err(|e| ApiError::bad_request(format!("invalid {}: {}", entry.kind, e)))?;
    admission::prepare_metadata(&mut resource, None);
    admission::prepare_create(&mut resource);
    let exists = s
        .store
        .get_by_kind(entry.kind)
        .await
        .iter()
        .any(|t| t.resource.name() == resource.name());
    admission::reject_duplicate_cluster_create(entry.kind, resource.name(), exists)?;
    if entry.kind == "Namespace" {
        return create_namespace(&s, resource).await;
    }
    crd::generic_create(&s, resource, entry.kind).await
}

async fn create_namespace(
    s: &AppState,
    resource: AnyResource,
) -> Result<axum::response::Response, ApiError> {
    let AnyResource::Namespace(ns) = resource else {
        return Err(ApiError::bad_request("expected Namespace".into()));
    };
    s.apply_and_broadcast(AnyResource::Namespace(ns.clone()))
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(ns)).into_response())
}

async fn delete_namespace_cascade(s: &AppState, name: &str) -> Result<Json<Status>, ApiError> {
    if name == "default" {
        return Err(ApiError::bad_request(
            "cannot delete default namespace".into(),
        ));
    }
    let trackers = s.store.get_by_kind("Namespace").await;
    for t in &trackers {
        if t.resource.name() != name {
            continue;
        }
        s.store.delete(&t.resource).await.ok();
        for kind in &[
            "Pod",
            "Deployment",
            "Service",
            "ConfigMap",
            "Secret",
            "PersistentVolumeClaim",
        ] {
            let items = s.store.get_by_kind(kind).await;
            for item in &items {
                if item.resource.namespace() == name {
                    s.registry.on_delete(&s.ctx, &item.resource).await;
                    s.store.delete(&item.resource).await.ok();
                }
            }
        }
        tracing::info!("Deleted namespace: {}", name);
        return Ok(Json(ok_status()));
    }
    Err(ApiError::not_found(format!(
        "namespace \"{name}\" not found"
    )))
}

pub async fn create_namespaced(
    State(s): State<AppState>,
    Path((namespace, plural)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if entry.kind == "Service" {
        return crate::api::enrich::create_service(&s, &namespace, &raw).await;
    }
    if entry.kind == "Pod" {
        return crate::api::enrich::create_pod(&s, &namespace, &raw).await;
    }
    let body = parse_body(&raw)?;
    let mut resource = AnyResource::from_json_value(body, entry.kind)
        .map_err(|e| ApiError::bad_request(format!("invalid {}: {}", entry.kind, e)))?;
    admission::prepare_metadata(&mut resource, Some(&namespace));
    admission::prepare_create(&mut resource);
    crd::generic_create_namespaced(&s, resource, &namespace, entry.kind).await
}

pub async fn update_namespaced(
    State(s): State<AppState>,
    Path((namespace, plural, name)): Path<(String, String, String)>,
    req: Request,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entry = entry_for_plural(&plural)?;
    let wire = wire_from_request(&req);
    let raw = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if entry.kind == "Deployment" {
        return crate::api::enrich::update_deployment(&s, &namespace, &name, &raw, &wire).await;
    }
    if entry.kind == "Service" {
        return crate::api::enrich::update_service(&s, &namespace, &name, &raw, &wire).await;
    }
    if entry.kind == "Pod" {
        return crate::api::enrich::update_pod(&s, &namespace, &name, &raw, &wire).await;
    }
    let value = crd::generic_update_namespaced(&s, entry.kind, &namespace, &name, &raw).await?;
    Ok(Json(compat::encode_resource_value(value.0, &wire)))
}

pub async fn delete_cluster(
    State(s): State<AppState>,
    Path((plural, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if entry.kind == "Namespace" {
        return delete_namespace_cascade(&s, &name).await;
    }
    crd::generic_delete(&s, entry.kind, &name).await
}

pub async fn delete_namespaced(
    State(s): State<AppState>,
    Path((namespace, plural, name)): Path<(String, String, String)>,
) -> Result<Json<Status>, ApiError> {
    let entry = entry_for_plural(&plural)?;
    if entry.kind == "Deployment" {
        return crate::api::enrich::delete_deployment_cascade(&s, &namespace, &name).await;
    }
    if entry.kind == "Pod" {
        return crate::api::enrich::delete_pod(&s, &namespace, &name).await;
    }
    crd::generic_delete_namespaced(&s, entry.kind, &namespace, &name).await
}
