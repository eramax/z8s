use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::api::types::{AnyResource, ResourceStore, extract_containers, parse_quantity_bytes, parse_quantity_cpu};
use crate::cri::spec::{ContainerConfig, ContainerSpec, ResolvedVolume};
use crate::cri::health::{ProbeConfig, ProbeAction, ExecProbe, HttpProbe, TcpProbe};
use k8s_openapi::api::core::v1::{ConfigMap, Container, Pod, Secret};
use anyhow::Result;

pub async fn build_spec(resource: &AnyResource, store: &ResourceStore) -> ContainerSpec {
    let containers = extract_containers(resource);
    let pod_name = resource.name().to_string();
    let pod_uid = resource.uid();
    let namespace = resource.namespace().to_string();
    let pod = match resource { AnyResource::Pod(p) => Some(p), _ => None };

    let (cms, secrets) = fetch_cms_and_secrets(store).await;
    let service_env = if let Some(pod) = pod { resolve_service_env(pod, store).await } else { vec![] };

    let mut configs = Vec::new();
    for container in &containers {
        let container_id = format!("{}-{}", pod_name, container.name);
        let image_ref = container.image.clone().unwrap_or_default();
        let is_native = image_ref.is_empty() || image_ref == "host" || image_ref.starts_with("host://");

        let mut env_vars: Vec<(String, String)> = container.env.as_ref()
            .map(|env| env.iter().map(|e| (e.name.clone(), e.value.clone().unwrap_or_default())).collect())
            .unwrap_or_default();

        if let Some(pod) = pod {
            env_vars.extend(resolve_env_from(container, pod, &cms, &secrets));
            env_vars.extend(service_env.clone());
        }

        let pod_sc = pod.and_then(|p| p.spec.as_ref().and_then(|s| s.security_context.as_ref()));
        let run_as_user = container.security_context.as_ref().and_then(|sc| sc.run_as_user)
            .or_else(|| pod_sc.and_then(|sc| sc.run_as_user)).map(|u| u as u32);
        let run_as_group = container.security_context.as_ref().and_then(|sc| sc.run_as_group)
            .or_else(|| pod_sc.and_then(|sc| sc.run_as_group)).map(|g| g as u32);
        let privileged = container.security_context.as_ref().and_then(|sc| sc.privileged).unwrap_or(false);
        let extra_capabilities = container.security_context.as_ref()
            .and_then(|sc| sc.capabilities.as_ref())
            .and_then(|c| c.add.as_ref()).cloned().unwrap_or_default();

        let volumes = if let Some(pod) = pod {
            if !is_native {
                crate::cri::volumes::prepare_volumes(pod, &container.name, &pod_uid,
                    &|ns, name| cms.get(&(ns.to_string(), name.to_string())).cloned(),
                    &|ns, name| secrets.get(&(ns.to_string(), name.to_string())).cloned(),
                ).unwrap_or_default()
            } else { vec![] }
        } else { vec![] };

        let (memory_limit, memory_low, cpu_quota, cpu_period) = resolve_resource_limits(container);

        let declared_ports: Vec<u16> = container.ports.as_ref()
            .map(|ps| ps.iter().map(|p| p.container_port as u16).collect())
            .unwrap_or_default();
        let isolated_net = !declared_ports.is_empty();

        let mut probes = Vec::new();
        for probe in [&container.liveness_probe, &container.readiness_probe, &container.startup_probe].into_iter().flatten() {
            if let Some(config) = convert_probe(probe) { probes.push(config); }
        }

        configs.push(ContainerConfig {
            container_id,
            container_name: container.name.clone(),
            image: image_ref,
            rootfs_path: String::new(),
            is_native,
            entrypoint: String::new(),
            args: vec![],
            working_dir: container.working_dir.clone(),
            env: env_vars,
            volumes,
            memory_limit_bytes: memory_limit,
            memory_low_bytes: memory_low,
            cpu_quota,
            cpu_period,
            run_as_user,
            run_as_group,
            privileged,
            extra_capabilities,
            isolated_net,
            published_ports: Default::default(),
            probes,
        });
    }

    let labels = pod.map(|p| p.metadata.labels.clone().unwrap_or_default()).unwrap_or_default();

    ContainerSpec {
        pod_name,
        pod_uid,
        namespace,
        hostname: resource.name().to_string(),
        containers: configs,
        cgroup_path: String::new(),
        labels,
    }
}

async fn fetch_cms_and_secrets(store: &ResourceStore) -> (HashMap<(String, String), ConfigMap>, HashMap<(String, String), Secret>) {
    let cms = store.get_by_kind("ConfigMap").await.into_iter()
        .filter_map(|t| if let AnyResource::ConfigMap(cm) = t.resource {
            let ns = cm.metadata.namespace.clone().unwrap_or_default();
            let name = cm.metadata.name.clone().unwrap_or_default();
            Some(((ns, name), cm))
        } else { None })
        .collect();

    let secrets = store.get_by_kind("Secret").await.into_iter()
        .filter_map(|t| if let AnyResource::Secret(sec) = t.resource {
            let ns = sec.metadata.namespace.clone().unwrap_or_default();
            let name = sec.metadata.name.clone().unwrap_or_default();
            Some(((ns, name), sec))
        } else { None })
        .collect();

    (cms, secrets)
}

