use axum::Router;
use axum::routing::{get, delete};
use crate::api::server::*;
use uuid::Uuid;

pub async fn list_ingresses_all(
    State(state): State<AppState>,
) -> Json<List<Ingress>> {
    let items: Vec<Ingress> = state.store.get_by_kind("Ingress").await
        .into_iter()
        .filter_map(|t| if let AnyResource::Ingress(ing) = t.resource { Some(ing) } else { None })
        .collect();
    Json(List { kind: Some("IngressList".into()), api_version: None, items, metadata: make_list_meta() })
}

pub async fn list_ingresses(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<Ingress>> {
    let items: Vec<Ingress> = state.store.get_by_kind("Ingress").await
        .into_iter()
        .filter(|t| t.resource.namespace() == namespace)
        .filter_map(|t| if let AnyResource::Ingress(ing) = t.resource { Some(ing) } else { None })
        .collect();
    Json(List { kind: Some("IngressList".into()), api_version: None, items, metadata: make_list_meta() })
}

pub async fn create_ingress(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut ing: Ingress = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Ingress: {}", e)))?;
    if ing.metadata.namespace.is_none() {
        ing.metadata.namespace = Some(namespace.clone());
    }
    if ing.metadata.uid.is_none() {
        ing.metadata.uid = Some(Uuid::new_v4().to_string());
    }
    if ing.metadata.creation_timestamp.is_none() {
        ing.metadata.creation_timestamp = Some(now_time());
    }

    let already_exists = state.store.get_by_kind("Ingress").await.iter()
        .any(|t| t.resource.name() == ing.metadata.name.as_deref().unwrap_or("") && t.resource.namespace() == namespace);

    let resource = AnyResource::Ingress(ing);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;

    state.registry.on_apply(&state.ctx, &resource).await;

    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}

pub async fn get_ingress(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Ingress").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("ingress \"{}/{}\" not found", namespace, name)))
}

pub async fn update_ingress(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = state.store.get_by_kind("Ingress").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::Ingress(ing) = t.resource { serde_json::to_value(ing).ok() } else { None });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut ing: Ingress = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Ingress: {}", e)))?;
    if ing.metadata.namespace.is_none() { ing.metadata.namespace = Some(namespace); }
    if ing.metadata.name.is_none() { ing.metadata.name = Some(name); }
    let resource = AnyResource::Ingress(ing);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.registry.on_apply(&state.ctx, &resource).await;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}

pub async fn delete_ingress(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Ingress").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("ingress \"{}/{}\" not found", namespace, name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/networking.k8s.io/v1/ingresses", get(list_ingresses_all))
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses", get(list_ingresses).post(create_ingress))
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses/{name}", get(get_ingress).put(update_ingress).patch(update_ingress).delete(delete_ingress))
}
