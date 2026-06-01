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
use tracing::warn;

static CONFIG: OnceLock<Config> = OnceLock::new();

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
                "--vnet-cidr-size" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.vnet_cidr_size = v.parse().unwrap_or_else(|_| {
                            eprintln!("Invalid --vnet-cidr-size value: {}", v);
                            std::process::exit(1);
                        });
                    }
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
