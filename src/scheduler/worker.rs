use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::components::ReconcileContext;
use crate::scheduler::process::ProcessTracker;
use crate::store::StoreBackend;
use crate::types::{AnyResource, ResourceState};

const WORKER_INTERVAL: Duration = Duration::from_secs(2);

/// Watch for pods assigned to this node and start them.
/// Runs on every node.
pub async fn run_worker(
    store: Arc<dyn StoreBackend>,
    node_name: String,
    ctx: Arc<ReconcileContext>,
    pt: Arc<ProcessTracker>,
) {
    info!("Worker starting on node {}", node_name);

    loop {
        let pods = store.get_by_kind("Pod").await;

        for t in &pods {
            if let AnyResource::Pod(pod) = &t.resource {
                let assigned = pod.assigned_node.as_deref();

                if assigned == Some(&node_name) {
                    // This pod is assigned to us — ensure it's running
                    let pod_name = pod.metadata.name.as_deref().unwrap_or("unknown");

                    if t.state == ResourceState::Pending || matches!(t.state, ResourceState::Failed(_)) {
                        // Start the pod
                        info!("Worker: starting pod {} on {}", pod_name, node_name);
                        if let Err(e) = pt.start_pod(&t.resource).await {
                            warn!("Worker: failed to start {}: {}", pod_name, e);
                            store.update_state(&t.resource.uid(), ResourceState::Failed(e.to_string())).await;
                        } else {
                            store.update_state(&t.resource.uid(), ResourceState::Running).await;
                        }
                    }

                    // Check if pod is running (process tracker)
                    if !pt.is_running(pod_name).await {
                        // Process died — restart or mark failed
                        if pt.restart_count(pod_name).await > 3 {
                            store.update_state(&t.resource.uid(), ResourceState::Failed("CrashLoopBackOff".into())).await;
                        }
                    }
                }
            }
        }

        sleep(WORKER_INTERVAL).await;
    }
}
