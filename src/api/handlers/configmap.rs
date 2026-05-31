use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_configmaps_all(State(state): State<AppState>) -> Json<List<ConfigMap>> {
    list_configmaps_in_ns(&state, None).await
}

pub async fn list_configmaps(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<ConfigMap>> {
    list_configmaps_in_ns(&state, Some(namespace)).await
}

pub async fn list_configmaps_in_ns(state: &AppState, namespace: Option<String>) -> Json<List<ConfigMap>> {
    let items: Vec<ConfigMap> = state.store.get_by_kind("ConfigMap").await
        .into_iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .filter_map(|t| if let AnyResource::ConfigMap(cm) = t.resource { Some(cm) } else { None })
        .collect();
    Json(List { kind: Some("ConfigMapList".into()), api_version: None, items, metadata: make_list_meta() })
}

pub async fn get_configmap(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<ConfigMap>, ApiError> {
    state.store.get_by_kind("ConfigMap").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::ConfigMap(cm) = t.resource { Some(Json(cm)) } else { None })
        .ok_or_else(|| ApiError::not_found(format!("configmap \"{}/{}\" not found", namespace, name)))
}

pub async fn create_configmap(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut cm: ConfigMap = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid ConfigMap: {}", e)))?;
    if cm.metadata.namespace.is_none() { cm.metadata.namespace = Some(namespace); }
    if cm.metadata.uid.is_none() { cm.metadata.uid = Some(uuid::Uuid::new_v4().to_string()); }
    if cm.metadata.creation_timestamp.is_none() { cm.metadata.creation_timestamp = Some(now_time()); }
    let resource = AnyResource::ConfigMap(cm);
    let already_exists = state.store.get_by_kind("ConfigMap").await.iter()
        .any(|t| t.resource.name() == resource.name() && t.resource.namespace() == resource.namespace());
    state.apply_and_broadcast(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(resource)).into_response())
}

pub async fn update_configmap(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<ConfigMap>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = state.store.get_by_kind("ConfigMap").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::ConfigMap(cm) = t.resource { serde_json::to_value(cm).ok() } else { None });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut cm: ConfigMap = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid ConfigMap: {}", e)))?;
    if cm.metadata.namespace.is_none() { cm.metadata.namespace = Some(namespace); }
    if cm.metadata.name.is_none() { cm.metadata.name = Some(name); }
    let resource = AnyResource::ConfigMap(cm);
    state.apply_and_broadcast(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    match resource { AnyResource::ConfigMap(cm) => Ok(Json(cm)), _ => unreachable!() }
}

pub async fn delete_configmap(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("ConfigMap").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("configmap \"{}/{}\" not found", namespace, name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/configmaps", get(list_configmaps_all))
        .route("/api/v1/namespaces/{namespace}/configmaps", get(list_configmaps).post(create_configmap))
        .route("/api/v1/namespaces/{namespace}/configmaps/{name}", get(get_configmap).put(update_configmap).patch(update_configmap).delete(delete_configmap))
}
