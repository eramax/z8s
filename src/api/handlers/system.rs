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
    Json(APIResourceList {
        group_version: "v1".into(),
        resources: vec![
            api_resource(
                "pods",
                "pod",
                true,
                "Pod",
                &["get", "list", "watch", "create", "update", "delete"],
                &["po"],
                &["all"],
            ),
            api_resource(
                "pods/exec",
                "",
                true,
                "PodExecOptions",
                &["create", "get"],
                &[],
                &[],
            ),
            api_resource("pods/log", "", true, "Pod", &["get"], &[], &[]),
            api_resource(
                "namespaces",
                "namespace",
                false,
                "Namespace",
                &["get", "list", "create", "delete"],
                &["ns"],
                &[],
            ),
            api_resource(
                "nodes",
                "node",
                false,
                "Node",
                &["get", "list"],
                &["no"],
                &[],
            ),
            api_resource(
                "services",
                "service",
                true,
                "Service",
                &[
                    "get", "list", "watch", "create", "update", "patch", "delete",
                ],
                &["svc"],
                &[],
            ),
            api_resource(
                "endpoints",
                "endpoints",
                true,
                "Endpoints",
                &["get", "list", "watch"],
                &["ep"],
                &[],
            ),
            api_resource(
                "configmaps",
                "configmap",
                true,
                "ConfigMap",
                &["get", "list", "create", "delete"],
                &["cm"],
                &[],
            ),
            api_resource(
                "secrets",
                "secret",
                true,
                "Secret",
                &["get", "list", "create", "delete"],
                &[],
                &[],
            ),
            api_resource(
                "events",
                "event",
                true,
                "Event",
                &["get", "list", "watch"],
                &["ev"],
                &[],
            ),
            api_resource(
                "persistentvolumes",
                "persistentvolume",
                false,
                "PersistentVolume",
                &["get", "list", "create", "delete"],
                &["pv"],
                &[],
            ),
            api_resource(
                "persistentvolumeclaims",
                "persistentvolumeclaim",
                true,
                "PersistentVolumeClaim",
                &["get", "list", "create", "delete"],
                &["pvc"],
                &[],
            ),
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
            APIGroup {
                name: "z8s.io".into(),
                versions: vec![gvd("z8s.io/v1", "v1")],
                preferred_version: Some(gvd("z8s.io/v1", "v1")),
                server_address_by_client_cidrs: None,
            },
            APIGroup {
                name: "apiextensions.k8s.io".into(),
                versions: vec![gvd("apiextensions.k8s.io/v1", "v1")],
                preferred_version: Some(gvd("apiextensions.k8s.io/v1", "v1")),
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
            &[
                "get", "list", "watch", "create", "update", "patch", "delete",
            ],
            &["deploy"],
            &["all"],
        )],
    })
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
    Json(APIResourceList {
        group_version: "discovery.k8s.io/v1".into(),
        resources: vec![api_resource(
            "endpointslices",
            "endpointslice",
            true,
            "EndpointSlice",
            &["get", "list", "watch"],
            &[],
            &[],
        )],
    })
}

