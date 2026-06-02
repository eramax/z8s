use crate::api::server::AppState;
use crate::cri::rootfs;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use nix::fcntl::OFlag;
use nix::mount::{MsFlags, mount};
use nix::pty;
use nix::sched::CloneFlags;
use nix::sys::termios::{self, InputFlags, LocalFlags, OutputFlags, SetArg};
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::info;

use crate::cri::runtime::RunningContainer;

#[derive(Clone)]
pub struct ExecState(pub Arc<Mutex<HashMap<String, RunningContainer>>>);

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
        let params = crate::cri::exec_query::parse_query_params(query);
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
    let info = resolve_container(&state, &name, params.container.as_deref()).await;
    let rootfs_pid = info.as_ref().and_then(|i| i.rootfs_pid.clone());
    let env_vars = info
        .as_ref()
        .map(|i| i.env_vars.clone())
        .unwrap_or_default();
    let isolated_net = info.as_ref().map(|i| i.isolated_net).unwrap_or(false);
    let run_as_user = info.as_ref().and_then(|i| i.run_as_user);
    let run_as_group = info.as_ref().and_then(|i| i.run_as_group);
    ws.protocols(
        crate::cri::exec_protocol::K8S_EXEC_PROTOCOLS
            .iter()
            .copied(),
    )
    .on_upgrade(move |socket| {
        exec_ws(
            socket,
            cmds,
            rootfs_pid,
            env_vars,
            isolated_net,
            run_as_user,
            run_as_group,
            tty,
            stdin_flag,
            stdout_flag,
            stderr_flag,
        )
    })
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

struct ContainerExecInfo {
    rootfs_pid: Option<(String, u32)>,
    env_vars: Vec<(String, String)>,
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
}

async fn resolve_container(
    state: &AppState,
    pod_name: &str,
    container_name: Option<&str>,
) -> Option<ContainerExecInfo> {
    let running = state.process_tracker.running.lock().await;
    let prefix = format!("{}-", pod_name);
    let rc = if let Some(container) = container_name {
        running.get(&format!("{}-{}", pod_name, container))
    } else {
        running
            .iter()
            .find(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v)
    }?;

    let pid = rc.instance.pid?;
    let env_vars = merge_exec_env(&rc.instance.env_vars, pid);
    let rootfs = rc.instance.rootfs.clone();
    let rootfs_pid = if rootfs.is_empty() {
        None
    } else {
        Some((rootfs, pid))
    };

    Some(ContainerExecInfo {
        rootfs_pid,
        env_vars,
        isolated_net: rc.instance.isolated_net,
        run_as_user: rc.instance.run_as_user,
        run_as_group: rc.instance.run_as_group,
    })
}

