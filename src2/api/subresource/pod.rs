//! Pod log/exec subresources.

use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn get_pod_log(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<String, ApiError> {
    let trackers = state.store.get_by_kind("Pod").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            let containers = crate::store::extract_containers(&t.resource);
            if let Some(container) = containers.first() {
                let logs = state.process_tracker.get_logs(&name, &container.name).await;
                return Ok(logs.join("\n"));
            }
            return Err(ApiError::bad_request("no containers in pod".into()));
        }
    }
    Err(ApiError::not_found(format!("pod \"{name}\" not found")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/namespaces/{namespace}/pods/{name}/log",
            get(get_pod_log),
        )
        .route(
            "/api/v1/namespaces/{namespace}/pods/{name}/exec",
            get(crate::cri::exec::exec_handler).post(crate::cri::exec::exec_post_handler),
        )
}
