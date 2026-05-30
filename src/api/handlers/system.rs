use axum::Router;
use axum::routing::{get, post};
use crate::api::server::*;

pub async fn root_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "paths": ["/api", "/apis", "/openapi/v2", "/healthz", "/readyz", "/livez", "/version"]
    }))
}


pub async fn api_versions() -> Json<APIVersions> {
    Json(APIVersions {
        versions: vec!["v1".into()],
        server_address_by_client_cidrs: vec![],
    })
}


pub async fn api_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "v1".into(),
        resources: vec![
            api_resource("pods", "pod", true, "Pod", &["get", "list", "watch", "create", "update", "delete"], &["po"], &["all"]),
            api_resource("pods/exec", "", true, "PodExecOptions", &["create", "get"], &[], &[]),
            api_resource("pods/log", "", true, "Pod", &["get"], &[], &[]),
            api_resource("namespaces", "namespace", false, "Namespace", &["get", "list", "create", "delete"], &["ns"], &[]),
            api_resource("nodes", "node", false, "Node", &["get", "list"], &["no"], &[]),
            api_resource("services", "service", true, "Service", &["get", "list", "watch", "create", "update", "patch", "delete"], &["svc"], &[]),
            api_resource("endpoints", "endpoints", true, "Endpoints", &["get", "list", "watch"], &["ep"], &[]),
            api_resource("configmaps", "configmap", true, "ConfigMap", &["get", "list", "create", "delete"], &["cm"], &[]),
            api_resource("secrets", "secret", true, "Secret", &["get", "list", "create", "delete"], &[], &[]),
            api_resource("events", "event", true, "Event", &["get", "list", "watch"], &["ev"], &[]),
            api_resource("persistentvolumes", "persistentvolume", false, "PersistentVolume", &["get", "list", "create", "delete"], &["pv"], &[]),
            api_resource("persistentvolumeclaims", "persistentvolumeclaim", true, "PersistentVolumeClaim", &["get", "list", "create", "delete"], &["pvc"], &[]),
        ],
    })
}


pub async fn api_groups() -> Json<APIGroupList> {
    Json(APIGroupList {
        groups: vec![
            APIGroup {
                name: "apps".into(),
                versions: vec![gvd("apps/v1", "v1")],
                preferred_version: Some(gvd("apps/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "metrics.k8s.io".into(),
                versions: vec![gvd("metrics.k8s.io/v1beta1", "v1beta1")],
                preferred_version: Some(gvd("metrics.k8s.io/v1beta1", "v1beta1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "authorization.k8s.io".into(),
                versions: vec![gvd("authorization.k8s.io/v1", "v1")],
                preferred_version: Some(gvd("authorization.k8s.io/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "discovery.k8s.io".into(),
                versions: vec![gvd("discovery.k8s.io/v1", "v1")],
                preferred_version: Some(gvd("discovery.k8s.io/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "networking.k8s.io".into(),
                versions: vec![gvd("networking.k8s.io/v1", "v1")],
                preferred_version: Some(gvd("networking.k8s.io/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "storage.k8s.io".into(),
                versions: vec![gvd("storage.k8s.io/v1", "v1")],
                preferred_version: Some(gvd("storage.k8s.io/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
        ],
    })
}


pub async fn api_apps_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "apps/v1".into(),
        resources: vec![api_resource(
            "deployments",
            "deployment",
            true,
            "Deployment",
            &["get", "list", "watch", "create", "update", "patch", "delete"],
            &["deploy"],
            &["all"],
        )],
    })
}


pub async fn api_authz_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "authorization.k8s.io/v1".into(),
        resources: vec![
            api_resource("selfsubjectaccessreviews", "", false, "SelfSubjectAccessReview", &["create"], &[], &[]),
            api_resource("subjectaccessreviews", "", false, "SubjectAccessReview", &["create"], &[], &[]),
        ],
    })
}


pub async fn api_discovery_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "discovery.k8s.io/v1".into(),
        resources: vec![
            api_resource("endpointslices", "endpointslice", true, "EndpointSlice", &["get", "list", "watch"], &[], &[]),
        ],
    })
}

pub async fn api_networking_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "networking.k8s.io/v1".into(),
        resources: vec![
            api_resource("ingresses", "ingress", true, "Ingress", &["get", "list", "watch", "create", "update", "delete"], &["ing"], &["all"]),
            api_resource("networkpolicies", "networkpolicy", true, "NetworkPolicy", &["get", "list", "watch", "create", "update", "delete"], &["netpol"], &["all"]),
        ],
    })
}

pub async fn api_storage_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "storage.k8s.io/v1".into(),
        resources: vec![
            api_resource("storageclasses", "storageclass", false, "StorageClass", &["get", "list"], &["sc"], &[]),
        ],
    })
}




pub async fn self_subject_access_review(
    _body: axum::body::Bytes,
) -> Json<SelfSubjectAccessReview> {
    Json(SelfSubjectAccessReview {
        metadata: ObjectMeta::default(),
        spec: SelfSubjectAccessReviewSpec::default(),
        status: Some(SubjectAccessReviewStatus {
            allowed: true,
            reason: Some("z8s grants all access".into()),
            ..Default::default()
        }),
    })
}


pub async fn openapi_v2(headers: axum::http::HeaderMap) -> Result<axum::response::Response, ApiError> {
    let _ = headers;
    let schema = serde_json::json!({
        "swagger": "2.0",
        "info": {"title": "z8s", "version": env!("CARGO_PKG_VERSION")},
        "paths": {}
    });
    Ok((StatusCode::OK, [("Content-Type", "application/json")], Json(schema)).into_response())
}


pub async fn openapi_v3() -> impl IntoResponse {
    Json(serde_json::json!({
        "openapi": "3.0.0",
        "info": {"title": "z8s", "version": env!("CARGO_PKG_VERSION")},
        "paths": {}
    }))
}


pub async fn version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "major": "0",
        "minor": "1",
        "gitVersion": format!("z8s-v{}", env!("CARGO_PKG_VERSION")),
        "gitCommit": "dev",
        "buildDate": now_rfc3339(),
        "goVersion": "go1.21",
        "compiler": "rustc",
        "platform": format!("linux/{}", detect_arch())
    }))
}


pub async fn healthz() -> &'static str { "ok" }


pub async fn readyz() -> &'static str { "ok" }


pub async fn livez() -> &'static str { "ok" }




pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(root_handler))
        .route("/api", get(api_versions))
        .route("/api/v1", get(api_v1_resources))
        .route("/apis", get(api_groups))
        .route("/apis/apps/v1", get(api_apps_v1_resources))
        .route("/apis/networking.k8s.io/v1", get(api_networking_v1_resources))
        .route("/apis/discovery.k8s.io/v1", get(api_discovery_v1_resources))
        .route("/apis/storage.k8s.io/v1", get(api_storage_v1_resources))
        .route("/apis/authorization.k8s.io/v1", get(api_authz_v1_resources))
        .route("/apis/authorization.k8s.io/v1/selfsubjectaccessreviews", post(self_subject_access_review))
        .route("/apis/authorization.k8s.io/v1/subjectaccessreviews", post(self_subject_access_review))
        .route("/openapi/v2", get(openapi_v2))
        .route("/openapi/v3", get(openapi_v3))
        .route("/version", get(version_handler))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/livez", get(livez))
}
