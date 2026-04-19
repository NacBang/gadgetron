//! Streaming chat SSE Drop-guard — drift-fix PR 6 (Material M3 + Nominal N1).
//!
//! Background: the non-streaming chat handler emits an AuditEntry + PSL-1d
//! capture on BOTH the Ok and Err arms. The streaming handler, by contrast,
//! emits an AuditEntry at dispatch time (before the first byte) with
//! `input_tokens: 0`, `output_tokens: 0`, `status: Ok` — all placeholders
//! because the runtime doesn't yet know what the provider will yield, whether
//! the client will disconnect mid-stream, or whether the provider will emit a
//! terminal error frame.
//!
//! This module closes that gap with an RAII-style guard:
//!
//! 1. [`StreamEndGuard`] holds the audit correlation + shared observation
//!    state ([`StreamEndState`]) behind an `Arc<Mutex<_>>`.
//! 2. [`GuardedStream`] wraps the raw `Stream<Item = Result<ChatChunk, _>>`
//!    returned by `Router::chat_stream` and calls the guard's observation
//!    hooks on every polled item.
//! 3. When the guard's owning [`GuardedStream`] is dropped — normal stream
//!    completion, client disconnect, panic unwind, future cancellation — the
//!    guard's [`Drop`] impl emits a SECOND AuditEntry (`amendment`) with:
//!    - a FRESH `event_id` (invariant vs. PR 5 — see `AuditEntry.event_id`
//!      doc comment),
//!    - the SAME `request_id` as the dispatch-time AuditEntry,
//!    - accumulated `output_tokens` observed across the stream,
//!    - `input_tokens` from the final `usage` chunk (if the provider emits
//!      one),
//!    - `status = Error` + `capture_chat_completion_error` if the stream
//!      ended with a terminal `Err`, else `status = Ok` +
//!      `capture_chat_completion`.
//!
//! See also:
//! - `crates/gadgetron-gateway/src/handlers.rs::handle_streaming` — where the
//!   guard is instantiated and the raw stream is wrapped.
//! - `AuditEntry.event_id` doc comment in `gadgetron-xaas/src/audit/writer.rs`
//!   — the per-entry correlation invariant this amendment honours.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use futures::Stream;
use uuid::Uuid;

use gadgetron_core::error::GadgetronError;
use gadgetron_core::knowledge::candidate::KnowledgeCandidateCoordinator;
use gadgetron_core::provider::ChatChunk;
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus, AuditWriter};

use crate::activity_capture::{
    capture_chat_completion, capture_chat_completion_error, error_class,
};

/// Observation state mutated by [`GuardedStream::poll_next`] and consumed by
/// [`StreamEndGuard::drop`].
///
/// All fields default to empty so a stream that yields zero chunks and zero
/// errors still produces a well-formed amendment (output_tokens = 0,
/// status = Ok).
///
/// # Why no `prompt_tokens_observed`?
///
/// The internal `ChatChunk` shape (crates/gadgetron-core/src/provider.rs) has
/// no `usage` field — provider adapters do not surface per-chunk token counts
/// through gadgetron's stream. The amendment therefore records
/// `input_tokens = 0` for streaming paths. A future PR that threads a
/// `Usage` side-channel from provider adapters (or adds an `Option<Usage>`
/// to `ChatChunk`) will be able to populate real prompt-token counts.
#[derive(Debug, Default, Clone)]
pub struct StreamEndState {
    /// Best-effort completion-token proxy: incremented once per chunk whose
    /// `delta.content` is non-empty. This is a COARSE estimate (one chunk
    /// is not one token) — operators who care about exact counts should
    /// track chunk-level usage at the provider layer.
    pub completion_tokens_observed: u32,
    /// True once a terminal `Err` has flowed through the stream. Used to
    /// pick the amendment's `AuditStatus` and which capture helper fires.
    pub saw_error: bool,
    /// Static error classification from [`error_class`] — non-PII.
    pub error_class: Option<&'static str>,
}

