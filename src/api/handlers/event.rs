use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_events_all(State(state): State<AppState>) -> Json<List<Event>> {
    let trackers = state.store.get_by_kind("Event").await;
    let items: Vec<Event> = trackers
        .into_iter()
        .filter_map(|t| match t.resource { AnyResource::Event(e) => Some(e), _ => None })
        .collect();
    Json(List {
        kind: Some("EventList".into()),
        api_version: None,
        items,
        metadata: make_list_meta(),
    })
}

pub async fn list_events(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<Event>> {
    let trackers = state.store.get_by_kind("Event").await;
    let items: Vec<Event> = trackers
        .into_iter()
        .filter_map(|t| match t.resource {
            AnyResource::Event(e) if e.metadata.namespace.as_deref() == Some(&namespace) => Some(e),
            _ => None,
        })
        .collect();
    Json(List {
        kind: Some("EventList".into()),
        api_version: None,
        items,
        metadata: make_list_meta(),
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/events", get(list_events_all))
        .route("/api/v1/namespaces/{namespace}/events", get(list_events))
}
