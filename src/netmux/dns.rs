use crate::types::{ResourceStore, AnyResource};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

const TTL: u32 = 30;
const MAX_UDP: usize = 4096;

pub async fn run_dns(store: Arc<ResourceStore>) -> Option<u16> {
    let cfg = crate::config::get();
    let ports: Vec<u16> = if let Some(p) = cfg.dns_port {
        vec![p]
    } else {
        vec![53, 5353]
    };
    for port in ports {
        if let Ok(sock) = UdpSocket::bind(format!("127.0.0.1:{}", port)).await {
            info!("DNS server listening on 127.0.0.1:{}", port);
            tokio::spawn(dns_loop(sock, store));
            return Some(port);
        }
    }
    warn!("DNS: failed to bind to configured DNS port(s) — in-cluster DNS disabled");
    None
}

async fn dns_loop(sock: UdpSocket, store: Arc<ResourceStore>) {
    let sock = Arc::new(sock);
    let mut buf = [0u8; MAX_UDP];
    let upstream = read_upstream_dns();
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, src)) => {
                let query = buf[..n].to_vec();
                let sock = sock.clone();
                let store = store.clone();
                let upstream = upstream.clone();
                tokio::spawn(async move {
                    if let Some(resp) = handle_query(&query, &store, &upstream).await {
                        sock.send_to(&resp, src).await.ok();
                    }
                });
            }
            Err(e) => warn!("DNS recv: {}", e),
        }
    }
}

fn read_upstream_dns() -> Vec<String> {
    let content = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    content
        .lines()
        .filter(|l| l.starts_with("nameserver "))
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|ip| *ip != "127.0.0.1") // don't forward to ourselves
        .map(|ip| format!("{}:53", ip))
        .collect()
}

// ── DNS packet helpers ────────────────────────────────────────────────────────

fn parse_name(buf: &[u8], offset: &mut usize) -> Option<String> {
    let mut labels = Vec::new();
    let mut cur = *offset;
    let mut jumped = false;
    let mut steps = 0;

    loop {
        if cur >= buf.len() || steps > 50 {
            return None;
        }
        steps += 1;
        let len = buf[cur] as usize;

        if len == 0 {
            if !jumped {
                *offset = cur + 1;
            }
            break;
        }

        if (len & 0xC0) == 0xC0 {
            if cur + 1 >= buf.len() {
                return None;
            }
            let ptr = ((len & 0x3F) << 8) | buf[cur + 1] as usize;
            if !jumped {
                *offset = cur + 2;
            }
            cur = ptr;
            jumped = true;
            continue;
        }

        cur += 1;
        if cur + len > buf.len() {
            return None;
        }
        labels.push(std::str::from_utf8(&buf[cur..cur + len]).ok()?.to_lowercase());
        cur += len;
    }

    Some(labels.join("."))
}

fn encode_name(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

fn make_response(
    id: u16,
    rd: bool,
    question_bytes: &[u8],
    answers: &[Vec<u8>],
    rcode: u8,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    // Header
    out.extend_from_slice(&id.to_be_bytes());
    let flags: u16 = 0x8000                          // QR=1 (response)
        | if rd { 0x0180 } else { 0x0100 }           // RA|RD
        | (rcode as u16 & 0xF);
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());       // QDCOUNT
    out.extend_from_slice(&(answers.len() as u16).to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes());       // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes());       // ARCOUNT
    // Question (copied verbatim)
    out.extend_from_slice(question_bytes);
    // Answers
    for a in answers {
        out.extend_from_slice(a);
    }
    out
}

fn a_record(name: &str, ip: [u8; 4]) -> Vec<u8> {
    let mut rr = Vec::new();
    rr.extend_from_slice(&encode_name(name));
    rr.extend_from_slice(&1u16.to_be_bytes());   // TYPE A
    rr.extend_from_slice(&1u16.to_be_bytes());   // CLASS IN
    rr.extend_from_slice(&TTL.to_be_bytes());
    rr.extend_from_slice(&4u16.to_be_bytes());   // RDLENGTH
    rr.extend_from_slice(&ip);
    rr
}

fn cname_record(name: &str, target: &str) -> Vec<u8> {
    let encoded_target = encode_name(target);
    let mut rr = Vec::new();
    rr.extend_from_slice(&encode_name(name));
    rr.extend_from_slice(&5u16.to_be_bytes());   // TYPE CNAME
    rr.extend_from_slice(&1u16.to_be_bytes());   // CLASS IN
    rr.extend_from_slice(&TTL.to_be_bytes());
    rr.extend_from_slice(&(encoded_target.len() as u16).to_be_bytes());
    rr.extend_from_slice(&encoded_target);
    rr
}

// ── Resolution logic ──────────────────────────────────────────────────────────

