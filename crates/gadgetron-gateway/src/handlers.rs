//! HTTP handler implementations for the OpenAI-compatible gateway endpoints.
//!
//! Handlers are wired into `build_router` in `server.rs`.  Each handler
//! receives shared state via `axum::extract::State<AppState>` and per-request
//! context via `axum::Extension<TenantContext>` (injected by the middleware
//! chain: auth → tenant_context).

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use gadgetron_core::{
    context::TenantContext,
    error::GadgetronError,
    knowledge::AuthenticatedContext,
    message::{Content, Message, Role},
    provider::{ChatRequest, ModelInfo},
};
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus};

use crate::activity_capture::{
    capture_chat_completion, capture_chat_completion_error, error_class,
};
use crate::error::ApiError;
use crate::penny::shared_context::render_penny_shared_context;
use crate::server::AppState;
use crate::sse::chat_chunk_to_sse;
use crate::stream_end_guard::StreamEndGuard;

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
    headers: HeaderMap,
    Json(mut req): Json<ChatRequest>,
) -> Response {
    // Inject conversation_id from header if not set in body.
    // Frontend sends X-Gadgetron-Conversation-Id for session continuity.
    if req.conversation_id.is_none() {
        if let Some(val) = headers.get("x-gadgetron-conversation-id") {
            if let Ok(s) = val.to_str() {
                let s = s.trim();
                if !s.is_empty() && s.len() <= 256 {
                    req.conversation_id = Some(s.to_string());
                }
            }
        }
    }

    // W3-PSL-1b: inject shared-context bootstrap before dispatch.
    // Graceful degrade: never 5xx the chat endpoint on bootstrap failure.
    // Authority: docs/design/phase2/13-penny-shared-surface-loop.md §2.2.2,
    // §2.2.4, §2.2.5.
    let shared_cfg = &state.agent_config.shared_context;
    if shared_cfg.enabled {
        if let Some(assembler) = state.penny_assembler.as_ref() {
            let actor = AuthenticatedContext::system();
            match assembler
                .build(&actor, req.conversation_id.as_deref(), ctx.request_id)
                .await
            {
                Ok(bootstrap) => {
                    let block = render_penny_shared_context(
                        &bootstrap,
                        shared_cfg.digest_summary_chars as usize,
                    );
                    let injection_mode = inject_shared_context_block(&mut req.messages, &block);
                    // Record a tracing event for observability. Using
                    // `tracing::info!` (not info_span!) because we want an
                    // event; the handler already runs inside the TraceLayer span.
                    tracing::info!(
                        target: "penny_shared_context.inject",
                        request_id = %ctx.request_id,
                        health = ?bootstrap.health,
                        degraded_reasons = bootstrap.degraded_reasons.len(),
                        rendered_bytes = block.len(),
                        injection_mode = %injection_mode,
                        "shared context block injected"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "penny_shared_context",
                        request_id = %ctx.request_id,
                        error = %e,
                        "penny_shared_context.build_failed — degrading gracefully"
                    );
                    // Continue with original req; do NOT fail the chat request.
                }
            }
        }
    }

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
            // ISSUE 4 TASK 4.2: compute real cost from token counts +
            // the model's entry in the pricing table. Unknown models
            // fall through to 0 cents so a brand-new model never bills
            // a phantom amount — see `gadgetron_core::pricing`.
            let pricing = gadgetron_core::pricing::default_pricing_table();
            let cost_cents = gadgetron_core::pricing::compute_cost_cents(
                &req.model,
                response.usage.prompt_tokens as u64,
                response.usage.completion_tokens as u64,
                &pricing,
            );
            // Drift-fix PR 5 (D-20260418-24): generate the audit correlation
            // key ONCE per outcome so both the audit row and the capture
            // event share an unambiguous identity.
            let audit_event_id = uuid::Uuid::new_v4();

            // Fire-and-forget: quota post-record and audit log.
            state
                .quota_enforcer
                .record_post(&quota_token, cost_cents)
                .await;
            state.audit_writer.send(AuditEntry {
                event_id: audit_event_id,
                tenant_id: ctx.tenant_id,
                api_key_id: ctx.api_key_id,
                actor_user_id: ctx.actor_user_id,
                actor_api_key_id: ctx.actor_api_key_id,
                request_id: ctx.request_id,
                model: Some(req.model.clone()),
                provider: None,
                status: AuditStatus::Ok,
                input_tokens: response.usage.prompt_tokens as i32,
                output_tokens: response.usage.completion_tokens as i32,
                cost_cents,
                latency_ms,
            });
            // ISSUE 4 TASK 4.4: fan out to the /events/ws WebSocket
            // bus so operator dashboards see the completion in real
            // time.
            state.activity_bus.publish(
                gadgetron_core::activity_bus::ActivityEvent::ChatCompleted {
                    tenant_id: ctx.tenant_id,
                    request_id: ctx.request_id,
                    model: req.model.clone(),
                    status: "ok".into(),
                    input_tokens: response.usage.prompt_tokens as i64,
                    output_tokens: response.usage.completion_tokens as i64,
                    cost_cents,
                    latency_ms: latency_ms as i64,
                },
            );

            // PSL-1d: fire-and-forget capture on successful non-streaming chat.
            // Authority: docs/design/core/knowledge-candidate-curation.md §2.1,
            // D-20260418-20. No capture on Err arm (success-only, this PR).
            if let Some(coord) = state.candidate_coordinator.clone() {
                let tenant_id = ctx.tenant_id;
                // PR 7 (doc-10): thread the caller's user_id into the
                // capture. Until a real user table lands, the API-key
                // id is the authoritative identity — captures no longer
                // record `Uuid::nil()` for every row.
                let actor_user_id = ctx.api_key_id;
                let request_id = ctx.request_id;
                let model = req.model.clone();
                let prompt_tokens = response.usage.prompt_tokens;
                let completion_tokens = response.usage.completion_tokens;
                tokio::spawn(async move {
                    capture_chat_completion(
                        coord,
                        tenant_id,
                        actor_user_id,
                        request_id,
                        audit_event_id,
                        model,
                        prompt_tokens,
                        completion_tokens,
                        false,
                    )
                    .await;
                });
            }

            Json(response).into_response()
        }
        Err(e) => {
            let latency_ms = ctx.started_at.elapsed().as_millis() as i32;
            let audit_event_id = uuid::Uuid::new_v4();

            state.audit_writer.send(AuditEntry {
                event_id: audit_event_id,
                tenant_id: ctx.tenant_id,
                api_key_id: ctx.api_key_id,
                actor_user_id: ctx.actor_user_id,
                actor_api_key_id: ctx.actor_api_key_id,
                request_id: ctx.request_id,
                model: Some(req.model.clone()),
                provider: None,
                status: AuditStatus::Error,
                input_tokens: 0,
                output_tokens: 0,
                cost_cents: 0,
                latency_ms,
            });
            state.activity_bus.publish(
                gadgetron_core::activity_bus::ActivityEvent::ChatCompleted {
                    tenant_id: ctx.tenant_id,
                    request_id: ctx.request_id,
                    model: req.model.clone(),
                    status: "error".into(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cost_cents: 0,
                    latency_ms: latency_ms as i64,
                },
            );

            // PSL-1d / KC-1c: fire-and-forget error capture for non-streaming.
            // Streaming error capture requires a Drop-guard (out of scope, W3-KC-1d).
            if let Some(coord) = state.candidate_coordinator.clone() {
                let tenant_id = ctx.tenant_id;
                let actor_user_id = ctx.api_key_id; // PR 7: see capture_chat_completion.
                let request_id = ctx.request_id;
                let model = req.model.clone();
                let ec = error_class(&e);
                tokio::spawn(async move {
                    capture_chat_completion_error(
                        coord,
                        tenant_id,
                        actor_user_id,
                        request_id,
                        audit_event_id,
                        model,
                        ec,
                        latency_ms,
                    )
                    .await;
                });
            }

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
    let stream_started_at = std::time::Instant::now();
    let raw_stream = router.chat_stream(req.clone());
    // Drift-fix PR 5 (D-20260418-24): generate the audit correlation key
    // once at dispatch time. PR 6 Drop-guard emits a SECOND AuditEntry on
    // stream end with a fresh event_id but the same request_id — that's
    // exactly why the two identities have to diverge.
    let audit_event_id = uuid::Uuid::new_v4();

    // Fire-and-forget: quota + dispatch-time AuditEntry (A4 TTFB semantics).
    // The Drop-guard will emit the amendment AuditEntry with real token
    // counts on stream end.
    let audit_writer = state.audit_writer.clone();
    let quota_enforcer = state.quota_enforcer.clone();
    let tenant_id = ctx.tenant_id;
    let api_key_id = ctx.api_key_id;
    let actor_user_id = ctx.actor_user_id;
    let actor_api_key_id = ctx.actor_api_key_id;
    let request_id = ctx.request_id;
    let model = req.model.clone();

    tokio::spawn(async move {
        quota_enforcer.record_post(&quota_token, 0).await;
        audit_writer.send(AuditEntry {
            event_id: audit_event_id,
            tenant_id,
            api_key_id,
            actor_user_id,
            actor_api_key_id,
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

    // PR 6: wrap the raw chunk stream with a StreamEndGuard. The guard
    // owns the audit-writer + coordinator handles and fires on Drop —
    // whether the stream completes normally, the client disconnects, the
    // provider yields a terminal error, or the future is cancelled.
    //
    // The guard's Drop emits:
    //   1. An amendment AuditEntry with the observed `output_tokens`,
    //      correct `latency_ms`, and `status = Ok` or `Error`. Fresh
    //      `event_id`, same `request_id` as the dispatch entry above —
    //      see the drift-fix PR 5 invariant.
    //   2. A PSL-1d `capture_chat_completion` (Ok) or
    //      `capture_chat_completion_error` (Err) activity event,
    //      correlated to the amendment via `audit_event_id`.
    //
    // This REPLACES the previous dispatch-time `capture_chat_completion`
    // call that emitted a 0/0 placeholder activity event — that was a
    // known gap (see the `0, 0` comment the code used to carry). The
    // dispatch-time AuditEntry stays: operators want an A4 audit row
    // even if the stream is abandoned before the first byte.
    let guard = StreamEndGuard::new_with_activity_bus(
        state.audit_writer.clone(),
        state.candidate_coordinator.clone(),
        Some(state.activity_bus.clone()),
        ctx.tenant_id,
        ctx.api_key_id,
        ctx.request_id,
        req.model.clone(),
        stream_started_at,
    );
    let guarded_stream = guard.wrap(raw_stream);

    chat_chunk_to_sse(guarded_stream).into_response()
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
// GET /v1/tools
// ---------------------------------------------------------------------------

/// `GET /v1/tools` — MCP-style tool discovery (ISSUE 7 TASK 7.1).
///
/// Lists every gadget registered with the Penny `GadgetRegistry` in
/// the form external MCP clients expect: `{tools: [{name, description,
/// tier, category, input_schema}], count}`.
///
/// Response notes:
/// - Deduped on `schema.name` — duplicates in `all_schemas` (operator
///   misconfig, see `GadgetRegistryBuilder::freeze`) collapse to the
///   last-registered entry, matching dispatch behavior.
/// - Returns `{"tools": [], "count": 0}` with 200 when the registry is
///   unwired (no `[knowledge]` section) — keeps clients happy.
pub async fn list_tools_handler(State(state): State<AppState>) -> Response {
    use gadgetron_core::agent::tools::GadgetTier;
    use std::collections::BTreeMap;
    let mut deduped: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    if let Some(catalog) = state.tool_catalog.as_ref() {
        for schema in catalog.all_schemas() {
            let tier_str = match schema.tier {
                GadgetTier::Read => "read",
                GadgetTier::Write => "write",
                GadgetTier::Destructive => "destructive",
            };
            deduped.insert(
                schema.name.clone(),
                serde_json::json!({
                    "name": schema.name,
                    "description": schema.description,
                    "tier": tier_str,
                    "input_schema": schema.input_schema,
                    "idempotent": schema.idempotent,
                }),
            );
        }
    }
    let tools: Vec<serde_json::Value> = deduped.into_values().collect();
    let count = tools.len();
    Json(serde_json::json!({
        "tools": tools,
        "count": count,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/tools/{name}/invoke
// ---------------------------------------------------------------------------

/// `POST /v1/tools/{name}/invoke` — MCP tool invocation (ISSUE 7 TASK 7.2).
///
/// External MCP clients (claude-code, custom agents) call this endpoint
/// to actually execute a gadget they discovered via `GET /v1/tools`.
/// Dispatch flows through `Arc<dyn GadgetDispatcher>`, which the gateway
/// holds in `AppState.gadget_dispatcher` — normally the same frozen
/// `GadgetRegistry` Penny uses, so the operator-config L3 allowed-names
/// gate runs here too (a tool the operator disabled in Penny is ALSO
/// unreachable via this path).
///
/// Wire shape:
///
/// - Request body: the gadget's `args` object (JSON) — schema lives at
///   `GET /v1/tools` under `tools[].input_schema`.
/// - Path param: full namespaced name, e.g. `wiki.list`. The axum route
///   uses `{name}` with no regex; dot-separated names work without
///   percent-encoding because axum normalizes `/` only.
/// - Success (gadget ran, possibly with `is_error: true`): HTTP 200
///   `{"content": <value>, "is_error": <bool>}` — matches MCP
///   `tool_result` block shape.
/// - Unknown gadget: HTTP 404 `{"error": {"code": "mcp_unknown_tool",
///   "message": "..."}}`.
/// - Denied by policy: HTTP 403 `{"error": {"code":
///   "mcp_denied_by_policy", ...}}`.
/// - Rate limited: HTTP 429.
/// - Invalid args (JSON Schema violation inside the gadget): HTTP 400.
/// - Execution failure: HTTP 500.
/// - Approval timeout (P2B+): HTTP 408.
/// - Dispatcher unwired (`AppState.gadget_dispatcher == None`): HTTP 503
///   `{"error": {"code": "mcp_not_available", ...}}` — keeps clients
///   from retrying a deployment that can never dispatch.
pub async fn invoke_tool_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    axum::extract::Path(name): axum::extract::Path<String>,
    args: Option<Json<serde_json::Value>>,
) -> Response {
    use gadgetron_core::agent::tools::{GadgetError, GadgetTier as AgentTier};
    use gadgetron_core::audit::{GadgetAuditEvent, GadgetCallOutcome, GadgetTier as AuditTier};

    let Some(dispatcher) = state.gadget_dispatcher.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "code": "mcp_not_available",
                    "message": "gadget dispatcher is not wired on this deployment — \
                                `[knowledge]` section and Penny registration required",
                }
            })),
        )
            .into_response();
    };

    let args_value = args.map(|Json(v)| v).unwrap_or(serde_json::Value::Null);
    let category = name.split('.').next().unwrap_or("").to_string();
    let tier = state
        .tool_catalog
        .as_ref()
        .and_then(|catalog| {
            catalog
                .all_schemas()
                .into_iter()
                .find(|schema| schema.name == name)
                .map(|schema| match schema.tier {
                    AgentTier::Read => AuditTier::Read,
                    AgentTier::Write => AuditTier::Write,
                    AgentTier::Destructive => AuditTier::Destructive,
                })
        })
        .unwrap_or(AuditTier::Read);

    let started = std::time::Instant::now();
    let result = dispatcher.dispatch_gadget(&name, args_value).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let (outcome, response) = match result {
        Ok(gadget_result) => {
            let outcome = if gadget_result.is_error {
                GadgetCallOutcome::Error {
                    error_code: "gadget_reported_error",
                }
            } else {
                GadgetCallOutcome::Success
            };
            let resp = Json(serde_json::json!({
                "content": gadget_result.content,
                "is_error": gadget_result.is_error,
            }))
            .into_response();
            (outcome, resp)
        }
        Err(err) => {
            let code = err.error_code();
            let status = match err {
                GadgetError::UnknownGadget(_) => StatusCode::NOT_FOUND,
                GadgetError::Denied { .. } => StatusCode::FORBIDDEN,
                GadgetError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
                GadgetError::ApprovalTimeout { .. } => StatusCode::REQUEST_TIMEOUT,
                GadgetError::InvalidArgs(_) => StatusCode::BAD_REQUEST,
                GadgetError::Execution(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let message = err.to_string();
            let resp = (
                status,
                Json(serde_json::json!({
                    "error": { "code": code, "message": message }
                })),
            )
                .into_response();
            (GadgetCallOutcome::Error { error_code: code }, resp)
        }
    };

    // ISSUE 7 TASK 7.3 — cross-session audit. Every `/v1/tools/{name}/invoke`
    // call lands a `GadgetCallCompleted` row in `tool_audit_events` with
    // `owner_id = Some(api_key_id)` and `tenant_id = Some(tenant_id)`.
    // Penny calls in P2A populate both as `None`, so operators can filter
    // `WHERE owner_id IS NOT NULL` to pick out cross-session (external MCP)
    // callers. Fire-and-forget: the sink is a bounded channel writer when
    // Postgres is wired, and a Noop otherwise — handler latency is
    // unaffected either way.
    // ISSUE 12 TASK 12.2 — billing ledger for tool calls. Only successful
    // calls land a billing row; error outcomes still emit an audit but no
    // billing event. `cost_cents=0` today (dispatcher doesn't surface cost);
    // invoice materializer (TASK 12.3) applies per-kind base fees at query
    // time. `source_event_id=None` — `tool_audit_events.id` is BIGSERIAL,
    // not a UUID, so TASK 12.4 reconciles by (tenant, gadget, timestamp).
    // Fire-and-forget to match the audit sink's non-blocking contract.
    let billing_tenant = ctx.tenant_id;
    let billing_gadget = name.clone();
    let billing_actor_user_id = ctx.actor_user_id;
    let billing_is_success = matches!(outcome, GadgetCallOutcome::Success);
    state
        .tool_audit_sink
        .send(GadgetAuditEvent::GadgetCallCompleted {
            gadget_name: name,
            tier,
            category,
            outcome,
            elapsed_ms,
            conversation_id: None,
            claude_session_uuid: None,
            owner_id: Some(ctx.api_key_id.to_string()),
            tenant_id: Some(ctx.tenant_id.to_string()),
        });
    if billing_is_success {
        if let Some(pool) = state.pg_pool.clone() {
            tokio::spawn(async move {
                if let Err(e) = gadgetron_xaas::billing::insert_billing_event(
                    &pool,
                    billing_tenant,
                    gadgetron_xaas::billing::BillingEventKind::Tool,
                    0,
                    None,
                    Some(&billing_gadget),
                    None,
                    billing_actor_user_id,
                )
                .await
                {
                    tracing::warn!(
                        target: "billing",
                        tenant_id = %billing_tenant,
                        gadget = %billing_gadget,
                        error = %e,
                        "failed to persist tool billing_events row"
                    );
                }
            });
        }
    }

    response
}

// ---------------------------------------------------------------------------
// inject_shared_context_block — PSL-1b helper
// ---------------------------------------------------------------------------

/// Prepend the shared-context block to a chat message slice.
///
/// Returns a tracing label for the injection mode:
///
/// - `"prepend_to_system"` — the first message was a `System` message with
///   `Content::Text`; the block was prepended to its text.
/// - `"insert_new_system"` — no suitable system message was found at index 0
///   (empty vec, first message is not System, or first message is System with
///   `Content::Parts`); a new `Message::system(block)` was inserted at index 0.
///
/// Rule for `Content::Parts` system messages: inserting a new system message
/// ahead of a Parts system message keeps structured parts intact.
fn inject_shared_context_block(messages: &mut Vec<Message>, block: &str) -> &'static str {
    if let Some(first) = messages.first_mut() {
        if first.role == Role::System {
            if let Content::Text(text) = &mut first.content {
                let mut prefixed = String::with_capacity(block.len() + 2 + text.len());
                prefixed.push_str(block);
                prefixed.push_str("\n\n");
                prefixed.push_str(text);
                *text = prefixed;
                return "prepend_to_system";
            }
            // System message with Parts content — insert a new system msg
            // ahead of it so we don't mangle structured parts.
        }
    }
    messages.insert(0, Message::system(block));
    "insert_new_system"
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
        agent::config::AgentConfig,
        context::Scope,
        error::GadgetronError,
        message::{Content, ContentPart, Message, Role},
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

    use crate::penny::shared_context::{
        DefaultPennyTurnContextAssembler, PennySharedSurfaceService,
    };
    use crate::test_helpers::{lazy_pool, TEST_AUDIT_CAPACITY, VALID_TOKEN};
    use gadgetron_core::agent::shared_context::PennyTurnContextAssembler;

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
                    user_id: None,
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
            workbench: None,
            penny_shared_surface: None,
            agent_config: Arc::new(AgentConfig::default()),
            penny_assembler: None,
            activity_capture_store: None,
            candidate_coordinator: None,
            activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
            tool_catalog: None,
            gadget_dispatcher: None,
            tool_audit_sink: std::sync::Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
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

    // -----------------------------------------------------------------------
    // PSL-1b: inject_shared_context_block unit tests
    // -----------------------------------------------------------------------

    const TEST_BLOCK: &str =
        "<gadgetron_shared_context>\nhealth: healthy\n</gadgetron_shared_context>";

    /// Prepend to existing system message that has Text content.
    /// After injection, content must start with the block and end with
    /// the original system text.
    #[test]
    fn inject_prepends_to_existing_system_message_with_text_content() {
        let original_text = "you are helpful";
        let mut messages = vec![Message::system(original_text), Message::user("hi")];
        let mode = inject_shared_context_block(&mut messages, TEST_BLOCK);
        assert_eq!(mode, "prepend_to_system");
        assert_eq!(messages.len(), 2, "no new messages should be inserted");
        match &messages[0].content {
            Content::Text(text) => {
                assert!(
                    text.starts_with(TEST_BLOCK),
                    "content must start with block; got: {text:?}"
                );
                assert!(
                    text.ends_with(original_text),
                    "content must end with original text; got: {text:?}"
                );
                assert!(
                    text.contains("\n\n"),
                    "block and original must be separated by \\n\\n"
                );
            }
            other => panic!("expected Content::Text, got {other:?}"),
        }
    }

    /// Insert new system message when messages vec starts with a user message.
    #[test]
    fn inject_inserts_new_system_when_messages_empty_of_system() {
        let mut messages = vec![Message::user("hi")];
        let mode = inject_shared_context_block(&mut messages, TEST_BLOCK);
        assert_eq!(mode, "insert_new_system");
        assert_eq!(messages.len(), 2, "a new system message must be prepended");
        assert_eq!(
            messages[0].role,
            Role::System,
            "injected message must have System role"
        );
        match &messages[0].content {
            Content::Text(text) => assert_eq!(text, TEST_BLOCK),
            other => panic!("expected Content::Text, got {other:?}"),
        }
        assert_eq!(
            messages[1].role,
            Role::User,
            "original user message must remain at index 1"
        );
    }

    /// When the first message is System with Parts content, insert a NEW system
    /// message at index 0 rather than mangling the structured parts.
    #[test]
    fn inject_inserts_new_system_when_first_message_is_parts_content() {
        let parts_message = Message {
            role: Role::System,
            content: Content::Parts(vec![ContentPart::Text {
                text: "structured system".to_string(),
            }]),
            reasoning_content: None,
        };
        let mut messages = vec![parts_message];
        let mode = inject_shared_context_block(&mut messages, TEST_BLOCK);
        assert_eq!(mode, "insert_new_system");
        assert_eq!(messages.len(), 2);
        // Index 0 is the newly inserted plain-text system message.
        assert_eq!(messages[0].role, Role::System);
        match &messages[0].content {
            Content::Text(text) => assert_eq!(text, TEST_BLOCK),
            other => panic!("expected Content::Text, got {other:?}"),
        }
        // Index 1 is the original Parts system message (unmodified).
        match &messages[1].content {
            Content::Parts(_) => {}
            other => panic!("original Parts message must be unchanged, got {other:?}"),
        }
    }

    /// When messages is empty, a new system message is inserted at index 0.
    #[test]
    fn inject_inserts_new_system_when_messages_vec_is_empty() {
        let mut messages: Vec<Message> = vec![];
        let mode = inject_shared_context_block(&mut messages, TEST_BLOCK);
        assert_eq!(mode, "insert_new_system");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::System);
    }

    // -----------------------------------------------------------------------
    // PSL-1b: handler-level graceful degrade tests
    // -----------------------------------------------------------------------

    /// Fake surface service that returns errors for all read operations.
    struct AllErrorService;

    #[async_trait]
    impl PennySharedSurfaceService for AllErrorService {
        async fn recent_activity(
            &self,
            _actor: &gadgetron_core::knowledge::AuthenticatedContext,
            _limit: u32,
        ) -> gadgetron_core::error::Result<
            Vec<gadgetron_core::agent::shared_context::PennyActivityDigest>,
        > {
            Err(GadgetronError::Provider("activity down".into()))
        }
        async fn pending_candidates(
            &self,
            _actor: &gadgetron_core::knowledge::AuthenticatedContext,
            _limit: u32,
        ) -> gadgetron_core::error::Result<
            Vec<gadgetron_core::agent::shared_context::PennyCandidateDigest>,
        > {
            Err(GadgetronError::Provider("candidates down".into()))
        }
        async fn pending_approvals(
            &self,
            _actor: &gadgetron_core::knowledge::AuthenticatedContext,
            _limit: u32,
        ) -> gadgetron_core::error::Result<
            Vec<gadgetron_core::agent::shared_context::PennyApprovalDigest>,
        > {
            Err(GadgetronError::Provider("approvals down".into()))
        }
        async fn request_evidence(
            &self,
            _actor: &gadgetron_core::knowledge::AuthenticatedContext,
            _request_id: Uuid,
        ) -> gadgetron_core::error::Result<
            gadgetron_core::workbench::WorkbenchRequestEvidenceResponse,
        > {
            Err(GadgetronError::Forbidden)
        }
        async fn decide_candidate(
            &self,
            _actor: &gadgetron_core::knowledge::AuthenticatedContext,
            _request: gadgetron_core::agent::shared_context::PennyCandidateDecisionRequest,
        ) -> gadgetron_core::error::Result<
            gadgetron_core::agent::shared_context::PennyCandidateDecisionReceipt,
        > {
            Err(GadgetronError::Forbidden)
        }
    }

    fn make_state_with_psl_service(
        provider: impl LlmProvider + 'static,
        provider_name: &str,
        surface: Option<Arc<dyn PennySharedSurfaceService>>,
        agent_cfg: AgentConfig,
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

        // Build the penny_assembler from the surface if present.
        // Uses DefaultPennyTurnContextAssembler which requires a Sized S.
        // We use the Arc<dyn PennySharedSurfaceService> blanket impl.
        let penny_assembler: Option<Arc<dyn PennyTurnContextAssembler>> =
            surface.as_ref().map(|svc| {
                let assembler = DefaultPennyTurnContextAssembler {
                    service: Arc::new(svc.clone()),
                    config: agent_cfg.shared_context.clone(),
                };
                Arc::new(assembler) as Arc<dyn PennyTurnContextAssembler>
            });

        AppState {
            key_validator: Arc::new(AlwaysAcceptValidator::new(vec![Scope::OpenAiCompat])),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(providers_for_state),
            router: Some(Arc::new(lrouter)),
            pg_pool: Some(lazy_pool()),
            no_db: false,
            tui_tx: None,
            workbench: None,
            penny_shared_surface: surface,
            agent_config: Arc::new(agent_cfg),
            penny_assembler,
            activity_capture_store: None,
            candidate_coordinator: None,
            activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
            tool_catalog: None,
            gadget_dispatcher: None,
            tool_audit_sink: std::sync::Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
        }
    }

    /// When `penny_shared_surface` is `None`, the request proceeds without any
    /// bootstrap injection — messages are unchanged.
    #[tokio::test]
    async fn inject_does_nothing_when_service_none() {
        let state = make_state_with_psl_service(
            FakeLlmProvider::new("fake-model"),
            "fake",
            None,
            AgentConfig::default(),
        );
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
        // Handler must return 200 normally — no bootstrap injection.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// When `agent_config.shared_context.enabled = false`, no bootstrap is
    /// injected even if the service is configured.
    #[tokio::test]
    async fn inject_does_nothing_when_flag_disabled() {
        let mut cfg = AgentConfig::default();
        cfg.shared_context.enabled = false;

        let state = make_state_with_psl_service(
            FakeLlmProvider::new("fake-model"),
            "fake",
            Some(Arc::new(AllErrorService)),
            cfg,
        );
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
        // Must return 200 — AllErrorService never gets called.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// When the assembler's service returns errors for all reads, the handler
    /// must still return 200 (graceful degrade — no 5xx caused by bootstrap failure).
    #[tokio::test]
    async fn inject_degrades_gracefully_when_assembler_errs() {
        // AllErrorService causes degraded bootstrap but assembler returns Ok(degraded).
        let state = make_state_with_psl_service(
            FakeLlmProvider::new("fake-model"),
            "fake",
            Some(Arc::new(AllErrorService)),
            AgentConfig::default(),
        );
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
        // Key invariant: bootstrap failure must NOT produce a 5xx response.
        let status = resp.status().as_u16();
        assert!(
            status < 500,
            "bootstrap service failure must not cause 5xx; got {status}"
        );
        assert_eq!(status, 200, "degraded bootstrap still produces 200 OK");
    }

    /// Calling the handler twice with the same conversation_id must invoke the
    /// assembler both times — session store does NOT short-circuit bootstrap
    /// (doc §2.2.4: "every turn gets a fresh bootstrap").
    #[tokio::test]
    async fn inject_reassembles_every_turn_even_with_conversation_id() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Service that counts how many times `recent_activity` was called.
        struct CountingService {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl PennySharedSurfaceService for CountingService {
            async fn recent_activity(
                &self,
                _actor: &gadgetron_core::knowledge::AuthenticatedContext,
                _limit: u32,
            ) -> gadgetron_core::error::Result<
                Vec<gadgetron_core::agent::shared_context::PennyActivityDigest>,
            > {
                self.call_count.fetch_add(1, Ordering::Relaxed);
                Ok(vec![])
            }
            async fn pending_candidates(
                &self,
                _actor: &gadgetron_core::knowledge::AuthenticatedContext,
                _limit: u32,
            ) -> gadgetron_core::error::Result<
                Vec<gadgetron_core::agent::shared_context::PennyCandidateDigest>,
            > {
                Ok(vec![])
            }
            async fn pending_approvals(
                &self,
                _actor: &gadgetron_core::knowledge::AuthenticatedContext,
                _limit: u32,
            ) -> gadgetron_core::error::Result<
                Vec<gadgetron_core::agent::shared_context::PennyApprovalDigest>,
            > {
                Ok(vec![])
            }
            async fn request_evidence(
                &self,
                _actor: &gadgetron_core::knowledge::AuthenticatedContext,
                _request_id: Uuid,
            ) -> gadgetron_core::error::Result<
                gadgetron_core::workbench::WorkbenchRequestEvidenceResponse,
            > {
                Err(GadgetronError::Forbidden)
            }
            async fn decide_candidate(
                &self,
                _actor: &gadgetron_core::knowledge::AuthenticatedContext,
                _request: gadgetron_core::agent::shared_context::PennyCandidateDecisionRequest,
            ) -> gadgetron_core::error::Result<
                gadgetron_core::agent::shared_context::PennyCandidateDecisionReceipt,
            > {
                Err(GadgetronError::Forbidden)
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(CountingService {
            call_count: call_count.clone(),
        });

        let state = make_state_with_psl_service(
            FakeLlmProvider::new("fake-model"),
            "fake",
            Some(service as Arc<dyn PennySharedSurfaceService>),
            AgentConfig::default(),
        );
        let app = build_test_app(state);

        // Use a fixed conversation_id in both requests.
        let body_json = serde_json::to_vec(&serde_json::json!({
            "model": "fake-model",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false,
            "conversation_id": "conv-resume-test"
        }))
        .unwrap();

        // First request.
        let req1 = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::from(body_json.clone()))
            .unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        // Second request with the same conversation_id.
        let req2 = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::from(body_json))
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);

        // Assembler must have been called twice (once per turn).
        let count = call_count.load(Ordering::Relaxed);
        assert_eq!(
            count, 2,
            "assembler must be invoked on every turn regardless of conversation_id; got {count}"
        );
    }
}
