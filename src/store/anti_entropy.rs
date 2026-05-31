use std::collections::HashMap;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, info};

use crate::store::gossip::GossipState;
use crate::store::StoreBackend;

const ANTI_ENTROPY_INTERVAL: Duration = Duration::from_secs(30);

/// Compute a rolling hash over all resources in the store.
pub fn compute_store_hash(state: &GossipState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut keys: Vec<&String> = state.seen.keys().collect();
    keys.sort();
    for key in keys {
        key.hash(&mut hasher);
        if let Some(term) = state.seen.get(key) {
            term.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Run the anti-entropy loop: periodically checks if peers have the same state.
/// Spawned as a background task per peer.
pub async fn run_anti_entropy(
    peer_name: String,
    state: std::sync::Arc<tokio::sync::Mutex<GossipState>>,
) {
    loop {
        sleep(ANTI_ENTROPY_INTERVAL).await;

        let hash = {
            let st = state.lock().await;
            compute_store_hash(&st)
        };
        debug!("Anti-entropy: local hash for {} is {}", peer_name, hash);
        // In a full implementation, this would send the hash to the peer,
        // compare, and request missing keys.
        // For now, this is a placeholder that logs the hash.
    }
}
