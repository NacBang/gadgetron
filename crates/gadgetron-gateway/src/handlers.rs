//! HTTP handler implementations for the OpenAI-compatible gateway endpoints.
//!
//! Handlers are wired into `build_router` in `server.rs`.  Each handler
//! receives shared state via `axum::extract::State<AppState>` and per-request
//! context via `axum::Extension<TenantContext>` (injected by the middleware
//! chain: auth → tenant_context).

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Extension, Json,
};
use gadgetron_core::{
    context::TenantContext,
    error::GadgetronError,
    provider::{ChatRequest, ModelInfo},
};
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus};

use crate::error::ApiError;
use crate::server::AppState;
use crate::sse::chat_chunk_to_sse;

// ---------------------------------------------------------------------------
// POST /v1/chat/completions
// ---------------------------------------------------------------------------

/// `POST /v1/chat/completions` — OpenAI-compatible chat handler.
///
/// Routing logic:
/// - `req.stream == false` → calls `router.chat()` → returns `Json<ChatResponse>` (200).
/// - `req.stream == true`  → calls `router.chat_stream()` → pipes through
///   `chat_chunk_to_sse` → returns `Sse<...>` with `Content-Type: text/event-stream`.
///
/// Error paths (all use `ApiError(GadgetronError)` → `IntoResponse`):
/// - `AppState.router` is `None`  → `GadgetronError::Routing` → 503.
/// - `quota_enforcer.check_pre` fails → `GadgetronError::QuotaExceeded` → 429.
/// - `router.chat()` fails → appropriate `GadgetronError` → matching HTTP status.
///
/// Quota and audit are recorded fire-and-forget after dispatch.
pub async fn chat_completions_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(req): Json<ChatRequest>,
) -> Response {
    // 1. Resolve the LLM router — return 503 if not configured.
    let router = match &state.router {
        Some(r) => r.clone(),
        None => {
            return ApiError(GadgetronError::Routing(
                "no LLM router configured".to_string(),
            ))
            .into_response();
        }
    };

    // 2. Pre-flight quota check.
    let quota_token = match state
        .quota_enforcer
        .check_pre(ctx.tenant_id, &ctx.quota_snapshot)
        .await
    {
        Ok(t) => t,
        Err(e) => return ApiError(e).into_response(),
    };

    if req.stream {
        handle_streaming(state, ctx, req, router, quota_token).await
    } else {
        handle_non_streaming(state, ctx, req, router, quota_token).await
    }
}

/// Non-streaming path: `router.chat()` → `Json<ChatResponse>`.
async fn handle_non_streaming(
    state: AppState,
    ctx: TenantContext,
    req: ChatRequest,
    router: std::sync::Arc<gadgetron_router::Router>,
    quota_token: gadgetron_xaas::quota::enforcer::QuotaToken,
) -> Response {
    match router.chat(req.clone()).await {
        Ok(response) => {
            let latency_ms = ctx.started_at.elapsed().as_millis() as i32;
            let cost_cents = 0i64;

            // Fire-and-forget: quota post-record and audit log.
            state
                .quota_enforcer
                .record_post(&quota_token, cost_cents)
                .await;
            state.audit_writer.send(AuditEntry {
                tenant_id: ctx.tenant_id,
                api_key_id: ctx.api_key_id,
                request_id: ctx.request_id,
                model: Some(req.model.clone()),
                provider: None,
                status: AuditStatus::Ok,
                input_tokens: response.usage.prompt_tokens as i32,
                output_tokens: response.usage.completion_tokens as i32,
                cost_cents,
                latency_ms,
            });

            Json(response).into_response()
        }
        Err(e) => {
            let latency_ms = ctx.started_at.elapsed().as_millis() as i32;
            state.audit_writer.send(AuditEntry {
                tenant_id: ctx.tenant_id,
                api_key_id: ctx.api_key_id,
                request_id: ctx.request_id,
                model: Some(req.model.clone()),
                provider: None,
                status: AuditStatus::Error,
                input_tokens: 0,
                output_tokens: 0,
                cost_cents: 0,
                latency_ms,
            });
            ApiError(e).into_response()
        }
    }
}

