//! Kubernetes-style watch streams backed by `StoreEventHub` (A3).

use std::convert::Infallible;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::response::Response;
use serde::Serialize;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;

use crate::api::catalog::ResourceEntry;
use crate::api::compat::{self, WireContext};
use crate::api::server::AppState;
use crate::store::ops::{StoreChange, StoreEvent};

#[derive(Serialize)]
struct WatchEvent {
    #[serde(rename = "type")]
    event_type: String,
    object: serde_json::Value,
}

fn ns_match(namespace: &Option<String>, resource: &crate::store::AnyResource) -> bool {
    namespace
        .as_ref()
        .map_or(true, |ns| resource.namespace() == ns)
}

pub fn is_watch_request(query: Option<&str>) -> bool {
    let q = query.unwrap_or("");
    q.split('&').any(|p| p == "watch=1" || p == "watch=true")
}

/// Stream watch events for a catalog entry (cluster- or namespace-scoped list).
pub async fn watch_list(
    state: AppState,
    entry: &'static ResourceEntry,
    namespace: Option<String>,
    wire: WireContext,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, Infallible>>(64);
    let kind = entry.kind.to_string();
    let store = state.store.clone();
    let mut events_rx = state.store_events.subscribe();

    tokio::spawn(async move {
        async fn send_line(
            tx: &tokio::sync::mpsc::Sender<Result<Bytes, Infallible>>,
            typ: &str,
            resource: &crate::store::AnyResource,
            wire: &WireContext,
        ) -> bool {
            let value = serde_json::to_value(resource).unwrap_or_default();
            let object = compat::encode_resource_value(value, wire);
            let ev = WatchEvent {
                event_type: typ.to_string(),
                object,
            };
            let line = match serde_json::to_string(&ev) {
                Ok(s) => format!("{s}\n"),
                Err(_) => return true,
            };
            tx.send(Ok(Bytes::from(line))).await.is_ok()
        }

        for tracker in store.get_by_kind(&kind).await {
            if ns_match(&namespace, &tracker.resource) {
                if !send_line(&tx, "ADDED", &tracker.resource, &wire).await {
                    return;
                }
            }
        }

        let mut timeout = tokio::time::interval(Duration::from_secs(30));
        timeout.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                msg = events_rx.recv() => {
                    match msg {
                        Ok(StoreEvent::Applied { resource, change }) => {
                            if resource.kind() != kind {
                                continue;
                            }
                            if !ns_match(&namespace, &resource) {
                                continue;
                            }
                            let typ = match change {
                                StoreChange::Created => "ADDED",
                                StoreChange::Updated => "MODIFIED",
                                StoreChange::Deleted => "DELETED",
                            };
                            if !send_line(&tx, typ, &resource, &wire).await {
                                break;
                            }
                        }
                        Ok(StoreEvent::Deleted { resource }) => {
                            if resource.kind() != kind {
                                continue;
                            }
                            if !ns_match(&namespace, &resource) {
                                continue;
                            }
                            if !send_line(&tx, "DELETED", &resource, &wire).await {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            debug!("watch hub lagged for kind {}", kind);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = timeout.tick() => {
                    // Keep connection alive for kubectl watch
                    if tx.send(Ok(Bytes::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    Response::builder()
        .header("Content-Type", "application/json")
        .header("Transfer-Encoding", "chunked")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
