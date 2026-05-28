use crate::api::types::ResourceStore;
use crate::net::PodResolver;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};
#[allow(unused_imports)]
use nix::libc;

/// Wait until at least one backend is ready before binding ClusterIP. On the host
/// network stack, binding ClusterIP:port before the pod listens can block the pod
/// from binding 0.0.0.0:targetPort.
pub async fn run_proxy_addr_when_ready(
    listen_addr: &str,
    selector: BTreeMap<String, String>,
    target_port: IntOrString,
    store: Arc<ResourceStore>,
    resolver: Arc<dyn PodResolver>,
    counter: Arc<AtomicUsize>,
    svc_name: &str,
    svc_ns: &str,
) {
    for _ in 0..120 {
        let endpoints = find_endpoints(&selector, &target_port, &store, &*resolver, svc_ns).await;
        if !endpoints.is_empty() {
            let probe = format!("{}:{}", endpoints[0].host, endpoints[0].port);
            if TcpStream::connect(&probe).await.is_ok() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    run_proxy_addr(
        listen_addr,
        selector,
        target_port,
        store,
        resolver,
        counter,
        svc_name,
        svc_ns,
    )
    .await;
}

/// Add `ip` as a secondary address on loopback via netlink RTM_NEWADDR.
/// Uses `AF_NETLINK` / `NETLINK_ROUTE` — requires CAP_NET_ADMIN.
/// Unlike SIOCSIFADDR this does NOT replace the primary loopback address.
fn ensure_loopback_alias(ip: &str) {
    use std::net::Ipv4Addr;
    use std::ptr;

    let addr: Ipv4Addr = match ip.parse() {
        Ok(a) => a,
        Err(_) => {
            warn!("ensure_loopback_alias: only IPv4 is supported, got {}", ip);
            return;
        }
    };
    let s_addr = u32::from(addr).to_be();

    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        );
        if fd < 0 {
            return;
        }

        // Get interface index of "lo" via SIOCGIFINDEX.
        let mut ifr: libc::ifreq = std::mem::zeroed();
        let name_bytes = b"lo\0";
        for (i, &b) in name_bytes.iter().enumerate() {
            ifr.ifr_name[i] = b as libc::c_char;
        }
        if libc::ioctl(fd, libc::SIOCGIFINDEX, &mut ifr as *mut _) < 0 {
            libc::close(fd);
            return;
        }
        let if_idx = ifr.ifr_ifru.ifru_ifindex;

        // Build RTM_NEWADDR payload as raw bytes.
        // Layout: nlmsghdr(16) + ifaddrmsg(8) + rtattr(4) + addr(4) + rtattr(4) + addr(4) = 40
        let mut buf = [0u8; 48];

        // nlmsghdr
        let total_len: u32 = 40;
        ptr::write(buf.as_mut_ptr().add(0) as *mut u32, total_len);               // nlmsg_len
        ptr::write(buf.as_mut_ptr().add(4) as *mut u16, libc::RTM_NEWADDR as u16); // nlmsg_type
        ptr::write(
            buf.as_mut_ptr().add(6) as *mut u16,
            (libc::NLM_F_REQUEST | libc::NLM_F_CREATE | libc::NLM_F_EXCL) as u16,
        );
        ptr::write(buf.as_mut_ptr().add(8) as *mut u32, 1u32);                     // nlmsg_seq
        ptr::write(buf.as_mut_ptr().add(12) as *mut u32, 0u32);                    // nlmsg_pid

        // ifaddrmsg
        ptr::write(buf.as_mut_ptr().add(16) as *mut u8, libc::AF_INET as u8);      // ifa_family
        ptr::write(buf.as_mut_ptr().add(17) as *mut u8, 32u8);                     // ifa_prefixlen
        ptr::write(buf.as_mut_ptr().add(18) as *mut u8, 0u8);                      // ifa_flags
        ptr::write(buf.as_mut_ptr().add(19) as *mut u8, 0u8);                      // ifa_scope
        ptr::write(buf.as_mut_ptr().add(20) as *mut u32, if_idx as u32);           // ifa_index

        // RTA attr: IFA_LOCAL
        ptr::write(buf.as_mut_ptr().add(24) as *mut u16, 8u16);                    // rta_len
        ptr::write(buf.as_mut_ptr().add(26) as *mut u16, libc::IFA_LOCAL as u16);   // rta_type
        ptr::write(buf.as_mut_ptr().add(28) as *mut u32, s_addr);                  // address

        // RTA attr: IFA_ADDRESS (same address)
        ptr::write(buf.as_mut_ptr().add(32) as *mut u16, 8u16);                    // rta_len
        ptr::write(buf.as_mut_ptr().add(34) as *mut u16, libc::IFA_ADDRESS as u16); // rta_type
        ptr::write(buf.as_mut_ptr().add(36) as *mut u32, s_addr);                  // address

        let iov = libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut libc::c_void,
            iov_len: total_len as usize,
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &iov as *const _ as *mut _;
        msg.msg_iovlen = 1;

        // Send (no ACK requested — errors like EEXIST for duplicate addresses are expected).
        libc::sendmsg(fd, &msg, 0);
        libc::close(fd);
    }
}

