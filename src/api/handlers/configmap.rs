use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn list_configmaps_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "ConfigMap", "ConfigMapList", None)
        .await
        .into_response()
}

pub async fn list_configmaps(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "ConfigMap", "ConfigMapList", Some(&ns))
        .await
        .into_response()
}

pub async fn get_configmap(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get_namespaced(&s, "ConfigMap", &ns, &name).await
}

pub async fn create_configmap(
    State(s): State<AppState>,
    Path(ns): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let cm: ConfigMap = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid ConfigMap: {}", e)))?;
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::ConfigMap(cm), &ns, "ConfigMap").await
}

pub async fn update_configmap(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_update_namespaced(&s, "ConfigMap", &ns, &name, &raw).await
}

pub async fn delete_configmap(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "ConfigMap", &ns, &name).await
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/configmaps", get(list_configmaps_all))
        .route(
            "/api/v1/namespaces/{namespace}/configmaps",
            get(list_configmaps).post(create_configmap),
        )
        .route(
            "/api/v1/namespaces/{namespace}/configmaps/{name}",
            get(get_configmap)
                .put(update_configmap)
                .patch(update_configmap)
                .delete(delete_configmap),
        )
}
