# gadgetron-xaas Phase 1 Design

> **담당**: @xaas-platform-lead
> **상태**: Round 3 retry (⚠️ Conditional Pass → ✅ 4 blocking 해제, Approved)
> **작성일**: 2026-04-11
> **최종 업데이트**: 2026-04-12 (Round 3 retry)
> **관련 크레이트**: `gadgetron-xaas`, `gadgetron-core`, `gadgetron-gateway`
> **Phase**: [P1]
> **반영 결정**: D-4, D-5, D-8, D-9, D-11, D-13, D-20260411-01/03/06/08/09/10/12/13

---

## 1. 철학 & 컨셉 (Why)

### 1.1 해결하는 문제

**멀티테넌트 API 게이트웨이가 안전하게 작동하려면 auth / tenant isolation / quota / audit 4가지가 반드시 필요하고, `gadgetron-gateway` 는 이 기능들을 다른 크레이트에 위임해야 한다.** Phase 1 은 이 4가지 최소 기능을 `gadgetron-xaas` 한 크레이트에 묶어 제공하고, 추후 Phase 2 billing/agent/catalog/gpuaas 를 같은 크레이트 내부에서 확장한다.

### 1.2 제품 비전과의 연결

- **`docs/00-overview.md §1.3` 비전**: "온프레미스 + 자체 호스팅, 데이터 주권" → 인증/감사 로그 `gadgetron-xaas` 내재화 (외부 SaaS 의존 없음)
- **`docs/modules/xaas-platform.md §1` 3계층 추상화**: Phase 1 은 그 아래의 "기반" (`gadgetron-xaas` 의 auth/tenant/quota/audit 인프라) 만 구현. Phase 2 에서 GPUaaS / ModelaaS / AgentaaS 를 이 위에 쌓음.
- **`docs/reviews/pm-decisions.md` D-4 / D-8 / D-11 / D-13**: PostgreSQL, i64 cents, `gad_` prefix, 확장된 `GadgetronError` 를 모두 충족.

### 1.3 고려한 대안 / 채택하지 않은 이유

| 대안 | 기각 이유 |
|------|----------|
| **3개 크레이트** (`-auth`, `-tenant`, `-quota`) | D-20260411-03 옵션 B. 복잡도 증가, 초기 개발 속도 저하, D-12 업데이트 3번 필요 |
| **기존 크레이트 흡수** (auth→gateway, tenant→core, quota→router) | D-20260411-03 옵션 C. 순환 의존 리스크, 테스트 격리 불가 |
| **Redis 기반 rate limit** | Phase 1 에는 단일 노드 기준. Redis 의존성 추가 피함. Phase 2 에서 검토 |
| **JWT 대신 opaque token** | 채택. JWT 는 revocation 복잡 (refresh token 필요). Opaque + DB hash lookup 이 단순 |
| **SQLite** | **금지 (D-4)**. 멀티 테넌트 동시 쓰기 불가 |
| **f64 money** | **금지 (D-8)**. 부동소수점 반올림 오류 |

### 1.4 핵심 설계 원칙

1. **Fail fast, fail loud on auth**: Bearer 토큰 누락/invalid 는 즉시 401/403. 모호한 200 없음.
2. **Async audit, never block**: Hot path 에 동기 DB write 없음. `mpsc` drop 시 요청 성공은 유지.
3. **Pre + post quota**: `check_pre` (estimate) + `record_post` (actual) 2단계로 정확도 확보.
4. **Single source of truth for error**: `GadgetronError` 의 12개 variant (D-13, `gadgetron-core/src/error.rs`) 로 모든 XaaS 에러 전파.
5. **Migration-first schema**: `sqlx migrate add` 로만 스키마 변경. runtime DDL 금지.
6. **Trait-first public API**: `KeyValidator` / `TenantRegistry` / `QuotaEnforcer` / `AuditWriter` trait 우선, `Pg*` impl 하단.

### 1.5 Trade-offs

- **단일 크레이트 vs 3개 분리**: 테스트 속도/컴파일 속도 약간 느려짐. 하지만 Phase 2 확장 시 이동 불필요.
- **mpsc drop vs backpressure**: Drop 으로 결정. Backpressure 는 요청 latency 에 악영향.
- **Pre-check estimate vs no-estimate**: Estimate 사용으로 정확도 희생, 실패 시 422 대신 429 재시도 허용.

### 1.6 Threat Model (STRIDE)

| Category | Threat | Asset | Trust Boundary | Mitigation | Phase |
|:---:|---|---|---|---|:---:|
| **S** (Spoofing) | Stolen API key | API keys | client→gateway | SHA-256 hash lookup in DB (raw key never stored); moka TTL 10 min limits exposure window; explicit `invalidate_key(hash)` called on revoke (SEC-4) | P1 |
| **S** (Spoofing) | Tenant impersonation via guessed prefix | API keys | client→gateway | 32-char base62 suffix ≈ 190 bits entropy (log₂(62)×32); birthday-paradox collision requires ~2⁹⁵ keys — infeasible | P1 |
| **T** (Tampering) | Audit log modification or deletion | `audit_log` table | app→PostgreSQL | DB role `gadgetron_app` has INSERT-only on `audit_log` (REVOKE UPDATE, DELETE, TRUNCATE) — SEC-3 | P1 |
| **T** (Tampering) | Quota bypass via process restart | In-process quota state | in-process | Phase 1 accepted risk: in-memory `TokenBucket` resets on restart. Phase 2: persist quota state to PostgreSQL | P1 risk |
| **R** (Repudiation) | Audit entry drop on channel full | `audit_log` | in-process | `gadgetron_xaas_audit_dropped_total` counter + alert (DX-3 runbook); Phase 2 WAL/S3 fallback | P1 accepted |
| **I** (Info Disclosure) | DB schema leak via error message in spans | Tracing/logs | in-process | `xaas.db.error` span records `kind: DatabaseErrorKind` only — `message` field suppressed (SEC-5) | P1 |
| **D** (Denial of Service) | Unbounded request body exhausting memory | Memory | client→gateway | `RequestBodyLimit 4MB` middleware in Tower stack (§3.2) | P1 |
| **E** (Elevation of Privilege) | Scope confusion: `OpenAiCompat` key used to call `XaasAdmin` endpoint | Authorization | gateway | `ScopeGuard` (`require_scope` helper) returns `GadgetronError::Forbidden` (403) before handler executes | P1 |

### 1.7 Compliance Mapping

| Control | Regulation | Feature | Section |
|---|---|---|---|
| CC6.1 Access control | SOC2 | `AuthLayer` + `ScopeGuard` (`require_scope`) enforce authentication and authorization on every request | §2.1.2 |
| CC6.7 Access revocation | SOC2 | Key revocation sets `revoked_at` in DB; `PgKeyValidator::invalidate_key` immediately evicts from moka cache | §2.2.2 |
| Art 32(1)(b) Confidentiality | GDPR | Tenant isolation via `tenant_id` on all tables; API key stored as SHA-256 hash only (raw key returned once at issuance) | §2.2.1 |
| §164.312(b) Audit controls | HIPAA | `audit_log` INSERT-only DB role prevents modification; 90-day retention policy (Phase 2 GDPR anonymization) | §2.2.4 |

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

#### 2.1.1 크레이트 구조

```
crates/gadgetron-xaas/
├── Cargo.toml
└── src/
    ├── lib.rs                    # 공개 re-export
    ├── auth/
    │   ├── mod.rs
    │   ├── key.rs                # ApiKey 파싱
    │   ├── validator.rs          # KeyValidator trait + PgKeyValidator
    │   └── middleware.rs         # AuthLayer (tower::Layer)
    ├── tenant/
    │   ├── mod.rs
    │   ├── model.rs              # Tenant, TenantContext, TenantSpec
    │   └── registry.rs           # TenantRegistry trait + PgTenantRegistry
    ├── quota/
    │   ├── mod.rs
    │   ├── config.rs             # QuotaConfig
    │   ├── enforcer.rs           # QuotaEnforcer trait + QuotaToken
    │   └── bucket.rs             # TokenBucket (RPM+TPM)
    ├── audit/
    │   ├── mod.rs
    │   ├── entry.rs              # AuditEntry
    │   └── writer.rs             # AuditWriter trait + PgAuditWriter
    └── db/
        └── migrations/
            ├── 20260411000001_tenants.sql
            ├── 20260411000002_api_keys.sql
            ├── 20260411000003_quotas.sql
            └── 20260411000004_audit_log.sql
```

#### 2.1.2 공개 4개 Trait

```rust
// crates/gadgetron-xaas/src/auth/validator.rs
use async_trait::async_trait;
use uuid::Uuid;
use gadgetron_core::error::Result;

#[async_trait]
pub trait KeyValidator: Send + Sync {
    /// Raw Bearer token 문자열 검증 후 ValidatedKey 반환.
    /// 실패: GadgetronError::TenantNotFound (unit variant — no String payload)
    async fn validate(&self, raw_key: &str) -> Result<ValidatedKey>;
}

#[derive(Debug, Clone)]
pub struct ValidatedKey {
    pub api_key_id: Uuid,    // maps to TenantContext::api_key_id (gadgetron-core)
    pub tenant_id: Uuid,
    pub kind: KeyKind,       // Live | Test | Virtual
    /// 1개 이상 scope 필수 (키 발급 시 최소 1개 부여).
    /// Phase 1 은 3개 coarse-grained scope (D-20260411-10).
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind { Live, Test, Virtual }

/// API 키 scope — D-20260411-10 Phase 1 정책.
/// TYPE DEFINITION 은 gadgetron-core::context 에 있음 (아래는 참조용 복사본).
///
/// `#[non_exhaustive]` 속성으로 Phase 2 에 추가 variant 를 도입해도
/// downstream consumer 의 `match` 가 breaking change 없이 컴파일된다.
/// Phase 2 확장 예 (주석 참조):
/// - `OpenAiCompat` → `{ChatRead, ChatWrite, ModelsList, EmbeddingsRead, ...}`
/// - `XaasAdmin`    → `{XaasGpuAllocate, XaasModelDeploy, XaasAgentCreate, ...}`
///
/// serde 직렬화: PascalCase (rename_all 없음) — `serde_json::to_string(&Scope::OpenAiCompat)` == `"\"OpenAiCompat\""`
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
// NO #[serde(rename_all = ...)] — serde uses PascalCase variant names as-is
pub enum Scope {
    /// `/v1/*` — OpenAI 호환 추론 경로 (chat/completions, models, embeddings).
    /// `gad_live_*` / `gad_test_*` / `gad_vk_*` 기본 부여.
    OpenAiCompat,

    /// `/api/v1/{nodes, models, usage, costs}` — 읽기·관리 API.
    /// 명시적 grant 필요 (기본 포함 안 함).
    Management,

    /// `/api/v1/xaas/*` — 테넌트 · 쿼터 · 에이전트 등 admin 작업.
    /// 명시적 grant 필요 (관리자 키 전용).
    XaasAdmin,
}
```

**키 발급 기본 scope 매트릭스** (D-20260411-10):

| 키 타입 | 기본 `scopes` | Management 부여 | XaasAdmin 부여 |
|---------|-------------|:---:|:---:|
| `gad_live_*`      | `[OpenAiCompat]` | 명시 grant | 명시 grant |
| `gad_test_*`      | `[OpenAiCompat]` | 명시 grant | 명시 grant |
| `gad_vk_*` (tenant 범위 내) | `[OpenAiCompat]` | ❌ 금지 | ❌ 금지 |

- `gad_vk_*` 는 테넌트 사용자 위임용 → 관리 scope 부여 불가 (`PgKeyValidator` 에서 강제).
- `Management` · `XaasAdmin` 은 운영 관리자용 `gad_live_*` 에만 명시적으로 부여 (CLI `gadgetron xaas keys grant --scope management <key_id>`).
- 최소 1개 scope 보장 → `ValidatedKey.scopes.is_empty()` 이면 `GadgetronError::TenantNotFound` (unit variant — opaque 401).

**`require_scope` helper** (호출 지점은 `gadgetron-gateway/src/middleware/auth.rs`):

```rust
// gadgetron-xaas/src/auth/middleware.rs
use gadgetron_core::context::Scope;
use gadgetron_core::context::TenantContext;

