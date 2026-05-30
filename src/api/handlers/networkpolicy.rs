use axum::Router;
use axum::routing::{get, delete};
use crate::api::server::*;
use uuid::Uuid;

pub async fn list_networkpolicies_all(
    State(state): State<AppState>,
) -> Json<List<k8s_openapi::api::networking::v1::NetworkPolicy>> {
    let items: Vec<k8s_openapi::api::networking::v1::NetworkPolicy> = state.store.get_by_kind("NetworkPolicy").await
        .into_iter()
        .filter_map(|t| if let AnyResource::NetworkPolicy(np) = t.resource { Some(np) } else { None })
        .collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn list_networkpolicies(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<k8s_openapi::api::networking::v1::NetworkPolicy>> {
    let items: Vec<k8s_openapi::api::networking::v1::NetworkPolicy> = state.store.get_by_kind("NetworkPolicy").await
        .into_iter()
        .filter(|t| t.resource.namespace() == namespace)
        .filter_map(|t| if let AnyResource::NetworkPolicy(np) = t.resource { Some(np) } else { None })
        .collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn create_networkpolicy(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut np: k8s_openapi::api::networking::v1::NetworkPolicy = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid NetworkPolicy: {}", e)))?;
    if np.metadata.namespace.is_none() { np.metadata.namespace = Some(namespace.clone()); }
    if np.metadata.uid.is_none() {
        np.metadata.uid = Some(Uuid::new_v4().to_string());
    }
    if np.metadata.creation_timestamp.is_none() {
        np.metadata.creation_timestamp = Some(now_time());
    }

    let already_exists = state.store.get_by_kind("NetworkPolicy").await.iter()
        .any(|t| t.resource.name() == np.metadata.name.as_deref().unwrap_or("") && t.resource.namespace() == namespace);

    let resource = AnyResource::NetworkPolicy(np);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;

    state.registry.on_apply(&state.ctx, &resource).await;

    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}

pub async fn get_networkpolicy(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("NetworkPolicy").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("networkpolicy \"{}/{}\" not found", namespace, name)))
}

pub async fn update_networkpolicy(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = state.store.get_by_kind("NetworkPolicy").await
        .into_iter()
        .find(|t| t.resource.namespace() == namespace && t.resource.name() == name)
        .and_then(|t| if let AnyResource::NetworkPolicy(np) = t.resource { serde_json::to_value(np).ok() } else { None });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut np: k8s_openapi::api::networking::v1::NetworkPolicy = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid NetworkPolicy: {}", e)))?;
    if np.metadata.namespace.is_none() { np.metadata.namespace = Some(namespace); }
    if np.metadata.name.is_none() { np.metadata.name = Some(name); }
    let resource = AnyResource::NetworkPolicy(np);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.registry.on_apply(&state.ctx, &resource).await;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}

pub async fn delete_networkpolicy(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("NetworkPolicy").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("networkpolicy \"{}/{}\" not found", namespace, name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/networking.k8s.io/v1/networkpolicies", get(list_networkpolicies_all))
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies", get(list_networkpolicies).post(create_networkpolicy))
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies/{name}", get(get_networkpolicy).put(update_networkpolicy).patch(update_networkpolicy).delete(delete_networkpolicy))
}
