//! Scheduler-only network reconcile (O3) — single entry for service DNAT + policy objects.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use tracing::{debug, info};

use crate::netmux::planner::{NetworkPlanner, PlannedDnat};
use crate::netmux::NetMux;
use crate::scheduler::process::ProcessTracker;
use crate::store::{AnyResource, StoreBackend, StoreSnapshot};
use crate::types::IntOrString;

/// Full network sync from a store snapshot (only path that should touch nft/service state).
pub async fn reconcile_network(
    snap: &StoreSnapshot,
    netmux: &Arc<NetMux>,
    store: &Arc<dyn StoreBackend>,
    process_tracker: &Arc<ProcessTracker>,
) {
    let plan = NetworkPlanner::new(crate::config::get().node_name.clone()).plan(snap);
    debug!(
        "SyncNetwork: {} local pods, {} services, {} planned DNAT rules",
        plan.local_pod_count,
        plan.service_count,
        plan.dnat_rules.len()
    );

    for t in snap.by_kind("Subnet") {
        if let AnyResource::Subnet(subnet) = &t.resource {
            let name = subnet.metadata.name.as_deref().unwrap_or("unknown");
            if let Err(e) = netmux.register_subnet_cidr(name, &subnet.spec.cidr) {
                tracing::warn!("SyncNetwork: subnet {}: {}", name, e);
            }
        }
    }

    for t in snap.by_kind("VNet") {
        if let AnyResource::VNet(vnet) = &t.resource {
            let cidr = vnet.spec.cidr.as_deref().unwrap_or("10.42.0.0/20");
            if let Err(e) = netmux.apply_vnet(vnet, cidr).await {
                tracing::warn!("SyncNetwork: vnet: {}", e);
            }
            if vnet.spec.internet_access {
                if let Err(e) = netmux
                    .nft
                    .add_snat(vnet.metadata.name.as_deref().unwrap_or("vnet"), cidr)
                    .await
                {
                    tracing::warn!("SyncNetwork: snat: {}", e);
                }
            }
        }
    }

    for t in snap.by_kind("NSG") {
        if let AnyResource::Nsg(nsg) = &t.resource {
            if let Err(e) = netmux.apply_nsg(nsg).await {
                tracing::warn!("SyncNetwork: nsg: {}", e);
            }
        }
    }

    for t in snap.by_kind("RouteTable") {
        if let AnyResource::RouteTable(rt) = &t.resource {
            let name = rt.metadata.name.as_deref().unwrap_or("unknown");
            if let Err(e) = netmux.apply_route_table_rules(name, &[]).await {
                tracing::warn!("SyncNetwork: routetable: {}", e);
            }
        }
    }

    for t in snap.by_kind("Ingress") {
        if let AnyResource::Ingress(ing) = &t.resource {
            if let Err(e) = crate::netmux::ingress::apply_ingress(&netmux.ingress_state, ing) {
                tracing::warn!("SyncNetwork: ingress: {}", e);
            }
            let gw = netmux.gateway;
            if let Some(spec) = &ing.spec {
                if let Some(rules) = &spec.rules {
                    let mut records = netmux.dns_records.write().unwrap_or_else(|e| e.into_inner());
                    for rule in rules {
                        if let Some(host) = &rule.host {
                            if !host.is_empty() {
                                records.insert(host.clone(), gw);
                            }
                        }
                    }
                }
            }
        }
    }

    for t in snap.by_kind("NetworkPolicy") {
        if let AnyResource::NetworkPolicy(np) = &t.resource {
            let npc = crate::netmux::np_controller::NetworkPolicyController::new(netmux.clone());
            if let Err(e) = npc.apply_network_policy(np).await {
                tracing::warn!("SyncNetwork: networkpolicy: {}", e);
            }
        }
    }

    for dnat in &plan.dnat_rules {
        apply_planned_dnat(dnat, snap, netmux, store, process_tracker).await;
    }
}

