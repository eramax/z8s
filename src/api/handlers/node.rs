use crate::api::server::*;
use crate::store::ResourceTracker;
use axum::Router;
use axum::routing::get;

pub async fn list_nodes(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let local = make_local_node();
    let mut nodes_with_sync: Vec<(Node, Option<chrono::DateTime<chrono::Utc>>)> = vec![(local.clone(), Some(chrono::Utc::now()))];

    // Add nodes from store (gossiped from peers)
    for t in state.store.get_by_kind("Node").await {
        if let AnyResource::Node(n) = &t.resource {
            if !nodes_with_sync.iter().any(|(i, _)| i.metadata.name == n.metadata.name) {
                nodes_with_sync.push((n.clone(), Some(t.last_updated)));
            }
        }
    }

    if accepts_table(&headers) {
        let pods = state.store.get_by_kind("Pod").await;
        let svc_count = state.store.get_by_kind("Service").await.len();
        // Pre-compute running pods
        let running: Vec<String> = futures_util::future::join_all(pods.iter().map(|t| async {
            if let AnyResource::Pod(p) = &t.resource {
                if state
                    .process_tracker
                    .is_running(p.metadata.name.as_deref().unwrap_or(""))
                    .await
                {
                    return p.metadata.name.clone().unwrap_or_default();
                }
            }
            String::new()
        }))
        .await
        .into_iter()
        .filter(|n| !n.is_empty())
        .collect();
        return (
            StatusCode::OK,
            Json(node_list_to_table(&nodes_with_sync, &pods, svc_count, &running)),
        )
            .into_response();
    }

    let items: Vec<Node> = nodes_with_sync.into_iter().map(|(n, _)| n).collect();

    Json(List {
        kind: Some("NodeList".into()),
        api_version: None,
        items,
        metadata: make_list_meta(),
    })
    .into_response()
}

pub async fn get_node(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Node>, ApiError> {
    let local = make_local_node();
    if local.metadata.name.as_deref() == Some(&name) {
        return Ok(Json(local));
    }
    for t in state.store.get_by_kind("Node").await {
        if let AnyResource::Node(n) = t.resource {
            if n.metadata.name.as_deref() == Some(&name) {
                return Ok(Json(n));
            }
        }
    }
    Err(ApiError::not_found(format!("node \"{}\" not found", name)))
}

fn node_list_to_table(
    nodes: &[(Node, Option<chrono::DateTime<chrono::Utc>>)],
    pods: &[ResourceTracker],
    svc_count: usize,
    running: &[String],
) -> serde_json::Value {
    let columns = serde_json::json!([
        {"name": "Name", "type": "string", "format": "name", "priority": 0},
        {"name": "Status", "type": "string", "priority": 0},
        {"name": "Roles", "type": "string", "priority": 0},
        {"name": "Age", "type": "string", "priority": 0},
        {"name": "CPU", "type": "string", "priority": 0},
        {"name": "RAM", "type": "string", "priority": 0},
        {"name": "Pods", "type": "string", "priority": 0},
        {"name": "Svc", "type": "string", "priority": 0},
        {"name": "Last Sync", "type": "string", "priority": 0},
    ]);

    // Pre-count pods per node and total services

    let now = std::time::SystemTime::now();
    let now_utc = chrono::Utc::now();
    let rows: Vec<serde_json::Value> = nodes
        .iter()
        .map(|(node, last_sync)| {
            let name = node.metadata.name.as_deref().unwrap_or("");
            let age = node
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|t| crate::api::server::format_ts_relative(&t.0, now))
                .unwrap_or_else(|| "<unknown>".into());

            let sync_str = last_sync
                .as_ref()
                .map(|t| {
                    let secs = now_utc.signed_duration_since(*t).num_seconds().max(0);
                    if secs < 60 {
                        format!("{}s", secs)
                    } else if secs < 3600 {
                        format!("{}m", secs / 60)
                    } else {
                        format!("{}h", secs / 3600)
                    }
                })
                .unwrap_or_else(|| "<unknown>".into());

            let ready = node
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                .map(|c| {
                    if c.status == "True" {
                        "Ready"
                    } else {
                        "NotReady"
                    }
                })
                .unwrap_or("Unknown");

            let cpu_cap = node
                .status
                .as_ref()
                .and_then(|s| s.capacity.as_ref())
                .and_then(|c| c.get("cpu"))
                .map(|q| {
                    let v: f64 =
                        q.0.trim_end_matches(|c: char| !c.is_ascii_digit())
                            .parse()
                            .unwrap_or(0.0);
                    format!("{}", v as u64)
                })
                .unwrap_or_else(|| "?".into());

            let mem_cap = node
                .status
                .as_ref()
                .and_then(|s| s.capacity.as_ref())
                .and_then(|c| c.get("memory"))
                .map(|q| {
                    let v = parse_ki(&q.0);
                    if v >= 1024 * 1024 * 1024 {
                        format!("{:.1}Gi", v as f64 / (1024.0 * 1024.0 * 1024.0))
                    } else if v >= 1024 * 1024 {
                        format!("{:.0}Mi", v / (1024 * 1024))
                    } else {
                        format!("{:.0}Ki", v / 1024)
                    }
                })
                .unwrap_or_else(|| "?".into());

            let total = pods
                .iter()
                .filter(|t| {
                    if let AnyResource::Pod(p) = &t.resource {
                        let match_name = p.assigned_node.as_deref() == Some(name);
                        if match_name {
                            tracing::info!(
                                "    node {} matched pod {}",
                                name,
                                p.metadata.name.as_deref().unwrap_or("?")
                            );
                        }
                        match_name
                    } else {
                        false
                    }
                })
                .count();
            let ready_pods = pods
                .iter()
                .filter(|t| {
                    if let AnyResource::Pod(p) = &t.resource {
                        p.assigned_node.as_deref() == Some(name)
                            && running
                                .contains(&p.metadata.name.as_deref().unwrap_or("").to_string())
                    } else {
                        false
                    }
                })
                .count();
            let pod_str = format!("{}/{}", ready_pods, total);
            if total > 0 || ready_pods > 0 {
                tracing::info!("    node {} total={} ready={}", name, total, ready_pods);
            }

            serde_json::json!({
                "cells": [name, ready, "worker", age, cpu_cap, mem_cap, pod_str, svc_count.to_string(), sync_str],
                "object": node,
            })
        })
        .collect();

    crate::api::server::make_table(columns, rows)
}

