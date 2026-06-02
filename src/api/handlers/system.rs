use crate::api::server::*;
use axum::Router;
use axum::routing::{get, post};

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
    let mut resources = crate::api::discovery::resources_for_api_version("v1");
    resources.push(api_resource(
        "pods/exec",
        "",
        true,
        "PodExecOptions",
        &["create", "get"],
        &[],
        &[],
    ));
    resources.push(api_resource("pods/log", "", true, "Pod", &["get"], &[], &[]));
    Json(APIResourceList {
        group_version: "v1".into(),
        resources,
    })
}

pub async fn api_groups() -> Json<APIGroupList> {
    let mut groups = crate::api::discovery::api_groups_from_catalog();
    groups.push(APIGroup {
        name: "metrics.k8s.io".into(),
        versions: vec![gvd("metrics.k8s.io/v1beta1", "v1beta1")],
        preferred_version: Some(gvd("metrics.k8s.io/v1beta1", "v1beta1")),
        server_address_by_client_cidrs: None,
    });
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    Json(APIGroupList { groups })
}

pub async fn api_apps_v1_resources() -> Json<APIResourceList> {
    Json(crate::api::discovery::api_resource_list("apps/v1"))
}

pub async fn api_rbac_v1_resources() -> Json<APIResourceList> {
    Json(crate::api::discovery::api_resource_list(
        "rbac.authorization.k8s.io/v1",
    ))
}

pub async fn api_authz_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "authorization.k8s.io/v1".into(),
        resources: vec![
            api_resource(
                "selfsubjectaccessreviews",
                "",
                false,
                "SelfSubjectAccessReview",
                &["create"],
                &[],
                &[],
            ),
            api_resource(
                "subjectaccessreviews",
                "",
                false,
                "SubjectAccessReview",
                &["create"],
                &[],
                &[],
            ),
        ],
    })
}

pub async fn api_discovery_v1_resources() -> Json<APIResourceList> {
    Json(crate::api::discovery::api_resource_list("discovery.k8s.io/v1"))
}

pub async fn api_z8s_v1_resources() -> Json<serde_json::Value> {
    Json(crate::api::discovery::z8s_v1_discovery_json())
}

pub async fn api_networking_v1_resources() -> Json<APIResourceList> {
    Json(crate::api::discovery::api_resource_list("networking.k8s.io/v1"))
}

pub async fn api_extensions_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "apiextensions.k8s.io/v1".into(),
        resources: vec![api_resource(
            "customresourcedefinitions",
            "customresourcedefinition",
            false,
            "CustomResourceDefinition",
            &["get", "list", "watch", "create", "update", "delete"],
            &["crd"],
            &[],
        )],
    })
}

pub async fn api_storage_v1_resources() -> Json<APIResourceList> {
    Json(crate::api::discovery::api_resource_list("storage.k8s.io/v1"))
}

pub async fn self_subject_access_review(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Json<SelfSubjectAccessReview> {
    use crate::api::auth::{api_group_for_resource, AuthzRequest};

    let review: SelfSubjectAccessReview = match crate::api::server::parse_body(&body) {
        Ok(val) => serde_json::from_value(val).unwrap_or_default(),
        Err(_) => serde_json::from_slice(&body).unwrap_or_default(),
    };

    let user = crate::api::auth::extract_user(&headers, Some(state.process_tracker.tokens.as_ref()))
        .await;

    let has_policy = crate::api::auth::has_any_rbac_policy(state.store.as_ref()).await;
    let rbac_on = crate::config::rbac_enforced() && has_policy;

    if let Some(attrs) = review.spec.resource_attributes.as_ref() {
        let resource = attrs.resource.as_deref().unwrap_or("");
        let namespace = attrs.namespace.as_deref().unwrap_or("default");
        let verb = attrs.verb.as_deref().unwrap_or("get");
        let api_group = attrs
            .group
            .as_deref()
            .unwrap_or_else(|| api_group_for_resource(resource));

        if resource.is_empty() || !rbac_on {
            return Json(SelfSubjectAccessReview {
                metadata: Some(ObjectMeta::default()),
                spec: review.spec,
                status: Some(SubjectAccessReviewStatus {
                    allowed: true,
                    reason: Some("allowed (no resource or RBAC not enforced)".into()),
                    ..Default::default()
                }),
            });
        }

        let req = AuthzRequest {
            user: &user,
            namespace,
            resource,
            verb,
            api_group,
            name: attrs.name.as_deref(),
        };
        let allowed = crate::api::auth::authorize(state.store.as_ref(), &req).await;
        return Json(SelfSubjectAccessReview {
            metadata: Some(ObjectMeta::default()),
            spec: review.spec,
            status: Some(SubjectAccessReviewStatus {
                allowed,
                reason: Some(if allowed {
                    "allowed by RBAC policy".into()
                } else {
                    "denied by RBAC policy".into()
                }),
                ..Default::default()
            }),
        });
    }

    // Could not parse resource attributes (e.g. CBOR body) — allow by default
    Json(SelfSubjectAccessReview {
        metadata: Some(ObjectMeta::default()),
        spec: review.spec,
        status: Some(SubjectAccessReviewStatus {
            allowed: true,
            reason: Some("allowed (could not parse request body)".into()),
            ..Default::default()
        }),
    })
}

pub async fn openapi_v2(
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    if let Some(accept) = headers.get("accept").and_then(|v| v.to_str().ok()) {
        if accept.contains("protobuf") {
            return Err(ApiError::not_found("openapi v2 protobuf not supported".into()));
        }
    }
    let schema = serde_json::json!({
        "swagger": "2.0",
        "info": {"title": "z8s", "version": env!("CARGO_PKG_VERSION")},
        "paths": {}
    });
    Ok((
        StatusCode::OK,
        [("Content-Type", "application/json")],
        Json(schema),
    )
        .into_response())
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

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn readyz() -> &'static str {
    "ok"
}

pub async fn livez() -> &'static str {
    "ok"
}

