use crate::container::rootfs;
use crate::server::api::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use nix::fcntl::OFlag;
use nix::pty;
use nix::sched::CloneFlags;
use nix::sys::termios::{self, InputFlags, LocalFlags, OutputFlags, SetArg};
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use tokio::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::info;

fn set_winsize(fd: RawFd, cols: u16, rows: u16) {
    let ws = nix::libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ ioctl on a valid PTY fd is always safe.
    unsafe { nix::libc::ioctl(fd, nix::libc::TIOCSWINSZ, &ws) };
}

pub struct ExecParams {
    pub command: Vec<String>,
    pub container: Option<String>,
    pub tty: bool,
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
}

impl<S> axum::extract::FromRequestParts<S> for ExecParams
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let query = parts.uri.query().unwrap_or("");
        let params = parse_query_params(query);
        let command = params
            .get("command")
            .or_else(|| params.get("command[]"))
            .cloned()
            .unwrap_or_default();
        let container = params.get("container").and_then(|v| v.first()).cloned();
        let tty = params
            .get("tty")
            .and_then(|v| v.first())
            .map(|v| v == "true")
            .unwrap_or(false);
        let stdin = params
            .get("stdin")
            .and_then(|v| v.first())
            .map(|v| v == "true")
            .unwrap_or(false);
        let stdout = params
            .get("stdout")
            .and_then(|v| v.first())
            .map(|v| v == "true")
            .unwrap_or(true);
        let stderr = params
            .get("stderr")
            .and_then(|v| v.first())
            .map(|v| v == "true")
            .unwrap_or(true);
        Ok(ExecParams {
            command,
            container,
            tty,
            stdin,
            stdout,
            stderr,
        })
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
        if pair.is_empty() {
            continue;
        }
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
    let tty = params.tty;
    let stdin_flag = params.stdin;
    let stdout_flag = params.stdout;
    let stderr_flag = params.stderr;
    info!(pod = %name, ns = %namespace, cmds = ?cmds, container = ?params.container, tty, "Exec WS");
    let rootfs_pid = resolve_rootfs(&state, &name, params.container.as_deref()).await;
    ws.protocols(["v5.channel.k8s.io", "v4.channel.k8s.io", "v3.channel.k8s.io", "channel.k8s.io"])
        .on_upgrade(move |socket| exec_ws(socket, cmds, rootfs_pid, tty, stdin_flag, stdout_flag, stderr_flag))
}

pub async fn exec_post_handler(
    State(_state): State<AppState>,
    Path((_namespace, name)): Path<(String, String)>,
    params: ExecParams,
) -> impl IntoResponse {
    info!(pod = %name, cmds = ?params.command, container = ?params.container, "Exec POST");
    (
        StatusCode::METHOD_NOT_ALLOWED,
        axum::Json(serde_json::json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {},
            "status": "Failure",
            "message": "SPDY exec not supported; use WebSocket (kubectl >= 1.30)",
            "reason": "MethodNotAllowed",
            "code": 405
        })),
    )
        .into_response()
}

async fn resolve_rootfs(
    state: &AppState,
    pod_name: &str,
    container_name: Option<&str>,
) -> Option<(String, u32)> {
    let running = state.supervisor.running.lock().await;
    let prefix = format!("{}-", pod_name);
    if let Some(container) = container_name {
        running
            .get(&format!("{}-{}", pod_name, container))
            .and_then(|rc| {
                let rootfs = rc.instance.rootfs.clone();
                let pid = rc.instance.pid?;
                (!rootfs.is_empty()).then_some((rootfs, pid))
            })
    } else {
        running
            .iter()
            .find(|(k, _)| k.starts_with(&prefix))
            .and_then(|(_, rc)| {
                let rootfs = rc.instance.rootfs.clone();
                let pid = rc.instance.pid?;
                (!rootfs.is_empty()).then_some((rootfs, pid))
            })
    }
}

