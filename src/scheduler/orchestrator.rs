use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use tokio::time::Duration;

use crate::components::{ComponentRegistry, ReconcileContext, ResourceCategory};
use crate::scheduler::index::OrchestratorIndex;
use crate::scheduler::process::ProcessTracker;
use crate::scheduler::sync_pod::{stop_pod_local, sync_pod, sync_pods};
use crate::scheduler::tasks::Task;
use crate::store::{AnyResource, ResourceTracker};
use crate::store::hub::StoreEventHub;
use crate::store::ops::StoreEvent;
use crate::store::StoreSnapshot;

const TICK_SECS: u64 = 2;
const FULL_SWEEP_EVERY: u64 = 15;

/// Pending reconcile scope accumulated from store events between loop iterations.
#[derive(Default)]
struct PendingWork {
    touched_uids: HashSet<String>,
    deleted: Vec<AnyResource>,
    network: bool,
    compute: bool,
    storage: bool,
    force_full: bool,
}

impl PendingWork {
    fn note_kind(&mut self, kind: &str, uid: String) {
        self.touched_uids.insert(uid);
        match kind {
            "Service" | "EdgeService" | "NetworkPolicy" | "VNet" | "Subnet" | "NSG"
            | "RouteTable" | "Ingress" => self.network = true,
            "Pod" | "Deployment" | "ReplicaSet" | "DaemonSet" | "StatefulSet" | "Job"
            | "CronJob" => self.compute = true,
            "PersistentVolume" | "PersistentVolumeClaim" | "StorageClass" => self.storage = true,
            _ => {}
        }
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

/// Event-driven reconcile loop (O0); dispatches `Task` stubs and delegates to the component registry.
pub struct Orchestrator {
    pub registry: Arc<ComponentRegistry>,
    pub ctx: Arc<ReconcileContext>,
    pub process_tracker: Arc<ProcessTracker>,
    pub notify: Arc<Notify>,
    pub store_events: StoreEventHub,
    pending: Mutex<PendingWork>,
    index: Mutex<OrchestratorIndex>,
    tick_count: Mutex<u64>,
}

impl Orchestrator {
    pub fn new(
        registry: Arc<ComponentRegistry>,
        ctx: Arc<ReconcileContext>,
        process_tracker: Arc<ProcessTracker>,
        store_events: StoreEventHub,
    ) -> Self {
        Self {
            registry,
            ctx,
            process_tracker,
            notify: Arc::new(Notify::new()),
            store_events,
            pending: Mutex::new(PendingWork::default()),
            index: Mutex::new(OrchestratorIndex::default()),
            tick_count: Mutex::new(0),
        }
    }

    pub async fn run(&self) {
        let snap = self.ctx.store.snapshot().await;
        self.index
            .lock()
            .await
            .rebuild_from_snapshot(&snap);

        let mut events = self.store_events.subscribe();
        let mut ticker = tokio::time::interval(Duration::from_secs(TICK_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Ok(ev) = events.recv() => self.enqueue_store_event(ev).await,
                _ = self.notify.notified() => {}
                _ = ticker.tick() => {}
            }
            self.iteration().await;
        }
    }

    async fn enqueue_store_event(&self, ev: StoreEvent) {
        let kind = ev.kind().to_string();
        let uid = ev.uid();
        let mut p = self.pending.lock().await;
        match &ev {
            StoreEvent::Deleted { resource } => {
                if resource.kind() == "Pod" {
                    p.deleted.push(resource.clone());
                }
                p.note_kind(&kind, uid);
            }
            StoreEvent::Applied { .. } => {
                p.note_kind(&kind, uid);
            }
        }
        drop(p);
        self.index.lock().await.apply_event(&ev);
        self.notify.notify_one();
    }

    async fn iteration(&self) {
        let reaped = self.process_tracker.reap_zombies();
        if !reaped.is_empty() {
            self.process_tracker
                .handle_exited_containers(reaped, &self.ctx.store)
                .await;
        }

        let mut tick = self.tick_count.lock().await;
        *tick += 1;
        let periodic_full = *tick == 1 || *tick % FULL_SWEEP_EVERY == 0;
        drop(tick);

        let work = self.pending.lock().await.take();
        let full = work.force_full || periodic_full;

        for resource in work.deleted {
            if resource.kind() == "Pod" {
                self.dispatch(Task::SyncPod {
                    uid: resource.uid(),
                })
                .await;
                stop_pod_local(&resource, &self.process_tracker, self.ctx.netmux.clone()).await;
            }
        }

        let snap = self.ctx.store.snapshot().await;

        if full {
            self.index
                .lock()
                .await
                .rebuild_from_snapshot(&snap);
            self.dispatch(Task::ReconcileSweep).await;
            self.reconcile_components(&snap, snap.all()).await;
            let local_uids = self.index.lock().await.local_pod_uids();
            let local_pods = snap.filter_uids(local_uids.iter().map(String::as_str));
            sync_pods(&local_pods, self.process_tracker.clone()).await;
            self.sync_network(&snap).await;
            return;
        }

        let index_dirty_network = self.index.lock().await.take_network_dirty();
        if work.network || index_dirty_network {
            self.dispatch(Task::SyncNetwork).await;
            self.sync_network(&snap).await;
        }

        let mut trackers = snap.filter_uids(work.touched_uids.iter().map(String::as_str));
        if work.compute && trackers.is_empty() {
            let local_uids = self.index.lock().await.local_pod_uids();
            trackers = snap.filter_uids(local_uids.iter().map(String::as_str));
        }
        if work.compute {
            let comp_kinds = [
                "Deployment",
                "ReplicaSet",
                "DaemonSet",
                "StatefulSet",
            ];
            for t in snap.by_kinds(&comp_kinds) {
                if !trackers.iter().any(|x| x.resource.uid() == t.resource.uid()) {
                    trackers.push((*t).clone());
                }
            }
        }
        if work.storage {
            for t in snap.by_kinds(&["PersistentVolume", "PersistentVolumeClaim"]) {
                if !trackers.iter().any(|x| x.resource.uid() == t.resource.uid()) {
                    trackers.push((*t).clone());
                }
            }
        }

        if !trackers.is_empty() {
            self.reconcile_components(&snap, &trackers).await;
            let pods: Vec<_> = trackers
                .iter()
                .filter(|t| t.resource.kind() == "Pod")
                .cloned()
                .collect();
            sync_pods(&pods, self.process_tracker.clone()).await;
        } else if work.compute {
            let kinds: Vec<&str> = self
                .registry
                .by_category(ResourceCategory::Compute)
                .into_iter()
                .map(|c| c.kind())
                .filter(|k| *k != "Pod")
                .collect();
            let comp: Vec<_> = snap
                .all()
                .iter()
                .filter(|t| kinds.contains(&t.resource.kind()))
                .cloned()
                .collect();
            self.reconcile_components(&snap, &comp).await;
            let pods: Vec<ResourceTracker> = snap.by_kind("Pod").into_iter().cloned().collect();
            sync_pods(&pods, self.process_tracker.clone()).await;
        }
    }

    /// Component reconcile (network/storage/apps) — pods are handled by `sync_pods`.
    async fn reconcile_components(&self, _snap: &StoreSnapshot, trackers: &[ResourceTracker]) {
        let non_pods: Vec<_> = trackers
            .iter()
            .filter(|t| t.resource.kind() != "Pod")
            .cloned()
            .collect();
        if !non_pods.is_empty() {
            self.registry.reconcile_trackers(&self.ctx, &non_pods).await;
        }
    }

    #[allow(dead_code)]
    async fn sync_pod_uid(&self, uid: &str, snap: &StoreSnapshot) {
        if let Some(tracker) = snap.get(uid) {
            self.dispatch(Task::SyncPod {
                uid: uid.to_string(),
            })
            .await;
            let _ = sync_pod(tracker, self.process_tracker.clone()).await;
        }
    }

    async fn dispatch(&self, task: Task) {
        tracing::trace!(?task, "orchestrator task");
    }

    async fn sync_network(&self, snap: &StoreSnapshot) {
        crate::netmux::sync_network::reconcile_network(
            snap,
            &self.ctx.netmux,
            &self.ctx.store,
            &self.process_tracker,
        )
        .await;
    }
}
