use crate::server::api::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::extract::{FromRequestParts, Path, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::http::StatusCode;
use axum::http::request::Parts;
use futures::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::info;
use std::collections::HashMap;

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

    // SPDY exec – not implemented, tell user to use WebSocket
    (StatusCode::METHOD_NOT_ALLOWED, axum::Json(serde_json::json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {},
        "status": "Failure",
        "message": "SPDY exec not implemented; set KUBECTL_REMOTE_COMMAND_WEBSOCKETS=true for WebSocket exec",
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

async fn exec_ws(mut socket: WebSocket, cmds: Vec<String>, rootfs: Option<String>) {
    if cmds.is_empty() {
        info!("Exec WS: no commands provided");
        let _ = socket.close().await;
        return;
    }

    let cmd = cmds[0].clone();
    let args: Vec<&str> = cmds.iter().skip(1).map(|s| s.as_str()).collect();

    info!("Exec WS: cmd={}, args={:?}, rootfs={:?}", cmd, args, rootfs);

    let mut child = if let Some(ref rootfs) = rootfs {
        info!("Executing in container rootfs: {}", rootfs);
        match Command::new("chroot").arg(rootfs).arg(&cmd).args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => { let _ = socket.send(Message::Text(format!("error: {}", e).into())).await; return; }
        }
    } else {
        info!("No container rootfs found, executing on host");
        match Command::new(&cmd).args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => { let _ = socket.send(Message::Text(format!("error: {}", e).into())).await; return; }
        }
    };

    info!("Exec WS: child spawned with PID {:?}", child.id());

    let mut child_stdin = child.stdin.take().expect("stdin");
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Read stdout → WebSocket channel 1
    {
        let mut sender = ws_sender;
        if let Some(stdout) = child.stdout.take() {
            let mut reader = tokio::io::BufReader::new(stdout);
            let mut buf = vec![0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut frame = vec![1u8];
                        frame.extend_from_slice(&buf[..n]);
                        if sender.send(Message::Binary(axum::body::Bytes::from(frame))).await.is_err() { break; }
                    }
                    Err(_) => break,
                }
            }
        }
        ws_sender = sender;
    }

    // Read stderr → WebSocket channel 2
    if let Some(stderr) = child.stderr.take() {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = vec![0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut frame = vec![2u8];
                    frame.extend_from_slice(&buf[..n]);
                    if ws_sender.send(Message::Binary(axum::body::Bytes::from(frame))).await.is_err() { break; }
                }
                Err(_) => break,
            }
        }
    }

    // Forward stdin from WebSocket channel 0 to process, wait for child exit
    use tokio::io::AsyncWriteExt;
    loop {
        tokio::select! {
            status = child.wait() => {
                let exit_code = status.ok().and_then(|s| s.code()).unwrap_or(0) as u8;
                // Send exit status as a JSON Status object on channel 3 (error channel)
                let status_obj = if exit_code == 0 {
                    None
                } else {
                    Some(serde_json::json!({
                        "kind": "Status",
                        "apiVersion": "v1",
                        "metadata": {},
                        "status": "Failure",
                        "message": format!("command exited with code {}", exit_code),
                        "reason": "NonZeroExitCode",
                        "details": { "exitCode": exit_code }
                    }))
                };
                if let Some(status) = status_obj {
                    let data = serde_json::to_string(&status).unwrap_or_default();
                    let mut frame = vec![3u8];
                    frame.extend_from_slice(data.as_bytes());
                    let _ = ws_sender.send(Message::Binary(axum::body::Bytes::from(frame))).await;
                }
                // Send WebSocket close frame with proper code 1000 (Normal Closure)
                let _ = ws_sender.send(Message::Close(Some(CloseFrame { code: 1000, reason: Default::default() }))).await;
                break;
            }
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if data.first() == Some(&0) && data.len() > 1 {
                            let _ = child_stdin.write_all(&data[1..]).await;
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
}
