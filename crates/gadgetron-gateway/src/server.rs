use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use gadgetron_core::provider::LlmProvider;
use gadgetron_core::ui::WsMessage;
use gadgetron_router::Router as LlmRouter;
use gadgetron_xaas::audit::writer::AuditWriter;
use gadgetron_xaas::auth::validator::KeyValidator;
use gadgetron_xaas::quota::enforcer::QuotaEnforcer;
use serde_json::json;
use tokio::sync::broadcast;
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};

use crate::handlers::{chat_completions_handler, list_models_handler};
use crate::middleware::{
    auth::auth_middleware, metrics::metrics_middleware, request_id::request_id_middleware,
    scope::scope_guard_middleware, tenant_context::tenant_context_middleware,
};

/// 4 MiB body limit. SEC-M2 / §2.B.8 layer 1 (outermost).
/// Rationale: 128k-token window × ~4 bytes/token ≈ 512 KB; 8× headroom.
pub(crate) const MAX_BODY_BYTES: usize = 4_194_304;

/// Format a byte count as a human-readable MiB string (e.g. 4_194_304 → "4 MiB").
/// Used by `openai_shape_413` so the 413 error message stays in sync with
/// `MAX_BODY_BYTES` if an operator tunes the limit. No hard-coded "4 MB" literals.
fn format_body_limit(limit: usize) -> String {
    let mib = limit as f64 / 1_048_576.0;
    if (mib.fract()).abs() < f64::EPSILON {
        format!("{} MiB", mib as u64)
    } else {
        format!("{mib:.1} MiB")
    }
}

/// `map_response` layer that converts `tower_http::RequestBodyLimitLayer`'s
/// raw `413 Payload Too Large` plain-text response into the OpenAI-shaped
/// `{error: {code, message, type}}` JSON that every other error path returns.
///
/// Rationale: OpenAI SDK clients call `response.json()` and surface the
/// resulting `error.message` to developers. If the body is plain text,
/// `json.JSONDecodeError` escapes as an opaque "server returned invalid
/// JSON" — a terrible DX when the real cause is "you sent a 5 MB request".
///
/// Non-413 responses pass through unchanged. Async signature is required by
/// `axum::middleware::map_response` even though the body is sync.
async fn openai_shape_413(mut resp: Response<Body>) -> Response<Body> {
    if resp.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return resp;
    }
    let message = format!(
        "Request body exceeds the {} limit. Reduce your request size or split it across multiple calls.",
        format_body_limit(MAX_BODY_BYTES),
    );
    let body_json = json!({
        "error": {
            "code": "request_too_large",
            "message": message,
            "type": "invalid_request_error",
        }
    });
    let bytes = serde_json::to_vec(&body_json).expect("static JSON shape serializes");
    let len = bytes.len();
    *resp.body_mut() = Body::from(bytes);
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&len.to_string()).expect("usize string is ASCII"),
    );
    resp
}

/// Shared application state injected into every handler via `axum::State`.
///
/// All fields are `Arc`-wrapped or `Clone`-cheap so `Clone` is a pointer copy (~1 ns).
/// `Send + Sync` is satisfied because every inner type is `Send + Sync`.
#[derive(Clone)]
pub struct AppState {
    /// Bearer-token validator (moka-cached, 10-min TTL, max 10 000 entries).
    pub key_validator: Arc<dyn KeyValidator + Send + Sync>,
    /// Pre/post quota enforcement. Phase 1: `InMemoryQuotaEnforcer`.
    pub quota_enforcer: Arc<dyn QuotaEnforcer + Send + Sync>,
    /// Async audit-log channel writer (capacity 4 096).
    pub audit_writer: Arc<AuditWriter>,
    /// Registered LLM providers, keyed by provider name.
    /// Retained for direct inspection (e.g. list_models) and test setup.
    pub providers: Arc<HashMap<String, Arc<dyn LlmProvider + Send + Sync>>>,
    /// Routing layer: wraps `providers` with strategy + fallback + metrics.
    /// `None` only in legacy unit-test fixtures that do not exercise handlers.
    pub router: Option<Arc<LlmRouter>>,
    /// PostgreSQL connection pool. `None` in no-db mode, `Some(_)` in full mode.
    /// Used by `/ready` health check and future audit flush.
    /// `PgPool` is internally `Arc<PoolInner>` — clone is a pointer increment.
    pub pg_pool: Option<sqlx::PgPool>,
    /// `true` when running in no-db mode (no PostgreSQL). `/ready` returns 200 unconditionally.
    pub no_db: bool,
    /// Broadcast sender for TUI live updates. `None` when `--tui` flag is absent.
    /// Capacity: 1_024 (1_000 QPS ceiling × ~1s TUI drain period + 24 headroom).
    /// `broadcast::Sender<T>` is `Clone` — clone is an Arc pointer increment.
    pub tui_tx: Option<broadcast::Sender<WsMessage>>,
}