fn spawn_with_pty(
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
) -> Result<(pty::PtyMaster, Command), String> {
    let master = pty::posix_openpt(OFlag::O_RDWR | OFlag::O_NONBLOCK)
        .map_err(|e| format!("posix_openpt: {}", e))?;
    pty::grantpt(&master).map_err(|e| format!("grantpt: {}", e))?;
    pty::unlockpt(&master).map_err(|e| format!("unlockpt: {}", e))?;
    let slave_name =
        unsafe { pty::ptsname(&master) }.map_err(|e| format!("ptsname: {}", e))?;

    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&slave_name)
        .map_err(|e| format!("open slave {}: {}", slave_name, e))?;

    if let Ok(mut t) = termios::tcgetattr(&slave) {
        termios::cfmakeraw(&mut t);
        t.output_flags |= OutputFlags::OPOST | OutputFlags::ONLCR | OutputFlags::ONOCR;
        t.local_flags |= LocalFlags::ECHO
            | LocalFlags::ECHOE
            | LocalFlags::ECHOK
            | LocalFlags::ISIG
            | LocalFlags::ICANON;
        t.local_flags &= !LocalFlags::ECHOCTL;
        t.input_flags |= InputFlags::ICRNL | InputFlags::IXON;
        let _ = termios::tcsetattr(&slave, SetArg::TCSANOW, &t);
    }
    set_winsize(slave.as_raw_fd(), 80, 24);

    if let Some((root, _pid)) = rootfs_pid {
        let _ = rootfs::setup_exec_mounts(root);
    }

    let mut child_cmd = build_command(cmd, args, rootfs_pid);

    child_cmd
        .stdin(Stdio::from(
            slave.try_clone().map_err(|e| format!("clone stdin: {}", e))?,
        ))
        .stdout(Stdio::from(
            slave.try_clone().map_err(|e| format!("clone stdout: {}", e))?,
        ))
        .stderr(Stdio::from(slave));
    child_cmd.kill_on_drop(true);

    let ns_fds = rootfs_pid.and_then(|(_, pid)| try_open_namespace_fds(pid));

    unsafe {
        child_cmd.as_std_mut().pre_exec(move || {
            if let Some((ref u_fd, ref m_fd)) = ns_fds {
                let _ = nix::sched::setns(u_fd, CloneFlags::CLONE_NEWUSER);
                let _ = nix::sched::setns(m_fd, CloneFlags::CLONE_NEWNS);
                let _ = nix::unistd::chdir("/");
            }
            let _ = nix::unistd::setsid();
            nix::libc::ioctl(0, nix::libc::TIOCSCTTY as _, 0);
            nix::libc::tcsetpgrp(0, nix::libc::getpid());
            for sig in [
                nix::libc::SIGINT,
                nix::libc::SIGHUP,
                nix::libc::SIGTERM,
                nix::libc::SIGPIPE,
                nix::libc::SIGTSTP,
                nix::libc::SIGTTIN,
                nix::libc::SIGTTOU,
            ] {
                nix::libc::signal(sig, nix::libc::SIG_DFL);
            }
            Ok(())
        });
    }

    Ok((master, child_cmd))
}

fn spawn_with_pipes(
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
) -> Result<Command, String> {
    if let Some((root, _pid)) = rootfs_pid {
        let _ = rootfs::setup_exec_mounts(root);
    }

    let mut child_cmd = build_command(cmd, args, rootfs_pid);

    child_cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    Ok(child_cmd)
}

fn try_open_namespace_fds(container_pid: u32) -> Option<(OwnedFd, OwnedFd)> {
    let user_ns_path = format!("/proc/{}/ns/user", container_pid);
    let mnt_ns_path = format!("/proc/{}/ns/mnt", container_pid);
    let open_flags = OFlag::O_RDONLY | OFlag::O_CLOEXEC;
    let user_fd = nix::fcntl::open(user_ns_path.as_str(), open_flags, nix::sys::stat::Mode::empty()).ok()?;
    let mnt_fd = nix::fcntl::open(mnt_ns_path.as_str(), open_flags, nix::sys::stat::Mode::empty()).ok()?;
    Some((user_fd, mnt_fd))
}

fn is_root() -> bool {
    rootfs::is_root()
}

