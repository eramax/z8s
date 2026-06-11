//! Integration tests for the API router (extracted from server.rs).

use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

async fn test_router(store: Arc<dyn StoreBackend>) -> axum::Router {
    let cgroup = Arc::new(
        crate::cri::cgroup::CgroupManager::new()
            .unwrap_or_else(|_| crate::cri::cgroup::CgroupManager::new().unwrap()),
    );
    let image = Arc::new(
        crate::cri::image::ImageManager::new()
            .unwrap_or_else(|_| crate::cri::image::ImageManager::new().unwrap()),
    );
    let supervisor = Arc::new(crate::cri::runtime::ProcessSupervisor::new(
        image,
        cgroup.clone(),
        Arc::new(
            crate::netmux::NetMux::new(
                &crate::config::get().pod_cidr,
                &crate::config::get().node_name,
            )
            .unwrap(),
        ),
    ));
    let container_runtime = Arc::new(crate::cri::runtime::ContainerRuntime::new(
        supervisor.clone(),
        cgroup,
    ));
    let process_tracker = Arc::new(ProcessTracker {
        running: supervisor.running.clone(),
        restart_counts: supervisor.restart_counts.clone(),
        cri: container_runtime.clone(),
        store: store.clone(),
        tokens: crate::api::auth::TokenRegistry::new(),
        broadcast_tx: tokio::sync::RwLock::new(None),
    });
    let test_netmux = Arc::new(
        crate::netmux::NetMux::new(
            &crate::config::get().pod_cidr,
            &crate::config::get().node_name,
        )
        .unwrap(),
    );
    let network = Arc::new(crate::components::network::service::NetworkManager::new(
        store.clone(),
        test_netmux.clone(),
    ));
    let pipeline = Arc::new(crate::components::ReconciliationPipeline::builder().build());
    let ctx = Arc::new(crate::components::ReconcileContext {
        store: store.clone(),
        pipeline: pipeline.clone(),
        cri: container_runtime.clone() as Arc<dyn crate::cri::RuntimeProvider>,
        net: network.clone() as Arc<dyn crate::netmux::network::NetworkEngine>,
        process_tracker: process_tracker.clone(),
        vol: Arc::new(crate::storage::ProvisionerDispatcher::new(store.clone()))
            as Arc<dyn crate::storage::StorageProvisioner>,
        netmux: test_netmux.clone(),
    });
    let registry = Arc::new(crate::components::ComponentRegistry::new());
    let state = build_app_state(
        store,
        process_tracker,
        registry,
        ctx,
        None,
        crate::store::StoreEventHub::new(),
        Arc::new(tokio::sync::Notify::new()),
        None,
        false,
    )
    .await;
    build_router(state)
}

pub async fn make_app() -> axum::Router {
    test_router(Arc::new(MemoryBackend::new())).await
}

pub async fn make_app_with_store() -> (axum::Router, Arc<dyn StoreBackend>) {
    let store: Arc<dyn StoreBackend> = Arc::new(MemoryBackend::new());
    let app = test_router(store.clone()).await;
    (app, store)
}

pub fn json_body(body: &str) -> Body {
    Body::from(body.to_string())
}

#[tokio::test]
pub async fn healthz_returns_ok() {
    let app = make_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
pub async fn readyz_returns_ok() {
    let app = make_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
pub async fn version_returns_json() {
    let app = make_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/version")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v.get("gitVersion").is_some());
}

#[tokio::test]
pub async fn create_and_get_configmap() {
    let cm_json = r#"{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"test-cm","namespace":"default"},"data":{"key":"value"}}"#;
    let (app, _) = make_app_with_store().await;
    let create_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/namespaces/default/configmaps")
                .header("content-type", "application/json")
                .body(json_body(cm_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        create_resp.status() == StatusCode::CREATED || create_resp.status() == StatusCode::OK
    );
    let get_resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/default/configmaps/test-cm")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(get_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["metadata"]["name"], "test-cm");
    assert_eq!(v["data"]["key"], "value");
}

#[tokio::test]
pub async fn get_nonexistent_configmap_returns_404() {
    let app = make_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/default/configmaps/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
pub async fn configmaps_in_different_namespaces_do_not_collide() {
    let (app, store) = make_app_with_store().await;
    let cm_a: ConfigMap = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"shared","namespace":"ns-a"},"data":{"env":"production"}
    }))
    .unwrap();
    let cm_b: ConfigMap = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"shared","namespace":"ns-b"},"data":{"env":"staging"}
    }))
    .unwrap();
    store.apply(AnyResource::ConfigMap(cm_a)).await.unwrap();
    store.apply(AnyResource::ConfigMap(cm_b)).await.unwrap();
    assert_eq!(store.get_by_kind("ConfigMap").await.len(), 2);
    let resp_a = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/ns-a/configmaps/shared")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK);
    let body_a = axum::body::to_bytes(resp_a.into_body(), usize::MAX)
        .await
        .unwrap();
    let v_a: serde_json::Value = serde_json::from_slice(&body_a).unwrap();
    assert_eq!(v_a["data"]["env"], "production");
    let resp_b = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/ns-b/configmaps/shared")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);
    let body_b = axum::body::to_bytes(resp_b.into_body(), usize::MAX)
        .await
        .unwrap();
    let v_b: serde_json::Value = serde_json::from_slice(&body_b).unwrap();
    assert_eq!(v_b["data"]["env"], "staging");
}