/// 특정 endpoint 에서 필요한 scope 를 TenantContext 에 확인.
/// 실패 시 `GadgetronError::Forbidden` 반환 (`403 Forbidden`, §2.4.2 참조).
/// TenantNotFound 를 쓰지 않음 — 키는 유효, scope 만 불충분.
pub fn require_scope(
    ctx: &TenantContext,
    required: Scope,
) -> Result<(), GadgetronError> {
    if ctx.scopes.iter().any(|s| *s == required) {
        Ok(())
    } else {
        Err(GadgetronError::Forbidden)
    }
}
```

호출 지점 (gateway-router-lead Week 3 설계 문서 반영):
- `/v1/*` 라우터 → `require_scope(&ctx, Scope::OpenAiCompat)?;`
- `/api/v1/{nodes, models, usage, costs}` 라우터 → `require_scope(&ctx, Scope::Management)?;`
- `/api/v1/xaas/*` 라우터 → `require_scope(&ctx, Scope::XaasAdmin)?;`

```rust
// crates/gadgetron-xaas/src/tenant/registry.rs
use async_trait::async_trait;
use uuid::Uuid;
use gadgetron_core::error::Result;
use crate::quota::config::QuotaConfig;

#[async_trait]
pub trait TenantRegistry: Send + Sync {
    /// 실패: GadgetronError::TenantNotFound (unit variant — no String payload)
    async fn get(&self, tenant_id: Uuid) -> Result<Tenant>;
    async fn create(&self, spec: TenantSpec) -> Result<Tenant>;
    async fn update_quota(&self, tenant_id: Uuid, quota: QuotaConfig) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub status: TenantStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy)]
pub enum TenantStatus { Active, Suspended, Deleted }

#[derive(Debug, Clone)]
pub struct TenantSpec {
    pub name: String,
    pub initial_quota: Option<QuotaConfig>,
}
```

```rust
// crates/gadgetron-core/src/context.rs  (TYPE DEFINITION lives in core)
// gadgetron-xaas::AuthLayer CONSTRUCTS TenantContext and inserts it into Request::extensions_mut()
use std::sync::Arc;
use uuid::Uuid;

/// Request-scoped context.
/// Gateway AuthLayer 가 Request::extensions_mut() 에 삽입.
/// 타입 정의는 gadgetron-core::context 에 있고, gadgetron-xaas 가 구성(construct)함.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: Uuid,
    pub api_key_id: Uuid,          // was key_id; renamed in gadgetron-core
    pub scopes: Vec<Scope>,
    pub quota_snapshot: Arc<QuotaSnapshot>,  // pre-fetched quota for this request
    pub request_id: Uuid,          // 요청 단위, audit 에 그대로 전달
    pub started_at: std::time::Instant,      // for latency tracking
}
```

```rust
// crates/gadgetron-xaas/src/quota/enforcer.rs
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use uuid::Uuid;
use gadgetron_core::provider::{ChatRequest, Usage};
use gadgetron_core::error::Result;
use gadgetron_core::context::TenantContext;

/// `QuotaEnforcer` 는 trait object 안전성 보장 (`dyn QuotaEnforcer`).
/// `Send + Sync + 'static` 으로 `Arc<dyn QuotaEnforcer>` 를 axum extension 에 주입.
/// 컴파일 타임 object-safety 검증 → §4.1.X `assert_object_safe()` 테스트 (A2).
#[async_trait]
pub trait QuotaEnforcer: Send + Sync + 'static {
    /// Pre-request 체크. TPM estimate = req.max_tokens.
    /// 실패: GadgetronError::QuotaExceeded { tenant_id: ctx.tenant_id } (struct variant, NOT tuple)
    async fn check_pre(
        &self,
        ctx: &TenantContext,
        req: &ChatRequest,
    ) -> Result<QuotaToken>;

    /// Post-inference 기록. estimate 와 actual 의 차이 정산.
    /// 실패 시 log 만 남기고 요청 실패 유발 금지.
    async fn record_post(
        &self,
        token: QuotaToken,
        usage: &Usage,
    ) -> Result<()>;

    /// A1 — async cancellation safety. `QuotaToken::drop` 이 `record_post`
    /// 호출 없이 발생한 경우 (request 취소, panic, timeout) `estimated_tokens`
    /// 만큼 RPM/TPM 버킷을 환불. 실패는 로그만 (요청 실패 유발 금지).
    async fn refund_estimate(
        &self,
        tenant_id: Uuid,
        tokens: u32,
    ) -> Result<()>;
}

/// RAII 정산 토큰. **drop guard** 가 `record_post` 누락 시 자동 환불.
///
/// # 라이프사이클 (A1)
/// 1. `check_pre` 가 token 발급 + RPM-1, TPM-estimate
/// 2-A. 정상 경로: `record_post(token, usage)` → `recorded.store(true)` → drop 시 환불 안 함
/// 2-B. 비정상 경로: handler 가 token drop 만 (cancel/panic/timeout) →
///       `Drop::drop` 이 `tokio::spawn(refund_estimate)` 발화 → 버킷 환불
///
/// # async cancellation safety (D-20260411-12 cohort)
/// `pub estimated_tokens` 는 environment 에서 `Drop` 으로 회수해야 영속 누수 없음.
/// 이전 설계는 `Clone` 가능 plain struct 였고 cancel 시 토큰이 영구 consume 되었음.
/// 이제 `Clone` 제거 + `recorded: Arc<AtomicBool>` flag 도입.
#[derive(Debug)]
pub struct QuotaToken {
    pub tenant_id:        Uuid,
    pub request_id:       Uuid,
    pub estimated_tokens: u32,
    pub issued_at:        Instant,
    pub(crate) enforcer:  Arc<dyn QuotaEnforcer>,
    pub(crate) recorded:  Arc<AtomicBool>,
}

impl QuotaToken {
    /// `record_post` / `refund_estimate` 핸들러가 호출 — drop 시점에 재처리 금지.
    pub(crate) fn mark_recorded(&self) {
        self.recorded.store(true, Ordering::Relaxed);
    }
}

impl Drop for QuotaToken {
    fn drop(&mut self) {
        // 정상 경로 (record_post 완료) 또는 panic 중 환불 시도 금지.
        if self.recorded.load(Ordering::Relaxed) || std::thread::panicking() {
            return;
        }
        // 비정상 경로 — request 취소/timeout. 환불은 fire-and-forget.
        let enforcer  = self.enforcer.clone();
        let tenant_id = self.tenant_id;
        let tokens    = self.estimated_tokens;
        // tokio::spawn 은 runtime 종료 중 실패 가능 → JoinError 는 무시.
        // refund_estimate 자체 실패도 warn 만 (drop 안에서 panic 금지).
        tokio::spawn(async move {
            if let Err(e) = enforcer.refund_estimate(tenant_id, tokens).await {
                tracing::warn!(?e, %tenant_id, refund = tokens,
                    "xaas.quota.refund_estimate failed (request cancelled)");
                metrics::counter!("gadgetron_xaas_quota_refund_failed_total").increment(1);
            }
        });
    }
}
```

```rust
// crates/gadgetron-xaas/src/audit/writer.rs
use async_trait::async_trait;
use gadgetron_core::error::Result;
use crate::audit::entry::AuditEntry;

#[async_trait]
pub trait AuditWriter: Send + Sync {
    /// Non-blocking: 내부 mpsc 에 try_send.
    /// 채널 full 시 log warning + drop (GadgetronError 반환하지 않음).
    async fn write(&self, entry: AuditEntry) -> Result<()>;
}
```

#### 2.1.3 Phase 1 XaaS REST Endpoints

Phase 1 `/api/v1/xaas/*` 엔드포인트 전체. 모두 `XaasAdmin` scope 필요 (단, 쿼터 조회는 `Management`). 인증 실패는 401 `TenantNotFound`, scope 실패는 403 `Forbidden`.

| Method | Path | Required Scope | Request Body | Response | Error codes |
|:---:|---|:---:|---|---|:---:|
| POST | `/api/v1/xaas/tenants` | `XaasAdmin` | `{"name": String}` | `{"id": Uuid, "name": String}` | `config_error` (400) |
| GET | `/api/v1/xaas/tenants` | `XaasAdmin` | — | `[{"id": Uuid, "name": String, "created_at": DateTime}]` | — |
| POST | `/api/v1/xaas/keys` | `XaasAdmin` | `{"tenant_id": Uuid, "scope": Scope}` | `{"key": "gad_live_...", "id": Uuid}` | `tenant_not_found` (401) |
| GET | `/api/v1/xaas/keys?tenant_id=X` | `XaasAdmin` | — | `[{"id": Uuid, "prefix": String, "scope": Scope, "created_at": DateTime}]` | — |
| DELETE | `/api/v1/xaas/keys/{id}` | `XaasAdmin` | — | 204 No Content | `db_row_not_found` (404) |
| GET | `/api/v1/xaas/quotas/{tenant_id}` | `Management` | — | `{"daily_limit": i64, "daily_used": i64, "monthly_limit": i64, "monthly_used": i64}` | `tenant_not_found` (401) |

**DELETE `/api/v1/xaas/keys/{id}` implementation requirement (SEC-4)**: The handler MUST call `cache.invalidate_key(&key_hash).await` synchronously (before returning 204) to ensure revocation takes effect immediately rather than waiting for moka TTL expiry. The `key_hash` is fetched from the DB row before deletion. If the DB row is not found, return 404 without cache interaction.

**필드 타입 명세**:
- `id`: `Uuid` (UUID v4, PostgreSQL `gen_random_uuid()`)
- `key`: `"gad_live_<32-char base62>"` — 발급 시 1회만 반환, DB 에 hash 만 저장 (D-11)
- `scope` in request: `"OpenAiCompat" | "Management" | "XaasAdmin"` (PascalCase, §2.1.2 Fix 2)
- `daily_limit`/`daily_used`/`monthly_limit`/`monthly_used`: `i64` cents (D-8), `QuotaSnapshot` 필드와 동일

**Error code → HTTP mapping** (§2.4.2 기준):
- `config_error` → 400 (잘못된 요청 본문)
- `tenant_not_found` → 401 (존재하지 않는 tenant_id 로 키 발급 시도 포함)
- `db_row_not_found` → 404 (존재하지 않는 key id)
- `forbidden` → 403 (scope 부족)

#### 2.1.4 QuotaToken lifecycle 상세

요약 (Tower 위치 상세는 §2.4.3):

```
Handler
  │ 1. check_pre(ctx, req)     ──▶ TokenBucket(RPM -1, TPM -estimate) ──▶ QuotaToken
  │ 2. request 실행
  │ 3-A. record_post(token, usage) ──▶ adjust(estimate − actual) → mark_recorded
  │ 3-B. (request cancelled / panic / timeout)
  │      └─ Drop::drop → tokio::spawn(refund_estimate(tenant_id, estimated))
  ▼
Response
```

- `check_pre` 가 발급한 `QuotaToken` 은 1 요청 당 1회만 `record_post` 로 소비.
- `record_post` 실패는 요청 실패가 아님 (log + metric).
- **Drop guard (A1)**: handler 가 `record_post` 를 호출하지 않고 token 을 drop 하면 `QuotaToken::Drop` 이 `refund_estimate(tenant_id, estimated)` 를 fire-and-forget 으로 호출하여 RPM/TPM 누수를 방지. cancel-safe (`tokio::time::timeout`, client disconnect, panic 모두 처리). 이전 설계는 cancel 시 estimate 가 **영구 consume** 되었음 — Round 3 chief-architect blocking.
- Phase 1 `concurrent_max` 는 config 만, 실제 카운팅은 Phase 2 (Redis).

#### 2.1.5 DB 스키마 & Migration 정책 (A6)

4개 마이그레이션 파일 (`src/db/migrations/`) 모두 **forward-only, idempotent** (같은 파일 두 번 실행해도 스키마 손상 없음). 전체 컬럼 정의는 `docs/modules/xaas-platform.md §3-6`. 본 섹션은 Round 1 retry 반영 사항만 명시.

**A6 적용 — 모든 DDL 에 `IF NOT EXISTS`**:

```sql
-- 20260411000001_tenants.sql
CREATE TABLE IF NOT EXISTS tenants ( ... );
CREATE INDEX IF NOT EXISTS tenants_status_idx ON tenants(status);

-- 20260411000002_api_keys.sql  (D-20260411-10: scopes TEXT[] 추가)
CREATE TABLE IF NOT EXISTS api_keys (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    prefix       VARCHAR(32) NOT NULL,
    key_hash     CHAR(64)    NOT NULL,                                         -- SHA-256 hex (D-11)
    kind         VARCHAR(16) NOT NULL,                                          -- 'live'|'test'|'virtual'
    scopes       TEXT[]      NOT NULL DEFAULT ARRAY['OpenAiCompat']::TEXT[],     -- D-10 (PascalCase — matches serde_json::to_string(&Scope::OpenAiCompat))
    name         VARCHAR(255),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ,
    UNIQUE (prefix, key_hash)
);
CREATE INDEX IF NOT EXISTS api_keys_tenant_idx ON api_keys(tenant_id) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS api_keys_hash_idx   ON api_keys(key_hash)  WHERE revoked_at IS NULL;
-- D-20260411-10: 최소 1 scope 강제
ALTER TABLE api_keys
    ADD CONSTRAINT api_keys_scopes_check CHECK (array_length(scopes, 1) >= 1);

-- 20260411000003_quotas.sql
CREATE TABLE IF NOT EXISTS quotas ( ... );

-- 20260411000004_audit_log.sql
CREATE TABLE IF NOT EXISTS audit_log (
    ...
    cost_cents BIGINT NOT NULL DEFAULT 0,   -- D-8, D-20260411-06: Phase 1 항상 0
    ...
);
CREATE INDEX IF NOT EXISTS audit_log_tenant_ts_idx ON audit_log(tenant_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS audit_log_request_idx   ON audit_log(request_id);

-- Migration: 002_roles.sql (SEC-3: INSERT-only role for audit integrity)
-- Run after 20260411000004_audit_log.sql. Requires superuser or rds_superuser.
REVOKE UPDATE, DELETE, TRUNCATE ON audit_log FROM gadgetron_app;
GRANT INSERT ON audit_log TO gadgetron_app;
```

`scopes TEXT[]` 값은 `serde_json::to_string(&Scope)` 결과 (PascalCase — `rename_all` 없음) — 예: `{"OpenAiCompat", "Management"}`. `serde_json::to_string(&Scope::OpenAiCompat)` == `"\"OpenAiCompat\""`. `PgKeyValidator` 가 `serde_json::from_str` 으로 역파싱 (§2.2.2). **주의**: snake_case 형태(`openai_compat`) 는 `.as_str()` Display 값이고 DB TEXT[] 저장값은 아님.

**`src/db/migrations/README.md` 스펙** (A6 필수 산출물):
1. **정책 (D-4, D-9)**: forward-only (`down.sql` 없음), idempotent (`IF NOT EXISTS`), `YYYYMMDDHHMMSS_<snake>.sql`, runtime DDL 금지.
2. **개발자 명령**: `sqlx migrate add -r <name>` · `sqlx migrate run --source <path>` · `sqlx migrate info`.
3. **CI 통합 (A10)** — `.github/workflows/ci.yml` 는 `services.postgres: postgres:16-alpine` + `DATABASE_URL=postgres://...` + `sqlx migrate run` + `cargo nextest run -p gadgetron-xaas`. 실제 yaml 은 devops-sre-lead (`docs/design/ops/ci-cd.md`).
4. **배포 (Q-5 옵션 C 하이브리드)**: Dev = `migrate_on_start=true` / Prod = `migrate_on_start=false` + CI 단계 별도 실행.

### 2.2 내부 구조

#### 2.2.1 `ApiKey` 파싱 (`src/auth/key.rs`)

```rust
pub struct ApiKey {
    pub raw:          String,          // 원본 — 로그 시 prefix 만 노출
    pub prefix:       String,          // 마스킹 표시용 ("gad_live_ab12")
    pub kind:         KeyKind,
    pub suffix:       String,          // base62 segment (32 char)
    pub tenant_short: Option<String>,  // `gad_vk_*` 일 때만 Some (12 char)
}

impl ApiKey {
    /// 세 포맷 분기 — 실패 시 `GadgetronError::TenantNotFound` (unit variant, opaque 401)
    /// - `gad_live_<32 base62>`
    /// - `gad_test_<32 base62>`
    /// - `gad_vk_<12 base62>_<32 base62>`
    pub fn parse(raw: &str) -> Result<Self, GadgetronError> { /* ... */ }

    /// base62 charset + 길이 검사. 실패 시 `GadgetronError::TenantNotFound` (unit variant).
    fn check_base62(s: &str, expected_len: usize) -> Result<(), GadgetronError> { /* ... */ }

    /// `SHA-256(raw)` → lowercase hex. 저장은 hex 만, raw 는 발급 응답에 1회만.
    pub fn hash_hex(&self) -> String { /* sha2 + hex */ }
}
```

**Masking for logs**: `{raw[..prefix.len()]}…` 만 노출. 해시·suffix 는 절대 log 금지 (Round 1 regression 기준).

**Entropy 근거 (A8 non-blocking)**: 32-char base62 suffix 의 엔트로피는 `32 × log₂(62) ≈ 190.5 bits`. 비교: UUIDv4 = 122 bits, 16-char base62 = 95 bits. 190 bits 는 birthday-paradox 충돌까지 ~`2⁹⁵` = 3.9 × 10²⁸ 키 발급 필요 → 사실상 불가능. SHA-256 (`hash_hex`) 의 256 bits 출력보다 약간 작지만 보안적으로는 동일한 무한 영역. base62 charset (`A-Za-z0-9`) 는 URL/header safe, base32 (160 bits @ 32-char) 보다 정보 밀도 높음. 따라서 `base62 = "2"` 의존성을 정당화 (§2.5).

#### 2.2.2 `PgKeyValidator` 동시성 모델 + LRU 캐시 (D-20260411-12)

```rust
use std::sync::Arc;
use std::time::Duration;
use moka::future::Cache;
use dashmap::DashMap;
use sqlx::PgPool;