fn spawn_with_pty(
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
    env_vars: &[(String, String)],
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
) -> Result<(pty::PtyMaster, Command), String> {
    let master = pty::posix_openpt(OFlag::O_RDWR | OFlag::O_NONBLOCK)
        .map_err(|e| format!("posix_openpt: {}", e))?;
    pty::grantpt(&master).map_err(|e| format!("grantpt: {}", e))?;
    pty::unlockpt(&master).map_err(|e| format!("unlockpt: {}", e))?;
    let slave_name = unsafe { pty::ptsname(&master) }.map_err(|e| format!("ptsname: {}", e))?;

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

    let mut child_cmd = build_command(
        cmd,
        args,
        rootfs_pid,
        env_vars,
        isolated_net,
        run_as_user,
        run_as_group,
    );

    child_cmd
        .stdin(Stdio::from(
            slave
                .try_clone()
                .map_err(|e| format!("clone stdin: {}", e))?,
        ))
        .stdout(Stdio::from(
            slave
                .try_clone()
                .map_err(|e| format!("clone stdout: {}", e))?,
        ))
        .stderr(Stdio::from(slave));
    child_cmd.kill_on_drop(true);

    let ns_fds = rootfs_pid.map(|(_, pid)| try_open_namespace_fds(pid));
    let use_mnt_for_pty = ns_fds.as_ref().map_or(false, |n| n.mnt.is_some());

    unsafe {
        child_cmd.as_std_mut().pre_exec(move || {
            if let Some(ref ns) = ns_fds {
                let _ = enter_container_namespaces(ns, isolated_net, use_mnt_for_pty);
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
    env_vars: &[(String, String)],
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
) -> Result<Command, String> {
    if let Some((root, _pid)) = rootfs_pid {
        let _ = rootfs::setup_exec_mounts(root);
    }

    let mut child_cmd = build_command(
        cmd,
        args,
        rootfs_pid,
        env_vars,
        isolated_net,
        run_as_user,
        run_as_group,
    );

    child_cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    Ok(child_cmd)
}

struct ContainerNamespaces {
    user: Option<OwnedFd>,
    pid: Option<OwnedFd>,
    mnt: Option<OwnedFd>,
    net: Option<OwnedFd>,
}

fn try_open_namespace_fds(container_pid: u32) -> ContainerNamespaces {
    let open_flags = OFlag::O_RDONLY | OFlag::O_CLOEXEC;
    let mode = nix::sys::stat::Mode::empty();
    let open_one = |path: &str| nix::fcntl::open(path, open_flags, mode).ok();
    ContainerNamespaces {
        user: open_one(&format!("/proc/{}/ns/user", container_pid)),
        pid: open_one(&format!("/proc/{}/ns/pid", container_pid)),
        mnt: open_one(&format!("/proc/{}/ns/mnt", container_pid)),
        net: open_one(&format!("/proc/{}/ns/net", container_pid)),
    }
}

fn enter_container_namespaces(
    ns: &ContainerNamespaces,
    isolated_net: bool,
    use_mnt_ns: bool,
) -> Result<(), std::io::Error> {
    if let Some(ref user) = ns.user {
        let _ = nix::sched::setns(user, CloneFlags::CLONE_NEWUSER);
    }
    if use_mnt_ns {
        if let Some(ref mnt) = ns.mnt {
            nix::sched::setns(mnt, CloneFlags::CLONE_NEWNS).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("setns(CLONE_NEWNS): {e}"),
                )
            })?;
        }
    }
    if isolated_net {
        if let Some(ref net) = ns.net {
            nix::sched::setns(net, CloneFlags::CLONE_NEWNET).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("setns(CLONE_NEWNET): {e}"),
                )
            })?;
        }
    }
    Ok(())
}

fn is_root() -> bool {
    rootfs::is_root()
}

fn read_container_environ(pid: u32) -> Vec<(String, String)> {
    let Ok(data) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for var in data.split(|&b| b == 0) {
        if var.is_empty() {
            continue;
        }
        let Some(eq) = var.iter().position(|&b| b == b'=') else {
            continue;
        };
        let Ok(key) = std::str::from_utf8(&var[..eq]) else {
            continue;
        };
        let Ok(val) = std::str::from_utf8(&var[eq + 1..]) else {
            continue;
        };
        out.push((key.to_string(), val.to_string()));
    }
    out
}

fn read_container_path(pid: u32) -> String {
    for (k, v) in read_container_environ(pid) {
        if k == "PATH" {
            return v;
        }
    }
    std::env::var("PATH").unwrap_or_else(|_| {
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
    })
}

fn merge_exec_env(
    stored: &[(String, String)],
    pid: u32,
) -> Vec<(String, String)> {
    let mut out = read_container_environ(pid);
    let mut seen: std::collections::HashSet<String> =
        out.iter().map(|(k, _)| k.clone()).collect();
    for (k, v) in stored {
        if seen.insert(k.clone()) {
            out.push((k.clone(), v.clone()));
        }
    }
    out
}

