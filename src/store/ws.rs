use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::store::StoreBackend;
use crate::store::gossip::{GossipMessage, GossipState, SyncEntry};

/// Handle an incoming WebSocket connection from a peer.
pub async fn handle_gossip_ws(
    ws: WebSocket,
    state: Arc<tokio::sync::Mutex<GossipState>>,
    notify: Arc<tokio::sync::Notify>,
) {
    info!("Gossip peer connected");

    let (mut ws_sender, mut ws_receiver) = ws.split();
    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    let (broadcast_tx, mut broadcast_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    state.lock().await.add_peer(broadcast_tx);

    let msg_tx_clone = msg_tx.clone();
    tokio::spawn(async move {
        while let Some(data) = broadcast_rx.recv().await {
            if let Ok(text) = String::from_utf8(data) {
                if msg_tx_clone.send(Message::Text(text.into())).is_err() {
                    break;
                }
            }
        }
    });

    tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    let msg = GossipMessage::SyncRequest { request_id: 0 };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = msg_tx.send(Message::Text(json.into()));
    }

    loop {
        match ws_receiver.next().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(msg) = serde_json::from_str::<GossipMessage>(&text) {
                    handle_message(msg, &msg_tx, &state, &notify).await;
                }
            }
            Some(Ok(Message::Close(_))) => {
                info!("Gossip peer disconnected");
                break;
            }
            Some(Ok(Message::Ping(data))) => {
                let _ = msg_tx.send(Message::Pong(data));
            }
            Some(Err(e)) => {
                warn!("Gossip WS error: {}", e);
                break;
            }
            None => break,
            _ => {}
        }
    }
}

async fn handle_message(
    msg: GossipMessage,
    tx: &tokio::sync::mpsc::UnboundedSender<Message>,
    state: &Arc<tokio::sync::Mutex<GossipState>>,
    notify: &Arc<tokio::sync::Notify>,
) {
    match msg {
        GossipMessage::Gossip {
            key,
            value,
            term,
            source: _,
        } => {
            tracing::info!("handle_message Gossip: key={}", key);
            let (should_apply, db) = {
                let mut st = state.lock().await;
                (st.dedup(&key, term), st.db.clone())
            };
            if should_apply {
                if let Ok(resource) = serde_json::from_slice::<crate::store::AnyResource>(&value) {
                    // Wake the reconciler if this is a Pod assignment for the local node
                    if let crate::store::AnyResource::Pod(ref p) = resource {
                        let local = crate::config::get().node_name.clone();
                        if p.assigned_node.as_deref() == Some(local.as_str()) {
                            notify.notify_one();
                        }
                    }
                    if let Err(e) = db.apply(resource).await {
                        tracing::warn!("Failed to apply gossiped resource {}: {}", key, e);
                    } else {
                        tracing::debug!("Applied gossiped resource {} (term {})", key, term);
                    }
                }
            }
        }
        GossipMessage::SyncRequest { request_id } => {
            let st = state.lock().await;
            let resources = st.db.get_all().await;
            let entries: Vec<SyncEntry> = resources
                .into_iter()
                .map(|t| {
                    let key = t.resource.uid();
                    let term = st.seen.get(&key).copied().unwrap_or(0);
                    let value = serde_json::to_vec(&t.resource).unwrap_or_default();
                    SyncEntry { key, value, term }
                })
                .collect();
            let count = entries.len();
            let response = GossipMessage::SyncFull {
                request_id,
                entries,
            };
            if let Ok(json) = serde_json::to_string(&response) {
                let _ = tx.send(Message::Text(json.into()));
            }
            info!("Sent sync_full ({} entries)", count);
        }
        GossipMessage::SyncFull { ref entries, .. } => {
            let db = state.lock().await.db.clone();
            for entry in entries.iter() {
                if let Ok(resource) =
                    serde_json::from_slice::<crate::store::AnyResource>(&entry.value)
                {
                    // Wake the reconciler on Pod assignments for this node
                    if let crate::store::AnyResource::Pod(ref p) = resource {
                        let local = crate::config::get().node_name.clone();
                        if p.assigned_node.as_deref() == Some(local.as_str()) {
                            notify.notify_one();
                        }
                    }
                    db.apply(resource).await.ok();
                }
                state
                    .lock()
                    .await
                    .seen
                    .insert(entry.key.clone(), entry.term);
            }
        }
        GossipMessage::Heartbeat => {
            let _ = tx.send(Message::Text(
                serde_json::to_string(&GossipMessage::Heartbeat)
                    .unwrap()
                    .into(),
            ));
        }
        _ => {}
    }
}

