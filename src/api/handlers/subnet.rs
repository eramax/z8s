use axum::Router;
use axum::routing::get;
use crate::api::server::*;
use uuid::Uuid;

pub async fn list_subnets(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let items: Vec<serde_json::Value> = state.store.get_by_kind("Subnet").await
        .into_iter()
        .filter_map(|t| serde_json::to_value(&t.resource).ok())
        .collect();
    Json(serde_json::json!({
        "apiVersion": "z8s.io/v1",
        "kind": "SubnetList",
        "items": items,
        "metadata": make_list_meta(),
    }))
}

pub async fn create_subnet(
    State(state): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut r: crate::netmux::crds::Subnet = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Subnet: {}", e)))?;
    if r.metadata.uid.is_none() { r.metadata.uid = Some(Uuid::new_v4().to_string()); }
    if r.metadata.creation_timestamp.is_none() { r.metadata.creation_timestamp = Some(now_time()); }
    let already_exists = state.store.get_by_kind("Subnet").await.iter()
        .any(|t| t.resource.name() == r.metadata.name.as_deref().unwrap_or(""));
    let resource = AnyResource::Subnet(r);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.registry.on_apply(&state.ctx, &resource).await;
    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}

pub async fn get_subnet(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("Subnet").await;
    for t in &trackers {
        if t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("subnet \"{}\" not found", name)))
}

pub async fn delete_subnet(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Subnet").await;
    for t in &trackers {
        if t.resource.name() == name {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("subnet \"{}\" not found", name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/subnets", get(list_subnets).post(create_subnet))
        .route("/apis/z8s.io/v1/subnets/{name}", get(get_subnet).delete(delete_subnet))
}