fn build_command(
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
    env_vars: &[(String, String)],
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
) -> Command {
    let apply_env = |c: &mut Command| {
        c.env_clear();
        let mut has_path = false;
        for (k, v) in env_vars {
            c.env(k, v);
            if k == "PATH" {
                has_path = true;
            }
        }
        if !has_path {
            let fallback_path = std::env::var("PATH").unwrap_or_else(|_| {
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
            });
            if let Some((_, pid)) = rootfs_pid {
                c.env("PATH", read_container_path(pid));
            } else {
                c.env("PATH", fallback_path);
            }
        }
    };
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();

    if let Some((root, container_pid)) = rootfs_pid {
        use std::os::unix::fs::MetadataExt;
        let (c_uid, c_gid) = if let Ok(m) = std::fs::metadata(format!("/proc/{container_pid}")) {
            (Some(m.uid()), Some(m.gid()))
        } else {
            (None, None)
        };

        let ns_fds = try_open_namespace_fds(container_pid);
        if ns_fds.mnt.is_none() {
            tracing::warn!(
                "exec: cannot open mnt namespace for PID {} (stale PID?). Falling back to host namespace.",
                container_pid
            );
        }
        let can_enter_mnt = ns_fds.mnt.is_some();
        let (exec_path, prog_args) = if can_enter_mnt {
            rootfs::build_container_argv_in_mount_ns(cmd, &args_owned, root)
        } else {
            rootfs::build_container_argv(cmd, &args_owned, root)
        };
        // If the resolved binary is a bare name (no /), it wasn't found in the
        // container rootfs — don't enter the mount namespace so the host PATH
        // is searched instead (e.g. wget in a scratch/minimal image).
        let binary_in_rootfs = exec_path.contains('/');
        let use_mnt_ns = can_enter_mnt && binary_in_rootfs;
        let (program, prog_args) = if use_mnt_ns {
            (exec_path, prog_args)
        } else {
            rootfs::wrap_dynamic_linker(&exec_path, prog_args, root)
        };

        let mut c = Command::new(&program);
        for a in &prog_args {
            c.arg(a);
        }
        apply_env(&mut c);

        let iso_net = isolated_net;
        let exec_gid = run_as_group;
        let exec_uid = run_as_user;
        unsafe {
            c.as_std_mut().pre_exec(move || {
                let _ = enter_container_namespaces(&ns_fds, iso_net, use_mnt_ns);
                if use_mnt_ns {
                    if let Some(ref pid) = ns_fds.pid {
                        let _ = nix::sched::setns(pid, CloneFlags::CLONE_NEWPID);
                        let child = nix::libc::fork();
                        if child > 0 {
                            // Parent exits immediately — no waitpid (would block tokio).
                            nix::libc::_exit(0);
                        }
                        // Child: mount fresh procfs scoped to this PID namespace.
                        if let Err(e) = mount(
                            Some("proc"),
                            "/proc",
                            Some("proc"),
                            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
                            None::<&str>,
                        ) {
                            tracing::warn!("exec: mount proc failed: {e}");
                        }
                    }
                }
                if let Some(gid) = exec_gid {
                    let _ = nix::unistd::setgid(nix::unistd::Gid::from_raw(gid));
                }
                if let Some(uid) = exec_uid {
                    let _ = nix::unistd::setuid(nix::unistd::Uid::from_raw(uid));
                }
                Ok(())
            });
        }
        c
    } else {
        let mut c = Command::new(cmd);
        for a in args {
            c.arg(a);
        }
        apply_env(&mut c);
        c
    }
}

async fn exec_ws(
    mut socket: WebSocket,
    cmds: Vec<String>,
    rootfs_pid: Option<(String, u32)>,
    env_vars: Vec<(String, String)>,
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
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
    info!(
        "Exec WS: cmd={}, args={:?}, rootfs={:?}, tty={}",
        cmd,
        args,
        rootfs_pid.as_ref().map(|(r, _)| r.as_str()),
        tty
    );

    if tty {
        exec_ws_tty(
            socket,
            &cmd,
            &args,
            rootfs_pid.as_ref().map(|(r, p)| (r.as_str(), *p)),
            &env_vars,
            isolated_net,
            run_as_user,
            run_as_group,
        )
        .await
    } else {
        exec_ws_pipes(
            socket,
            &cmd,
            &args,
            rootfs_pid.as_ref().map(|(r, p)| (r.as_str(), *p)),
            &env_vars,
            isolated_net,
            run_as_user,
            run_as_group,
            stdin_flag,
            stdout_flag,
            stderr_flag,
        )
        .await
    }
}

async fn send_error_and_close(
    ws_tx: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    msg: &str,
) {
    crate::cri::exec_protocol::send_exit_status_msg(ws_tx, 1, Some(msg)).await;
}

async fn exec_ws_tty(
    socket: WebSocket,
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
    env_vars: &[(String, String)],
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let (master, mut child_cmd) = match spawn_with_pty(
        cmd,
        args,
        rootfs_pid,
        env_vars,
        isolated_net,
        run_as_user,
        run_as_group,
    ) {
            Ok(pair) => pair,
            Err(e) => {
                send_error_and_close(&mut ws_tx, &format!("pty setup: {}", e)).await;
                return;
            }
        };

    let mut child = match child_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            send_error_and_close(&mut ws_tx, &format!("spawn error: {}", e)).await;
            return;
        }
    };
    drop(child_cmd);

    info!("Exec WS TTY: child PID {:?}", child.id());

    let async_master = tokio::io::unix::AsyncFd::new(master).expect("AsyncFd for PTY master");
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
                        let frame = crate::cri::exec_protocol::encode_frame(
                            crate::cri::exec_protocol::CHANNEL_STDOUT,
                            &read_buf[..n],
                        );
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
                                let buf = &data[1..];
                                let mut written = 0;
                                while written < buf.len() {
                                    let mut guard = match async_master.writable().await {
                                        Ok(g) => g,
                                        Err(e) => { tracing::warn!("PTY writable: {e}"); break; }
                                    };
                                    match guard.try_io(|inner| {
                                        nix::unistd::write(inner, &buf[written..])
                                            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
                                    }) {
                                        Ok(Ok(0)) => break,
                                        Ok(Ok(n)) => written += n,
                                        _ => break,
                                    }
                                }
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
                    Some(Ok(Message::Close(frame))) => {
                        let _ = child.kill().await;
                        let _ = ws_tx.send(Message::Close(frame)).await;
                        break;
                    }
                    None => {
                        let _ = child.kill().await;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(0);
    crate::cri::exec_protocol::send_exit_status(&mut ws_tx, exit_code).await;
}

