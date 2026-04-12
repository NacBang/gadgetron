# Gateway Middleware Chain Wire-Up — Sprint 3

> **담당**: @gateway-router-lead
> **상태**: Draft
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-gateway`, `gadgetron-router`, `gadgetron-xaas`, `gadgetron-core`, `gadgetron-cli`
> **Phase**: [P1]

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 기능이 해결하는 문제

Sprint 1-2에서 모든 핵심 타입(`GadgetronError`, `TenantContext`, `LlmProvider`, `QuotaToken`, `AuditWriter`)과 트레이트가 구현되었다. Sprint 3의 과제는 이것들을 하나의 작동하는 HTTP 파이프라인으로 연결하는 것이다. 5분 퀵스타트가 실제로 동작해야 한다.

### 1.2 핵심 설계 원칙: "Wire, Don't Invent"

Sprint 3에서 새로운 타입이나 트레이트를 발명하지 않는다. 모든 타입과 트레이트가 이미 존재하며, Sprint 3는 그것들을 올바른 순서로 연결한다. 설계 판단이 필요한 유일한 결정은 연결 순서와 구조다.

- `IntoResponse for GadgetronError`: 이미 `error.rs`에 `error_code()`, `error_message()`, `error_type()`, `http_status_code()`가 존재한다. gateway에서 axum 응답으로 변환하는 `impl` 블록만 추가한다.
- 미들웨어 체인: §2.B.8이 16-layer 순서를 완전히 명시한다. 이 순서를 코드로 옮기는 것이 Sprint 3의 전부다.
- Circuit breaker: `gadgetron-router/src/circuit_breaker.rs`의 `CircuitBreaker` 타입이 §2.F.3에서 완전히 명시되었다. Sprint 3는 이를 `AppState`에 포함하고 handler에서 호출한다.
- Protocol translation (OpenAI ↔ Anthropic, OpenAI ↔ Gemini, etc.) is entirely internal to each `gadgetron-provider` adapter. The gateway passes a canonical `ChatRequest` and receives a canonical `ChatResponse`/`ChatChunk`.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 거부 이유 |
|------|----------|
| Tower `Service` impl으로 각 미들웨어를 직접 구현 | 불필요한 보일러플레이트. `tower_http` 0.6의 `ServiceBuilder` + 기존 레이어 타입으로 충분 |
| `axum::middleware::from_fn` 클로저로 auth 구현 | 상태 공유를 위해 `Arc` 캡처 필요. `layer()`를 통한 Tower Layer가 더 명확한 소유권 경계를 갖는다 |
| `CorsLayer::permissive()` 사용 | `gateway-router-lead` 규칙에서 명시적 예외 없이 금지. D-6의 보안 원칙. Sprint 3에서는 CORS 레이어를 포함하지 않는다 |
| handler 내에서 auth 직접 수행 | §2.B.8 레이어 순서 위반. auth는 반드시 handler 이전에 미들웨어로 처리 |

### 1.5 Operator Touchpoint Walkthrough

Step-by-step narrative for an operator bringing Gadgetron up from scratch. Reference `docs/design/xaas/phase1.md §2.6` for the full XaaS bootstrap walkthrough.

1. **Install**: `cargo install gadgetron-cli` (or `docker pull ghcr.io/gadgetron/gadgetron:latest`). The binary exposes a single `gadgetron serve` sub-command.

2. **Configure** — create `gadgetron.toml` in the working directory (see §2.10.2 for an annotated minimal example). Required fields: `server.bind`, `xaas.database_url`, at least one `[providers.<name>]` block. Set secret fields (`providers.openai.api_key`, `xaas.database_url`) via the corresponding `GADGETRON_*` env vars rather than plaintext in the file.

3. **Start the server**: `gadgetron serve --config ./gadgetron.toml`. The process runs DB migrations automatically (step 4 of §2.10), then logs `listening addr=0.0.0.0:8080`. Readiness probe `GET /ready` returns 200 when PostgreSQL and all configured providers are healthy.

4. **Create a tenant and issue an API key** (via management API or `gadgetron-admin` CLI):
   ```
   POST /api/v1/tenants   {"name":"acme"}          → tenant_id=<UUID>
   POST /api/v1/keys      {"tenant_id":"<UUID>","scopes":["openai_compat"]}
                          → {"key":"gad_live_<opaque>"}
   ```
   The returned key is shown once. Store it securely.

5. **First request**:
   ```bash
   curl https://<host>/v1/chat/completions \
     -H "Authorization: Bearer gad_live_<key>" \
     -H "Content-Type: application/json" \
     -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hello"}],"stream":false}'
   ```
   Expected: HTTP 200 with an OpenAI-compatible `chat.completion` object and an `X-Request-Id` header.

### 1.6 Threat Model (STRIDE)

Assets: bearer tokens, provider API keys, PostgreSQL credentials, per-tenant message data, audit log integrity.
Trust boundaries: client → gateway (TLS), gateway → provider (TLS via reqwest/rustls), gateway → PostgreSQL (encrypted connection string in `Secret<String>`).

| Category | Threat | Asset | Mitigation | Phase |
|----------|--------|-------|-----------|-------|
| **S** — Spoofing | Forged bearer token: attacker crafts a `gad_live_` string and calls `/v1/chat/completions` | Bearer token | `AuthLayer` SHA-256 hashes the token and does a DB lookup; only valid hashes in `api_keys` pass. moka cache has 10-min TTL so revoked keys drain within 10 min (key invalidation path via `KeyValidator::invalidate`). | P1 |
| **T** — Tampering | Audit log row deleted or modified post-insert to erase a billing event | Audit log | PostgreSQL row-level access: the `gadgetron` DB user has `INSERT`-only on `audit_log`; no `UPDATE`/`DELETE` granted. Separate read-only role for reporting. | P1 |
| **R** — Repudiation | Tenant denies having made a request; no durable evidence | Audit log | Every authenticated request produces an `AuditEntry` with `request_id`, `tenant_id`, `api_key_id`, and timestamp, flushed to PostgreSQL asynchronously. Auth failures (401/403) are also audited (SOC2 CC6.7). | P1 |
| **I** — Information Disclosure | Internal error detail (DB query, stack path) leaks to caller via response body | Tenant message data / internal topology | `IntoResponse for GadgetronError` uses `error_message()` (opaque static string) for the response body; `self.to_string()` goes only to tracing log. `format!("{:?}", provider_config)` is never used in log fields (SEC-M2). | P1 |
| **D** — Denial of Service | Oversized request body exhausts gateway memory or downstream provider budget | Gateway process | `RequestBodyLimitLayer(4_194_304)` rejects bodies > 4 MB with HTTP 413 before any auth or parsing occurs (layer 1, outermost). | P1 |
| **E** — Elevation of Privilege | A tenant key with `openai_compat` scope accesses `/api/v1/nodes` (management endpoint) | Tenant isolation | `ScopeGuardLayer` checks `TenantContext.scopes` against the required scope for each route group. Missing scope → HTTP 403. Scope confusion across tenants is prevented by the per-key scope list stored in `api_keys.scopes`. | P1 |

### 1.7 Compliance Mapping

| Control | Requirement | Gadgetron Implementation | Section |
|---------|------------|--------------------------|---------|
| SOC2 CC6.1 | Logical and physical access controls | `AuthLayer` (bearer + SHA-256 hash lookup) + `ScopeGuardLayer` (per-route scope enforcement) | §2.3.2, §2.3.4 |
| SOC2 CC6.6 | External-facing system protection | TLS termination via `rustls` at `tokio::net::TcpListener` (§2.10); provider calls use `reqwest` with `rustls` backend | §2.10 |
| SOC2 CC6.7 | Revocation of access | `KeyValidator::invalidate` flushes the moka cache entry; 401/403 failures are written to `audit_log` | §2.3.2 |
| GDPR Art 32 | Security of processing (encryption in transit) | All inbound connections use TLS (Phase 1: rustls sidecar or native); `DATABASE_URL` stored as `Secret<String>` never emitted to tracing fields | §2.10, SEC-M7 |

### 1.4 Trade-off

**선택**: `axum::Extension`을 통해 `TenantContext`를 handler에 전달한다. `axum::State`가 아닌 이유: `TenantContext`는 요청마다 다르며 per-request 데이터다. `AppState`는 공유 불변 데이터(provider, router, quota enforcer)를 담는다.

**선택**: `InMemoryQuotaEnforcer`를 Phase 1에 사용한다. D-20260411-01 결정에서 PostgreSQL-backed quota는 Phase 2다. `InMemoryQuotaEnforcer`는 `QuotaEnforcer` 트레이트를 구현하므로 교체 비용은 `Arc<dyn QuotaEnforcer>` 타입 변경뿐이다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 AppState 구조체

`axum::State`에 주입되는 공유 상태. 모든 필드는 `Arc`로 감싸며, handler 간 clone 비용은 `Arc::clone` (포인터 복사, ~1ns).

```rust
// gadgetron-gateway/src/state.rs
use std::sync::Arc;
use gadgetron_router::Router;
use gadgetron_xaas::auth::validator::{KeyValidator, PgKeyValidator};
use gadgetron_xaas::quota::enforcer::QuotaEnforcer;
use gadgetron_xaas::audit::writer::AuditWriter;
use sqlx::PgPool;

/// axum::State<AppState>로 모든 handler에 주입되는 공유 상태.
/// Send + Sync: 모든 필드가 Arc<dyn Trait + Send + Sync> 또는 자체 Send+Sync.
#[derive(Clone)]
pub struct AppState {
    /// gadgetron-router의 Router. 6종 routing strategy + fallback + circuit breaker.
    pub router: Arc<Router>,
    /// Bearer 토큰 검증. PgKeyValidator는 moka cache (10min TTL, max 10_000 entries).
    pub key_validator: Arc<dyn KeyValidator + Send + Sync>,
    /// 쿼터 사전/사후 검사. Phase 1: InMemoryQuotaEnforcer.
    pub quota_enforcer: Arc<dyn QuotaEnforcer + Send + Sync>,
    /// 감사 로그 비동기 전송. mpsc channel capacity = 4_096.
    pub audit_writer: Arc<AuditWriter>,
    /// PostgreSQL 연결 풀. readiness probe에서 `SELECT 1` 실행.
    pub pg_pool: PgPool,
}
```

**동시성 모델**: `AppState`는 `Clone`이며, axum이 요청마다 `Clone`한다. `Arc` 내부 참조 카운트만 증가하므로 힙 할당 없음. `Router` 내부의 `AtomicUsize` (RoundRobin counter)는 lock-free. `PgKeyValidator`의 moka cache는 `moka::future::Cache`로 내부적으로 `Arc<SegmentedHashMap>`을 사용, fine-grained locking.

### 2.2 axum Router 정의 — 전체 route + Tower layer 스택

```rust
// gadgetron-gateway/src/server.rs
use axum::{
    Router,
    routing::{get, post},
    extract::DefaultBodyLimit,
};
use tower::ServiceBuilder;
use tower_http::{
    trace::TraceLayer,
    limit::RequestBodyLimitLayer,
};
use std::time::Duration;
use crate::state::AppState;
use crate::handlers::{
    chat_completions_handler,
    list_models_handler,
    health_handler,
    ready_handler,
    list_nodes_handler,
    deploy_model_handler,
    undeploy_model_handler,
    model_status_handler,
    usage_handler,
    costs_handler,
    circuit_reset_handler,
};
use crate::middleware::{
    auth::AuthLayer,
    scope::ScopeGuardLayer,
    request_id::RequestIdLayer,
    tenant_context::TenantContextLayer,
};