// chat_completions_handler and list_models_handler are the real implementations
// imported from crate::handlers. See crates/gadgetron-gateway/src/handlers.rs.

// Admin stubs
async fn list_nodes_handler() -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn deploy_model_handler() -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn undeploy_model_handler() -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn model_status_handler() -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn usage_handler() -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn costs_handler() -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

// ---------------------------------------------------------------------------
// Health handlers — real implementations (always 200 in Phase 1)
// ---------------------------------------------------------------------------

/// `GET /health`
///
/// Always returns `{"status":"ok"}` with HTTP 200.
/// Does not depend on AppState — can be called before the DB pool is ready.
pub async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

/// `GET /ready`
///
/// In no-db mode (`state.no_db == true`): always returns HTTP 200 `{"status":"ready"}`.
/// In full mode: executes `SELECT 1` against the PG pool.
/// Returns HTTP 200 `{"status":"ready"}` on success.
/// Returns HTTP 503 `{"status":"unavailable"}` when the pool is absent or cannot reach PG.
/// Used as a K8s readiness probe target.
pub async fn ready_handler(State(state): State<AppState>) -> impl IntoResponse {
    if state.no_db {
        return (StatusCode::OK, Json(json!({"status": "ready"})));
    }
    match &state.pg_pool {
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "unavailable"})),
        ),
        Some(pool) => match sqlx::query("SELECT 1").execute(pool).await {
            Ok(_) => (StatusCode::OK, Json(json!({"status": "ready"}))),
            Err(e) => {
                tracing::warn!(error = %e, "readiness check failed");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"status": "unavailable"})),
                )
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the gateway `axum::Router` with the canonical Tower middleware chain.
///
/// Route groups:
/// - `authenticated_routes`: require Bearer auth (Tower layers applied).
///   `/v1/chat/completions` (POST), `/v1/models` (GET), `/api/v1/*` admin.
/// - `public_routes`: no auth. `/health` (GET), `/ready` (GET).
///
/// Tower layer order on `authenticated_routes` (outermost → innermost):
///
///   RequestBodyLimitLayer (4 MB)
///   → TraceLayer
///   → request_id_middleware    (generates UUID, sets x-request-id header)
///   → auth_middleware           (validates Bearer token → inserts ValidatedKey)
///   → tenant_context_middleware (builds TenantContext from ValidatedKey)
///   → scope_guard_middleware    (enforces per-route scope requirements)
///   → handler
///
/// `ServiceBuilder` preserves declaration order — first-declared = outermost.
/// Requests traverse layers top-to-bottom; responses bottom-to-top.
///
/// CorsLayer is intentionally absent (D-6: no `CorsLayer::permissive()`).
pub fn build_router(state: AppState) -> Router {
    let authenticated_routes = Router::new()
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/models", get(list_models_handler))
        .route("/api/v1/nodes", get(list_nodes_handler))
        .route("/api/v1/models/deploy", post(deploy_model_handler))
        .route(
            "/api/v1/models/{id}",
            axum::routing::delete(undeploy_model_handler),
        )
        .route("/api/v1/models/status", get(model_status_handler))
        .route("/api/v1/usage", get(usage_handler))
        .route("/api/v1/costs", get(costs_handler))
        // Auth middleware stack (layers 3–7). These all operate on `Body` (not
        // `Limited<Body>`), so they must be separate from RequestBodyLimitLayer
        // which wraps the body type and would require downstream middleware to
        // accept `Request<Limited<Body>>`.
        //
        // Layer ordering in axum: each `.layer()` call on `Router` is the NEW
        // outermost layer — it wraps all previously applied layers. Therefore we
        // call `.layer()` from innermost-to-outermost (metrics first, then scope, auth, etc.).
        //
        // Final inbound request flow:
        //   [body-limit] → [trace] → [request-id] → [auth] → [tenant-ctx] → [scope] → [metrics] → handler
        //
        // Layer 7 (innermost — closest to handler): emit RequestLog after handler completes.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            metrics_middleware,
        ))
        // Layer 6: scope enforcement → 403 on mismatch.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            scope_guard_middleware,
        ))
        // Layer 5: build TenantContext from ValidatedKey.
        .layer(middleware::from_fn(tenant_context_middleware))
        // Layer 4: Bearer auth → insert Arc<ValidatedKey> into extensions.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // Layer 3: generate request UUID, insert into extensions + response header.
        .layer(middleware::from_fn(request_id_middleware))
        // Layer 2: distributed tracing spans.
        .layer(TraceLayer::new_for_http())
        // Layer 1b: body size guard (4 MiB). SEC-M2. The raw 413 produced here
        // is plain text ("length limit exceeded"), which breaks OpenAI SDK clients
        // that call `response.json()`.
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        // Layer 1a (outermost): response shape guard. Catches the 413 above and
        // rewrites the body to OpenAI-shaped JSON. Non-413 responses pass through.
        .layer(middleware::map_response(openai_shape_413))
        .with_state(state.clone()); // clone BEFORE consuming state in public_routes

    let public_routes = Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .with_state(state); // state moved here (last consumer)

    Router::new()
        .merge(authenticated_routes)
        .merge(public_routes)
}

