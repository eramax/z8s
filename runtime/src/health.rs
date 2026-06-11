//! # Health Probes — Liveness, Readiness, and Startup Checks
//!
//! Kubernetes-style health probes: exec, HTTP GET, and TCP socket.
//! Each probe runs on a configurable schedule with timeouts.
//!
//! ## Probe Types
//!
//! - **Exec** — runs a command inside the container; exit 0 = healthy
//! - **HTTPGet** — sends GET to a path:port; 2xx/3xx = healthy
//! - **TCPSocket** — connects to host:port; connection succeeds = healthy

use std::time::Duration;

use tokio::net::TcpStream;

// ── Types ──────────────────────────────────────────────────────────────────

/// Result of a health probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

/// What kind of probe to run.
#[derive(Debug, Clone)]
pub enum ProbeAction {
    Exec(ExecProbe),
    HTTPGet(HttpProbe),
    TCPSocket(TcpProbe),
}

/// Run a command inside the container.
#[derive(Debug, Clone)]
pub struct ExecProbe {
    pub command: Option<Vec<String>>,
}

/// HTTP GET against the container.
#[derive(Debug, Clone)]
pub struct HttpProbe {
    pub host: Option<String>,
    pub path: String,
    pub port: u16,
    pub scheme: Option<String>,
    pub headers: Vec<(String, String)>,
}

/// TCP connection check.
#[derive(Debug, Clone)]
pub struct TcpProbe {
    pub host: Option<String>,
    pub port: u16,
}

/// Full probe configuration: what to check, when, and how long to wait.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub action: ProbeAction,
    pub initial_delay_seconds: i32,
    pub period_seconds: i32,
    pub timeout_seconds: i32,
}

impl ProbeConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds.max(0) as u64)
    }
}

// ── Checker ────────────────────────────────────────────────────────────────

/// Stateless probe executor. All methods are pure async functions on input.
pub struct HealthChecker;

impl HealthChecker {
    /// Run an exec probe: spawn command, check exit status.
    pub async fn check_exec(cmd: &[String], timeout: Duration) -> HealthStatus {
        if cmd.is_empty() {
            return HealthStatus::Unknown;
        }
        let result = tokio::time::timeout(timeout, async {
            let child = tokio::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .kill_on_drop(true)
                .spawn();
            match child {
                Ok(mut c) => c.wait().await.map(|s| s.success()).unwrap_or(false),
                Err(_) => false,
            }
        })
        .await;
        match result {
            Ok(true) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    /// Run an HTTP GET probe: connect, send request, check status code.
    pub async fn check_http(probe: &HttpProbe, timeout: Duration) -> HealthStatus {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let host = probe.host.as_deref().unwrap_or("localhost");
        let result = tokio::time::timeout(timeout, async {
            let stream = TcpStream::connect(format!("{}:{}", host, probe.port)).await?;
            let mut reader = BufReader::new(stream);

            let headers: String = probe
                .headers
                .iter()
                .map(|(k, v)| format!("{}: {}\r\n", k, v))
                .collect();

            let req = format!(
                "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n{}\r\n",
                probe.path, host, headers
            );
            reader.get_mut().write_all(req.as_bytes()).await?;

            let mut status_line = String::new();
            reader.read_line(&mut status_line).await?;
            let code: u16 = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            Ok::<bool, std::io::Error>((200..400).contains(&code))
        })
        .await;

        match result {
            Ok(Ok(true)) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    /// Run a TCP socket probe: attempt connection.
    pub async fn check_tcp(probe: &TcpProbe, timeout: Duration) -> HealthStatus {
        let host = probe.host.as_deref().unwrap_or("localhost");
        let result = tokio::time::timeout(timeout, async {
            TcpStream::connect(format!("{}:{}", host, probe.port)).await
        })
        .await;
        match result {
            Ok(Ok(_)) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    /// Dispatch a probe by its action type.
    pub async fn run(probe: &ProbeConfig, port_map: &std::collections::HashMap<u16, u16>) -> HealthStatus {
        match &probe.action {
            ProbeAction::Exec(exec) => {
                Self::check_exec(
                    exec.command.as_deref().unwrap_or(&[]),
                    probe.timeout(),
                )
                .await
            }
            ProbeAction::HTTPGet(http) => {
                let mut h = http.clone();
                if let Some(&host_port) = port_map.get(&h.port) {
                    h.port = host_port;
                }
                Self::check_http(&h, probe.timeout()).await
            }
            ProbeAction::TCPSocket(tcp) => {
                let mut t = tcp.clone();
                if let Some(&host_port) = port_map.get(&t.port) {
                    t.port = host_port;
                }
                Self::check_tcp(&t, probe.timeout()).await
            }
        }
    }
}
