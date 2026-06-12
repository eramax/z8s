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
            if !is_ready {
                dead_nodes.push(node.name().to_string());
            }
        }
    }

    if dead_nodes.is_empty() {
        return Ok(0);
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