fn resolve_env_from(container: &Container, pod: &Pod, cms: &HashMap<(String, String), ConfigMap>, secrets: &HashMap<(String, String), Secret>) -> Vec<(String, String)> {
    let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
    let mut vars = Vec::new();
    for env_from in container.env_from.as_deref().unwrap_or(&[]) {
        let prefix = env_from.prefix.as_deref().unwrap_or("");
        if let Some(cm_ref) = &env_from.config_map_ref {
            if let Some(cm) = cms.get(&(ns.to_string(), cm_ref.name.clone())) {
                for (k, v) in cm.data.as_ref().into_iter().flatten() {
                    vars.push((format!("{}{}", prefix, k), v.clone()));
                }
            }
        }
        if let Some(sec_ref) = &env_from.secret_ref {
            if let Some(sec) = secrets.get(&(ns.to_string(), sec_ref.name.clone())) {
                for (k, v) in sec.data.as_ref().into_iter().flatten() {
                    if let Ok(s) = std::str::from_utf8(&v.0) { vars.push((format!("{}{}", prefix, k), s.to_string())); }
                }
                for (k, v) in sec.string_data.as_ref().into_iter().flatten() {
                    vars.push((format!("{}{}", prefix, k), v.clone()));
                }
            }
        }
    }
    vars
}

async fn resolve_service_env(pod: &Pod, store: &ResourceStore) -> Vec<(String, String)> {
    let pod_ns = pod.metadata.namespace.as_deref().unwrap_or("default");
    let trackers = store.get_by_kind("Service").await;
    let mut vars = Vec::new();
    for t in trackers {
        if let AnyResource::Service(svc) = &t.resource {
            if svc.metadata.namespace.as_deref().unwrap_or("default") != pod_ns { continue; }
            let cluster_ip = svc.spec.as_ref().and_then(|s| s.cluster_ip.as_deref()).unwrap_or("None");
            if cluster_ip == "None" || cluster_ip.is_empty() { continue; }
            let svc_name = svc.metadata.name.as_deref().unwrap_or_default();
            let prefix = svc_name.to_uppercase().replace('-', "_");
            vars.push((format!("{}_SERVICE_HOST", prefix), cluster_ip.to_string()));
            if let Some(ports) = svc.spec.as_ref().and_then(|s| s.ports.as_ref()) {
                for port in ports {
                    vars.push((format!("{}_SERVICE_PORT", prefix), port.port.to_string()));
                    if let Some(pname) = &port.name {
                        let pname_up = pname.to_uppercase().replace('-', "_");
                        vars.push((format!("{}_SERVICE_PORT_{}", prefix, pname_up), port.port.to_string()));
                    }
                }
            }
        }
    }
    vars
}

fn resolve_resource_limits(container: &Container) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    let mut ml = None; let mut mlo = None; let mut cq = None; let mut cp = None;
    if let Some(resources) = &container.resources {
        if let Some(limits) = &resources.limits {
            if let Some(mem) = limits.get("memory") { let bytes = parse_quantity_bytes(mem); if bytes > 0 { ml = Some(bytes as i64); } }
            if let Some(cpu) = limits.get("cpu") { let (q, p) = parse_quantity_cpu(cpu); cq = Some(q); cp = Some(p); }
        }
        if let Some(requests) = &resources.requests {
            if let Some(mem) = requests.get("memory") { let bytes = parse_quantity_bytes(mem); if bytes > 0 { mlo = Some(bytes as i64); } }
        }
    }
    (ml, mlo, cq, cp)
}

fn convert_probe(probe: &k8s_openapi::api::core::v1::Probe) -> Option<ProbeConfig> {
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    let action = if let Some(exec) = &probe.exec {
        ProbeAction::Exec(ExecProbe { command: exec.command.clone() })
    } else if let Some(http) = &probe.http_get {
        let port = match &http.port {
            IntOrString::Int(i) => *i as u16,
            IntOrString::String(s) => s.parse().unwrap_or(80),
        };
        ProbeAction::HTTPGet(HttpProbe {
            host: http.host.clone(),
            path: http.path.clone().unwrap_or_else(|| "/".to_string()),
            port,
            scheme: http.scheme.clone(),
            headers: http.http_headers.as_deref().unwrap_or(&[]).iter().map(|h| (h.name.clone(), h.value.clone())).collect(),
        })
    } else if let Some(tcp) = &probe.tcp_socket {
        let port = match &tcp.port {
            IntOrString::Int(i) => *i as u16,
            IntOrString::String(s) => s.parse().unwrap_or(80),
        };
        ProbeAction::TCPSocket(TcpProbe { host: tcp.host.clone(), port })
    } else {
        return None;
    };
    Some(ProbeConfig {
        action,
        initial_delay_seconds: probe.initial_delay_seconds.unwrap_or(0),
        period_seconds: probe.period_seconds.unwrap_or(10),
        timeout_seconds: probe.timeout_seconds.unwrap_or(1),
    })
}
