use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Clone)]
pub enum ProbeAction {
    Exec(ExecProbe),
    HTTPGet(HttpProbe),
    TCPSocket(TcpProbe),
}

#[derive(Debug, Clone)]
pub struct ExecProbe {
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct HttpProbe {
    pub host: Option<String>,
    pub path: String,
    pub port: u16,
    pub scheme: Option<String>,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct TcpProbe {
    pub host: Option<String>,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub action: ProbeAction,
    pub initial_delay_seconds: i32,
    pub period_seconds: i32,
    pub timeout_seconds: i32,
}

impl ProbeConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds as u64)
    }
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
        }).await;
        match result {
            Ok(true) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    pub async fn check_http(probe: &HttpProbe, timeout: Duration) -> HealthStatus {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let host = probe.host.as_deref().unwrap_or("localhost");
        let port = probe.port;
        let path = &probe.path;
        let mut extra_headers = String::new();
        for (name, value) in &probe.headers {
            extra_headers.push_str(&format!("{}: {}\r\n", name, value));
        }
        let result = tokio::time::timeout(timeout, async {
            let stream = TcpStream::connect(format!("{}:{}", host, port)).await?;
            let mut reader = BufReader::new(stream);
            let req = format!("GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n{}\r\n", path, host, extra_headers);
            reader.get_mut().write_all(req.as_bytes()).await?;
            let mut status_line = String::new();
            reader.read_line(&mut status_line).await?;
            let code: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            Ok::<bool, std::io::Error>(code >= 200 && code < 400)
        }).await;
        match result {
            Ok(Ok(true)) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    pub async fn check_tcp(probe: &TcpProbe, timeout: Duration) -> HealthStatus {
        let host = probe.host.as_deref().unwrap_or("localhost");
        let port = probe.port;
        let result = tokio::time::timeout(timeout, async {
            TcpStream::connect(format!("{}:{}", host, port)).await
        }).await;
        match result {
            Ok(Ok(_)) => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }
}
