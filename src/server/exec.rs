use crate::server::api::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::http::StatusCode;
use axum::http::request::Parts;
use futures::{SinkExt, StreamExt};
use nix::fcntl::OFlag;
use nix::mount::MsFlags;
use nix::pty;
use nix::sys::termios::{self, SetArg, InputFlags, OutputFlags, LocalFlags};
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;

#[repr(C)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

const TIOCSWINSZ: u64 = 0x5414;

fn set_winsize(fd: RawFd, cols: u16, rows: u16) {
    let ws = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        let r = nix::libc::ioctl(fd, TIOCSWINSZ as std::os::raw::c_ulong, &ws as *const Winsize);
        std::mem::drop(r);
    }
}
use tokio::process::Command;
use tracing::info;

pub struct ExecParams {
    pub command: Vec<String>,
    pub container: Option<String>,
}

impl<S> axum::extract::FromRequestParts<S> for ExecParams
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let uri = parts.uri.clone();
        let query = uri.query().unwrap_or("");
        let params: HashMap<String, Vec<String>> = parse_query_params(query);

        let command = params.get("command")
            .or_else(|| params.get("command[]"))
            .cloned()
            .unwrap_or_default();

        let container = params.get("container")
            .and_then(|v| v.first())
            .cloned();

        Ok(ExecParams { command, container })
    }
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '+' => result.push(' '),
            '%' => {
                let hi = chars.next().and_then(|c| c.to_digit(16)).unwrap_or(0);
                let lo = chars.next().and_then(|c| c.to_digit(16)).unwrap_or(0);
                result.push((hi as u8 * 16 + lo as u8) as char);
            }
            _ => result.push(c),
        }
    }
    result
}

fn parse_query_params(query: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() { continue; }
        let mut parts = pair.splitn(2, '=');
        let key = url_decode(parts.next().unwrap_or(""));
        let value = url_decode(parts.next().unwrap_or(""));
        map.entry(key).or_default().push(value);
    }
    map
}

pub async fn exec_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    params: ExecParams,
) -> impl IntoResponse {
    let cmds = params.command.clone();
    info!(pod = %name, ns = %namespace, cmds = ?cmds, container = ?params.container, "Exec WS");

    let rootfs = resolve_rootfs(&state, &name, params.container.as_deref()).await;
    ws.protocols(["v5.channel.k8s.io", "v4.channel.k8s.io", "v3.channel.k8s.io", "channel.k8s.io"])
        .on_upgrade(move |socket| exec_ws(socket, cmds, rootfs))
}

pub async fn exec_post_handler(
    State(state): State<AppState>,
    Path((_namespace, name)): Path<(String, String)>,
    params: ExecParams,
) -> impl IntoResponse {
    info!(pod = %name, cmds = ?params.command, container = ?params.container, "Exec POST handler reached");

    (StatusCode::METHOD_NOT_ALLOWED, axum::Json(serde_json::json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {},
        "status": "Failure",
        "message": "SPDY exec not implemented; use WebSocket exec via kubectl >= 1.30",
        "reason": "MethodNotAllowed",
        "code": 405
    }))).into_response()
}

async fn resolve_rootfs(state: &AppState, pod_name: &str, container_name: Option<&str>) -> Option<String> {
    let running = state.supervisor.running.lock().await;
    let prefix = format!("{}-", pod_name);
    if let Some(container) = container_name {
        let cid = format!("{}-{}", pod_name, container);
        running.get(&cid).map(|rc| rc.instance.rootfs.clone())
    } else {
        running.iter()
            .find(|(key, _)| key.starts_with(&prefix))
            .map(|(_, rc)| rc.instance.rootfs.clone())
    }
}

fn spawn_with_pty(cmd: &str, args: &[&str], rootfs: Option<&str>) -> Result<(OwnedFd, tokio::process::Child), String> {
    let master_fd = pty::posix_openpt(OFlag::O_RDWR | OFlag::O_NONBLOCK)
        .map_err(|e| format!("posix_openpt: {}", e))?;
    pty::grantpt(&master_fd).map_err(|e| format!("grantpt: {}", e))?;
    pty::unlockpt(&master_fd).map_err(|e| format!("unlockpt: {}", e))?;
    let slave_name = unsafe { pty::ptsname(&master_fd) }.map_err(|e| format!("ptsname: {}", e))?;

    let slave_fd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&slave_name)
        .map_err(|e| format!("open slave {}: {}", slave_name, e))?;

    // Set up the slave terminal for a proper interactive shell
