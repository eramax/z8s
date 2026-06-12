//! # Cluster DNS
//!
//! A tiny authoritative DNS server for the cluster zone (e.g.
//! `*.svc.cluster.local`). It answers `A` queries from an in-memory record
//! map that the [`crate::plan`] planner fills from Service ClusterIPs, and
//! optionally forwards everything else to an upstream resolver.
//!
//! ## Design
//!
//! - **Hand-rolled wire codec** — no external DNS crate. We parse just enough
//!   of [RFC 1035](https://www.rfc-editor.org/rfc/rfc1035) to read a single
//!   question and emit a single (or zero) answer.
//! - **Pure parse/build** — [`parse_query`] and [`build_response`] are pure
//!   and unit-tested; [`DnsServer::serve`] is the only IO surface.
//! - **Hot-swappable records** — the record map lives behind an
//!   `Arc<RwLock<..>>` so a reconcile loop can replace it without restarting
//!   the listener (see [`DnsServer::records`]).
//!
//! Only `A` (IPv4) records are authoritative; `AAAA`/others fall through to the
//! upstream (if configured) or return an empty `NOERROR` answer.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{bail, Result};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, warn};

// ═══════════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════════

/// DNS server configuration.
#[derive(Debug, Clone)]
pub struct DnsConfig {
    /// UDP address to bind (e.g. `10.96.0.10:53`).
    pub listen: SocketAddr,
    /// Authoritative cluster domain (e.g. `cluster.local`).
    pub domain: String,
    /// Optional upstream resolver for non-authoritative queries.
    pub upstream: Option<SocketAddr>,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:53".parse().expect("valid default listen addr"),
            domain: "cluster.local".into(),
            upstream: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Zone
// ═══════════════════════════════════════════════════════════════════════════

/// The authoritative record set: lowercase FQDN → IPv4 address.
///
/// Names are stored without a trailing dot and lowercased for case-insensitive
/// matching.
#[derive(Debug, Clone, Default)]
pub struct DnsZone {
    records: HashMap<String, Ipv4Addr>,
}

impl DnsZone {
    /// Build a zone from a hostname→IPv4 map (as produced by the planner).
    pub fn from_records(records: &HashMap<String, Ipv4Addr>) -> Self {
        let records = records
            .iter()
            .map(|(k, v)| (normalize(k), *v))
            .collect();
        Self { records }
    }

