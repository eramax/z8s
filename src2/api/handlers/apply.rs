//! Multi-document kubectl-style apply with per-object RBAC (R6).

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;

use crate::api::auth::{self, apply as apply_auth};
use crate::api::server::*;

pub async fn apply_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let resources = apply_auth::parse_apply_body(&raw)
        .map_err(|e| ApiError::bad_request(format!("invalid apply body: {}", e)))?;

    if resources.is_empty() {
        return Err(ApiError::bad_request("apply body has no documents".into()));
    }

    let user = auth::extract_user(&headers, Some(state.process_tracker.tokens.as_ref())).await;

    if crate::config::rbac_enforced() && auth::has_any_rbac_policy(state.store.as_ref()).await {
        let denied =
            apply_auth::authorize_apply_documents(state.store.as_ref(), &user, &resources).await;
        if !denied.is_empty() {
            let status = Status {
                status: Some("Failure".into()),
                message: Some(format!(
                    "RBAC denied {} of {} document(s): {}",
                    denied.len(),
                    resources.len(),
                    serde_json::to_string(&denied).unwrap_or_default()
                )),
                reason: Some("Forbidden".into()),
                code: Some(403),
                ..Default::default()
            };
            return Ok((StatusCode::FORBIDDEN, Json(status)).into_response());
        }
    }

    let mut applied = Vec::new();
    for mut resource in resources {
        let meta = resource.metadata_mut();
        if meta.uid.is_none() {
            meta.uid = Some(crate::config::random_id());
        }
        if meta.creation_timestamp.is_none() {
            meta.creation_timestamp = Some(now_time());
        }
        let kind = resource.kind().to_string();
        let name = resource.name().to_string();
        let ns = resource.namespace().to_string();
        state
            .apply_and_broadcast(resource)
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        applied.push(serde_json::json!({
            "kind": kind,
            "name": name,
            "namespace": ns,
        }));
    }

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "kind": "ApplyResult",
            "apiVersion": "z8s.io/v1",
            "items": applied,
        })),
    )
        .into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/apply", post(apply_handler))
        .route("/apis/z8s.io/v1/apply", post(apply_handler))
}
