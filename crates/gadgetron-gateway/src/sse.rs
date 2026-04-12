use axum::response::sse::{Event, KeepAlive, Sse};
use futures::{Stream, StreamExt};
use gadgetron_core::error::GadgetronError;
use gadgetron_core::provider::ChatChunk;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{convert::Infallible, time::Duration};

/// SSE KeepAlive interval — 15 seconds.
///
/// Rationale: most LLM inference delivers the first token in < 10 s.
/// 15 s keeps the connection alive through typical proxy/load-balancer
/// idle-connection timeouts without generating unnecessary traffic.
const SSE_KEEPALIVE_SECS: u64 = 15;

/// Convert a `ChatChunk` stream into an OpenAI-compatible SSE response.
///
/// Each `Ok(chunk)` is serialised to JSON and emitted as `data: {json}\n\n`.
///
/// On any `Err(e)`:
///   - An `event: error` SSE frame is emitted with the opaque three-field
///     error payload (`message`, `type`, `code`).
///   - The stream terminates after that frame.
///   - `data: [DONE]` is NOT emitted (per P3 decision: no [DONE] after error).
///
/// On normal stream completion (no error):
///   - `data: [DONE]\n\n` is appended as the final frame (OpenAI compatible).
///
/// A 15-second `KeepAlive` comment frame prevents proxy/LB idle timeouts.
///
/// # Implementation note
///
/// We use an `Arc<AtomicBool>` flag (`saw_error`) shared between the `.map()`
/// closure and a trailing conditional item produced via `flat_map` + `chain`.
/// The pipeline:
///
///   stream
///     .flat_map(|item| {
///         // Converts each item to 0 or more SSE events.
///         // On error: sets `saw_error = true`, yields the error event.
///         // On success: yields the chunk event.
///     })
///     .chain(stream::once(async {
///         // Yields `data: [DONE]` only when !saw_error.
///         // Wrapped in Option so we can produce 0 items on the error path
///         // by returning None and using .filter_map(|x| x) afterwards.
///     }))
///     .filter_map(identity) // removes the None from error path
pub fn chat_chunk_to_sse<S>(stream: S) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send>
where
    S: Stream<Item = Result<ChatChunk, GadgetronError>> + Send + 'static,
{
    let saw_error: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let saw_error_for_chain = saw_error.clone();

    // flat_map each chunk item into 0 or 1 SSE events (as Option<Result<Event,_>>).
    // We need the outer stream type to be `Option<Result<Event, Infallible>>`
    // throughout so `chain` can attach the conditional [DONE] item.
    let event_stream = stream
        .flat_map(move |result| {
            let event_opt: Option<Result<Event, Infallible>> = match result {
                Ok(chunk) => {
                    if saw_error.load(Ordering::Relaxed) {
                        // Defensive: suppress further items after an error.
                        None
                    } else {
                        let data = serde_json::to_string(&chunk).unwrap_or_else(|e| {
                            format!("{{\"error\":\"serialization failed: {}\"}}", e)
                        });
                        Some(Ok(Event::default().data(data)))
                    }
                }
                Err(e) => {
                    saw_error.store(true, Ordering::Relaxed);
                    tracing::error!(
                        error.code = e.error_code(),
                        error.type_ = e.error_type(),
                        "sse stream error: {}",
                        e
                    );
                    let data = serde_json::json!({
                        "error": {
                            "message": e.error_message(),
                            "type":    e.error_type(),
                            "code":    e.error_code(),
                        }
                    })
                    .to_string();
                    Some(Ok(Event::default().event("error").data(data)))
                }
            };
            futures::stream::iter(event_opt)
        })
        // Append the conditional [DONE] terminator.
        // We chain a single `Option<Result<Event, Infallible>>` item and then
        // use filter_map to remove the None case.
        .map(Some) // lift every item to Some(Ok(event))
        .chain(futures::stream::once(async move {
            // Emit [DONE] only on the normal (no-error) path.
            if saw_error_for_chain.load(Ordering::Relaxed) {
                None
            } else {
                Some(Ok::<Event, Infallible>(Event::default().data("[DONE]")))
            }
        }))
        .filter_map(|item| async move { item });

    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(SSE_KEEPALIVE_SECS)))
}
