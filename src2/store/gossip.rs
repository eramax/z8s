use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::store::{AnyResource, ResourceState, StoreBackend};

/// Gossip message types exchanged over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GossipMessage {
    Gossip {
        key: String,
        value: Vec<u8>,
        term: u64,
        source: String,
    },
    BatchGossip {
        entries: Vec<SyncEntry>,
        source: String,
    },
    SyncRequest {
        request_id: u64,
    },
    SyncFull {
        request_id: u64,
        entries: Vec<SyncEntry>,
    },
    Checksum {
        hash: u64,
        keys: Vec<String>,
    },
    KeyRequest {
        keys: Vec<String>,
    },
    KeyResponse {
        entries: Vec<SyncEntry>,
    },
    Heartbeat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntry {
    pub key: String,
    pub value: Vec<u8>,
    pub term: u64,
    #[serde(default)]
    pub state: Option<ResourceState>,
}

/// Tracks known terms per key for deduplication.
pub struct GossipState {
    pub node_name: String,
    pub local_term: u64,
    pub seen: HashMap<String, u64>,
    pub db: Arc<dyn StoreBackend>,
    pub peers: Vec<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    /// Coalesce by resource uid — last write wins before flush.
    pending: HashMap<String, GossipEntry>,
}

pub const GOSSIP_BATCH_MAX: usize = 32;
pub const GOSSIP_FLUSH_MS: u64 = 100;

#[derive(Clone)]
struct GossipEntry {
    key: String,
    value: Vec<u8>,
    term: u64,
    state: Option<ResourceState>,
}

impl GossipState {
    pub fn new(node_name: String, db: Arc<dyn StoreBackend>) -> Self {
        Self {
            node_name,
            local_term: 0,
            seen: HashMap::new(),
            db,
            peers: Vec::new(),
            pending: HashMap::new(),
        }
    }

    /// Add a peer broadcast channel
    pub fn add_peer(&mut self, tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
        self.peers.push(tx);
    }

    /// Queue a resource for batched broadcast (coalesces duplicate keys).
    /// Returns true when the pending map reached `GOSSIP_BATCH_MAX` and caller should flush.
    pub fn queue_write(&mut self, resource: &AnyResource) -> bool {
        self.queue_write_with_state(resource, None)
    }

    /// Queue a resource with explicit ResourceState for gossip.
    pub fn queue_write_with_state(&mut self, resource: &AnyResource, state: Option<ResourceState>) -> bool {
        if self.peers.is_empty() {
            return false;
        }
        if let Ok(value) = serde_json::to_vec(resource) {
            let term = self.next_term();
            let key = resource.uid();
            self.pending.insert(
                key,
                GossipEntry {
                    key: resource.uid(),
                    value,
                    term,
                    state,
                },
            );
        }
        self.pending.len() >= GOSSIP_BATCH_MAX
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Flush all pending entries as a single batched message per peer.
    /// This serializes once and sends once per peer instead of N times.
    pub async fn flush_batch(&mut self) {
        if self.pending.is_empty() || self.peers.is_empty() {
            return;
        }
        let batch: Vec<GossipEntry> = self.pending.drain().map(|(_, e)| e).collect();
        let msg = GossipMessage::BatchGossip {
            entries: batch
                .iter()
                .map(|e| SyncEntry {
                    key: e.key.clone(),
                    value: e.value.clone(),
                    term: e.term,
                    state: e.state.clone(),
                })
                .collect(),
            source: self.node_name.clone(),
        };
        if let Ok(frame) = serde_json::to_vec(&msg) {
            for (i, tx) in self.peers.iter().enumerate() {
                match tx.send(frame.clone()) {
                    Ok(()) => debug!("Flushed batch ({} entries) to peer {}", batch.len(), i),
                    Err(e) => warn!("Flush to peer {} failed: {}", i, e),
                }
            }
        }
    }

    /// Queue then flush immediately (used when batch threshold is hit).
    pub async fn broadcast_write(&mut self, resource: &AnyResource) {
        if self.queue_write(resource) {
            self.flush_batch().await;
        }
    }

    pub fn next_term(&mut self) -> u64 {
        self.local_term += 1;
        self.local_term
    }

    /// Returns true if this message is new (should be applied).
    pub fn dedup(&mut self, key: &str, term: u64) -> bool {
        let known = self.seen.get(key).copied().unwrap_or(0);
        if term >= known {
            self.seen.insert(key.to_string(), term);
            true
        } else {
            false
        }
    }

    /// Apply a gossip message to the local database.
    pub async fn apply(&self, key: &str, value: &[u8], term: u64) {
        if let Ok(resource) = serde_json::from_slice::<AnyResource>(value) {
            if let Err(e) = self.db.apply(resource).await {
                warn!("Failed to apply gossiped resource {}: {}", key, e);
            } else {
                debug!("Applied gossiped resource {} (term {})", key, term);
            }
        }
    }
}

/// Compute a simple checksum over a list of keys and their terms.
pub fn compute_checksum(keys: &[String], terms: &[u64]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (k, t) in keys.iter().zip(terms.iter()) {
        k.hash(&mut hasher);
        t.hash(&mut hasher);
    }
    hasher.finish()
}
