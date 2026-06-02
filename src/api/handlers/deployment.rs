//! Deployment `/scale` subresource (A2 — CRUD via catalog + `api::enrich`).

use crate::api::enrich;
use crate::api::server::*;
use axum::Router;
use axum::routing::patch;

pub async fn patch_deployment_scale(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::types::{Scale, ScaleSpec, ScaleStatus};
    let body = parse_body(&raw)?;
    let mut deploy = enrich::find_deployment(&state, &namespace, &name)
        .await
        .ok_or_else(|| ApiError::not_found(format!("deployment \"{name}\" not found")))?;
    let desired = body
        .get("spec")
        .and_then(|s| s.get("replicas"))
        .or_else(|| body.get("replicas"))
        .and_then(|r| r.as_i64());
    let replicas = desired.unwrap_or_else(|| {
        deploy
            .spec
            .as_ref()
            .and_then(|s| s.replicas)
            .unwrap_or(1) as i64
    }) as i32;
    if let Some(spec) = deploy.spec.as_mut() {
        spec.replicas = Some(replicas);
    }
    state
        .apply_and_broadcast(AnyResource::Deployment(deploy.clone()))
        .await
        .ok();
    let selector = deploy
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref())
        .map(|l| {
            l.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        });
    Ok(Json(
        serde_json::to_value(&Scale {
            metadata: Some(ObjectMeta {
                name: Some(name),
                namespace: Some(namespace),
                uid: Some(format!(
                    "Deployment/{}/{}",
                    deploy.metadata.namespace.as_deref().unwrap_or("default"),
                    deploy.metadata.name.as_deref().unwrap_or("unknown")
                )),
                ..Default::default()
            }),
            spec: Some(ScaleSpec {
                replicas: Some(replicas),
            }),
            status: Some(ScaleStatus {
                replicas,
                selector,
            }),
        })
        .unwrap_or_default(),
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale",
        patch(patch_deployment_scale),
    )
}
