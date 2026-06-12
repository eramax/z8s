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