/// Postgres-backed `KeyValidator` 구현.
///
/// # Send + Sync (A20)
/// `sqlx::PgPool`, `moka::future::Cache`, `DashMap` 이 모두 `Clone + Send + Sync`
/// 이므로 `PgKeyValidator: Send + Sync + 'static` → `Arc<dyn KeyValidator>` 를
/// `axum::Extension<Arc<dyn KeyValidator>>` 으로 handler 들이 공유 가능.
/// 컴파일 타임 object-safety 검증 → §4.1.X `assert_object_safe()` (A2).
///
/// # 캐시 정책 (D-20260411-12, A8, A17)
/// **두 단계 캐시**:
/// 1. `key_cache: moka::future::Cache<String, ValidatedKey>` — 10k entries,
///    10분 TTL (D-20260411-12). Key = `ApiKey::hash_hex()` (raw key 의 SHA-256).
///    Phase 1 SLO `auth p99 < 1ms` 충족의 핵심: hit 99% / miss 1%.
///    무효화: `invalidate_key(hash)` (revocation 시 수동) + 자연 TTL.
/// 2. `tenant_cache: DashMap<Uuid, CachedTenant>` — 30s TTL (기존 A17).
///    tenant suspend/delete 전파 latency 가 더 빨라야 함 → 짧은 TTL.
///
/// # 에러 매핑 (D-20260411-13, A6)
/// `sqlx::Error → GadgetronError::Database { kind, message }` 매핑은
/// `crate::error::sqlx_to_gadgetron` helper. 모든 sqlx call site:
/// `.await.map_err(sqlx_to_gadgetron)?` (§2.4.1).
pub struct PgKeyValidator {
    pool:         PgPool,
    /// D-20260411-12: SHA-256 hex → ValidatedKey, 10k entries, 10min TTL.
    key_cache:    Cache<String, ValidatedKey>,
    /// A17: tenant 활성 상태 캐시 (30초 TTL).
    tenant_cache: Arc<DashMap<Uuid, CachedTenant>>,
}

struct CachedTenant { tenant: Tenant, cached_at: std::time::Instant }
impl CachedTenant {
    const TTL: std::time::Duration = std::time::Duration::from_secs(30);
    fn is_fresh(&self) -> bool { self.cached_at.elapsed() < Self::TTL }
}

impl PgKeyValidator {
    /// 표준 생성자. moka 캐시 builder 로 D-20260411-12 spec 그대로.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            key_cache: Cache::builder()
                .max_capacity(10_000)                      // D-20260411-12
                .time_to_live(Duration::from_secs(600))   // 10 minutes
                .build(),
            tenant_cache: Arc::new(DashMap::new()),
        }
    }

    /// 관리자 키 revocation 시 호출 (CLI `gadgetron xaas keys revoke ...`).
    /// hash 인자는 `ApiKey::hash_hex()` 결과 (lowercase hex).
    /// natural TTL 와 함께 안정성 보장 (`AdminChannel` broadcast 는 Phase 2).
    pub async fn invalidate_key(&self, hash: &str) {
        self.key_cache.invalidate(hash).await;
        metrics::counter!("gadgetron_xaas_auth_cache_invalidations_total").increment(1);
    }

    /// A17: 30s TTL cache. 만료/miss 시 DB fetch 후 삽입.
    async fn get_tenant_cached(&self, tenant_id: Uuid) -> Result<Tenant> { /* ... */ }
}

#[async_trait]
impl KeyValidator for PgKeyValidator {
    #[tracing::instrument(skip(self, raw_key), fields(key_kind, tenant_id, cache_hit))]
    async fn validate(&self, raw_key: &str) -> Result<ValidatedKey> {
        let api_key = ApiKey::parse(raw_key)?;
        let hash    = api_key.hash_hex();

        // === Fast path — moka cache hit (D-20260411-12) ===
        // 99% 의 요청이 여기서 끝남. p99 < 1ms (in-process LRU).
        if let Some(cached) = self.key_cache.get(&hash).await {
            tracing::Span::current().record("cache_hit", true);
            metrics::counter!("gadgetron_xaas_auth_cache_hits_total").increment(1);
            return Ok(cached);
        }
        tracing::Span::current().record("cache_hit", false);
        metrics::counter!("gadgetron_xaas_auth_cache_misses_total").increment(1);

        // === Slow path — DB lookup (cache miss, ~5ms) ===
        // 1) api_keys SELECT (scopes TEXT[] 포함, D-20260411-10).
        //    sqlx::Error → GadgetronError::Database (D-20260411-13, §2.4.1).
        //    row 없음 → GadgetronError::TenantNotFound (unit variant — NOT Database — UX 의도)
        let row = fetch_key_row(&self.pool, &hash).await
            .map_err(sqlx_to_gadgetron)?;

        // 2) tenant 활성 상태 — 30s cache hit, miss 시 DB (A17)
        let tenant = self.get_tenant_cached(row.tenant_id).await?;
        if !matches!(tenant.status, TenantStatus::Active) {
            return Err(GadgetronError::TenantNotFound);  // unit variant — no String payload
        }

        // 3) A18 — Virtual key tenant_short mismatch 검증
        //    `gad_vk_<tenant_short>_<suffix>` 의 앞 12 char 는
        //    row.tenant_id.to_string()[..12] 와 일치해야 함.
        if api_key.kind == KeyKind::Virtual {
            let key_short = api_key.tenant_short.as_deref().unwrap_or("");
            let expected  = &row.tenant_id.to_string().replace('-', "")[..12];
            if key_short != expected {
                return Err(GadgetronError::TenantNotFound);  // unit variant — mismatch is opaque to client
            }
        }

        // 4) scopes TEXT[] → Vec<Scope> (D-20260411-10)
        //    Phase 1 은 최소 1개 보장, 없으면 `GadgetronError::TenantNotFound` (unit variant).
        let scopes = parse_scopes(&row.scopes)?;

        // 5) fire-and-forget last_used_at UPDATE
        tokio::spawn(update_last_used(self.pool.clone(), row.id));

        let validated = ValidatedKey {
            api_key_id: row.id,    // matches TenantContext::api_key_id (gadgetron-core)
            tenant_id: row.tenant_id,
            kind: parse_kind(&row.kind),
            scopes,
        };

        // === Cache insert (D-20260411-12) ===
        // 동일 키의 동시 미스 가 있어도 moka 는 last-write-wins 안전.
        self.key_cache.insert(hash, validated.clone()).await;

        Ok(validated)
    }
}
```

**동시성**: `sqlx::PgPool` 이 connection pool 역할, `moka::future::Cache` 는 segmented LRU + TinyLFU eviction (lock-free read), `DashMap` 로 tenant cache read 락 제거.

**Phase 1 auth latency SLO** (D-20260411-12 — sub-millisecond 충족):

| 경로 | 빈도 | p99 | 비고 |
|------|:---:|:---:|------|
| **Cache hit** | **99%** | **< 1ms** | moka in-process LRU lookup (sub-μs SHA-256 hex 비교 + O(1) read). sub-millisecond SLO 충족. |
| **Cache miss** | **1%** | **< 8ms** | SHA-256 hex (~10μs) + PG connection acquire + `SELECT key+scopes` (~5ms) + `tenant_cache` 조회. cold start, TTL 만료, 첫 요청 시점에 발생. |
| Cache TTL | — | 600s (10 min) | 신선도 vs DB 부하의 균형. revocation 은 즉시 `invalidate_key(hash)` 로 무효화 가능. |
| Cache size | — | 10,000 entries | 10k 동시 distinct key 수용 (Phase 1 운용 규모 충분). |
| Phase 2 | — | — | Redis 분산 캐시 (multi-instance deployment), `AdminChannel` broadcast 무효화. |

**A11 — Boot-time DB connection test** (`src/db/mod.rs`):
```rust
pub async fn validate_connection(pool: &PgPool) -> Result<()> {
    sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(pool).await
        .map(|_| ())
        .map_err(sqlx_to_gadgetron)  // D-20260411-13: Database{ConnectionFailed,..}
}
```
`main.rs` boot 순서: `pool.connect() → validate_connection() → config.validate_xaas() → audit_writer::spawn()`. 어느 단계에서든 실패 = fail-fast 종료.

#### 2.2.3 TokenBucket 알고리즘

```rust
// src/quota/bucket.rs — per-tenant RPM 또는 TPM 버킷 (1분 윈도우).
// refill_per_sec = max_tokens / 60
pub struct TokenBucket {
    max_tokens:     u64,
    tokens:         f64,      // f64 precision for sub-second refill (Q-8 A)
    refill_per_sec: f64,
    last_refill:    Instant,
}

impl TokenBucket {
    pub fn new(max_per_minute: u64) -> Self { /* tokens = max, refill = max/60 */ }

    /// 원자적 consume — `Ok(remaining)` 또는 `Err(wait_secs)`.
    pub fn try_consume(&mut self, amount: u64) -> Result<u64, u64> {
        self.refill();
        if self.tokens >= amount as f64 {
            self.tokens -= amount as f64;
            Ok(self.tokens as u64)
        } else {
            let deficit = amount as f64 - self.tokens;
            Err((deficit / self.refill_per_sec).ceil() as u64)
        }
    }

    /// TPM 정산: estimate vs actual 차이 refund 또는 추가 consume.
    pub fn adjust(&mut self, delta: i64) {
        self.refill();
        self.tokens = (self.tokens + delta as f64).clamp(0.0, self.max_tokens as f64);
    }

    fn refill(&mut self) { /* elapsed * refill_per_sec, clamp max */ }
}
```

**동시성 포장**: `PgQuotaEnforcer` 가 `Mutex<(TokenBucket, TokenBucket)>` 를 per-tenant `DashMap<Uuid, ...>` 로 관리. Critical section 은 3~5 μs 수준. 분산은 Phase 2 (Redis).

#### 2.2.4 PgAuditWriter 비동기 batch flush (D-20260411-09 반영)

> **Phase 1 경량 정책 — 배포 전 compliance review 필수** (D-20260411-09 옵션 C).
> SOC2/HIPAA/GDPR 대상 배포 전에는 반드시 Phase 2 WAL/S3 fallback 을 활성화하거나
> compliance 팀과 drop rate 허용선을 합의할 것. Phase 1 은 **경량 drop + 버퍼 확대** 정책만 제공.

```rust
// src/audit/writer.rs — Phase 1: channel(4096), batch=100, flush=1s
use tokio::sync::{mpsc, oneshot};
use std::sync::Arc;

pub struct PgAuditWriter { tx: mpsc::Sender<WriterCmd> }

/// 내부 채널 메시지. `Flush` 는 graceful shutdown 시 호출.
enum WriterCmd {
    Entry(AuditEntry),
    Flush(oneshot::Sender<Result<(), GadgetronError>>),
}

impl PgAuditWriter {
    /// D-20260411-09 반영: 채널 capacity 1024 → **4096** (4배 확대).
    pub async fn spawn(pool: PgPool) -> Result<Arc<Self>, GadgetronError> {
        let (tx, mut rx) = mpsc::channel(4096); // ← D-20260411-09
        tokio::spawn(async move {
            let mut buf: Vec<AuditEntry> = Vec::with_capacity(100);
            let mut ticker = tokio::time::interval(Duration::from_millis(1000));
            loop {
                tokio::select! {
                    cmd = rx.recv() => match cmd {
                        Some(WriterCmd::Entry(e)) => {
                            buf.push(e);
                            if buf.len() >= 100 { let _ = flush(&pool, &mut buf).await; }
                        }
                        Some(WriterCmd::Flush(ack)) => {
                            let _ = ack.send(flush(&pool, &mut buf).await);
                        }
                        None => { let _ = flush(&pool, &mut buf).await; break; }
                    },
                    _ = ticker.tick() => {
                        if !buf.is_empty() { let _ = flush(&pool, &mut buf).await; }
                    }
                }
            }
        });
        Ok(Arc::new(Self { tx }))
    }

    /// A5: Graceful shutdown 용 — buffer 를 즉시 push 하고 ack 대기.
    /// 호출자는 `tokio::time::timeout` 으로 5초 한도 걸어야 함 (main.rs 참조).
    pub async fn flush(&self) -> Result<(), GadgetronError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx.send(WriterCmd::Flush(ack_tx)).await
            .map_err(|_| GadgetronError::Config("audit writer closed".into()))?;
        ack_rx.await
            .map_err(|_| GadgetronError::Config("audit writer ack lost".into()))?
    }
}

#[async_trait]
impl AuditWriter for PgAuditWriter {
    async fn write(&self, entry: AuditEntry) -> Result<()> {
        match self.tx.try_send(WriterCmd::Entry(entry.clone())) {
            Ok(_) => Ok(()),
            // D-20260411-09: drop 정책 — metric + warn, 요청 실패 유발 금지
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics::counter!("gadgetron_xaas_audit_dropped_total").increment(1);
                tracing::warn!(
                    tenant_id = ?entry.tenant_id,
                    request_id = ?entry.request_id,
                    "audit entry dropped due to channel full"
                );
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(GadgetronError::Config("audit writer closed".into()))
            }
        }
    }
}

