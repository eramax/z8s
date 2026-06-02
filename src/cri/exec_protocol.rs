//! kubectl exec WebSocket binary framing (v4/v5 channel.k8s.io).

use futures_util::SinkExt;
use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message};

/// stdin
pub const CHANNEL_STDIN: u8 = 0;
/// stdout
pub const CHANNEL_STDOUT: u8 = 1;
/// stderr
pub const CHANNEL_STDERR: u8 = 2;
/// error/status JSON
pub const CHANNEL_ERROR: u8 = 3;
/// resize (TTY)
pub const CHANNEL_RESIZE: u8 = 4;

pub const K8S_EXEC_PROTOCOLS: &[&str] = &[
    "v5.channel.k8s.io",
    "v4.channel.k8s.io",
    "v3.channel.k8s.io",
    "channel.k8s.io",
];

/// Prefix a payload with the multiplexed stream byte.
pub fn encode_frame(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + payload.len());
    frame.push(channel);
    frame.extend_from_slice(payload);
    frame
}

pub fn status_json(exit_code: i32, message: Option<&str>) -> serde_json::Value {
    let (status_str, message) = if let Some(msg) = message {
        ("Failure", msg.to_string())
    } else if exit_code == 0 {
        ("Success", "command exited with code 0".to_string())
    } else {
        ("Failure", format!("command exited with code {}", exit_code))
    };
    serde_json::json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {},
        "status": status_str, "message": message,
        "details": { "exitCode": exit_code }
    })
}

pub fn status_frame_bytes(exit_code: i32, message: Option<&str>) -> Vec<u8> {
    encode_frame(
        CHANNEL_ERROR,
        serde_json::to_string(&status_json(exit_code, message))
            .unwrap_or_default()
            .as_bytes(),
    )
}

pub fn error_frame_message(msg: &str) -> Message {
    Message::Binary(Bytes::from(status_frame_bytes(1, Some(msg))))
}

pub async fn send_exit_status(
    ws_tx: &mut futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    exit_code: i32,
) {
    send_exit_status_msg(ws_tx, exit_code, None).await;
}

pub async fn send_exit_status_msg(
    ws_tx: &mut futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    exit_code: i32,
    message: Option<&str>,
) {
    let _ = ws_tx
        .send(Message::Binary(Bytes::from(status_frame_bytes(
            exit_code, message,
        ))))
        .await;
    let _ = ws_tx
        .send(Message::Close(Some(CloseFrame {
            code: 1000,
            reason: Default::default(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_frame_prefix() {
        let f = encode_frame(CHANNEL_STDOUT, b"hi");
        assert_eq!(f, vec![1, b'h', b'i']);
    }

    #[test]
    fn status_frame_is_error_channel() {
        let f = status_frame_bytes(0, None);
        assert_eq!(f[0], CHANNEL_ERROR);
    }
}
