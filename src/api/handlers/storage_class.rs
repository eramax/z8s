use crate::api::server::*;
use crate::types::{ObjectMeta, StorageClass as K8sStorageClass};
use axum::Router;
use axum::routing::get;

fn sc_meta(name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.into()),
        creation_timestamp: Some(now_time()),
        ..Default::default()
    }
}

fn builtin_classes() -> Vec<K8sStorageClass> {
    vec![
        K8sStorageClass {
            metadata: sc_meta("standard"),
            provisioner: "z8s.io/loop".into(),
            reclaim_policy: Some("Delete".into()),
            volume_binding_mode: Some("Immediate".into()),
            ..Default::default()
        },
        K8sStorageClass {
            metadata: sc_meta("hostpath"),
            provisioner: "z8s.io/hostpath".into(),
            reclaim_policy: Some("Delete".into()),
            volume_binding_mode: Some("Immediate".into()),
            ..Default::default()
        },
    ]
}

pub async fn list_storage_classes() -> Json<List<K8sStorageClass>> {
    let items = builtin_classes();
    Json(List {
        kind: Some("StorageClassList".into()),
        api_version: None,
        items,
        metadata: make_list_meta(),
    })
}

pub async fn get_storage_class(
    Path(name): Path<String>,
) -> Result<Json<K8sStorageClass>, ApiError> {
    builtin_classes()
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