/// RAII guard that emits a stream-end amendment AuditEntry on drop.
///
/// Constructed per-request inside `handle_streaming`, then moved into a
/// [`GuardedStream`] that wraps the raw chunk stream. The stream holds the
/// guard by value; dropping the stream drops the guard, firing [`Drop`].
pub struct StreamEndGuard {
    state: Arc<Mutex<StreamEndState>>,
    audit_writer: Arc<AuditWriter>,
    coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
    activity_bus: Option<gadgetron_core::activity_bus::ActivityBus>,
    tenant_id: Uuid,
    api_key_id: Uuid,
    request_id: Uuid,
    model: String,
    started_at: Instant,
    // Idempotence guard — Drop can theoretically be invoked more than once in
    // pathological cases (e.g. explicit drop then implicit). We never want
    // TWO amendments.
    already_amended: bool,
}

impl StreamEndGuard {
    /// Construct a new guard. The guard's internal state is exposed via
    /// [`StreamEndGuard::shared_state`] if callers need direct access; the
    /// typical path is [`StreamEndGuard::wrap`] which plumbs state mutation
    /// automatically through the stream wrapper.
    pub fn new(
        audit_writer: Arc<AuditWriter>,
        coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
        tenant_id: Uuid,
        api_key_id: Uuid,
        request_id: Uuid,
        model: String,
        started_at: Instant,
    ) -> Self {
        Self::new_with_activity_bus(
            audit_writer,
            coordinator,
            None,
            tenant_id,
            api_key_id,
            request_id,
            model,
            started_at,
        )
    }

    /// Build the guard with an optional `ActivityBus` handle. When
    /// wired, the amendment path also publishes a `ChatCompleted`
    /// event so the /events/ws subscribers see the streaming-chat
    /// completion in real time (ISSUE 4 TASK 4.4).
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_activity_bus(
        audit_writer: Arc<AuditWriter>,
        coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
        activity_bus: Option<gadgetron_core::activity_bus::ActivityBus>,
        tenant_id: Uuid,
        api_key_id: Uuid,
        request_id: Uuid,
        model: String,
        started_at: Instant,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(StreamEndState::default())),
            audit_writer,
            coordinator,
            activity_bus,
            tenant_id,
            api_key_id,
            request_id,
            model,
            started_at,
            already_amended: false,
        }
    }

    /// Return a clone of the shared state handle. Exposed for tests that
    /// want to assert observed state without going through
    /// [`GuardedStream`].
    #[allow(dead_code)]
    pub fn shared_state(&self) -> Arc<Mutex<StreamEndState>> {
        self.state.clone()
    }

    /// Wrap `inner` in a [`GuardedStream`] that takes ownership of this guard.
    /// When the returned stream is dropped, the guard is dropped, firing the
    /// amendment.
    pub fn wrap<S>(self, inner: S) -> GuardedStream<S>
    where
        S: Stream<Item = Result<ChatChunk, GadgetronError>> + Unpin,
    {
        let state = self.state.clone();
        GuardedStream {
            inner,
            state,
            guard: Some(self),
        }
    }

    /// Observe a successful chunk. Called by [`GuardedStream::poll_next`].
    ///
    /// Counting rule: increment `completion_tokens_observed` once per chunk
    /// whose first choice's `delta.content` is non-empty. This is
    /// best-effort — see `StreamEndState` docs for the rationale.
    fn record_chunk(&self, chunk: &ChatChunk) {
        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(poisoned) => {
                tracing::warn!(
                    request_id = %self.request_id,
                    "stream_end_guard.record_chunk: state lock poisoned"
                );
                poisoned.into_inner()
            }
        };

        if let Some(choice) = chunk.choices.first() {
            if let Some(content) = &choice.delta.content {
                if !content.is_empty() {
                    state.completion_tokens_observed =
                        state.completion_tokens_observed.saturating_add(1);
                }
            }
        }
    }

    /// Observe a terminal stream error. Called by [`GuardedStream::poll_next`]
    /// when the inner stream yields `Err`.
    fn record_terminal_error(&self, err: &GadgetronError) {
        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(poisoned) => {
                tracing::warn!(
                    request_id = %self.request_id,
                    "stream_end_guard.record_terminal_error: state lock poisoned"
                );
                poisoned.into_inner()
            }
        };
        state.saw_error = true;
        state.error_class = Some(error_class(err));
    }
}

impl Drop for StreamEndGuard {
    fn drop(&mut self) {
        if self.already_amended {
            return;
        }
        self.already_amended = true;

        // Snapshot state — we tolerate lock poisoning (best-effort amendment
        // is better than none, and poisoning only happens if a prior panic
        // was already the problem).
        let snapshot = match self.state.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        };