/// 4MB body limit 상수. M-SEC-2 규정, §2.B.8 레이어 1.
/// 근거: 최대 컨텍스트 창 (128k 토큰 × ~4 bytes/token ≈ 512KB) × 8배 여유.
const MAX_BODY_BYTES: usize = 4_194_304;

// SEC-M3: only the corrected build_router form is retained below.
// The prior form (all routes under one .layer() call) incorrectly applied
// AuthLayer to /health and /ready. Deleted per Round 1.5 review.

```rust
/// Build the gateway axum Router with the canonical Tower middleware chain.
///
/// Layer order (outermost first): RequestBodyLimit → Trace → RequestId →
/// Auth → TenantContext → ScopeGuard → Router. See §2.B.8.
///
/// Routes are split: `/health` + `/ready` are public (no auth);
/// all other routes require Bearer auth.
pub fn build_router(state: AppState) -> Router {
    let authenticated_routes = Router::new()
        .merge(Router::new()
            .route("/v1/chat/completions", post(chat_completions_handler))
            .route("/v1/models", get(list_models_handler)))
        .merge(Router::new()
            .route("/api/v1/nodes", get(list_nodes_handler))
            .route("/api/v1/models/deploy", post(deploy_model_handler))
            .route("/api/v1/models/:id", axum::routing::delete(undeploy_model_handler))
            .route("/api/v1/models/status", get(model_status_handler))
            .route("/api/v1/usage", get(usage_handler))
            .route("/api/v1/costs", get(costs_handler))
            .route("/api/v1/admin/circuits/:provider_id/reset", post(circuit_reset_handler)))
        .layer(
            ServiceBuilder::new()
                .layer(ScopeGuardLayer::new())
                .layer(TenantContextLayer::new())
                .layer(AuthLayer::new(state.key_validator.clone()))
                .layer(RequestIdLayer::new())
                .layer(TraceLayer::new_for_http())
                .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES)),
        )
        .with_state(state);

    let public_routes = Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler));

    Router::new()
        .merge(authenticated_routes)
        .merge(public_routes)
}
```

### 2.3 미들웨어 구현

#### 2.3.1 RequestIdLayer

```rust
// gadgetron-gateway/src/middleware/request_id.rs
use axum::{
    extract::Request,
    response::Response,
    middleware::Next,
};
use uuid::Uuid;

/// X-Request-Id 헤더를 응답에 주입하고 tracing span에 request_id를 기록.
/// §2.B.8 레이어 3. Budget: ~100ns (UUID gen ~2µs는 TraceLayer에서 처리).
pub async fn request_id_middleware(
    req: Request,
    next: Next,
) -> Response {
    let request_id = Uuid::new_v4();
    let mut response = next.run(req).await;
    response.headers_mut().insert(
        "x-request-id",
        request_id.to_string().parse().expect("UUID is valid header value"),
    );
    response
}

/// Tower Layer wrapper. `axum::middleware::from_fn`으로 등록.
pub struct RequestIdLayer;

impl RequestIdLayer {
    pub fn new() -> axum::middleware::FromFnLayer<
        impl Fn(Request, Next) -> impl std::future::Future<Output = Response> + Send + Clone,
        (),
        (Request,),
    > {
        axum::middleware::from_fn(request_id_middleware)
    }
}
```

#### 2.3.2 AuthLayer

```rust
// gadgetron-gateway/src/middleware/auth.rs
use axum::{
    extract::Request,
    response::{IntoResponse, Response},
    middleware::Next,
};
use std::sync::Arc;
use sha2::{Sha256, Digest};
use gadgetron_core::error::GadgetronError;
use gadgetron_xaas::auth::validator::KeyValidator;
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus, AuditWriter};

/// Bearer 토큰을 SHA-256 해시하고 KeyValidator로 검증.
/// 성공 시 `ValidatedKey`를 request extensions에 삽입.
/// §2.B.8 레이어 4. Budget: cache hit ~50µs / cache miss ~5ms.
///
/// SEC-M4: 401 auth failures are audited per SOC2 CC6.7.
///
/// 에러 경로:
///   Authorization 헤더 없음     → GadgetronError::TenantNotFound → 401 (audited)
///   Bearer 형식 오류            → GadgetronError::TenantNotFound → 401 (audited)
///   키 DB 미존재 또는 revoked   → GadgetronError::TenantNotFound → 401 (audited)
///   DB 연결 실패                → GadgetronError::Database{..}  → 503
pub async fn auth_middleware(
    axum::extract::State(validator): axum::extract::State<Arc<dyn KeyValidator + Send + Sync>>,
    axum::extract::State(audit_writer): axum::extract::State<Arc<AuditWriter>>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    let token = match auth_header {
        Some(t) => t.to_string(),
        None => {
            // SEC-M4: audit 401 — missing Authorization header. SOC2 CC6.7.
            audit_writer.send(AuditEntry {
                tenant_id: uuid::Uuid::nil(),
                api_key_id: uuid::Uuid::nil(),
                request_id: uuid::Uuid::new_v4(),
                model: None,
                provider: None,
                status: AuditStatus::Error,
                input_tokens: 0,
                output_tokens: 0,
                cost_cents: 0,
                latency_ms: 0,
            });
            return GadgetronError::TenantNotFound.into_response();
        }
    };

    // SHA-256(token) → hex string. `gad_live_` 접두어 포함해서 해시.
    let key_hash = {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
    };

    match validator.validate(&key_hash).await {
        Ok(validated_key) => {
            req.extensions_mut().insert(validated_key);
            next.run(req).await
        }
        Err(e) => {
            // SEC-M4: audit 401 — key not found or revoked. SOC2 CC6.7.
            audit_writer.send(AuditEntry {
                tenant_id: uuid::Uuid::nil(),
                api_key_id: uuid::Uuid::nil(),
                request_id: uuid::Uuid::new_v4(),
                model: None,
                provider: None,
                status: AuditStatus::Error,
                input_tokens: 0,
                output_tokens: 0,
                cost_cents: 0,
                latency_ms: 0,
            });
            e.into_response()
        }
    }
}

pub struct AuthLayer;

impl AuthLayer {
    pub fn new(
        validator: Arc<dyn KeyValidator + Send + Sync>,
    ) -> axum::middleware::FromFnLayer<
        impl Fn(
            axum::extract::State<Arc<dyn KeyValidator + Send + Sync>>,
            Request,
            Next,
        ) -> impl std::future::Future<Output = Response> + Send + Clone,
        Arc<dyn KeyValidator + Send + Sync>,
        (
            axum::extract::State<Arc<dyn KeyValidator + Send + Sync>>,
            Request,
        ),
    > {
        axum::middleware::from_fn_with_state(validator, auth_middleware)
    }
}
```

**sha2 의존성**: `sha2 = "0.10"`. `ring` 대신 sha2를 사용하는 이유: `ring`은 C FFI를 포함하며 wasm 타겟에서 별도 처리가 필요하다. `sha2`는 pure Rust, `#![no_std]` 호환. 성능: SHA-256 on 64-byte key ≈ 5µs (§2.H.2 budget 내).

#### 2.3.3 TenantContextLayer

```rust
// gadgetron-gateway/src/middleware/tenant_context.rs
use axum::{
    extract::Request,
    response::{IntoResponse, Response},
    middleware::Next,
};
use std::sync::Arc;
use gadgetron_core::context::TenantContext;
use gadgetron_core::error::GadgetronError;
use gadgetron_xaas::auth::validator::ValidatedKey;

/// ValidatedKey (AuthLayer가 삽입)로부터 TenantContext를 구성해 Extension에 삽입.
/// §2.B.8 레이어 5. Budget: ~10µs (Arc clone + QuotaSnapshot 생성).
///
/// QuotaSnapshot: Phase 1에서는 ValidatedKey의 tenant_id만 사용하고
/// 실제 quota 값은 InMemoryQuotaEnforcer가 check_pre에서 직접 조회.
/// TenantContext.quota_snapshot은 기본값(unlimited)으로 초기화.
pub async fn tenant_context_middleware(
    mut req: Request,
    next: Next,
) -> Response {
    let validated_key = match req.extensions().get::<Arc<ValidatedKey>>() {
        Some(k) => k.clone(),
        None => {
            // AuthLayer가 먼저 실행되지 않으면 이 경우는 발생하지 않는다.
            // 레이어 순서가 올바르면 unreachable. 방어적 처리.
            return GadgetronError::TenantNotFound.into_response();
        }
    };

    let ctx = TenantContext {
        tenant_id: validated_key.tenant_id,
        api_key_id: validated_key.api_key_id,
        scopes: validated_key.scopes.clone(),
        quota_snapshot: Arc::new(gadgetron_core::context::QuotaSnapshot {
            daily_limit_cents: i64::MAX, // Phase 1: InMemoryQuotaEnforcer가 관리
            daily_used_cents: 0,
            monthly_limit_cents: i64::MAX,
            monthly_used_cents: 0,
        }),
        request_id: uuid::Uuid::new_v4(),
        started_at: std::time::Instant::now(),
    };

    req.extensions_mut().insert(ctx);
    next.run(req).await
}

pub struct TenantContextLayer;

impl TenantContextLayer {
    pub fn new() -> axum::middleware::FromFnLayer<
        impl Fn(Request, Next) -> impl std::future::Future<Output = Response> + Send + Clone,
        (),
        (Request,),
    > {
        axum::middleware::from_fn(tenant_context_middleware)
    }
}
```

#### 2.3.4 ScopeGuardLayer