    /// Look up an `A` record (case-insensitive, trailing dot ignored).
    pub fn lookup(&self, name: &str) -> Option<Ipv4Addr> {
        self.records.get(&normalize(name)).copied()
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the zone is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Normalize a DNS name: strip a trailing dot and lowercase.
fn normalize(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

// ═══════════════════════════════════════════════════════════════════════════
// Wire format
// ═══════════════════════════════════════════════════════════════════════════

const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;
const FLAG_RESPONSE: u16 = 0x8000;
const FLAG_AA: u16 = 0x0400;
const RCODE_NXDOMAIN: u16 = 0x0003;

/// A parsed DNS question (the part of the query we care about).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// Transaction id (echoed back in the response).
    pub id: u16,
    /// Queried name (dot-joined labels, no trailing dot).
    pub name: String,
    /// Query type (1 = A).
    pub qtype: u16,
    /// Query class (1 = IN).
    pub qclass: u16,
    /// Raw question section bytes (qname + qtype + qclass), echoed verbatim.
    pub raw_question: Vec<u8>,
}

/// Parse the header + first question of a DNS query packet.
pub fn parse_query(buf: &[u8]) -> Result<Question> {
    if buf.len() < 12 {
        bail!("dns packet too short ({} bytes)", buf.len());
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount == 0 {
        bail!("dns query has no questions");
    }

    let mut pos = 12;
    let q_start = pos;
    let mut labels = Vec::new();
    loop {
        if pos >= buf.len() {
            bail!("dns name overruns packet");
        }
        let len = buf[pos] as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        if len & 0xC0 != 0 {
            bail!("dns compression pointer in question (unsupported)");
        }
        if pos + len > buf.len() {
            bail!("dns label overruns packet");
        }
        labels.push(String::from_utf8_lossy(&buf[pos..pos + len]).into_owned());
        pos += len;
    }
    if pos + 4 > buf.len() {
        bail!("dns question truncated");
    }
    let qtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
    let qclass = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]);
    pos += 4;

    Ok(Question {
        id,
        name: labels.join("."),
        qtype,
        qclass,
        raw_question: buf[q_start..pos].to_vec(),
    })
}

/// Build an authoritative response for a question. If `answer` is `Some`, an
/// `A` record is included; otherwise an `NXDOMAIN` response is returned.
pub fn build_response(q: &Question, answer: Option<Ipv4Addr>, ttl: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    let mut flags = FLAG_RESPONSE | FLAG_AA;
    let ancount: u16 = if answer.is_some() { 1 } else { 0 };
    if answer.is_none() {
        flags |= RCODE_NXDOMAIN;
    }

    out.extend_from_slice(&q.id.to_be_bytes());
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // qdcount
    out.extend_from_slice(&ancount.to_be_bytes()); // ancount
    out.extend_from_slice(&0u16.to_be_bytes()); // nscount
    out.extend_from_slice(&0u16.to_be_bytes()); // arcount

    // Question section, echoed verbatim.
    out.extend_from_slice(&q.raw_question);

    if let Some(ip) = answer {
        // Answer: name pointer to the question (offset 12), A/IN, ttl, rdata.
        out.extend_from_slice(&[0xC0, 0x0C]);
        out.extend_from_slice(&TYPE_A.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&ttl.to_be_bytes());
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&ip.octets());
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// Server
// ═══════════════════════════════════════════════════════════════════════════

/// The cluster DNS server. Owns a hot-swappable zone behind an `RwLock`.
pub struct DnsServer {
    config: DnsConfig,
    zone: Arc<RwLock<DnsZone>>,
}

impl DnsServer {
    /// Create a server with the given config and initial zone.
    pub fn new(config: DnsConfig, zone: DnsZone) -> Self {
        Self {
            config,
            zone: Arc::new(RwLock::new(zone)),
        }
    }

    /// A clonable handle to the zone for live updates from a reconcile loop.
    pub fn records(&self) -> Arc<RwLock<DnsZone>> {
        Arc::clone(&self.zone)
    }

    /// Replace the entire zone (e.g. after a planner tick).
    pub async fn update_zone(&self, zone: DnsZone) {
        *self.zone.write().await = zone;
    }

    /// Run the UDP serve loop forever. Each datagram is answered from the zone
    /// or forwarded upstream.
    pub async fn serve(&self) -> Result<()> {
        let sock = UdpSocket::bind(self.config.listen).await?;
        debug!(addr = %self.config.listen, domain = %self.config.domain, "dns server listening");
        let mut buf = [0u8; 1500];
        loop {
            let (n, peer) = match sock.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "dns recv failed");
                    continue;
                }
            };
            let resp = self.handle_packet(&buf[..n]).await;
            if let Some(bytes) = resp
                && let Err(e) = sock.send_to(&bytes, peer).await {
                    warn!(error = %e, peer = %peer, "dns send failed");
                }
        }
    }

    /// Resolve a single packet to a response (or `None` to drop it).
    async fn handle_packet(&self, packet: &[u8]) -> Option<Vec<u8>> {
        let q = match parse_query(packet) {
            Ok(q) => q,
            Err(e) => {
                debug!(error = %e, "dropping malformed dns query");
                return None;
            }
        };
        if q.qtype == TYPE_A && q.qclass == CLASS_IN {
            let ip = self.zone.read().await.lookup(&q.name);
            if let Some(ip) = ip {
                return Some(build_response(&q, Some(ip), 30));
            }
            // Authoritative for the cluster domain → NXDOMAIN, else forward.
            if normalize(&q.name).ends_with(&normalize(&self.config.domain)) {
                return Some(build_response(&q, None, 30));
            }
        }
        match self.config.upstream {
            Some(up) => forward(packet, up).await.ok(),
            None => Some(build_response(&q, None, 30)),
        }
    }
}

