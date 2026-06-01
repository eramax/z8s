mod api;
mod components;
mod config;
mod cri;
mod init;
mod manifest;
mod netmux;
mod node;
mod scheduler;
mod storage;
mod store;
mod types;

use anyhow::Result;

fn lock_file_path(port: u16) -> String {
    format!("/tmp/z8s-{}.lock", port)
}

fn acquire_port_lock(port: u16) -> Result<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    let path = lock_file_path(port);
    let file = std::fs::File::create(&path)
        .map_err(|e| anyhow::anyhow!("Failed to create lock file {path}: {e}"))?;
    let ret = unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    if ret != 0 {
        anyhow::bail!("z8s is already running on port {port} (lock held). If stale, remove {path} and retry.");
    }
    Ok(file)
}

fn find_z8s_pid() -> Result<i32> {
    for entry in std::fs::read_dir("/proc").map_err(|_| anyhow::anyhow!("Cannot access /proc"))? {
        let entry = match entry { Ok(e) => e, Err(_) => continue };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.chars().all(|c| c.is_ascii_digit()) { continue; }
        if let Ok(pid) = name_str.parse::<i32>() {
            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
                if comm.trim() == "z8s" { return Ok(pid); }
            }
        }
    }
    anyhow::bail!("No z8s process found");
}

/// Spawn a node process in the background (stdout/stderr to log file).
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str());

    // ── Detect if we're already a node process (spawned by runner) ─────
    if cmd == Some("run") {
        crate::config::init();
        let cfg = crate::config::get();
        let port = cfg.api_port;
        let lock = acquire_port_lock(port)?;
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(crate::node::run_node(port, lock))?;
        return Ok(());
    }

    // ── Join mode needs tokio ─────────────────────────────────
    if cmd == Some("join") {
        crate::config::init();
        let cfg = crate::config::get();
        let url = args.get(2).expect("Usage: z8s join <ws-url> [--token <token>]");
        let token = args.iter().position(|a| a == "--token")
            .and_then(|i| args.get(i + 1)).map(|s| s.clone());
        let token = token.or_else(|| cfg.join_token.clone());
        let url = url.to_string();
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(join_cluster(&url, token));
        return Ok(());
    }

    // ── Runner mode: synchronous CLI, no tokio needed ────────
    if cmd == Some("restart") {
        return restart_z8s(&args);
    }

    if cmd == Some("node") && args.get(2).map(|s| s.as_str()) == Some("start") {
        return node_start(&args, 6443);
    }

    // Default: spawn a node and exit
    default_start(&args)
}

// ── Helper functions ──────────────────────────────────────────────

fn parse_port(args: &[String], default: u16) -> u16 {
    args.iter().position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn parse_opt_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag)
        .and_then(|i| args.get(i + 1)).cloned()
}

fn restart_z8s(args: &[String]) -> Result<()> {
    let port = parse_port(args, 6443);
    let lock_path = lock_file_path(port);
    let pid = match std::fs::read_to_string(&lock_path).ok()
        .and_then(|s| s.trim().parse().ok())
    {
        Some(pid) if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok() => pid,
        _ => find_z8s_pid()?,
    };
    eprintln!("Sending SIGTERM to z8s (PID {pid})...");
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM);
    for _ in 0..50 {
        if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    let _lock = acquire_port_lock(port)?;
    let child = spawn_node_inner(&["run", "--port", &port.to_string()])?;
    eprintln!("z8s restarted (PID {}).", child.id());
    Ok(())
}

fn node_start(args: &[String], main_port: u16) -> Result<()> {
    let node_port = parse_port(args, 7443);
    if std::net::TcpListener::bind(format!("0.0.0.0:{node_port}")).is_err() {
        anyhow::bail!("Port {node_port} is already in use");
    }
    let peer_host = parse_opt_arg(args, "--peer-addr").unwrap_or_else(|| "127.0.0.1".to_string());
    let self_port = main_port;
    let mut node_args = vec![
        "run".to_string(),
        "--port".to_string(), node_port.to_string(),
        "--node-name".to_string(), format!("node-{}", node_port),
        "--peers".to_string(), format!("main={}:{}", peer_host, self_port),
    ];
    if let Some(cidr) = parse_opt_arg(args, "--service-cidr") {
        node_args.push("--service-cidr".to_string()); node_args.push(cidr);
    }
    if let Some(cidr) = parse_opt_arg(args, "--pod-cidr") {
        node_args.push("--pod-cidr".to_string()); node_args.push(cidr);
    }
    eprintln!("Starting node on port {}...", node_port);
    let refs: Vec<&str> = node_args.iter().map(|s| s.as_str()).collect();
    let child = spawn_node_inner(&refs)?;
    eprintln!("Node started on port {} (PID {}).", node_port, child.id());
    Ok(())
}

fn default_start(args: &[String]) -> Result<()> {
    let port = parse_port(args, 6443);
    if std::net::TcpListener::bind(format!("0.0.0.0:{port}")).is_err() {
        anyhow::bail!("Port {port} is already in use");
    }
    let child = spawn_node_inner(&["run", "--port", &port.to_string()])?;
    eprintln!("z8s started on port {} (PID {}).", port, child.id());
    Ok(())
}

fn spawn_node_inner(args: &[&str]) -> Result<std::process::Child> {
    let self_path = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&self_path);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    Ok(cmd.spawn().map_err(|e| anyhow::anyhow!("Failed to spawn node: {e}"))?)
}

/// Connect to a cluster as a worker via WebSocket.
async fn join_cluster(url: &str, _token: Option<String>) {
    use std::time::Duration;
    use tokio::time::sleep;
    use tracing::warn;

    loop {
        match tokio_tungstenite::connect_async(url).await {
            Ok((ws_stream, _)) => {
                warn!("Connected to cluster at {}", url);
                let (mut _write, mut read) = ws_stream.split();
                use futures_util::StreamExt;
                loop {
                    match read.next().await {
                        Some(Ok(msg)) => {
                            if msg.is_close() { break; }
                        }
                        Some(Err(e)) => { warn!("WebSocket error: {}", e); break; }
                        None => break,
                        _ => {}
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to {}: {}. Retrying in 5s...", url, e);
                sleep(Duration::from_secs(5)).await;
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}