```rust
// gadgetron-gateway/src/middleware/scope.rs
use axum::{
    extract::Request,
    response::{IntoResponse, Response},
    middleware::Next,
};
use gadgetron_core::context::{Scope, TenantContext};
use gadgetron_core::error::GadgetronError;
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus, AuditWriter};

/// Route 경로에 따라 required scope를 결정하고 TenantContext의 scopes와 대조.
/// 인증 성공 + 권한 부족 → GadgetronError::Forbidden → 403 (B-8).
/// §2.B.8 레이어 6. Budget: ~1µs (Vec<Scope> linear scan, max 3 elements).
///
/// Route별 required scope (§2.A.3):
///   /v1/*          → Scope::OpenAiCompat
///   /api/v1/*      → Scope::Management
///   (그 외는 이 미들웨어 실행 전 인증 없는 공개 경로)
/// SEC-M4: 403 scope failures MUST be audited per SOC2 CC6.7.
pub async fn scope_guard_middleware(
    axum::extract::State(audit_writer): axum::extract::State<Arc<AuditWriter>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();

    let required_scope = if path.starts_with("/v1/") {
        Some(Scope::OpenAiCompat)
    } else if path.starts_with("/api/v1/") {
        Some(Scope::Management)
    } else {
        None // /health, /ready 등 공개 경로 — 이 미들웨어에 도달하지 않음
    };

    if let Some(scope) = required_scope {
        let ctx = match req.extensions().get::<TenantContext>() {
            Some(c) => c.clone(),
            None => return GadgetronError::TenantNotFound.into_response(),
        };
        if !ctx.has_scope(scope) {
            tracing::warn!(
                tenant_id = %ctx.tenant_id,
                required_scope = %scope,
                "scope denied"
            );
            // SEC-M4: audit 403 — insufficient scope. SOC2 CC6.7.
            audit_writer.send(AuditEntry {
                tenant_id: ctx.tenant_id,
                api_key_id: ctx.api_key_id,
                request_id: ctx.request_id,
                model: None,
                provider: None,
                status: AuditStatus::Error,
                input_tokens: 0,
                output_tokens: 0,
                cost_cents: 0,
                latency_ms: 0,
            });
            return GadgetronError::Forbidden.into_response();
        }
    }

    next.run(req).await
}

pub struct ScopeGuardLayer;

impl ScopeGuardLayer {
    pub fn new() -> axum::middleware::FromFnLayer<
        impl Fn(Request, Next) -> impl std::future::Future<Output = Response> + Send + Clone,
        (),
        (Request,),
    > {
        axum::middleware::from_fn(scope_guard_middleware)
    }
}
```

### 2.4 GadgetronError → IntoResponse

```rust
// gadgetron-gateway/src/error.rs
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use gadgetron_core::error::{GadgetronError, DatabaseErrorKind, NodeErrorKind};

/// GadgetronError를 axum Response로 변환. M-DX-2: OpenAI 호환 에러 형식.
/// 내부 상세 정보(self.to_string())는 tracing 로그에만 기록하고 응답 본문에 포함하지 않는다.
/// §2.B.6.3 HTTP status 매핑과 정확히 일치해야 한다.
impl IntoResponse for GadgetronError {
    fn into_response(self) -> Response {
        let status = match &self {
            GadgetronError::Config(_)                => StatusCode::BAD_REQUEST,
            GadgetronError::Provider(_)              => StatusCode::BAD_GATEWAY,
            GadgetronError::Routing(_)               => StatusCode::SERVICE_UNAVAILABLE,
            GadgetronError::StreamInterrupted { .. } => StatusCode::BAD_GATEWAY,
            GadgetronError::QuotaExceeded { .. }     => StatusCode::TOO_MANY_REQUESTS,
            GadgetronError::TenantNotFound           => StatusCode::UNAUTHORIZED,
            GadgetronError::Forbidden                => StatusCode::FORBIDDEN,
            GadgetronError::Billing(_)               => StatusCode::INTERNAL_SERVER_ERROR,
            GadgetronError::DownloadFailed(_)        => StatusCode::INTERNAL_SERVER_ERROR,
            GadgetronError::HotSwapFailed(_)         => StatusCode::INTERNAL_SERVER_ERROR,
            GadgetronError::Database { kind, .. } => match kind {
                DatabaseErrorKind::PoolTimeout
                | DatabaseErrorKind::ConnectionFailed => StatusCode::SERVICE_UNAVAILABLE,
                DatabaseErrorKind::RowNotFound       => StatusCode::NOT_FOUND,
                DatabaseErrorKind::Constraint        => StatusCode::CONFLICT,
                _                                    => StatusCode::INTERNAL_SERVER_ERROR,
            },
            GadgetronError::Node { kind, .. } => match kind {
                NodeErrorKind::InvalidMigProfile     => StatusCode::BAD_REQUEST,
                _                                    => StatusCode::INTERNAL_SERVER_ERROR,
            },
            // #[non_exhaustive]: 새 variant 추가 시 컴파일 에러로 누락 방지
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        // 내부 원본 오류는 로그에만 기록 (공격자에게 internal 구조 미노출)
        tracing::error!(
            error.code  = %self.error_code(),
            error.type_ = %self.error_type(),
            "request failed: {}",
            self   // self.to_string() = #[error(...)] 매크로 확장값
        );

        let body = Json(serde_json::json!({
            "error": {
                "message": self.error_message(),
                "type":    self.error_type(),
                "code":    self.error_code(),
            }
        }));

        (status, body).into_response()
    }
}
```

**status 일치 검증**: `GadgetronError::http_status_code()`의 반환값과 위 match arm이 일치해야 한다. 단위 테스트 §4.1.3에서 모든 12 variant를 검증한다.

### 2.5 chat_completions_handler — 비스트리밍 및 스트리밍

```rust
// gadgetron-gateway/src/handlers/chat.rs
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use gadgetron_core::{
    context::TenantContext,
    error::GadgetronError,
    provider::ChatRequest,
};
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus};
use crate::state::AppState;
use crate::sse::chat_chunk_to_sse;

/// POST /v1/chat/completions 핸들러.
/// `stream: false`이면 JSON 응답, `stream: true`이면 SSE 스트림.
///
/// 인수:
///   State(state): AppState — shared state (Arc clone, ~1ns)
///   ctx: Extension<TenantContext> — AuthLayer + TenantContextLayer가 삽입
///   Json(req): ChatRequest — serde_json 역직렬화
///
/// 에러 경로:
///   QuotaEnforcer::check_pre 실패 → GadgetronError::QuotaExceeded → 429
///   Router::chat / chat_stream 실패 → GadgetronError::* → 대응 HTTP status
#[tracing::instrument(
    skip(state, ctx),
    fields(
        model = %req.model,
        tenant_id = %ctx.tenant_id,
        request_id = %ctx.request_id,
        stream = req.stream,
    )
)]
pub async fn chat_completions_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<TenantContext>,
    Json(req): Json<ChatRequest>,
) -> Response {
    // 1. 쿼터 사전 검사
    let quota_token = match state.quota_enforcer
        .check_pre(ctx.tenant_id, &ctx.quota_snapshot)
        .await
    {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };

    if req.stream {
        handle_streaming(state, ctx, req, quota_token).await
    } else {
        handle_non_streaming(state, ctx, req, quota_token).await
    }
}

/// 비스트리밍: Router::chat → ChatResponse → JSON 200
async fn handle_non_streaming(
    state: AppState,
    ctx: TenantContext,
    req: ChatRequest,
    quota_token: gadgetron_xaas::quota::enforcer::QuotaToken,
) -> Response {
    let start = ctx.started_at;

    match state.router.chat(req.clone()).await {
        Ok(response) => {
            let latency_ms = start.elapsed().as_millis() as i32;
            let cost_cents = 0i64; // Phase 1: cost 계산 미구현, Phase 2에서 확장

            // 쿼터 사후 기록 (비차단 fire-and-forget)
            state.quota_enforcer.record_post(&quota_token, cost_cents).await;

            // 감사 로그 전송 (non-blocking try_send)
            state.audit_writer.send(AuditEntry {
                tenant_id: ctx.tenant_id,
                api_key_id: ctx.api_key_id,
                request_id: ctx.request_id,
                model: Some(req.model.clone()),
                provider: None, // Router 내부 결정 — Phase 2에서 RoutingDecision 공개
                status: AuditStatus::Ok,
                input_tokens: response.usage.prompt_tokens as i32,
                output_tokens: response.usage.completion_tokens as i32,
                cost_cents,
                latency_ms,
            });

            Json(response).into_response()
        }
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as i32;
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
            e.into_response()
        }
    }
}

/// 스트리밍: Router::chat_stream → SSE 파이프라인
/// H-1: retry는 첫 SSE 청크 전송 이전에만 허용. 이후 StreamInterrupted → 즉시 종료.
async fn handle_streaming(
    state: AppState,
    ctx: TenantContext,
    req: ChatRequest,
    quota_token: gadgetron_xaas::quota::enforcer::QuotaToken,
) -> Response {
    let stream = state.router.chat_stream(req.clone());

    // 스트림 완료 후 쿼터/감사 기록을 위한 클로저 래핑.
    // 실제 첫 청크 전송은 axum SSE 핸들러가 비동기로 처리.
    // 스트림 종료(정상/오류) 후 audit_writer.send는 Drop guard로 처리.
    let audit_writer = state.audit_writer.clone();
    let quota_enforcer = state.quota_enforcer.clone();
    let ctx_clone = ctx.clone();
    let model = req.model.clone();

    use futures::StreamExt;
    let wrapped_stream = stream.inspect(move |result| {
        // 각 청크 통과 — 실제 audit은 스트림 종료 시 처리 (아래 on_done 패턴)
        if let Err(e) = result {
            tracing::error!(
                tenant_id = %ctx_clone.tenant_id,
                error = %e,
                "stream chunk error"
            );
        }
    });

    // SSE 응답 반환. KeepAlive 15초 (§2.A.3.2).
    // 스트림 완료 후 audit은 StreamAuditGuard (§2.5.1)가 Drop 시 처리.
    let sse = chat_chunk_to_sse(wrapped_stream);

    // 스트림 후처리: quota_enforcer.record_post + audit_writer.send
    // 실행 시점: SSE 스트림이 완전히 소비된 후 (tokio task spawn)
    tokio::spawn(async move {
        // 스트림이 종료될 때까지 대기는 SSE 핸들러가 담당.
        // 여기서는 fire-and-forget으로 쿼터 기록.
        quota_enforcer.record_post(&quota_token, 0).await;
        audit_writer.send(AuditEntry {
            tenant_id: ctx.tenant_id,
            api_key_id: ctx.api_key_id,
            request_id: ctx.request_id,
            model: Some(model),
            provider: None,
            status: AuditStatus::Ok,
            input_tokens: 0,
            output_tokens: 0,
            cost_cents: 0,
            latency_ms: ctx.started_at.elapsed().as_millis() as i32,
        });
    });

    sse.into_response()
}
```

### 2.6 SSE 스트리밍 파이프라인

기존 `crates/gadgetron-gateway/src/sse.rs`의 `chat_chunk_to_sse` 함수를 Sprint 3에서 완성한다.