pub async fn run_proxy_addr(
    listen_addr: &str,
    selector: BTreeMap<String, String>,
    target_port: IntOrString,
    store: Arc<ResourceStore>,
    resolver: Arc<dyn PodResolver>,
    counter: Arc<AtomicUsize>,
    svc_name: &str,
    svc_ns: &str,
) {
    // Extract the IP part from listen_addr (e.g. "127.96.0.3:80" → "127.96.0.3")
    let clusterip = listen_addr.split(':').next().unwrap_or("").to_string();

    let listener = loop {
        match TcpListener::bind(listen_addr).await {
            Ok(l) => break l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                warn!(
                    "Service proxy {}/{}: {} in use, retrying in 2s",
                    svc_ns, svc_name, listen_addr
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) if e.raw_os_error() == Some(nix::libc::EADDRNOTAVAIL) => {
                // ClusterIP not assigned to any interface — add it to loopback and retry.
                if !clusterip.is_empty() {
                    ensure_loopback_alias(&clusterip);
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => {
                warn!("Service proxy {}/{}: failed to bind {}: {}", svc_ns, svc_name, listen_addr, e);
                return;
            }
        }
    };
    info!("Service proxy {}/{} listening on {}", svc_ns, svc_name, listen_addr);

    loop {
        match listener.accept().await {
            Ok((client, _peer)) => {
                let store = store.clone();
                let resolver = resolver.clone();
                let counter = counter.clone();
                let selector = selector.clone();
                let target_port = target_port.clone();
                let svc_ns = svc_ns.to_string();
                let svc_name = svc_name.to_string();
                tokio::spawn(async move {
                    handle_connection(client, selector, target_port, store, resolver, counter, &svc_ns, &svc_name).await;
                });
            }
            Err(e) => {
                warn!("Service proxy accept error: {}", e);
                break;
            }
        }
    }
}

pub async fn run_proxy(
    listen_port: u16,
    selector: BTreeMap<String, String>,
    target_port: IntOrString,
    store: Arc<ResourceStore>,
    resolver: Arc<dyn PodResolver>,
    counter: Arc<AtomicUsize>,
    svc_name: &str,
    svc_ns: &str,
) {
    let addr = format!("0.0.0.0:{}", listen_port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            warn!("Service proxy {}/{}: failed to bind {}: {}", svc_ns, svc_name, addr, e);
            return;
        }
    };
    info!("Service proxy {}/{} listening on {}", svc_ns, svc_name, addr);

    loop {
        match listener.accept().await {
            Ok((client, peer)) => {
                info!("Service proxy {}/{}: new connection from {}", svc_ns, svc_name, peer);
                let store = store.clone();
                let resolver = resolver.clone();
                let counter = counter.clone();
                let selector = selector.clone();
                let target_port = target_port.clone();
                let svc_ns = svc_ns.to_string();
                let svc_name = svc_name.to_string();
                tokio::spawn(async move {
                    handle_connection(client, selector, target_port, store, resolver, counter, &svc_ns, &svc_name).await;
                });
            }
            Err(e) => {
                warn!("Service proxy accept error: {}", e);
                break;
            }
        }
    }
}

async fn handle_connection(
    mut client: TcpStream,
    selector: BTreeMap<String, String>,
    target_port: IntOrString,
    store: Arc<ResourceStore>,
    resolver: Arc<dyn PodResolver>,
    counter: Arc<AtomicUsize>,
    svc_ns: &str,
    svc_name: &str,
) {
    let endpoints = find_endpoints(&selector, &target_port, &store, &*resolver, svc_ns).await;
    if endpoints.is_empty() {
        warn!("Service {}/{}: no endpoints available", svc_ns, svc_name);
        return;
    }

    let idx = counter.fetch_add(1, Ordering::Relaxed) % endpoints.len();
    let endpoint = &endpoints[idx];

    let mut backend = match TcpStream::connect(format!("{}:{}", endpoint.host, endpoint.port)).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Service {}/{}: failed to connect to {}:{}: {}", svc_ns, svc_name, endpoint.host, endpoint.port, e);
            return;
        }
    };

    info!("Service {}/{}: proxying to {}:{}", svc_ns, svc_name, endpoint.host, endpoint.port);
    tokio::io::copy_bidirectional(&mut client, &mut backend).await.ok();
}