async fn handle_query(
    query: &[u8],
    store: &ResourceStore,
    upstream: &[String],
) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }

    let id = u16::from_be_bytes([query[0], query[1]]);
    let flags = u16::from_be_bytes([query[2], query[3]]);
    if flags & 0x8000 != 0 {
        return None; // ignore responses
    }
    let rd = flags & 0x0100 != 0;
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    if qdcount == 0 {
        return None;
    }

    let mut offset = 12usize;
    let q_start = offset;
    let name = parse_name(query, &mut offset)?;
    if offset + 4 > query.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([query[offset], query[offset + 1]]);
    offset += 4;
    let question_bytes = &query[q_start..offset];

    debug!("DNS query: {} type={}", name, qtype);

    // Only handle IN class (type A=1, AAAA=28, ANY=255)
    if qtype != 1 && qtype != 28 && qtype != 255 {
        return forward(query, upstream).await;
    }

    // Try to resolve as a service name
    match resolve_service(&name, store).await {
        ServiceResolution::ClusterIP(cluster_ip) => {
            let ip_bytes = parse_ipv4(&cluster_ip)?;
            let answers = if qtype == 28 {
                vec![] // AAAA — no IPv6, NOERROR empty
            } else {
                vec![a_record(&name, ip_bytes)]
            };
            return Some(make_response(id, rd, question_bytes, &answers, 0));
        }
        ServiceResolution::ExternalName(external_name) => {
            // For ExternalName services return CNAME pointing to external_name.
            // If it looks like an IP, return it as an A record directly.
            if let Some(ip_bytes) = parse_ipv4(&external_name) {
                let answers = if qtype == 28 { vec![] } else { vec![a_record(&name, ip_bytes)] };
                return Some(make_response(id, rd, question_bytes, &answers, 0));
            }
            let answers = if qtype == 28 { vec![] } else { vec![cname_record(&name, &external_name)] };
            return Some(make_response(id, rd, question_bytes, &answers, 0));
        }
        ServiceResolution::None => {}
    }

    // Not a service name — forward if RD set
    if rd {
        if !upstream.is_empty() {
            forward(query, upstream).await
        } else {
            // No upstream configured in /etc/resolv.conf, use defaults
            let default_upstream: Vec<String> = vec!["1.1.1.1:53".into(), "8.8.8.8:53".into()];
            forward(query, &default_upstream).await
        }
    } else {
        Some(make_response(id, rd, question_bytes, &[], 3)) // NXDOMAIN
    }
}

enum ServiceResolution {
    ClusterIP(String),
    ExternalName(String),
    None,
}

/// Resolve a DNS name to a service endpoint.
async fn resolve_service(name: &str, store: &ResourceStore) -> ServiceResolution {
    let name = name.trim_end_matches('.');
    let Some((svc_name, ns_hint)) = parse_service_name(name) else {
        return ServiceResolution::None;
    };
    let trackers = store.get_by_kind("Service").await;

    for t in &trackers {
        if let AnyResource::Service(svc) = &t.resource {
            let svc_n = svc.metadata.name.as_deref().unwrap_or_default();
            let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default");

            if svc_n != svc_name {
                continue;
            }
            if let Some(ref hint) = ns_hint {
                if svc_ns != hint.as_str() {
                    continue;
                }
            }

            let spec = match svc.spec.as_ref() {
                Some(s) => s,
                None => continue,
            };

            // ExternalName service — return CNAME to external_name
            if spec.type_.as_deref() == Some("ExternalName") {
                if let Some(ext) = spec.external_name.as_deref() {
                    if !ext.is_empty() {
                        return ServiceResolution::ExternalName(ext.to_string());
                    }
                }
                return ServiceResolution::None;
            }

            // Regular ClusterIP service
            let ip = spec.cluster_ip.as_deref().unwrap_or("None");
            if ip == "None" || ip.is_empty() {
                return ServiceResolution::None;
            }
            return ServiceResolution::ClusterIP(ip.to_string());
        }
    }
    ServiceResolution::None
}

/// Parse a DNS name into (service_name, optional_namespace).
fn parse_service_name(name: &str) -> Option<(String, Option<String>)> {
    let parts: Vec<&str> = name.split('.').collect();
    let n = parts.len();
    let domain = crate::config::get().cluster_domain.clone();
    let domain_parts: Vec<&str> = domain.split('.').collect();
    let dn = domain_parts.len();

    // <svc>.<ns>.svc.<cluster-domain>
    let full_prefix = dn + 2; // +svc +ns
    if n >= full_prefix + 1 {
        let tail = &parts[n - dn..];
        if tail == domain_parts.as_slice() && parts[n - dn - 1] == "svc" {
            let ns = parts[n - dn - 2].to_string();
            let svc = parts[..n - dn - 2].join(".");
            return Some((svc, Some(ns)));
        }
    }
    // <svc>.svc.<cluster-domain>
    if n >= dn + 2 {
        let tail = &parts[n - dn..];
        if tail == domain_parts.as_slice() && parts[n - dn - 1] == "svc" {
            return Some((parts[0].to_string(), None));
        }
    }
    // <svc>.<cluster-domain>
    if n >= dn + 1 {
        let tail = &parts[n - dn..];
        if tail == domain_parts.as_slice() {
            return Some((parts[0].to_string(), None));
        }
    }
    // bare name
    if n == 1 {
        return Some((parts[0].to_string(), None));
    }
    // <svc>.<ns>
    if n == 2 {
        return Some((parts[0].to_string(), Some(parts[1].to_string())));
    }
    None
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    Some([
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
        parts[3].parse().ok()?,
    ])
}

/// Forward a DNS query to upstream resolvers and return their response.
async fn forward(query: &[u8], upstream: &[String]) -> Option<Vec<u8>> {
    for addr in upstream {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await {
            if sock.send_to(query, addr).await.is_ok() {
                let mut buf = [0u8; MAX_UDP];
                match tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    sock.recv(&mut buf),
                )
                .await
                {
                    Ok(Ok(n)) => return Some(buf[..n].to_vec()),
                    _ => continue,
                }
            }
        }
    }
    None
}
