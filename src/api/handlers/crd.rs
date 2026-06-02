use crate::api::server::*;

pub fn wants_table(accept: &axum::http::HeaderMap) -> bool {
    accept
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map_or(false, |v| v.contains("as=Table"))
}

pub async fn generic_list(s: &AppState, kind: &str, list_kind: &str) -> Json<serde_json::Value> {
    let items: Vec<serde_json::Value> = s
        .store
        .get_by_kind(kind)
        .await
        .into_iter()
        .filter_map(|t| serde_json::to_value(&t.resource).ok())
        .collect();
    Json(
        serde_json::json!({"apiVersion":"z8s.io/v1","kind":list_kind,"items":items,"metadata":make_list_meta()}),
    )
}

pub async fn generic_create(
    s: &AppState,
    mut resource: AnyResource,
    kind: &str,
) -> Result<axum::response::Response, ApiError> {
    let meta = resource.metadata_mut();
    if meta.uid.is_none() {
        meta.uid = Some(crate::config::random_id());
    }
    if meta.creation_timestamp.is_none() {
        meta.creation_timestamp = Some(now_time());
    }
    let exists = s
        .store
        .get_by_kind(kind)
        .await
        .iter()
        .any(|t| t.resource.name() == resource.name());
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let status = if exists {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(serde_json::to_value(&resource).unwrap_or_default()),
    )
        .into_response())
}

pub async fn generic_get(
    s: &AppState,
    kind: &str,
    name: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    for t in &s.store.get_by_kind(kind).await {
        if t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!(
        "{} \"{}\" not found",
        kind.to_lowercase(),
        name
    )))
}

pub async fn generic_delete(
    s: &AppState,
    kind: &str,
    name: &str,
) -> Result<Json<Status>, ApiError> {
    for t in &s.store.get_by_kind(kind).await {
        if t.resource.name() == name {
            s.registry.on_delete(&s.ctx, &t.resource).await;
            s.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!(
        "{} \"{}\" not found",
        kind.to_lowercase(),
        name
    )))
}

// ── Namespaced CRUD helpers ─────────────────────────────────────────

pub async fn generic_list_namespaced(
    s: &AppState,
    kind: &str,
    list_kind: &str,
    namespace: Option<&str>,
) -> Json<serde_json::Value> {
    let items: Vec<serde_json::Value> = s
        .store
        .get_by_kind(kind)
        .await
        .into_iter()
        .filter(|t| namespace.map_or(true, |ns| t.resource.namespace() == ns))
        .filter_map(|t| serde_json::to_value(&t.resource).ok())
        .collect();
    Json(serde_json::json!({
        "apiVersion":"v1",
        "kind": list_kind,
        "items": items,
        "metadata": make_list_meta()
    }))
}

pub async fn generic_get_namespaced(
    s: &AppState,
    kind: &str,
    namespace: &str,
    name: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    for t in &s.store.get_by_kind(kind).await {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!(
        "{} \"{}/{}\" not found",
        kind.to_lowercase(),
        namespace,
        name
    )))
}

pub async fn generic_create_namespaced(
    s: &AppState,
    mut resource: AnyResource,
    namespace: &str,
    kind: &str,
) -> Result<axum::response::Response, ApiError> {
    {
        let meta = resource.metadata_mut();
        if meta.namespace.is_none() {
            meta.namespace = Some(namespace.to_string());
        }
        if meta.uid.is_none() {
            meta.uid = Some(crate::config::random_id());
        }
        if meta.creation_timestamp.is_none() {
            meta.creation_timestamp = Some(now_time());
        }
    }
    let already_exists = s
        .store
        .get_by_kind(kind)
        .await
        .iter()
        .any(|t| t.resource.namespace() == namespace && t.resource.name() == resource.name());
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let status = if already_exists {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(serde_json::to_value(&resource).unwrap_or_default()),
    )
        .into_response())
}

pub async fn generic_update_namespaced(
    s: &AppState,
    kind: &str,
    namespace: &str,
    name: &str,
    raw: &axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(raw)?;
    let existing = s
        .store
        .get_by_kind(kind)
        .await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| serde_json::to_value(&t.resource).ok());
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut resource = AnyResource::from_json_value(merged, kind)
        .map_err(|e| ApiError::bad_request(format!("invalid {}: {}", kind, e)))?;
    {
        let meta = resource.metadata_mut();
        if meta.namespace.is_none() {
            meta.namespace = Some(namespace.to_string());
        }
        if meta.name.is_none() {
            meta.name = Some(name.to_string());
        }
    }
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}

pub async fn generic_delete_namespaced(
    s: &AppState,
    kind: &str,
    namespace: &str,
    name: &str,
) -> Result<Json<Status>, ApiError> {
    for t in &s.store.get_by_kind(kind).await {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            s.registry.on_delete(&s.ctx, &t.resource).await;
            s.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!(
        "{} \"{}/{}\" not found",
        kind.to_lowercase(),
        namespace,
        name
    )))
}

// Required for metadata_mut to work — AnyResource has metadata_mut() method.
pub use crate::store::AnyResource;
