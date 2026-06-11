//! # Network Sync (Reconciler)
//!
//! Applies the planner's desired state to the live system. Single
//! entry point: `reconcile_network` — reads a store snapshot, plans,
//! and applies the diff.
//!
//! ## Apply Order
//!
//! Order matters because:
//! - Subnets must be registered before pods can be assigned IPs
//! - VNets must be SNAT'd before pod traffic can reach the internet
//! - NSG rules must be in place before services forward traffic
//! - Service DNAT must be installed before Ingress tries to use them
//!
//! ```text
//! 1. subnets
//! 2. vnets (SNAT for internet access)
//! 3. NSG rules
//! 4. route tables (stub)
//! 5. ingress L7
//! 6. network policies
//! 7. service DNAT
//! 8. DNS records
//! 9. remote pod routes
//! 10. kubernetes API DNAT (always last, in case service CIDR overlaps)
//! ```

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::Arc;
use tracing::{debug, info, warn};

use z8s_core::store::{StoreBackend, StoreSnapshot};
use z8s_core::types::AnyResource;

use crate::dns::DnsState;
use crate::ingress::IngressState;
use crate::nft::NftEngine;
use crate::np_controller::NetworkPolicyController;
use crate::planner::{NetworkPlanner, PlannedDnat, PlannedNetwork};

/// Full network sync from a store snapshot.
pub async fn reconcile_network(
    snap: &StoreSnapshot,
    nft: &Arc<NftEngine>,
    dns: &DnsState,
    ingress: &IngressState,
    npc: &NetworkPolicyController,
    node_name: &str,
    gateway: Ipv4Addr,
    store: &Arc<dyn StoreBackend>,
) {
    let plan = NetworkPlanner::new(node_name)
        .with_gateway(gateway)
        .plan(snap);

    debug!(
        "SyncNetwork: {} local pods, {} services, dnat={} dns={} nsg={} np={} routes={}",
        plan.local_pod_count,
        plan.service_count,
        plan.dnat_rules.len(),
        plan.dns_records.len(),
        plan.nsg_rules.len(),
        plan.network_policies.len(),
        plan.remote_routes.len(),
    );

    apply_subnets(snap, nft).await;
    apply_vnets(snap, nft).await;
    apply_planned_nsg(nft, &plan).await;
    apply_route_tables(snap, nft).await;
    apply_ingress(snap, ingress);
    apply_planned_network_policies(snap, npc, &plan).await;

    for dnat in &plan.dnat_rules {
        apply_planned_dnat(dnat, snap, nft, store).await;
    }

    apply_planned_dns(dns, &plan);
    apply_in_cluster_api_dnat(nft).await;
    apply_planned_remote_routes(&plan);

    info!(
        "SyncNetwork done: dnat={} dns={} nsg={} np={} routes={}",
        plan.dnat_rules.len(),
        plan.dns_records.len(),
        plan.nsg_rules.len(),
        plan.network_policies.len(),
        plan.remote_routes.len()
    );
}

async fn apply_subnets(snap: &StoreSnapshot, _nft: &Arc<NftEngine>) {
    for t in snap.by_kind("Subnet") {
        if let AnyResource::Subnet(subnet) = &t.spec {
            let name = subnet.metadata.name.as_deref().unwrap_or("unknown");
            debug!("Subnet registered: {} = {}", name, subnet.spec.cidr);
        }
    }
}

async fn apply_vnets(snap: &StoreSnapshot, nft: &Arc<NftEngine>) {
    for t in snap.by_kind("VNet") {
        if let AnyResource::VNet(vnet) = &t.spec {
            let cidr = vnet.spec.cidr.as_deref().unwrap_or("10.42.0.0/20");
            let name = vnet.metadata.name.as_deref().unwrap_or("vnet");
            if vnet.spec.internet_access {
                if let Err(e) = nft.add_snat(name, cidr).await {
                    warn!("SyncNetwork: SNAT for vnet {}: {}", name, e);
                } else {
                    info!("VNet '{}' SNAT applied for internet access ({})", name, cidr);
                }
            } else {
                info!("VNet '{}' internet access denied ({})", name, cidr);
            }
        }
    }
}

