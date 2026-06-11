use std::collections::HashMap;

use crate::store::ResourceTracker;

/// Point-in-time view of the store for a single reconcile pass (one `get_all`).
#[derive(Debug, Clone)]
pub struct StoreSnapshot {
    by_uid: HashMap<String, ResourceTracker>,
    trackers: Vec<ResourceTracker>,
}

impl StoreSnapshot {
    pub fn from_trackers(trackers: Vec<ResourceTracker>) -> Self {
        let by_uid = trackers
            .iter()
            .map(|t| (t.resource.uid(), t.clone()))
            .collect();
        Self { by_uid, trackers }
    }

    pub fn empty() -> Self {
        Self {
            by_uid: HashMap::new(),
            trackers: vec![],
        }
    }

    pub fn len(&self) -> usize {
        self.trackers.len()
    }

    pub fn all(&self) -> &[ResourceTracker] {
        &self.trackers
    }

    pub fn get(&self, uid: &str) -> Option<&ResourceTracker> {
        self.by_uid.get(uid)
    }

    pub fn by_kind(&self, kind: &str) -> Vec<&ResourceTracker> {
        self.trackers
            .iter()
            .filter(|t| t.resource.kind() == kind)
            .collect()
    }

    pub fn by_kinds<'a>(&self, kinds: &[&str]) -> Vec<&ResourceTracker> {
        self.trackers
            .iter()
            .filter(|t| kinds.contains(&t.resource.kind()))
            .collect()
    }

    pub fn filter_uids<'a>(&self, uids: impl IntoIterator<Item = &'a str>) -> Vec<ResourceTracker> {
        uids.into_iter()
            .filter_map(|uid| self.by_uid.get(uid).cloned())
            .collect()
    }
}
