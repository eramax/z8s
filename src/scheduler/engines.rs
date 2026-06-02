use std::sync::Arc;

use crate::cri::RuntimeProvider;
use crate::netmux::NetMux;
use crate::scheduler::process::ProcessTracker;
use crate::storage::StorageProvisioner;
use crate::store::StoreBackend;

/// Runtime engines owned by the orchestrator (constructed once in `node.rs`).
pub struct EngineSet {
    pub store: Arc<dyn StoreBackend>,
    pub cri: Arc<dyn RuntimeProvider>,
    pub netmux: Arc<NetMux>,
    pub vol: Arc<dyn StorageProvisioner>,
    pub process_tracker: Arc<ProcessTracker>,
    pub node_name: String,
}