fn make_crd(
    name: &str,
    group: &str,
    kind: &str,
    plural: &str,
    singular: &str,
    cols: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "apiextensions.k8s.io/v1",
        "kind": "CustomResourceDefinition",
        "metadata": {"name": format!("{}.{}", plural, group), "uid": "00000000-0000-0000-0000-000000000000", "creationTimestamp": "2026-01-01T00:00:00Z"},
        "spec": {
            "group": group,
            "scope": "Cluster",
            "names": {"kind": kind, "plural": plural, "singular": singular, "shortNames": []},
            "versions": [{
                "name": "v1",
                "served": true,
                "storage": true,
                "schema": {"openAPIV3Schema": {"type": "object", "properties": {"spec": {"type": "object"}, "status": {"type": "object"}}}},
                "additionalPrinterColumns": cols,
            }]
        }
    })
}

fn crd_col(name: &str, json_path: &str, col_type: &str) -> serde_json::Value {
    serde_json::json!({"name": name, "type": col_type, "jsonPath": json_path})
}

fn all_crds() -> Vec<serde_json::Value> {
    vec![
        make_crd(
            "vnets.z8s.io",
            "z8s.io",
            "VNet",
            "vnets",
            "vnet",
            vec![
                crd_col("CIDR", ".spec.cidr", "string"),
                crd_col("Role", ".spec.role", "string"),
                crd_col("Internet", ".spec.internetAccess", "boolean"),
                crd_col("Subnets", ".spec._subnetCount", "integer"),
                crd_col("Pods", ".spec._podCount", "integer"),
                crd_col("Services", ".spec._serviceCount", "integer"),
                crd_col("Age", ".metadata.creationTimestamp", "date"),
            ],
        ),
        make_crd(
            "subnets.z8s.io",
            "z8s.io",
            "Subnet",
            "subnets",
            "subnet",
            vec![
                crd_col("CIDR", ".spec.cidr", "string"),
                crd_col("VNet", ".spec.vnet", "string"),
                crd_col("Pods", ".spec._podCount", "integer"),
                crd_col("Services", ".spec._serviceCount", "integer"),
                crd_col("Age", ".metadata.creationTimestamp", "date"),
            ],
        ),
        make_crd(
            "nsgs.z8s.io",
            "z8s.io",
            "NSG",
            "nsgs",
            "nsg",
            vec![
                crd_col("Targets", ".spec._targets", "string"),
                crd_col("Rules", ".spec._ruleCount", "integer"),
                crd_col("Allows", ".spec._allowCount", "integer"),
                crd_col("Denies", ".spec._denyCount", "integer"),
                crd_col("Age", ".metadata.creationTimestamp", "date"),
            ],
        ),
        make_crd(
            "routetables.z8s.io",
            "z8s.io",
            "RouteTable",
            "routetables",
            "routetable",
            vec![
                crd_col("Rules", ".spec._ruleCount", "integer"),
                crd_col("Allows", ".spec._allowCount", "integer"),
                crd_col("Denies", ".spec._denyCount", "integer"),
                crd_col("Methods", ".spec._methods", "string"),
                crd_col("Age", ".metadata.creationTimestamp", "date"),
            ],
        ),
    ]
}

pub async fn list_crds() -> Json<serde_json::Value> {
    let items: Vec<serde_json::Value> = all_crds();
    Json(
        serde_json::json!({"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinitionList", "items": items, "metadata": {"resourceVersion": "1"}}),
    )
}

pub async fn get_crd(Path(name): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    for crd in all_crds() {
        if crd["metadata"]["name"].as_str() == Some(&name) {
            return Ok(Json(crd));
        }
    }
    Err(ApiError::not_found(format!(
        "customresourcedefinition \"{}\" not found",
        name
    )))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(root_handler))
        .route("/api", get(api_versions))
        .route("/api/v1", get(api_v1_resources))
        .route("/apis", get(api_groups))
        .route("/apis/z8s.io/v1", get(api_z8s_v1_resources))
        .route(
            "/apis/rbac.authorization.k8s.io/v1",
            get(api_rbac_v1_resources),
        )
        .route("/apis/apps/v1", get(api_apps_v1_resources))
        .route(
            "/apis/networking.k8s.io/v1",
            get(api_networking_v1_resources),
        )
        .route("/apis/discovery.k8s.io/v1", get(api_discovery_v1_resources))
        .route("/apis/storage.k8s.io/v1", get(api_storage_v1_resources))
        .route("/apis/authorization.k8s.io/v1", get(api_authz_v1_resources))
        .route(
            "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews",
            post(self_subject_access_review),
        )
        .route(
            "/apis/authorization.k8s.io/v1/subjectaccessreviews",
            post(self_subject_access_review),
        )
        .route(
            "/apis/apiextensions.k8s.io/v1",
            get(api_extensions_v1_resources),
        )
        .route(
            "/apis/apiextensions.k8s.io/v1/customresourcedefinitions",
            get(list_crds),
        )
        .route(
            "/apis/apiextensions.k8s.io/v1/customresourcedefinitions/{name}",
            get(get_crd),
        )
        .route("/openapi/v2", get(openapi_v2))
        .route("/openapi/v3", get(openapi_v3))
        .route("/version", get(version_handler))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/livez", get(livez))
}
