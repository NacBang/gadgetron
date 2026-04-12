use axum::response::sse::{Event, Sse};
use futures::{Stream, StreamExt};
use std::convert::Infallible;

/// Convert a stream of ChatChunks into SSE events for the OpenAI-compatible API.
pub fn chat_chunk_to_sse(
    stream: impl Stream<Item = gadgetron_core::error::Result<gadgetron_core::provider::ChatChunk>>
        + Send
        + 'static,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send> {
    let event_stream = stream.map(
        |result: gadgetron_core::error::Result<gadgetron_core::provider::ChatChunk>| match result {
            Ok(chunk) => {
                let data = serde_json::to_string(&chunk).unwrap_or_default();
                Ok(Event::default().data(data))
            }
            Err(e) => {
                let data = serde_json::json!({"error": e.to_string()}).to_string();
                Ok(Event::default().data(data))
            }
        },
    );

    Sse::new(event_stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}