#[tokio::test]
pub async fn label_selector_filters_pods() {
    let (app, store) = make_app_with_store().await;
    let pod_a: crate::types::Pod = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1","kind":"Pod","metadata":{"name":"pod-a","namespace":"default","labels":{"app":"web"}},
        "spec":{"containers":[{"name":"c","image":"alpine"}]}
    }))
    .unwrap();
    let pod_b: crate::types::Pod = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1","kind":"Pod","metadata":{"name":"pod-b","namespace":"default","labels":{"app":"db"}},
        "spec":{"containers":[{"name":"c","image":"postgres"}]}
    }))
    .unwrap();
    store.apply(AnyResource::Pod(pod_a)).await.unwrap();
    store.apply(AnyResource::Pod(pod_b)).await.unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/default/pods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/default/pods?labelSelector=app%3Dweb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["metadata"]["name"], "pod-a");
}

#[tokio::test]
pub async fn create_service_defaults_target_port() {
    let app = make_app().await;
    let svc_json = r#"{"apiVersion":"v1","kind":"Service","metadata":{"name":"my-svc","namespace":"default"},"spec":{"selector":{"app":"web"},"ports":[{"port":80,"protocol":"TCP"}]}}"#;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/namespaces/default/services")
                .header("content-type", "application/json")
                .body(json_body(svc_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!v["spec"]["ports"][0]["targetPort"].is_null());
}

#[tokio::test]
pub async fn list_services_returns_service_list_kind() {
    let app = make_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/default/services")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["kind"], "ServiceList");
}

#[tokio::test]
pub async fn create_and_list_pv() {
    let app = make_app().await;
    let pv_json = r#"{"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":"pv1"},"spec":{"capacity":{"storage":"5Gi"},"accessModes":["ReadWriteOnce"],"hostPath":{"path":"/data"}}}"#;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/persistentvolumes")
                .header("content-type", "application/json")
                .body(json_body(pv_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/persistentvolumes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["kind"], "PersistentVolumeList");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
pub async fn create_and_list_pvc() {
    let app = make_app().await;
    let pvc_json = r#"{"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":"pvc1","namespace":"default"},"spec":{"accessModes":["ReadWriteOnce"],"resources":{"requests":{"storage":"1Gi"}}}}"#;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/namespaces/default/persistentvolumeclaims")
                .header("content-type", "application/json")
                .body(json_body(pvc_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/default/persistentvolumeclaims")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["kind"], "PersistentVolumeClaimList");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
pub async fn list_pods_scoped_to_namespace() {
    let (app, store) = make_app_with_store().await;
    let pod_default: crate::types::Pod = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1","kind":"Pod","metadata":{"name":"p1","namespace":"default"},"spec":{"containers":[{"name":"c","image":"alpine"}]}
    }))
    .unwrap();
    let pod_other: crate::types::Pod = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1","kind":"Pod","metadata":{"name":"p2","namespace":"other"},"spec":{"containers":[{"name":"c","image":"alpine"}]}
    }))
    .unwrap();
    store.apply(AnyResource::Pod(pod_default)).await.unwrap();
    store.apply(AnyResource::Pod(pod_other)).await.unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/namespaces/default/pods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["metadata"]["name"], "p1");
}

#[tokio::test]
pub async fn unknown_route_returns_404() {
    let app = make_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/not/a/real/path")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
