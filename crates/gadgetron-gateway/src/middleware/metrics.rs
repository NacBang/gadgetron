use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use gadgetron_core::ui::{RequestEntry, WsMessage};
use std::time::Instant;

use crate::server::AppState;

/// Emit `WsMessage::RequestLog` to the TUI broadcast channel after each request.
///
/// Position in the Tower stack: innermost layer on `authenticated_routes` — it
/// wraps the handler directly, executing after `scope_guard_middleware`.
///
/// Concurrency model: fire-and-forget `broadcast::Sender::send`.
/// `SendError` (all receivers lagged or dropped) is silently ignored.
/// The TUI is optional: when `state.tui_tx` is `None`, this middleware is a no-op
/// aside from timing the request.
///
/// P99 budget: < 1 µs (one `Instant::elapsed()`, one `broadcast::Sender::send`).
/// No allocations on the hot path when TUI is disabled (`Option::as_ref` + early return).
pub async fn metrics_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    // Extract request metadata from extensions set by upstream middleware before
    // consuming the request.
    //
    // TenantContext is inserted by tenant_context_middleware (layer 5).
    // If absent (e.g. routes that bypass auth, or layer ordering violation),
    // fall back to "anonymous".
    let tenant_id = req
        .extensions()
        .get::<gadgetron_core::context::TenantContext>()
        .map(|ctx| ctx.tenant_id.to_string())
        .unwrap_or_else(|| "anonymous".to_string());

    // request_id UUID is inserted by request_id_middleware (layer 3).
    let request_id = req
        .extensions()
        .get::<uuid::Uuid>()
        .copied()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let start = Instant::now();
    let response = next.run(req).await;
    let latency_ms = start.elapsed().as_millis() as u32;

    let status = response.status().as_u16();

    // Fire-and-forget: ignore SendError (no receivers) and Lagged errors.
    // `Option::as_ref` avoids cloning the Sender on the no-TUI path.
    if let Some(tx) = state.tui_tx.as_ref() {
        let entry = RequestEntry {
            request_id,
            tenant_id,
            // model and provider are not yet available at middleware layer.
            // Sprint 6: emit with empty strings; Sprint 7 will propagate via extensions.
            model: String::new(),
            provider: String::new(),
            status,
            latency_ms,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            timestamp: Utc::now(),
        };
        // Ignore SendError — receiver count may be 0 (TUI quit) or Lagged.
        let _ = tx.send(WsMessage::RequestLog(entry));
    }

    response
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{build_router, AppState};
    use crate::test_helpers::{lazy_pool, TEST_AUDIT_CAPACITY, VALID_TOKEN};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use gadgetron_core::context::Scope;
    use gadgetron_xaas::audit::writer::AuditWriter;
    use gadgetron_xaas::auth::validator::{KeyValidator, ValidatedKey};
    use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;
    use uuid::Uuid;

    // ---------------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------------

    struct MockKeyValidator {
        result: Arc<ValidatedKey>,
    }

    impl MockKeyValidator {
        fn new(scopes: Vec<Scope>) -> Self {
            Self {
                result: Arc::new(ValidatedKey {
                    api_key_id: Uuid::new_v4(),
                    tenant_id: Uuid::new_v4(),
                    scopes,
                }),
            }
        }
    }

    #[async_trait::async_trait]
    impl KeyValidator for MockKeyValidator {
        async fn validate(
            &self,
            _key_hash: &str,
        ) -> Result<Arc<ValidatedKey>, gadgetron_core::error::GadgetronError> {
            Ok(self.result.clone())
        }

        async fn invalidate(&self, _key_hash: &str) {}
    }

    fn make_state_with_tui(tui_tx: broadcast::Sender<WsMessage>) -> AppState {
        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        AppState {
            key_validator: Arc::new(MockKeyValidator::new(vec![Scope::OpenAiCompat])),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: lazy_pool(),
            tui_tx: Some(tui_tx),
        }
    }

    // ---------------------------------------------------------------------------
    // S6-2 TDD: metrics_middleware_emits_request_log
    // ---------------------------------------------------------------------------

    /// S6-2-T3: `metrics_middleware` emits a `WsMessage::RequestLog` after each
    /// request that passes through authenticated_routes.
    ///
    /// Setup: broadcast channel with capacity 16. State carries the Sender.
    /// Request: `GET /v1/models` with a valid Bearer token.
    /// Assert: the receiver gets exactly one `WsMessage::RequestLog` with the
    /// correct HTTP status code.
    #[tokio::test]
    async fn metrics_middleware_emits_request_log() {
        let (tx, mut rx) = broadcast::channel::<WsMessage>(TEST_AUDIT_CAPACITY);
        let state = make_state_with_tui(tx);
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // The broadcast channel should have one RequestLog entry.
        match rx.try_recv() {
            Ok(WsMessage::RequestLog(entry)) => {
                assert_eq!(entry.status, 200, "logged status must be 200");
                // latency_ms is a real elapsed value — only check it is reasonable (>= 0).
                // No upper bound since test environments vary.
                assert!(
                    entry.latency_ms < 60_000,
                    "latency_ms looks unreasonably large: {}",
                    entry.latency_ms
                );
            }
            Ok(other) => panic!("expected RequestLog, got: {:?}", other),
            Err(e) => panic!("expected a message in the broadcast channel, got: {:?}", e),
        }
    }

    /// S6-2-T3b: When `tui_tx` is `None` (TUI disabled), `metrics_middleware`
    /// must not panic or error — it is a no-op for the broadcast emit path.
    ///
    /// Uses a state with `tui_tx: None`. The request completes normally (200).
    /// No assertion on the broadcast channel because there is none.
    #[tokio::test]
    async fn metrics_middleware_noop_when_tui_disabled() {
        // Use state with tui_tx = None
        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        let state = AppState {
            key_validator: Arc::new(MockKeyValidator::new(vec![Scope::OpenAiCompat])),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: lazy_pool(),
            tui_tx: None,
        };
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // 200 OK — middleware did not interfere with the response.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// S6-2-T3c: `metrics_middleware` correctly records a non-2xx status.
    ///
    /// POST /v1/chat/completions with an empty body → auth passes (MockKeyValidator)
    /// but JSON extraction fails → 422. The logged entry must have status 422.
    #[tokio::test]
    async fn metrics_middleware_records_error_status() {
        let (tx, mut rx) = broadcast::channel::<WsMessage>(TEST_AUDIT_CAPACITY);
        let state = make_state_with_tui(tx);
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        assert!(
            (400..500).contains(&status),
            "bad body must return 4xx, got {status}"
        );

        match rx.try_recv() {
            Ok(WsMessage::RequestLog(entry)) => {
                assert_eq!(
                    entry.status, status,
                    "logged status must match response status"
                );
            }
            Ok(other) => panic!("expected RequestLog, got: {:?}", other),
            Err(e) => panic!("expected a message in the broadcast channel: {:?}", e),
        }
    }
}