/// Streaming path: `router.chat_stream()` → SSE pipeline.
///
/// Quota and audit are recorded at dispatch time (A4: time-to-first-byte
/// semantics).  Phase 2 will add a Drop guard on the SSE stream to record
/// total stream duration after the last byte.
async fn handle_streaming(
    state: AppState,
    ctx: TenantContext,
    req: ChatRequest,
    router: std::sync::Arc<gadgetron_router::Router>,
    quota_token: gadgetron_xaas::quota::enforcer::QuotaToken,
) -> Response {
    // Measure dispatch latency BEFORE spawning the audit task (previous bug:
    // the value was captured inside tokio::spawn, always yielding 0ms).
    //
    // KNOWN Phase 1 BEHAVIOR (not a bug — documented for operators):
    //   latency_ms captures ONLY middleware chain + dispatch overhead (sub-millisecond
    //   on current hardware). Streaming total duration is NOT measured because the
    //   audit entry is fired before the first byte leaves the server.
    //
    //   For real end-to-end latency, use:
    //   - `metrics_middleware` → TUI RequestLog broadcast (measures full chain)
    //   - `/metrics` Prometheus histogram (Phase 2)
    //   - Client-side timing (current best option)
    //
    //   Phase 2: wrap the SSE stream in a Drop guard that captures total duration
    //   and accumulates output_tokens from the final stream chunk.
    let latency_ms = ctx.started_at.elapsed().as_millis() as i32;
    let stream = router.chat_stream(req.clone());

    // Fire-and-forget: quota and audit at dispatch time (A4 semantics).
    let audit_writer = state.audit_writer.clone();
    let quota_enforcer = state.quota_enforcer.clone();
    let tenant_id = ctx.tenant_id;
    let api_key_id = ctx.api_key_id;
    let request_id = ctx.request_id;
    let model = req.model.clone();

    tokio::spawn(async move {
        quota_enforcer.record_post(&quota_token, 0).await;
        audit_writer.send(AuditEntry {
            tenant_id,
            api_key_id,
            request_id,
            model: Some(model),
            provider: None,
            status: AuditStatus::Ok,
            input_tokens: 0,
            output_tokens: 0,
            cost_cents: 0,
            latency_ms,
        });
    });

    chat_chunk_to_sse(stream).into_response()
}

// ---------------------------------------------------------------------------
// GET /v1/models
// ---------------------------------------------------------------------------

