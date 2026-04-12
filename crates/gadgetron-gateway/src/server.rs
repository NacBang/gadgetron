use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gadgetron_core::provider::LlmProvider;
use gadgetron_router::Router as LlmRouter;
use gadgetron_xaas::audit::writer::AuditWriter;
use gadgetron_xaas::auth::validator::KeyValidator;
use gadgetron_xaas::quota::enforcer::QuotaEnforcer;
use serde_json::json;
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};

use crate::handlers::{chat_completions_handler, list_models_handler};
use crate::middleware::{
    auth::auth_middleware, request_id::request_id_middleware, scope::scope_guard_middleware,
    tenant_context::tenant_context_middleware,
};

/// 4 MB body limit. SEC-M2 / §2.B.8 layer 1 (outermost).
/// Rationale: 128k-token window × ~4 bytes/token ≈ 512 KB; 8× headroom.
const MAX_BODY_BYTES: usize = 4_194_304;

/// Shared application state injected into every handler via `axum::State`.
///
/// All fields are `Arc`-wrapped so `Clone` is a pointer copy (~1 ns).
/// `Send + Sync` is satisfied because every inner type is already
/// `Send + Sync` (trait-object bounds are explicit).
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
/// Phase 1 placeholder: returns HTTP 200 unconditionally.
/// Phase 2 will add a `SELECT 1` ping against the PG pool stored in AppState.
pub async fn ready_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ready"})))
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
        // Auth middleware stack (layers 3–6). These all operate on `Body` (not
        // `Limited<Body>`), so they must be separate from RequestBodyLimitLayer
        // which wraps the body type and would require downstream middleware to
        // accept `Request<Limited<Body>>`.
        //
        // Layer ordering in axum: each `.layer()` call on `Router` is the NEW
        // outermost layer — it wraps all previously applied layers. Therefore we
        // call `.layer()` from innermost-to-outermost (scope first, then auth, etc.).
        //
        // Final inbound request flow:
        //   [body-limit] → [trace] → [request-id] → [auth] → [tenant-ctx] → [scope] → handler
        //
        // Layer 6 (innermost of auth stack): scope enforcement → 403 on mismatch.
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
        // Layer 1 (outermost): body size guard (4 MB). SEC-M2. Applied last so it
        // wraps all other layers and is first to intercept the incoming request.
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .with_state(state);

    let public_routes = Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler));

    Router::new()
        .merge(authenticated_routes)
        .merge(public_routes)
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
        let (audit_writer, _rx) = AuditWriter::new(16);
        AppState {
            key_validator: Arc::new(NoopKeyValidator),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
        }
    }

    fn make_state_with_validator(validator: impl KeyValidator + 'static) -> AppState {
        let (audit_writer, _rx) = AuditWriter::new(16);
        AppState {
            key_validator: Arc::new(validator),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
        }
    }

    // A valid-format token for test purposes (gad_ prefix + 16+ char suffix).
    const VALID_TOKEN: &str = "gad_live_abcdefghijklmnop1234567890";

    // ------------------------------------------------------------------
    // S3-2 required tests (preserved)
    // ------------------------------------------------------------------

    /// AppState must derive Clone (all fields are Arc — clone is O(1)).
    #[test]
    fn app_state_is_clone() {
        let state = make_state();
        let cloned = state.clone();
        // Arc pointer equality: both point to the same allocations.
        assert!(Arc::ptr_eq(&state.audit_writer, &cloned.audit_writer));
        assert!(Arc::ptr_eq(&state.providers, &cloned.providers));
    }

    /// `build_router` must compile and produce a valid `Router`.
    /// Smoke test: confirms the function signature and return type are correct.
    #[test]
    fn build_router_compiles() {
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

    /// `GET /ready` must return HTTP 200 in Phase 1.
    #[tokio::test]
    async fn ready_returns_200() {
        let state = make_state();
        let app = build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
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
}
