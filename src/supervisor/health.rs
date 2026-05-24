use crate::api::types::extract_containers;
use crate::api::AnyResource;
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
    pub fn new() -> Self {
        Self
    }

    pub fn get_probes(&self, resource: &AnyResource) -> Vec<(String, ProbeConfig)> {
        let mut probes = Vec::new();
        let containers = extract_containers(resource);

        for container in &containers {
            let name = container.name.clone();

            if let Some(liveness) = &container.liveness_probe {
                if let Some(config) = ProbeConfig::from_probe(liveness) {
                    probes.push((format!("{}/liveness", name), config));
                }
            }

            if let Some(readiness) = &container.readiness_probe {
                if let Some(config) = ProbeConfig::from_probe(readiness) {
                    probes.push((format!("{}/readiness", name), config));
                }
            }

            if let Some(startup) = &container.startup_probe {
                if let Some(config) = ProbeConfig::from_probe(startup) {
                    probes.push((format!("{}/startup", name), config));
                }
            }
        }

        probes
    }

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
        let host = action.host.as_deref().unwrap_or("localhost");
        let port = int_or_string_port(&action.port);
        let path = action.path.as_deref().unwrap_or("/");
        let scheme = action.scheme.as_deref().unwrap_or("HTTP");

        let url = format!("{}://{}:{}{}", scheme.to_lowercase(), host, port, path);

        let result = tokio::time::timeout(timeout, async { reqwest::get(&url).await }).await;

        match result {
            Ok(Ok(resp)) if resp.status().is_success() => HealthStatus::Healthy,
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
    pub success_threshold: i32,
    pub failure_threshold: i32,
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
            success_threshold: probe.success_threshold.unwrap_or(1),
            failure_threshold: probe.failure_threshold.unwrap_or(3),
        })
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds as u64)
    }
}