/// Forward a raw query to an upstream resolver and return its raw reply.
async fn forward(packet: &[u8], upstream: SocketAddr) -> Result<Vec<u8>> {
    let bind: SocketAddr = if upstream.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let sock = UdpSocket::bind(bind).await?;
    sock.send_to(packet, upstream).await?;
    let mut buf = vec![0u8; 1500];
    let n = tokio::time::timeout(std::time::Duration::from_secs(3), sock.recv(&mut buf)).await??;
    buf.truncate(n);
    Ok(buf)
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal A-query packet for `name`.
    fn query_packet(id: u16, name: &str) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&id.to_be_bytes());
        p.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: standard query, RD
        p.extend_from_slice(&1u16.to_be_bytes()); // qdcount
        p.extend_from_slice(&0u16.to_be_bytes()); // ancount
        p.extend_from_slice(&0u16.to_be_bytes()); // nscount
        p.extend_from_slice(&0u16.to_be_bytes()); // arcount
        for label in name.split('.') {
            p.push(label.len() as u8);
            p.extend_from_slice(label.as_bytes());
        }
        p.push(0); // root
        p.extend_from_slice(&TYPE_A.to_be_bytes());
        p.extend_from_slice(&CLASS_IN.to_be_bytes());
        p
    }

    #[test]
    fn zone_lookup_is_case_insensitive() {
        let mut m = HashMap::new();
        m.insert("web.default.svc.cluster.local".to_string(), Ipv4Addr::new(10, 96, 0, 10));
        let z = DnsZone::from_records(&m);
        assert_eq!(
            z.lookup("WEB.default.SVC.cluster.local."),
            Some(Ipv4Addr::new(10, 96, 0, 10))
        );
        assert_eq!(z.lookup("missing.svc"), None);
    }

    #[test]
    fn parse_roundtrips_name() {
        let pkt = query_packet(0x1234, "web.default.svc.cluster.local");
        let q = parse_query(&pkt).unwrap();
        assert_eq!(q.id, 0x1234);
        assert_eq!(q.name, "web.default.svc.cluster.local");
        assert_eq!(q.qtype, TYPE_A);
        assert_eq!(q.qclass, CLASS_IN);
    }

    #[test]
    fn parse_rejects_short_packet() {
        assert!(parse_query(&[0u8; 4]).is_err());
    }

    #[test]
    fn build_answer_has_one_record() {
        let pkt = query_packet(0xABCD, "web.svc");
        let q = parse_query(&pkt).unwrap();
        let resp = build_response(&q, Some(Ipv4Addr::new(10, 0, 0, 7)), 30);
        // id echoed
        assert_eq!(&resp[0..2], &0xABCDu16.to_be_bytes());
        // response + AA bit set
        let flags = u16::from_be_bytes([resp[2], resp[3]]);
        assert_ne!(flags & FLAG_RESPONSE, 0);
        // ancount == 1
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1);
        // last 4 bytes are the A record rdata
        assert_eq!(&resp[resp.len() - 4..], &[10, 0, 0, 7]);
    }

    #[test]
    fn build_nxdomain_when_no_answer() {
        let pkt = query_packet(1, "nope.svc");
        let q = parse_query(&pkt).unwrap();
        let resp = build_response(&q, None, 30);
        let flags = u16::from_be_bytes([resp[2], resp[3]]);
        assert_eq!(flags & 0x000F, RCODE_NXDOMAIN);
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 0);
    }
}
