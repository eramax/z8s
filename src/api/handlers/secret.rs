use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn list_secrets_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "Secret", "SecretList", None)
        .await
        .into_response()
}

pub async fn list_secrets(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "Secret", "SecretList", Some(&ns))
        .await
        .into_response()
}

pub async fn get_secret(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get_namespaced(&s, "Secret", &ns, &name).await
}

pub async fn create_secret(
    State(s): State<AppState>,
    Path(ns): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut sec: Secret = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Secret: {}", e)))?;
    if sec.metadata.namespace.is_none() {
        sec.metadata.namespace = Some(ns.clone());
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
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::Secret(sec), &ns, "Secret").await
}

pub async fn update_secret(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let patch = parse_body(&raw)?;
    let existing = s
        .store
        .get_by_kind("Secret")
        .await
        .into_iter()
        .find(|t| t.resource.namespace() == ns && t.resource.name() == name)
        .and_then(|t| {
            if let AnyResource::Secret(sec) = t.resource {
                serde_json::to_value(sec).ok()
            } else {
                None
            }
        });
    let mut merged = existing.unwrap_or(serde_json::Value::Object(Default::default()));
    json_merge_patch(&mut merged, &patch);
    let mut sec: Secret = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid Secret: {}", e)))?;
    if sec.metadata.namespace.is_none() {
        sec.metadata.namespace = Some(ns);
    }
    if sec.metadata.name.is_none() {
        sec.metadata.name = Some(name);
    }
    if let Some(sd) = sec.string_data.take() {
        let data = sec.data.get_or_insert_with(Default::default);
        for (k, v) in sd {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(v);
            data.insert(k, encoded);
        }
    }
    let resource = AnyResource::Secret(sec);
    s.apply_and_broadcast(resource.clone())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::to_value(&resource).unwrap_or_default()))
}

pub async fn delete_secret(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "Secret", &ns, &name).await
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/secrets", get(list_secrets_all))
        .route(
            "/api/v1/namespaces/{namespace}/secrets",
            get(list_secrets).post(create_secret),
        )
        .route(
            "/api/v1/namespaces/{namespace}/secrets/{name}",
            get(get_secret)
                .put(update_secret)
                .patch(update_secret)
                .delete(delete_secret),
        )
}
