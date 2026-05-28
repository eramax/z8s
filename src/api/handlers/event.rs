use axum::Router;
use axum::routing::get;
use crate::api::server::*;

pub async fn list_events_all(State(state): State<AppState>) -> Json<List<Event>> {
    let ev = state.events.lock().await;
    Json(List {
        items: ev.clone(),
        metadata: make_list_meta(),
    })
}


pub async fn list_events(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Json<List<Event>> {
    let ev = state.events.lock().await;
    Json(List {
        items: ev
            .iter()
            .filter(|e| e.metadata.namespace.as_deref() == Some(&namespace))
            .cloned()
            .collect(),
        metadata: make_list_meta(),
    })
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/events", get(list_events_all))
        .route("/api/v1/namespaces/{namespace}/events", get(list_events))
}