async fn apply_planned_nsg(nft: &Arc<NftEngine>, plan: &PlannedNetwork) {
    if plan.nsg_rules.is_empty() {
        return;
    }
    if let Err(e) = nft.reset_nsg_rules().await {
        warn!("SyncNetwork: reset NSG: {}", e);
        return;
    }
    for rule in &plan.nsg_rules {
        match &rule.action {
            crate::rule::NftAction::Accept => {
                if let (Some(src), Some(dst)) = (rule.source.as_deref(), rule.dest.as_deref()) {
                    if let Err(e) = nft.add_forward_allow(src, dst).await {
                        warn!("SyncNetwork: NSG allow {}=>{}: {}", src, dst, e);
                    }
                }
            }
            crate::rule::NftAction::Drop => {
                if let (Some(src), Some(dst)) = (rule.source.as_deref(), rule.dest.as_deref()) {
                    if let Err(e) = nft.add_forward_deny(src, dst).await {
                        warn!("SyncNetwork: NSG deny {}=>{}: {}", src, dst, e);
                    }
                }
            }
            _ => {
                // Jump/other actions not supported for NSG
            }
        }
    }
    // Default deny: drop everything not explicitly allowed
    if let Err(e) = nft.add_forward_deny("0.0.0.0/0", "0.0.0.0/0").await {
        warn!("SyncNetwork: default deny: {}", e);
    }
}

async fn apply_route_tables(snap: &StoreSnapshot, _nft: &Arc<NftEngine>) {
    for t in snap.by_kind("RouteTable") {
        if let AnyResource::RouteTable(rt) = &t.spec {
            let name = rt.metadata.name.as_deref().unwrap_or("unknown");
            debug!("RouteTable '{}' has {} routes (stub)", name, rt.spec.routes.len());
        }
    }
}

fn apply_ingress(snap: &StoreSnapshot, ingress: &IngressState) {
    for t in snap.by_kind("Ingress") {
        if let AnyResource::Ingress(_ing) = &t.spec {
            // Spec is currently empty in core types — minimal wiring
            let mut routes = HashMap::new();
            routes.insert(t.uid().to_string(), (t.uid().to_string(), 80));
            ingress.set_routes(t.uid(), routes);
        }
    }
}

