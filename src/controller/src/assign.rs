use z8s_core::store::StoreBackend;
use z8s_core::types::{AnyResource, Resource};

use crate::index::NodeIndex;

pub async fn pick_least_loaded(index: &NodeIndex) -> Option<String> {
    index
        .snapshot()
        .values()
        .min_by(|a, b| {
            a.score()
                .partial_cmp(&b.score())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|l| l.node_name.clone())
}

pub async fn assign_unassigned_pods(
    store: &dyn StoreBackend,
    index: &mut NodeIndex,
    _node_name: &str,
    is_leader: bool,
) -> anyhow::Result<usize> {
    if !is_leader {
        return Ok(0);
    }

    let unassigned = store.get_unassigned("Pod").await;
    if unassigned.is_empty() {
        return Ok(0);
    }

    let nodes = store.get_by_kind("Node").await;
    for record in &nodes {
        if let AnyResource::Node(node) = &record.spec {
            index.upsert_node(node.name());
            if let Some(ref status) = node.status {
                let cpu = status
                    .allocatable
                    .as_ref()
                    .and_then(|a| a.get("cpu"))
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                let mem = status
                    .allocatable
                    .as_ref()
                    .and_then(|a| a.get("memory"))
                    .and_then(|v| parse_memory(v).ok())
                    .unwrap_or(0);
                index.set_capacity(node.name(), cpu, mem);
            }
        }
    }

    let mut assigned = 0;
    for record in &unassigned {
        let Some(node) = pick_least_loaded(index).await else {
            tracing::warn!("no available nodes for pod {}", record.name());
            continue;
        };

        match store.assign_node(record.uid(), &node).await {
            Ok(()) => {
                tracing::info!(
                    "assigned pod {} ({}) -> {}",
                    record.name(),
                    record.uid(),
                    node
                );
                index.increment(&node);
                assigned += 1;
            }
            Err(e) => {
                tracing::error!(
                    "failed to assign pod {} -> {}: {}",
                    record.name(),
                    node,
                    e
                );
            }
        }
    }

    Ok(assigned)
}

pub async fn reassign_dead_node_pods(
    store: &dyn StoreBackend,
    index: &mut NodeIndex,
) -> anyhow::Result<usize> {
    let all_nodes = store.get_by_kind("Node").await;
    let mut dead_nodes = Vec::new();
    for record in &all_nodes {
        if let AnyResource::Node(node) = &record.spec {
            let is_ready = node
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .map(|conds| {
                    conds.iter().any(|c| c.type_ == "Ready" && c.status == "True")
                })
                .unwrap_or(false);
            if is_ready {
                index.upsert_node(node.name());
            } else {
                dead_nodes.push(node.name().to_string());
            }
        }
    }

    if dead_nodes.is_empty() {
        return Ok(0);
    }

    for dead in &dead_nodes {
        index.remove_node(dead);
    }

    let all_pods = store.get_by_kind("Pod").await;
    let mut reassigned = 0;
    for record in &all_pods {
        if let Some(ref assigned) = record.assigned_node
            && dead_nodes.contains(assigned)
            && let Some(node) = pick_least_loaded(index).await
        {
            match store.assign_node(record.uid(), &node).await {
                Ok(()) => {
                    tracing::info!(
                        "reassigned pod {} from dead node {} -> {}",
                        record.name(),
                        assigned,
                        node
                    );
                    index.increment(&node);
                    reassigned += 1;
                }
                Err(e) => {
                    tracing::error!(
                        "failed to reassign pod {} from {}: {}",
                        record.name(),
                        assigned,
                        e
                    );
                }
            }
        }
    }

    Ok(reassigned)
}

#[allow(clippy::result_unit_err)]
pub fn parse_memory(v: &str) -> Result<i64, ()> {
    let v = v.trim();
    if let Some(num) = v.strip_suffix("Ki") {
        num.parse::<i64>().map(|n| n * 1024).map_err(|_| ())
    } else if let Some(num) = v.strip_suffix("Mi") {
        num.parse::<i64>().map(|n| n * 1024 * 1024).map_err(|_| ())
    } else if let Some(num) = v.strip_suffix("Gi") {
        num.parse::<i64>().map(|n| n * 1024 * 1024 * 1024).map_err(|_| ())
    } else {
        v.parse::<i64>().map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_least_loaded_empty_returns_none() {
        let idx = NodeIndex::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(pick_least_loaded(&idx));
        assert!(result.is_none());
    }

    #[test]
    fn pick_least_loaded_single_node() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("only");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(pick_least_loaded(&idx));
        assert_eq!(result.unwrap(), "only");
    }

    #[test]
    fn pick_least_loaded_selects_fewest_pods() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("heavy");
        idx.upsert_node("light");
        idx.increment("heavy");
        idx.increment("heavy");
        idx.increment("heavy");
        idx.increment("light");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let chosen = rt.block_on(pick_least_loaded(&idx)).unwrap();
        assert_eq!(chosen, "light");
    }

    #[test]
    fn pick_least_loaded_prefers_lower_cpu_usage() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("a");
        idx.upsert_node("b");
        idx.set_capacity("a", 4000, 8_000_000_000);
        idx.set_capacity("b", 4000, 8_000_000_000);
        idx.increment("a");
        idx.increment("b");
        idx.add_resource_usage("a", 3000, 2_000_000_000);
        idx.add_resource_usage("b", 500, 2_000_000_000);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let chosen = rt.block_on(pick_least_loaded(&idx)).unwrap();
        assert_eq!(chosen, "b");
    }

    #[test]
    fn parse_memory_plain_number() {
        assert_eq!(parse_memory("1073741824"), Ok(1073741824));
    }

    #[test]
    fn parse_memory_kibibytes() {
        assert_eq!(parse_memory("1024Ki"), Ok(1024 * 1024));
    }

    #[test]
    fn parse_memory_mebibytes() {
        assert_eq!(parse_memory("512Mi"), Ok(512 * 1024 * 1024));
    }

    #[test]
    fn parse_memory_gibibytes() {
        assert_eq!(parse_memory("4Gi"), Ok(4 * 1024 * 1024 * 1024));
    }

    #[test]
    fn parse_memory_whitespace() {
        assert_eq!(parse_memory("  128Mi  "), Ok(128 * 1024 * 1024));
    }

    #[test]
    fn parse_memory_invalid() {
        assert!(parse_memory("abc").is_err());
        assert!(parse_memory("").is_err());
        assert!(parse_memory("Mi").is_err());
    }

    #[tokio::test]
    async fn assign_unassigned_not_leader_is_noop() {
        let store = z8s_core::store::MemoryBackend::new();
        let mut idx = NodeIndex::new();
        let result = assign_unassigned_pods(&store, &mut idx, "node-1", false)
            .await
            .unwrap();
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn assign_unassigned_empty_store() {
        let store = z8s_core::store::MemoryBackend::new();
        let mut idx = NodeIndex::new();
        let result = assign_unassigned_pods(&store, &mut idx, "node-1", true)
            .await
            .unwrap();
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn assign_unassigned_pods_to_least_loaded() {
        let store = z8s_core::store::MemoryBackend::new();

        let node_a = z8s_core::types::Node {
            metadata: z8s_core::types::ObjectMeta::named("node-a"),
            ..Default::default()
        };
        let node_b = z8s_core::types::Node {
            metadata: z8s_core::types::ObjectMeta::named("node-b"),
            ..Default::default()
        };
        store
            .write_spec(node_a.into_any(), None)
            .await
            .unwrap();
        store
            .write_spec(node_b.into_any(), None)
            .await
            .unwrap();

        let pod1 = z8s_core::types::Pod {
            metadata: z8s_core::types::ObjectMeta::new("web-1", "default"),
            spec: Some(z8s_core::types::PodSpec {
                containers: vec![z8s_core::types::ContainerSpec {
                    name: "main".into(),
                    image: "nginx".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let pod2 = z8s_core::types::Pod {
            metadata: z8s_core::types::ObjectMeta::new("web-2", "default"),
            spec: Some(z8s_core::types::PodSpec {
                containers: vec![z8s_core::types::ContainerSpec {
                    name: "main".into(),
                    image: "nginx".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        store
            .write_spec(pod1.into_any(), None)
            .await
            .unwrap();
        store
            .write_spec(pod2.into_any(), None)
            .await
            .unwrap();

        let unassigned_before = store.get_unassigned("Pod").await;
        assert_eq!(unassigned_before.len(), 2);

        let mut idx = NodeIndex::new();
        let assigned = assign_unassigned_pods(&store, &mut idx, "node-1", true)
            .await
            .unwrap();
        assert_eq!(assigned, 2);

        let unassigned_after = store.get_unassigned("Pod").await;
        assert_eq!(unassigned_after.len(), 0);

        let all = store.get_all().await;
        let pods: Vec<_> = all.iter().filter(|r| r.kind() == "Pod").collect();
        for pod in &pods {
            assert!(pod.assigned_node.is_some());
        }

        assert_eq!(idx.total_pods(), 2);
    }

    #[tokio::test]
    async fn reassign_dead_node_pods_no_dead_nodes() {
        let store = z8s_core::store::MemoryBackend::new();
        let mut idx = NodeIndex::new();

        let node = z8s_core::types::Node {
            metadata: z8s_core::types::ObjectMeta::named("node-1"),
            status: Some(z8s_core::types::NodeStatus {
                conditions: Some(vec![z8s_core::types::NodeCondition {
                    type_: "Ready".into(),
                    status: "True".into(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        store.write_spec(node.into_any(), None).await.unwrap();

        let result = reassign_dead_node_pods(&store, &mut idx)
            .await
            .unwrap();
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn reassign_dead_node_pods_moves_pods() {
        let store = z8s_core::store::MemoryBackend::new();

        let dead_node = z8s_core::types::Node {
            metadata: z8s_core::types::ObjectMeta::named("dead-node"),
            status: Some(z8s_core::types::NodeStatus {
                conditions: Some(vec![z8s_core::types::NodeCondition {
                    type_: "Ready".into(),
                    status: "False".into(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let alive_node = z8s_core::types::Node {
            metadata: z8s_core::types::ObjectMeta::named("alive-node"),
            status: Some(z8s_core::types::NodeStatus {
                conditions: Some(vec![z8s_core::types::NodeCondition {
                    type_: "Ready".into(),
                    status: "True".into(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        store
            .write_spec(dead_node.into_any(), None)
            .await
            .unwrap();
        store
            .write_spec(alive_node.into_any(), None)
            .await
            .unwrap();

        let pod = z8s_core::types::Pod {
            metadata: z8s_core::types::ObjectMeta::new("orphan-pod", "default"),
            spec: Some(z8s_core::types::PodSpec {
                containers: vec![z8s_core::types::ContainerSpec {
                    name: "main".into(),
                    image: "nginx".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let pod_any = pod.into_any();
        store
            .write_spec(pod_any, Some("dead-node".into()))
            .await
            .unwrap();

        let mut idx = NodeIndex::new();
        idx.upsert_node("dead-node");
        idx.upsert_node("alive-node");

        let reassigned = reassign_dead_node_pods(&store, &mut idx)
            .await
            .unwrap();
        assert_eq!(reassigned, 1);

        let record = store
            .get_by_kind("Pod")
            .await
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(record.assigned_node.as_deref(), Some("alive-node"));
    }
}