pub fn make_local_node() -> Node {
    let cfg = crate::config::get();
    let time = now_time();
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get().to_string())
        .unwrap_or_else(|_| "1".into());
    let mut labels = BTreeMap::new();
    labels.insert("kubernetes.io/hostname".into(), cfg.node_name.clone());
    labels.insert("kubernetes.io/os".into(), "linux".into());
    labels.insert("kubernetes.io/arch".into(), detect_arch());
    labels.insert("beta.kubernetes.io/os".into(), "linux".into());
    labels.insert("beta.kubernetes.io/arch".into(), detect_arch());

    let mut capacity = BTreeMap::new();
    capacity.insert("cpu".into(), Quantity(cpu_count.clone()));
    capacity.insert("memory".into(), Quantity(host_memory_ki()));
    capacity.insert("pods".into(), Quantity("110".into()));

    Node {
        api_version: "v1".into(),
        kind: "Node".into(),
        metadata: ObjectMeta {
            name: Some(cfg.node_name.clone()),
            uid: Some(cfg.node_name.clone()),
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
                NodeAddress {
                    type_: "InternalIP".into(),
                    address: cfg.node_ip.clone(),
                },
                NodeAddress {
                    type_: "Hostname".into(),
                    address: cfg.node_name.clone(),
                },
            ]),
            daemon_endpoints: Some(NodeDaemonEndpoints {
                kubelet_endpoint: Some(DaemonEndpoint {
                    port: z8s_port() as i32,
                }),
            }),
            node_info: Some(NodeSystemInfo {
                machine_id: format!("z8s-{}", cfg.node_name),
                system_uuid: format!("z8s-{}", cfg.node_name),
                boot_id: format!("z8s-{}", cfg.node_name),
                kernel_version: kernel_version(),
                os_image: "Linux".into(),
                container_runtime_version: format!("z8s://{}", env!("CARGO_PKG_VERSION")),
                kubelet_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                kube_proxy_version: format!("z8s-{}", env!("CARGO_PKG_VERSION")),
                operating_system: "linux".into(),
                architecture: detect_arch(),
                ..Default::default()
            }),
            capacity: Some(capacity.clone()),
            allocatable: Some(capacity),
            ..Default::default()
        }),
    }
}

fn kernel_version() -> String {
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|s| s.split_whitespace().nth(2).map(|v| v.to_string()))
        .unwrap_or_else(|| "unknown".into())
}

fn host_memory_ki() -> String {
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

fn parse_ki(s: &str) -> u64 {
    let s = s.trim();
    if let Some(rest) = s.strip_suffix("Ki") {
        rest.parse().unwrap_or(0) * 1024
    } else if let Some(rest) = s.strip_suffix("Mi") {
        rest.parse().unwrap_or(0) * 1024 * 1024
    } else if let Some(rest) = s.strip_suffix("Gi") {
        rest.parse().unwrap_or(0) * 1024 * 1024 * 1024
    } else {
        s.parse().unwrap_or(0)
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/nodes/{name}", get(get_node))
}