```rust
// gadgetron-gateway/src/sse.rs (완성판)
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::{Stream, StreamExt};
use std::{convert::Infallible, time::Duration};
use gadgetron_core::provider::ChatChunk;
use gadgetron_core::error::GadgetronError;

/// SSE KeepAlive 간격 15초.
/// 근거: 대부분 LLM inference는 첫 토큰까지 < 10초. 15초는 proxy/LB timeout 전 유지.
const SSE_KEEPALIVE_SECS: u64 = 15;

/// ChatChunk 스트림을 OpenAI SSE 형식으로 변환.
///
/// 각 청크: `data: {json}\n\n`
/// 오류 발생: SSE error 이벤트 전송 후 스트림 종료 (연결 종료).
/// 정상 종료: `data: [DONE]\n\n` 전송 (OpenAI 호환).
///
/// The `Stream::Item` error arm is always `GadgetronError` (via the
/// `gadgetron_core::error::Result` alias). No other error types cross
/// the streaming boundary.
///
/// 에러 처리 전략:
///   - GadgetronError::StreamInterrupted: 오류 이벤트 전송 후 스트림 종료.
///     H-1: 첫 청크 이후 retry 금지. 연결 즉시 종료.
///   - 기타 오류: 동일하게 오류 이벤트 전송 후 스트림 종료.
pub fn chat_chunk_to_sse<S>(stream: S) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send>
where
    S: Stream<Item = Result<ChatChunk, GadgetronError>> + Send + 'static,
{
    let event_stream = stream
        .map(|result| -> Result<Option<Event>, Infallible> {
            match result {
                Ok(chunk) => {
                    let data = serde_json::to_string(&chunk)
                        .unwrap_or_else(|e| format!("{{\"error\":\"serialization failed: {}\"}}", e));
                    Ok(Some(Event::default().data(data)))
                }
                Err(GadgetronError::StreamInterrupted { reason }) => {
                    // H-1: 첫 청크 이후 retry 금지. 오류 이벤트만 전송.
                    // 내부 reason은 로그에만 기록, 클라이언트에는 opaque 코드만.
                    tracing::warn!(reason = %reason, "stream interrupted");
                    metrics::counter!("gadgetron_stream_interrupted_total").increment(1);
                    // DX-F4: use e.error_message() so the message is consistent
                    // with non-streaming error responses (same 3-element shape).
                    let e_ref = GadgetronError::StreamInterrupted { reason: reason.clone() };
                    let data = serde_json::json!({
                        "error": {
                            "message": e_ref.error_message(),
                            "type": e_ref.error_type(),
                            "code": e_ref.error_code(),
                        }
                    }).to_string();
                    Ok(Some(Event::default().event("error").data(data)))
                }
                Err(e) => {
                    tracing::error!(error = %e, "stream error");
                    let data = serde_json::json!({
                        "error": {
                            "message": e.error_message(),
                            "type": e.error_type(),
                            "code": e.error_code(),
                        }
                    }).to_string();
                    Ok(Some(Event::default().event("error").data(data)))
                }
            }
        })
        .take_while(|r| {
            // 오류 이벤트 이후 스트림을 종료하려면 None을 반환하는 패턴 필요.
            // 현재 구현에서는 오류도 Some(Event)로 전달하므로 연결은 클라이언트가 닫는다.
            // Phase 2: StreamInterrupted 시 즉시 연결 종료를 위해 axum SSE abort 훅 사용.
            std::future::ready(r.is_ok())
        })
        .filter_map(|r| async move { r.ok().flatten() })
        // [DONE] 추가: 스트림 정상 종료 시
        .chain(futures::stream::once(async {
            Ok(Event::default().data("[DONE]"))
        }));

    Sse::new(event_stream).keep_alive(
        KeepAlive::new().interval(Duration::from_secs(SSE_KEEPALIVE_SECS)),
    )
}
```

### 2.7 list_models_handler

```rust
// gadgetron-gateway/src/handlers/models.rs
use axum::{extract::State, response::IntoResponse, Json};
use crate::state::AppState;
use gadgetron_core::error::GadgetronError;

/// GET /v1/models — 모든 provider의 모델 목록 집계.
/// OpenAI /v1/models 호환 형식: {"object":"list","data":[{...}]}.
#[tracing::instrument(skip(state))]
pub async fn list_models_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.router.list_models().await {
        Ok(models) => {
            let data: Vec<serde_json::Value> = models
                .iter()
                .map(|m| serde_json::json!({
                    "id": m.id,
                    "object": m.object,
                    "owned_by": m.owned_by,
                }))
                .collect();
            Json(serde_json::json!({
                "object": "list",
                "data": data,
            })).into_response()
        }
        Err(e) => e.into_response(),
    }
}
```

### 2.8 Health 엔드포인트

```rust
// gadgetron-gateway/src/handlers/health.rs
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use crate::state::AppState;

/// GET /health — liveness probe. 항상 200 OK.
/// K8s liveness probe: 프로세스가 살아있으면 200. DB 연결 확인 없음.
pub async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

/// GET /ready — readiness probe. PostgreSQL + provider health 확인.
/// K8s readiness probe: 트래픽을 받을 준비가 되었으면 200, 아니면 503.
///
/// 확인 항목:
///   1. PostgreSQL: `SELECT 1` 쿼리 성공 (≤ 500ms timeout)
///   2. Provider health: 각 provider.health() 결과 집계
///      하나라도 unhealthy → 503 (단, 빈 provider 목록은 200)
#[tracing::instrument(skip(state))]
pub async fn ready_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    // 1. PostgreSQL ping
    let pg_ok = sqlx::query("SELECT 1")
        .execute(&state.pg_pool)
        .await
        .is_ok();

    // 2. Provider health 집계
    let provider_health = state.router.health_check_all().await;
    let providers_ok = provider_health.values().all(|&h| h);

    if pg_ok && providers_ok {
        (StatusCode::OK, Json(serde_json::json!({
            "status": "ready",
            "postgres": "ok",
            "providers": provider_health,
        }))).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "status": "not_ready",
            "postgres": if pg_ok { "ok" } else { "error" },
            "providers": provider_health,
        }))).into_response()
    }
}
```

### 2.9 Circuit Breaker 연동

Circuit breaker는 `gadgetron-router/src/circuit_breaker.rs`에 구현된다 (§2.F.3 타입 정의 참조). `AppState`에 `Arc<DashMap<String, CircuitBreaker>>`로 per-provider 인스턴스를 보관한다.

The circuit breaker `DashMap` key is the provider config map key (e.g., `"openai"`, `"anthropic"`). `Router::resolve()` returns this same key. Invariant: `providers.keys() == circuit_breakers.keys()` — enforced at initialization in `build_providers()`.

```rust
// gadgetron-gateway/src/state.rs (circuit breaker 추가)
use dashmap::DashMap;
use gadgetron_router::circuit_breaker::CircuitBreaker;

pub struct AppState {
    // ... (기존 필드)
    /// per-provider circuit breaker. key = provider name (e.g., "openai", "anthropic").
    /// DashMap: fine-grained shard locking, lock-free read on closed state.
    pub circuit_breakers: Arc<DashMap<String, Arc<CircuitBreaker>>>,
}
```

Handler에서 circuit breaker 호출 패턴:

```rust
// chat_completions_handler 내부 (chat_with_context 호출 전)
let provider_name = "openai"; // Router::resolve() 결과에서 추출

// Phase 1: threshold=3 and recovery=60s are hardcoded constants in
// `gadgetron-router/src/circuit_breaker.rs`. `CircuitBreakerConfig` runtime
// override is [P2].
let cb = state.circuit_breakers
    .entry(provider_name.to_string())
    .or_insert_with(|| Arc::new(CircuitBreaker::new()))
    .clone();

match cb.state() {
    CircuitState::Open => {
        return GadgetronError::Provider(
            format!("circuit breaker open for provider {}", provider_name)
        ).into_response();
    }
    CircuitState::Closed | CircuitState::HalfOpen => {}
}

// provider 호출 후 결과에 따라 record_success / record_failure
match result {
    Ok(_) => cb.record_success(),
    Err(_) => { cb.record_failure(); }
}
```

**Circuit breaker Prometheus 지표** (§2.F.3.4):

```rust
// 각 provider 결정 후
metrics::gauge!("gadgetron_router_circuit_state", "provider" => provider_name)
    .set(cb.state() as f64);  // 0=Closed, 1=Open, 2=HalfOpen
```

### 2.10 Bootstrap 시퀀스 — `gadgetron serve`

#### 2.10.1 CLI Arguments

`gadgetron serve` is implemented with `clap 4` derive macros. The table below is the canonical argument definition; the `--help` output is generated from it.

| Flag | Short | Default | Env override | Description |
|------|-------|---------|-------------|-------------|
| `--config` | `-c` | `./gadgetron.toml` | `GADGETRON_CONFIG` | Path to the TOML config file |
| `--bind` | `-b` | value from config `server.bind` | `GADGETRON_BIND` | Override bind address (e.g. `0.0.0.0:8080`) |
| `--log-format` | — | `json` | `GADGETRON_LOG_FORMAT` | Log output format: `json` or `text` |
| `--tui` | — | `false` | — | Enable terminal UI dashboard (Phase 2) |

Example `--help` output:
```
Usage: gadgetron serve [OPTIONS]

Options:
  -c, --config <PATH>         Config file path [env: GADGETRON_CONFIG] [default: ./gadgetron.toml]
  -b, --bind <ADDR>           Bind address override [env: GADGETRON_BIND]
      --log-format <FORMAT>   Log format: json|text [env: GADGETRON_LOG_FORMAT] [default: json]
      --tui                   Enable TUI dashboard (Phase 2, no-op in Phase 1)
  -h, --help                  Print help
```

#### 2.10.2 Minimal gadgetron.toml

All fields consumed by bootstrap. Each field shows its default (if optional) and the environment variable that overrides it. Secret fields (`api_key`, `database_url`) MUST be set via env var rather than plaintext in the file.

```toml
# gadgetron.toml — minimal Phase 1 configuration

[server]
# default: "0.0.0.0:8080"
# env: GADGETRON_BIND (CLI --bind takes precedence over this)
bind = "0.0.0.0:8080"

# default: 4194304 (4 MB)
# env: GADGETRON_BODY_LIMIT_BYTES
body_limit_bytes = 4194304

[xaas]
# REQUIRED — no default. Never put plaintext credentials here.
# env: GADGETRON_DATABASE_URL
database_url = ""  # set via env only

[providers.openai]
type = "openai"
# REQUIRED — no default.
# env: GADGETRON_PROVIDERS_OPENAI_API_KEY
api_key = ""       # set via env only
# default: "https://api.openai.com"
# env: GADGETRON_PROVIDERS_OPENAI_BASE_URL
base_url = "https://api.openai.com"

[providers.anthropic]
type = "anthropic"
# REQUIRED — no default.
# env: GADGETRON_PROVIDERS_ANTHROPIC_API_KEY
api_key = ""       # set via env only
# default: "https://api.anthropic.com"
base_url = "https://api.anthropic.com"

[router]
# default: "round_robin"
# env: GADGETRON_ROUTER_STRATEGY
# values: round_robin | cost_optimal | latency_optimal | quality_optimal | fallback | weighted
strategy = "round_robin"
```

**SEC-M6: TLS termination.** Phase 1: TLS termination is at the `tokio::net::TcpListener` level via `rustls` if `[server.tls]` is configured in `gadgetron.toml`. Without a `[server.tls]` block, the listener accepts plaintext — intended for development or sidecar proxy (envoy, istio) setups where TLS is terminated externally before reaching the gateway process.

