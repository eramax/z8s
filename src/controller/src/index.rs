use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct NodeIndex {
    loads: HashMap<String, NodeLoad>,
}

#[derive(Debug, Clone)]
pub struct NodeLoad {
    pub node_name: String,
    pub pod_count: usize,
    pub cpu_capacity: i64,
    pub memory_capacity: i64,
    pub cpu_used: i64,
    pub memory_used: i64,
}

impl NodeLoad {
    pub fn new(node_name: &str) -> Self {
        Self {
            node_name: node_name.to_string(),
            pod_count: 0,
            cpu_capacity: 0,
            memory_capacity: 0,
            cpu_used: 0,
            memory_used: 0,
        }
    }

    pub fn score(&self) -> f64 {
        if self.pod_count == 0 {
            return 0.0;
        }
        let cpu_frac = if self.cpu_capacity > 0 {
            self.cpu_used as f64 / self.cpu_capacity as f64
        } else {
            0.5
        };
        let mem_frac = if self.memory_capacity > 0 {
            self.memory_used as f64 / self.memory_capacity as f64
        } else {
            0.5
        };
        (self.pod_count as f64) * 0.5 + cpu_frac * 0.25 + mem_frac * 0.25
    }
}

impl NodeIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_node(&mut self, node_name: &str) {
        self.loads
            .entry(node_name.to_string())
            .or_insert_with(|| NodeLoad::new(node_name));
    }

    pub fn remove_node(&mut self, node_name: &str) {
        self.loads.remove(node_name);
    }

    pub fn increment(&mut self, node_name: &str) {
        if let Some(load) = self.loads.get_mut(node_name) {
            load.pod_count += 1;
        }
    }

    pub fn decrement(&mut self, node_name: &str) {
        if let Some(load) = self.loads.get_mut(node_name) {
            load.pod_count = load.pod_count.saturating_sub(1);
        }
    }

    pub fn set_capacity(&mut self, node_name: &str, cpu: i64, memory: i64) {
        if let Some(load) = self.loads.get_mut(node_name) {
            load.cpu_capacity = cpu;
            load.memory_capacity = memory;
        }
    }

    pub fn add_resource_usage(&mut self, node_name: &str, cpu: i64, memory: i64) {
        if let Some(load) = self.loads.get_mut(node_name) {
            load.cpu_used += cpu;
            load.memory_used += memory;
        }
    }

    pub fn sub_resource_usage(&mut self, node_name: &str, cpu: i64, memory: i64) {
        if let Some(load) = self.loads.get_mut(node_name) {
            load.cpu_used = (load.cpu_used - cpu).max(0);
            load.memory_used = (load.memory_used - memory).max(0);
        }
    }

    pub fn get(&self, node_name: &str) -> Option<&NodeLoad> {
        self.loads.get(node_name)
    }

    pub fn snapshot(&self) -> &HashMap<String, NodeLoad> {
        &self.loads
    }

    pub fn node_count(&self) -> usize {
        self.loads.len()
    }

    pub fn total_pods(&self) -> usize {
        self.loads.values().map(|l| l.pod_count).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_load_new_has_zero_counts() {
        let load = NodeLoad::new("node-1");
        assert_eq!(load.node_name, "node-1");
        assert_eq!(load.pod_count, 0);
        assert_eq!(load.cpu_capacity, 0);
        assert_eq!(load.memory_capacity, 0);
        assert_eq!(load.cpu_used, 0);
        assert_eq!(load.memory_used, 0);
    }

    #[test]
    fn node_load_score_zero_when_empty() {
        let load = NodeLoad::new("node-1");
        assert_eq!(load.score(), 0.0);
    }

    #[test]
    fn node_load_score_increases_with_pods() {
        let mut load = NodeLoad::new("node-1");
        load.pod_count = 3;
        let score_3 = load.score();
        load.pod_count = 5;
        let score_5 = load.score();
        assert!(score_5 > score_3);
    }

    #[test]
    fn node_load_score_factors_cpu_and_memory() {
        let mut light = NodeLoad::new("light");
        light.pod_count = 4;
        light.cpu_capacity = 8000;
        light.memory_capacity = 16_000_000_000;
        light.cpu_used = 1000;
        light.memory_used = 4_000_000_000;

        let mut heavy = NodeLoad::new("heavy");
        heavy.pod_count = 4;
        heavy.cpu_capacity = 8000;
        heavy.memory_capacity = 16_000_000_000;
        heavy.cpu_used = 6000;
        heavy.memory_used = 14_000_000_000;

        assert!(heavy.score() > light.score());
    }

    #[test]
    fn index_new_is_empty() {
        let idx = NodeIndex::new();
        assert_eq!(idx.node_count(), 0);
        assert_eq!(idx.total_pods(), 0);
        assert!(idx.snapshot().is_empty());
    }

    #[test]
    fn upsert_adds_new_node() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("node-1");
        assert_eq!(idx.node_count(), 1);
        assert!(idx.get("node-1").is_some());
    }

    #[test]
    fn upsert_is_idempotent() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("node-1");
        idx.increment("node-1");
        idx.upsert_node("node-1");
        assert_eq!(idx.node_count(), 1);
        assert_eq!(idx.get("node-1").unwrap().pod_count, 1);
    }

    #[test]
    fn remove_node() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("node-1");
        idx.upsert_node("node-2");
        idx.remove_node("node-1");
        assert_eq!(idx.node_count(), 1);
        assert!(idx.get("node-1").is_none());
        assert!(idx.get("node-2").is_some());
    }

    #[test]
    fn increment_decrement() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("node-1");
        assert_eq!(idx.total_pods(), 0);

        idx.increment("node-1");
        idx.increment("node-1");
        idx.increment("node-1");
        assert_eq!(idx.get("node-1").unwrap().pod_count, 3);
        assert_eq!(idx.total_pods(), 3);

        idx.decrement("node-1");
        assert_eq!(idx.get("node-1").unwrap().pod_count, 2);

        idx.decrement("node-1");
        idx.decrement("node-1");
        assert_eq!(idx.get("node-1").unwrap().pod_count, 0);

        idx.decrement("node-1");
        assert_eq!(idx.get("node-1").unwrap().pod_count, 0);
    }

    #[test]
    fn increment_decrement_unknown_node_is_noop() {
        let mut idx = NodeIndex::new();
        idx.increment("nonexistent");
        idx.decrement("nonexistent");
        assert_eq!(idx.total_pods(), 0);
    }

    #[test]
    fn set_capacity() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("node-1");
        idx.set_capacity("node-1", 4000, 8_000_000_000);
        let load = idx.get("node-1").unwrap();
        assert_eq!(load.cpu_capacity, 4000);
        assert_eq!(load.memory_capacity, 8_000_000_000);
    }

    #[test]
    fn set_capacity_unknown_node_is_noop() {
        let mut idx = NodeIndex::new();
        idx.set_capacity("nonexistent", 4000, 8_000_000_000);
        assert!(idx.get("nonexistent").is_none());
    }

    #[test]
    fn add_sub_resource_usage() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("node-1");
        idx.add_resource_usage("node-1", 500, 2_000_000_000);
        let load = idx.get("node-1").unwrap();
        assert_eq!(load.cpu_used, 500);
        assert_eq!(load.memory_used, 2_000_000_000);

        idx.sub_resource_usage("node-1", 200, 1_000_000_000);
        let load = idx.get("node-1").unwrap();
        assert_eq!(load.cpu_used, 300);
        assert_eq!(load.memory_used, 1_000_000_000);
    }

    #[test]
    fn sub_resource_usage_does_not_underflow() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("node-1");
        idx.sub_resource_usage("node-1", 1000, 1000);
        let load = idx.get("node-1").unwrap();
        assert_eq!(load.cpu_used, 0);
        assert_eq!(load.memory_used, 0);
    }

    #[test]
    fn total_pods_across_multiple_nodes() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("a");
        idx.upsert_node("b");
        idx.upsert_node("c");
        idx.increment("a");
        idx.increment("a");
        idx.increment("b");
        idx.increment("c");
        idx.increment("c");
        idx.increment("c");
        assert_eq!(idx.total_pods(), 6);
        assert_eq!(idx.node_count(), 3);
    }

    #[test]
    fn snapshot_returns_all_nodes() {
        let mut idx = NodeIndex::new();
        idx.upsert_node("a");
        idx.upsert_node("b");
        let snap = idx.snapshot();
        assert!(snap.contains_key("a"));
        assert!(snap.contains_key("b"));
        assert_eq!(snap.len(), 2);
    }
}
