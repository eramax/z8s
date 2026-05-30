use axum::Router;
use axum::routing::{get, post, delete};
use crate::api::server::*;

pub async fn list_ingresses(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<k8s_openapi::api::networking::v1::Ingress>> {
    let items: Vec<k8s_openapi::api::networking::v1::Ingress> = state.store.get_by_kind("Ingress").await
        .into_iter()
        .filter(|t| t.resource.namespace() == namespace)
        .filter_map(|t| if let AnyResource::Ingress(ing) = t.resource { Some(ing) } else { None })
        .collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn create_ingress(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    Json(mut ing): Json<k8s_openapi::api::networking::v1::Ingress>,
) -> Result<Json<k8s_openapi::api::networking::v1::Ingress>, ApiError> {
    if ing.metadata.namespace.is_none() { ing.metadata.namespace = Some(namespace); }
    let resource = AnyResource::Ingress(ing);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    match resource { AnyResource::Ingress(ing) => Ok(Json(ing)), _ => unreachable!() }
}

pub async fn delete_ingress(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("Ingress").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("ingress \"{}/{}\" not found", namespace, name)))
}

pub async fn list_networkpolicies(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<k8s_openapi::api::networking::v1::NetworkPolicy>> {
    let items: Vec<k8s_openapi::api::networking::v1::NetworkPolicy> = state.store.get_by_kind("NetworkPolicy").await
        .into_iter()
        .filter(|t| t.resource.namespace() == namespace)
        .filter_map(|t| if let AnyResource::NetworkPolicy(np) = t.resource { Some(np) } else { None })
        .collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn create_networkpolicy(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    Json(mut np): Json<k8s_openapi::api::networking::v1::NetworkPolicy>,
) -> Result<Json<k8s_openapi::api::networking::v1::NetworkPolicy>, ApiError> {
    if np.metadata.namespace.is_none() { np.metadata.namespace = Some(namespace); }
    let resource = AnyResource::NetworkPolicy(np);
    state.store.apply(resource.clone()).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    match resource { AnyResource::NetworkPolicy(np) => Ok(Json(np)), _ => unreachable!() }
}

pub async fn delete_networkpolicy(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    let trackers = state.store.get_by_kind("NetworkPolicy").await;
    for t in &trackers {
        if t.resource.namespace() == namespace && t.resource.name() == name {
            state.store.delete(&t.resource).await.ok();
            return Ok(Json(ok_status()));
        }
    }
    Err(ApiError::not_found(format!("networkpolicy \"{}/{}\" not found", namespace, name)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses", get(list_ingresses).post(create_ingress))
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses/{name}", delete(delete_ingress))
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies", get(list_networkpolicies).post(create_networkpolicy))
        .route("/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies/{name}", delete(delete_networkpolicy))
}
