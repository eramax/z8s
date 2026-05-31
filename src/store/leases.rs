use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::types::{LeaseRecord, NodeRecord, NodeState};
use crate::store::RedbBackend;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const LEASE_TTL_MS: i64 = 30_000;
const LEASE_RENEW_BEFORE_MS: i64 = 10_000;

/// Run a node's heartbeat loop. Writes a NodeRecord every 5s.
pub async fn run_heartbeat(db: Arc<RedbBackend>, node_name: String, node_ip: String) {
    info!("Heartbeat started for {} ({})", node_name, node_ip);
    loop {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let record = NodeRecord {
            node_name: node_name.clone(),
            node_ip: node_ip.clone(),
            last_seen: now,
            state: NodeState::Active,
            pod_count: 0,
            capacity_pods: 110,
        };
        if let Err(e) = db.write_node(&record).await {
            warn!("Failed to write heartbeat for {}: {}", node_name, e);
        }
        debug!("Heartbeat written for {} at {}", node_name, now);
        sleep(HEARTBEAT_INTERVAL).await;
    }
}

/// Try to acquire or renew the scheduler lease. Returns the current lease.
/// Every node runs this loop; at most one holds the lease at any time.
pub async fn run_lease_loop(db: Arc<RedbBackend>, node_name: String) -> LeaseRecord {
    loop {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;

        let mut lease = db.read_lease().await.unwrap_or(LeaseRecord {
            holder: String::new(),
            epoch: 0,
            expires_at_ms: 0,
            acquired_at_ms: 0,
        });

        let expired = lease.expires_at_ms < now_ms;
        let is_ours = lease.holder == node_name;

        if expired || is_ours {
            // We can (re)acquire or renew
            lease.epoch += 1;
            lease.holder = node_name.clone();
            lease.acquired_at_ms = now_ms;
            lease.expires_at_ms = now_ms + LEASE_TTL_MS;

            if let Err(e) = db.write_lease(&lease).await {
                warn!("Failed to write scheduler lease: {}", e);
                sleep(Duration::from_millis(500)).await;
                continue;
            }

            if expired {
                info!("Acquired scheduler lease (epoch {})", lease.epoch);
            }

            return lease;
        }

        // Lease is held by another node and not expired — wait and retry
        let retry_ms = (lease.expires_at_ms - now_ms + 100).min(2000);
        sleep(Duration::from_millis(retry_ms as u64)).await;
    }
}

/// Renew an already-held lease. Returns the updated lease, or None if lost.
pub async fn renew_lease(db: Arc<RedbBackend>, node_name: &str, current: &LeaseRecord) -> Option<LeaseRecord> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;

    let mut lease = current.clone();
    if lease.holder != node_name {
        return None; // Lost the lease
    }
    lease.epoch += 1;
    lease.acquired_at_ms = now_ms;
    lease.expires_at_ms = now_ms + LEASE_TTL_MS;

    if let Err(e) = db.write_lease(&lease).await {
        warn!("Failed to renew scheduler lease: {}", e);
        return None;
    }
    Some(lease)
}

/// Run lease renewal loop — call after `run_lease_loop` returns a lease.
pub async fn run_lease_renewal(db: Arc<RedbBackend>, node_name: String) {
    // First, acquire the lease
    let mut lease = run_lease_loop(db.clone(), node_name.clone()).await;
    info!("Scheduler lease held by {} (epoch {})", node_name, lease.epoch);

    loop {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let until_expiry = lease.expires_at_ms - now_ms;

        if until_expiry < LEASE_RENEW_BEFORE_MS {
            match renew_lease(db.clone(), &node_name, &lease).await {
                Some(new_lease) => lease = new_lease,
                None => {
                    warn!("Lost scheduler lease, re-acquiring...");
                    lease = run_lease_loop(db.clone(), node_name.clone()).await;
                    info!("Re-acquired scheduler lease (epoch {})", lease.epoch);
                }
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
}
