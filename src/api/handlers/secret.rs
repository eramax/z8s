use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_secrets_all(State(state): State<AppState>) -> Json<List<Secret>> {
    list_secrets_in_ns(&state, None).await
}


pub async fn list_secrets(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<Secret>> {
    list_secrets_in_ns(&state, Some(namespace)).await
}


pub async fn list_secrets_in_ns(state: &AppState, namespace: Option<String>) -> Json<List<Secret>> {
    let items: Vec<Secret> = state.store.get_by_kind("Secret").await
        .into_iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .filter_map(|t| if let AnyResource::Secret(s) = t.resource { Some(s) } else { None })
        .collect();
    Json(List::<Secret> {
        kind: Some("SecretList".into()),
        api_version: None,
        items,
        metadata: ListMeta { resource_version: Some("1".into()), ..Default::default() },
    })
}


pub async fn get_secret(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Secret").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("secret \"{}/{}\" not found", namespace, name)))
}


pub async fn create_secret(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut sec: Secret = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Secret: {}", e)))?;
    if sec.metadata.namespace.is_none() {
        sec.metadata.namespace = Some(namespace);
    }
    if sec.metadata.uid.is_none() {
        sec.metadata.uid = Some(uuid::Uuid::new_v4().to_string());
    }
    if sec.metadata.creation_timestamp.is_none() {
        sec.metadata.creation_timestamp = Some(now_time());
    }
    // Kubernetes API: move stringData into data as base64-encoded values
    if let Some(sd) = sec.string_data.take() {
        let data = sec.data.get_or_insert_with(Default::default);
        for (k, v) in sd {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(v);
            data.insert(k, encoded);
        }
    }
    let resource = AnyResource::Secret(sec);
    let already_exists = state.store.get_by_kind("Secret").await.iter()
        .any(|t| t.resource.name() == resource.name() && t.resource.namespace() == resource.namespace());
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}


pub async fn update_secret(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = state.store.get_by_kind("Secret").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::Secret(sec) = t.resource { serde_json::to_value(sec).ok() } else { None });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut sec: Secret = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Secret: {}", e)))?;
    if sec.metadata.namespace.is_none() { sec.metadata.namespace = Some(namespace); }
    if sec.metadata.name.is_none() { sec.metadata.name = Some(name); }
    if let Some(sd) = sec.string_data.take() {
        let data = sec.data.get_or_insert_with(Default::default);
        for (k, v) in sd {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(v);
            data.insert(k, encoded);
        }
    }
    let resource = AnyResource::Secret(sec);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}


pub async fn delete_secret(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Secret").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("secret \"{}/{}\" not found", namespace, name)))
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/secrets", get(list_secrets_all))
        .route("/api/v1/namespaces/{namespace}/secrets", get(list_secrets).post(create_secret))
        .route("/api/v1/namespaces/{namespace}/secrets/{name}", get(get_secret).put(update_secret).patch(update_secret).delete(delete_secret))
}