fn build_command(cmd: &str, args: &[&str], rootfs_pid: Option<(&str, u32)>) -> Command {
    if let Some((_root, container_pid)) = rootfs_pid {
        // Join the container's user+mount namespaces for real isolation.
        // ns/user must be joined first to gain capabilities, then ns/mnt.
        let user_ns_path = format!("/proc/{}/ns/user", container_pid);
        let mnt_ns_path = format!("/proc/{}/ns/mnt", container_pid);
        let open_flags = OFlag::O_RDONLY | OFlag::O_CLOEXEC;
        let user_fd = nix::fcntl::open(
            user_ns_path.as_str(),
            open_flags,
            nix::sys::stat::Mode::empty(),
        );
        let mnt_fd = nix::fcntl::open(
            mnt_ns_path.as_str(),
            open_flags,
            nix::sys::stat::Mode::empty(),
        );
        let fds = match (user_fd, mnt_fd) {
            (Ok(u), Ok(m)) => Some((u, m)),
            _ => None,
        };

        let mut c = Command::new(cmd);
        for a in args {
            c.arg(a);
        }
        let rootfs_for_exec = rootfs_pid.map(|(r, _)| r.to_string());
        unsafe { c.as_std_mut().pre_exec(move || {
            if let Some((ref u_fd, ref m_fd)) = fds {
                nix::sched::setns(u_fd, CloneFlags::CLONE_NEWUSER)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("setns(CLONE_NEWUSER): {e}")))?;
                nix::sched::setns(m_fd, CloneFlags::CLONE_NEWNS)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("setns(CLONE_NEWNS): {e}")))?;
                // After setns(NEWNS): "/" is the container root when pivot_root was used.
                // When chroot fallback was used, "/" is still the host root — chroot
                // into the rootfs path (which is visible from the host-root view).
                if let Some(ref rfs) = rootfs_for_exec {
                    let _ = nix::unistd::chroot(rfs.as_str());
                }
                nix::unistd::chdir("/")
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("chdir(/): {e}")))?;
            }
            Ok(())
        }); }
        c
    } else if let Some(root) = rootfs_pid.map(|(r, _)| r) {
        // No container PID — just set cwd to rootfs (legacy non-isolated path)
        if is_root() {
            let mut c = Command::new("chroot");
            c.arg(root).arg(cmd);
            for a in args {
                c.arg(a);
            }
            c
        } else {
            let mut c = Command::new(cmd);
            for a in args {
                c.arg(a);
            }
            c.current_dir(root);
            c
        }
    } else {
        let mut c = Command::new(cmd);
        for a in args {
            c.arg(a);
        }
        c
    }
}

async fn exec_ws(
    mut socket: WebSocket,
    cmds: Vec<String>,
    rootfs_pid: Option<(String, u32)>,
    tty: bool,
    stdin_flag: bool,
    stdout_flag: bool,
    stderr_flag: bool,
) {
    if cmds.is_empty() {
        let _ = socket.close().await;
        return;
    }

    let cmd = cmds[0].clone();
    let args: Vec<&str> = cmds.iter().skip(1).map(|s| s.as_str()).collect();
    info!("Exec WS: cmd={}, args={:?}, rootfs={:?}, tty={}", cmd, args, rootfs_pid.as_ref().map(|(r, _)| r.as_str()), tty);

    if tty {
        exec_ws_tty(socket, &cmd, &args, rootfs_pid.as_ref().map(|(r, p)| (r.as_str(), *p))).await
    } else {
        exec_ws_pipes(socket, &cmd, &args, rootfs_pid.as_ref().map(|(r, p)| (r.as_str(), *p)), stdin_flag, stdout_flag, stderr_flag).await
    }
}

