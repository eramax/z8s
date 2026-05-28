use axum::Router;
use axum::routing::get;
use crate::api::server::*;
use k8s_openapi::api::storage::v1::StorageClass as K8sStorageClass;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::collections::BTreeMap;

pub async fn list_storage_classes(
) -> Json<List<K8sStorageClass>> {
    let items: Vec<K8sStorageClass> = crate::storage::StorageClass::builtin().into_iter().map(|sc| {
        K8sStorageClass {
            metadata: ObjectMeta {
                name: Some(sc.name),
                ..Default::default()
            },
            provisioner: sc.provisioner.to_string(),
            reclaim_policy: Some("Delete".to_string()),
            volume_binding_mode: Some("Immediate".to_string()),
            ..Default::default()
        }
    }).collect();
    Json(List { items, metadata: make_list_meta() })
}

pub async fn get_storage_class(
    Path(name): Path<String>,
) -> Result<Json<K8sStorageClass>, ApiError> {
    let sc = crate::storage::StorageClass::by_name(&name)
        .ok_or_else(|| ApiError::not_found(format!("storageclass \"{}\" not found", name)))?;
    Ok(Json(K8sStorageClass {
        metadata: ObjectMeta {
            name: Some(sc.name),
            ..Default::default()
        },
        provisioner: sc.provisioner.to_string(),
        reclaim_policy: Some("Delete".to_string()),
        volume_binding_mode: Some("Immediate".to_string()),
        ..Default::default()
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/storage.k8s.io/v1/storageclasses", get(list_storage_classes))
        .route("/apis/storage.k8s.io/v1/storageclasses/{name}", get(get_storage_class))
}
