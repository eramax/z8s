use axum::Router;
use axum::routing::get;
use crate::api::server::*;
use uuid::Uuid;

pub async fn list_routetables(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let items: Vec<serde_json::Value> = state.store.get_by_kind("RouteTable").await
        .into_iter()
        .filter_map(|t| serde_json::to_value(&t.resource).ok())
        .collect();
    Json(serde_json::json!({
        "apiVersion": "z8s.io/v1",
        "kind": "RouteTableList",
        "items": items,
        "metadata": make_list_meta(),
    }))
}

pub async fn create_routetable(
    State(state): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut r: crate::netmux::crds::RouteTable = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid RouteTable: {}", e)))?;
    if r.metadata.uid.is_none() { r.metadata.uid = Some(Uuid::new_v4().to_string()); }
    if r.metadata.creation_timestamp.is_none() { r.metadata.creation_timestamp = Some(now_time()); }
    let already_exists = state.store.get_by_kind("RouteTable").await.iter()
        .any(|t| t.resource.name() == r.metadata.name.as_deref().unwrap_or(""));
    let resource = AnyResource::RouteTable(r);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.registry.on_apply(&state.ctx, &resource).await;
    let status = if already_exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::to_value(&resource).unwrap_or_default())).into_response())
}

pub async fn get_routetable(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let trackers = state.store.get_by_kind("RouteTable").await;
    for t in &trackers {
        if t.resource.name() == name {
            return Ok(Json(serde_json::to_value(&t.resource).unwrap_or_default()));
        }
    }
    Err(ApiError::not_found(format!("routetable \"{}\" not found", name)))
}

pub async fn delete_routetable(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("RouteTable").await;
    for t in &trackers {
        if t.resource.name() == name {
            state.registry.on_delete(&state.ctx, &t.resource).await;
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("routetable \"{}\" not found", name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/z8s.io/v1/routetables", get(list_routetables).post(create_routetable))
        .route("/apis/z8s.io/v1/routetables/{name}", get(get_routetable).delete(delete_routetable))
}