async fn apply_planned_network_policies(
    snap: &StoreSnapshot,
    npc: &NetworkPolicyController,
    plan: &PlannedNetwork,
) {
    for np_ref in &plan.network_policies {
        for t in snap.by_kind("NetworkPolicy") {
            if let AnyResource::NetworkPolicy(np) = &t.spec {
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

fn apply_planned_dns(dns: &DnsState, plan: &PlannedNetwork) {
    let mut snap = crate::dns::DnsSnapshot::new();
    for r in &plan.dns_records {
        snap.insert(r.hostname.clone(), r.ip);
    }
    dns.replace(snap);
}

async fn apply_in_cluster_api_dnat(nft: &Arc<NftEngine>) {
    // Wire the kubernetes.default.svc ClusterIP to localhost:6443 (the API server).
    // This is best-effort — if no API is running, the DNAT just drops.
    let cluster_ip = Ipv4Addr::new(10, 96, 0, 1);
    let backends = vec![(Ipv4Addr::new(127, 0, 0, 1), 6443)];
    if let Err(e) = nft.add_dnat(cluster_ip, 443, &backends).await {
        debug!(
            "SyncNetwork: kubernetes API DNAT {}:443 -> 127.0.0.1:6443: {}",
            cluster_ip, e
        );
    }
}

fn apply_planned_remote_routes(plan: &PlannedNetwork) {
    for route in &plan.remote_routes {
        if let Err(e) = crate::netlink::add_route(
            &route.pod_ip,
            32,
            Some(&route.via),
            None,
        ) {
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
    nft: &Arc<NftEngine>,
    store: &Arc<dyn StoreBackend>,
) {
    let key = format!("{}/{}", dnat.service_ns, dnat.service_name);
    let Some(selector) = service_selector(snap, &dnat.service_ns, &dnat.service_name) else {
        return;
    };
    if selector.is_empty() {
        return;
    }

    let backends = resolve_backend_pods(store, &selector, &dnat.service_ns, dnat.target_port).await;
    if backends.is_empty() {
        return;
    }

    if let Some(cip) = dnat.cluster_ip {
        if let Err(e) = nft.add_dnat(cip, dnat.listen_port, &backends).await {
            tracing::error!("SyncNetwork: add_dnat {}: {:?}", key, e);
        }
    }

    if dnat.is_nodeport {
        if let Some(np) = dnat.node_port {
            if let Err(e) = nft.add_nodeport_dnat(np, &backends).await {
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
        let AnyResource::Service(svc) = &t.spec else {
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
    selector: &BTreeMap<String, String>,
    ns: &str,
    port: u16,
) -> Vec<(Ipv4Addr, u16)> {
    let pod_trackers = store.get_by_kind("Pod").await;
    let mut backends = Vec::new();
    for t in &pod_trackers {
        if let AnyResource::Pod(pod) = &t.spec {
            if pod.metadata.namespace.as_deref().unwrap_or("default") != ns {
                continue;
            }
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            if !selector.iter().all(|(k, v)| labels.get(k) == Some(v)) {
                continue;
            }
            if let Some(pod_ip_str) = t.status.pod_ip.as_deref() {
                if let Ok(pod_ip) = pod_ip_str.parse::<Ipv4Addr>() {
                    backends.push((pod_ip, port));
                }
            }
        }
    }
    backends
}

/// Remove service DNAT state (API delete path).
pub async fn remove_service(
    nft: &Arc<NftEngine>,
    store: &Arc<dyn StoreBackend>,
    ns: &str,
    name: &str,
) {
    let trackers = store.get_by_kind("Service").await;
    for t in &trackers {
        if t.spec.namespace() == Some(ns) && t.spec.name() == name {
            if let AnyResource::Service(svc) = &t.spec {
                if let Some(spec) = &svc.spec {
                    if let Some(cip) = &spec.cluster_ip {
                        if let Ok(ip) = cip.parse::<Ipv4Addr>() {
                            for svc_port in spec.ports.as_deref().unwrap_or(&[]) {
                                if let Err(e) = nft.remove_dnat(ip, svc_port.port).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use z8s_core::store::MemoryBackend;
    use z8s_core::types::{ObjectMeta, Pod, PodSpec, Service, ServicePort, ServiceSpec};

    #[tokio::test]
    async fn reconcile_empty_snapshot_is_noop() {
        let snap = StoreSnapshot::empty();
        let nft = Arc::new(NftEngine::new("node-a"));
        let dns = DnsState::new();
        let ingress = IngressState::new();
        let npc = NetworkPolicyController::new(nft.clone());
        let store: Arc<dyn StoreBackend> = Arc::new(MemoryBackend::new());
        // Should not panic
        reconcile_network(
            &snap,
            &nft,
            &dns,
            &ingress,
            &npc,
            "node-a",
            "10.42.0.1".parse().unwrap(),
            &store,
        )
        .await;
    }

    #[tokio::test]
    async fn reconcile_dns_records_populated() {
        let mut selector = BTreeMap::new();
        selector.insert("app".into(), "web".into());
        let svc = Service {
            api_version: "v1".into(),
            kind: "Service".into(),
            metadata: ObjectMeta {
                name: Some("web".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                selector: Some(selector),
                cluster_ip: Some("10.96.0.10".into()),
                type_: Some("ClusterIP".into()),
                ports: Some(vec![ServicePort {
                    name: "http".to_string(),
                    port: 80,
                    target_port: Some(80),
                    node_port: None,
                    protocol: None,
                }]),
            }),
            ..Default::default()
        };
        let pod = Pod {
            metadata: ObjectMeta {
                name: Some("web-1".into()),
                namespace: Some("default".into()),
                labels: Some(BTreeMap::from([("app".into(), "web".into())])),
                uid: Some("uid-web-1".into()),
                ..Default::default()
            },
            spec: Some(PodSpec::default()),
            ..Default::default()
        };
        let snap = StoreSnapshot::from_records(vec![
            z8s_core::types::ResourceRecord {
                spec: AnyResource::Pod(pod),
                status: z8s_core::types::ResourceStatus {
                    pod_ip: Some("10.42.0.5".into()),
                    ..Default::default()
                },
                generation: 1,
                observed_generation: 1,
                assigned_node: Some("node-a".into()),
                last_updated: 0,
            },
            z8s_core::types::ResourceRecord {
                spec: AnyResource::Service(svc),
                status: Default::default(),
                generation: 1,
                observed_generation: 1,
                assigned_node: None,
                last_updated: 0,
            },
        ]);
        let nft = Arc::new(NftEngine::new("node-a"));
        let dns = DnsState::new();
        let ingress = IngressState::new();
        let npc = NetworkPolicyController::new(nft.clone());
        let store: Arc<dyn StoreBackend> = Arc::new(MemoryBackend::new());
        reconcile_network(
            &snap,
            &nft,
            &dns,
            &ingress,
            &npc,
            "node-a",
            "10.42.0.1".parse().unwrap(),
            &store,
        )
        .await;
        // DNS state should have the cluster.local and short forms + API record
        assert!(dns.resolve("web.default.svc.cluster.local").is_some());
        assert!(dns.resolve("kubernetes.default.svc.cluster.local").is_some());
    }
}