pub async fn api_z8s_v1_resources() -> Json<serde_json::Value> {
    fn col(name: &str, json_path: &str, col_type: &str) -> serde_json::Value {
        serde_json::json!({"name": name, "type": col_type, "jsonPath": json_path, "description": name})
    }
    fn resource_with_cols(
        name: &str,
        singular: &str,
        kind: &str,
        verbs: &[&str],
        short: &[&str],
        cols: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let mut r = serde_json::json!({
            "name": name, "singularName": singular, "namespaced": false, "kind": kind,
            "verbs": verbs, "categories": ["all"],
        });
        if !short.is_empty() {
            r["shortNames"] = serde_json::json!(short);
        }
        if !cols.is_empty() {
            r["additionalPrinterColumns"] = serde_json::json!(cols);
        }
        r
    }
    Json(serde_json::json!({
        "groupVersion": "z8s.io/v1",
        "resources": [
            resource_with_cols("vnets", "vnet", "VNet", &["get","list","create","delete"], &["vn"],
                vec![col("CIDR", ".spec.cidr", "string"),
                     col("Role", ".spec.role", "string"),
                     col("Internet", ".spec.internetAccess", "boolean"),
                     col("Subnets", ".spec._subnetCount", "integer"),
                     col("Pods", ".spec._podCount", "integer"),
                     col("Services", ".spec._serviceCount", "integer"),
                     col("Age", ".metadata.creationTimestamp", "date")]),
            resource_with_cols("subnets", "subnet", "Subnet", &["get","list","create","delete"], &["sn"],
                vec![col("CIDR", ".spec.cidr", "string"),
                     col("VNet", ".spec.vnet", "string"),
                     col("Pods", ".spec._podCount", "integer"),
                     col("Services", ".spec._serviceCount", "integer"),
                     col("Age", ".metadata.creationTimestamp", "date")]),
            resource_with_cols("nsgs", "nsg", "NSG", &["get","list","create","delete"], &["nsg"],
                vec![col("Targets", ".spec._targets", "string"),
                     col("Rules", ".spec._ruleCount", "integer"),
                     col("Allows", ".spec._allowCount", "integer"),
                     col("Denies", ".spec._denyCount", "integer"),
                     col("Age", ".metadata.creationTimestamp", "date")]),
            resource_with_cols("routetables", "routetable", "RouteTable", &["get","list","create","delete"], &["rt"],
                vec![col("Rules", ".spec._ruleCount", "integer"),
                     col("Allows", ".spec._allowCount", "integer"),
                     col("Denies", ".spec._denyCount", "integer"),
                     col("Methods", ".spec._methods", "string"),
                     col("Age", ".metadata.creationTimestamp", "date")]),
        ]
    }))
}

pub async fn api_networking_v1_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "networking.k8s.io/v1".into(),
        resources: vec![
            api_resource(
                "ingresses",
                "ingress",
                true,
                "Ingress",
                &["get", "list", "watch", "create", "update", "delete"],
                &["ing"],
                &["all"],
            ),
            api_resource(
                "networkpolicies",
                "networkpolicy",
                true,
                "NetworkPolicy",
                &["get", "list", "watch", "create", "update", "delete"],
                &["netpol"],
                &["all"],
            ),
        ],
    })
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
    Json(APIResourceList {
        group_version: "storage.k8s.io/v1".into(),
        resources: vec![api_resource(
            "storageclasses",
            "storageclass",
            false,
            "StorageClass",
            &["get", "list"],
            &["sc"],
            &[],
        )],
    })
}

pub async fn self_subject_access_review(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Json<SelfSubjectAccessReview> {
    use crate::api::auth::{api_group_for_resource, AuthzRequest};
    let review: SelfSubjectAccessReview = serde_json::from_slice(&body).unwrap_or_default();
    let user = crate::api::auth::extract_user(&headers, Some(state.process_tracker.tokens.as_ref()))
        .await;

    let mut allowed = !crate::api::auth::has_any_rbac_policy(state.store.as_ref()).await;
    let mut reason = Some("no RBAC bindings configured".into());

    if let Some(attrs) = review
        .spec
        .resource_attributes
        .as_ref()
    {
        let resource = attrs.resource.as_deref().unwrap_or("");
        let namespace = attrs.namespace.as_deref().unwrap_or("default");
        let verb = attrs.verb.as_deref().unwrap_or("get");
        let api_group = attrs
            .group
            .as_deref()
            .unwrap_or_else(|| api_group_for_resource(resource));
        if !resource.is_empty() {
            let req = AuthzRequest {
                user: &user,
                namespace,
                resource,
                verb,
                api_group,
                name: attrs.name.as_deref(),
            };
            allowed = crate::api::auth::authorize(state.store.as_ref(), &req).await;
            reason = if allowed {
                Some("allowed by RBAC policy".into())
            } else {
                Some("denied by RBAC policy".into())
            };
        }
    }

    Json(SelfSubjectAccessReview {
        metadata: Some(ObjectMeta::default()),
        spec: review.spec,
        status: Some(SubjectAccessReviewStatus {
            allowed,
            reason,
            ..Default::default()
        }),
    })
}

pub async fn openapi_v2(
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let _ = headers;
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
