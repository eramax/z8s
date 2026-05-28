use k8s_openapi::api::core::v1::{ExecAction, HTTPGetAction, TCPSocketAction};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

pub struct HealthChecker;

impl HealthChecker {
    pub async fn check_exec(cmd: &[String], timeout: Duration) -> HealthStatus {
        if cmd.is_empty() {
            return HealthStatus::Unknown;
        }
        let result = tokio::time::timeout(timeout, async {
            let child = Command::new(&cmd[0]).args(&cmd[1..]).kill_on_drop(true).spawn();

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

    pub async fn check_http(action: &HTTPGetAction, timeout: Duration) -> HealthStatus {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let host = action.host.as_deref().unwrap_or("localhost");
        let port = int_or_string_port(&action.port);
        let path = action.path.as_deref().unwrap_or("/");

        // Build host header including custom headers from the probe spec
        let mut extra_headers = String::new();
        for h in action.http_headers.as_deref().unwrap_or(&[]) {
            extra_headers.push_str(&format!("{}: {}\r\n", h.name, h.value));
        }

        let result = tokio::time::timeout(timeout, async {
            let stream = TcpStream::connect(format!("{}:{}", host, port)).await?;
            let mut reader = BufReader::new(stream);
            let req = format!(
                "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n{}\r\n",
                path, host, extra_headers
            );
            reader.get_mut().write_all(req.as_bytes()).await?;

            // Read the status line only — sufficient to determine success/failure
            let mut status_line = String::new();
            reader.read_line(&mut status_line).await?;

            // Accept 2xx and 3xx as healthy (matches Kubernetes behaviour)
            let code: u16 = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            Ok::<bool, std::io::Error>(code >= 200 && code < 400)
        })
        .await;

        match result {
            Ok(Ok(true)) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    pub async fn check_tcp(action: &TCPSocketAction, timeout: Duration) -> HealthStatus {
        let host = action.host.as_deref().unwrap_or("localhost");
        let port = int_or_string_port(&action.port);

        let result = tokio::time::timeout(timeout, async {
            TcpStream::connect(format!("{}:{}", host, port)).await
        })
        .await;

        match result {
            Ok(Ok(_)) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }
}

fn int_or_string_port(ios: &IntOrString) -> String {
    match ios {
        IntOrString::Int(i) => format!("{}", i),
        IntOrString::String(s) => s.clone(),
    }
}

#[derive(Debug, Clone)]
pub enum ProbeAction {
    Exec(ExecAction),
    HTTPGet(HTTPGetAction),
    TCPSocket(TCPSocketAction),
}

#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub action: ProbeAction,
    pub initial_delay_seconds: i32,
    pub period_seconds: i32,
    pub timeout_seconds: i32,
}

impl ProbeConfig {
    pub fn from_probe(probe: &k8s_openapi::api::core::v1::Probe) -> Option<Self> {
        let action = if let Some(exec) = &probe.exec {
            ProbeAction::Exec(exec.clone())
        } else if let Some(http) = &probe.http_get {
            ProbeAction::HTTPGet(http.clone())
        } else if let Some(tcp) = &probe.tcp_socket {
            ProbeAction::TCPSocket(tcp.clone())
        } else {
            return None;
        };

        Some(Self {
            action,
            initial_delay_seconds: probe.initial_delay_seconds.unwrap_or(0),
            period_seconds: probe.period_seconds.unwrap_or(10),
            timeout_seconds: probe.timeout_seconds.unwrap_or(1),
        })
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds as u64)
    }
}
