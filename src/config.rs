//! Runtime configuration parsed from CLI flags or environment variables.
//!
//! Usage:
//!   z8s [OPTIONS]
//!
//! Options:
//!   --port <PORT>             API server port            [default: 6443]
//!   --service-cidr <CIDR>     ClusterIP allocation range  [default: 10.96.0.0/16]
//!   --cluster-domain <DOM>    In-cluster DNS domain       [default: cluster.local]
//!   --dns-port <PORT>         Force DNS port (53 or 5353) [default: auto]
//!   --manifests-dir <PATH>    Directory to watch          [default: /etc/z8s/manifests]
//!   --data-dir <PATH>         Override data directory
//!   --help                    Print this help

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::warn;

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Set to true on the node where the scheduler successfully acquires the
/// cluster-wide lease. Deployment reconciliation only runs on this node.
pub static IS_SCHEDULER_LEADER: AtomicBool = AtomicBool::new(false);

pub fn set_scheduler_leader(leader: bool) {
    IS_SCHEDULER_LEADER.store(leader, Ordering::Relaxed);
}

pub fn is_scheduler_leader() -> bool {
    IS_SCHEDULER_LEADER.load(Ordering::Relaxed)
}

/// How API RBAC is applied when RoleBindings / ClusterRoleBindings exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RbacMode {
    /// Enforce policy (deny unless a rule allows).
    #[default]
    Enforce,
    /// Log-friendly dev mode: bindings exist but all API calls are allowed.
    Permissive,
}

impl RbacMode {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "enforce" | "strict" => Some(Self::Enforce),
            "permissive" | "open" => Some(Self::Permissive),
            _ => None,
        }
    }
}

/// When false, RBAC middleware and apply checks allow all requests.
pub fn rbac_enforced() -> bool {
    get().rbac_mode == RbacMode::Enforce
}

pub struct Config {
    pub api_port: u16,
    pub service_cidr_base: [u8; 4],
    pub service_cidr_prefix: u8,
    pub cluster_domain: String,
    pub dns_port: Option<u16>,
    pub manifests_dir: String,
    pub data_dir: Option<String>,
    pub db_path: Option<String>,
    pub pod_cidr: String,
    pub vnet_cidr_size: u8,
    pub node_name: String,
    pub node_ip: String,
    pub peers: Vec<(String, String)>,
    pub join_token: Option<String>,
    pub rbac_mode: RbacMode,
    /// Use OverlayFS (lower=image cache) for container rootfs when possible.
    pub overlay_rootfs: bool,
    /// Max concurrent pod starts on this node (scheduler SyncPod).
    pub pod_start_parallelism: usize,
    /// TLS certificate path (PEM). If set, server listens on HTTPS.
    pub tls_cert: Option<String>,
    /// TLS key path (PEM).
    pub tls_key: Option<String>,
}

impl Config {
    /// Allocate the next ClusterIP within the service CIDR.
    /// Uses a global counter that advances on each call.
    pub fn alloc_cluster_ip(&self) -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(1);
        // Calculate the usable host range from the CIDR
        let host_bits = 32u32.saturating_sub(self.service_cidr_prefix as u32);
        let max_hosts = (1u32 << host_bits).saturating_sub(2); // exclude network & broadcast
        let idx = COUNTER.fetch_add(1, Ordering::Relaxed) % max_hosts.max(1);
        let base = u32::from_be_bytes(self.service_cidr_base);
        let ip = base | idx;
        let bytes = ip.to_be_bytes();
        format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
    }
}

/// Access the global config (initialised once via `init`).
pub fn get() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// Parse CLI args and initialise the global config. Must be called once before `get()`.
pub fn init() {
    let cfg = Config::from_args();
    if CONFIG.set(cfg).is_err() {
        warn!("Config already initialised");
    }
}

impl Config {
    fn default() -> Self {
        Self {
            api_port: 6443,
            service_cidr_base: [10, 96, 0, 0],
            service_cidr_prefix: 16,
            cluster_domain: "cluster.local".to_string(),
            dns_port: None,
            manifests_dir: "/etc/z8s/manifests".to_string(),
            data_dir: Some("/var/lib/z8s".to_string()),
            db_path: None,
            pod_cidr: "10.100.0.0/16".to_string(),
            vnet_cidr_size: 20,
            node_name: hostname(),
            node_ip: auto_detect_node_ip().unwrap_or_else(|| "127.0.0.1".to_string()),
            peers: Vec::new(),
            join_token: None,
            rbac_mode: RbacMode::Enforce,
            overlay_rootfs: false,
            pod_start_parallelism: 10,
            tls_cert: None,
            tls_key: None,
        }
    }

