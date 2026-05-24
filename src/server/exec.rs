use crate::server::api::AppState;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Json};
use axum::http::StatusCode;
use futures::SinkExt;
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

/// Handles WebSocket exec (GET with Upgrade headers)
pub async fn exec_handler(
    ws: WebSocketUpgrade,
    State(_state): State<AppState>,
    Path((_namespace, name)): Path<(String, String)>,
    Query(params): Query<ExecParams>,
) -> impl IntoResponse {
    let cmds = params
        .command_list
        .clone()
        .or_else(|| params.command.clone().map(|c| vec![c]))
        .unwrap_or_default();

    info!(pod = %name, cmds = ?cmds, "Exec WebSocket request");

    ws.protocols(["v5.channel.k8s.io", "v4.channel.k8s.io", "v3.channel.k8s.io", "channel.k8s.io"])
        .on_upgrade(move |socket| exec_ws(socket, cmds))
}

/// Handles POST exec (kubectl's standard exec protocol)
pub async fn exec_post_handler(
    State(_state): State<AppState>,
    Path((_namespace, name)): Path<(String, String)>,
    Query(params): Query<ExecParams>,
) -> impl IntoResponse {
    let cmds = params
        .command_list
        .clone()
        .or_else(|| params.command.clone().map(|c| vec![c]))
        .unwrap_or_default();

    info!(pod = %name, cmds = ?cmds, "Exec POST request (stub)");

    // Return 405 forcing kubectl to fall back to WebSocket
    (StatusCode::METHOD_NOT_ALLOWED, Json(serde_json::json!({
        "kind": "Status",
        "apiVersion": "v1",
        "metadata": {},
        "status": "Failure",
        "message": "use WebSocket protocol",
        "reason": "MethodNotAllowed",
        "code": 405
    }))).into_response()
}

async fn exec_ws(mut socket: WebSocket, cmds: Vec<String>) {
    if cmds.is_empty() {
        let _ = socket.close().await;
        return;
    }

    let cmd = cmds[0].clone();
    let args: Vec<&str> = cmds.iter().skip(1).map(|s| s.as_str()).collect();

    // Simple exec: run command, capture all output, send it back
    let output = match Command::new(&cmd)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            let _ = socket.send(Message::Text(format!("error: {}", e).into())).await;
            return;
        }
    };

    // Send stdout on channel 1
    if !output.stdout.is_empty() {
        let mut frame = vec![1u8];
        frame.extend_from_slice(&output.stdout);
        let _ = socket.send(Message::Binary(axum::body::Bytes::from(frame))).await;
    }

    // Send stderr on channel 2
    if !output.stderr.is_empty() {
        let mut frame = vec![2u8];
        frame.extend_from_slice(&output.stderr);
        let _ = socket.send(Message::Binary(axum::body::Bytes::from(frame))).await;
    }

    // Send exit status on channel 3
    let exit_code = output.status.code().unwrap_or(-1);
    let mut frame = vec![3u8];
    frame.extend_from_slice(exit_code.to_string().as_bytes());
    let _ = socket.send(Message::Binary(axum::body::Bytes::from(frame))).await;

    let _ = socket.close().await;
}