// UNNEST 기반 bulk insert (A16 Phase 1 은 COPY 대신 UNNEST 선택).
// pseudocode:
//   INSERT INTO audit_log (request_id, tenant_id, api_key_id, model, ...)
//   SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[], $4::text[], ...)
async fn flush(pool: &PgPool, buf: &mut Vec<AuditEntry>) -> Result<(), GadgetronError> {
    if buf.is_empty() { return Ok(()); }
    let r = sqlx::query!("/* UNNEST batch insert */").execute(pool).await;
    if let Err(e) = &r {
        tracing::error!(error = %e, batch_len = buf.len(), "xaas.audit: flush failed");
        metrics::counter!("gadgetron_xaas_audit_flush_failed_total").increment(1);
    }
    buf.clear();
    r.map(|_| ()).map_err(|e| GadgetronError::Config(format!("audit flush: {e}")))
}
```

**Backpressure 정책 (D-20260411-09 옵션 C)**:
1. 채널 용량 `4096` 으로 확대 (기존 `1024` 의 4배) → 정상 운용 중 drop 확률 minimal.
2. `try_send` 실패 → `gadgetron_xaas_audit_dropped_total` counter + `tracing::warn!(tenant_id, request_id)` → 요청 hot path 영향 없음.
3. **Phase 2 fallback** — 상세 `docs/modules/xaas-platform.md §6.5`:
   - `src/audit/fallback.rs` (1차 WAL 파일, 2차 S3 batch upload).
   - SIGTERM chain: `PgAuditWriter::flush() → WAL flush → S3 upload`.
   - Compliance 체크리스트: 90일 retention, 30/90일 GDPR 익명화, 검색 indexing.

**A5 — Graceful shutdown drain audit** (`main.rs` 개념적):

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = build_pool().await?;
    gadgetron_xaas::db::validate_connection(&pool).await?; // A11 fail-fast
    gadgetron_xaas::config::XaasConfig::load()?.validate_xaas()?; // A14

    let audit_writer = gadgetron_xaas::audit::PgAuditWriter::spawn(pool.clone()).await?;
    let writer_for_shutdown = audit_writer.clone();
    let app = build_router(pool, audit_writer).await?;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("SIGTERM received — draining audit channel");
            // 5초 한도 best-effort flush
            let res = tokio::time::timeout(
                Duration::from_secs(5),
                writer_for_shutdown.flush()
            ).await;
            match res {
                Ok(Ok(()))  => tracing::info!("audit drain OK"),
                Ok(Err(e))  => tracing::warn!(error = %e, "audit drain returned error"),
                Err(_)      => tracing::warn!("audit drain timed out after 5s"),
            }
        })
        .await?;
    Ok(())
}
```

Spec: **5 초 timeout** · **progress logging** · **best-effort** (timeout 은 배포 재시작 차단 안 함, drop counter 로만 노출).

#### 2.2.5 상태머신 (QuotaEnforcer)

```
 [Fresh]
    | check_pre(ctx, req)
    | (issue QuotaToken, RPM-1, TPM-estimate)
    v
 [Pending]
    | request runs
    v
 [Completed] ── record_post(token, usage) ──> [Settled]
    |                                              |
    |  (record_post skipped?)                      |
    v                                              v
 [Orphan] ── reconciliation (Phase 2) ──>     [Settled]
```

Phase 1 에서는 Orphan 화해 없음 — 요청 실패/crash 시 RPM 은 정상 refill 되고, TPM 은 estimate 만큼 영구 consume (보수적). Phase 2 에 timeout-based reconciler 추가 예정.

### 2.3 설정 스키마

```toml
# gadgetron.toml (발췌)
# Every field can be overridden by an environment variable.
# Convention: GADGETRON_XAAS_<SECTION>_<FIELD> (uppercase, underscores).
# Environment variables take precedence over file values.

[xaas.database]
url              = "postgres://gadgetron:${PGPASSWORD}@localhost:5432/gadgetron"
# env: GADGETRON_XAAS_DATABASE_URL
max_connections  = 20
# env: GADGETRON_XAAS_DATABASE_MAX_CONNECTIONS
min_connections  = 2
# env: GADGETRON_XAAS_DATABASE_MIN_CONNECTIONS
acquire_timeout_ms = 3000
# env: GADGETRON_XAAS_DATABASE_ACQUIRE_TIMEOUT_MS
migrate_on_start = true
# env: GADGETRON_XAAS_DATABASE_MIGRATE_ON_START

[xaas.auth]
allow_test_keys  = false           # Production default. Set true only for development.
# env: GADGETRON_XAAS_AUTH_ALLOW_TEST_KEYS
# Startup emits tracing::warn! when true and no explicit dev-mode flag is set.
default_key_ttl  = "365d"
# env: GADGETRON_XAAS_AUTH_DEFAULT_KEY_TTL
tenant_cache_ttl_ms = 30000        # A17 — 30초 TTL
# env: GADGETRON_XAAS_AUTH_TENANT_CACHE_TTL_MS

[xaas.quota.defaults]
rpm             = 60               # Requests Per Minute
# env: GADGETRON_XAAS_QUOTA_DEFAULTS_RPM
tpm             = 1_000_000        # Tokens Per Minute
# env: GADGETRON_XAAS_QUOTA_DEFAULTS_TPM
concurrent_max  = 10               # Phase 1 은 config only
# env: GADGETRON_XAAS_QUOTA_DEFAULTS_CONCURRENT_MAX

[xaas.audit]
# D-20260411-09: 채널 용량 4096 (기존 1024 의 4배 확대)
channel_capacity  = 4096           # mpsc buffer
# env: GADGETRON_XAAS_AUDIT_CHANNEL_CAPACITY
batch_size        = 100
# env: GADGETRON_XAAS_AUDIT_BATCH_SIZE
flush_interval_ms = 1000
# env: GADGETRON_XAAS_AUDIT_FLUSH_INTERVAL_MS
# 배포 전 compliance review 필수. Phase 2 에 WAL/S3 fallback 추가 예정.

[logging]
# A15 — 프로덕션 기본 로그 레벨
# RUST_LOG=gadgetron_xaas=info,sqlx=warn (CLI 또는 환경변수 우선)
```

**A14 — `validate_xaas()` 함수 스펙**:

```rust
// gadgetron-xaas/src/config.rs
impl XaasConfig {
    /// Boot 시점에 `main.rs` 에서 호출 → fail-fast 검증.
    /// 실패 시 `GadgetronError::Config(...)` 반환하여 프로세스 종료.
    pub fn validate_xaas(&self) -> Result<(), GadgetronError> {
        // 1. DATABASE_URL 형식 (D-4 위반 방지)
        if !self.database.url.starts_with("postgres://")
            && !self.database.url.starts_with("postgresql://") {
            return Err(GadgetronError::Config(
                "xaas.database.url must start with postgres:// (D-4)".into()));
        }
        // 2. 채널 용량 하한 (너무 작으면 대량 drop)
        if self.audit.channel_capacity < 16 {
            return Err(GadgetronError::Config(
                "xaas.audit.channel_capacity must be >= 16".into()));
        }
        // 3. 쿼터 값 하한
        if self.quota.defaults.rpm == 0 || self.quota.defaults.tpm == 0 {
            return Err(GadgetronError::Config(
                "xaas.quota.defaults: rpm and tpm must be > 0".into()));
        }
        // 4. default_key_ttl humantime 파싱
        humantime::parse_duration(&self.auth.default_key_ttl)
            .map_err(|e| GadgetronError::Config(format!("default_key_ttl: {e}")))?;
        Ok(())
    }
}
```

**Validation 규칙 요약**:
- `xaas.database.url` 는 `postgres://` / `postgresql://` 로 시작 (D-4 위반 방지)
- `xaas.quota.defaults.rpm` > 0, `tpm` > 0 (A14)
- `xaas.audit.channel_capacity` >= 16 (너무 작으면 대량 drop)
- `xaas.auth.default_key_ttl` 은 humantime 형식 (`"365d"`, `"7d"`, `"24h"`)
- `xaas.audit.channel_capacity` 기본 4096 (D-20260411-09)

### 2.4 에러 & 로깅

**DX-6 — i18n scope (Phase 1)**: All `error_message()` return values are hardcoded `&'static str` in Rust source (`gadgetron-core/src/error.rs`). There is no message catalog, no locale detection, and no runtime string substitution in Phase 1. Extraction to a message catalog for internationalization is deferred to Phase 2 (out of scope for this design).

#### 2.4.1 Error Mapping — `sqlx::Error → GadgetronError::Database` (D-20260411-13)

**Round 3 변경 (A6, D-20260411-13)**: 이전 Round 1 에서 sqlx 에러를 `Config(String)` 으로 collapse 했던 임시 정책을 **폐기**. Production debug 시 root cause (pool timeout vs row not found vs connection failed vs constraint) 추적, Prometheus 라벨링, retry 정책 분기를 위해 `gadgetron-core` 에 `Database { kind, message }` variant 를 추가 (Track 1 chief-architect 작업).

**Leaf crate 보존**: `gadgetron-core` 는 여전히 `sqlx` 의존성 0. consumer crate (xaas) 가 helper function 으로 매핑 (orphan rule + crate boundary 동시 준수).

```rust
// gadgetron-xaas/src/error.rs  — pub(crate) helper
use gadgetron_core::error::{GadgetronError, DatabaseErrorKind};

/// D-20260411-13: 모든 sqlx call site 에서 사용.
/// `GadgetronError::Database { kind, message }` 로 변환하여
/// HTTP status mapping (§2.4.2) 과 retry 정책 (§2.4.4 tracing 표) 에 활용.
pub(crate) fn sqlx_to_gadgetron(e: sqlx::Error) -> GadgetronError {
    let kind = match &e {
        sqlx::Error::RowNotFound                  => DatabaseErrorKind::RowNotFound,
        sqlx::Error::PoolTimedOut                 => DatabaseErrorKind::PoolTimeout,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_)  => DatabaseErrorKind::ConnectionFailed,
        sqlx::Error::Database(_)                  => DatabaseErrorKind::Constraint,
        sqlx::Error::Migrate(_)                   => DatabaseErrorKind::MigrationFailed,
        _                                         => DatabaseErrorKind::Other,
    };
    GadgetronError::Database { kind, message: e.to_string() }
}
```

**호출 패턴 (모든 sqlx call site, no exception)**:
```rust
// PgKeyValidator
let row = sqlx::query_as!(KeyRow, "SELECT ... FROM api_keys WHERE key_hash = $1", hash)
    .fetch_one(&self.pool)
    .await
    .map_err(sqlx_to_gadgetron)?;

// PgTenantRegistry
sqlx::query!("INSERT INTO tenants (name) VALUES ($1)", name)
    .execute(&self.pool)
    .await
    .map_err(sqlx_to_gadgetron)?;

// PgAuditWriter::flush — UNNEST batch
sqlx::query!("INSERT INTO audit_log SELECT * FROM UNNEST($1::uuid[], ...)", ids)
    .execute(pool)
    .await
    .map_err(sqlx_to_gadgetron)?;
```

**`row not found` UX 예외**: `PgKeyValidator::validate` 에서 키가 없을 때는 일부러 `Database { RowNotFound }` 가 아닌 `GadgetronError::TenantNotFound` (unit variant) 를 반환 (HTTP 401 mapping 유지). DB layer 외 의도적인 lookup miss 는 도메인 에러 우선.

#### 2.4.1.1 `GadgetronError` variant 사용 (D-13 / D-20260411-13)

| 사용 지점 | variant | 메시지 예 |
|-----------|---------|----------|
| `ApiKey::parse` 실패 | `TenantNotFound` (unit) | — (error_message() = "Invalid API key. Verify your API key is correct and has not been revoked.") |
| 키 DB 조회 miss | `TenantNotFound` (unit) | — |
| 테넌트 suspended | `TenantNotFound` (unit) | — |
| Scope denied | `Forbidden` (unit) | — (error_message() = "Your API key does not have permission for this operation. Contact your administrator to grant the required scope.") |
| RPM 초과 | `QuotaExceeded { tenant_id: Uuid }` (struct) | — (error_message() = "Your API usage quota has been exceeded. Retry after the quota resets, or contact your administrator to increase limits.") |
| TPM 초과 | `QuotaExceeded { tenant_id: Uuid }` (struct) | — (error_message() = "Your API usage quota has been exceeded. Retry after the quota resets, or contact your administrator to increase limits.") |
| sqlx pool timeout (D-20260411-13) | `Database { PoolTimeout, .. }` | `"pool timeout: timed out waiting for connection"` |
| sqlx connection IO/TLS | `Database { ConnectionFailed, .. }` | `"connection failed: io error"` |
| sqlx constraint violation | `Database { Constraint, .. }` | `"constraint violation: duplicate key"` |
| sqlx migration | `Database { MigrationFailed, .. }` | `"migration failed: version 4"` |
| sqlx other | `Database { Other, .. }` | `"db error: protocol error"` |
| Billing overflow (Phase 2) | `Billing(String)` | `"cost overflow in req={req_id}"` |
| HF download 실패 (Phase 2) | `DownloadFailed(String)` | `"checksum mismatch"` |
| Stream 중단 (D-08) | `StreamInterrupted { reason }` | `"client abort at chunk 5"` |

**주의**:
- Phase 1 에서는 `HotSwapFailed` 사용하지 않음 (Phase 2 scheduler 용).
- **D-20260411-13 (A6)**: 이전 `Config(String)` 임시 매핑 폐기. `sqlx_to_gadgetron` helper 가 단일 진입점.
- **D-20260411-08 (A4/Track 1)**: `StreamInterrupted { reason: String }` variant 는 `gadgetron-core/src/error.rs` 에 구현 완료 (12개 variant 중 하나). 본 설계는 HTTP 매핑만 정의.
- **DX-4 — error_message() remediation hints**: The `Forbidden` and `QuotaExceeded` messages above include operator-facing remediation text. The actual implementation is in `gadgetron-core/src/error.rs` (`error_message()` method) and will be updated separately to match. Phase 1: all `error_message()` return values are hardcoded `&'static str` in Rust source (DX-6).

#### 2.4.2 HTTP Status Mapping (A3, D-08, D-20260411-13 반영)

`gadgetron-gateway` 의 에러 핸들러가 `GadgetronError` variant 를 OpenAI-compatible 에러 body 로 변환. Status code 와 body shape 은 아래 테이블로 고정:

| `GadgetronError` variant | HTTP | Response Body (OpenAI 호환) |
|:---|:---:|:---|
| `TenantNotFound` (unit — invalid/missing/revoked key) | **401** | `{"error":{"message":"...","type":"authentication_error","code":"tenant_not_found"}}` |
| `Forbidden` (unit — `require_scope` 실패) | **403** | `{"error":{"message":"...","type":"permission_error","code":"forbidden"}}` |
| `QuotaExceeded { tenant_id }` (struct — RPM) | **429** | `{"error":{"message":"...","type":"quota_error","code":"quota_exceeded","retry_after":12}}` |
| `QuotaExceeded { tenant_id }` (struct — TPM) | **429** | `{"error":{"message":"...","type":"quota_error","code":"quota_exceeded","retry_after":45}}` |
| `Billing` (Phase 2) | **402** | `{"error":{"message":"...","type":"api_error","code":"billing_error"}}` |
| `Database { kind: PoolTimeout }` (D-20260411-13) | **503** | `{"error":{"message":"...","type":"server_error","code":"db_pool_timeout"}}` + `Retry-After: 1` |
| `Database { kind: ConnectionFailed }` | **503** | `{"error":{"message":"...","type":"server_error","code":"db_connection_failed"}}` |
| `Database { kind: RowNotFound }` | **404** | `{"error":{"message":"...","type":"server_error","code":"db_row_not_found"}}` |
| `Database { kind: Constraint }` | **409** | `{"error":{"message":"...","type":"server_error","code":"db_constraint"}}` |
| `Database { kind: MigrationFailed }` | **500** | `{"error":{"message":"...","type":"server_error","code":"db_migration_failed"}}` |
| `Database { kind: QueryFailed \| Other }` | **500** | `{"error":{"message":"...","type":"server_error","code":"db_query_failed"/"db_error"}}` |
| `DownloadFailed` (Phase 2) | **500** | `{"error":{"message":"...","type":"api_error","code":"download_failed"}}` |
| `HotSwapFailed` (Phase 2) | **503** | `{"error":{"message":"...","type":"api_error","code":"hotswap_failed"}}` |
| `StreamInterrupted { reason }` (D-20260411-08) | **499**† | `{"error":{"message":"...","type":"api_error","code":"stream_interrupted"}}` |
| `Config` | **500** | `{"error":{"message":"...","type":"invalid_request_error","code":"config_error"}}` (generic, PII-safe) |
| `Provider(String)` | **502** | `{"error":{"message":"...","type":"api_error","code":"provider_error"}}` |

**Database variants → 6 distinct HTTP codes** (D-20260411-13): 503 (PoolTimeout / ConnectionFailed), 404 (RowNotFound), 409 (Constraint), 500 (MigrationFailed / QueryFailed / Other). 7 variants → 6 codes.

> **†**: `499 Client Closed Request` 는 nginx 관례적 코드. Phase 1 은 SSE 스트림 중단에 한해 이 코드를 송출 (proxy/gateway 호환성 이슈 우려 시 `503` 로 fallback 가능 — Round 2 에서 확정). D-20260411-08 으로 `StreamInterrupted` variant 추가 확정.

**OpenAI 호환 에러 body** schema (3-field, canonical `gadgetron-core` methods):
```json
{
  "error": {
    "message": "string — human-readable (self.error_message())",
    "type":    "string — OpenAI taxonomy (self.error_type())",
    "code":    "string — machine-readable (self.error_code())"
  }
}
```
이 3개 필드는 `GadgetronError` 의 `error_message()` / `error_type()` / `error_code()` 메서드에서 가져옴 (`gadgetron-core/src/error.rs` 구현). `rate_limit_exceeded` / `quota_exceeded` 에는 `retry_after: <seconds>` 추가 (HTTP `Retry-After` 헤더와 동기화).

**`IntoResponse` 변환 함수** — `gadgetron-gateway/src/error.rs` 에 구현. 핵심 매칭 (pseudo-code):

```rust
use gadgetron_core::error::DatabaseErrorKind;

impl IntoResponse for GadgetronError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.http_status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::json!({
            "error": {
                "message": self.error_message(),   // Fix 6: human-readable (per gadgetron-core)
                "type":    self.error_type(),       // Fix 6: OpenAI taxonomy
                "code":    self.error_code(),       // Fix 6: machine-readable
            }
        });

        // Pattern matching for additional fields (retry_after) — canonical variant shapes:
        // TenantNotFound  — unit variant (NO payload string)
        // Forbidden       — unit variant (NO payload string)
        // QuotaExceeded   — struct variant: QuotaExceeded { tenant_id: Uuid }
        // Database        — struct variant: Database { kind: DatabaseErrorKind, message: String }
        // StreamInterrupted — struct variant: StreamInterrupted { reason: String }
        // All others are tuple variants with one String field.
        match &self {
            GadgetronError::QuotaExceeded { .. } => {
                // Emit Retry-After hint. Actual wait_secs computed by enforcer and embedded via
                // a separate body field when available. Phase 1: static hint only.
            }
            _ => {}
        }

        (status, axum::Json(body)).into_response()
    }
}
```

호출 지점: `gadgetron-gateway/src/routes/chat.rs` handler 의 `?` propagation · `AuthLayer`/`QuotaLayer` 의 early return · `gadgetron-gateway/src/sse.rs` 가 SSE 스트림 중단 시 `StreamInterrupted` 로 `event: error` frame flush 후 close.

#### 2.4.3 `QuotaEnforcer::record_post()` Tower layer 위치 (A2)

**미들웨어 체인 순서** (Phase 1, `docs/modules/gateway-routing.md §5.2` 기반):

```
Request
  │
  ▼
Auth ──▶ RateLimit ──▶ QuotaCheckPre ──▶ Routing ──▶ ProtocolTranslate(in)
   │           │              │                                │
   │ insert    │ 429?         │ insert QuotaToken              ▼
   │ TenantCtx │              │ (Request::extensions)    Provider
   │           │              │                                │
   ▼           ▼              ▼                                ▼
 401/403     429           429                  ProtocolTranslate(reverse)
                                                                │
                                                                ▼
                       QuotaRecordPost  ◀── Response (body + Usage in ext)
                       └─tokio::spawn(record_post) — fire & forget
                                                                │
                                                                ▼
                                                      Metrics ──▶ Response
```

**핵심 invariant**:
1. `QuotaCheckPre` (pre-layer) 는 `ValidatedKey → TenantContext` 다음, `Routing` 이전에 실행.
2. `QuotaRecordPost` (post-layer) 는 `Provider` 응답이 돌아온 **후** 실행되지만, `Response` 스트림이 client 로 나가기 **전에** `tokio::spawn` 으로 off-load 하여 latency 에 영향 없음.
3. `QuotaCheckPre` 실패 → early 429 (handler 미호출).
4. `QuotaRecordPost` 실패 → **요청 실패 유발 안 함** (log + metric 만).

**`QuotaToken` lifetime** (pre → handler → post, Tower pseudo-code):

```rust
use tower::ServiceExt;

// 1) Pre-layer: QuotaCheckPre — TenantContext extensions 로 token 삽입
async fn pre(req: Request) -> Result<Request, Response> {
    let ctx  = req.extensions().get::<TenantContext>().cloned()
        .ok_or_else(|| err(401, "missing tenant ctx"))?;
    let tok  = enforcer.check_pre(&ctx, extract_chat_request(&req)?).await
        .map_err(IntoResponse::into_response)?;
    req.extensions_mut().insert::<QuotaToken>(tok);
    Ok(req)
}

// 2) Handler: 요청 처리 후 token + Usage 를 Response extensions 에 이식
async fn chat_handler(Extension(ctx): Extension<TenantContext>, mut req: Request) -> Result<Response, GadgetronError> {
    let token   = req.extensions_mut().remove::<QuotaToken>()
        .ok_or_else(|| GadgetronError::Config("missing quota token".into()))?;
    let result  = router.route(&ctx, extract_body(req).await?).await?;
    let mut res = result.into_response();
    res.extensions_mut().insert(token);
    res.extensions_mut().insert(result.usage);
    Ok(res)
}

// 3) Post-layer: tower::ServiceExt::map_result — fire-and-forget record_post
let svc = inner.map_result(|r: Result<Response, _>| {
    if let Ok(ref mut resp) = r {
        if let (Some(token), Some(usage)) = (
            resp.extensions_mut().remove::<QuotaToken>(),
            resp.extensions().get::<Usage>().cloned(),
        ) {
            let enforcer = enforcer.clone();
            tokio::spawn(async move {
                if let Err(e) = enforcer.record_post(token, &usage).await {
                    tracing::warn!(error = %e, "xaas.quota.record_post failed");
                    metrics::counter!("gadgetron_xaas_quota_record_post_failed_total").increment(1);
                }
            });
        }
    }
    r
});
```

**왜 `tokio::spawn` fire-and-forget 인가**: D-20260411-09 와 동일한 resilience 원칙 — 과금 post 실패는 요청 실패로 이어지지 않는다. `record_post` 의 `TokenBucket::adjust()` 는 μs 단위지만 Phase 2 에서 DB write-back 을 포함할 수 있어 response path 에서 분리. `tokio::spawn` 실패 (runtime 종료 중) 는 drop → `JoinError` 로그만.

> **Cross-track 의존성**: `gadgetron-gateway` 의 `map_result` / `QuotaRecordPostLayer` 실제 타입은 gateway-router-lead Week 3 설계 문서 (`docs/design/gateway/phase1.md`) 가 최종 확정. 본 문서는 **xaas 측 계약**만 명시.

#### 2.4.4 Tracing spans (A7 — `#[tracing::instrument]` 표준화)

**A7 정책**: `KeyValidator`/`TenantRegistry`/`QuotaEnforcer`/`AuditWriter` 4 trait 의 모든 method impl 에 `#[tracing::instrument]` 부착, 공통 필드 `tenant_id` / `request_id` / `cache_hit` / `error_kind` 를 자동 capture. middleware 에서 설정한 `request_id` 가 child span 으로 상속됨 (gateway 의 `TraceLayer`).

```rust
// PgKeyValidator
#[tracing::instrument(skip(self, raw_key), fields(key_kind, tenant_id, cache_hit))]
async fn validate(&self, raw_key: &str) -> Result<ValidatedKey> { /* §2.2.2 */ }

// PgTenantRegistry
#[tracing::instrument(skip(self), fields(tenant_id, request_id))]
async fn get(&self, tenant_id: Uuid) -> Result<Tenant> { /* ... */ }

// PgQuotaEnforcer
#[tracing::instrument(skip(self, req), fields(tenant_id = %ctx.tenant_id, request_id = %ctx.request_id, estimated_tokens))]
async fn check_pre(&self, ctx: &TenantContext, req: &ChatRequest) -> Result<QuotaToken> { /* ... */ }

#[tracing::instrument(skip(self, token, usage), fields(tenant_id = %token.tenant_id, request_id = %token.request_id, actual_tokens = usage.total_tokens, delta))]
async fn record_post(&self, token: QuotaToken, usage: &Usage) -> Result<()> { /* ... */ }

#[tracing::instrument(skip(self), fields(tenant_id = %tenant_id, refund_tokens = tokens))]
async fn refund_estimate(&self, tenant_id: Uuid, tokens: u32) -> Result<()> { /* ... */ }

// PgAuditWriter
#[tracing::instrument(skip(self, entry), fields(tenant_id = %entry.tenant_id, request_id = %entry.request_id))]
async fn write(&self, entry: AuditEntry) -> Result<()> { /* ... */ }
```

| span | level | fields |
|------|-------|--------|
| `xaas.auth.parse` | trace | `kind` |
| `xaas.auth.validate` | debug | `key_kind`, `tenant_id`, **`cache_hit`** |
| `xaas.auth.cache.invalidate` | info | `hash_prefix` (8 char) |
| `xaas.tenant.get` | debug | `tenant_id`, `request_id` |
| `xaas.quota.check_pre` | debug | `tenant_id`, `request_id`, `estimated_tokens`, `rpm_remaining`, `tpm_remaining` |
| `xaas.quota.record_post` | trace | `tenant_id`, `request_id`, `actual_tokens`, `delta` |
| `xaas.quota.refund_estimate` | warn | `tenant_id`, `refund_tokens` (request cancel 시) |
| `xaas.audit.write` | trace | `request_id`, `tenant_id` |
| `xaas.audit.batch_flush` | info | `batch_size`, `lag_ms`, `result` |
| `xaas.audit.drop` | warn | `reason=channel_full`, `tenant_id`, `request_id` |
| `xaas.db.error` | warn | `kind: DatabaseErrorKind`, `query_name` — `message` field is suppressed to prevent DB schema leakage (SEC-5) |

**SEC-5 — `xaas.db.error` message suppression**: The `message` field from `sqlx::Error::to_string()` (which may contain table names, column names, or constraint names) MUST NOT appear in tracing spans. The `sqlx_to_gadgetron` helper stores the message in `GadgetronError::Database { message }` for internal structured logging only; the span records `kind: DatabaseErrorKind` only. The `message` field is never forwarded to HTTP response bodies or external observability pipelines.

**A15**: 프로덕션 기본 환경변수 `RUST_LOG=gadgetron_xaas=info,sqlx=warn` (배포 템플릿 `deploy/production.env` 에 명시).
모든 span 은 `tracing` crate 기본, middleware 에서 `request_id` 필드 상속 (gateway 가 설정).

#### 2.4.5 Cost 변환 정책 (D-20260411-06, A7 반영)

D-20260411-06 옵션 A 확정: **routing 은 `f64`, billing 은 `i64 cents`**. 변환 지점은 유일하게 `gadgetron-xaas/src/quota/enforcer.rs::compute_cost_cents`.

```rust
use gadgetron_router::metrics::CostEntry;
use gadgetron_core::provider::Usage;

/// D-20260411-06: 유일한 f64 → i64 cents 변환 지점.
/// Phase 1 은 `cost_cents = 0` (구조만 유지), Phase 2 에 실계산 도입.
pub fn compute_cost_cents(usage: &Usage, rate: &CostEntry) -> i64 {
    let _ = (usage, rate);
    0  // Phase 1: audit_log.cost_cents 고정 0. Phase 2 에 billing engine 도입.
}
```

Phase 2 계산 개요 (billing 크레이트 분리 시 이동):
- `rate.input_usd_per_ktoken: f64 → millicents/ktoken (i64)` 변환.
- `in_mc * usage.prompt_tokens / 1_000 + out_mc * usage.completion_tokens / 1_000`, `saturating_mul` 필수.
- `total_mc / 1_000 → cents` (round down, 보수적).

요약:
- f64 산술은 routing decision (`estimated_cost_usd`) 에서만 사용 → "대략 저렴한 것" 선택의 hot path.
- i64 산술은 `audit_log.cost_cents` 와 Phase 2 `billing_ledger` 에만 사용.
- 본 함수는 `QuotaEnforcer::record_post` 에서 호출되며 결과는 `AuditEntry.cost_cents` 로 전달.

### 2.5 의존성 (Cargo.toml)