    fn from_args() -> Self {
        let mut cfg = Self::default();
        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "run" | "restart" | "node" | "--daemon" => {} // handled by main
                "--help" | "-h" => {
                    eprintln!("{}", HELP);
                    std::process::exit(0);
                }
                "--port" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.api_port = v.parse().unwrap_or_else(|_| {
                            eprintln!("Invalid --port value: {}", v);
                            std::process::exit(1);
                        });
                    }
                }
                "--service-cidr" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        if let Some((ip, prefix)) = parse_cidr(v) {
                            cfg.service_cidr_base = ip;
                            cfg.service_cidr_prefix = prefix;
                        } else {
                            eprintln!(
                                "Invalid --service-cidr (expected e.g. 127.96.0.0/16): {}",
                                v
                            );
                            std::process::exit(1);
                        }
                    }
                }
                "--cluster-domain" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.cluster_domain = v.trim_start_matches('.').to_string();
                    }
                }
                "--dns-port" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.dns_port = Some(v.parse().unwrap_or_else(|_| {
                            eprintln!("Invalid --dns-port value: {}", v);
                            std::process::exit(1);
                        }));
                    }
                }
                "--manifests-dir" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.manifests_dir = v.to_string();
                    }
                }
                "--data-dir" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.data_dir = Some(v.to_string());
                    }
                }
                "--db-path" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.db_path = Some(v.to_string());
                    }
                }
                "--pod-cidr" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        if parse_cidr(v).is_some() {
                            cfg.pod_cidr = v.to_string();
                        } else {
                            eprintln!("Invalid --pod-cidr (expected e.g. 10.42.0.0/16): {}", v);
                            std::process::exit(1);
                        }
                    }
                }
                "--node-name" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.node_name = v.to_string();
                    }
                }
                "--node-ip" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.node_ip = v.to_string();
                    }
                }
                "--peers" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        for pair in v.split(',') {
                            if let Some((name, ip)) = pair.split_once('=') {
                                cfg.peers.push((name.to_string(), ip.to_string()));
                            }
                        }
                    }
                }
                "--join-token" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.join_token = Some(v.to_string());
                    }
                }
                "--rbac-mode" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.rbac_mode = RbacMode::parse(v).unwrap_or_else(|| {
                            eprintln!(
                                "Invalid --rbac-mode (use enforce or permissive): {}",
                                v
                            );
                            std::process::exit(1);
                        });
                    }
                }
                "--overlay-rootfs" => {
                    cfg.overlay_rootfs = true;
                }
                "--pod-start-parallelism" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.pod_start_parallelism = v.parse().unwrap_or_else(|_| {
                            eprintln!("Invalid --pod-start-parallelism: {}", v);
                            std::process::exit(1);
                        });
                    }
                }
                "--vnet-cidr-size" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.vnet_cidr_size = v.parse().unwrap_or_else(|_| {
                            eprintln!("Invalid --vnet-cidr-size value: {}", v);
                            std::process::exit(1);
                        });
                    }
                }
                "--tls-cert" => {
                    i += 1;
                    cfg.tls_cert = args.get(i).cloned();
                }
                "--tls-key" => {
                    i += 1;
                    cfg.tls_key = args.get(i).cloned();
                }
                other => {
                    eprintln!("Unknown argument: {}", other);
                    eprintln!("{}", HELP);
                    std::process::exit(1);
                }
            }
            i += 1;
        }
        cfg
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "node-0".to_string())
}

fn auto_detect_node_ip() -> Option<String> {
    // Parse /proc/net/fib_trie for the first non-loopback local address
    let content = std::fs::read_to_string("/proc/self/net/fib_trie").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("Local:") {
            if let Some(ip_str) = rest.split('/').next() {
                if let Ok(ip) = ip_str.trim().parse::<std::net::Ipv4Addr>() {
                    if !ip.is_loopback() && !ip.is_link_local() && !ip.is_multicast() {
                        return Some(ip.to_string());
                    }
                }
            }
        }
    }
    None
}

fn parse_cidr(s: &str) -> Option<([u8; 4], u8)> {
    let (ip_str, prefix_str) = s.split_once('/')?;
    let prefix: u8 = prefix_str.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let parts: Vec<u8> = ip_str
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 4 {
        return None;
    }
    Some(([parts[0], parts[1], parts[2], parts[3]], prefix))
}

static DNS_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
static DNS_SERVER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_dns_port(port: u16) {
    DNS_PORT.set(port).ok();
}

pub fn dns_port() -> Option<u16> {
    DNS_PORT.get().copied()
}

pub fn set_dns_server(ip: String) {
    DNS_SERVER.set(ip).ok();
}

pub fn dns_server() -> Option<&'static str> {
    DNS_SERVER.get().map(|s| s.as_str())
}

static TLS_CERT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static TLS_KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_tls_paths(cert: Option<String>, key: Option<String>) {
    if let Some(c) = cert { TLS_CERT.set(c).ok(); }
    if let Some(k) = key { TLS_KEY.set(k).ok(); }
}

