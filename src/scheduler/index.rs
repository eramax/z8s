use std::collections::{HashMap, HashSet};

use crate::store::ops::StoreEvent;
use crate::store::{AnyResource, StoreSnapshot};

/// In-memory index updated from store events (avoids full-store scans on every tick).
#[derive(Default)]
pub struct OrchestratorIndex {
    pub pods: HashMap<String, PodView>,
    pub unassigned: HashSet<String>,
    pub by_node: HashMap<String, HashSet<String>>,
    pub network_dirty: bool,
}

#[derive(Clone, Debug)]
pub struct PodView {
    pub uid: String,
    pub name: String,
    pub namespace: String,
    pub assigned_node: Option<String>,
}

impl OrchestratorIndex {
    pub fn rebuild_from_snapshot(&mut self, snap: &StoreSnapshot) {
        self.pods.clear();
        self.unassigned.clear();
        self.by_node.clear();
        for t in snap.by_kind("Pod") {
            self.upsert_pod(&t.resource);
        }
    }

    pub fn apply_event(&mut self, ev: &StoreEvent) {
        match ev {
            StoreEvent::Applied { resource, .. } => {
                if resource.kind() == "Pod" {
                    self.upsert_pod(resource);
                } else if matches!(
                    resource.kind(),
                    "Service"
                        | "NetworkPolicy"
                        | "VNet"
                        | "Subnet"
                        | "NSG"
                        | "RouteTable"
                        | "Ingress"
                ) {
                    self.network_dirty = true;
                }
            }
            StoreEvent::Deleted { resource } => match resource.kind() {
                "Pod" => self.remove_pod(&resource.uid()),
                "Service" | "NetworkPolicy" | "VNet" | "Subnet" | "NSG"
                | "RouteTable" | "Ingress" => self.network_dirty = true,
                _ => {}
            },
        }
    }

    fn upsert_pod(&mut self, resource: &AnyResource) {
        let AnyResource::Pod(pod) = resource else {
            return;
        };
        let uid = resource.uid();
        let view = PodView {
            uid: uid.clone(),
            name: resource.name().to_string(),
            namespace: resource.namespace().to_string(),
            assigned_node: pod.assigned_node.clone(),
        };
        self.remove_pod(&uid);
        self.pods.insert(uid.clone(), view);
        match &pod.assigned_node {
            Some(node) if !node.is_empty() => {
                self.by_node.entry(node.clone()).or_default().insert(uid);
            }
            _ => {
                self.unassigned.insert(uid);
            }
        }
    }

    fn remove_pod(&mut self, uid: &str) {
        if let Some(view) = self.pods.remove(uid) {
            self.unassigned.remove(uid);
            if let Some(node) = view.assigned_node {
                if let Some(set) = self.by_node.get_mut(&node) {
                    set.remove(uid);
                    if set.is_empty() {
                        self.by_node.remove(&node);
                    }
                }
            }
        }
    }

    /// Pod uids assigned to this node (for SyncPod batching).
    pub fn local_pod_uids(&self) -> Vec<String> {
        let node = crate::config::get().node_name.clone();
        self.by_node
            .get(&node)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn take_network_dirty(&mut self) -> bool {
        std::mem::take(&mut self.network_dirty)
    }
}