if let Ok(mut t) = termios::tcgetattr(&slave_fd) {
    termios::cfmakeraw(&mut t);
    t.output_flags |= OutputFlags::OPOST | OutputFlags::ONLCR | OutputFlags::ONOCR;
    t.local_flags |= LocalFlags::ECHO | LocalFlags::ECHOE | LocalFlags::ECHOK
        | LocalFlags::ISIG | LocalFlags::ICANON;
    t.local_flags &= !LocalFlags::ECHOCTL;
    t.input_flags |= InputFlags::ICRNL | InputFlags::IXON;
    let _ = termios::tcsetattr(&slave_fd, SetArg::TCSANOW, &t);
}
    set_winsize(slave_fd.as_raw_fd(), 80, 24);

    // Prepare rootfs: mount /proc and ensure /etc/resolv.conf has DNS
    if let Some(rootfs) = rootfs.as_ref() {
        let root = std::path::Path::new(rootfs);
        let _ = std::fs::create_dir_all(root.join("proc"));
        let _ = nix::mount::mount(
            Some("proc"),
            &root.join("proc"),
            Some("proc"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
            None::<&str>,
        );
        let _ = std::fs::create_dir_all(root.join("etc"));
        let resolv = root.join("etc").join("resolv.conf");
        let content = std::fs::read_to_string(&resolv).unwrap_or_default();
        if content.trim().is_empty() || content.contains("127.0.0.53") || content.contains("systemd-resolved") {
            let _ = std::fs::write(&resolv, "nameserver 1.1.1.1\nnameserver 8.8.8.8\n");
        }
    }

    let mut child_cmd = if let Some(rootfs) = rootfs {
        let mut c = Command::new("chroot");
        c.arg(rootfs).arg(cmd);
        for a in args { c.arg(a); }
        c
    } else {
        let mut c = Command::new(cmd);
        for a in args { c.arg(a); }
        c
    };

    unsafe {
        child_cmd
            .stdin(std::process::Stdio::from_raw_fd(slave_fd.try_clone().map_err(|e| format!("clone: {}", e))?.into_raw_fd()))
            .stdout(std::process::Stdio::from_raw_fd(slave_fd.try_clone().map_err(|e| format!("clone: {}", e))?.into_raw_fd()))
            .stderr(std::process::Stdio::from_raw_fd(slave_fd.into_raw_fd()));
    }
    child_cmd.kill_on_drop(true);
    unsafe {
        child_cmd.as_std_mut().pre_exec(move || {
            unsafe {
                nix::libc::setsid();
                nix::libc::ioctl(0, 0x540E, 0);
            }
            Ok(())
        });
    }

    let child = child_cmd.spawn().map_err(|e| format!("spawn: {}", e))?;
    Ok((master_fd.into(), child))
}

async fn exec_ws(mut socket: WebSocket, cmds: Vec<String>, rootfs: Option<String>) {
    if cmds.is_empty() {
        let _ = socket.close().await;
        return;
    }

    let cmd = cmds[0].clone();
    let args: Vec<&str> = cmds.iter().skip(1).map(|s| s.as_str()).collect();

    info!("Exec WS: cmd={}, args={:?}, rootfs={:?}", cmd, args, rootfs);

    // Spawn child with PTY
    let (master_fd, mut child) = match spawn_with_pty(&cmd, &args, rootfs.as_deref()) {
        Ok((fd, c)) => (fd, c),
        Err(e) => {
            let _ = socket.send(Message::Text(format!("error: {}", e).into())).await;
            return;
        }
    };

    let pid = child.id();
    info!("Exec WS: child spawned with PID {:?}", pid);

    let async_master = tokio::io::unix::AsyncFd::new(master_fd)
        .expect("AsyncFd for PTY master");

    let (mut ws_sender, mut ws_receiver) = socket.split();

    let mut read_buf = vec![0u8; 4096];

    loop {
        tokio::select! {
            result = async_master.readable() => {
                let mut guard = match result {
                    Ok(g) => g,
                    _ => break,
                };
                match guard.try_io(|inner| {
                    use std::io::Read;
                    let fd = inner.as_raw_fd();
                    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
                    let ret = f.read(&mut read_buf);
                    std::mem::forget(f); // don't close
                    ret
                }) {
                    Ok(Ok(0)) => break,
                    Ok(Ok(n)) => {
                        let mut frame = vec![1u8];
                        frame.extend_from_slice(&read_buf[..n]);
                        if ws_sender.send(Message::Binary(axum::body::Bytes::from(frame))).await.is_err() {
                            break;
                        }
                    }
                    Ok(Err(ref e)) => {
                        info!("PTY read error: {}", e);
                        break;
                    }
                    Err(_would_block) => {}
                }
            }

            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if data.is_empty() { continue; }
                        let channel = data[0];
                        match channel {
                            0 => {
                                if data.len() > 1 {
                                    let mut guard = async_master.writable().await.unwrap();
                                    let _ = guard.try_io(|inner| {
                                        nix::unistd::write(inner, &data[1..])
                                            .map(|_| 0)
                                            .map_err(std::io::Error::other)
                                    });
                                }
                            }
                            4 => {
                                if data.len() > 1 {
                                    if let Ok(resize) = serde_json::from_slice::<serde_json::Value>(&data[1..]) {
                                        let cols = resize.get("Width").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                                        let rows = resize.get("Height").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                                        set_winsize(async_master.get_ref().as_raw_fd(), cols, rows);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = child.kill().await;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Wait for child exit and send status on channel 3
    let exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(0);
    let (status_str, message) = if exit_code == 0 {
        ("Success", "command exited with code 0".to_string())
    } else {
        ("Failure", format!("command exited with code {}", exit_code))
    };
    let status = serde_json::json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {},
        "status": status_str,
        "message": message,
        "details": { "exitCode": exit_code }
    });
    let data = serde_json::to_string(&status).unwrap_or_default();
    let mut frame = vec![3u8];
    frame.extend_from_slice(data.as_bytes());
    let _ = ws_sender.send(Message::Binary(axum::body::Bytes::from(frame))).await;

    let _ = ws_sender.send(Message::Close(Some(CloseFrame { code: 1000, reason: Default::default() }))).await;
}
