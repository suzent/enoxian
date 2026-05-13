use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use crate::state::AppState;

pub async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        result.ok().and_then(|ev| {
            serde_json::to_string(&ev).ok().map(|data| {
                Ok(Event::default().data(data))
            })
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