async fn apply_planned_dnat(
    dnat: &PlannedDnat,
    snap: &StoreSnapshot,
    netmux: &Arc<NetMux>,
    store: &Arc<dyn StoreBackend>,
    process_tracker: &Arc<ProcessTracker>,
) {
    let key = format!("{}/{}", dnat.service_ns, dnat.service_name);
    let Some(selector) = service_selector(snap, &dnat.service_ns, &dnat.service_name) else {
        return;
    };
    if selector.is_empty() {
        return;
    }

    let default_port = dnat.listen_port as i32;
    let container_port = resolve_container_port(
        store,
        &dnat.service_ns,
        &dnat.target_port,
        default_port,
    )
    .await;
    let backends = resolve_backend_pods(
        store,
        process_tracker,
        &selector,
        &dnat.service_ns,
        container_port,
    )
    .await;
    if backends.is_empty() {
        return;
    }

    if let Some(cip) = dnat.cluster_ip {
        if !dnat.is_nodeport {
            if let Err(e) = netmux
                .nft
                .add_dnat(cip, dnat.listen_port, &backends)
                .await
            {
                tracing::error!("SyncNetwork: add_dnat {}: {:?}", key, e);
            }
        } else if let Err(e) = netmux
            .nft
            .add_dnat(cip, dnat.listen_port, &backends)
            .await
        {
            tracing::error!("SyncNetwork: add_dnat (nodeport svc) {}: {:?}", key, e);
        }
    }

    if dnat.is_nodeport {
        if let Some(np) = dnat.node_port {
            if let Err(e) = netmux.nft.add_nodeport_dnat(np, &backends).await {
                tracing::error!("SyncNetwork: add_nodeport_dnat {}: {:?}", key, e);
            }
        }
    }
}

fn service_selector(
    snap: &StoreSnapshot,
    ns: &str,
    name: &str,
) -> Option<BTreeMap<String, String>> {
    snap.by_kind("Service").into_iter().find_map(|t| {
        let AnyResource::Service(svc) = &t.resource else {
            return None;
        };
        if svc.metadata.namespace.as_deref().unwrap_or("default") != ns {
            return None;
        }
        if svc.metadata.name.as_deref() != Some(name) {
            return None;
        }
        svc.spec.as_ref()?.selector.clone()
    })
}

async fn resolve_backend_pods(
    store: &Arc<dyn StoreBackend>,
    tracker: &ProcessTracker,
    selector: &BTreeMap<String, String>,
    ns: &str,
    port: u16,
) -> Vec<(Ipv4Addr, u16)> {
    let pod_trackers = store.get_by_kind("Pod").await;
    let mut backends = Vec::new();
    for t in &pod_trackers {
        if let AnyResource::Pod(pod) = &t.resource {
            if pod.metadata.namespace.as_deref().unwrap_or("default") != ns {
                continue;
            }
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            if !selector.iter().all(|(k, v)| labels.get(k) == Some(v)) {
                continue;
            }
            if pod.assigned_node.as_deref() != Some(crate::config::get().node_name.as_str()) {
                continue;
            }
            if let Some(pod_ip) = tracker
                .pod_ip(pod.metadata.name.as_deref().unwrap_or(""))
                .await
            {
                backends.push((pod_ip, port));
            }
        }
    }
    backends
}

async fn resolve_container_port(
    store: &Arc<dyn StoreBackend>,
    ns: &str,
    target_port: &IntOrString,
    default_port: i32,
) -> u16 {
    match target_port {
        IntOrString::Int(n) => *n as u16,
        IntOrString::String(name) => {
            resolve_named_port(store, ns, name)
                .await
                .unwrap_or(default_port as u16)
        }
    }
}

async fn resolve_named_port(store: &Arc<dyn StoreBackend>, ns: &str, name: &str) -> Option<u16> {
    let pods = store.get_by_kind("Pod").await;
    for t in &pods {
        if t.resource.namespace() != ns {
            continue;
        }
        let containers = crate::store::extract_containers(&t.resource);
        for c in &containers {
            if let Some(ports) = &c.ports {
                for p in ports {
                    if p.name.as_deref() == Some(name) {
                        return Some(p.container_port as u16);
                    }
                }
            }
        }
    }
    name.parse::<u16>().ok()
}

/// Remove service DNAT state (API delete path).
pub async fn remove_service(netmux: &Arc<NetMux>, store: &Arc<dyn StoreBackend>, ns: &str, name: &str) {
    let trackers = store.get_by_kind("Service").await;
    for t in &trackers {
        if t.resource.namespace() == ns && t.resource.name() == name {
            if let AnyResource::Service(svc) = &t.resource {
                if let Some(spec) = &svc.spec {
                    if let Some(cip) = &spec.cluster_ip {
                        if let Ok(ip) = cip.parse::<Ipv4Addr>() {
                            for svc_port in spec.ports.as_deref().unwrap_or(&[]) {
                                if let Err(e) = netmux.remove_service_dnat(ip, svc_port.port as u16).await
                                {
                                    tracing::warn!("remove_service_dnat: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    info!("SyncNetwork: removed service {}/{}", ns, name);
}