async fn exec_ws_pipes(
    socket: WebSocket,
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
    env_vars: &[(String, String)],
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
    stdin_flag: bool,
    stdout_flag: bool,
    stderr_flag: bool,
) {
    let (ws_tx, mut ws_rx) = socket.split();
    let ws_tx = std::sync::Arc::new(tokio::sync::Mutex::new(ws_tx));

    let mut child_cmd = match spawn_with_pipes(
        cmd,
        args,
        rootfs_pid,
        env_vars,
        isolated_net,
        run_as_user,
        run_as_group,
    ) {
        Ok(c) => c,
        Err(e) => {
            let mut tx = ws_tx.lock().await;
            send_error_and_close(&mut tx, &format!("pipe setup: {}", e)).await;
            return;
        }
    };

    let mut child = match child_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut tx = ws_tx.lock().await;
            send_error_and_close(&mut tx, &format!("spawn error: {}", e)).await;
            return;
        }
    };

    info!("Exec WS pipes: child PID {:?}", child.id());

    let child_stdin = if stdin_flag { child.stdin.take() } else { None };
    let child_stdout = if stdout_flag {
        child.stdout.take()
    } else {
        None
    };
    let child_stderr = if stderr_flag {
        child.stderr.take()
    } else {
        None
    };

    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(64);

    let mut reader_handles = Vec::new();
    if let Some(stdout) = child_stdout {
        let out_tx = out_tx.clone();
        reader_handles.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let mut stdout = tokio::io::BufReader::new(stdout);
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let frame = crate::cri::exec_protocol::encode_frame(
                            crate::cri::exec_protocol::CHANNEL_STDOUT,
                            &buf[..n],
                        );
                        if out_tx
                            .send(Message::Binary(axum::body::Bytes::from(frame)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }));
    }

    if let Some(stderr) = child_stderr {
        let out_tx = out_tx.clone();
        reader_handles.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let mut stderr = tokio::io::BufReader::new(stderr);
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let frame = crate::cri::exec_protocol::encode_frame(
                            crate::cri::exec_protocol::CHANNEL_STDERR,
                            &buf[..n],
                        );
                        if out_tx
                            .send(Message::Binary(axum::body::Bytes::from(frame)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }));
    }
    drop(out_tx);

    let mut child_stdin = child_stdin;
    let ws_tx_arc = ws_tx.clone();

    let forwarder = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let mut tx = ws_tx_arc.lock().await;
            if tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut child_done = false;
    let mut exit_code = 0;

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
                        } else if data[0] == 0xFF && data.len() >= 2 && data[1] == 0 {
                            drop(child_stdin.take());
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        drop(child_stdin.take());
                        let _ = child.kill().await;
                        {
                            let mut tx = ws_tx.lock().await;
                            let _ = tx.send(Message::Close(frame)).await;
                        }
                        if !child_done {
                            exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(137);
                            child_done = true;
                        }
                        break;
                    }
                    None => {
                        drop(child_stdin.take());
                        let _ = child.kill().await;
                        if !child_done {
                            exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(137);
                            child_done = true;
                        }
                        break;
                    }
                    Some(Err(_)) => {
                        drop(child_stdin.take());
                        child.kill().await.ok();
                        if !child_done {
                            exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(137);
                            child_done = true;
                        }
                        break;
                    }
                    _ => {}
                }
            }

            status = child.wait(), if !child_done => {
                drop(child_stdin.take());
                exit_code = status.ok().and_then(|s| s.code()).unwrap_or(0);
                child_done = true;
                break;
            }
        }
    }

    for h in reader_handles {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), h).await;
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), forwarder).await;

    let mut tx = ws_tx.lock().await;
    crate::cri::exec_protocol::send_exit_status(&mut tx, exit_code).await;
}
