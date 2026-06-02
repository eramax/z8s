//! Pod status projection for API responses (kubectl-compatible).

use std::collections::HashMap;

use crate::store::AnyResource;
use crate::types::{
    ContainerState, ContainerStateRunning, ContainerStateTerminated, ContainerStateWaiting,
    ContainerStatus, HostIP, Pod, PodCondition, PodIP, PodStatus, ResourceState, Time,
};

pub fn fill_pod_metadata(pod: &mut Pod) {
    let meta = &mut pod.metadata;
    if meta.creation_timestamp.is_none() {
        meta.creation_timestamp = Some(Time::now());
    }
    if meta.resource_version.is_none() {
        meta.resource_version = Some("1".into());
    }
    if meta.uid.is_none() {
        let ns = meta.namespace.as_deref().unwrap_or("default");
        let name = meta.name.as_deref().unwrap_or("unknown");
        meta.uid = Some(format!("Pod/{ns}/{name}"));
    }
}

pub fn resource_to_pod_json_with_status(
    resource: &AnyResource,
    state: &ResourceState,
    is_ready: bool,
    restart_counts: &HashMap<String, u32>,
    pod_ip: Option<&str>,
) -> serde_json::Value {
    let pod = match resource {
        AnyResource::Pod(p) => p,
        _ => return serde_json::Value::Null,
    };

    let time = Time::now();
    let phase = if is_ready {
        "Running"
    } else {
        match state {
            ResourceState::Pending => "Pending",
            ResourceState::Running => "Running",
            ResourceState::Succeeded => "Succeeded",
            ResourceState::Failed(_) => "Failed",
            ResourceState::Terminated => "Succeeded",
        }
    };
    let ready = if is_ready { "True" } else { "False" };
    let ip = pod_ip.unwrap_or("127.0.0.1").to_string();
    let status = PodStatus {
        phase: Some(phase.into()),
        host_ip: Some(ip.clone()),
        host_ips: Some(vec![HostIP { ip: ip.clone() }]),
        pod_ip: Some(ip.clone()),
        pod_ips: Some(vec![PodIP { ip }]),
        start_time: Some(time.clone()),
        conditions: Some(vec![
            pod_condition("Initialized", "True", &time),
            pod_condition("Ready", ready, &time),
            pod_condition("ContainersReady", ready, &time),
            pod_condition("PodScheduled", "True", &time),
        ]),
        container_statuses: pod.spec.as_ref().map(|s| {
            s.containers
                .iter()
                .map(|c| {
                    let restarts = restart_counts.get(&c.name).copied().unwrap_or(0) as i32;
                    let crash_loop = restarts >= 3 && !is_ready;
                    let cstate = Some(if is_ready {
                        ContainerState {
                            running: Some(ContainerStateRunning {
                                started_at: Some(time.clone()),
                            }),
                            ..Default::default()
                        }
                    } else if matches!(state, ResourceState::Succeeded | ResourceState::Terminated)
                    {
                        ContainerState {
                            terminated: Some(ContainerStateTerminated {
                                exit_code: 0,
                                reason: Some("Completed".into()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    } else if let ResourceState::Failed(msg) = state {
                        ContainerState {
                            terminated: Some(ContainerStateTerminated {
                                exit_code: 1,
                                reason: Some("Error".into()),
                                message: Some(msg.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    } else {
                        ContainerState {
                            waiting: Some(ContainerStateWaiting {
                                reason: Some(
                                    if crash_loop {
                                        "CrashLoopBackOff"
                                    } else {
                                        "ContainerCreating"
                                    }
                                    .into(),
                                ),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    });
                    ContainerStatus {
                        name: c.name.clone(),
                        image: c.image.clone().unwrap_or_default(),
                        image_id: c
                            .image
                            .clone()
                            .map(|i| format!("z8s://{i}"))
                            .unwrap_or_default(),
                        ready: is_ready,
                        restart_count: restarts,
                        container_id: Some(format!("z8s://{}", c.name)),
                        state: cstate,
                        started: Some(matches!(state, ResourceState::Running) && !crash_loop),
                        ..Default::default()
                    }
                })
                .collect()
        }),
        message: if let ResourceState::Failed(msg) = state {
            Some(msg.clone())
        } else {
            None
        },
        reason: if restart_counts.values().any(|&v| v >= 3) && !is_ready {
            Some("CrashLoopBackOff".into())
        } else {
            None
        },
        qos_class: Some("Burstable".into()),
        ..Default::default()
    };

    let mut pod = pod.clone();
    fill_pod_metadata(&mut pod);
    pod.status = Some(status);
    serde_json::to_value(&pod).unwrap_or_default()
}

fn pod_condition(type_: &str, status: &str, time: &Time) -> PodCondition {
    PodCondition {
        type_: type_.into(),
        status: status.into(),
        last_transition_time: Some(time.clone()),
        ..Default::default()
    }
}
