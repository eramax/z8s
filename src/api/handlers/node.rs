use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_nodes() -> Json<List<Node>> {
    let time = now_time();
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get().to_string())
        .unwrap_or_else(|_| "1".into());
    let mut labels = BTreeMap::new();
    labels.insert("kubernetes.io/hostname".into(), "z8s-node".into());
    labels.insert("kubernetes.io/os".into(), "linux".into());
    labels.insert("kubernetes.io/arch".into(), detect_arch());
    labels.insert("beta.kubernetes.io/os".into(), "linux".into());
    labels.insert("beta.kubernetes.io/arch".into(), detect_arch());

    let mut capacity = BTreeMap::new();
    capacity.insert("cpu".into(), Quantity(cpu_count.clone()));
    capacity.insert("memory".into(), Quantity(host_memory_ki()));
    capacity.insert("pods".into(), Quantity("110".into()));

    Json(List {
        items: vec![Node {
            metadata: ObjectMeta {
                name: Some("z8s-node".into()),
                uid: Some("z8s-node".into()),
                labels: Some(labels),
                creation_timestamp: Some(time.clone()),
                ..Default::default()
            },
            spec: Some(NodeSpec {
                pod_cidr: Some("10.42.0.0/24".into()),
                pod_cidrs: Some(vec!["10.42.0.0/24".into()]),
                ..Default::default()
            }),
            status: Some(NodeStatus {
                conditions: Some(vec![NodeCondition {
                    type_: "Ready".into(),
                    status: "True".into(),
                    last_heartbeat_time: Some(time.clone()),
                    last_transition_time: Some(time.clone()),
                    reason: Some("KubeletReady".into()),
                    message: Some("z8s is ready".into()),
                    ..Default::default()
                }]),
                addresses: Some(vec![
                    NodeAddress { type_: "InternalIP".into(), address: "127.0.0.1".into() },
                    NodeAddress { type_: "Hostname".into(), address: "z8s-node".into() },
                ]),
                daemon_endpoints: Some(NodeDaemonEndpoints {
                    kubelet_endpoint: Some(DaemonEndpoint { port: z8s_port() as i32 }),
                }),
                node_info: Some(NodeSystemInfo {
                    machine_id: "z8s-1".into(),
                    system_uuid: "z8s-1".into(),
                    boot_id: "z8s-1".into(),
                    kernel_version: kernel_version(),
                    os_image: "Linux".into(),
                    container_runtime_version: format!("z8s://{}", env!("CARGO_PKG_VERSION")),
                    kubelet_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                    kube_proxy_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                    operating_system: "linux".into(),
                    architecture: detect_arch(),
                    swap: None,
                }),
                capacity: Some(capacity.clone()),
                allocatable: Some(capacity),
                ..Default::default()
            }),
        }],
        metadata: make_list_meta(),
    })
}


pub async fn get_node(Path(name): Path<String>) -> Result<Json<Node>, ApiError> {
    if name == "z8s-node" {
        let list = list_nodes().await;
        list.0.items.into_iter().next()
            .map(Json)
            .ok_or_else(|| ApiError::not_found("node not found".into()))
    } else {
        Err(ApiError::not_found(format!("node \"{}\" not found", name)))
    }
}




pub fn kernel_version() -> String {
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|s| s.split_whitespace().nth(2).map(|v| v.to_string()))
        .unwrap_or_else(|| "unknown".into())
}


pub fn host_memory_ki() -> String {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .map(|kb| format!("{}Ki", kb))
        })
        .unwrap_or_else(|| "8192Ki".into())
}



pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/nodes/{name}", get(get_node))
}
