//! # In-Cluster DNS Server
//!
//! A minimal UDP DNS server that resolves:
//! - **Service names** (`myservice.mynamespace.svc.cluster.local` → ClusterIP)
//! - **Ingress hostnames** (`myapp.example.com` → gateway IP)
//! - **ExternalName services** (CNAME to external target)
//! - **Custom records** (populated by ingress controller)
//!
//! Everything else is forwarded to upstream resolvers (`/etc/resolv.conf`).
//!
//! ## Caching
//!
//! The DNS server keeps an in-memory cache that is rebuilt whenever
//! `apply_dns_records` is called. This avoids repeated store lookups
//! per query (the bottleneck called out in PERFORMANCE-PLAN.md §13).
//!
//! ## Wire Format
//!
//! The server implements just enough DNS to answer A and CNAME queries.
//! It does not implement AXFR, IXFR, TSIG, or DNSSEC — those are not
//! needed for in-cluster resolution.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

const TTL: u32 = 30;
const MAX_UDP: usize = 4096;

/// Default upstream resolvers if `/etc/resolv.conf` is missing or empty.
pub const DEFAULT_UPSTREAM: &[&str] = &["1.1.1.1:53", "8.8.8.8:53"];

/// A snapshot of DNS records built from the store.
#[derive(Debug, Clone, Default)]
pub struct DnsSnapshot {
    /// Fully qualified hostname → IP
    pub records: HashMap<String, Ipv4Addr>,
    /// Hostname → CNAME target
    pub cnames: HashMap<String, String>,
}

impl DnsSnapshot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, hostname: impl Into<String>, ip: Ipv4Addr) {
        let key = hostname.into().to_lowercase();
        self.records.insert(key, ip);
    }

    pub fn insert_cname(&mut self, hostname: impl Into<String>, target: impl Into<String>) {
        let key = hostname.into().to_lowercase();
        self.cnames.insert(key, target.into());
    }

    pub fn resolve(&self, name: &str) -> Option<DnsAnswer> {
        let name = name.trim_end_matches('.').to_lowercase();
        if let Some(&ip) = self.records.get(&name) {
            return Some(DnsAnswer::A(ip));
        }
        if let Some(target) = self.cnames.get(&name) {
            // Follow CNAME chain (one level deep)
            if let Some(&ip) = self.records.get(target) {
                return Some(DnsAnswer::A(ip));
            }
            return Some(DnsAnswer::Cname(target.clone()));
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsAnswer {
    A(Ipv4Addr),
    Cname(String),
}

/// Shared DNS state — updated by the network reconciler, read by the DNS server.
#[derive(Clone, Default)]
pub struct DnsState {
    inner: Arc<RwLock<DnsSnapshot>>,
}

impl DnsState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entire snapshot atomically.
    pub fn replace(&self, snapshot: DnsSnapshot) {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        *guard = snapshot;
    }

    /// Look up a name.
    pub fn resolve(&self, name: &str) -> Option<DnsAnswer> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .resolve(name)
    }

    /// Number of records (for tests/metrics).
    pub fn len(&self) -> usize {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Try to bind to a DNS port; returns the bound port and socket.
pub async fn bind_dns(ports: &[u16]) -> Option<(u16, UdpSocket)> {
    for &port in ports {
        if let Ok(sock) = UdpSocket::bind(format!("0.0.0.0:{}", port)).await {
            info!("DNS server bound to 0.0.0.0:{}", port);
            return Some((port, sock));
        }
    }
    warn!("DNS: failed to bind to configured ports {:?}", ports);
    None
}

/// Start the DNS server loop. Returns the bound port.
pub async fn run_dns(state: DnsState, ports: Vec<u16>) -> Option<u16> {
    let (port, sock) = bind_dns(&ports).await?;
    let sock = Arc::new(sock);
    let upstream = read_upstream_dns();
    tokio::spawn(dns_loop(sock, state, upstream));
    Some(port)
}

async fn dns_loop(sock: Arc<UdpSocket>, state: DnsState, upstream: Vec<String>) {
    let mut buf = [0u8; MAX_UDP];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, src)) => {
                let query = buf[..n].to_vec();
                let sock = sock.clone();
                let state = state.clone();
                let upstream = upstream.clone();
                tokio::spawn(async move {
                    if let Some(resp) = handle_query(&query, &state, &upstream).await {
                        if let Err(e) = sock.send_to(&resp, src).await {
                            warn!("DNS send error: {}", e);
                        }
                    }
                });
            }
            Err(e) => warn!("DNS recv: {}", e),
        }
    }
}

