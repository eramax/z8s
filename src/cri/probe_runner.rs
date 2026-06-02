//! One async task per container running all configured probes (CRI plan §6).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::warn;

use crate::cri::health::{
    HealthChecker, HealthStatus, ProbeAction, ProbeConfig,
};

/// Start probe loops for a container; returns ready/healthy atomics shared with the supervisor.
pub fn spawn_container_probes(
    probes: &[ProbeConfig],
    container_id: &str,
    container_port_map: &std::collections::HashMap<u16, u16>,
) -> (Arc<AtomicBool>, Arc<Mutex<bool>>) {
    let ready = Arc::new(AtomicBool::new(probes.is_empty()));
    let healthy = Arc::new(Mutex::new(true));
    if probes.is_empty() {
        return (ready, healthy);
    }

    let p_ready = ready.clone();
    let p_healthy = healthy.clone();
    let cid = container_id.to_string();
    let probes_owned = probes.to_vec();
    let port_map = container_port_map.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut next_fire: Vec<(std::time::Instant, usize)> = probes_owned
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    std::time::Instant::now()
                        + Duration::from_secs(p.initial_delay_seconds.max(0) as u64),
                    i,
                )
            })
            .collect();

        loop {
            interval.tick().await;
            let now = std::time::Instant::now();
            let due: Vec<usize> = next_fire
                .iter()
                .filter(|(t, _)| *t <= now)
                .map(|(_, i)| *i)
                .collect();
            if due.is_empty() {
                continue;
            }

            let mut all_ok = true;
            for i in due {
                let config = &probes_owned[i];
                let status = run_probe(config, &port_map).await;
                let ok = matches!(status, HealthStatus::Healthy);
                all_ok &= ok;
                if !ok {
                    warn!("Probe {} for {} failed", i, cid);
                }
                let period = config.period_seconds.max(1) as u64;
                if let Some(slot) = next_fire.iter_mut().find(|(_, idx)| *idx == i) {
                    slot.0 = now + Duration::from_secs(period);
                }
            }
            p_ready.store(all_ok, Ordering::SeqCst);
            *p_healthy.lock().await = all_ok;
        }
    });

    (ready, healthy)
}

async fn run_probe(
    config: &ProbeConfig,
    port_map: &std::collections::HashMap<u16, u16>,
) -> HealthStatus {
    match &config.action {
        ProbeAction::Exec(exec) => {
            HealthChecker::check_exec(
                exec.command.as_deref().unwrap_or(&[]),
                config.timeout(),
            )
            .await
        }
        ProbeAction::HTTPGet(http) => {
            let mut h = http.clone();
            if let Some(&host_port) = port_map.get(&h.port) {
                h.port = host_port;
            }
            HealthChecker::check_http(&h, config.timeout()).await
        }
        ProbeAction::TCPSocket(tcp) => {
            let mut t = tcp.clone();
            if let Some(&host_port) = port_map.get(&t.port) {
                t.port = host_port;
            }
            HealthChecker::check_tcp(&t, config.timeout()).await
        }
    }
}
