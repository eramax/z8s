use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct TrackedPod {
    pub uid: String,
    pub name: String,
    pub namespace: String,
    pub pod_ip: String,
    pub pid: u32,
    pub container_ids: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ProcessTrackerInner {
    pods: HashMap<String, TrackedPod>,
}

impl ProcessTrackerInner {
    pub fn insert(&mut self, pod: TrackedPod) {
        self.pods.insert(pod.uid.clone(), pod);
    }

    pub fn remove(&mut self, uid: &str) -> Option<TrackedPod> {
        self.pods.remove(uid)
    }

    pub fn get(&self, uid: &str) -> Option<&TrackedPod> {
        self.pods.get(uid)
    }

    pub fn contains(&self, uid: &str) -> bool {
        self.pods.contains_key(uid)
    }

    pub fn len(&self) -> usize {
        self.pods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pods.is_empty()
    }

    pub fn all_uids(&self) -> Vec<String> {
        self.pods.keys().cloned().collect()
    }

    pub fn pods(&self) -> &HashMap<String, TrackedPod> {
        &self.pods
    }
}

#[derive(Debug, Clone)]
pub struct ProcessTracker {
    inner: Arc<Mutex<ProcessTrackerInner>>,
}

impl ProcessTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProcessTrackerInner::default())),
        }
    }

    pub async fn insert(&self, pod: TrackedPod) {
        self.inner.lock().await.insert(pod);
    }

    pub async fn remove(&self, uid: &str) -> Option<TrackedPod> {
        self.inner.lock().await.remove(uid)
    }

    pub async fn get(&self, uid: &str) -> Option<TrackedPod> {
        self.inner.lock().await.get(uid).cloned()
    }

    pub async fn contains(&self, uid: &str) -> bool {
        self.inner.lock().await.contains(uid)
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }

    pub async fn all_uids(&self) -> Vec<String> {
        self.inner.lock().await.all_uids()
    }
}

impl Default for ProcessTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pod(uid: &str, name: &str) -> TrackedPod {
        TrackedPod {
            uid: uid.to_string(),
            name: name.to_string(),
            namespace: "default".to_string(),
            pod_ip: "10.42.0.5".to_string(),
            pid: 12345,
            container_ids: vec![format!("{}-main", &uid[..8.min(uid.len())])],
        }
    }

    #[tokio::test]
    async fn new_tracker_is_empty() {
        let t = ProcessTracker::new();
        assert!(t.is_empty().await);
        assert_eq!(t.len().await, 0);
        assert!(t.all_uids().await.is_empty());
    }

    #[tokio::test]
    async fn insert_and_get() {
        let t = ProcessTracker::new();
        let pod = make_pod("uid-abc-123", "web");
        t.insert(pod.clone()).await;

        assert!(!t.is_empty().await);
        assert_eq!(t.len().await, 1);
        assert!(t.contains("uid-abc-123").await);

        let got = t.get("uid-abc-123").await.unwrap();
        assert_eq!(got.uid, "uid-abc-123");
        assert_eq!(got.name, "web");
        assert_eq!(got.pod_ip, "10.42.0.5");
        assert_eq!(got.pid, 12345);
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let t = ProcessTracker::new();
        assert!(t.get("nope").await.is_none());
        assert!(!t.contains("nope").await);
    }

    #[tokio::test]
    async fn remove_returns_pod() {
        let t = ProcessTracker::new();
        let pod = make_pod("uid-remove", "app");
        t.insert(pod).await;

        let removed = t.remove("uid-remove").await.unwrap();
        assert_eq!(removed.name, "app");
        assert!(t.is_empty().await);
        assert!(!t.contains("uid-remove").await);
    }

    #[tokio::test]
    async fn remove_nonexistent_returns_none() {
        let t = ProcessTracker::new();
        assert!(t.remove("nope").await.is_none());
    }

    #[tokio::test]
    async fn multiple_pods() {
        let t = ProcessTracker::new();
        for i in 0..5 {
            t.insert(make_pod(&format!("uid-{i}"), &format!("pod-{i}"))).await;
        }
        assert_eq!(t.len().await, 5);

        let uids = t.all_uids().await;
        assert_eq!(uids.len(), 5);
        for i in 0..5 {
            assert!(uids.contains(&format!("uid-{i}")));
        }

        t.remove("uid-2").await;
        assert_eq!(t.len().await, 4);
        assert!(!t.contains("uid-2").await);
        assert!(t.contains("uid-3").await);
    }

    #[tokio::test]
    async fn insert_replaces_existing() {
        let t = ProcessTracker::new();
        t.insert(make_pod("uid-same", "first")).await;
        let mut updated = make_pod("uid-same", "second");
        updated.pid = 99999;
        t.insert(updated).await;

        assert_eq!(t.len().await, 1);
        let got = t.get("uid-same").await.unwrap();
        assert_eq!(got.name, "second");
        assert_eq!(got.pid, 99999);
    }
}
