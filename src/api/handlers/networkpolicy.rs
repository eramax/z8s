use crate::api::server::*;
use axum::Router;
use axum::routing::get;

pub async fn list_networkpolicies_all(State(s): State<AppState>) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "NetworkPolicy", "NetworkPolicyList", None)
        .await
        .into_response()
}

pub async fn list_networkpolicies(
    State(s): State<AppState>,
    Path(ns): Path<String>,
) -> axum::response::Response {
    crate::api::handlers::crd::generic_list_namespaced(&s, "NetworkPolicy", "NetworkPolicyList", Some(&ns))
        .await
        .into_response()
}

pub async fn get_networkpolicy(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_get_namespaced(&s, "NetworkPolicy", &ns, &name).await
}

pub async fn create_networkpolicy(
    State(s): State<AppState>,
    Path(ns): Path<String>,
    raw: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    let body = parse_body(&raw)?;
    let np: NetworkPolicy = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid NetworkPolicy: {}", e)))?;
    crate::api::handlers::crd::generic_create_namespaced(&s, AnyResource::NetworkPolicy(np), &ns, "NetworkPolicy").await
}

pub async fn update_networkpolicy(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::api::handlers::crd::generic_update_namespaced(&s, "NetworkPolicy", &ns, &name, &raw).await
}

pub async fn delete_networkpolicy(
    State(s): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    crate::api::handlers::crd::generic_delete_namespaced(&s, "NetworkPolicy", &ns, &name).await
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/apis/networking.k8s.io/v1/networkpolicies",
            get(list_networkpolicies_all),
        )
        .route(
            "/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies",
            get(list_networkpolicies).post(create_networkpolicy),
        )
        .route(
            "/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies/{name}",
            get(get_networkpolicy)
                .put(update_networkpolicy)
                .patch(update_networkpolicy)
                .delete(delete_networkpolicy),
        )
}