async fn find_endpoints(
    selector: &BTreeMap<String, String>,
    target_port: &IntOrString,
    store: &ResourceStore,
    resolver: &dyn PodResolver,
    svc_ns: &str,
) -> Vec<super::ServiceEndpoint> {
    let pod_trackers = store.get_by_kind("Pod").await;
    let mut endpoints = Vec::new();

    for t in &pod_trackers {
        if let crate::api::AnyResource::Pod(pod) = &t.resource {
            if pod.metadata.namespace.as_deref().unwrap_or("default") != svc_ns {
                continue;
            }

            let pod_labels: BTreeMap<String, String> = pod.metadata.labels.clone().unwrap_or_default();
            if !selector.iter().all(|(k, v)| pod_labels.get(k) == Some(v)) {
                continue;
            }

            let pod_name = pod.metadata.name.as_deref().unwrap_or_default();
            if !resolver.is_pod_alive(pod_name).await {
                continue;
            }

            let port = match resolve_container_port(pod, target_port) {
                Some(p) => p,
                None => continue,
            };

            let connect_port = resolver.backend_connect_port(pod_name, port).await;

            let addr = format!("127.0.0.1:{}", connect_port);
            if tokio::time::timeout(Duration::from_millis(500), TcpStream::connect(&addr))
                .await
                .ok()
                .and_then(|r| r.ok())
                .is_none()
            {
                continue;
            }

            endpoints.push(super::ServiceEndpoint {
                host: "127.0.0.1".to_string(),
                port: connect_port,
            });
        }
    }

    endpoints
}

fn resolve_container_port(
    pod: &k8s_openapi::api::core::v1::Pod,
    target_port: &IntOrString,
) -> Option<u16> {
    let spec = pod.spec.as_ref()?;
    match target_port {
        IntOrString::Int(p) => Some(*p as u16),
        IntOrString::String(name) => {
            for container in &spec.containers {
                if let Some(ports) = &container.ports {
                    for cp in ports {
                        if cp.name.as_deref() == Some(name.as_str()) {
                            return Some(cp.container_port as u16);
                        }
                    }
                }
            }
            None
        }
    }
}