```toml
[package]
name = "gadgetron-xaas"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
gadgetron-core = { workspace = true }

# D-4, D-9: PostgreSQL + migrations
sqlx = { version = "0.8", features = ["runtime-tokio","postgres","uuid","chrono","macros","migrate"] }

tokio       = { workspace = true }
async-trait = "0.1"
serde       = { workspace = true }
serde_json  = { workspace = true }
uuid        = { workspace = true }
chrono      = { workspace = true }
humantime   = "2"                # A14 validate_xaas — default_key_ttl 파싱

# D-11 키 해시 + base62 (32-char × log₂(62) ≈ 190 bits entropy ≫ UUIDv4 122 bits)
sha2   = "0.10"
hex    = "0.4"
base62 = "2"

# D-20260411-12: PgKeyValidator LRU cache (10k entries, 10min TTL).
# moka 는 segmented LRU + TinyLFU eviction, async-aware (`future::Cache`).
# 대안 검토: lru 0.12 (sync only — async lock contention) · cached 0.x (proc-macro 위주, capacity TTL 미지원).
moka = { version = "0.12", features = ["future"] }

# Middleware (gateway 와 shared workspace dep)
axum       = { workspace = true }
tower      = { workspace = true }
tower-http = { workspace = true }
http       = "1"

dashmap  = { workspace = true }   # tenant cache (A17)
tracing  = { workspace = true }
metrics  = "0.24"                 # Prometheus
thiserror = { workspace = true }

[dev-dependencies]
gadgetron-testing = { path = "../gadgetron-testing" }   # D-20260411-05 (Track 2)
tokio = { workspace = true, features = ["test-util","macros","rt-multi-thread"] }
proptest = "1"
```

**의존성 정당화** (중복 제외): `sqlx 0.8` (D-4/D-9) · `sha2/hex/base62` (D-11; 32-char × log₂(62) ≈ 190 bits 엔트로피, UUIDv4 122 bits 보다 우월) · `dashmap` (tenant cache) · `moka 0.12` (D-20260411-12 PgKeyValidator LRU, async-native, TinyLFU 제거) · `metrics 0.24` (Phase 1 Prometheus subset) · `proptest` (bucket refill / base62) · `humantime` (`default_key_ttl` 파싱 for A14).

### 2.6 Operator Walkthrough

Complete sequence to go from zero to a verified XaaS request. All commands are copy-pasteable. Replace `<uuid>` and key placeholders with actual values printed by each step.

**Prerequisites**: PostgreSQL 16+ running, `DATABASE_URL` set in environment or `gadgetron.toml`.

```
# Step 1: Configure [xaas] section in gadgetron.toml
#   Set database_url, defaults (rpm, tpm), audit channel_capacity.
#   Minimum config:
#     [xaas.database]
#     url = "postgres://gadgetron:secret@localhost:5432/gadgetron"
#     migrate_on_start = true   # dev mode; set false in production

# Step 2: Start the server — migrations run automatically (migrate_on_start=true)
gadgetron serve -c gadgetron.toml
# Expected output: "sqlx migrations applied", "listening on 0.0.0.0:8080"

# Step 3: Create a tenant — prints tenant_id UUID
gadgetron tenant create --name "my-team"
# Output example: tenant_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890"

# Step 4: Create an API key for the tenant — gad_live_... printed ONCE, not stored raw
gadgetron key create --tenant-id a1b2c3d4-e5f6-7890-abcd-ef1234567890 --scope openai-compat
# Output example: key = "gad_live_Xk9mNpQrVwZaBcDeFgHiJkLmNo1234"  id = "<key-uuid>"
# Save this key securely — it cannot be retrieved again.

# Step 5: Send a chat completions request
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer gad_live_Xk9mNpQrVwZaBcDeFgHiJkLmNo1234" \
  -H "Content-Type: application/json" \
  -d '{"model":"default","messages":[{"role":"user","content":"hello"}]}'
# Expected: 200 JSON with choices[]

# Step 6: Check quota status (requires XaasAdmin key)
curl http://localhost:8080/api/v1/xaas/quotas/a1b2c3d4-e5f6-7890-abcd-ef1234567890 \
  -H "Authorization: Bearer gad_live_<admin-key-with-management-scope>"
# Expected: {"daily_limit":...,"daily_used":...,"monthly_limit":...,"monthly_used":...}

# Step 7: Verify audit log was written
psql "$DATABASE_URL" -c \
  "SELECT count(*) FROM audit_log WHERE tenant_id = 'a1b2c3d4-e5f6-7890-abcd-ef1234567890'"
# Expected: count > 0 (may be up to 1s delayed due to batch flush interval)
```

### 2.7 Runbook Alert Mapping

Metrics emitted by `gadgetron-xaas`. Set up alerts on these in Prometheus/Grafana. Each row is a distinct operational issue with a specific remediation path.

| Metric | Alert Condition | Diagnostic Check | Remediation |
|---|---|---|---|
| `gadgetron_xaas_audit_dropped_total` | Counter increases by > 0 within any 5-minute window | Is PG pool saturated? (`gadgetron_xaas_db_pool_wait_seconds` P99)? Is mpsc channel at capacity (`channel_capacity`)? | Increase `xaas.audit.channel_capacity` (env: `GADGETRON_XAAS_AUDIT_CHANNEL_CAPACITY`); or scale PG connection pool (`max_connections`); or increase PG throughput |
| `gadgetron_xaas_quota_refund_failed_total` | Counter increases by > 0 | DB write failure during quota adjustment after request cancellation — check `xaas.db.error` spans for `kind=PoolTimeout` or `ConnectionFailed` | Check PG connectivity (`psql $DATABASE_URL -c "SELECT 1"`); verify `DATABASE_URL` env var is set and accessible |
| `gadgetron_xaas_auth_validation_duration_seconds` P99 > 50ms | Auth latency spike — likely moka cache miss rate increased + PG slow queries | Check `gadgetron_xaas_auth_cache_misses_total` rate; run `EXPLAIN ANALYZE SELECT ... FROM api_keys WHERE key_hash = $1` on PG | Verify `api_keys_hash_idx` index exists; increase moka TTL (`time_to_live`) if miss rate is high; check PG slow query log |

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 상위 → 현재 → 하위 의존 크레이트

```
+----------------+
| gadgetron-cli  |
+-------+--------+
        |
+-------v----------+                 +----------------+
| gadgetron-gateway|--- depends ---->| gadgetron-xaas |
+-------+----------+                 +-------+--------+
        |                                    |
        v                                    v
+-------+----------+                 +----------------+
| gadgetron-router |----- . . . --->| gadgetron-core |
+-------+----------+                 +----------------+
        |                                    ^
        v                                    |
+-------+----------+                          |
| gadgetron-provider|----------- depends ----+
+------------------+
```

- **`gadgetron-xaas` 의 의존**: `gadgetron-core` 만 (타입/에러). 외부: `sqlx`, `tokio`, `axum`, `tower`.
- **`gadgetron-gateway`** 이 `gadgetron-xaas` 를 의존 (Phase 1 에 `Cargo.toml` 에 추가 필요).
- **`gadgetron-router`**, **`gadgetron-provider`** 는 `gadgetron-xaas` 를 의존하지 **않음** (순환 방지).
- **`gadgetron-testing`** (Track 2) 는 `gadgetron-xaas` 를 `dev-dependency` 로 import.

### 3.2 데이터 흐름 다이어그램 (Phase 1)

핵심 체인 (상세 pre/post 위치는 §2.4.3, 전체 미들웨어 순서는 `platform-architecture.md §2.B.8` 참조):

```
Client ─HTTP─▶ [RequestBodyLimit 4MB] ─▶ [TraceLayer] ─▶ [RequestId]
               (pre-auth layers — owned by gateway-router-lead, see platform-architecture §2.B.8)
                   │
                   ▼
              [AuthLayer] ─▶ [TenantContextLayer] ─▶ [ScopeGuardLayer] ─▶ [QuotaCheckPre] ─▶ [Routing]
                   │                                        │                   │
                   ▼                                        ▼                   ▼
            validate Bearer                        require_scope check    insert QuotaToken
            → insert TenantCtx

              ─▶ [ProtocolTranslate in] ─▶ [Provider] ─▶ [ProtocolTranslate out]
                                              │
                                              ▼
                                      Response + Usage
                                              │
              ─▶ [QuotaRecordPost (map_result)] ─▶ [AuditLayer] ─▶ Client
                        │                               │
              tokio::spawn(record_post)       AuditWriter::write
                                                        │
                                                        ▼
                                            mpsc(4096) → batcher (100/1s)
                                                        │
                                                        ▼
                                                 audit_log (PostgreSQL)
```

### 3.3 타 서브에이전트 도메인과의 인터페이스 계약

| 상대 | 계약 | 비고 |
|------|------|------|
| `chief-architect` (Track 1) | `GadgetronError` 12개 variant (현재 구현 완료 in `gadgetron-core/src/error.rs`): `Config`, `Provider`, `Routing`, `StreamInterrupted { reason }`, `QuotaExceeded { tenant_id }`, `TenantNotFound`, `Forbidden`, `Billing`, `DownloadFailed`, `HotSwapFailed`, `Database { kind, message }`, `Node { kind, message }` | 구현 완료 — `gadgetron-core/src/error.rs` |
| `chief-architect` | `ChatRequest.max_tokens: Option<u32>` 기존 필드 사용 | TPM estimate 용 |
| `chief-architect` | `Usage { total_tokens: u32 }` 기존 필드 사용 | record_post 용 |
| `gateway-router-lead` | `AuthLayer` / `QuotaLayer` / `AuditLayer` 를 `server.rs` 에 마운트 | Phase 1 MVP (D-6) |
| `gateway-router-lead` | `request_id: Uuid` 를 Request extensions 에 주입 (auth 이전) | `TenantContext.request_id` 로 복사 |
| `devops-sre-lead` | PostgreSQL 16+ 배포, `DATABASE_URL` 시크릿 관리 | Phase 1 |
| `devops-sre-lead` | `sqlx migrate run` CI step 추가 | Phase 1 |
| `devops-sre-lead` | DB role `gadgetron_app` has INSERT-only on `audit_log` (SEC-3: `REVOKE UPDATE, DELETE, TRUNCATE; GRANT INSERT`) — apply `002_roles.sql` during initial DB provisioning | Phase 1 |
| `qa-test-architect` (Track 2) | `gadgetron-testing::harness::pg::PgTestContext` 제공 | testcontainers 기반 |
| `qa-test-architect` | `gadgetron-testing::mocks::provider::MockLlmProvider` | auth/quota 통합 테스트에서 사용 |

### 3.4 `docs/reviews/pm-decisions.md` **D-12 크레이트 경계표** 준수 여부

D-20260411-03 에 의해 D-12 에 `gadgetron-xaas` 행 추가:

| 타입 | 크레이트 | 파일 |
|------|----------|------|
| `Scope` | **gadgetron-core** | `src/context.rs` |
| `TenantContext` | **gadgetron-core** | `src/context.rs` |
| `QuotaSnapshot` | **gadgetron-core** | `src/context.rs` |
| `ApiKey`, `KeyKind` | gadgetron-xaas | `src/auth/key.rs` |
| `KeyValidator`, `ValidatedKey`, `PgKeyValidator` | gadgetron-xaas | `src/auth/validator.rs` |
| `AuthLayer` | gadgetron-xaas | `src/auth/middleware.rs` |
| `Tenant`, `TenantStatus`, `TenantSpec` | gadgetron-xaas | `src/tenant/model.rs`, `src/tenant/registry.rs` |
| `TenantRegistry`, `PgTenantRegistry` | gadgetron-xaas | `src/tenant/registry.rs` |
| `QuotaConfig` | gadgetron-xaas | `src/quota/config.rs` |
| `QuotaEnforcer`, `QuotaToken`, `PgQuotaEnforcer` | gadgetron-xaas | `src/quota/enforcer.rs` |
| `TokenBucket` | gadgetron-xaas | `src/quota/bucket.rs` |
| `AuditEntry` | gadgetron-xaas | `src/audit/entry.rs` |
| `AuditWriter`, `PgAuditWriter` | gadgetron-xaas | `src/audit/writer.rs` |

- **`Scope`, `TenantContext`, `QuotaSnapshot` 는 `gadgetron-core::context` 에 배치** (codebase divergence 반영 — 이들은 cross-cut 타입으로 core 에 이동됨)
- gadgetron-xaas 의 `AuthLayer` 가 TenantContext 를 **구성(construct)** 하고 Request extensions 에 삽입. 타입 정의는 core.
- `gadgetron-gateway` 는 이들 타입을 re-export 없이 consume 만.
- Phase 2 의 `billing/`, `agent/`, `catalog/`, `gpuaas/` 모듈 추가 시에도 D-12 한 행만 update.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

#### 4.1.1 `ApiKey::parse` (src/auth/key.rs)

| 케이스 | 입력 | 기대 결과 | invariant |
|--------|------|-----------|-----------|
| valid live | `gad_live_abcdefghijklmnopqrstuvwxyz123456` | `Ok(kind=Live)` | suffix 32 chars |
| valid test | `gad_test_XYZ01234567890abcdefghijklmnop` | `Ok(kind=Test)` | base62 only |
| valid virtual | `gad_vk_abcdef012345_XYZ01234567890abcdefghijklmnop` | `Ok(kind=Virtual, tenant_short="abcdef012345")` | 12+32 split |
| empty | `""` | `Err(TenantNotFound)` | immediate reject |
| too short live | `gad_live_short` | `Err(TenantNotFound)` | suffix length check |
| too long live | `gad_live_` + 50 char | `Err(TenantNotFound)` | length == 32 |
| unknown prefix | `gdt_live_xxx...` | `Err(TenantNotFound)` | D-11 rejects old prefix |
| non-base62 | `gad_live_aaa$$$aaa...` | `Err(TenantNotFound)` | charset check |
| vk no underscore | `gad_vk_abcdef012345XYZ012...` | `Err(TenantNotFound)` | split required |
| vk bad tenant_short | `gad_vk_short_XYZ01234567890abcdefghijklmnop` | `Err(TenantNotFound)` | 12-char tenant_short |

**Property-based (proptest)**:
- `prop::string::string_regex("[A-Za-z0-9]{32}")` 로 랜덤 32-char base62 생성, `gad_live_{s}` 파싱 후 `suffix == s` 재확인
- Invariant: `ApiKey::parse(valid).unwrap().raw == input` (라운드트립)
- Invariant: `hash_hex(k1) == hash_hex(k2)` iff `k1.raw == k2.raw`

#### 4.1.2 `PgKeyValidator` (src/auth/validator.rs)