async fn exec_ws_tty(mut socket: WebSocket, cmd: &str, args: &[&str], rootfs_pid: Option<(&str, u32)>) {
    let (master, mut child_cmd) = match spawn_with_pty(cmd, args, rootfs_pid) {
        Ok(pair) => pair,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("error: {}", e).into()))
                .await;
            return;
        }
    };

    let mut child = match child_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("spawn error: {}", e).into()))
                .await;
            return;
        }
    };
    drop(child_cmd);

    info!("Exec WS TTY: child PID {:?}", child.id());

    let async_master =
        tokio::io::unix::AsyncFd::new(master).expect("AsyncFd for PTY master");
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut read_buf = vec![0u8; 4096];

    loop {
        tokio::select! {
            result = async_master.readable() => {
                let mut guard = match result { Ok(g) => g, _ => break };
                match guard.try_io(|inner| {
                    nix::unistd::read(inner.get_ref(), &mut read_buf)
                        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
                }) {
                    Ok(Ok(0)) => break,
                    Ok(Ok(n)) => {
                        let mut frame = vec![1u8];
                        frame.extend_from_slice(&read_buf[..n]);
                        if ws_tx.send(Message::Binary(axum::body::Bytes::from(frame))).await.is_err() {
                            break;
                        }
                    }
                    Ok(Err(e)) => { info!("PTY read: {}", e); break; }
                    Err(_would_block) => {}
                }
            }

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) if !data.is_empty() => {
                        match data[0] {
                            0 if data.len() > 1 => {
                                let mut guard = async_master.writable().await.unwrap();
                                let _ = guard.try_io(|inner| {
                                    nix::unistd::write(inner, &data[1..])
                                        .map(|_| 0usize)
                                        .map_err(std::io::Error::other)
                                });
                            }
                            4 if data.len() > 1 => {
                                if let Ok(r) = serde_json::from_slice::<serde_json::Value>(&data[1..]) {
                                    let cols = r.get("Width").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                                    let rows = r.get("Height").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                                    set_winsize(async_master.get_ref().as_raw_fd(), cols, rows);
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

    let exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(0);
    send_exit_status(&mut ws_tx, exit_code).await;
}

async fn exec_ws_pipes(
    mut socket: WebSocket,
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
    stdin_flag: bool,
    stdout_flag: bool,
    stderr_flag: bool,
) {
    let mut child_cmd = match spawn_with_pipes(cmd, args, rootfs_pid) {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("error: {}", e).into()))
                .await;
            return;
        }
    };

    let mut child = match child_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("spawn error: {}", e).into()))
                .await;
            return;
        }
    };

    info!("Exec WS pipes: child PID {:?}", child.id());

    let child_stdin = if stdin_flag { child.stdin.take() } else { None };
    let child_stdout = if stdout_flag { child.stdout.take() } else { None };
    let child_stderr = if stderr_flag { child.stderr.take() } else { None };

    let (ws_tx, mut ws_rx) = socket.split();
    let ws_tx = std::sync::Arc::new(tokio::sync::Mutex::new(ws_tx));
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(64);

    if let Some(stdout) = child_stdout {
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let mut stdout = tokio::io::BufReader::new(stdout);
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut frame = vec![1u8];
                        frame.extend_from_slice(&buf[..n]);
                        if out_tx.send(Message::Binary(axum::body::Bytes::from(frame))).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    if let Some(stderr) = child_stderr {
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let mut stderr = tokio::io::BufReader::new(stderr);
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut frame = vec![2u8];
                        frame.extend_from_slice(&buf[..n]);
                        if out_tx.send(Message::Binary(axum::body::Bytes::from(frame))).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    drop(out_tx);

    let mut child_stdin = child_stdin;
    let mut child = child;
    let ws_tx_arc = ws_tx.clone();

    let forwarder = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let mut tx = ws_tx_arc.lock().await;
            if tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) if !data.is_empty() => {
                        if data[0] == 0 {
                            if let Some(stdin) = child_stdin.as_mut() {
                                let _ = stdin.write_all(&data[1..]).await;
                                let _ = stdin.flush().await;
                            }
                        } else if data[0] == 0xFF && data.len() >= 2 {
                            // Client closed a stream (channel 255 + stream_id byte)
                            if data[1] == 0 {
                                drop(child_stdin.take());
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        drop(child_stdin);
                        let _ = child.kill().await;
                        break;
                    }
                    Some(Err(e)) => {
                        drop(child_stdin);
                        child.kill().await.ok();
                        break;
                    }
                    None => {
                        drop(child_stdin);
                        let _ = child.kill().await;
                        break;
                    }
                    _ => {}
                }
            }

            status = child.wait() => {
                drop(child_stdin);
                let exit_code = status.ok().and_then(|s| s.code()).unwrap_or(0);
                let mut tx = ws_tx.lock().await;
                send_exit_status(&mut tx, exit_code).await;
                forwarder.abort();
                return;
            }
        }
    }

    let exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(0);
    let mut tx = ws_tx.lock().await;
    send_exit_status(&mut tx, exit_code).await;
    forwarder.abort();
}

async fn send_exit_status(ws_tx: &mut futures_util::stream::SplitSink<WebSocket, Message>, exit_code: i32) {
    let (status_str, message) = if exit_code == 0 {
        ("Success", "command exited with code 0".to_string())
    } else {
        ("Failure", format!("command exited with code {}", exit_code))
    };
    let status_json = serde_json::json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {},
        "status": status_str, "message": message,
        "details": { "exitCode": exit_code }
    });
    let mut frame = vec![3u8];
    frame.extend_from_slice(serde_json::to_string(&status_json).unwrap_or_default().as_bytes());
    let _ = ws_tx.send(Message::Binary(axum::body::Bytes::from(frame))).await;
    let _ = ws_tx
        .send(Message::Close(Some(CloseFrame {
            code: 1000,
            reason: Default::default(),
        })))
        .await;
}
