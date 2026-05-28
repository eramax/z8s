use std::sync::Arc;
use tokio::time::Duration;

use crate::components::{ComponentRegistry, ReconcileContext};
use crate::scheduler::process::ProcessTracker;

pub struct Reconciler {
    pub registry: Arc<ComponentRegistry>,
    pub ctx: Arc<ReconcileContext>,
    pub process_tracker: Arc<ProcessTracker>,
}

impl Reconciler {
    pub fn new(
        registry: Arc<ComponentRegistry>,
        ctx: Arc<ReconcileContext>,
        process_tracker: Arc<ProcessTracker>,
    ) -> Self {
        Self { registry, ctx, process_tracker }
    }

    pub async fn run(&self) {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        loop {
            ticker.tick().await;
            let reaped = self.process_tracker.reap_zombies();
            if !reaped.is_empty() {
                self.process_tracker.handle_exited_containers(reaped, &self.ctx.store).await;
            }
            self.registry.reconcile_all(&self.ctx).await;
        }
    }
}
