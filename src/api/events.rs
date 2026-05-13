use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse},
};
use serde_json::json;
use axum::Json;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use crate::daemon::DaemonState;

pub async fn sse_handler(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        result.ok().and_then(|ev| {
            serde_json::to_string(&ev).ok().map(|data| Ok::<_, std::convert::Infallible>(Event::default().data(data)))
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}