/// `GET /v1/models` — OpenAI-compatible model listing.
///
/// Aggregates models from all configured providers via `router.list_models()`.
/// Falls back to direct provider iteration when `router` is `None`.
///
/// Response shape: `{"object": "list", "data": [{...}, ...]}`
pub async fn list_models_handler(State(state): State<AppState>) -> Response {
    let models: Vec<ModelInfo> = if let Some(router) = &state.router {
        match router.list_models().await {
            Ok(m) => m,
            Err(e) => return ApiError(e).into_response(),
        }
    } else {
        // Fallback: iterate providers directly (used in tests with router=None).
        let mut all = Vec::new();
        for provider in state.providers.values() {
            if let Ok(m) = provider.models().await {
                all.extend(m);
            }
        }
        all
    };

    let data: Vec<serde_json::Value> = models
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "object": m.object,
                "owned_by": m.owned_by,
            })
        })
        .collect();

    Json(serde_json::json!({
        "object": "list",
        "data": data,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Tests — TDD (written before full integration, red → green)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        Router,
    };
    use futures::stream;
    use gadgetron_core::{
        context::Scope,
        error::GadgetronError,
        message::{Content, Message, Role},
        provider::{
            ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, ChunkDelta, LlmProvider,
            ModelInfo, Usage,
        },
    };
    use gadgetron_router::{router::Router as LlmRouter, MetricsStore};
    use gadgetron_xaas::{
        audit::writer::AuditWriter,
        auth::validator::{KeyValidator, ValidatedKey},
        quota::enforcer::InMemoryQuotaEnforcer,
    };
    use std::{collections::HashMap, sync::Arc};
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::test_helpers::{lazy_pool, TEST_AUDIT_CAPACITY, VALID_TOKEN};

    // -----------------------------------------------------------------------
    // Constants for FakeLlmProvider fixed responses
    // -----------------------------------------------------------------------

    /// Stable chat-completion ID used across the two fake SSE chunks.
    const FAKE_CHAT_ID: &str = "chatcmpl-test-001";
    /// Unix timestamp embedded in fake responses (2023-11-14 22:13:20 UTC).
    const FAKE_CREATED_TS: u64 = 1_700_000_000;
    /// Model name used by the deterministic `FakeLlmProvider`.
    const FAKE_MODEL_NAME: &str = "fake-model";
    /// `owned_by` field for `ModelInfo` entries returned by `FakeLlmProvider`.
    const FAKE_PROVIDER_ORG: &str = "fake-org";

    // -----------------------------------------------------------------------
    // FakeLlmProvider — deterministic test double for LlmProvider
    // -----------------------------------------------------------------------

    /// Fake provider that returns a fixed `ChatResponse` and a 2-chunk stream.
    ///
    /// Cannot be replaced by `gadgetron_testing::FakeLlmProvider` because
    /// `gadgetron-testing` depends on `gadgetron-gateway` (circular dependency).
    struct FakeLlmProvider {
        model_name: String,
    }

    impl FakeLlmProvider {
        fn new(model_name: impl Into<String>) -> Self {
            Self {
                model_name: model_name.into(),
            }
        }

        fn fixed_response() -> ChatResponse {
            ChatResponse {
                id: FAKE_CHAT_ID.to_string(),
                object: "chat.completion".to_string(),
                created: FAKE_CREATED_TS,
                model: FAKE_MODEL_NAME.to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message {
                        role: Role::Assistant,
                        content: Content::Text("Hello from FakeLlmProvider!".to_string()),
                        reasoning_content: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 7,
                    total_tokens: 17,
                },
            }
        }

        fn fixed_chunks() -> Vec<ChatChunk> {
            vec![
                ChatChunk {
                    id: FAKE_CHAT_ID.to_string(),
                    object: "chat.completion.chunk".to_string(),
                    created: FAKE_CREATED_TS,
                    model: FAKE_MODEL_NAME.to_string(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: Some("assistant".to_string()),
                            content: Some("Hello".to_string()),
                            tool_calls: None,
                            reasoning_content: None,
                        },
                        finish_reason: None,
                    }],
                },
                ChatChunk {
                    id: FAKE_CHAT_ID.to_string(),
                    object: "chat.completion.chunk".to_string(),
                    created: FAKE_CREATED_TS,
                    model: FAKE_MODEL_NAME.to_string(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: None,
                            content: Some(" World!".to_string()),
                            tool_calls: None,
                            reasoning_content: None,
                        },
                        finish_reason: Some("stop".to_string()),
                    }],
                },
            ]
        }
    }

    #[async_trait]
    impl LlmProvider for FakeLlmProvider {
        async fn chat(&self, _req: ChatRequest) -> gadgetron_core::error::Result<ChatResponse> {
            Ok(Self::fixed_response())
        }

        fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> std::pin::Pin<
            Box<dyn futures::Stream<Item = gadgetron_core::error::Result<ChatChunk>> + Send>,
        > {
            let chunks = Self::fixed_chunks();
            Box::pin(stream::iter(chunks.into_iter().map(Ok)))
        }

        async fn models(&self) -> gadgetron_core::error::Result<Vec<ModelInfo>> {
            Ok(vec![ModelInfo {
                id: self.model_name.clone(),
                object: "model".to_string(),
                owned_by: FAKE_PROVIDER_ORG.to_string(),
            }])
        }

        fn name(&self) -> &str {
            "fake"
        }

        async fn health(&self) -> gadgetron_core::error::Result<()> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // FakeErrorProvider — returns StreamInterrupted for chat_stream
    // -----------------------------------------------------------------------

    struct FakeErrorProvider;

    #[async_trait]
    impl LlmProvider for FakeErrorProvider {
        async fn chat(&self, _req: ChatRequest) -> gadgetron_core::error::Result<ChatResponse> {
            Err(GadgetronError::Provider("fake error".to_string()))
        }

        fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> std::pin::Pin<
            Box<dyn futures::Stream<Item = gadgetron_core::error::Result<ChatChunk>> + Send>,
        > {
            Box::pin(stream::iter(vec![Err(GadgetronError::StreamInterrupted {
                reason: "fake stream error".to_string(),
            })]))
        }

        async fn models(&self) -> gadgetron_core::error::Result<Vec<ModelInfo>> {
            Ok(vec![])
        }

        fn name(&self) -> &str {
            "fake-error"
        }

        async fn health(&self) -> gadgetron_core::error::Result<()> {
            Err(GadgetronError::Provider("unhealthy".to_string()))
        }
    }

    // -----------------------------------------------------------------------
    // Auth doubles
    // -----------------------------------------------------------------------

    struct AlwaysAcceptValidator {
        key: Arc<ValidatedKey>,
    }

    impl AlwaysAcceptValidator {
        fn new(scopes: Vec<Scope>) -> Self {
            Self {
                key: Arc::new(ValidatedKey {
                    api_key_id: Uuid::new_v4(),
                    tenant_id: Uuid::new_v4(),
                    scopes,
                }),
            }
        }
    }

    #[async_trait]
    impl KeyValidator for AlwaysAcceptValidator {
        async fn validate(
            &self,
            _key_hash: &str,
        ) -> gadgetron_core::error::Result<Arc<ValidatedKey>> {
            Ok(self.key.clone())
        }
        async fn invalidate(&self, _key_hash: &str) {}
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build an `AppState` with the given `LlmProvider` wired into a `Router`.
    fn make_state_with_provider(
        provider: impl LlmProvider + 'static,
        provider_name: &str,
    ) -> AppState {
        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        let mut providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
        providers.insert(provider_name.to_string(), Arc::new(provider));

        let metrics = Arc::new(MetricsStore::new());
        let routing_config = gadgetron_core::routing::RoutingConfig {
            default_strategy: gadgetron_core::routing::RoutingStrategy::RoundRobin,
            fallbacks: HashMap::new(),
            costs: HashMap::new(),
        };
        let lrouter = LlmRouter::new(providers.clone(), routing_config, metrics);

        let providers_for_state: HashMap<String, Arc<dyn LlmProvider + Send + Sync>> = providers
            .into_iter()
            .map(|(k, v)| (k, v as Arc<dyn LlmProvider + Send + Sync>))
            .collect();

        AppState {
            key_validator: Arc::new(AlwaysAcceptValidator::new(vec![Scope::OpenAiCompat])),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(providers_for_state),
            router: Some(Arc::new(lrouter)),
            pg_pool: Some(lazy_pool()),
            no_db: false,
            tui_tx: None,
        }
    }

    /// Build the full axum `Router` from an `AppState` that has a real provider.
    fn build_test_app(state: AppState) -> Router {
        crate::server::build_router(state)
    }

    /// Minimal valid `ChatRequest` JSON body.
    fn chat_request_body(stream: bool) -> serde_json::Value {
        serde_json::json!({
            "model": "fake-model",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": stream
        })
    }

    // -----------------------------------------------------------------------
    // S3-4 required TDD tests
    // -----------------------------------------------------------------------

    /// `POST /v1/chat/completions` with `stream: false` returns HTTP 200
    /// and a valid `ChatResponse` JSON body.
    #[tokio::test]
    async fn non_streaming_returns_json_response() {
        let state = make_state_with_provider(FakeLlmProvider::new("fake-model"), "fake");
        let app = build_test_app(state);

        let body_json = serde_json::to_vec(&chat_request_body(false)).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::from(body_json))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "non-streaming must return 200"
        );

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Verify OpenAI-compatible shape.
        assert_eq!(value["object"], "chat.completion");
        assert!(value["id"].is_string(), "id must be present");
        assert!(value["choices"].is_array(), "choices must be an array");
        assert!(!value["choices"].as_array().unwrap().is_empty());
        assert!(value["usage"]["prompt_tokens"].is_number());
    }

    /// `POST /v1/chat/completions` with `stream: true` returns HTTP 200
    /// and `Content-Type: text/event-stream`.
    #[tokio::test]
    async fn streaming_returns_sse_content_type() {
        let state = make_state_with_provider(FakeLlmProvider::new("fake-model"), "fake");
        let app = build_test_app(state);

        let body_json = serde_json::to_vec(&chat_request_body(true)).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::from(body_json))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "streaming must return 200");

        let ct = resp
            .headers()
            .get("content-type")
            .expect("content-type header must be present")
            .to_str()
            .unwrap();
        assert!(
            ct.contains("text/event-stream"),
            "content-type must be text/event-stream, got: {ct}"
        );
    }

    /// A normal streaming response ends with `data: [DONE]`.
    ///
    /// We drive the full SSE body and confirm the last non-empty data line
    /// is `data: [DONE]`.
    #[tokio::test]
    async fn sse_stream_ends_with_done() {
        let state = make_state_with_provider(FakeLlmProvider::new("fake-model"), "fake");
        let app = build_test_app(state);

        let body_json = serde_json::to_vec(&chat_request_body(true)).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::from(body_json))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();

        // SSE frames are separated by "\n\n".  Collect all `data:` lines.
        let data_lines: Vec<&str> = body_str
            .lines()
            .filter(|l| l.starts_with("data:"))
            .collect();

        assert!(
            !data_lines.is_empty(),
            "SSE response must contain at least one data: line"
        );

        let last = *data_lines.last().unwrap();
        assert_eq!(
            last.trim(),
            "data: [DONE]",
            "last SSE frame must be 'data: [DONE]', got: {last:?}"
        );
    }

    /// A stream that errors emits an `event: error` SSE frame and does NOT
    /// emit `data: [DONE]`.
    #[tokio::test]
    async fn sse_error_emits_error_event() {
        let state = make_state_with_provider(FakeErrorProvider, "fake-error");
        let app = build_test_app(state);

        let body_json = serde_json::to_vec(&chat_request_body(true)).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::from(body_json))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // The SSE response itself is HTTP 200 — errors are signalled inside the stream.
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();

        // There must be an `event: error` frame.
        assert!(
            body_str.contains("event: error"),
            "SSE body must contain 'event: error', got:\n{body_str}"
        );

        // `data: [DONE]` must NOT appear after an error (P3 decision).
        assert!(
            !body_str.contains("[DONE]"),
            "SSE body must NOT contain [DONE] after an error, got:\n{body_str}"
        );
    }

    /// `GET /v1/models` returns HTTP 200 with a non-empty model list that
    /// includes the model registered on `FakeLlmProvider`.
    #[tokio::test]
    async fn list_models_returns_configured_models() {
        let state = make_state_with_provider(FakeLlmProvider::new("fake-gpt-4"), "fake");
        let app = build_test_app(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "list_models must return 200");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["object"], "list", "envelope object must be 'list'");
        let data = value["data"].as_array().expect("data must be an array");
        assert!(
            !data.is_empty(),
            "data must contain at least one model entry"
        );

        let ids: Vec<&str> = data.iter().filter_map(|m| m["id"].as_str()).collect();
        assert!(
            ids.contains(&"fake-gpt-4"),
            "model 'fake-gpt-4' must appear in the listing, got: {ids:?}"
        );
    }
}
