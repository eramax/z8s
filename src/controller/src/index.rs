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
