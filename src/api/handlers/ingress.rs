use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn list_ingresses_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "Ingress", "IngressList", None)
        .await
        .into_response()
}

pub async fn list_ingresses(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "Ingress", "IngressList", Some(&ns))
        .await
        .into_response()
}

pub async fn get_ingress(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get_namespaced(&s, "Ingress", &ns, &name).await
}

pub async fn create_ingress(
    State(s): State<AppState>,
    Path(ns): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let ing: Ingress = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid Ingress: {}", e)))?;
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::Ingress(ing), &ns, "Ingress").await
}

pub async fn update_ingress(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_update_namespaced(&s, "Ingress", &ns, &name, &raw).await
}

pub async fn delete_ingress(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "Ingress", &ns, &name).await
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/apis/networking.k8s.io/v1/ingresses",
            get(list_ingresses_all),
        )
        .route(
            "/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses",
            get(list_ingresses).post(create_ingress),
        )
        .route(
            "/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses/{name}",
            get(get_ingress)
                .put(update_ingress)
                .patch(update_ingress)
                .delete(delete_ingress),
        )
}
