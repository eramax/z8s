use std::sync::Arc;

use crate::store::StoreBackend;
use crate::types::{AnyResource, Event, ObjectMeta, ObjectReference, Time};

const MAX_EVENTS: usize = 1000;

/// Record an event in the store.
pub async fn record_event(
    store: &Arc<dyn StoreBackend>,
    name: &str,
    namespace: &str,
    kind: &str,
    obj_name: &str,
    reason: &str,
    message: &str,
    event_type: &str,
) {
    let now = Time::now();
    let event = Event {
        api_version: "v1".into(),
        kind: "Event".into(),
        action: None,
        count: Some(1),
        event_time: None,
        first_timestamp: Some(now.clone()),
        involved_object: ObjectReference {
            api_version: None,
            field_path: None,
            kind: Some(kind.into()),
            name: Some(obj_name.into()),
            namespace: Some(namespace.into()),
            resource_version: None,
            uid: None,
        },
        last_timestamp: Some(now.clone()),
        message: Some(message.into()),
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some(namespace.into()),
            creation_timestamp: Some(now),
            ..Default::default()
        },
        reason: Some(reason.into()),
        related: None,
        reporting_component: Some("z8s".into()),
        reporting_instance: None,
        series: None,
        source: None,
        type_: Some(event_type.into()),
    };

    store.apply(AnyResource::Event(event)).await.ok();
    prune_events(store).await;
}

/// Remove oldest events when over limit.
async fn prune_events(store: &Arc<dyn StoreBackend>) {
    let mut all = store.get_by_kind("Event").await;
    if all.len() <= MAX_EVENTS {
        return;
    }
    all.sort_by(|a, b| a.last_updated.cmp(&b.last_updated));
    let to_remove: Vec<_> = all.drain(..all.len() - MAX_EVENTS).collect();
    for t in to_remove {
        store.delete(&t.resource).await.ok();
    }
}