- **Fixture**: testcontainers Postgres + `sqlx migrate run`, `tenants` + `api_keys` seed
- **Cases**:
  - Insert key → validate returns `ValidatedKey { tenant_id, kind=Live }`
  - Validate unknown key → `Err(GadgetronError::TenantNotFound)` (unit variant)
  - Revoked key (`revoked_at IS NOT NULL`) → `Err(GadgetronError::TenantNotFound)` (unit variant)
  - Tenant suspended (status='suspended') → `Err(GadgetronError::TenantNotFound)` (unit variant)
  - Valid key → `last_used_at` 업데이트 확인 (eventual, `tokio::task::yield_now` 후 재조회)
  - Virtual key: tenant_short 파싱 + 해당 tenant 조회
  - **SEC-4 — Revoke + invalidate_key**: Insert key → validate (cache populated) → DELETE handler calls `invalidate_key(hash).await` → validate same key again → `Err(GadgetronError::TenantNotFound)` (cache miss, DB confirms `revoked_at IS NOT NULL`). Verifies that revocation is immediate and does not rely on TTL expiry.

- **Mock**: `KeyValidator` trait 의 `MockKeyValidator` 는 `gadgetron-testing::mocks::xaas::MockKeyValidator` 로 제공 (Track 2).

#### 4.1.3 `TokenBucket` (src/quota/bucket.rs)

| 케이스 | 시나리오 | 기대 |
|--------|----------|------|
| fresh | `new(60)`, `try_consume(1)` | `Ok(59)` |
| drain | 60회 consume | 마지막 호출 `Ok(0)` |
| overdrain | 61번째 | `Err(wait_secs >= 1)` |
| refill | `tokio::time::pause` + `advance(30s)` | 30 tokens 회복 |
| partial refill | `consume(30)`, advance(15s), `consume(10)` | `Ok(remaining)` |
| adjust refund | `consume(100)`, `adjust(+50)` | 50 refund (clamped) |
| adjust clamp | `max=60`, `adjust(+1000)` | tokens == 60 |

**Tokio time control**:
```rust
#[tokio::test(start_paused = true)]
async fn refill_correctness() {
    let mut b = TokenBucket::new(60);
    assert!(b.try_consume(60).is_ok());
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(b.try_consume(10), Ok(20));  // 30 refilled, 10 consumed
}
```

**Property-based**:
- Invariant: `tokens` 항상 `[0, max_tokens]` 범위
- Invariant: `consume(a) + consume(b) ≡ consume(a+b)` (동일 상태)
- Invariant: refill 은 monotonically nondecreasing

#### 4.1.4 `QuotaEnforcer` 상태머신

- `check_pre` → `QuotaToken` 발급, RPM bucket -1, TPM bucket -estimate
- `record_post(token, Usage { total_tokens: actual })` → TPM bucket `adjust(estimate - actual)`
- RPM 초과: `Err(QuotaExceeded { tenant_id })` (struct variant — tenant_id = ctx.tenant_id)
- TPM 초과: `Err(QuotaExceeded { tenant_id })` (struct variant — tenant_id = ctx.tenant_id)
- `record_post` 호출 없이 `check_pre` 만: TPM 영구 consume (Phase 1 의도)

#### 4.1.5 `PgAuditWriter` 배치/오버플로우

- **Fixture**: testcontainers Postgres + `audit_log` 테이블
- **Case 1 — Batch on size**: 100 개 `write()` → flush 1회 발생, DB `SELECT COUNT(*) FROM audit_log = 100`
- **Case 2 — Batch on time**: 10 개 `write()`, 2초 대기 → flush 발생, DB count 10
- **Case 3 — Channel full**: capacity=4 로 생성, 1000 개 write → drop 발생, `gadgetron_xaas_audit_dropped_total > 0`
- **Case 4 — Writer closed**: `drop(writer)` 후 `write()` → `Err(Config)` (실제로는 Phase 1 에서는 Ok 반환 — 테스트는 behavior 기준으로 작성)

#### 4.1.5b Graceful Shutdown Timeout Test (Round 2 A3, 신규)

**목적**: `PgAuditWriter::flush()` 5 초 timeout 을 **virtual time** (`#[tokio::test(start_paused = true)]`) 으로 deterministic 검증, timeout metric 증가 + 잔여 엔트리 관찰 가능성 보장 (D-20260411-09 옵션 C + §2.2.4 shutdown 체인).

**Fixture**: `tokio::time::pause` + `PgAuditWriter` + `SlowMockPool` (batch insert 내부 `sleep(10s)`).

```rust
#[tokio::test(start_paused = true)]
async fn audit_flush_graceful_shutdown_timeout_virtual_time() {
    let pool = create_slow_mock_pool().await;   // INSERT 가 10s sleep
    let writer = Arc::new(PgAuditWriter::spawn(pool).await.unwrap());
    for i in 0..50 { writer.write(test_entry(i)).await.unwrap(); }

    let flush_task = tokio::spawn({
        let writer = writer.clone();
        async move { tokio::time::timeout(Duration::from_secs(5), writer.flush()).await }
    });
    tokio::time::advance(Duration::from_secs(6)).await;   // timeout boundary 초과

    let result = flush_task.await.unwrap();
    assert!(result.is_err(), "flush should have timed out");
    let counter = metrics::counter!("gadgetron_xaas_audit_shutdown_timeout_total");
    assert_eq!(counter.get_value(), 1);
    assert!(writer.pending_count().await > 0, "expected partial flush");
}
```

**Timeout 계약**: `flush()` → `Err(Config("flush timeout after 5s"))` · `gadgetron_xaas_audit_shutdown_timeout_total` +1 · `tracing::error!(pending)` · unflushed entries **drop** (Phase 1, D-20260411-09). Phase 2: WAL fallback + 다음 startup retry (`src/audit/fallback.rs`).

**회귀 방지**: SIGTERM flush 호출 보장 · timeout metric 관찰 · compliance drop rate 증명 · virtual time → CI wall-clock 부담 없음. **Cross-ref**: §2.2.4 `with_graceful_shutdown` · §4.1.5 Case 4 (동기 drop) vs 본 케이스 (timeout 경로).

#### 4.1.6 `AuthLayer` 미들웨어

- **Fixture**: `MockKeyValidator` 주입된 tower::Service
- **Cases**:
  - 200: `Authorization: Bearer gad_live_...` → handler 호출, `TenantContext` 주입됨
  - 401: 헤더 없음 → `{ "error": "missing_api_key" }`
  - 403: invalid key → `{ "error": "invalid_api_key" }`
  - 403: expired/revoked → `{ "error": "invalid_api_key" }`
  - `Authorization: Basic xxx` → 401 (Bearer 만 허용)

#### 4.1.7 Migration Idempotency Test (Round 2 A2, 신규)

**목적**: `src/db/migrations/` 를 **2 회 연속 실행**해도 스키마·데이터·`_sqlx_migrations` 가 변하지 않음을 증명 (운영 hot-upgrade / rollback 재실행 안전성, D-4 · D-9 · §2.1.4 A6). Dev = `migrate_on_start=true` (Q-5 C) 로 재시작마다 자동 실행 → **1 회만 테스트하면 idempotency 버그가 production 에서 처음 드러난다.**

**Fixture**: `PgHarness::fresh()` — 테스트당 fresh PostgreSQL testcontainer (`gadgetron-testing::harness::pg::PgTestContext::new()` rename 예정).

```rust
#[tokio::test]
async fn migration_idempotent_double_run() -> Result<(), Box<dyn std::error::Error>> {
    let pg = PgHarness::fresh().await?;
    let pool = pg.pool();

    sqlx::migrate!("src/db/migrations").run(pool).await?;               // 1st
    let schema_v1 = capture_schema_snapshot(pool).await?;
    let sqlx_v1 = sqlx::query_scalar!("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(pool).await?;

    sqlx::query!("INSERT INTO tenants (name) VALUES ('test-tenant')").execute(pool).await?;

    sqlx::migrate!("src/db/migrations").run(pool).await?;               // 2nd — no-op
    let schema_v2 = capture_schema_snapshot(pool).await?;
    let sqlx_v2 = sqlx::query_scalar!("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(pool).await?;

    assert_eq!(schema_v1, schema_v2, "schema mutated on re-run");
    assert_eq!(sqlx_v1, sqlx_v2, "migration version advanced");
    let count = sqlx::query_scalar!("SELECT COUNT(*) FROM tenants").fetch_one(pool).await?;
    assert_eq!(count, Some(1), "data lost on re-run");
    Ok(())
}

async fn capture_schema_snapshot(pool: &PgPool) -> Result<String, sqlx::Error> {
    let tables: Vec<String> = sqlx::query_scalar!(
        "SELECT table_name::text FROM information_schema.tables
         WHERE table_schema = 'public' ORDER BY table_name"
    ).fetch_all(pool).await?;
    Ok(tables.join("\n"))
}
```

**Assertions**: (1) schema snapshot 동일 (`CREATE TABLE IF NOT EXISTS` 검증) · (2) `_sqlx_migrations` MAX(version) 동일 (중복 entry 없음) · (3) `tenants` row 1 유지 (`DROP`/`TRUNCATE` 금지).

**회귀 방지** — 2 회 실행으로 잡는 버그: `CREATE TABLE`/`INDEX` 에서 `IF NOT EXISTS` 누락 / `ALTER TABLE ADD CONSTRAINT` 중복 / `_sqlx_migrations` 중복 entry / 실수 `DROP`·`TRUNCATE` 로 데이터 손실. **Cross-ref**: §2.1.4 migration 정책 · `README.md` (A6) · `docs/design/testing/harness.md` · D-4 · D-9.

### 4.2 테스트 하네스

- **Mock 전략**:
  - `gadgetron-testing::mocks::xaas::MockKeyValidator` — `validate()` 가 미리 설정된 map lookup
  - `gadgetron-testing::mocks::xaas::MockTenantRegistry` — in-memory HashMap
  - `gadgetron-testing::mocks::xaas::MockAuditWriter` — `Arc<Mutex<Vec<AuditEntry>>>` 로 collected entries 검사
- **Stub**: `FakeQuotaEnforcer` 로 항상 Allow 반환 (다른 테스트용)
- **Fixture**:
  - `gadgetron-testing::fixtures::tenant::acme_tenant() -> Tenant`
  - `gadgetron-testing::fixtures::api_key::live_key_for(tenant_id) -> (ApiKey, KeyRow)`
- **Property-based**: `proptest` (`crate::auth::key` 의 base62 파싱 invariant)
- **Time control**: `tokio::time::pause` + `advance` (TokenBucket refill)
- **PostgreSQL**: `gadgetron-testing::harness::pg::PgTestContext::new().await` → fresh migrated DB per test

### 4.3 커버리지 목표

- **Line coverage**: 85% (unit)
- **Branch coverage**: 80% (특히 error path)
- **Critical paths** (100% coverage required):
  - `ApiKey::parse`
  - `TokenBucket::try_consume` + `adjust`
  - `PgKeyValidator::validate`
  - `AuditWriter::write` + batcher flush
- 측정: `cargo tarpaulin --packages gadgetron-xaas --out Html` (Phase 1 CI)

### 4.4 Performance Benchmark

`benches/auth_middleware.rs` (criterion 0.5):

```rust
fn bench_key_validation_cache_hit(c: &mut Criterion) {
    // Pre-warm moka cache with seeded ValidatedKey
    // Measure: PgKeyValidator::validate(&cache_hit_prefix) → Arc<ValidatedKey>
    // Target: p99 < 50µs (moka in-process lookup)
    c.bench_function("key_validation_cache_hit", |b| {
        b.iter(|| validator.validate(black_box("gad_live_test123")).await)
    });
}

fn bench_key_validation_cache_miss(c: &mut Criterion) {
    // PgHarness required — measures full DB round-trip
    // Target: p99 < 5ms
}
```

Cross-reference: `gadgetron-testing::harness::pg::PgHarness` for cold-start variant.

### 4.5 Error Shape Snapshots

`gadgetron-xaas/tests/snapshots/` — protected by `insta 1`:

| Snapshot file | Variant | HTTP | Body shape |
|---|---|---|---|
| `error__scope_forbidden.snap` | Forbidden | 403 | `{"error":{"message":"...","type":"permission_error","code":"forbidden"}}` |
| `error__rate_limit_exceeded.snap` | QuotaExceeded | 429 | `{"error":{"message":"...","type":"quota_error","code":"quota_exceeded"},"retry_after":N}` |
| `error__invalid_api_key.snap` | TenantNotFound | 401 | `{"error":{"message":"...","type":"authentication_error","code":"tenant_not_found"}}` |

Update policy: `INSTA_UPDATE=unseen cargo test` → `cargo insta review` → commit. CI: `INSTA_UPDATE=no` (fails on unreviewed).
Cross-reference: `docs/design/testing/harness.md` Appendix B.

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

**함께 테스트할 크레이트**:
- `gadgetron-gateway` (실제 `AuthLayer`/`QuotaLayer`/`AuditLayer` 마운트)
- `gadgetron-router` (기본 라우팅 pass-through)
- `gadgetron-provider` (MockLlmProvider 로 대체)
- `gadgetron-testing` (harness / mocks / fixtures)
- `gadgetron-xaas` (본체)

**위치**: `crates/gadgetron-testing/tests/xaas_e2e.rs`

### 5.2 e2e 시나리오

| ID | Given | When | Then |
|:---|---|---|---|
| 5.2.1 Happy | seeded tenant + live_key | `POST /v1/chat/completions` Bearer live | 200 · audit row · `rpm_remaining == default-1` |
| 5.2.2 Missing auth | — | Authorization 헤더 제거 | 401 `invalid_api_key` · audit row 없음 |
| 5.2.3 Invalid key | — | Bearer `gad_live_notarealkey...` | 401 `invalid_api_key` |
| 5.2.3b Scope denied | live + `[OpenAiCompat]` only | `GET /api/v1/xaas/tenants` | 403 `scope_forbidden` (D-10) |
| 5.2.4 RPM 초과 | rpm=5 | 6번째 요청 | 429 `rate_limit_exceeded` · `Retry-After: 12` |
| 5.2.5 TPM 초과 | tpm=100 | `max_tokens=500` | 429 `token_limit_exceeded` |
| 5.2.6 Virtual key | T1 + `gad_vk_<T1_short>_<32>` | 요청 | 200 · `audit.tenant_id == T1.id` · kind=Virtual |
| 5.2.6b VK mismatch | T1 + `gad_vk_<T2_short>_<...>` | 요청 | 401 `invalid_api_key` (A18) |
| 5.2.7 Audit batch flush | `batch_size=10` | 10 요청 | 200 · 1초 내 10 row |
| 5.2.8 Audit backpressure | `channel_capacity=4` | 100 요청 burst | 200 · `audit_dropped_total > 0` · row < 100 |
| 5.2.9 Graceful shutdown (A5) | `N < 100` pending | SIGTERM | 5s 내 flush · drop 없음 |
| 5.2.10 StreamInterrupted (D-08) | SSE client disconnect | — | `StreamInterrupted { reason }` → 499 + metric |

