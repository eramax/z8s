use crate::server::api::AppState;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::info;

#[derive(Deserialize)]
pub struct ExecParams {
    pub command: Option<String>,
    #[serde(rename = "command[]")]
    pub command_list: Option<Vec<String>>,
    pub stdin: Option<bool>,
    pub stdout: Option<bool>,
    pub stderr: Option<bool>,
    pub tty: Option<bool>,
}

pub async fn exec_handler(
    ws: WebSocketUpgrade,
    State(_state): State<AppState>,
    Path((_namespace, name)): Path<(String, String)>,
    Query(params): Query<ExecParams>,
) -> impl IntoResponse {
    let cmds = params.command_list.clone()
        .or_else(|| params.command.clone().map(|c| vec![c]))
        .unwrap_or_default();
    info!(pod = %name, cmds = ?cmds, "Exec WS");
    ws.protocols(["v5.channel.k8s.io", "v4.channel.k8s.io", "v3.channel.k8s.io", "channel.k8s.io"])
        .on_upgrade(move |socket| exec_ws(socket, cmds))
}

pub async fn exec_post_handler(
    State(_state): State<AppState>,
    Path((_namespace, name)): Path<(String, String)>,
    Query(params): Query<ExecParams>,
) -> impl IntoResponse {
    let cmds = params.command_list.clone()
        .or_else(|| params.command.clone().map(|c| vec![c]))
        .unwrap_or_default();
    info!(pod = %name, cmds = ?cmds, "Exec POST");
    // Use GET-based WebSocket: set env KUBECTL_REMOTE_COMMAND_WEBSOCKETS=true
    (StatusCode::METHOD_NOT_ALLOWED, (axum::response::Json(serde_json::json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {},
        "status": "Failure", "message": "try KUBECTL_REMOTE_COMMAND_WEBSOCKETS=true",
        "reason": "MethodNotAllowed", "code": 405
    })))).into_response()
}

async fn exec_ws(mut socket: WebSocket, cmds: Vec<String>) {
    if cmds.is_empty() { let _ = socket.close().await; return; }

    let cmd = cmds[0].clone();
    let args: Vec<&str> = cmds.iter().skip(1).map(|s| s.as_str()).collect();

    let mut child = match Command::new(&cmd).args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => { let _ = socket.send(axum::extract::ws::Message::Text(format!("error: {}", e).into())).await; return; }
    };

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

    let _ = ws_sender.send(Message::Binary(axum::body::Bytes::from(vec![3u8]))).await;
    drop(ws_sender);

    // Forward stdin from WebSocket channel 0 to process
    use tokio::io::AsyncWriteExt;
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Binary(data) => {
                if data.first() == Some(&0) && data.len() > 1 {
                    let _ = child_stdin.write_all(&data[1..]).await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}
