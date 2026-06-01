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
use std::io::Write;
use std::os::unix::io::AsRawFd;

// ── Path helpers ─────────────────────────────────────────────────────

const Z8S_RUN_DIR: &str = "/tmp";

fn global_lock_path() -> String {
    format!("{Z8S_RUN_DIR}/z8s.lock")
}

fn port_lock_path(port: u16) -> String {
    format!("{Z8S_RUN_DIR}/z8s-{}.lock", port)
}

// ── Lock management ─────────────────────────────────────────────────
//
// The running process holds an exclusive flock on the lock file for its
// entire lifetime. The file also stores the PID for external inspection.
// When the process exits, the fd is closed and the flock is released.

/// Try to acquire an exclusive lock on `path`. If the lock is held by a
/// live process, returns Err with that PID. If the lock is stale (process
/// dead), removes the file and retries. On success, writes our PID and
/// returns the open File (caller must keep it alive).
fn acquire_lock(path: &str) -> Result<std::fs::File> {
    loop {
        // Open without truncating — we need to read existing PID before flocking
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open lock file {path}: {e}"))?;

        let ret = unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
        if ret == 0 {
            // Lock acquired — write our PID
            use std::os::unix::io::AsRawFd;
            unsafe { nix::libc::ftruncate(file.as_raw_fd(), 0) };
            use std::io::Seek;
            let mut f = file;
            f.seek(std::io::SeekFrom::Start(0)).ok();
            write!(f, "{}\n", std::process::id()).ok();
            f.flush().ok();
            return Ok(f);
        }

        // Lock held by someone else — check if they're alive
        drop(file);
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse::<i32>() {
                if is_pid_alive(pid) {
                    anyhow::bail!("z8s is already running (PID {pid}).");
                }
                // Stale lock — owner died without cleanup
                eprintln!("Removing stale lock {path} (dead PID {pid})");
                let _ = std::fs::remove_file(path);
                continue;
            }
        }
        anyhow::bail!("Cannot acquire lock at {path}.");
    }
}

/// Read the PID from a lock file without acquiring the lock.
fn read_lock_pid(path: &str) -> Option<i32> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents.trim().parse::<i32>().ok()
}

/// Detect the main z8s port by matching the global lock PID against port lock files.
fn detect_main_port() -> Option<u16> {
    let main_pid = read_lock_pid(&global_lock_path())?;
    if !is_pid_alive(main_pid) {
        return None;
    }
    // Find which port lock has this PID
    let locks = scan_lock_files();
    locks.iter()
        .find(|(_, pid)| *pid == main_pid)
        .map(|(port, _)| *port)
}

/// Scan /tmp for z8s lock files and return (port, pid) pairs for live processes.
fn scan_lock_files() -> Vec<(u16, i32)> {
    let mut result = Vec::new();
    let Ok(entries) = std::fs::read_dir(Z8S_RUN_DIR) else { return result };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Match z8s-<port>.lock
        if let Some(port_str) = name.strip_prefix("z8s-").and_then(|s| s.strip_suffix(".lock")) {
            if let Ok(port) = port_str.parse::<u16>() {
                if let Some(pid) = read_lock_pid(&entry.path().to_string_lossy()) {
                    if is_pid_alive(pid) {
                        result.push((port, pid));
                    }
                }
            }
        }
    }
    result
}