fn read_upstream_dns() -> Vec<String> {
    let content = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    let parsed: Vec<String> = content
        .lines()
        .filter(|l| l.starts_with("nameserver "))
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|ip| *ip != "127.0.0.1")
        .map(|ip| format!("{}:53", ip))
        .collect();
    if parsed.is_empty() {
        DEFAULT_UPSTREAM.iter().map(|s| s.to_string()).collect()
    } else {
        parsed
    }
}

// ── DNS packet parser ───────────────────────────────────────────────────

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
        labels.push(
            std::str::from_utf8(&buf[cur..cur + len])
                .ok()?
                .to_lowercase(),
        );
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
    out.extend_from_slice(&id.to_be_bytes());
    let flags: u16 = 0x8000 | if rd { 0x0180 } else { 0x0100 } | (rcode as u16 & 0xF);
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&(answers.len() as u16).to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out.extend_from_slice(question_bytes);
    for a in answers {
        out.extend_from_slice(a);
    }
    out
}

fn a_record(name: &str, ip: [u8; 4]) -> Vec<u8> {
    let mut rr = Vec::new();
    rr.extend_from_slice(&encode_name(name));
    rr.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
    rr.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    rr.extend_from_slice(&TTL.to_be_bytes());
    rr.extend_from_slice(&4u16.to_be_bytes());
    rr.extend_from_slice(&ip);
    rr
}

fn cname_record(name: &str, target: &str) -> Vec<u8> {
    let encoded_target = encode_name(target);
    let mut rr = Vec::new();
    rr.extend_from_slice(&encode_name(name));
    rr.extend_from_slice(&5u16.to_be_bytes()); // TYPE CNAME
    rr.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    rr.extend_from_slice(&TTL.to_be_bytes());
    rr.extend_from_slice(&(encoded_target.len() as u16).to_be_bytes());
    rr.extend_from_slice(&encoded_target);
    rr
}

// ── Query handling ──────────────────────────────────────────────────────

async fn handle_query(query: &[u8], state: &DnsState, upstream: &[String]) -> Option<Vec<u8>> {
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

    // Only A (1), AAAA (28), and ANY (255) — everything else forward
    if qtype != 1 && qtype != 28 && qtype != 255 {
        return forward(query, upstream).await;
    }

    // Look up in our cache
    if let Some(answer) = state.resolve(&name) {
        let answers: Vec<Vec<u8>> = match (answer, qtype) {
            (DnsAnswer::A(ip), 1) | (DnsAnswer::A(ip), 255) => {
                vec![a_record(&name, ip.octets())]
            }
            (DnsAnswer::A(_), 28) => vec![], // AAAA query, no AAAA record
            (DnsAnswer::Cname(target), 1) | (DnsAnswer::Cname(target), 255) => {
                vec![cname_record(&name, &target)]
            }
            (DnsAnswer::Cname(_), 28) => vec![],
            _ => vec![], // any other qtype: no answer in cache
        };
        return Some(make_response(id, rd, question_bytes, &answers, 0));
    }

    if rd {
        forward(query, upstream).await
    } else {
        Some(make_response(id, rd, question_bytes, &[], 3)) // NXDOMAIN
    }
}

async fn forward(query: &[u8], upstream: &[String]) -> Option<Vec<u8>> {
    for addr in upstream {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await {
            if sock.send_to(query, addr).await.is_ok() {
                let mut buf = [0u8; MAX_UDP];
                match tokio::time::timeout(Duration::from_secs(3), sock.recv(&mut buf)).await {
                    Ok(Ok(n)) => return Some(buf[..n].to_vec()),
                    _ => continue,
                }
            }
        }
    }
    None
}

// ── Name parsing ────────────────────────────────────────────────────────

