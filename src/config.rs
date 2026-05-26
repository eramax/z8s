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
    /// Base address of the service CIDR (e.g. [127, 96, 0, 0]).
    pub service_cidr_base: [u8; 4],
    /// Prefix length of the service CIDR (e.g. 16 for /16).
    pub service_cidr_prefix: u8,
    pub cluster_domain: String,
    /// Explicit DNS port override; None = auto-detect (try 53 then 5353).
    pub dns_port: Option<u16>,
    pub manifests_dir: String,
    /// Override data directory (images, rootfs). None = use default per-uid path.
    pub data_dir: Option<String>,
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
            data_dir: None,
        }
    }

    fn from_args() -> Self {
        let mut cfg = Self::default();
        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
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
                            eprintln!("Invalid --service-cidr (expected e.g. 127.96.0.0/16): {}", v);
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

fn parse_cidr(s: &str) -> Option<([u8; 4], u8)> {
    let (ip_str, prefix_str) = s.split_once('/')?;
    let prefix: u8 = prefix_str.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let parts: Vec<u8> = ip_str.split('.').map(|p| p.parse().ok()).collect::<Option<Vec<_>>>()?;
    if parts.len() != 4 {
        return None;
    }
    Some(([parts[0], parts[1], parts[2], parts[3]], prefix))
}

const HELP: &str = "\
z8s — minimal Kubernetes-compatible container orchestrator

USAGE:
    z8s [OPTIONS]

OPTIONS:
    --port <PORT>             API server listen port        [default: 6443]
    --service-cidr <CIDR>     ClusterIP allocation CIDR     [default: 10.96.0.0/16]
    --cluster-domain <DOMAIN> In-cluster DNS search domain  [default: cluster.local]
    --dns-port <PORT>         Force DNS listen port         [default: auto: try 53, then 5353]
    --manifests-dir <PATH>    Manifests directory to watch  [default: /etc/z8s/manifests]
    --data-dir <PATH>         Override data directory       [default: /var/lib/z8s or ~/.local/share/z8s]
    --help                    Show this help

EXAMPLES:
    # Default — listens on :6443, ClusterIPs in 10.96.0.0/16
    z8s

    # Custom port and CIDR
    z8s --port 8443 --service-cidr 10.96.0.0/12

    # Watch a custom manifests directory
    z8s --manifests-dir /home/user/k8s-manifests
";