/// Check if a PID is alive (tries direct, then sudo).
/// Also verifies it's not a zombie (state Z) or D-state (uninterruptible).
/// D-state processes are effectively dead — they hold resources but can't
/// process signals or be killed. We treat them as dead for lock purposes
/// so their resources can be reclaimed.
fn is_pid_alive(pid: i32) -> bool {
    // Check for unrecoverable states (Z = zombie, D = uninterruptible sleep)
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
        for line in stat.lines() {
            if line.starts_with("State:") {
                if line.contains("(Z") || line.contains("(D") {
                    return false;
                }
                break;
            }
        }
    }
    if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok() {
        return true;
    }
    // Might be a root process
    std::process::Command::new("sudo")
        .args(["kill", "-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a PID is in D-state (uninterruptible sleep / zombie stuck in kernel).
fn is_pid_dstate(pid: i32) -> bool {
    let path = format!("/proc/{pid}/stat");
    if let Ok(stat) = std::fs::read_to_string(&path) {
        // The state field is after the comm field which is in parens: "name (comm) S ..."
        if let Some(paren_end) = stat.rfind(')') {
            if let Some(state_char) = stat.get(paren_end + 2..paren_end + 3) {
                return state_char == "D";
            }
        }
    }
    false
}

// ── CLI dispatch ─────────────────────────────────────────────────────

fn print_help() {
    if let Ok(path) = std::env::current_exe() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        eprintln!("Usage: {name} [SUBCOMMAND] [OPTIONS]

Commands:
  run          Start the node server (called internally by spawner)
  join <url>   Join a cluster as a worker node
  node start   Start a peer node
  node stop    Stop a peer node
  node list    List all running nodes
  stop         Stop the main z8s instance
  restart      Restart the main z8s instance
  status       Show running z8s processes

Options:
  --port <PORT>  API server listen port     [default: 6443]
  -h, --help     Show this help message");
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str());

    match cmd {
        // ── Help ────────────────────────────────────────────────────────
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }

        // ── Run as the actual server (called by daemon spawner) ────────
        Some("run") => {
            crate::config::init();
            let cfg = crate::config::get();
            let port = cfg.api_port;

            // Acquire per-port lock (holds flock for entire lifetime)
            let port_lock = acquire_lock(&port_lock_path(port))?;

            // Main z8s also acquires the global lock
            let global_lock = if cfg.peers.is_empty() {
                let lock = acquire_lock(&global_lock_path())?;
                Some(lock)
            } else {
                None
            };

            let rt = tokio::runtime::Runtime::new()?;
            let result = rt.block_on(crate::node::run_node(port, port_lock, global_lock));
            result
        }

        // ── Join a cluster as a worker ──────────────────────────────────
        Some("join") => {
            crate::config::init();
            let cfg = crate::config::get();
            let url = args.get(2).expect("Usage: z8s join <ws-url> [--token <token>]");
            let token = args.iter().position(|a| a == "--token")
                .and_then(|i| args.get(i + 1)).map(|s| s.clone());
            let token = token.or_else(|| cfg.join_token.clone());
            let url = url.to_string();
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(join_cluster(&url, token));
            Ok(())
        }

        // ── Restart: stop running instance, then start fresh ────────────
        Some("restart") => restart_z8s(&args),

        // ── Node management ─────────────────────────────────────────────
        Some("node") => match args.get(2).map(|s| s.as_str()) {
            Some("start") => node_start(&args),
            Some("stop") => node_stop(&args),
            Some("list") => node_list(),
            _ => {
                eprintln!("Usage: z8s node {{start|stop|list}} [options]");
                Ok(())
            }
        },

        // ── Stop the main z8s instance ──────────────────────────────────
        Some("stop") => stop_z8s(),

        // ── Status ──────────────────────────────────────────────────────
        Some("status") => show_status(),

        // ── Default: start z8s (daemon mode) ────────────────────────────
        _ => default_start(&args),
    }
}

// ── Helper functions ────────────────────────────────────────────────

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

fn default_start(args: &[String]) -> Result<()> {
    let port = parse_port(args, 6443);

    // Quick port check before spawning
    if std::net::TcpListener::bind(format!("0.0.0.0:{port}")).is_err() {
        anyhow::bail!("Port {port} is already in use");
    }

    let child = spawn_daemon(&["run", "--port", &port.to_string()])?;
    let pid = child.id() as i32;

    // Poll the global lock PID. The child (main) writes its PID to the global
    // lock only AFTER acquiring BOTH the port lock AND the global lock.
    // If the global lock fails, the child exits and we'll detect it.
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let alive = is_pid_alive(pid);
        match read_lock_pid(&global_lock_path()) {
            Some(lock_pid) if lock_pid == pid && alive => {
                eprintln!("z8s started on port {port} (PID {pid}).");
                return Ok(());
            }
            _ => {}
        }
        if !alive {
            break;
        }
    }

    // Child failed or died. Read daemon log for reason.
    let log_path = format!("{Z8S_RUN_DIR}/z8s-daemon.log");
    if let Ok(log) = std::fs::read_to_string(&log_path) {
        for line in log.lines().rev().take(5) {
            if line.contains("Error") || line.contains("error") || line.contains("panicked") {
                anyhow::bail!("z8s failed to start: {line}");
            }
        }
    }
    anyhow::bail!("z8s failed to start on port {port}");
}