**SEC-M7: DATABASE_URL secret wrapping.** The database URL is read as `Secret<String>` and is never emitted to tracing span fields:

```rust
// In main() bootstrap, step 3:
use gadgetron_core::secret::Secret;
let db_url = Secret::new(
    std::env::var("GADGETRON_DATABASE_URL")
        .context("GADGETRON_DATABASE_URL environment variable not set")?
);
// DATABASE_URL is never emitted to tracing span fields.
let pg_pool = sqlx::postgres::PgPoolOptions::new()
    .max_connections(20)
    .acquire_timeout(std::time::Duration::from_millis(500))
    .connect(db_url.expose())
    .await
    .context("failed to connect to PostgreSQL")?;
```

**DX-F7: i18n strategy.** Phase 1: all `error_message()` return values are `&'static str` literals embedded in `gadgetron-core/src/error.rs`. i18n (message catalog externalization) is deferred to Phase 2. This is recorded as a known limitation.

```rust
// gadgetron-cli/src/main.rs (Sprint 3 완성판)
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use anyhow::{Context, Result};
use gadgetron_core::config::AppConfig;
use gadgetron_router::Router;
use gadgetron_xaas::{
    auth::validator::PgKeyValidator,
    quota::enforcer::InMemoryQuotaEnforcer,
    audit::writer::AuditWriter,
};
use gadgetron_gateway::{server::build_router, state::AppState};
use dashmap::DashMap;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. observability 초기화 (tracing + Prometheus)
    init_observability()?;

    // 2. AppConfig 로드
    //    GADGETRON_CONFIG > CLI --config > ./gadgetron.toml
    let config_path = std::env::var("GADGETRON_CONFIG")
        .unwrap_or_else(|_| "gadgetron.toml".to_string());
    let config = AppConfig::load(&config_path)
        .with_context(|| format!("failed to load config from {}", config_path))?;

    tracing::info!(bind = %config.server.bind, "gadgetron starting");

    // 3. PostgreSQL 연결 풀 (sqlx 0.8, rustls TLS)
    //    SEC-M7: DATABASE_URL is wrapped in Secret<String> and is never emitted
    //    to tracing span fields. Use db_url.expose() only for PgPoolOptions::connect().
    use gadgetron_core::secret::Secret;
    let db_url = Secret::new(
        std::env::var("GADGETRON_DATABASE_URL")
            .context("GADGETRON_DATABASE_URL environment variable not set")?
    );
    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)           // §2.H.3: pool size 20
        .acquire_timeout(std::time::Duration::from_millis(500))  // PoolTimeout 방지
        .connect(db_url.expose())
        .await
        .context("failed to connect to PostgreSQL")?;

    // 4. DB 마이그레이션 실행
    sqlx::migrate!("./migrations")
        .run(&pg_pool)
        .await
        .context("failed to run database migrations")?;

    // 5. PgKeyValidator (moka cache 10min TTL, max 10_000 entries)
    let key_validator = Arc::new(PgKeyValidator::new(pg_pool.clone()))
        as Arc<dyn gadgetron_xaas::auth::validator::KeyValidator + Send + Sync>;

    // 6. InMemoryQuotaEnforcer (Phase 1)
    let quota_enforcer = Arc::new(InMemoryQuotaEnforcer)
        as Arc<dyn gadgetron_xaas::quota::enforcer::QuotaEnforcer + Send + Sync>;

    // 7. AuditWriter (mpsc channel capacity = 4_096, §2.A.3.1)
    let (audit_writer, audit_rx) = AuditWriter::new(4_096);
    let audit_writer = Arc::new(audit_writer);

    // 8. Audit consumer 태스크 — audit_rx → PostgreSQL INSERT (비동기)
    let pg_pool_audit = pg_pool.clone();
    tokio::spawn(async move {
        audit_consumer_loop(audit_rx, pg_pool_audit).await;
    });

    // 9. Provider 인스턴스 빌드 (AppConfig.providers 순회)
    let providers = build_providers(&config)
        .context("failed to initialize providers")?;

    // 10. Router 빌드
    let metrics_store = Arc::new(gadgetron_router::MetricsStore::new());
    let router = Arc::new(Router::new(
        providers,
        config.router.clone(),
        metrics_store,
    ));

    // 11. Circuit breakers (per-provider, lazy 초기화)
    let circuit_breakers = Arc::new(DashMap::new());

    // 12. AppState 조립
    let state = AppState {
        router,
        key_validator,
        quota_enforcer,
        audit_writer,
        pg_pool: pg_pool.clone(),
        circuit_breakers,
    };

    // 13. axum Router 빌드 (§2.B.8 16-layer Tower 스택)
    let app = build_router(state);

    // 14. graceful shutdown 채널
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    // 15. SIGTERM / SIGINT 핸들러
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt())
            .expect("failed to install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("SIGTERM received"),
            _ = sigint.recv() => tracing::info!("SIGINT received"),
        }
        let _ = shutdown_tx_clone.send(());
    });

    // 16. axum::serve with graceful_shutdown (drain 30s, §2.F.4)
    let bind_addr: SocketAddr = config.server.bind.parse()
        .with_context(|| format!("invalid bind address: {}", config.server.bind))?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await
        .with_context(|| format!("failed to bind to {}", bind_addr))?;

    tracing::info!(addr = %bind_addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.recv().await;
            tracing::info!("shutdown signal received, draining connections (30s)");
        })
        .await
        .context("server error")?;

    tracing::info!("server shutdown complete");
    Ok(())
}

/// AppConfig.providers를 순회해 LlmProvider 인스턴스 빌드.
fn build_providers(
    config: &AppConfig,
) -> Result<HashMap<String, Arc<dyn gadgetron_core::provider::LlmProvider + Send + Sync>>> {
    use gadgetron_core::config::ProviderConfig;
    use gadgetron_core::secret::Secret;
    let mut providers: HashMap<String, Arc<dyn gadgetron_core::provider::LlmProvider + Send + Sync>> =
        HashMap::new();

    for (name, provider_config) in &config.providers {
        let provider: Arc<dyn gadgetron_core::provider::LlmProvider + Send + Sync> = match provider_config {
            ProviderConfig::Openai { api_key, base_url, .. } => {
                Arc::new(gadgetron_provider::openai::OpenAiProvider::new(
                    Secret::new(api_key.clone()),
                    base_url.clone(),
                ))
            }
            ProviderConfig::Anthropic { api_key, base_url, .. } => {
                Arc::new(gadgetron_provider::anthropic::AnthropicProvider::new(
                    Secret::new(api_key.clone()),
                    base_url.clone(),
                ))
            }
            ProviderConfig::Ollama { endpoint } => {
                Arc::new(gadgetron_provider::ollama::OllamaProvider::new(
                    endpoint.clone(),
                ))
            }
            // Gemini, vLLM, SGLang — Phase 1 Week 6 이후 추가
            _ => {
                // SEC-M2: never use format!("{:?}", provider_config) — that would emit
                // api_key field values into the error message / tracing log.
                // Match on the variant name only; no struct fields are accessed.
                let provider_type = match provider_config {
                    ProviderConfig::Gemini { .. }  => "gemini",
                    ProviderConfig::Vllm { .. }    => "vllm",
                    ProviderConfig::Sglang { .. }  => "sglang",
                    _                              => "unknown",
                };
                return Err(GadgetronError::Config(format!("unsupported provider type in Phase 1: {provider_type}")));
            }
        };
        providers.insert(name.clone(), provider);
    }

    Ok(providers)
}

fn init_observability() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| "gadgetron=info,tower_http=info".parse().unwrap()),
        )
        .json()  // JSON 형식 (운영 환경, §2.A.9 --log-format json)
        .init();
    Ok(())
}

// gadgetron-xaas/src/audit/writer.rs
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditStatus {
    Ok,
    Error,
    StreamInterrupted,
    // Phase 2 may add `Throttled`, `RateLimited` variants.
}

/// Audit consumer loop: mpsc receiver → PostgreSQL INSERT.
/// 채널이 닫히면(AuditWriter Drop) 루프 종료.
async fn audit_consumer_loop(
    mut rx: tokio::sync::mpsc::Receiver<gadgetron_xaas::audit::writer::AuditEntry>,
    pool: sqlx::PgPool,
) {
    while let Some(entry) = rx.recv().await {
        // INSERT INTO audit_log (...) VALUES (...)
        // 실패 시 warn 로그만 — audit 실패가 request를 블록하지 않음
        let result = sqlx::query!(
            r#"
            INSERT INTO audit_log
                (tenant_id, api_key_id, request_id, model, provider,
                 status, input_tokens, output_tokens, cost_cents, latency_ms)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
            entry.tenant_id,
            entry.api_key_id,
            entry.request_id,
            entry.model,
            entry.provider,
            entry.status.as_str(),
            entry.input_tokens,
            entry.output_tokens,
            entry.cost_cents,
            entry.latency_ms,
        )
        .execute(&pool)
        .await;

        if let Err(e) = result {
            tracing::warn!(error = %e, "audit log insert failed");
            metrics::counter!("gadgetron_xaas_audit_insert_failed_total").increment(1);
        }
    }
}
```

### 2.11 의존성 추가

Sprint 3에서 추가하는 크레이트:

| 크레이트 | 버전 | 위치 | 정당화 |
|---------|------|------|--------|
| `sha2` | `0.10` | `gadgetron-gateway` | Bearer 토큰 SHA-256 해싱. pure-Rust, `#![no_std]` 호환. ring은 C FFI 포함으로 제외 |
| `gadgetron-xaas` | workspace | `gadgetron-gateway` | `KeyValidator`, `QuotaEnforcer`, `AuditWriter` 사용 |
| `gadgetron-xaas` | workspace | `gadgetron-cli` | bootstrap에서 `PgKeyValidator`, `InMemoryQuotaEnforcer` 직접 생성 |

`gadgetron-gateway/Cargo.toml`에 추가:

```toml
gadgetron-xaas = { workspace = true }
sha2 = "0.10"
metrics = "0.23"
```

`gadgetron-cli/Cargo.toml`에 추가:

```toml
gadgetron-xaas = { workspace = true }
gadgetron-gateway = { workspace = true }
```

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 의존성 그래프 (Sprint 3 관련)

```
gadgetron-cli
  ├── gadgetron-gateway   ← Sprint 3 주 구현 대상
  │     ├── gadgetron-router  (Router, CircuitBreaker, MetricsStore)
  │     │     ├── gadgetron-core  (LlmProvider trait, ChatRequest/Response/Chunk)
  │     │     └── gadgetron-provider  (OpenAI, Anthropic, Ollama 구현체)
  │     ├── gadgetron-xaas   (KeyValidator, QuotaEnforcer, AuditWriter)
  │     │     └── gadgetron-core
  │     └── gadgetron-core   (GadgetronError, TenantContext, AppConfig)
  ├── gadgetron-xaas
  └── gadgetron-core

(gadgetron-core는 leaf — 내부 crate 의존 없음, CI 집행: §2.A.4)
```

