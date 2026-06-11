//! Scheduler-only network reconcile (O3) — single entry; applies `NetworkPlanner` output.

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::netmux::planner::{NetworkPlanner, PlannedDnat, PlannedNetwork};
use crate::netmux::reconciler::ReconcileReport;
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
    let plan = NetworkPlanner::new(crate::config::get().node_name.clone())
        .with_gateway(netmux.gateway())
        .plan(snap);

    let mut report = ReconcileReport {
        dnat_rules: plan.dnat_rules.len(),
        dns_records: plan.dns_records.len(),
        nsg_rules: plan.nsg_rules.len(),
        network_policies: plan.network_policies.len(),
        remote_routes: plan.remote_routes.len(),
    };

    debug!(
        "SyncNetwork: {} local pods, {} services, dnat={} dns={} nsg={} np={} routes={}",
        plan.local_pod_count,
        plan.service_count,
        report.dnat_rules,
        report.dns_records,
        report.nsg_rules,
        report.network_policies,
        report.remote_routes,
    );

    apply_subnets(snap, netmux);
    apply_vnets(snap, netmux).await;
    apply_planned_nsg(netmux, &plan).await;
    apply_route_tables(snap, netmux).await;
    apply_ingress(snap, netmux);
    apply_planned_network_policies(snap, netmux, &plan).await;

    for dnat in &plan.dnat_rules {
        apply_planned_dnat(dnat, snap, netmux, store, process_tracker).await;
    }

    apply_planned_dns(netmux, &plan);
    apply_in_cluster_api_dnat(netmux).await;
    apply_planned_remote_routes(&plan);

    debug!("SyncNetwork done: {:?}", report);
}

fn apply_subnets(snap: &StoreSnapshot, netmux: &Arc<NetMux>) {
    for t in snap.by_kind("Subnet") {
        if let AnyResource::Subnet(subnet) = &t.resource {
            let name = subnet.metadata.name.as_deref().unwrap_or("unknown");
            if let Err(e) = netmux.register_subnet_cidr(name, &subnet.spec.cidr) {
                warn!("SyncNetwork: subnet {}: {}", name, e);
            }
        }
    }
}

async fn apply_vnets(snap: &StoreSnapshot, netmux: &Arc<NetMux>) {
    for t in snap.by_kind("VNet") {
        if let AnyResource::VNet(vnet) = &t.resource {
            let cidr = vnet.spec.cidr.as_deref().unwrap_or("10.42.0.0/20");
            if let Err(e) = netmux.apply_vnet(vnet, cidr).await {
                warn!("SyncNetwork: vnet: {}", e);
            }
            if vnet.spec.internet_access {
                if let Err(e) = netmux
                    .nft
                    .add_snat(vnet.metadata.name.as_deref().unwrap_or("vnet"), cidr)
                    .await
                {
                    warn!("SyncNetwork: snat: {}", e);
                }
            }
        }
    }
}

async fn apply_planned_nsg(netmux: &Arc<NetMux>, plan: &PlannedNetwork) {
    if plan.nsg_rules.is_empty() {
        return;
    }
    if let Err(e) = netmux.apply_nsg_rules(&plan.nsg_rules).await {
        warn!("SyncNetwork: nsg: {}", e);
    }
}

async fn apply_route_tables(snap: &StoreSnapshot, netmux: &Arc<NetMux>) {
    for t in snap.by_kind("RouteTable") {
        if let AnyResource::RouteTable(rt) = &t.resource {
            let name = rt.metadata.name.as_deref().unwrap_or("unknown");
            if let Err(e) = netmux.apply_route_table_rules(name, &[]).await {
                warn!("SyncNetwork: routetable: {}", e);
            }
        }
    }
}

fn apply_ingress(snap: &StoreSnapshot, netmux: &Arc<NetMux>) {
    for t in snap.by_kind("Ingress") {
        if let AnyResource::Ingress(ing) = &t.resource {
            if let Err(e) = crate::netmux::ingress::apply_ingress(&netmux.ingress_state, ing) {
                warn!("SyncNetwork: ingress: {}", e);
            }
        }
    }
}

async fn apply_planned_network_policies(
    snap: &StoreSnapshot,
    netmux: &Arc<NetMux>,
    plan: &PlannedNetwork,
) {
    let npc = crate::netmux::np_controller::NetworkPolicyController::new(netmux.clone());
    for np_ref in &plan.network_policies {
        for t in snap.by_kind("NetworkPolicy") {
            if let AnyResource::NetworkPolicy(np) = &t.resource {
                let ns = np.metadata.namespace.as_deref().unwrap_or("default");
                let name = np.metadata.name.as_deref().unwrap_or("");
                if ns == np_ref.namespace && name == np_ref.name {
                    if let Err(e) = npc.apply_network_policy(np).await {
                        warn!("SyncNetwork: networkpolicy {}/{}: {}", ns, name, e);
                    }
                }
            }
        }
    }
}

fn apply_planned_dns(netmux: &Arc<NetMux>, plan: &PlannedNetwork) {
    let mut records: HashMap<String, Ipv4Addr> = HashMap::new();
    for r in &plan.dns_records {
        records.insert(r.hostname.clone(), r.ip);
    }
    let mut guard = netmux.dns_records.write().unwrap_or_else(|e| e.into_inner());
    *guard = records;
}

async fn apply_in_cluster_api_dnat(netmux: &Arc<NetMux>) {
    let cluster_ip = crate::bootstrap::kubernetes_cluster_ip();
    let (backend_ip, backend_port) = crate::bootstrap::api_backend_endpoint();
    let backends = vec![(backend_ip, backend_port)];
    if let Err(e) = netmux.nft.add_dnat(cluster_ip, 443, &backends).await {
        tracing::warn!(
            "SyncNetwork: kubernetes API DNAT {}:443 -> {}:{}: {}",
            cluster_ip, backend_ip, backend_port, e
        );
    }
}

fn apply_planned_remote_routes(plan: &PlannedNetwork) {
    for route in &plan.remote_routes {
        if let Err(e) =
            crate::netmux::netlink::add_route(&route.pod_ip, 32, Some(&route.via), None)
        {
            debug!(
                "SyncNetwork: remote route {} via {} (node {}): {}",
                route.pod_ip, route.via, route.remote_node, e
            );
        } else {
            debug!(
                "SyncNetwork: remote route {}/32 via {} for node {}",
                route.pod_ip, route.via, route.remote_node
            );
        }
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
        if let Err(e) = netmux
            .nft
            .add_dnat(cip, dnat.listen_port, &backends)
            .await
        {
            tracing::error!("SyncNetwork: add_dnat {}: {:?}", key, e);
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
                                if let Err(e) =
                                    netmux.remove_service_dnat(ip, svc_port.port as u16).await
                                {
                                    warn!("remove_service_dnat: {}", e);
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