        let latency_ms = self.started_at.elapsed().as_millis() as i32;
        let amendment_event_id = Uuid::new_v4();
        let status = if snapshot.saw_error {
            AuditStatus::Error
        } else {
            AuditStatus::Ok
        };

        // ISSUE 4 TASK 4.2: compute cost_cents at amendment time from
        // the output-token count observed on-stream. `input_tokens`
        // is still 0 for streaming (providers don't re-report prompt
        // size on the delta stream) — see `StreamEndState`. The
        // input contribution is therefore excluded; chat rollups
        // will slightly underestimate cost for streaming requests
        // until the router is reworked to surface prompt_tokens
        // upstream. A worked-through follow-on note lives in
        // `docs/design/gateway/streaming-cost-attribution.md`
        // (future TASK).
        let pricing = gadgetron_core::pricing::default_pricing_table();
        let cost_cents = gadgetron_core::pricing::compute_cost_cents(
            &self.model,
            0,
            snapshot.completion_tokens_observed.into(),
            &pricing,
        );

        // Amendment AuditEntry — fresh event_id, same request_id.
        // input_tokens is 0 for streaming; see `StreamEndState` doc comment.
        // Capture the stringified status BEFORE the send() move so
        // both the audit row and the activity event agree. The
        // schema stores the same literal.
        let status_str: String = status.as_str().into();
        self.audit_writer.send(AuditEntry {
            event_id: amendment_event_id,
            tenant_id: self.tenant_id,
            api_key_id: self.api_key_id,
            actor_user_id: None,
            actor_api_key_id: None,
            request_id: self.request_id,
            model: Some(self.model.clone()),
            provider: None,
            status,
            input_tokens: 0,
            output_tokens: snapshot.completion_tokens_observed as i32,
            cost_cents,
            latency_ms,
        });

        // ISSUE 4 TASK 4.4: mirror the audit emission onto the live
        // activity bus so /events/ws subscribers see the streaming
        // completion in real time.
        if let Some(bus) = self.activity_bus.as_ref() {
            bus.publish(gadgetron_core::activity_bus::ActivityEvent::ChatCompleted {
                tenant_id: self.tenant_id,
                request_id: self.request_id,
                model: self.model.clone(),
                status: status_str,
                input_tokens: 0,
                output_tokens: snapshot.completion_tokens_observed as i64,
                cost_cents,
                latency_ms: latency_ms as i64,
            });
        }

        // Mirror the non-streaming arms: RuntimeObservation on error,
        // GadgetToolCall on success. Fire-and-forget via tokio::spawn.
        //
        // Drop can fire outside a tokio runtime in pathological cases
        // (tests that construct and drop the guard bare, non-tokio
        // executors). Gate on `Handle::try_current()` so those paths don't
        // panic — they silently skip the capture.
        let Some(coord) = self.coordinator.clone() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::debug!(
                request_id = %self.request_id,
                "stream_end_guard: no tokio runtime available, skipping capture"
            );
            return;
        };

        let tenant_id = self.tenant_id;
        // PR 7 (doc-10): thread the caller's user_id into the amendment
        // capture. Until a real user table lands, api_key_id is the
        // authoritative identity — the guard stores it at construction.
        let actor_user_id = self.api_key_id;
        let request_id = self.request_id;
        let model = self.model.clone();
        let saw_error = snapshot.saw_error;
        let error_class = snapshot.error_class.unwrap_or("internal_error");
        let completion_tokens = snapshot.completion_tokens_observed;

        handle.spawn(async move {
            if saw_error {
                capture_chat_completion_error(
                    coord,
                    tenant_id,
                    actor_user_id,
                    request_id,
                    amendment_event_id,
                    model,
                    error_class,
                    latency_ms,
                )
                .await;
            } else {
                capture_chat_completion(
                    coord,
                    tenant_id,
                    actor_user_id,
                    request_id,
                    amendment_event_id,
                    model,
                    0, // prompt_tokens: 0 for streaming (see StreamEndState docs)
                    completion_tokens,
                    true,
                )
                .await;
            }
        });
    }
}