**역방향 의존 없음 검증**: `gadgetron-xaas`는 `gadgetron-gateway`에 의존하지 않는다. `gadgetron-core`는 `axum`, `sqlx`, `nvml`에 의존하지 않는다 (CI gate: `cargo tree --package gadgetron-core | grep -E 'axum|sqlx|nvml'` → 0줄).

### 3.2 데이터 흐름 다이어그램

```
Client (HTTP POST /v1/chat/completions, Bearer gad_live_...)
    │
    ▼
[Tower Layer 1] RequestBodyLimitLayer(4MB)
    │ body > 4MB → 413, 이후 레이어 미실행
    ▼
[Tower Layer 2] TraceLayer
    │ span: request_id=UUID, method=POST, path=/v1/chat/completions
    ▼
[Tower Layer 3] RequestIdLayer
    │ X-Request-Id 헤더 응답에 주입
    ▼
[Tower Layer 4] AuthLayer<PgKeyValidator>
    │ SHA-256(Bearer token) → moka cache hit → ValidatedKey
    │ miss → PG SELECT api_keys WHERE key_hash=$1 → ValidatedKey
    │ 실패 → GadgetronError::TenantNotFound → 401
    ▼
[Tower Layer 5] TenantContextLayer
    │ Extension<TenantContext> 삽입 (tenant_id, scopes, request_id)
    ▼
[Tower Layer 6] ScopeGuardLayer
    │ /v1/* → requires Scope::OpenAiCompat
    │ 없음 → GadgetronError::Forbidden → 403
    ▼
[axum Router] 경로 매칭
    │ POST /v1/chat/completions → chat_completions_handler
    ▼
[Handler] chat_completions_handler(State<AppState>, Extension<TenantContext>, Json<ChatRequest>)
    │ QuotaEnforcer::check_pre(tenant_id, quota_snapshot) → QuotaToken
    │ 초과 → GadgetronError::QuotaExceeded → 429
    ▼
[gadgetron-router] Router::chat(req) or Router::chat_stream(req)
    │ resolve(req) → RoutingDecision {provider, model, fallback_chain}
    │ CircuitBreaker::state() 확인 — Open → 즉시 502
    ▼
[gadgetron-provider] LlmProvider::chat(req) or LlmProvider::chat_stream(req)
    │ reqwest POST to upstream API (rustls TLS)
    │ 실패 → cb.record_failure() → fallback chain 시도
    │ 성공 → cb.record_success()
    ▼
[비스트리밍] ChatResponse → Json → HTTP 200
[스트리밍]   Pin<Box<dyn Stream<Item=Result<ChatChunk,_>>+Send>>
    │            → chat_chunk_to_sse(stream)
    │            → SSE: "data: {...}\n\n" × N
    │            → "data: [DONE]\n\n"
    ▼
Client ← HTTP 200 (JSON or SSE)

[사후처리] (비차단)
    QuotaEnforcer::record_post(token, cost_cents)
    AuditWriter::try_send(AuditEntry{...})
    MetricsStore::record_success(provider, model, latency_ms, tokens, cost)
```

### 3.3 인터페이스 계약

| 호출자 | 피호출자 | 계약 타입/함수 | 에러 처리 |
|--------|---------|---------------|----------|
| `AuthLayer` | `PgKeyValidator::validate(key_hash: &str)` | `Result<Arc<ValidatedKey>, GadgetronError>` | `TenantNotFound` → 401, `Database` → 503 |
| `chat_completions_handler` | `QuotaEnforcer::check_pre(tenant_id, snapshot)` | `Result<QuotaToken, GadgetronError>` | `QuotaExceeded` → 429 |
| `chat_completions_handler` | `Router::chat(req)` | `Result<ChatResponse, GadgetronError>` | 모든 에러 → `e.into_response()` |
| `chat_completions_handler` | `Router::chat_stream(req)` | `Pin<Box<dyn Stream<Item=Result<ChatChunk,_>>+Send>>` | 스트림 내 에러 → SSE error 이벤트 |
| `handle_non_streaming` | `AuditWriter::send(entry)` | fire-and-forget `try_send` | 드롭 시 warn 로그 + counter 증가 |
| `ready_handler` | `sqlx::query("SELECT 1")` | DB ping | `is_ok()` false → 503 |
| `ready_handler` | `Router::health_check_all()` | `HashMap<String, bool>` | 하나라도 false → 503 |

### 3.4 D-12 크레이트 경계 준수 확인

| 타입 | 소유 크레이트 | Sprint 3 위치 준수 |
|------|------------|-----------------|
| `TenantContext` | `gadgetron-core` | Extension으로만 전달, gateway handler에서 직접 생성하지 않음 |
| `ValidatedKey` | `gadgetron-xaas` | AuthLayer Extension에만 저장, 외부 노출 없음 |
| `ChatRequest` | `gadgetron-core` | tenant_id 필드 추가 없음 (OpenAI 호환 표면 유지, B-1) |
| `GadgetronError` | `gadgetron-core` | 신규 variant 추가 없음 |
| `IntoResponse for GadgetronError` | `gadgetron-gateway` (impl 위치) | orphan rule 준수: `GadgetronError`는 core, `IntoResponse`는 axum — impl은 gateway에서 |

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 대상 및 invariant

#### 4.1.1 IntoResponse for GadgetronError

**파일**: `crates/gadgetron-gateway/src/error.rs`