/// Like [`build_router`], but also mounts the embedded Gadgetron Web UI at `/web`
/// when the `web-ui` Cargo feature is enabled AND `web_cfg.enabled == true`.
///
/// This is the production entry point used by `gadgetron serve`. Tests that do not
/// need the Web UI keep calling [`build_router`] directly.
///
/// Added by D-20260414-02 / `docs/design/phase2/03-gadgetron-web.md` §7.
#[cfg(feature = "web-ui")]
pub fn build_router_with_web(
    state: AppState,
    web_cfg: &gadgetron_core::config::WebConfig,
) -> Router {
    let base = build_router(state);
    if !web_cfg.enabled {
        return base;
    }
    let service_cfg = crate::web_csp::translate_config(web_cfg);
    let web_router = crate::web_csp::apply_web_headers(gadgetron_web::service(&service_cfg));
    base.nest("/web", web_router)
}

// ---------------------------------------------------------------------------
// Tests — written before implementation (TDD red → green)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use gadgetron_core::context::Scope;
    use gadgetron_xaas::audit::writer::AuditWriter;
    use gadgetron_xaas::auth::validator::ValidatedKey;
    use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
    use tower::ServiceExt; // for `oneshot`
    use uuid::Uuid;

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    use crate::test_helpers::{lazy_pool, TEST_AUDIT_CAPACITY, VALID_TOKEN};
    use async_trait::async_trait;

    /// Minimal `KeyValidator` that always rejects — used only for tests
    /// that do not exercise the auth path.
    struct NoopKeyValidator;

    #[async_trait]
    impl KeyValidator for NoopKeyValidator {
        async fn validate(
            &self,
            _key_hash: &str,
        ) -> Result<
            Arc<gadgetron_xaas::auth::validator::ValidatedKey>,
            gadgetron_core::error::GadgetronError,
        > {
            Err(gadgetron_core::error::GadgetronError::TenantNotFound)
        }
        async fn invalidate(&self, _key_hash: &str) {}
    }

    /// `KeyValidator` that returns a fixed `ValidatedKey` for any input.
    ///
    /// Used for tests that need a fully-authenticated request.
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

    #[async_trait]
    impl KeyValidator for MockKeyValidator {
        async fn validate(
            &self,
            _key_hash: &str,
        ) -> Result<Arc<ValidatedKey>, gadgetron_core::error::GadgetronError> {
            Ok(self.result.clone())
        }

        async fn invalidate(&self, _key_hash: &str) {}
    }

    fn make_state() -> AppState {
        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        AppState {
            key_validator: Arc::new(NoopKeyValidator),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: Some(lazy_pool()),
            no_db: false,
            tui_tx: None,
        }
    }

    fn make_state_with_validator(validator: impl KeyValidator + 'static) -> AppState {
        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        AppState {
            key_validator: Arc::new(validator),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: Some(lazy_pool()),
            no_db: false,
            tui_tx: None,
        }
    }

    // ------------------------------------------------------------------
    // S3-2 required tests (preserved)
    // ------------------------------------------------------------------

    /// AppState must derive Clone (all fields are Arc — clone is O(1)).
    #[tokio::test]
    async fn app_state_is_clone() {
        let state = make_state();
        let cloned = state.clone();
        // Arc pointer equality: both point to the same allocations.
        assert!(Arc::ptr_eq(&state.audit_writer, &cloned.audit_writer));
        assert!(Arc::ptr_eq(&state.providers, &cloned.providers));
    }

    /// `build_router` must compile and produce a valid `Router`.
    /// Smoke test: confirms the function signature and return type are correct.
    #[tokio::test]
    async fn build_router_compiles() {
        let state = make_state();
        let _router: Router = build_router(state);
        // If this compiles and runs, the smoke test passes.
    }

    /// `GET /health` must return HTTP 200 with body `{"status":"ok"}`.
    #[tokio::test]
    async fn health_returns_200_with_ok_body() {
        let state = make_state();
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");
    }

    /// `GET /ready` returns HTTP 503 when the pool cannot reach PG.
    ///
    /// The lazy pool (no real PG server) will fail `SELECT 1` at query time,
    /// causing `ready_handler` to return 503 SERVICE_UNAVAILABLE.
    /// This is the expected unit-test behaviour — integration tests use a real pool.
    #[tokio::test]
    async fn ready_returns_503_with_lazy_pool() {
        let state = make_state();
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "unavailable");
    }

    /// POST /v1/chat/completions with an invalid body returns a 4xx error.
    ///
    /// The real `chat_completions_handler` extracts `Json<ChatRequest>` from the body.
    /// An empty body fails JSON extraction → axum returns 400 or 422 before
    /// reaching handler logic.  This replaces the old "501 stub" test now that
    /// the real handler is wired.  We assert any 4xx (not 2xx or 5xx) to avoid
    /// coupling to axum's exact status choice for malformed bodies.
    #[tokio::test]
    async fn chat_completions_bad_body_returns_4xx() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        assert!(
            (400..500).contains(&status),
            "bad body must return a 4xx status, got: {status}"
        );
    }

    /// GET /v1/models with router=None returns 200 with an empty model list.
    ///
    /// When `AppState.router` is `None`, `list_models_handler` falls back to
    /// iterating `state.providers` (empty map) → `{"object":"list","data":[]}`.
    /// This replaces the old "501 stub" test now that the real handler is wired.
    ///
    /// Also serves as a 2xx regression guard for the `openai_shape_413`
    /// map_response layer: verifies successful responses keep their
    /// `application/json` content type and are not rewritten.
    #[tokio::test]
    async fn list_models_returns_200_with_empty_list() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Regression guard (A4): the openai_shape_413 map_response layer must
        // not touch 2xx responses. Proves non-413 status passes through
        // unmodified, with Content-Type preserved.
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("200 must carry a Content-Type")
            .to_str()
            .expect("Content-Type must be ASCII");
        assert!(
            ct.starts_with("application/json"),
            "2xx Content-Type must be JSON, got {ct:?}"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "list");
        assert!(value["data"].as_array().unwrap().is_empty());
    }

    /// Unknown routes return 404 — confirms no catch-all swallows typos.
    #[tokio::test]
    async fn unknown_route_returns_404() {
        let state = make_state();
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/does/not/exist")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ------------------------------------------------------------------
    // S3-3 TDD tests — middleware chain
    // ------------------------------------------------------------------

    /// A request with no Authorization header must return HTTP 401.
    ///
    /// AuthLayer: missing header path → GadgetronError::TenantNotFound → 401.
    #[tokio::test]
    async fn missing_auth_header_returns_401() {
        let state = make_state();
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// A request with an invalid/unknown bearer token must return HTTP 401.
    ///
    /// The token has valid format (`gad_live_...`) but `NoopKeyValidator`
    /// rejects every hash → `TenantNotFound` → 401.
    #[tokio::test]
    async fn invalid_key_returns_401() {
        // NoopKeyValidator always rejects.
        let state = make_state();
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// A valid key with the correct scope reaches the handler.
    ///
    /// MockKeyValidator returns `OpenAiCompat` scope; route is `GET /v1/models`.
    /// With `router: None`, `list_models_handler` returns 200 + empty list —
    /// confirming TenantContext is built and scope check passes.
    #[tokio::test]
    async fn valid_key_injects_tenant_context() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // 200 means the request passed auth + scope and reached the real handler.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A key with only `OpenAiCompat` scope accessing `/api/v1/nodes`
    /// (which requires `Management` scope) must return HTTP 403.
    ///
    /// ScopeGuardLayer enforces per-route scope; mismatch → Forbidden → 403.
    #[tokio::test]
    async fn wrong_scope_returns_403() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/nodes")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// A key with `Management` scope on `/api/v1/nodes` must pass scope guard
    /// and reach the stub handler (501).
    #[tokio::test]
    async fn correct_scope_passes() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::Management]));
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/nodes")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // 501 = reached the stub handler, scope guard passed.
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// Every response from an authenticated route must carry an `x-request-id`
    /// header with a valid UUID value.
    ///
    /// Verifies RequestIdMiddleware is wired and its header reaches the caller.
    #[tokio::test]
    async fn request_id_header_present() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let header_val = resp
            .headers()
            .get("x-request-id")
            .expect("x-request-id header must be present");

        // Must parse as a valid UUID.
        let id_str = header_val.to_str().expect("header is valid UTF-8");
        Uuid::parse_str(id_str).expect("x-request-id must be a valid UUID");
    }

    // ------------------------------------------------------------------
    // S6-1 TDD tests — AppState expansion + /ready PG check
    // ------------------------------------------------------------------

    /// S6-1-T1: AppState must contain `pg_pool` (sqlx::PgPool) and `tui_tx`
    /// (Option<broadcast::Sender<WsMessage>>).
    ///
    /// Verifies both fields are present and correctly typed by construction.
    /// `tui_tx: None` covers the headless (no-TUI) code path.
    #[tokio::test]
    async fn app_state_has_pg_pool_and_tui_tx() {
        let state = make_state();
        // pg_pool: field access must compile and pool must be in a usable (lazy) state.
        // PgPool exposes .size() which returns 0 for a pool with no live connections.
        assert_eq!(
            state.pg_pool.as_ref().unwrap().size(),
            0,
            "lazy pool starts with 0 connections"
        );
        // tui_tx: None for the default headless state.
        assert!(
            state.tui_tx.is_none(),
            "tui_tx must be None when TUI is disabled"
        );

        // Verify that tui_tx: Some(sender) is also constructible.
        // 1_024 = production broadcast channel capacity (see server.rs AppState doc).
        let (tx, _rx) = broadcast::channel::<WsMessage>(1_024);
        let (audit_writer, _arx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        let state_with_tui = AppState {
            key_validator: Arc::new(NoopKeyValidator),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: Some(lazy_pool()),
            no_db: false,
            tui_tx: Some(tx),
        };
        assert!(
            state_with_tui.tui_tx.is_some(),
            "tui_tx must be Some when TUI sender is set"
        );
    }

    /// S6-1-T2: `GET /ready` returns HTTP 503 when the pool is closed.
    ///
    /// A pool that has been explicitly closed rejects all queries immediately.
    /// `ready_handler` must catch the error and return 503 SERVICE_UNAVAILABLE
    /// with body `{"status":"unavailable"}`.
    #[tokio::test]
    async fn ready_returns_503_when_pool_closed() {
        let state = make_state();
        // Close the pool before the request — all subsequent queries fail immediately.
        if let Some(ref pool) = state.pg_pool {
            pool.close().await;
        }

        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "unavailable");
    }

    /// S6-1-T3: `GET /ready` returns HTTP 200 with a valid connected pool.
    ///
    /// Requires a local PostgreSQL instance at `postgresql://localhost:5432/gadgetron`.
    /// Skipped automatically when `GADGETRON_TEST_DB_URL` is not set.
    /// When running in CI with PG, set `GADGETRON_TEST_DB_URL=postgresql://localhost:5432/gadgetron`.
    #[tokio::test]
    async fn ready_returns_200_with_valid_pool() {
        let db_url = match std::env::var("GADGETRON_TEST_DB_URL") {
            Ok(url) => url,
            Err(_) => {
                // Skip: no real PG available in this environment.
                eprintln!("SKIP ready_returns_200_with_valid_pool: GADGETRON_TEST_DB_URL not set");
                return;
            }
        };

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(3))
            .connect(&db_url)
            .await
            .expect("test PG connection must succeed");

        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        let state = AppState {
            key_validator: Arc::new(NoopKeyValidator),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: Some(pool),
            no_db: false,
            tui_tx: None,
        };

        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ready");
    }

    // ------------------------------------------------------------------
    // Hotfix: 413 response shape (docs/design/polish/hotfix-error-shape-findings.md)
    // ------------------------------------------------------------------

    /// `format_body_limit` renders the runtime `MAX_BODY_BYTES` as a
    /// human-readable MiB string. Pure function; no I/O.
    #[test]
    fn format_body_limit_renders_whole_and_fractional_mib() {
        assert_eq!(format_body_limit(4_194_304), "4 MiB");
        assert_eq!(format_body_limit(8_388_608), "8 MiB");
        // 6 MiB == 6_291_456 — still whole.
        assert_eq!(format_body_limit(6_291_456), "6 MiB");
        // 4.5 MiB — fractional case uses one decimal place.
        assert_eq!(format_body_limit(4_718_592), "4.5 MiB");
    }

    /// A1: body over `MAX_BODY_BYTES` must return `413` with a JSON
    /// `Content-Type`. Regression guard for the OpenAI SDK compatibility
    /// bug found in Test 6.
    #[tokio::test]
    async fn body_too_large_returns_413_with_json_content_type() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
        let app = build_router(state);

        // Exactly one byte over the limit — cheapest trigger.
        let oversized_body = vec![b'x'; MAX_BODY_BYTES + 1];

        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(oversized_body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("413 must carry a Content-Type")
            .to_str()
            .expect("Content-Type must be ASCII");
        assert!(
            ct.starts_with("application/json"),
            "413 Content-Type must be JSON, got {ct:?}"
        );
    }

    /// A2: the 413 body must be OpenAI-shaped JSON with the correct error
    /// envelope so SDK clients can parse it. Also proves the message embeds
    /// the dynamic MiB formatter (no hard-coded "4 MB" literal).
    #[tokio::test]
    async fn body_too_large_returns_openai_shaped_json() {
        let state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
        let app = build_router(state);

        let oversized_body = vec![b'x'; MAX_BODY_BYTES + 1];

        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(oversized_body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .expect("413 body must deserialize as JSON (not plain text)");

        assert_eq!(value["error"]["code"], "request_too_large");
        assert_eq!(value["error"]["type"], "invalid_request_error");
        let msg = value["error"]["message"]
            .as_str()
            .expect("error.message must be a string");
        assert!(!msg.is_empty(), "error.message must not be empty");
        assert!(
            msg.contains("MiB"),
            "error.message must embed dynamic MiB limit (no hard-coded '4 MB'), got {msg:?}"
        );
    }
}
