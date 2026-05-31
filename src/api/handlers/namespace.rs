use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_namespaces(State(state): State<AppState>) -> Json<List<Namespace>> {
    let trackers = state.store.get_by_kind("Namespace").await;
    let items: Vec<Namespace> = trackers
        .into_iter()
        .filter_map(|t| match t.resource { AnyResource::Namespace(ns) => Some(ns), _ => None })
        .collect();
    Json(List {
        kind: Some("NamespaceList".into()),
        api_version: None,
        items,
        metadata: make_list_meta(),
    })
}

pub async fn get_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Namespace>, ApiError> {
    let trackers = state.store.get_by_kind("Namespace").await;
    trackers
        .into_iter()
        .find_map(|t| match t.resource {
            AnyResource::Namespace(ns) if ns.metadata.name.as_deref() == Some(&name) => Some(Json(ns)),
            _ => None,
        })
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

    // Check if already exists
    let existing = state.store.get_by_kind("Namespace").await;
    if existing.iter().any(|t| t.resource.name() == name) {
        return Err(ApiError::bad_request(format!("namespace \"{}\" already exists", name)));
    }

    state.apply_and_broadcast(AnyResource::Namespace(ns.clone())).await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    info!("Created namespace: {} — verifying...", name);
    // Verify by reading back
    let check = state.store.get_by_kind("Namespace").await;
    let found = check.iter().any(|t| t.resource.name() == name);
    info!("Namespace verify: found={}, total_namespaces={}", found, check.len());
    for t in &check {
        info!("  stored namespace: {}", t.resource.uid());
    }
    Ok((StatusCode::CREATED, Json(ns)).into_response())
}

pub async fn delete_namespace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Status>, ApiError> {
    if name == "default" {
        return Err(ApiError::bad_request("cannot delete default namespace".into()));
    }
    let trackers = state.store.get_by_kind("Namespace").await;
    for t in &trackers {
        if t.resource.name() == name {
            state.store.delete(&t.resource).await.ok();
            // Cascade delete namespaced resources
            for kind in &["Pod", "Deployment", "Service", "ConfigMap", "Secret", "PersistentVolumeClaim"] {
                let items = state.store.get_by_kind(kind).await;
                for item in &items {
                    if item.resource.namespace() == name.as_str() {
                        state.registry.on_delete(&state.ctx, &item.resource).await;
                        state.store.delete(&item.resource).await.ok();
                    }
                }
            }
            info!("Deleted namespace: {}", name);
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("namespace \"{}\" not found", name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/namespaces", get(list_namespaces).post(create_namespace))
        .route("/api/v1/namespaces/{name}", get(get_namespace).delete(delete_namespace))
}
