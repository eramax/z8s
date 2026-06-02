use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::store::{AnyResource, StoreBackend};

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
}

/// Tracks known terms per key for deduplication.
pub struct GossipState {
    pub node_name: String,
    pub local_term: u64,
    pub seen: HashMap<String, u64>,
    pub db: Arc<dyn StoreBackend>,
    pub peers: Vec<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    pending: Vec<GossipEntry>,
}

#[derive(Clone)]
struct GossipEntry {
    key: String,
    value: Vec<u8>,
    term: u64,
}

impl GossipState {
    pub fn new(node_name: String, db: Arc<dyn StoreBackend>) -> Self {
        Self {
            node_name,
            local_term: 0,
            seen: HashMap::new(),
            db,
            peers: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Add a peer broadcast channel
    pub fn add_peer(&mut self, tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
        self.peers.push(tx);
    }

    /// Queue a resource for batched broadcast (reduces per-write allocations)
    pub fn queue_write(&mut self, resource: &AnyResource) {
        if self.peers.is_empty() {
            return;
        }
        if let Ok(value) = serde_json::to_vec(resource) {
            let term = self.next_term();
            self.pending.push(GossipEntry {
                key: resource.uid(),
                value,
                term,
            });
        }
    }

    /// Flush all pending entries as a single batched message per peer.
    /// This serializes once and sends once per peer instead of N times.
    pub async fn flush_batch(&mut self) {
        if self.pending.is_empty() || self.peers.is_empty() {
            return;
        }
        let batch: Vec<GossipEntry> = self.pending.drain(..).collect();
        let msg = GossipMessage::BatchGossip {
            entries: batch
                .iter()
                .map(|e| SyncEntry {
                    key: e.key.clone(),
                    value: e.value.clone(),
                    term: e.term,
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

    /// Called after a local write to broadcast to all peers (immediate, unbatched)
    pub async fn broadcast_write(&self, resource: &AnyResource) {
        if self.peers.is_empty() {
            tracing::warn!("broadcast_write: no peers");
            return;
        }
        let key = resource.uid();
        if let Ok(value) = serde_json::to_vec(resource) {
            let msg = GossipMessage::Gossip {
                key: key.clone(),
                value,
                term: 0,
                source: self.node_name.clone(),
            };
            if let Ok(json) = serde_json::to_string(&msg) {
                for (i, tx) in self.peers.iter().enumerate() {
                    match tx.send(json.as_bytes().to_vec()) {
                        Ok(()) => tracing::info!("Broadcast {} to peer {}", key, i),
                        Err(e) => tracing::warn!("Broadcast to peer {} failed: {}", i, e),
                    }
                }
            }
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