#### 5.2.3c Scope 가드 경로별 매트릭스 (Round 2 A1, 신규)

D-20260411-10 의 3 개 `Scope` (`OpenAiCompat`, `Management`, `XaasAdmin`) 가 **모든 endpoint × scope 조합**에서 정확히 게이팅됨을 증명하는 **6 개 integration test case**. `GatewayHarness::spawn` 으로 실제 미들웨어 체인을 띄우고 `TenantContext.scopes` 조합을 변조하여 `require_scope` 가 403/200 을 경로별로 구분함을 검증.

| # | Endpoint | Required Scope | Key Scopes | Expected |
|:-:|---|:---:|:---:|:---:|
| 1 | `POST /v1/chat/completions` | `OpenAiCompat` | `[OpenAiCompat]` | **200** |
| 2 | `POST /v1/chat/completions` | `OpenAiCompat` | `[Management]` | **403** `scope_forbidden` |
| 3 | `GET /api/v1/nodes` | `Management` | `[OpenAiCompat]` | **403** |
| 4 | `GET /api/v1/nodes` | `Management` | `[Management]` | **200** |
| 5 | `POST /api/v1/xaas/tenants` | `XaasAdmin` | `[OpenAiCompat, Management]` | **403** |
| 6 | `POST /api/v1/xaas/tenants` | `XaasAdmin` | `[XaasAdmin]` | **200** |

**구현 위치**: `crates/gadgetron-xaas/tests/integration/scope_guard.rs`. **Fixture 패턴** (각 케이스 공통):

```rust
#[tokio::test]
async fn scope_matrix_case_2_chat_requires_openai_compat() -> Result<()> {
    let harness = GatewayHarness::spawn().await?;
    let key = harness.issue_key_with_scopes(KeyKind::Live, vec![Scope::Management]).await?;
    let resp = harness.client()
        .post("/v1/chat/completions").bearer_auth(&key)
        .json(&minimal_chat_request()).send().await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = resp.json().await?;
    assert_eq!(body["error"]["type"], "scope_forbidden");
    Ok(())
}
```

**6 개 case 공유 invariant**: (1) `require_scope` 실패 시 반드시 **403** (401 금지 — key valid, 권한만 부족) · (2) body `error.type == "scope_forbidden"` (§2.4.2) · (3) auth 통과 시 audit row 생성 (compliance) · (4) 필요한 scope 포함 시 handler 통과 (200).

**회귀 방지** — 이 매트릭스가 잡는 구현 버그:
- 키 발급 로직이 `scopes` 를 비워서 저장 (case 2/3/5 가 200 으로 fail).
- `/v1/*` 라우터에서 `require_scope(&ctx, Scope::OpenAiCompat)` 호출 누락 (case 2 fail).
- **가장 위험**: `/api/v1/xaas/*` 경로가 `require_scope` 없이 pass-through (case 5 fail — admin endpoint 가 일반 키로 열림).
- `require_scope` 역방향 매칭 / `any` 누락 으로 복수 scope 키 정당 통과 실패 (case 4/6 확장 시 드러남).
- `#[non_exhaustive]` Phase 2 variant 추가 시 `_` arm 금지 — 매트릭스가 컴파일러 exhaustiveness 로 보호됨.

**Cross-reference**: §2.1.2 `require_scope` · §2.4.2 403 `scope_forbidden` 매핑 · 5.2.3b (smoke) vs 5.2.3c (full matrix).

### 5.3 테스트 환경

- **testcontainers 0.23** (Track 2 D-20260411-05 확정)
- **Postgres**: `testcontainers::modules::postgres::Postgres::default()` — 테스트당 fresh
- **MockLlmProvider**: `gadgetron-testing::mocks::provider::MockLlmProvider::new()` 로 OpenAI 스키마 fake
- **sqlx migrate**: `sqlx::migrate!("./crates/gadgetron-xaas/src/db/migrations").run(&pool).await`
- **axum test server**: `gadgetron-testing::harness::gateway::TestGateway::new(deps).await`
- **CI**: GitHub Actions runner, `docker` 서비스, `cargo nextest run -p gadgetron-testing --test xaas_e2e`

### 5.4 회귀 방지

어떤 변경이 이 테스트를 실패시켜야 하는가:

| 회귀 | 증상 |
|------|------|
| SQLite 재도입 | migration 파일 syntax error, `sqlx migrate run` 실패 |
| `f64 cost` 재도입 | 타입 컴파일 에러 또는 `cost_cents::BIGINT` 캐스팅 실패 |
| `gdt_` prefix 재도입 | `ApiKey::parse` 테스트 실패 (정확히 `gad_` prefix 만 허용) |
| `GadgetronError` variant 삭제 | 컴파일 실패 (특히 `QuotaExceeded { tenant_id }`, `TenantNotFound`, `Forbidden`) |
| `AuthLayer` 가 extensions 에 TenantContext 주입 안 함 | handler integration test 에서 `extensions().get::<TenantContext>()` == None |
| `AuditWriter` 가 hot path block | audit 테스트에서 latency > 10ms (p99) |
| RPM bucket refill 버그 | `refill_correctness` 단위 테스트 실패 |
| Test 키 disable 시 test 키로 인증 성공 | auth 테스트 실패 |

---

## 6. Phase 구분

| 섹션 / 필드 | Phase |
|-------------|-------|
| 크레이트 `gadgetron-xaas` 생성 | `[P1]` |
| `auth/` 모듈 (key, validator, middleware) | `[P1]` |
| `tenant/` 모듈 (model, registry) | `[P1]` |
| `quota/` 모듈 (config, enforcer, bucket) | `[P1]` |
| `audit/` 모듈 (entry, writer) | `[P1]` |
| `db/migrations/` 4개 테이블 (`tenants`, `api_keys`, `quotas`, `audit_log`) | `[P1]` |
| Bearer 미들웨어 `AuthLayer`, `QuotaLayer`, `AuditLayer` | `[P1]` |
| `TenantContext` request extension 주입 | `[P1]` |
| `gad_live_*`, `gad_test_*`, `gad_vk_*` 키 형식 | `[P1]` |
| SHA-256 hex 해시 저장 | `[P1]` |
| Token bucket RPM+TPM 1분 윈도우 | `[P1]` |
| mpsc 기반 audit batcher (100 / 1s) | `[P1]` |
| Prometheus 메트릭 (auth/quota/audit subset) | `[P1]` |
| Billing engine 계산 (i64 cents) | `[P2]` |
| `billing/` 모듈 (`ledger`, `calculator`, `invoice`) | `[P2]` |
| `agent/` 모듈 (lifecycle, memory, tools) | `[P2]` |
| `catalog/` 모듈 (HuggingFace, DownloadManager) | `[P2]` |
| `gpuaas/` 모듈 (allocation, MIG, reservation) | `[P2]` |
| GDPR 마스킹 (30/90일) | `[P2]` |
| Concurrent max 실제 enforcement (Redis) | `[P2]` |
| OAuth2/OIDC | `[P2]` |
| gRPC endpoints | `[P2]` (D-5) |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID  | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| Q-1 | Token bucket 분산 (multi-node) | A. Phase 1 은 single-node (in-memory), B. Phase 1 부터 Redis | A (Phase 1 최소화) | 🟢 내부 결정 |
| Q-2 | TPM estimate 없이 오는 요청 (`max_tokens=None`) | A. 무제한 consume, B. 기본 2048 fallback, C. reject | B (보수적, 429 대신 200 유지) | 🟡 PM 확인 |
| Q-3 | Virtual key 만료 정책 (Phase 1) | A. 없음, B. 고정 TTL, C. tenant 파생 | A (Phase 2 로 연기) | 🟢 내부 결정 |
| Q-4 | Audit drop 시 client 통지 여부 | A. 조용히 drop, B. `X-Audit-Dropped` 헤더, C. 500 실패 | A (drop 은 운영 이슈, client 비가시) | 🟢 내부 결정 |
| Q-5 | Migration run 위치 (CI vs runtime) | A. `migrate_on_start=true`, B. CI 단계에서 먼저, C. 하이브리드 | C (dev=A, prod=B) | 🟡 devops-sre-lead 협의 |
| Q-6 | Tenant cache TTL (`tenant_cache_ttl_ms`) | A. 10s, B. 30s, C. 60s | B. 30s (관리 API 와 균형) | 🟢 내부 결정 |
| Q-7 | `record_post` 실패 시 재시도 | A. 없음, B. best-effort retry 1회, C. reconciler task | A (Phase 1 최소화, Phase 2 C 로 승격) | 🟢 내부 결정 |
| Q-8 | RPM 1 refill 정밀도 (f64 vs integer nanos) | A. f64, B. i64 nanos | A (충분히 정밀, 과도한 optimization 회피) | 🟢 내부 결정 |
| Q-9 | CI signal handling — GitHub Actions / docker stop 이 axum graceful shutdown 을 트리거하는지 | A. devops Week 2 협의, B. wrapper, C. skip | A (§4.1.5b virtual time 이 unit 담보, runner 동작은 devops 스펙) | 🟡 devops-sre-lead Week 2 협의 예정 (Round 2 A4 non-blocking) |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-11 — @gateway-router-lead + @devops-sre-lead
**결론**: ⚠️ Conditional Pass
**체크리스트 (§1)**: 8개 항목 Pass, 운영 준비성에서 15개 선결 조건.
**핵심 발견**: (1) `Scope` enum 미정 → route guard 불가 (D-10 해결) · (2) `record_post()` 미들웨어 위치 불명확 → lifetime 오류 위험 · (3) 감사 drop compliance 위험 (D-09 완화).
**Action Items 원본**: A1~A6 blocking, A7~A21 non-blocking (상세: `docs/reviews/round1-week1-results.md §Track 3`). 조건: 6 blocking resolve 후 Round 1 retry.

### Round 1 Retry — 2026-04-11 — @xaas-platform-lead
**결론**: ✅ All blocking action items resolved

**Resolutions (blocking)**:
- **A1** ✅ `Scope` enum 정의 (D-20260411-10): `#[non_exhaustive] enum Scope { OpenAiCompat, Management, XaasAdmin }` + `ValidatedKey.scopes` + `api_keys.scopes TEXT[]` + 키별 기본 scope 매트릭스 + `require_scope` helper (§2.1.2).
- **A2** ✅ `QuotaEnforcer::record_post` tower 위치 (§2.4.3): 체인 순서도 + `QuotaToken` lifetime + `map_result` + `tokio::spawn` fire-and-forget.
- **A3** ✅ HTTP Status Mapping 테이블 (§2.4.2): 9개 variant + D-08 `StreamInterrupted` 499 + OpenAI body schema + `IntoResponse` sketch.
- **A4** ✅ 감사 drop D-20260411-09 반영 (§2.2.4): 버퍼 4096 + `*_dropped_total` metric + tenant/request tracing warn + Phase 2 WAL/S3 fallback + 배포 전 compliance review 게이트.
- **A5** ✅ Graceful shutdown drain audit (§2.2.4): `flush()` async + `with_graceful_shutdown` 5 초 timeout + progress log.
- **A6** ✅ Migration idempotency (§2.1.4): 4개 파일 `IF NOT EXISTS` + forward-only + `README.md` 스펙.

**Non-blocking 반영**: A7 D-06 인용(§2.4.5) · A8 캐싱 주석(§2.2.2) · A9 Sqlx→Config(§2.2.2) · A10 CI DATABASE_URL(§2.1.4) · A11 `validate_connection`(§2.2.2) · A14 `validate_xaas`(§2.3) · A15 `RUST_LOG` 기본(§2.4.4) · A17 `tenant_cache` 30s TTL(§2.2.2) · A18 virtual key mismatch(§2.2.2) · A20 `PgKeyValidator` rustdoc(§2.2.2).

**Cross-track**: Track 1 A4 (`StreamInterrupted`) 머지 후 HTTP 499 최종 검증 · gateway-router-lead Week 3 문서 머지 후 `QuotaRecordPostLayer` 타입 align.

**다음 라운드**: Round 2 (qa-test-architect) 진입 가능.

### Round 2 — 2026-04-12 — @qa-test-architect
**결론**: ⚠️ Conditional Pass — 3 blocking + 1 non-blocking
**체크리스트 (§2)**: 6/8 Pass, 2 Partial (회귀 커버리지 gap + schema isolation).
**핵심**: TokenBucket/PgKeyValidator/AuditWriter 철저. 3 gaps 는 "구현 중 놓치기 쉬운 경계 케이스" — 지금 문서에 명시하는 비용 < 나중 hotfix 비용.
**Action Items**: A1 Scope 매트릭스 6 case / A2 migration 2 회 실행 / A3 graceful shutdown timeout virtual time / A4 (non-blocking) CI signal handling Week 2 devops 협의.

### Round 2 Retry — 2026-04-12 — @xaas-platform-lead
**결론**: ✅ 3 blocking action items resolved, A4 non-blocking noted

**Resolutions**:
- **A1** ✅ §5.2.3c Scope 가드 매트릭스 — 6 개 integration test case (endpoint × scope), `GatewayHarness::spawn` + `TenantContext.scopes` 변조, 403 `scope_forbidden` invariant, `/api/v1/xaas/*` 방치 시나리오 covered, `#[non_exhaustive]` exhaustiveness 보호.
- **A2** ✅ §4.1.7 Migration Idempotency — `PgHarness::fresh()` 2 회 실행 + `capture_schema_snapshot` + `_sqlx_migrations` MAX(version) 일치 + 중간 `tenants` row 생존 검증. `IF NOT EXISTS` 누락 / 중복 constraint / 중복 index / 중복 sqlx entry 모두 catch.
- **A3** ✅ §4.1.5b Graceful Shutdown Timeout — `#[tokio::test(start_paused = true)]` virtual time, `SlowMockPool` (10s sleep), `advance(6s)`, `*_shutdown_timeout_total` metric + `pending_count() > 0` partial flush assertion, D-20260411-09 drop 정책 cross-ref.
- **A4** ✅ Q-9 추가 — CI signal handling (SIGTERM propagation) Week 2 devops-sre-lead 협의 예정. §4.1.5b 가 unit 레벨 담보.

**Cross-track**: Track 1 A4 (`StreamInterrupted`) 머지 후 HTTP 499 최종 검증 (기존 유지) · gateway-router-lead Week 3 문서 머지 후 `QuotaRecordPostLayer` align (기존 유지) · `PgHarness::fresh()` rename (현 `PgTestContext::new()`) → qa-test-architect 에 non-blocking 코멘트.

**다음 라운드**: Round 3 (chief-architect) 진행 가능.

### Round 3 — TBD — @chief-architect
**결론**: (대기)
(`03-review-rubric.md §3` 기준)

### 최종 승인 — TBD — PM
