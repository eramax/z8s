use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use tokio::time::Duration;

use crate::components::network::service::NetworkManager;
use crate::components::{ComponentRegistry, ReconcileContext, ResourceCategory};
use crate::scheduler::process::ProcessTracker;
use crate::scheduler::tasks::Task;
use crate::store::hub::StoreEventHub;
use crate::store::ops::StoreEvent;
use crate::store::StoreSnapshot;

const TICK_SECS: u64 = 2;
const FULL_SWEEP_EVERY: u64 = 15;

/// Pending reconcile scope accumulated from store events between loop iterations.
#[derive(Default)]
struct PendingWork {
    touched_uids: HashSet<String>,
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
    pub network: Arc<NetworkManager>,
    pub process_tracker: Arc<ProcessTracker>,
    pub notify: Arc<Notify>,
    pub store_events: StoreEventHub,
    pending: Mutex<PendingWork>,
    tick_count: Mutex<u64>,
}

impl Orchestrator {
    pub fn new(
        registry: Arc<ComponentRegistry>,
        ctx: Arc<ReconcileContext>,
        network: Arc<NetworkManager>,
        process_tracker: Arc<ProcessTracker>,
        store_events: StoreEventHub,
    ) -> Self {
        Self {
            registry,
            ctx,
            network,
            process_tracker,
            notify: Arc::new(Notify::new()),
            store_events,
            pending: Mutex::new(PendingWork::default()),
            tick_count: Mutex::new(0),
        }
    }

    pub async fn run(&self) {
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
        p.note_kind(&kind, uid);
        drop(p);
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

        let snap = self.ctx.store.snapshot().await;

        if full {
            self.dispatch(Task::ReconcileSweep).await;
            self.reconcile_all(&snap).await;
            self.network.sync_all_services().await;
            return;
        }

        if work.network {
            self.dispatch(Task::SyncNetwork).await;
            self.network.sync_all_services().await;
        }

        let mut trackers = snap.filter_uids(work.touched_uids.iter().map(String::as_str));
        if work.compute && trackers.is_empty() {
            trackers = snap
                .by_kinds(&[
                    "Pod",
                    "Deployment",
                    "ReplicaSet",
                    "DaemonSet",
                    "StatefulSet",
                ])
                .into_iter()
                .cloned()
                .collect();
        }
        if work.storage {
            for t in snap.by_kinds(&["PersistentVolume", "PersistentVolumeClaim"]) {
                if !trackers.iter().any(|x| x.resource.uid() == t.resource.uid()) {
                    trackers.push((*t).clone());
                }
            }
        }

        if !trackers.is_empty() {
            self.registry.reconcile_trackers(&self.ctx, &trackers).await;
        } else if work.compute {
            self.reconcile_category(&snap, ResourceCategory::Compute).await;
        }
    }

    async fn reconcile_all(&self, snap: &StoreSnapshot) {
        self.registry.reconcile_trackers(&self.ctx, snap.all()).await;
    }

    async fn reconcile_category(&self, snap: &StoreSnapshot, cat: ResourceCategory) {
        let kinds: Vec<&str> = self
            .registry
            .by_category(cat)
            .into_iter()
            .map(|c| c.kind())
            .collect();
        let trackers: Vec<_> = snap
            .all()
            .iter()
            .filter(|t| kinds.contains(&t.resource.kind()))
            .cloned()
            .collect();
        self.registry.reconcile_trackers(&self.ctx, &trackers).await;
    }

    async fn dispatch(&self, task: Task) {
        tracing::trace!(?task, "orchestrator task");
    }
}