fn stop_z8s() -> Result<()> {
    // 1. Find the running main z8s by checking the global lock
    let global_path = global_lock_path();
    if let Some(pid) = read_lock_pid(&global_path) {
        if is_pid_alive(pid) {
            eprintln!("Stopping z8s (PID {pid})...");
            send_shutdown(pid);
            return Ok(());
        }
        // PID in global lock is dead — remove stale lock
        eprintln!("Removing stale global lock (dead PID {pid})");
        let _ = std::fs::remove_file(&global_path);
    }

    // 2. Fallback: scan all port lock files for any running process
    let locks = scan_lock_files();
    for (port, pid) in &locks {
        eprintln!("Stopping z8s on port {port} (PID {pid})...");
        send_shutdown(*pid);
        return Ok(());
    }

    // 3. Fallback: /proc scan
    let pids = find_z8s_pids();
    if let Some(&pid) = pids.first() {
        eprintln!("Stopping z8s (PID {pid})...");
        send_shutdown(pid);
        return Ok(());
    }

    eprintln!("z8s is not running.");
    Ok(())
}

fn send_shutdown(pid: i32) {
    // D-state processes are stuck in kernel — can't be killed by userspace
    if is_pid_dstate(pid) {
        eprintln!("PID {pid} is stuck in D-state (kernel zombie) — cannot be killed. Reboot required.");
        return;
    }

    // Try direct kill first
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    );
    // Fallback to sudo
    let _ = std::process::Command::new("sudo")
        .args(["kill", "-TERM", &pid.to_string()])
        .output();

    // Wait up to 5 seconds
    for _ in 0..50 {
        if !is_pid_alive(pid) {
            if is_pid_dstate(pid) {
                // Process entered D-state during shutdown cleanup
                eprintln!("Process {pid} entered D-state during shutdown (unrecoverable).");
                cleanup_external(pid);
            } else {
                eprintln!("z8s stopped.");
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Still alive after SIGTERM — try force kill
    if is_pid_dstate(pid) {
        eprintln!("Process {pid} entered D-state (unrecoverable).");
        cleanup_external(pid);
        return;
    }

    eprintln!("Force killing z8s (PID {pid})...");
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
    let _ = std::process::Command::new("sudo")
        .args(["kill", "-KILL", &pid.to_string()])
        .output();
    // Wait up to 2 more seconds for SIGKILL
    for _ in 0..20 {
        if !is_pid_alive(pid) {
            eprintln!("z8s stopped.");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if is_pid_dstate(pid) {
        eprintln!("Process {pid} stuck in D-state after SIGKILL (unrecoverable).");
        cleanup_external(pid);
    } else {
        eprintln!("Process {pid} is still alive after SIGKILL.");
    }
}

/// Clean up resources held by an unrecoverable process from outside.
/// Removes stale lock files and z8s nftables tables.
fn cleanup_external(pid: i32) {
    // Remove stale lock files for this PID
    let Ok(entries) = std::fs::read_dir(Z8S_RUN_DIR) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "z8s.lock" || name.starts_with("z8s-") {
            if let Some(lock_pid) = read_lock_pid(&entry.path().to_string_lossy()) {
                if lock_pid == pid {
                    eprintln!("Removing stale lock file: {}", entry.path().display());
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    // Remove z8s nftables tables from outside (the stuck process can't do it)
    eprintln!("Cleaning up nftables from external process...");
    for table in ["z8s_nat", "z8s_filter"] {
        let _ = std::process::Command::new("sudo")
            .args(["nft", "delete", "table", "ip", table])
            .output();
    }
}

fn restart_z8s(args: &[String]) -> Result<()> {
    let port = parse_port(args, 6443);

    // Stop existing on this port
    let path = port_lock_path(port);
    if let Some(pid) = read_lock_pid(&path) {
        if is_pid_alive(pid) {
            eprintln!("Stopping z8s on port {port} (PID {pid})...");
            send_shutdown(pid);
            // send_shutdown already waits for the process to die
        } else {
            // Stale lock — clean up
            eprintln!("Removing stale lock for port {port} (dead PID {pid})");
            let _ = std::fs::remove_file(&path);
        }
    }

    // Also clean global lock if stale
    let global_path = global_lock_path();
    if let Some(pid) = read_lock_pid(&global_path) {
        if !is_pid_alive(pid) {
            eprintln!("Removing stale global lock (dead PID {pid})");
            let _ = std::fs::remove_file(&global_path);
        }
    }

    // Small extra wait for port release
    std::thread::sleep(std::time::Duration::from_millis(500));
    default_start(args)
}

fn show_status() -> Result<()> {
    let global_pid = read_lock_pid(&global_lock_path());
    let locks = scan_lock_files();

    if locks.is_empty() && global_pid.is_none() {
        println!("z8s not running.");
        return Ok(());
    }

    let mut found = false;
    for (port, pid) in &locks {
        let role = if global_pid == Some(*pid) { "main" } else { "node" };
        if is_pid_dstate(*pid) {
            println!("{role}:{port}  PID={pid}  (stuck in D-state, needs reboot)");
        } else {
            println!("{role}:{port}  PID={pid}");
        }
        found = true;
    }

    if !found {
        println!("z8s not running.");
    }
    Ok(())
}

fn node_start(args: &[String]) -> Result<()> {
    let node_port = parse_port(args, 7443);

    // Check port availability
    if std::net::TcpListener::bind(format!("0.0.0.0:{node_port}")).is_err() {
        anyhow::bail!("Port {node_port} is already in use");
    }

    // Auto-detect main port from global lock, or from --main-port arg
    let main_port: u16 = if let Some(p) = parse_opt_arg(args, "--main-port") {
        p.parse().unwrap_or(6443)
    } else {
        detect_main_port().unwrap_or(6443)
    };

    let peer_host = parse_opt_arg(args, "--peer-addr").unwrap_or_else(|| "127.0.0.1".to_string());
    let mut node_args = vec![
        "run".to_string(),
        "--port".to_string(), node_port.to_string(),
        "--node-name".to_string(), format!("node-{}", node_port),
        "--peers".to_string(), format!("main={}:{}", peer_host, main_port),
    ];
    if let Some(cidr) = parse_opt_arg(args, "--service-cidr") {
        node_args.push("--service-cidr".to_string());
        node_args.push(cidr);
    }
    if let Some(cidr) = parse_opt_arg(args, "--pod-cidr") {
        node_args.push("--pod-cidr".to_string());
        node_args.push(cidr);
    }

    eprintln!("Starting node on port {node_port}...");
    let refs: Vec<&str> = node_args.iter().map(|s| s.as_str()).collect();
    let child = spawn_daemon(&refs)?;
    let pid = child.id() as i32;

    // Poll for node startup (port lock PID matches child PID)
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if read_lock_pid(&port_lock_path(node_port)) == Some(pid) && is_pid_alive(pid) {
            eprintln!("Node started on port {node_port} (PID {pid}).");
            return Ok(());
        }
        if !is_pid_alive(pid) {
            break;
        }
    }

    // Check daemon log for reason
    let log_path = format!("{Z8S_RUN_DIR}/z8s-daemon.log");
    if let Ok(log) = std::fs::read_to_string(&log_path) {
        for line in log.lines().rev().take(5) {
            if line.contains("Error") || line.contains("error") || line.contains("panicked") {
                anyhow::bail!("z8s failed to start: {line}");
            }
        }
    }
    anyhow::bail!("z8s node failed to start on port {node_port}");
}

fn node_stop(args: &[String]) -> Result<()> {
    let node_port = parse_port(args, 7443);
    let path = port_lock_path(node_port);

    if let Some(pid) = read_lock_pid(&path) {
        if is_pid_alive(pid) {
            eprintln!("Stopping node on port {node_port} (PID {pid})...");
            send_shutdown(pid);
            // Don't remove lock file — flock releases on process exit
            return Ok(());
        }
    }

    eprintln!("No node running on port {node_port}.");
    Ok(())
}

fn node_list() -> Result<()> {
    let global_pid = read_lock_pid(&global_lock_path());
    let locks = scan_lock_files();

    let mut found = false;
    for (port, pid) in &locks {
        let role = if global_pid == Some(*pid) { "main" } else { "node" };
        println!("{role}:{port}  PID={pid}");
        found = true;
    }

    if !found {
        println!("No z8s processes running.");
    }
    Ok(())
}

fn find_z8s_pids() -> Vec<i32> {
    let mut pids = Vec::new();
    let self_pid = std::process::id() as i32;
    let Ok(entries) = std::fs::read_dir("/proc") else { return pids };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.chars().all(|c| c.is_ascii_digit()) { continue; }
        if let Ok(pid) = name_str.parse::<i32>() {
            if pid == self_pid { continue; }
            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
                if comm.trim() == "z8s" {
                    pids.push(pid);
                }
            }
        }
    }
    pids
}

/// Spawn a z8s process in the background (detached, no stdio).
fn spawn_daemon(args: &[&str]) -> Result<std::process::Child> {
    let self_path = std::env::current_exe()?;
    let log_path = format!("{Z8S_RUN_DIR}/z8s-daemon.log");
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| anyhow::anyhow!("Cannot create log file {log_path}: {e}"))?;
    let log_file_err = log_file.try_clone()
        .map_err(|e| anyhow::anyhow!("Cannot clone log file handle: {e}"))?;
    let mut cmd = std::process::Command::new(&self_path);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_file_err));
    cmd.spawn().map_err(|e| anyhow::anyhow!("Failed to spawn z8s: {e}"))
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
