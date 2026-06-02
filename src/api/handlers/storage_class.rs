use crate::api::server::*;
use crate::store::AnyResource;
use crate::types::StorageClass as K8sStorageClass;
use axum::Router;
use axum::routing::get;

async fn list_from_store(s: &AppState) -> Vec<K8sStorageClass> {
    let mut items = Vec::new();
    for t in s.store.get_by_kind("StorageClass").await {
        if let AnyResource::StorageClass(sc) = t.resource {
            items.push(sc);
        }
    }
    if items.is_empty() {
        return crate::storage::class::default_storage_classes();
    }
    items
}

pub async fn list_storage_classes(State(s): State<AppState>) -> Json<List<K8sStorageClass>> {
    let items = list_from_store(&s).await;
    Json(List {
        kind: Some("StorageClassList".into()),
        api_version: None,
        items,
        metadata: make_list_meta(),
    })
}

pub async fn get_storage_class(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<K8sStorageClass>, ApiError> {
    let uid = format!("StorageClass/{name}");
    if let Some(t) = s.store.get(&uid).await {
        if let AnyResource::StorageClass(sc) = t.resource {
            return Ok(Json(sc));
        }
    }
    list_from_store(&s)
        .await
        .into_iter()
        .find(|c| c.metadata.name.as_deref() == Some(&name))
        .ok_or_else(|| ApiError::not_found(format!("storageclass \"{}\" not found", name)))
        .map(Json)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/apis/storage.k8s.io/v1/storageclasses",
            get(list_storage_classes),
        )
        .route(
            "/apis/storage.k8s.io/v1/storageclasses/{name}",
            get(get_storage_class),
        )
}
