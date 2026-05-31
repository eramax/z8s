use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::store::gossip::{GossipMessage, GossipState, SyncEntry};
use crate::store::StoreBackend;

/// Handle an incoming WebSocket connection from a peer.
pub async fn handle_gossip_ws(mut ws: WebSocket, state: Arc<tokio::sync::Mutex<GossipState>>) {
    info!("Gossip peer connected");

    let msg = GossipMessage::SyncRequest { request_id: 0 };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = ws.send(Message::Text(json.into())).await;
    }

    loop {
        match ws.recv().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(msg) = serde_json::from_str::<GossipMessage>(&text) {
                    handle_message(msg, &mut ws, &state).await;
                }
            }
            Some(Ok(Message::Close(_))) => {
                info!("Gossip peer disconnected");
                break;
            }
            Some(Ok(Message::Ping(data))) => {
                let _ = ws.send(Message::Pong(data)).await;
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

async fn handle_message(msg: GossipMessage, ws: &mut WebSocket, state: &Arc<tokio::sync::Mutex<GossipState>>) {
    match msg {
        GossipMessage::Gossip { key, value, term, source: _ } => {
            let mut st = state.lock().await;
            if st.dedup(&key, term) {
                st.apply(&key, &value, term).await;
                debug!("Gossip: applied {}", key);
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
            let count = entries.len();
            let response = GossipMessage::SyncFull { request_id, entries };
            if let Ok(json) = serde_json::to_string(&response) {
                let _ = ws.send(Message::Text(json.into())).await;
            }
            info!("Sent sync_full ({} entries)", count);
        }
        GossipMessage::SyncFull { ref entries, .. } => {
            let mut st = state.lock().await;
            for entry in entries.iter() {
                st.apply(&entry.key, &entry.value, entry.term);
                st.seen.insert(entry.key.clone(), entry.term);
            }
        }
        GossipMessage::Heartbeat => {
            let _ = ws.send(Message::Text(serde_json::to_string(&GossipMessage::Heartbeat).unwrap().into())).await;
        }
        _ => {}
    }
}

/// Connect to a peer and run the gossip loop.
pub async fn run_gossip_client(
    peer_name: String,
    peer_url: String,
    state: Arc<tokio::sync::Mutex<GossipState>>,
) {
    loop {
        match tokio_tungstenite::connect_async(&peer_url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to gossip peer {} at {}", peer_name, peer_url);
                let (mut write, mut read) = ws_stream.split();

                let msg = GossipMessage::SyncRequest { request_id: 0 };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = write.send(tokio_tungstenite::tungstenite::Message::Text(json.into())).await;
                }

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    if let Ok(gmsg) = serde_json::from_str::<GossipMessage>(&text) {
                                        let mut st = state.lock().await;
                                        match gmsg {
                                            GossipMessage::Gossip { key, value, term, source: _ } => {
                                                if st.dedup(&key, term) {
                                                    st.apply(&key, &value, term).await;
                                                }
                                            }
                                            GossipMessage::SyncFull { ref entries, .. } => {
                                                for entry in entries.iter() {
                                                    st.apply(&entry.key, &entry.value, entry.term);
                                                    st.seen.insert(entry.key.clone(), entry.term);
                                                }
                                            }
                                            GossipMessage::SyncRequest { request_id } => {
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
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to peer {} ({}): {}. Retrying in 5s...", peer_name, peer_url, e);
                sleep(Duration::from_secs(5)).await;
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}

