/// Work units drained by the orchestrator loop (see docs/plan/05-scheduler.md).
#[derive(Debug, Clone)]
pub enum Task {
    AssignPod { uid: String },
    ReassignPodsOnNode { node: String },
    ReconcileDeployment { uid: String },
    SyncPod { uid: String },
    SyncNetwork,
    ProvisionVolume { pvc_uid: String },
    DeprovisionVolume { pv_uid: String },
    UpdatePodStatus { uid: String },
    ReapZombies,
    ReconcileSweep,
}