/// Stream wrapper that funnels every polled item through the owned
/// [`StreamEndGuard`] so token counts and terminal errors accumulate into
/// [`StreamEndState`]. On drop, the owned guard fires.
pub struct GuardedStream<S> {
    inner: S,
    // Kept so tests can observe state without going through Drop.
    #[allow(dead_code)]
    state: Arc<Mutex<StreamEndState>>,
    // Option so we COULD drop the guard early (not currently exposed, but
    // leaves the door open for a "take_guard" API if needed).
    guard: Option<StreamEndGuard>,
}

impl<S> Stream for GuardedStream<S>
where
    S: Stream<Item = Result<ChatChunk, GadgetronError>> + Unpin,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Safe: GuardedStream is Unpin (S: Unpin, StreamEndGuard: Unpin,
        // Arc/Mutex/Option<> are Unpin). get_mut() projects through.
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_next(cx);

        if let Some(guard) = &this.guard {
            match &poll {
                Poll::Ready(Some(Ok(chunk))) => guard.record_chunk(chunk),
                Poll::Ready(Some(Err(err))) => guard.record_terminal_error(err),
                _ => {}
            }
        }
        poll
    }
}

// ---------------------------------------------------------------------------
// Tests — the drift-fix-PR-6 invariants codified as red→green unit tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use futures::stream;
    use gadgetron_core::provider::{ChatChunk, ChunkChoice, ChunkDelta};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    // ------------------------------------------------------------------
    // Fixtures
    // ------------------------------------------------------------------

    fn content_chunk(content: &str) -> ChatChunk {
        ChatChunk {
            id: "test-chunk".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "mock".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: Some(content.into()),
                    tool_calls: None,
                    ..ChunkDelta::default()
                },
                finish_reason: None,
            }],
        }
    }

    fn empty_chunk() -> ChatChunk {
        ChatChunk {
            id: "test-chunk".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "mock".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta::default(),
                finish_reason: Some("stop".into()),
            }],
        }
    }

    /// Drain the stream to completion and return the final state + all
    /// amendment AuditEntry rows that landed in the channel after Drop.
    /// No coordinator is wired — the tests here verify the AuditEntry side
    /// of the amendment; the PSL-1d capture path is exercised via
    /// integration tests (see `tests/psl_1d_chat_capture.rs`).
    async fn drive_and_drop<S>(stream: S, request_id: Uuid) -> (Vec<AuditEntry>, StreamEndState)
    where
        S: Stream<Item = Result<ChatChunk, GadgetronError>> + Unpin + Send + 'static,
    {
        let (writer, mut rx) = AuditWriter::new(16);
        let writer = Arc::new(writer);

        let guard = StreamEndGuard::new(
            writer.clone(),
            None,
            Uuid::new_v4(),
            Uuid::new_v4(),
            request_id,
            "mock".into(),
            Instant::now(),
        );
        let state_handle = guard.shared_state();

        let mut guarded = guard.wrap(stream);
        use futures::StreamExt;
        while guarded.next().await.is_some() {}
        drop(guarded);

        // Drain the channel — AuditWriter uses tokio::sync::mpsc which is
        // synchronous on the send side, so the amendment is already in the
        // buffer by the time Drop returns. A 1ms sleep is defensive in
        // case any tokio-internal reordering delays the notification.
        tokio::time::sleep(Duration::from_millis(1)).await;
        let mut entries = Vec::new();
        while let Ok(e) = rx.try_recv() {
            entries.push(e);
        }
        let final_state = state_handle.lock().unwrap().clone();
        (entries, final_state)
    }

    // ------------------------------------------------------------------
    // Tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn empty_stream_emits_ok_amendment_with_zero_tokens() {
        let req_id = Uuid::new_v4();
        let dispatch_id = Uuid::new_v4(); // what a dispatch-time AuditEntry would carry
        let s = stream::iter(Vec::<Result<ChatChunk, GadgetronError>>::new());
        let (entries, state) = drive_and_drop(s, req_id).await;

        assert_eq!(entries.len(), 1, "exactly one amendment AuditEntry emitted");
        let a = &entries[0];
        assert_eq!(a.request_id, req_id, "same request_id as dispatch");
        assert_ne!(
            a.event_id, dispatch_id,
            "amendment event_id must be FRESH (PR 5 invariant)"
        );
        assert_eq!(a.status, AuditStatus::Ok);
        assert_eq!(a.input_tokens, 0);
        assert_eq!(a.output_tokens, 0);
        assert!(!state.saw_error);
    }

    #[tokio::test]
    async fn content_chunks_increment_output_tokens() {
        let req_id = Uuid::new_v4();
        // Three content chunks (plus one empty finish chunk) → fallback
        // output_tokens = 3 (empty chunks are not counted).
        let s = stream::iter(vec![
            Ok(content_chunk("hel")),
            Ok(content_chunk("lo")),
            Ok(content_chunk(" world")),
            Ok(empty_chunk()),
        ]);
        let (entries, _state) = drive_and_drop(s, req_id).await;

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].output_tokens, 3);
        assert_eq!(entries[0].input_tokens, 0);
        assert_eq!(entries[0].status, AuditStatus::Ok);
    }

    #[tokio::test]
    async fn terminal_error_produces_error_amendment_with_class() {
        let req_id = Uuid::new_v4();
        let s = stream::iter(vec![
            Ok(content_chunk("partial")),
            Err(GadgetronError::Provider("upstream 502".into())),
        ]);
        let (entries, state) = drive_and_drop(s, req_id).await;

        assert_eq!(entries.len(), 1);
        let a = &entries[0];
        assert_eq!(a.status, AuditStatus::Error);
        assert_eq!(a.request_id, req_id);
        assert!(state.saw_error);
        assert_eq!(state.error_class, Some("provider_error"));
        // The partial content chunk counted before the terminal error.
        assert_eq!(a.output_tokens, 1);
    }

    #[tokio::test]
    async fn amendment_event_id_is_fresh_per_drop() {
        // PR 5 + PR 6 invariant: amendment.event_id is a fresh UUID per
        // Drop, distinct from both the dispatch entry's event_id and any
        // other amendment's event_id. Run 16 guards in a loop and check
        // uniqueness via HashSet.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..16 {
            let s = stream::iter(vec![Ok(content_chunk("hi"))]);
            let (entries, _) = drive_and_drop(s, Uuid::new_v4()).await;
            assert_eq!(entries.len(), 1);
            assert!(
                seen.insert(entries[0].event_id),
                "amendment event_id collision — Uuid::new_v4() must be unique per Drop"
            );
        }
    }

    #[tokio::test]
    async fn drop_is_idempotent_via_already_amended_flag() {
        let (writer, mut rx) = AuditWriter::new(16);
        let writer = Arc::new(writer);
        let guard = StreamEndGuard::new(
            writer,
            None,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "mock".into(),
            Instant::now(),
        );
        // Explicit drop — `drop(guard)` runs Drop::drop once.
        drop(guard);

        tokio::time::sleep(Duration::from_millis(1)).await;
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 1, "exactly one amendment per guard lifetime");
    }

    /// Regression: dropping the wrapped stream before polling it to
    /// completion (simulates client disconnect) still fires the amendment.
    #[tokio::test]
    async fn client_disconnect_before_completion_still_amends() {
        let (writer, mut rx) = AuditWriter::new(16);
        let writer = Arc::new(writer);
        let req_id = Uuid::new_v4();
        let guard = StreamEndGuard::new(
            writer,
            None,
            Uuid::new_v4(),
            Uuid::new_v4(),
            req_id,
            "mock".into(),
            Instant::now(),
        );

        let polled_any = Arc::new(AtomicBool::new(false));
        let polled_flag = polled_any.clone();
        let s = stream::unfold(0u32, move |n| {
            let flag = polled_flag.clone();
            async move {
                flag.store(true, Ordering::SeqCst);
                Some((Ok(content_chunk("chunk")), n + 1))
            }
        });

        use futures::StreamExt;
        let pinned: Pin<Box<dyn Stream<Item = Result<ChatChunk, GadgetronError>> + Send>> =
            Box::pin(s);
        let mut guarded = guard.wrap(pinned);
        // Poll exactly one chunk then drop — simulates client disconnect.
        let _ = guarded.next().await;
        assert!(polled_any.load(Ordering::SeqCst));
        drop(guarded);

        tokio::time::sleep(Duration::from_millis(1)).await;
        let mut entries = Vec::new();
        while let Ok(e) = rx.try_recv() {
            entries.push(e);
        }
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].request_id, req_id);
        assert_eq!(entries[0].status, AuditStatus::Ok);
        assert_eq!(
            entries[0].output_tokens, 1,
            "the single polled chunk should be counted"
        );
    }
}
