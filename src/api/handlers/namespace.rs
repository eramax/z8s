use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_namespaces(State(state): State<AppState>) -> Json<List<Namespace>> {
    let ns = state.namespaces.read().await;
    Json(List {
        items: ns.values().cloned().collect(),
        metadata: make_list_meta(),
    })
}


pub async fn get_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Namespace>, ApiError> {
    let ns = state.namespaces.read().await;
    ns.get(&name)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("namespace \"{}\" not found", name)))
}


pub async fn create_namespace(
    State(state): State<AppState>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let val = parse_body(&raw)?;
    let mut ns: Namespace = serde_json::from_value(val)
        .map_err(|e| ApiError::bad_request(format!("invalid Namespace: {}", e)))?;
    let name = ns
        .metadata
        .name
        .clone()
        .unwrap_or_else(|| format!("ns-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("x")));

    ns.metadata.name = Some(name.clone());
    if ns.metadata.uid.is_none() {
        ns.metadata.uid = Some(format!("ns-{}", name));
    }
    if ns.metadata.creation_timestamp.is_none() {
        ns.metadata.creation_timestamp = Some(now_time());
    }
    ns.status = Some(NamespaceStatus {
        phase: Some("Active".into()),
        ..Default::default()
    });

    let mut store = state.namespaces.write().await;
    if store.contains_key(&name) {
        return Err(ApiError::bad_request(format!("namespace \"{}\" already exists", name)));
    }
    info!("Created namespace: {}", name);
    store.insert(name, ns.clone());
    Ok((StatusCode::CREATED, Json(ns)).into_response())
}


pub async fn delete_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Status>, ApiError> {
    if name == "default" {
        return Err(ApiError::bad_request("cannot delete default namespace".into()));
    }
    let mut ns = state.namespaces.write().await;
    if ns.remove(&name).is_some() {
        // Cascade: drop namespaced objects from the in-memory store (kubectl expects
        // namespace delete to remove children; otherwise stale pods skew status).
        let pod_trackers = state.store.get_by_kind("Pod").await;
        for t in &pod_trackers {
            if t.resource.namespace() == name.as_str() {
                state.registry.on_delete(&state.ctx, &t.resource).await;
                state.store.delete(&t.resource).await.ok();
            }
        }
        for kind in ["Deployment", "Service", "ConfigMap", "Secret"] {
            let trackers = state.store.get_by_kind(kind).await;
            for t in &trackers {
                if t.resource.namespace() == name.as_str() {
                    state.registry.on_delete(&state.ctx, &t.resource).await;
                    state.store.delete(&t.resource).await.ok();
                }
            }
        }
        info!("Deleted namespace: {}", name);
        Ok(Json(ok_status()))
    } else {
        Err(ApiError::not_found(format!("namespace \"{}\" not found", name)))
    }
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/namespaces", get(list_namespaces).post(create_namespace))
        .route("/api/v1/namespaces/{name}", get(get_namespace).delete(delete_namespace))
}