/// Parse a DNS name to determine if it's a service name like
/// `myservice.mynamespace.svc.cluster.local`.
///
/// Returns `(service_name, optional_namespace)` or `None` if it's not a service name.
pub fn parse_service_name(name: &str, cluster_domain: &str) -> Option<(String, Option<String>)> {
    let parts: Vec<&str> = name.split('.').collect();
    let n = parts.len();
    let domain_parts: Vec<&str> = cluster_domain.split('.').collect();
    let dn = domain_parts.len();

    // <svc>.svc.<cluster-domain>  (no namespace) — only when the count is exactly dn+2
    if n == dn + 2 {
        let tail = &parts[n - dn..];
        if tail == domain_parts.as_slice() && parts[n - dn - 1] == "svc" {
            return Some((parts[0].to_string(), None));
        }
    }
    // <svc>.<ns>.svc.<cluster-domain>  (3 or more parts before "svc")
    if n >= dn + 3 {
        let tail = &parts[n - dn..];
        if tail == domain_parts.as_slice() && parts[n - dn - 1] == "svc" {
            let ns = parts[n - dn - 2].to_string();
            let svc = parts[..n - dn - 2].join(".");
            return Some((svc, Some(ns)));
        }
    }
    // <svc>.<cluster-domain>
    if n >= dn + 1 {
        let tail = &parts[n - dn..];
        if tail == domain_parts.as_slice() {
            return Some((parts[0].to_string(), None));
        }
    }
    // <svc>.<ns>
    if n == 2 {
        return Some((parts[0].to_string(), Some(parts[1].to_string())));
    }
    // bare name
    if n == 1 && !parts[0].is_empty() {
        return Some((parts[0].to_string(), None));
    }
    None
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let name = "foo.bar.example.com";
        let encoded = encode_name(name);
        let mut offset = 0;
        let decoded = parse_name(&encoded, &mut offset).unwrap();
        assert_eq!(decoded, name);
    }

    #[test]
    fn encode_empty_label_terminator() {
        let encoded = encode_name("foo");
        assert_eq!(*encoded.last().unwrap(), 0);
    }

    #[test]
    fn a_record_layout() {
        let rr = a_record("test.local", [10, 0, 0, 5]);
        // Skip the name (variable length); check tail
        let n = rr.len();
        assert_eq!(&rr[n - 4..], &[10, 0, 0, 5]);
    }

    #[test]
    fn parse_service_name_full() {
        let (svc, ns) = parse_service_name("web.default.svc.cluster.local", "cluster.local").unwrap();
        assert_eq!(svc, "web");
        assert_eq!(ns, Some("default".to_string()));
    }

    #[test]
    fn parse_service_name_no_ns() {
        let (svc, ns) = parse_service_name("web.svc.cluster.local", "cluster.local").unwrap();
        assert_eq!(svc, "web");
        assert_eq!(ns, None);
    }

    #[test]
    fn parse_service_name_short() {
        let (svc, ns) = parse_service_name("web.cluster.local", "cluster.local").unwrap();
        assert_eq!(svc, "web");
        assert_eq!(ns, None);
    }

    #[test]
    fn parse_service_name_svc_ns() {
        let (svc, ns) = parse_service_name("web.default", "cluster.local").unwrap();
        assert_eq!(svc, "web");
        assert_eq!(ns, Some("default".to_string()));
    }

    #[test]
    fn parse_service_name_bare() {
        let (svc, ns) = parse_service_name("web", "cluster.local").unwrap();
        assert_eq!(svc, "web");
        assert_eq!(ns, None);
    }

    #[test]
    fn snapshot_resolve_a() {
        let mut s = DnsSnapshot::new();
        s.insert("web.default.svc.cluster.local", Ipv4Addr::new(10, 96, 0, 10));
        let r = s.resolve("web.default.svc.cluster.local").unwrap();
        assert_eq!(r, DnsAnswer::A(Ipv4Addr::new(10, 96, 0, 10)));
    }

    #[test]
    fn snapshot_resolve_cname() {
        let mut s = DnsSnapshot::new();
        s.insert_cname("foo.example.com", "bar.example.com");
        s.insert("bar.example.com", Ipv4Addr::new(1, 2, 3, 4));
        let r = s.resolve("foo.example.com").unwrap();
        assert_eq!(r, DnsAnswer::A(Ipv4Addr::new(1, 2, 3, 4)));
    }

    #[test]
    fn snapshot_resolve_trailing_dot() {
        let mut s = DnsSnapshot::new();
        s.insert("web.local", Ipv4Addr::new(10, 0, 0, 1));
        assert!(s.resolve("web.local.").is_some());
    }

    #[test]
    fn snapshot_resolve_case_insensitive() {
        let mut s = DnsSnapshot::new();
        s.insert("Web.Local", Ipv4Addr::new(10, 0, 0, 1));
        assert!(s.resolve("web.local").is_some());
    }

    #[test]
    fn state_replaces_atomically() {
        let state = DnsState::new();
        let mut s = DnsSnapshot::new();
        s.insert("a.local", Ipv4Addr::new(10, 0, 0, 1));
        state.replace(s);
        assert_eq!(state.len(), 1);

        let mut s2 = DnsSnapshot::new();
        s2.insert("b.local", Ipv4Addr::new(10, 0, 0, 2));
        s2.insert("c.local", Ipv4Addr::new(10, 0, 0, 3));
        state.replace(s2);
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn state_resolve_unknown_returns_none() {
        let state = DnsState::new();
        assert!(state.resolve("unknown").is_none());
    }
}
