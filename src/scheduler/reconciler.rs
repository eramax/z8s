use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::Duration;

use crate::components::network::service::NetworkManager;
use crate::components::{ComponentRegistry, ReconcileContext};
use crate::scheduler::process::ProcessTracker;

pub struct Reconciler {
    pub registry: Arc<ComponentRegistry>,
    pub ctx: Arc<ReconcileContext>,
    pub network: Arc<NetworkManager>,
    pub process_tracker: Arc<ProcessTracker>,
    /// External callers (e.g. gossip handler) call `notify.notify_one()` to
    /// wake the reconciler immediately instead of waiting for the 2s ticker.
    pub notify: Arc<Notify>,
}

impl Reconciler {
    pub fn new(
        registry: Arc<ComponentRegistry>,
        ctx: Arc<ReconcileContext>,
        network: Arc<NetworkManager>,
        process_tracker: Arc<ProcessTracker>,
    ) -> Self {
        Self {
            registry,
            ctx,
            network,
            process_tracker,
            notify: Arc::new(Notify::new()),
        }
    }

    pub async fn run(&self) {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = self.notify.notified() => {}
            }
            let reaped = self.process_tracker.reap_zombies();
            if !reaped.is_empty() {
                self.process_tracker
                    .handle_exited_containers(reaped, &self.ctx.store)
                    .await;
            }
            self.registry.reconcile_all(&self.ctx).await;
            self.network.sync_all_services().await;
        }
    }
}
