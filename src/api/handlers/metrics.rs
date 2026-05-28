use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn metrics_api_resources() -> Json<APIResourceList> {
    Json(APIResourceList {
        group_version: "metrics.k8s.io/v1beta1".into(),
        resources: vec![
            api_resource("pods", "", true, "PodMetrics", &["get", "list"], &[], &[]),
            api_resource("nodes", "", false, "NodeMetrics", &["get", "list"], &[], &[]),
        ],
    })
}


pub async fn metrics_top_nodes() -> Json<serde_json::Value> {
    let now = now_rfc3339();
    let cpu_usec = cgroup_cpu_usage("z8s");
    let mem_bytes = cgroup_memory_current("z8s");
    Json(serde_json::json!({
        "kind": "NodeMetricsList",
        "apiVersion": "metrics.k8s.io/v1beta1",
        "metadata": { "resourceVersion": "1" },
        "items": [{
            "metadata": {"name": "z8s-node", "creationTimestamp": now},
            "timestamp": now, "window": "1m0s",
            "usage": {
                "cpu": format!("{}n", cpu_usec * 1000),
                "memory": format!("{}Ki", mem_bytes / 1024)
            }
        }]
    }))
}


pub async fn top_pods_all(State(state): State<AppState>) -> Json<serde_json::Value> {
    top_pods_in_ns(state, None).await
}


pub async fn top_pods(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<serde_json::Value> {
    top_pods_in_ns(state, Some(namespace)).await
}


pub async fn top_pods_in_ns(state: AppState, namespace: Option<String>) -> Json<serde_json::Value> {
    let trackers = state.store.get_by_kind("Pod").await;
    let now = now_rfc3339();
    let items: Vec<serde_json::Value> = trackers
        .iter()
        .filter(|t| namespace.as_deref().map_or(true, |ns| t.resource.namespace() == ns))
        .map(|t| {
            let pod_uid = t.resource.uid();
            let cg = sanitize_cg(&pod_uid);
            let mem = cgroup_memory_current(&cg);
            let cpu = cgroup_cpu_usage(&cg);
            serde_json::json!({
                "metadata": {
                    "name": t.resource.name(),
                    "namespace": t.resource.namespace(),
                    "creationTimestamp": now,
                },
                "timestamp": now, "window": "1m0s",
                "containers": [{
                    "name": t.resource.name(),
                    "usage": {
                        "cpu": format!("{}n", cpu * 1000),
                        "memory": format!("{}Ki", mem / 1024),
                    }
                }]
            })
        })
        .collect();
    Json(serde_json::json!({
        "kind": "PodMetricsList",
        "apiVersion": "metrics.k8s.io/v1beta1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}


pub fn cgroup_cpu_usage(cg: &str) -> u64 {
    std::fs::read_to_string(format!("/sys/fs/cgroup/z8s/{}/cpu.stat", cg))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("usage_usec "))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}


pub fn cgroup_memory_current(cg: &str) -> u64 {
    std::fs::read_to_string(format!("/sys/fs/cgroup/z8s/{}/memory.current", cg))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub fn sanitize_cg(name: &str) -> String {
    name.replace(['/', '.', ':'], "_")
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apis/metrics.k8s.io/v1beta1", get(metrics_api_resources))
        .route("/apis/metrics.k8s.io/v1beta1/nodes", get(metrics_top_nodes))
        .route("/apis/metrics.k8s.io/v1beta1/pods", get(top_pods_all))
        .route("/apis/metrics.k8s.io/v1beta1/namespaces/{namespace}/pods", get(top_pods))
}
