//! Runtime tracking and cluster membership records.

use serde::{Deserialize, Serialize};

use super::AnyResource;

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceState {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Terminated,
}

#[derive(Debug, Clone)]
pub struct ResourceTracker {
    pub resource: AnyResource,
    pub state: ResourceState,
    pub last_updated: String,
}

impl ResourceTracker {
    pub fn new(resource: AnyResource) -> Self {
        Self {
            resource,
            state: ResourceState::Pending,
            last_updated: crate::config::now_rfc3339(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// NodeRecord / LeaseRecord — cluster membership and scheduler lease
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeRecord {
    pub node_name: String,
    #[serde(rename = "nodeIP")]
    pub node_ip: String,
    pub last_seen: i64,
    pub state: NodeState,
    pub pod_count: u32,
    pub capacity_pods: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum NodeState {
    Active,
    Dead,
}

impl Default for NodeState {
    fn default() -> Self {
        NodeState::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LeaseRecord {
    pub holder: String,
    pub epoch: u64,
    pub expires_at_ms: i64,
    pub acquired_at_ms: i64,
}
