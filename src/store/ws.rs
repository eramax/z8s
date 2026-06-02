use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::store::gossip::{GossipMessage, GossipState, SyncEntry};
use crate::store::gossip_apply::apply_incoming_batch;
use crate::store::hub::StoreEventHub;

fn gossip_connect_url(base: &str, node_name: &str) -> String {
    if base.contains('?') {
        format!("{base}&node_name={node_name}")
    } else {
        format!("{base}?node_name={node_name}")
    }
}

pub fn build_gossip_request(
    url: &str,
    token: Option<&str>,
    node_name: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, tokio_tungstenite::tungstenite::Error> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderValue};

    let full_url = gossip_connect_url(url, node_name);
    let mut req = full_url.as_str().into_client_request()?;
    if let Some(t) = token {
        let value = HeaderValue::from_str(&format!("Bearer {t}"))
            .expect("join token must be valid header value");
        req.headers_mut().insert(AUTHORIZATION, value);
    }
    Ok(req)
}

/// Handle an incoming WebSocket connection from a peer.
pub async fn handle_gossip_ws(
    ws: WebSocket,
    state: Arc<tokio::sync::Mutex<GossipState>>,
    store_events: StoreEventHub,
    notify: Arc<Notify>,
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
                    handle_message(msg, &msg_tx, &state, &store_events, &notify).await;
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
    store_events: &StoreEventHub,
    notify: &Arc<Notify>,
) {
    match msg {
        GossipMessage::Gossip {
            key,
            value,
            term,
            source: _,
        } => {
            let (should_apply, db) = {
                let mut st = state.lock().await;
                (st.dedup(&key, term), st.db.clone())
            };
            if should_apply {
                if let Ok(resource) = serde_json::from_slice::<crate::store::AnyResource>(&value) {
                    apply_incoming_batch(&db, store_events, notify, vec![resource]).await;
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
            let (db, to_apply) = {
                let mut st = state.lock().await;
                let db = st.db.clone();
                let mut to_apply = Vec::with_capacity(entries.len());
                for entry in entries {
                    if !st.dedup(&entry.key, entry.term) {
                        continue;
                    }
                    if let Ok(resource) =
                        serde_json::from_slice::<crate::store::AnyResource>(&entry.value)
                    {
                        to_apply.push(resource);
                    }
                }
                (db, to_apply)
            };
            apply_incoming_batch(&db, store_events, notify, to_apply).await;
        }
        GossipMessage::Heartbeat => {
            let _ = tx.send(Message::Text(
                serde_json::to_string(&GossipMessage::Heartbeat)
                    .unwrap()
                    .into(),
            ));
        }
        GossipMessage::BatchGossip { entries, source: _ } => {
            let (db, to_apply) = {
                let mut st = state.lock().await;
                let db = st.db.clone();
                let mut to_apply = Vec::with_capacity(entries.len());
                for entry in entries {
                    if !st.dedup(&entry.key, entry.term) {
                        continue;
                    }
                    if let Ok(resource) =
                        serde_json::from_slice::<crate::store::AnyResource>(&entry.value)
                    {
                        to_apply.push(resource);
                    }
                }
                (db, to_apply)
            };
            apply_incoming_batch(&db, store_events, notify, to_apply).await;
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
    store_events: StoreEventHub,
    notify: Arc<Notify>,
    join_token: Option<String>,
    node_name: String,
) {
    loop {
        let connect = build_gossip_request(&peer_url, join_token.as_deref(), &node_name);
        // Build a TLS connector that accepts self-signed certs (for z8s auto-generated certs)
        let ws_connector = {
            let nc = native_tls::TlsConnector::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .expect("Failed to build TLS connector");
            tokio_tungstenite::Connector::NativeTls(nc)
        };
        match connect {
            Ok(req) => match tokio_tungstenite::connect_async_tls_with_config(
                req,
                None,
                false,
                Some(ws_connector),
            ).await {
                Ok((ws_stream, _)) => {
                    handle_connected_client(
                        ws_stream,
                        &peer_name,
                        &state,
                        &store_events,
                        &notify,
                        &mut broadcast_rx,
                    )
                    .await;
                }
                Err(e) => {
                    warn!(
                        "Failed to connect to peer {} ({}): {}. Retrying in 5s...",
                        peer_name, peer_url, e
                    );
                    sleep(Duration::from_secs(5)).await;
                }
            },
            Err(e) => {
                warn!("Invalid gossip request for {}: {}", peer_url, e);
                sleep(Duration::from_secs(5)).await;
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}

async fn handle_connected_client(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    peer_name: &str,
    state: &Arc<tokio::sync::Mutex<GossipState>>,
    store_events: &StoreEventHub,
    notify: &Arc<Notify>,
    broadcast_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) {
    info!("Connected to gossip peer {} ", peer_name);
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
                            handle_client_gossip(
                                gmsg,
                                state,
                                store_events,
                                notify,
                                &mut write,
                            ).await;
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

async fn handle_client_gossip(
    gmsg: GossipMessage,
    state: &Arc<tokio::sync::Mutex<GossipState>>,
    store_events: &StoreEventHub,
    notify: &Arc<Notify>,
    write: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        tokio_tungstenite::tungstenite::Message,
    >,
) {
    match gmsg {
        GossipMessage::Gossip { key, value, term, source: _ } => {
            let (should_apply, db) = {
                let mut st = state.lock().await;
                (st.dedup(&key, term), st.db.clone())
            };
            if should_apply {
                if let Ok(resource) = serde_json::from_slice::<crate::store::AnyResource>(&value) {
                    apply_incoming_batch(&db, store_events, notify, vec![resource]).await;
                }
            }
        }
        GossipMessage::SyncFull { ref entries, .. } => {
            let (db, to_apply) = {
                let mut st = state.lock().await;
                let db = st.db.clone();
                let mut to_apply = Vec::with_capacity(entries.len());
                for entry in entries {
                    if !st.dedup(&entry.key, entry.term) {
                        continue;
                    }
                    if let Ok(resource) =
                        serde_json::from_slice::<crate::store::AnyResource>(&entry.value)
                    {
                        to_apply.push(resource);
                    }
                }
                (db, to_apply)
            };
            apply_incoming_batch(&db, store_events, notify, to_apply).await;
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
            let response = GossipMessage::SyncFull {
                request_id,
                entries,
            };
            if let Ok(json) = serde_json::to_string(&response) {
                let _ = write
                    .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
                    .await;
            }
        }
        GossipMessage::BatchGossip { entries, source: _ } => {
            let (db, to_apply) = {
                let mut st = state.lock().await;
                let db = st.db.clone();
                let mut to_apply = Vec::with_capacity(entries.len());
                for entry in entries {
                    if !st.dedup(&entry.key, entry.term) {
                        continue;
                    }
                    if let Ok(resource) =
                        serde_json::from_slice::<crate::store::AnyResource>(&entry.value)
                    {
                        to_apply.push(resource);
                    }
                }
                (db, to_apply)
            };
            apply_incoming_batch(&db, store_events, notify, to_apply).await;
        }
        _ => {}
    }
}