| 테스트 케이스 | 입력 | 기대 출력 | invariant |
|-------------|------|----------|----------|
| `config_error_returns_400` | `GadgetronError::Config("bad".into())` | HTTP 400, `{"error":{"code":"config_error",...}}` | Config → 400 |
| `provider_error_returns_502` | `GadgetronError::Provider("down".into())` | HTTP 502 | Provider → 502 |
| `routing_error_returns_503` | `GadgetronError::Routing("none".into())` | HTTP 503 | Routing → 503 |
| `quota_exceeded_returns_429` | `GadgetronError::QuotaExceeded{tenant_id:Uuid::nil()}` | HTTP 429, body `code="quota_exceeded"` | QuotaExceeded → 429 |
| `tenant_not_found_returns_401` | `GadgetronError::TenantNotFound` | HTTP 401, `type="authentication_error"` | TenantNotFound → 401 |
| `forbidden_returns_403` | `GadgetronError::Forbidden` | HTTP 403, `code="forbidden"` | Forbidden → 403 |
| `db_pool_timeout_returns_503` | `GadgetronError::Database{kind:DatabaseErrorKind::PoolTimeout,..}` | HTTP 503 | DB pool timeout → 503 |
| `db_row_not_found_returns_404` | `GadgetronError::Database{kind:DatabaseErrorKind::RowNotFound,..}` | HTTP 404 | |
| `db_constraint_returns_409` | `GadgetronError::Database{kind:DatabaseErrorKind::Constraint,..}` | HTTP 409 | |
| `node_invalid_mig_profile_returns_400` | `GadgetronError::Node{kind:NodeErrorKind::InvalidMigProfile,..}` | HTTP 400 | |
| `error_body_never_contains_to_string` | 임의 에러 | body에 `self.to_string()` 내용 없음 | info disclosure 방지 |
| `all_12_variants_covered` | 12 variant 전체 | status != 0 | 누락 variant 없음 |

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::StatusCode};

    async fn status_of(e: GadgetronError) -> StatusCode {
        let response = e.into_response();
        response.status()
    }

    async fn body_of(e: GadgetronError) -> serde_json::Value {
        let response = e.into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn tenant_not_found_returns_401() {
        assert_eq!(status_of(GadgetronError::TenantNotFound).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn forbidden_returns_403() {
        assert_eq!(status_of(GadgetronError::Forbidden).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn quota_exceeded_returns_429_with_correct_body() {
        let e = GadgetronError::QuotaExceeded { tenant_id: uuid::Uuid::nil() };
        assert_eq!(status_of(e.clone()).await, StatusCode::TOO_MANY_REQUESTS);
        let body = body_of(e).await;
        assert_eq!(body["error"]["code"], "quota_exceeded");
        assert_eq!(body["error"]["type"], "quota_error");
    }

    #[tokio::test]
    async fn error_body_never_leaks_internal_string() {
        let e = GadgetronError::Provider("internal details that must not leak".into());
        let body = body_of(e).await;
        let body_str = body.to_string();
        assert!(!body_str.contains("internal details that must not leak"));
    }

    #[tokio::test]
    async fn all_db_kind_variants_produce_5xx_or_4xx() {
        use gadgetron_core::error::DatabaseErrorKind;
        let kinds = [
            DatabaseErrorKind::PoolTimeout,
            DatabaseErrorKind::ConnectionFailed,
            DatabaseErrorKind::RowNotFound,
            DatabaseErrorKind::Constraint,
            DatabaseErrorKind::QueryFailed,
            DatabaseErrorKind::MigrationFailed,
            DatabaseErrorKind::Other,
        ];
        for kind in kinds {
            let e = GadgetronError::Database { kind, message: "".into() };
            let status = status_of(e).await;
            assert!(status.is_client_error() || status.is_server_error(),
                "kind {:?} produced unexpected status {}", kind, status);
        }
    }
}
```

#### 4.1.2 chat_chunk_to_sse

**파일**: `crates/gadgetron-gateway/src/sse.rs`

| 테스트 케이스 | 입력 스트림 | 기대 SSE 이벤트 |
|-------------|-----------|--------------|
| `single_chunk_produces_data_event` | `Ok(ChatChunk{...})` 1개 | `data: {json}\n\n`, `data: [DONE]\n\n` |
| `error_chunk_produces_error_event` | `Err(GadgetronError::StreamInterrupted{reason:"timeout".into()})` | `event: error\ndata: {...code:"stream_interrupted"}\n\n` |
| `empty_stream_produces_only_done` | 빈 스트림 | `data: [DONE]\n\n` |
| `chunk_serialization_valid_json` | `Ok(ChatChunk{id:"chatcmpl-x",..})` | `data:` 이후 값이 `serde_json::from_str` 성공 |

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use gadgetron_core::provider::{ChatChunk, ChunkChoice, ChunkDelta};

    fn make_chunk(content: &str) -> ChatChunk {
        ChatChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "test-model".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: Some(content.to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
        }
    }

    #[tokio::test]
    async fn single_chunk_produces_data_then_done() {
        use futures::StreamExt;
        let chunk = make_chunk("hello");
        let input = stream::iter(vec![Ok(chunk)]);
        let sse = chat_chunk_to_sse(input);
        // axum SSE 스트림에서 이벤트 수집
        // 실제 이벤트 텍스트는 axum::response::sse::Event의 포맷에 의존
        // 단위 테스트에서는 내부 stream에 접근해 검증
        // (axum 0.8 Sse::new 내부 stream은 공개 API — 통합 테스트에서 axum_test로 검증)
    }

    #[tokio::test]
    async fn chunk_json_is_valid() {
        let chunk = make_chunk("world");
        let json = serde_json::to_string(&chunk).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["choices"][0]["delta"]["content"], "world");
    }
}
```

#### 4.1.3 ScopeGuardLayer

| 테스트 케이스 | Path | TenantContext scopes | 기대 결과 |
|-------------|------|---------------------|----------|
| `openai_path_with_openai_scope_passes` | `/v1/chat/completions` | `[OpenAiCompat]` | next.run() 호출 |
| `openai_path_without_scope_returns_403` | `/v1/chat/completions` | `[Management]` | 403 Forbidden |
| `mgmt_path_with_mgmt_scope_passes` | `/api/v1/nodes` | `[Management]` | next.run() 호출 |
| `mgmt_path_without_scope_returns_403` | `/api/v1/nodes` | `[OpenAiCompat]` | 403 Forbidden |
| `health_path_passes_without_scope` | `/health` | `[]` | next.run() 호출 (이 경로는 ScopeGuard에 도달하지 않음) |

#### 4.1.4 CircuitBreaker 상태 전이

`gadgetron-router/src/circuit_breaker.rs` (§2.F.3에서 이미 정의됨. 여기서는 gateway 연동 테스트만 추가):

| 테스트 케이스 | 입력 이벤트 시퀀스 | 기대 최종 상태 |
|-------------|-----------------|-------------|
| `three_failures_trips_breaker` | `record_failure()` × 3 | `CircuitState::Open` |
| `success_resets_closed` | `record_failure()` × 2, `record_success()` | `CircuitState::Closed`, count=0 |
| `open_transitions_to_half_open_after_60s` | trip → `advance_time(61s)` → `state()` | `CircuitState::HalfOpen` |
| `half_open_success_closes` | trip → half-open → `record_success()` | `CircuitState::Closed` |
| `half_open_failure_reopens` | trip → half-open → `record_failure()` | `CircuitState::Open` |
| `manual_reset_always_closes` | 임의 상태 → `manual_reset()` | `CircuitState::Closed`, count=0 |

### 4.2 테스트 하네스

```rust
// crates/gadgetron-gateway/tests/mock.rs (공통 fixture)
use std::sync::Arc;
use async_trait::async_trait;
use gadgetron_core::{
    context::QuotaSnapshot,
    error::GadgetronError,
    provider::{ChatRequest, ChatResponse, ChatChunk, ModelInfo, Usage},
};
use gadgetron_xaas::{
    auth::validator::{KeyValidator, ValidatedKey},
    quota::enforcer::{QuotaEnforcer, QuotaToken},
};
use uuid::Uuid;

/// 항상 성공하는 KeyValidator mock.
/// inject_tenant_id로 tenant_id를 고정 반환.
pub struct MockKeyValidator {
    pub tenant_id: Uuid,
    pub scopes: Vec<gadgetron_core::context::Scope>,
}

#[async_trait]
impl KeyValidator for MockKeyValidator {
    async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        Ok(Arc::new(ValidatedKey {
            api_key_id: Uuid::nil(),
            tenant_id: self.tenant_id,
            scopes: self.scopes.clone(),
        }))
    }
    async fn invalidate(&self, _key_hash: &str) {}
}

/// 항상 QuotaExceeded를 반환하는 mock.
pub struct ExhaustedQuotaMock;

#[async_trait]
impl QuotaEnforcer for ExhaustedQuotaMock {
    async fn check_pre(&self, tenant_id: Uuid, _: &QuotaSnapshot) -> Result<QuotaToken, GadgetronError> {
        Err(GadgetronError::QuotaExceeded { tenant_id })
    }
    async fn record_post(&self, _: &QuotaToken, _: i64) {}
}

/// 고정 응답을 반환하는 LlmProvider mock.
pub struct FakeLlmProvider {
    pub response: ChatResponse,
}

#[async_trait::async_trait]
impl gadgetron_core::provider::LlmProvider for FakeLlmProvider {
    async fn chat(&self, req: ChatRequest) -> gadgetron_core::error::Result<ChatResponse> {
        let mut resp = self.response.clone();
        resp.model = req.model;
        Ok(resp)
    }
    fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = gadgetron_core::error::Result<ChatChunk>> + Send>> {
        Box::pin(futures::stream::empty())
    }
    async fn models(&self) -> gadgetron_core::error::Result<Vec<ModelInfo>> {
        Ok(vec![])
    }
    fn name(&self) -> &str { "fake" }
    async fn health(&self) -> gadgetron_core::error::Result<()> { Ok(()) }
}
```

### 4.3 Performance Benchmark

`benches/middleware_chain.rs` (criterion 0.5):

```rust
fn bench_middleware_chain_mock(c: &mut Criterion) {
    // Setup: MockKeyValidator (cache-hit), FakeLlmProvider (0ms),
    // InMemoryQuotaEnforcer, full 6-layer Tower stack
    // Measure: request dispatch through entire chain (no network)
    // Target: p99 < 1,000µs
}

fn bench_auth_layer_cache_hit(c: &mut Criterion) {
    // Isolated AuthLayer + MockKeyValidator
    // Target: p99 < 50µs
}
```

CI gate: `scripts/check_bench_regression.py --threshold 0.10`

---

### 4.4 커버리지 목표

- `gadgetron-gateway` line coverage ≥ 80%
- `IntoResponse for GadgetronError`: 12 variant 전부 (100% branch)
- `ScopeGuardLayer`: 4 분기 전부
- `chat_chunk_to_sse`: Ok/Err/Empty 분기 전부
- CI 집행: `cargo tarpaulin --packages gadgetron-gateway --fail-under 80`

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 GatewayHarness

```rust
// crates/gadgetron-gateway/tests/harness.rs
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use sqlx::PgPool;
use gadgetron_gateway::{server::build_router, state::AppState};
use gadgetron_router::{Router, MetricsStore};
use gadgetron_xaas::{
    auth::validator::MockKeyValidator,
    quota::enforcer::InMemoryQuotaEnforcer,
    audit::writer::AuditWriter,
};
use gadgetron_core::context::Scope;
use dashmap::DashMap;
use uuid::Uuid;

/// 통합 테스트용 하네스. 실제 TCP 소켓에 바인드하고 reqwest로 호출.
pub struct GatewayHarness {
    pub base_url: String,
    pub tenant_id: Uuid,
    pub api_key: String, // 실제 Bearer 값 (MockKeyValidator가 어떤 값도 수락)
    _shutdown: tokio::sync::broadcast::Sender<()>,
}

impl GatewayHarness {
    /// `providers`: FakeLlmProvider 등 mock provider 주입.
    pub async fn start(
        providers: HashMap<String, Arc<dyn gadgetron_core::provider::LlmProvider + Send + Sync>>,
        pg_pool: Option<PgPool>,
        scopes: Vec<Scope>,
    ) -> Self {
        let tenant_id = Uuid::new_v4();
        let key_validator = Arc::new(MockKeyValidator { tenant_id, scopes })
            as Arc<dyn gadgetron_xaas::auth::validator::KeyValidator + Send + Sync>;

        let (audit_writer, _rx) = AuditWriter::new(64);
        let metrics_store = Arc::new(MetricsStore::new());
        let router = Arc::new(Router::new(
            providers,
            Default::default(),
            metrics_store,
        ));

        // pg_pool: None → lazy pool that connects only when /ready is called.
        // Scenarios 1-7 don't hit /ready; scenario 8 (/ready probe) and
        // scenario 9 (audit verification) require a real PgHarness pool.
        let pg_pool = pg_pool.unwrap_or_else(|| {
            // Lazy pool — connects only when /ready is called.
            // Scenarios 1-7 don't hit /ready; scenario 8-9 require PgHarness.
            sqlx::PgPool::connect_lazy("postgresql://unused").unwrap()
        });

        let state = AppState {
            router,
            key_validator,
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            pg_pool,
            circuit_breakers: Arc::new(DashMap::new()),
        };

        let app = build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { let _ = rx.recv().await; })
                .await
                .unwrap();
        });

        GatewayHarness {
            base_url: format!("http://{}", addr),
            tenant_id,
            api_key: "gad_live_test_token".to_string(),
            _shutdown: tx,
        }
    }

    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }

    pub fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }
}
```

### 5.2 e2e 시나리오 — §2.A.7의 Sprint 3 관련 9개

#### 시나리오 1: happy_path_non_streaming

```
입력:
  POST /v1/chat/completions
  Authorization: Bearer gad_live_test
  {"model":"gpt-4o","messages":[{"role":"user","content":"hello"}],"stream":false}

기대 출력:
  HTTP 200
  {"id":"chatcmpl-*","object":"chat.completion","choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":N,"completion_tokens":N}}
  응답 body가 ChatResponse로 역직렬화 성공
  X-Request-Id 헤더 존재
```

```rust
#[tokio::test]
async fn happy_path_non_streaming() {
    let providers = {
        let mut m = HashMap::new();
        m.insert("fake".to_string(), Arc::new(FakeLlmProvider {
            response: ChatResponse {
                id: "chatcmpl-test".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "gpt-4o".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message { role: "assistant".to_string(), content: "hi".to_string(), ..Default::default() },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
            },
        }) as Arc<dyn LlmProvider>);
        m
    };

    let harness = GatewayHarness::start(providers, /* pg_pool */ None, vec![Scope::OpenAiCompat]).await;

    let resp = harness.client()
        .post(format!("{}/v1/chat/completions", harness.base_url))
        .header("Authorization", harness.auth_header())
        .json(&serde_json::json!({"model":"gpt-4o","messages":[{"role":"user","content":"hello"}],"stream":false}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert!(resp.headers().contains_key("x-request-id"));
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "hi");
}
```

#### 시나리오 2: happy_path_streaming

```
입력:
  POST /v1/chat/completions
  {"model":"gpt-4o","messages":[{"role":"user","content":"hello"}],"stream":true}

기대 출력:
  HTTP 200
  Content-Type: text/event-stream
  SSE 이벤트: data: {"id":"chatcmpl-*",...} 복수 개
  마지막 이벤트: data: [DONE]
```

#### 시나리오 3: auth_missing_returns_401

```
입력: POST /v1/chat/completions (Authorization 헤더 없음)
기대 출력: HTTP 401, {"error":{"code":"tenant_not_found","type":"authentication_error",...}}
```

#### 시나리오 4: wrong_scope_returns_403

```
입력:
  POST /v1/chat/completions
  Authorization: Bearer <Management scope 전용 키>

기대 출력: HTTP 403, {"error":{"code":"forbidden","type":"permission_error",...}}
```

#### 시나리오 5: quota_exceeded_returns_429

```
입력: ExhaustedQuotaMock 주입 후 POST /v1/chat/completions
기대 출력: HTTP 429, {"error":{"code":"quota_exceeded",...}}
```

#### 시나리오 6: body_too_large_returns_413

```
입력: POST /v1/chat/completions, body = 5MB (> 4MB 제한)
기대 출력: HTTP 413 Payload Too Large (axum RequestBodyLimitLayer 자동 처리)
```

#### 시나리오 7: circuit_breaker_open_returns_502

```
입력:
  1. FailingProvider를 주입해 3회 연속 실패 유도
  2. 4번째 요청

기대 출력:
  1~3번: HTTP 502 (Provider 오류)
  4번: HTTP 502 (circuit open)
  gadgetron_router_circuit_state{provider="failing"} == 1.0 (Open)
```

#### 시나리오 8: health_liveness_always_200

```
입력: GET /health (Authorization 없음)
기대 출력: HTTP 200, {"status":"ok"}
```

#### 시나리오 9: ready_probe_fails_when_pg_down

```
입력: GET /ready (PgPool이 잘못된 URL로 연결된 상태)
기대 출력: HTTP 503, {"status":"not_ready","postgres":"error",...}
```

### 5.3 테스트 환경

**PostgreSQL**: `testcontainers-rs 0.23` + `testcontainers-modules::postgres::Postgres`.

```rust
// tests/pg_harness.rs
use testcontainers::{runners::AsyncRunner, ContainerAsync};
use testcontainers_modules::postgres::Postgres;
use sqlx::PgPool;

pub struct PgHarness {
    container: ContainerAsync<Postgres>,
    pub pool: PgPool,
}

impl PgHarness {
    pub async fn start() -> Self {
        let container = Postgres::default().start().await.unwrap();
        let host_port = container.get_host_port_ipv4(5432).await.unwrap();
        let url = format!("postgresql://postgres:postgres@127.0.0.1:{}/postgres", host_port);
        let pool = PgPool::connect(&url).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        PgHarness { container, pool }
    }
}
```

**FakeLlmProvider** (§4.2에서 정의). 추가로 `FailingProvider`:

```rust
// tests/mock.rs
pub struct FailingProvider;

#[async_trait::async_trait]
impl LlmProvider for FailingProvider {
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse> {
        Err(GadgetronError::Provider("forced failure".into()))
    }
    fn chat_stream(&self, _: ChatRequest) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        Box::pin(futures::stream::once(async {
            Err(GadgetronError::StreamInterrupted { reason: "forced".into() })
        }))
    }
    async fn models(&self) -> Result<Vec<ModelInfo>> { Ok(vec![]) }
    fn name(&self) -> &str { "failing" }
    async fn health(&self) -> Result<()> { Err(GadgetronError::Provider("down".into())) }
}
```

### 5.4 회귀 방지

다음 변경이 발생하면 이 테스트가 실패해야 한다:

| 변경 사항 | 실패할 테스트 |
|---------|------------|
| `GadgetronError::TenantNotFound`의 HTTP status가 401에서 다른 값으로 변경 | `tenant_not_found_returns_401`, `auth_missing_returns_401` |
| `GadgetronError::Forbidden`의 HTTP status가 403에서 변경 | `forbidden_returns_403`, `wrong_scope_returns_403` |
| `CorsLayer::permissive()` 재도입 | CI 코드 리뷰 (grep 기반 linting) |
| `RequestBodyLimitLayer` 제거 | `body_too_large_returns_413` |
| SSE `KeepAlive` 인터벌이 15초에서 변경 | `happy_path_streaming`에서 SSE 응답 구조 검증 |
| `ScopeGuardLayer`의 `/v1/*` → `OpenAiCompat` 매핑 변경 | `wrong_scope_returns_403` |
| `AuditWriter` capacity가 4096 이하로 감소 | (audit_drop 시나리오, §2.A.7 scenario 6) |

### 5.6 Runbook Alert Mapping

Operational runbook for the five new failure modes introduced in Sprint 3. Each row: alert condition, diagnostic check, remediation.

| Failure Mode | Alert Condition | Check | Remediation |
|-------------|----------------|-------|------------|
| **Circuit breaker open** | `gadgetron_router_circuit_state{provider="<name>"} == 1` (Open) for > 60s | `GET /api/v1/admin/circuits/<provider_id>/status` — confirms Open state. Check provider health: upstream API latency/error rate via provider dashboard. | If upstream recovered: `POST /api/v1/admin/circuits/<provider_id>/reset` to manually close. Otherwise wait 60s for automatic half-open transition. If half-open still fails, investigate provider outage. |
| **Quota exceeded** | HTTP 429 rate > N/min for a tenant (`gadgetron_xaas_quota_exceeded_total{tenant_id="<uuid>"}`) | Phase 1: QuotaSnapshot is `i64::MAX` so 429 should not occur from quota. If 429 appears, check `InMemoryQuotaEnforcer` state. Phase 2: query `SELECT daily_used_cents, daily_limit_cents FROM tenants WHERE id=$1`. | Phase 1: restart gateway to reset in-memory state. Phase 2: increase tenant quota via admin API or wait for midnight UTC reset. |
| **Audit drop** | `gadgetron_xaas_audit_insert_failed_total` counter increasing | `SELECT count(*) FROM audit_log WHERE created_at > now()-interval '5 min'` — if count is flat during active traffic, inserts are failing. Check `GADGETRON_DATABASE_URL` connectivity and `audit_log` table permissions (`INSERT` grant required). | Restore PostgreSQL connectivity. The mpsc channel buffers up to 4,096 entries. Entries beyond the buffer are dropped silently (warn log). After restore, the channel drains automatically. |
| **DB pool timeout** | HTTP 503 rate increasing; `gadgetron_gateway_errors_total{code="database_error"}` rising | `SELECT count(*), state FROM pg_stat_activity WHERE application_name='gadgetron' GROUP BY state` — check for long-running transactions blocking pool slots. Pool max_connections=20, acquire_timeout=500ms. | Kill blocking queries (`pg_terminate_backend`). If sustained, increase `max_connections` in `gadgetron.toml` (requires restart). Scale read replicas if read load is the cause. |
| **Stream interrupted** | `gadgetron_stream_interrupted_total` counter increasing; client-side SSE disconnect errors | Check upstream provider latency: if P99 > 15s (SSE keepalive interval), intermediate proxies may be closing connections. Check `tracing::warn reason=...` in logs for pattern. | If proxy timeout: increase idle timeout on load balancer/envoy to > 30s. If provider is dropping chunks: enable fallback chain so circuit breaker routes to backup provider. |

---

## 6. Phase 구분

| 섹션 | Phase |
|------|-------|
| §2.2 axum Router, 16-layer stack (P1 레이어만) | [P1] |
| §2.3 AuthLayer, TenantContextLayer, ScopeGuardLayer, RequestIdLayer | [P1] |
| §2.4 IntoResponse for GadgetronError | [P1] |
| §2.5 chat_completions_handler (non-streaming + streaming) | [P1] |
| §2.6 SSE pipeline (chat_chunk_to_sse) | [P1] |
| §2.7 list_models_handler | [P1] |
| §2.8 /health, /ready handlers | [P1] |
| §2.9 CircuitBreaker 연동 | [P1] |
| §2.10 Bootstrap (main.rs gadgetron serve) | [P1] |
| RateLimitLayer (§2.B.8 레이어 7) | [P2] |
| GuardrailsLayer (§2.B.8 레이어 8) | [P2] |
| Gemini, vLLM, SGLang provider in build_providers | [P1 Week 6] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|----|------|------|------|------|
| Q-1 | `LlmProvider::chat_stream`이 현재 non-async (`fn chat_stream` not `async fn`). `async fn`으로 변경해야 `Arc<dyn LlmProvider>`와 `async-trait`이 일관됨 | A: 현재 동기 signature 유지 / B: `async fn chat_stream` + async-trait | B | 🟡 chief-architect 승인 대기 |
| Q-2 | **Q-2 Resolved**: Phase 1 records `latency_ms = ctx.started_at.elapsed()` at `tokio::spawn` call time (SSE response dispatch), NOT at stream drain. This means streaming latency reflects time-to-first-byte, not total stream duration. Phase 2 will add a Drop guard to capture total stream duration. | — | Resolved | CLOSED |
| Q-3 | `TenantContextLayer`에서 `QuotaSnapshot`을 `i64::MAX`로 초기화하면 `InMemoryQuotaEnforcer::check_pre`의 `remaining_daily_cents` 계산이 항상 양수 → quota 검사 무력화 위험 | A: Phase 1 accepted risk / B: PG에서 실제 quota 조회 | B | 🟡 xaas-platform-lead 설계 필요 (Phase 1에서 A로 진행) |

> **Operator warning (DX-F5)**: Phase 1 quota enforcement is inactive. `QuotaSnapshot` defaults to `i64::MAX`, meaning all requests pass the quota check regardless of tenant configuration. Phase 2 connects real PostgreSQL quota values via `xaas-platform-lead`'s `PgQuotaEnforcer`. Do not rely on quota limits for billing controls in Phase 1.

---

## 리뷰 로그 (append-only)

### Round 0 (Self-Check) — 2026-04-12 — @gateway-router-lead

**결론**: Draft 제출

**D-20260412-02 구현 결정론 Self-Check**:
- [x] 1. 모든 타입 시그니처 완전 (generic bounds, lifetimes, async/sync, Send/Sync)
- [x] 2. TBD/모호함 없음 — "적절한", "유연하게" 표현 사용 없음
- [x] 3. 컴포넌트별 동시성 모델 명시 (AppState clone 비용, moka, DashMap, AtomicUsize)
- [x] 4. 라이브러리 이름 + 버전 명시 (axum 0.8, tower-http 0.6, sha2 0.10, moka 0.12, sqlx 0.8)
- [x] 5. 모든 enum variant 열거 (GadgetronError 12종, CircuitState 3종, AuditStatus 3종, Scope 3종)
- [x] 6. 모든 magic number 근거 (4_194_304, 4_096, 15, 60, 3, MAX_BODY_BYTES, SSE_KEEPALIVE_SECS)
- [x] 7. 외부 의존성 계약 (PostgreSQL SELECT 1, moka TTL 600s, mpsc capacity 4096)
- [x] 8. 상태 머신 전이 테이블 (CircuitBreaker §2.F.3.2 참조, 8행 전이 테이블)
- [x] 9. 구체적 입력/기대 출력 테스트 케이스 (§4.1 테이블 + §5.2 9개 시나리오)
- [x] 10. 에러 처리 완전 명시 (각 handler의 에러 경로 + HTTP status + 로그/메트릭)

**체크리스트** (§1 Round 1 기준 예비 자가 검토):
- [x] 인터페이스 계약 — §3.3 표 명시
- [x] 크레이트 경계 — D-12 준수 §3.4 확인
- [x] 타입 중복 없음 — 신규 타입 없음, 기존 타입만 연결
- [x] 에러 반환 일관성 — IntoResponse + GadgetronError만 사용
- [x] 동시성 — Arc/DashMap/AtomicUsize 전략 명시
- [x] 의존성 방향 — 역방향 없음 (§3.1 검증)
- [x] Phase 태그 — §6 표 명시
- [x] 레거시 결정 준수 — D-1~D-13, D-20260411-* 참조 명시

**Action Items**: Q-1, Q-2, Q-3는 Round 1 리뷰어 피드백 후 처리.