/// Connect to a peer and run the gossip loop.
pub async fn run_gossip_client(
    peer_name: String,
    peer_url: String,
    state: Arc<tokio::sync::Mutex<GossipState>>,
    mut broadcast_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    notify: Arc<tokio::sync::Notify>,
) {
    loop {
        match tokio_tungstenite::connect_async(&peer_url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to gossip peer {} at {}", peer_name, peer_url);
                let (mut write, mut read) = ws_stream.split();

                let msg = GossipMessage::SyncRequest { request_id: 0 };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = write
                        .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
                        .await;
                }

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    if let Ok(gmsg) = serde_json::from_str::<GossipMessage>(&text) {
                                        match gmsg {
                                            GossipMessage::Gossip { key, value, term, source: _ } => {
                                                tracing::info!("Client received Gossip: key={}", key);
                                                let (should_apply, db) = {
                                                    let mut st = state.lock().await;
                                                    (st.dedup(&key, term), st.db.clone())
                                                };
                                                if should_apply {
                                                    if let Ok(resource) = serde_json::from_slice::<crate::store::AnyResource>(&value) {
                                                        // Wake reconciler on Pod assignments for local node
                                                        if let crate::store::AnyResource::Pod(ref p) = resource {
                                                            let local = crate::config::get().node_name.clone();
                                                            if p.assigned_node.as_deref() == Some(local.as_str()) {
                                                                notify.notify_one();
                                                            }
                                                        }
                                                        db.apply(resource).await.ok();
                                                    }
                                                }
                                            }
                                            GossipMessage::SyncFull { ref entries, .. } => {
                                                let db = { state.lock().await.db.clone() };
                                                for entry in entries.iter() {
                                                    if let Ok(resource) = serde_json::from_slice::<crate::store::AnyResource>(&entry.value) {
                                                        // Wake reconciler on Pod assignments for local node
                                                        if let crate::store::AnyResource::Pod(ref p) = resource {
                                                            let local = crate::config::get().node_name.clone();
                                                            if p.assigned_node.as_deref() == Some(local.as_str()) {
                                                                notify.notify_one();
                                                            }
                                                        }
                                                        db.apply(resource).await.ok();
                                                    }
                                                    state.lock().await.seen.insert(entry.key.clone(), entry.term);
                                                }
                                            }
                                            GossipMessage::SyncRequest { request_id } => {
                                                let st = state.lock().await;
                                                let resources = st.db.get_all().await;
                                                let entries: Vec<SyncEntry> = resources.into_iter().map(|t| {
                                                    let key = t.resource.uid();
                                                    let term = st.seen.get(&key).copied().unwrap_or(0);
                                                    let value = serde_json::to_vec(&t.resource).unwrap_or_default();
                                                    SyncEntry { key, value, term }
                                                }).collect();
                                                let response = GossipMessage::SyncFull { request_id, entries };
                                                if let Ok(json) = serde_json::to_string(&response) {
                                                    let _ = write.send(tokio_tungstenite::tungstenite::Message::Text(json.into())).await;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
                                    info!("Peer {} disconnected", peer_name);
                                    break;
                                }
                                Some(Err(e)) => {
                                    warn!("Gossip client error from {}: {}", peer_name, e);
                                    break;
                                }
                                None => break,
                                _ => {}
                            }
                        }
                        _ = sleep(Duration::from_secs(10)) => {
                            let msg = GossipMessage::Heartbeat;
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let _ = write.send(tokio_tungstenite::tungstenite::Message::Text(json.into())).await;
                            }
                        }
                        msg = broadcast_rx.recv() => {
                            if let Some(bytes) = msg {
                                let text = String::from_utf8_lossy(&bytes).to_string();
                                info!("Broadcast forwarding to {}: {}", peer_name, &text[..text.len().min(80)]);
                                let _ = write.send(tokio_tungstenite::tungstenite::Message::Text(text.into())).await;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to connect to peer {} ({}): {}. Retrying in 5s...",
                    peer_name, peer_url, e
                );
                sleep(Duration::from_secs(5)).await;
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}