pub fn tls_cert_path() -> Option<&'static str> {
    TLS_CERT.get().map(|s| s.as_str())
}

pub fn tls_key_path() -> Option<&'static str> {
    TLS_KEY.get().map(|s| s.as_str())
}

/// Generate a random hex ID (8 hex chars) — replaces uuid::Uuid::new_v4()
pub fn random_id() -> String {
    let mut buf = [0u8; 8];
    getrandom::getrandom(&mut buf).expect("getrandom failed");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Current time as RFC3339 string — replaces chrono::Utc::now().to_rfc3339()
pub fn now_rfc3339() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    let mut y = 1970u32;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year as u64 { break; }
        remaining -= days_in_year as u64;
        y += 1;
    }
    let leap = is_leap(y);
    let md = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 1u32;
    for &d in &md {
        if remaining < d as u64 { break; }
        remaining -= d as u64;
        m += 1;
    }
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z", y, m, remaining + 1, hours, minutes, seconds, nanos / 1000)
}

fn is_leap(y: u32) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }

/// Parse RFC3339 timestamp and return seconds since epoch
pub fn parse_rfc3339_secs(ts: &str) -> Option<i64> {
    let ts = ts.trim_end_matches('Z');
    let (date, time) = ts.split_once('T')?;
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 { return None; }
    let y = parts[0].parse::<u32>().ok()?;
    let m = parts[1].parse::<u32>().ok()?;
    let d = parts[2].parse::<u32>().ok()?;
    let time_part = time.split('.').next().unwrap_or(time);
    let tp: Vec<&str> = time_part.split(':').collect();
    if tp.len() != 3 { return None; }
    let h = tp[0].parse::<u32>().ok()?;
    let min = tp[1].parse::<u32>().ok()?;
    let s = tp[2].parse::<u32>().ok()?;
    let mut total_days: u64 = 0;
    for year in 1970..y { total_days += if is_leap(year) { 366 } else { 365 }; }
    let md = [31, if is_leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for i in 0..(m - 1) as usize { total_days += md[i] as u64; }
    total_days += (d - 1) as u64;
    Some((total_days * 86400 + h as u64 * 3600 + min as u64 * 60 + s as u64) as i64)
}

/// Format seconds-since-epoch as human-readable age
pub fn age_from_epoch_secs(secs: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let diff = (now - secs).max(0) as u64;
    if diff < 60 { format!("{}s", diff) } else if diff < 3600 { format!("{}m", diff / 60) } else if diff < 86400 { format!("{}h", diff / 3600) } else { format!("{}d", diff / 86400) }
}

const HELP: &str = "\
z8s — minimal Kubernetes-compatible container orchestrator

USAGE:
    z8s [OPTIONS]
    z8s join <ws-url> [--token <token>]   (join a cluster as a worker)
    z8s restart                           (restart a running instance)
    z8s node start --port <PORT> [--peer-addr <IP>] [--service-cidr <CIDR>] [--pod-cidr <CIDR>]   (start a new cluster node)

OPTIONS:
    --port <PORT>             API server listen port        [default: 6443]
    --service-cidr <CIDR>     ClusterIP allocation CIDR     [default: 10.96.0.0/16]
    --pod-cidr <CIDR>         Pod IP allocation CIDR        [default: 10.42.0.0/16]
    --vnet-cidr-size <PREFIX> Default VNet CIDR size         [default: 20]
    --cluster-domain <DOMAIN> In-cluster DNS search domain  [default: cluster.local]
    --dns-port <PORT>         Force DNS listen port         [default: auto: try 53, then 5353]
    --manifests-dir <PATH>    Manifests directory to watch  [default: /etc/z8s/manifests]
    --data-dir <PATH>         Override data directory       [default: /var/lib/z8s or ~/.local/share/z8s]
    --db-path <PATH>          Exact path to database file   [default: <data-dir>/z8s.redb]
    --peers <NAME=IP,...>     Other server nodes for gossip  [default: none]
    --join-token <TOKEN>      Token for worker node auth    [default: none]
    --rbac-mode <MODE>        RBAC enforcement: enforce|permissive [default: enforce]
    --overlay-rootfs          Use OverlayFS for container rootfs (fallback: copy)
    --pod-start-parallelism N Concurrent pod starts per node [default: 10]
    --help                    Show this help

EXAMPLES:
    # Single node — listens on :6443
    z8s

    # Custom port
    z8s --port 7443

    # Multi-node cluster: first server
    z8s --port 6443 --peers node-b=10.0.0.2:6443

    # Multi-node cluster: second server
    z8s --port 6443 --peers node-a=10.0.0.1:6443

    # Run two servers on the same machine (different ports)
    z8s --port 7443 --peers node-b=127.0.0.1:8443 &
    z8s --port 8443 --peers node-a=127.0.0.1:7443

    # Join as a worker
    z8s join ws://10.0.0.1:6443/ws/db --token mytoken
";
