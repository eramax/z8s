use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Result;
use tokio::sync::Semaphore;

use crate::netmux::NetMux;
use crate::scheduler::process::ProcessTracker;
use crate::store::{AnyResource, ResourceState, ResourceTracker};

fn pod_start_semaphore() -> &'static Semaphore {
    static SEM: OnceLock<Semaphore> = OnceLock::new();
    SEM.get_or_init(|| {
        let n = crate::config::get().pod_start_parallelism.max(1);
        Semaphore::new(n)
    })
}

/// Reconcile one pod against CRI (scheduler-owned; components must not call CRI for pods).
pub async fn sync_pod(tracker: &ResourceTracker, process_tracker: Arc<ProcessTracker>) -> Result<()> {
    let AnyResource::Pod(pod) = &tracker.resource else {
        return Ok(());
    };

    let assigned = pod.assigned_node.as_deref().unwrap_or("");
    let local_node = crate::config::get().node_name.as_str();
    if assigned != local_node {
        return Ok(());
    }

    let pod_name = tracker.resource.name();
    let already_running = process_tracker.is_running(pod_name).await;

    if !already_running && tracker.state == ResourceState::Pending {
        let permit = pod_start_semaphore().acquire().await;
        let resource = tracker.resource.clone();
        let pt = process_tracker;
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = pt.start_pod(&resource).await {
                tracing::error!("SyncPod: failed to start {}: {}", resource.name(), e);
            }
        });
    }

    Ok(())
}

/// Stop a pod on this node (network policy cleanup + CRI).
pub async fn stop_pod_local(
    resource: &AnyResource,
    process_tracker: &ProcessTracker,
    netmux: Arc<NetMux>,
) {
    let AnyResource::Pod(pod) = resource else {
        return;
    };
    let assigned = pod.assigned_node.as_deref().unwrap_or("");
    if assigned != crate::config::get().node_name.as_str() {
        return;
    }

    let name = pod.metadata.name.as_deref().unwrap_or("");
    if let Some(ip) = process_tracker.pod_ip(name).await {
        let npc = crate::netmux::np_controller::NetworkPolicyController::new(netmux);
        if let Err(e) = npc.remove_pod(ip).await {
            tracing::warn!("SyncPod: NetworkPolicy remove_pod failed: {}", e);
        }
    }
    process_tracker.stop_pod(resource).await;
}

/// Run SyncPod for all pod trackers in the batch.
pub async fn sync_pods(trackers: &[ResourceTracker], process_tracker: Arc<ProcessTracker>) {
    for tracker in trackers {
        if tracker.resource.kind() != "Pod" {
            continue;
        }
        if let Err(e) = sync_pod(tracker, process_tracker.clone()).await {
            tracing::error!("SyncPod {}: {}", tracker.resource.uid(), e);
        }
    }
}
