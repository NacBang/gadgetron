# Gadgetron XaaS 플랫폼 설계 문서

> **버전**: 2.0.0-draft
> **작성일**: 2026-04-11
> **최종 업데이트**: 2026-04-11
> **상태**: Draft (Round 2 작성중)
> **담당**: @xaas-platform-lead
> **관련 크레이트**: `gadgetron-xaas`, `gadgetron-core`, `gadgetron-gateway`
> **재작성**: 2026-04-11 per D-20260411-01/03, D-4, D-8, D-11

---

## 변경 이력 (2026-04-11 재작성)

이 문서는 Round 2 Platform Review (`docs/reviews/round2-platform-review.md §6.5`)에서 지적된 다음 **BLOCKER** 를 해결하기 위해 전면 재작성되었습니다.

| 위반 | 이전 | 변경 | 결정 |
|------|------|------|------|
| DB 엔진 | SQLite (`AUTOINCREMENT`, `datetime('now')`) | PostgreSQL (`BIGSERIAL`, `NOW()`) | **D-4** |
| 과금 타입 | `REAL` / `f64` USD | `BIGINT` / `i64` cents | **D-8** |
| API 키 prefix | `gdt_*`, `gdt_vk_*` | `gad_*`, `gad_vk_*` | **D-11** |
| ORM | 스키마만 (구현 없음) | `sqlx 0.8` + `sqlx-cli` 마이그레이션 | **D-9** |
| 프로토콜 | gRPC + REST 듀얼 | Phase 1 = REST 전용, Phase 2 = gRPC 추가 | **D-5** |
| Phase 1 범위 | 단일 문서에 모든 기능 | `[P1]` / `[P2]` 태그로 분리 | **D-20260411-01** (옵션 B) |
| 크레이트 | 설계만 존재 | `gadgetron-xaas` 단일 크레이트 신설 | **D-20260411-03** |
| 에러 전파 | ad-hoc String | `GadgetronError::{Billing, TenantNotFound, QuotaExceeded, DownloadFailed, HotSwapFailed}` | **D-13** |

Phase 1 상세 구현 문서: [`docs/design/xaas/phase1.md`](../design/xaas/phase1.md) — 이 문서는 Phase 2 포함 전체 개념을 서술하고, Phase 1 실제 구현 설계는 별도 문서에서 관리합니다.

---

## 목차

1. [개요](#1-개요)
2. [Phase 분리 및 크레이트 구조](#2-phase-분리-및-크레이트-구조)
3. [인증: API 키 계층](#3-인증-api-키-계층) `[P1]`
4. [테넌트 관리](#4-테넌트-관리) `[P1]`
5. [쿼터 적용 (경량)](#5-쿼터-적용-경량) `[P1]`
6. [감사 로깅](#6-감사-로깅) `[P1]`
7. [게이트웨이 통합 (미들웨어 체인)](#7-게이트웨이-통합-미들웨어-체인) `[P1]`
8. [과금 엔진](#8-과금-엔진) `[P2]`
9. [HuggingFace 카탈로그](#9-huggingface-카탈로그) `[P2]`
10. [AgentaaS](#10-agentaas) `[P2]`
11. [GPUaaS (MIG, 타임슬라이싱, 예약)](#11-gpuaas-mig-타임슬라이싱-예약) `[P2]`
12. [부록: 설정 스키마](#12-부록-설정-스키마)

---

## 1. 개요

Gadgetron XaaS 플랫폼은 AI 오케스트레이션을 위한 **3 계층 서비스 추상화**를 제공합니다. 본 문서는 Round 1 (`docs/reviews/round1-pm-review.md`)과 Round 2 (`docs/reviews/round2-platform-review.md`) 리뷰 결과 + 확정 결정 D-4/D-8/D-11/D-13/D-20260411-01/D-20260411-03 을 반영한 재작성본입니다.

### 1.1 3 계층 개념 (보존)

```
+----------------------------------------------------+
|                    AgentaaS [P2]                   |
|  lifecycle · template · multi-agent orchestration  |
+----------------------------------------------------+
|                    ModelaaS [P2]                   |
|  HF catalog · deploy · A/B · profile · rollback    |
+----------------------------------------------------+
|                    GPUaaS [P2]                     |
|  allocation · MIG · time-slicing · quota · reserve |
+----------------------------------------------------+
|   gadgetron-xaas Phase 1 기반 (auth/tenant/quota)  |  [P1]
|   -- 이 위에 Phase 2 세 계층이 올라간다            |
+----------------------------------------------------+
```

**핵심 설계 원칙**:
- 상위 계층은 하위 계층에만 의존 (단방향 의존성)
- Phase 1: **경량 XaaS** — auth + tenant + quota + audit 만 (D-20260411-01 B)
- Phase 2: **full XaaS** — billing engine, agent orchestration, HF catalog, GPUaaS
- 모든 통화 계산: `i64` cents (D-8)
- PostgreSQL only (SQLite 금지, D-4)
- gRPC는 Phase 2 (D-5)
- 모든 api key prefix: `gad_*` (D-11)

### 1.2 설계 목표

1. **Gateway 미들웨어 체인과 투명하게 연결**: `Auth → Quota → Audit` hook 을 `gadgetron-gateway` 가 호출
2. **Phase 2 확장에 crate 재배치 없음**: `billing/`, `agent/`, `catalog/`, `gpuaas/` 모듈을 `gadgetron-xaas` 내부에 추가
3. **Cross-crate 의존성 최소화**: `gadgetron-xaas -> gadgetron-core` 만 (router/provider 역참조 없음)
4. **Tenant isolation-first**: 모든 쓰기 쿼리는 `tenant_id` WHERE 절 의무
5. **Audit async-first**: 요청 hot path 에서 동기 DB write 금지

---

## 2. Phase 분리 및 크레이트 구조

### 2.1 D-20260411-03 확정 구조

```
crates/gadgetron-xaas/
├── Cargo.toml
└── src/
    ├── lib.rs                       # 공개 re-export
    ├── auth/                        # [P1]
    │   ├── key.rs                   # ApiKey 파싱 (gad_*, gad_vk_*)
    │   ├── validator.rs             # KeyValidator trait + PgKeyValidator
    │   └── middleware.rs            # axum Layer (Bearer 추출 -> TenantContext 주입)
    ├── tenant/                      # [P1]
    │   ├── model.rs                 # Tenant, TenantContext (request-scoped)
    │   └── registry.rs              # TenantRegistry trait + PgTenantRegistry
    ├── quota/                       # [P1]
    │   ├── config.rs                # QuotaConfig (RPM/TPM/concurrent_max)
    │   ├── enforcer.rs              # QuotaEnforcer (pre-check/post-record)
    │   └── bucket.rs                # TokenBucket (RPM + TPM refill)
    ├── audit/                       # [P1]
    │   ├── entry.rs                 # AuditEntry struct
    │   └── writer.rs                # AuditWriter (mpsc channel -> batch insert)
    ├── db/
    │   └── migrations/              # sqlx PostgreSQL migrations
    │       ├── 20260411000001_tenants.sql
    │       ├── 20260411000002_api_keys.sql
    │       ├── 20260411000003_quotas.sql
    │       └── 20260411000004_audit_log.sql
    │
    ├── billing/                     # [P2] — 추가 예정 모듈
    │   ├── ledger.rs                # LedgerEntry (i64 cents)
    │   ├── calculator.rs            # CostCalculator (tokens/gpu/vram)
    │   └── invoice.rs               # Invoice aggregation
    ├── agent/                       # [P2]
    │   ├── lifecycle.rs             # CREATED -> CONFIGURED -> RUNNING -> PAUSED -> DESTROYED
    │   ├── memory.rs                # short-term (PG) + long-term (pgvector/Qdrant)
    │   └── tools.rs                 # tool-call bridge
    ├── catalog/                     # [P2]
    │   ├── hf_client.rs             # HuggingFace Hub API
    │   └── download.rs              # DownloadManager (uses DownloadState from core)
    └── gpuaas/                      # [P2]
        ├── allocation.rs            # GpuAllocation (leases)
        ├── mig.rs                   # MIG profile selection (uses MigManager from node)
        └── reservation.rs           # priority queue + iCalendar RRULE
```

**중요**: Phase 2 확장 시 **크레이트 경계 변경 없음**. `billing/` · `agent/` · `catalog/` · `gpuaas/` 디렉토리를 `gadgetron-xaas/src/` 에 **추가**만 하면 됩니다. D-12 크레이트 경계표에 `gadgetron-xaas` 한 행만 추가되었습니다 (D-20260411-03).

### 2.2 의존 방향 (D-12 준수)

```
gadgetron-cli
    v
gadgetron-gateway  -> gadgetron-xaas -> gadgetron-core
    v                     |                   ^
gadgetron-router      (PG via sqlx)           |
    v                                          |
gadgetron-provider --------------------------> +
```

- `gadgetron-xaas` 는 오직 `gadgetron-core` (타입/에러) 에만 의존
- `gadgetron-gateway` 가 `gadgetron-xaas` 의 `AuthLayer`/`QuotaLayer`/`AuditLayer` 를 마운트
- `gadgetron-router`, `gadgetron-provider` 는 `gadgetron-xaas` 를 참조하지 않음 (순환 방지)
- `TenantContext` 는 `axum::Request::extensions_mut()` 으로 핸들러까지 전달

### 2.3 Phase 1 vs Phase 2 스코프 요약

| 영역 | Phase 1 (D-20260411-01 B) | Phase 2 |
|------|--------------------------|---------|
| **Auth** | `gad_live_*`, `gad_test_*`, `gad_vk_*` 키 검증, SHA-256 해시, Bearer 미들웨어 | API 키 scope fine-grained (per-endpoint), OAuth2/OIDC |
| **Tenant** | PG 기반 CRUD, `TenantContext` 전파 | 조직(Org) 계층, 사용자(User) + 역할(Role) |
| **Quota** | Token bucket (RPM + TPM), pre-check + post-record, 429 응답 | Concurrent max 스냅샷, burst credit, monthly soft limit |
| **Audit** | `request_id`, tenant/model/tokens/latency/status/cost_cents, async batch | GDPR 마스킹, 30/90 day anonymization, S3 export |
| **Billing** | cost_cents 기록만 (계산 없음) | Token rate + GPU seconds + VRAM hour + QoS multiplier 계산, Ledger, Invoice |
| **Agent** | 없음 | Lifecycle FSM + memory (PG + pgvector) + tool-call bridge |
| **Catalog** | 없음 | HuggingFace integration + DownloadManager + 호환성 검사 |
| **GPUaaS** | 없음 | MIG, time-slicing, reservation queue |

---

## 3. 인증: API 키 계층 `[P1]`

### 3.1 키 형식 (D-11)

세 종류의 API 키를 구분:

| 종류 | 형식 | 목적 |
|------|------|------|
| **Live** | `gad_live_<32char_base62>` | 운영 환경 마스터 키 (테넌트 관리자가 발급) |
| **Test** | `gad_test_<32char_base62>` | 테스트 환경 키 (실제 과금 제외) |
| **Virtual** | `gad_vk_<tenant_id_12_base62>_<32char_base62>` | 테넌트 하위 가상 키 (scope 제한) |

**금지**: `gdt_*` 접두사 (Round 1 O-9 지적, D-11 로 금지).

### 3.2 ApiKey 타입 (`auth/key.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub raw: String,             // "gad_live_..." 원본 (로그 시 마스킹)
    pub prefix: String,          // "gad_live" or "gad_test" or "gad_vk_<short>"
    pub kind: KeyKind,
    pub suffix: String,          // base62 부분
    pub tenant_short: Option<String>,  // virtual key 인 경우 tenant_id short form
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Live,
    Test,
    Virtual,
}

impl ApiKey {
    pub fn parse(raw: &str) -> Result<Self, GadgetronError> {
        // "gad_live_XXX" | "gad_test_XXX" | "gad_vk_<12char>_<32char>"
        // base62 문자 집합 검증, 길이 검증
        // 실패 시 GadgetronError::TenantNotFound("invalid api key format")
    }

    pub fn hash_hex(&self) -> String {
        // SHA-256(self.raw) -> lowercase hex string
    }
}
```

### 3.3 키 저장소: SHA-256 hex, plaintext 금지

- DB 에 `key_hash CHAR(64)` (SHA-256 hex) 만 저장
- 원본 raw 는 **발급 시 1회만** 반환, 이후 재조회 불가
- `prefix VARCHAR(32)` 는 표시용 (`gad_live_ab12…`) 으로 별도 저장하여 UI 리스트 표시

### 3.4 API 키 PostgreSQL 스키마

```sql
-- 20260411000002_api_keys.sql
-- A6: Forward-only + idempotent. 같은 파일 두 번 실행해도 스키마 손상 없음.
CREATE TABLE IF NOT EXISTS api_keys (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    prefix       VARCHAR(32) NOT NULL,            -- display prefix (gad_live_ab12)
    key_hash     CHAR(64) NOT NULL,               -- SHA-256 hex (D-11)
    kind         VARCHAR(16) NOT NULL,            -- 'live' | 'test' | 'virtual'
    scopes       TEXT[] NOT NULL DEFAULT ARRAY['openai_compat']::TEXT[],   -- D-20260411-10
    name         VARCHAR(255),                    -- human-readable label
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ,                     -- soft delete
    UNIQUE(prefix, key_hash)
);

CREATE INDEX IF NOT EXISTS api_keys_tenant_idx
    ON api_keys(tenant_id) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS api_keys_hash_idx
    ON api_keys(key_hash) WHERE revoked_at IS NULL;

-- D-20260411-10: 최소 1개 scope 강제
ALTER TABLE api_keys
    ADD CONSTRAINT api_keys_scopes_check CHECK (array_length(scopes, 1) >= 1);
```

**D-20260411-10 기본 scope 매트릭스**:

| 키 타입 | 기본 `scopes` | Management 부여 | XaasAdmin 부여 |
|---------|-------------|:---:|:---:|
| `gad_live_*` | `[OpenAiCompat]` | 명시 grant | 명시 grant |
| `gad_test_*` | `[OpenAiCompat]` | 명시 grant | 명시 grant |
| `gad_vk_*`   | `[OpenAiCompat]` | ❌ 금지 | ❌ 금지 |

### 3.5 KeyValidator trait

```rust
#[async_trait]
pub trait KeyValidator: Send + Sync {
    async fn validate(&self, raw_key: &str) -> Result<ValidatedKey>;
}

pub struct ValidatedKey {
    pub key_id:    Uuid,
    pub tenant_id: Uuid,
    pub kind:      KeyKind,
    pub scopes:    Vec<Scope>,    // [P1] 1+ scope (D-20260411-10)
}

/// D-20260411-10: Phase 1 scope 정책 — OpenAiCompat / Management / XaasAdmin.
/// `#[non_exhaustive]` 로 Phase 2 에 fine-grained scope (ChatRead/ChatWrite/...)
/// 를 breaking change 없이 추가.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    OpenAiCompat,   // /v1/* — chat/completions, models, embeddings
    Management,     // /api/v1/{nodes, models, usage, costs}
    XaasAdmin,      // /api/v1/xaas/* — tenant/quota/agent admin
}

pub struct PgKeyValidator { pool: PgPool /* + tenant_cache */ }
```

상세 구현은 `docs/design/xaas/phase1.md §2.1.2 / §2.2.2` 참조.

---

## 4. 테넌트 관리 `[P1]`

### 4.1 Tenant 모델

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub status: TenantStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TenantStatus {
    Active,
    Suspended,       // quota 초과, billing 지연 등
    Deleted,         // soft delete
}
```

### 4.2 TenantContext (request-scoped)

```rust
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: Uuid,
    pub key_id: Uuid,
    pub key_kind: KeyKind,
    pub scopes: Vec<Scope>,
    pub request_id: Uuid,       // per-request, audit 연계
}
```

- `AuthLayer` 가 Bearer 토큰 검증 후 `TenantContext` 를 `Request::extensions_mut()` 에 삽입
- 하류 핸들러는 `req.extensions().get::<TenantContext>()` 로 접근
- 한번도 덮어쓰지 않음 (immutable after auth)

### 4.3 PostgreSQL 스키마

```sql
-- 20260411000001_tenants.sql
CREATE TABLE tenants (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         VARCHAR(255) NOT NULL,
    status       VARCHAR(16) NOT NULL DEFAULT 'active',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX tenants_status_idx ON tenants(status);
```

### 4.4 TenantRegistry trait

```rust
#[async_trait]
pub trait TenantRegistry: Send + Sync {
    async fn get(&self, tenant_id: Uuid) -> Result<Tenant>;
    async fn create(&self, spec: TenantSpec) -> Result<Tenant>;
    async fn update_quota(&self, tenant_id: Uuid, quota: QuotaConfig) -> Result<()>;
}
```

존재하지 않는 tenant_id 조회 시 `GadgetronError::TenantNotFound(String)` (D-13).

---

## 5. 쿼터 적용 (경량) `[P1]`

### 5.1 QuotaConfig

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct QuotaConfig {
    pub rpm: u32,              // Requests Per Minute
    pub tpm: u64,              // Tokens Per Minute
    pub concurrent_max: u32,    // 동시 in-flight 요청 수 (Phase 2 snapshot)
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self { rpm: 60, tpm: 1_000_000, concurrent_max: 10 }
    }
}
```

### 5.2 Token Bucket 알고리즘 (Round 1 보존)

**per-tenant** 2개의 토큰 버킷 유지 (RPM + TPM, 1분 윈도우):

```
   ┌─────────────────────────────────────────┐
   │  RPM bucket (60 tokens, refill 1/sec)   │
   │  TPM bucket (1M tokens, refill 16666/s) │
   └──────────┬──────────────────────────────┘
              │ pre_check(req) -> QuotaToken
              ▼
   +----------+---+
   | request exec |
   +----------+---+
              │ record_post(token, actual_usage)
              ▼
      adjust TPM bucket (actual - estimated)
```

### 5.3 QuotaEnforcer (pre/post hook)

```rust
#[async_trait]
pub trait QuotaEnforcer: Send + Sync {
    async fn check_pre(
        &self,
        ctx: &TenantContext,
        req: &ChatRequest,
    ) -> Result<QuotaToken>;

    async fn record_post(
        &self,
        token: QuotaToken,
        usage: &Usage,
    ) -> Result<()>;
}

pub struct QuotaToken {
    pub tenant_id: Uuid,
    pub request_id: Uuid,
    pub estimated_tokens: u32,
    pub issued_at: Instant,
}
```

**Lifecycle**:
1. `check_pre`: RPM bucket 에서 1 consume, TPM bucket 에서 `max_tokens` estimate consume → `QuotaToken` 발급. 실패 시 `GadgetronError::QuotaExceeded("rpm exceeded")`.
2. 요청 실행 (router/provider)
3. `record_post`: 실제 `usage.total_tokens` 와 estimate 의 차이만큼 TPM bucket 에 환불 또는 추가 consume. 실패 시 로그 경고만 (요청 실패 유발 안 함).

### 5.4 PostgreSQL 스키마

```sql
-- 20260411000003_quotas.sql
CREATE TABLE quotas (
    tenant_id       UUID PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    rpm             INT NOT NULL DEFAULT 60,
    tpm             BIGINT NOT NULL DEFAULT 1000000,
    concurrent_max  INT NOT NULL DEFAULT 10,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### 5.5 Phase 2 확장

- `concurrent_max` 실제 체크 (현재는 config만, Phase 2 에서 Redis 또는 DB counter)
- Burst credit (5분 windowed token refund)
- Monthly soft limit (warning email + grace period)
- Model-level override (특정 고가 모델 제한)

---

## 6. 감사 로깅 `[P1]`

### 6.1 AuditEntry

```rust
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub request_id: Uuid,
    pub tenant_id: Uuid,
    pub api_key_id: Uuid,
    pub model: String,                  // 요청 model 이름
    pub provider: String,               // 실제 route 된 provider
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub latency_ms: u32,
    pub status_code: u16,
    pub cost_cents: i64,                // D-8: i64 cents (Phase 1 은 0)
    pub timestamp: DateTime<Utc>,
}
```

**의도 (Round 1 보존)**:
- 요청 단위 추적
- 90 일 보존 (Phase 2 에 GDPR 마스킹 30/90 day 정책)
- Billing/analytics 의 raw source

### 6.2 AuditWriter trait

```rust
#[async_trait]
pub trait AuditWriter: Send + Sync {
    async fn write(&self, entry: AuditEntry) -> Result<()>;
}
```

### 6.3 비동기 write path (hot path 미보호)

`PgAuditWriter` 는 내부에서 `tokio::sync::mpsc::channel(4096)` 를 소유 (**D-20260411-09**: 1024 → 4096 4배 확대). `write()` 는 **non-blocking** (`try_send`), 가득 찬 경우:
- `metrics::counter!("gadgetron_xaas_audit_dropped_total").increment(1)` — Prometheus 경보 소스.
- `tracing::warn!(tenant_id, request_id, ...)` — 디버깅 용 single-entry log.
- 요청 hot path 영향 없음 (200 유지).

백그라운드 task 가 100 entries 또는 1초 타임아웃 단위로 batch 하여 `sqlx::query!` INSERT 수행.

```
request -> AuditWriter.write() -> try_send(entry) --+
                                                    |
                         [mpsc channel bufsize=4096]|   (D-20260411-09)
                                                    v
                         batcher task <-------------+
                         flushes 100 or 1s -> PG INSERT (UNNEST)
```

**Graceful shutdown (A5)**: `PgAuditWriter::flush(&self) -> Result<(), GadgetronError>` async 메서드 제공. `main.rs` 의 `axum::serve().with_graceful_shutdown(...)` 콜백에서 5초 `tokio::time::timeout` 으로 호출. 상세는 `docs/design/xaas/phase1.md §2.2.4`.

> **Phase 1 경량 정책 — 배포 전 compliance review 필수** (D-20260411-09 옵션 C).
> SOC2/HIPAA/GDPR 대상 배포 전에는 반드시 §6.5 Phase 2 WAL/S3 fallback 을 활성화하거나 compliance 팀과 drop rate 허용선 합의 필수.

### 6.4 PostgreSQL 스키마

```sql
-- 20260411000004_audit_log.sql
CREATE TABLE audit_log (
    id            BIGSERIAL PRIMARY KEY,
    request_id    UUID NOT NULL,
    tenant_id     UUID NOT NULL,
    api_key_id    UUID NOT NULL,
    model         VARCHAR(255) NOT NULL,
    provider      VARCHAR(64) NOT NULL,
    input_tokens  INT NOT NULL DEFAULT 0,
    output_tokens INT NOT NULL DEFAULT 0,
    latency_ms    INT NOT NULL,
    status_code   SMALLINT NOT NULL,
    cost_cents    BIGINT NOT NULL DEFAULT 0,   -- D-8: i64 cents
    timestamp     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX audit_log_tenant_ts_idx ON audit_log(tenant_id, timestamp DESC);
CREATE INDEX audit_log_request_idx   ON audit_log(request_id);
```

**인덱스 설명**:
- `audit_log_tenant_ts_idx` — 테넌트별 최신순 쿼리 (대시보드)
- `audit_log_request_idx` — 디버깅용 single request 조회

### 6.5 Phase 2 확장 (D-20260411-09 fallback 포함)

**Phase 2 감사 drop fallback** (D-20260411-09 옵션 C 의 "Phase 2 계획" 명시화):

1. **`gadgetron-xaas/src/audit/fallback.rs`** 신설 — `AuditWriter` trait 뒤에 1차 / 2차 fallback chain 도입.
2. **1차 fallback: local WAL 파일**
   - `/var/lib/gadgetron/audit-wal/*.jsonl` 에 append + size-based rotation (100 MB / 1시간).
   - mpsc 채널이 full 이거나 PG insert 가 지속적으로 실패할 때 flush.
   - Process restart 시 startup 루틴이 WAL 을 PG 로 drain.
3. **2차 fallback: S3 batch upload**
   - 1차 fallback 이 disk full 상태이거나 PG 장애가 15분 이상 지속될 때 발동.
   - tenant 별 prefix (`s3://gadgetron-audit/<tenant_id>/YYYY/MM/DD/*.jsonl.gz`), 15분 flush 윈도우.
   - 상세 사양은 위 (1)–(4) 항목에 인라인 명시; 별도 설계 문서 미작성.
4. **Graceful shutdown chain**: `PgAuditWriter::flush() → WAL flush → S3 upload`, 전체 30 초 timeout.
5. **Compliance 체크리스트**: 90일 retention, 30/90일 GDPR 익명화, 검색 indexing (Elasticsearch 또는 OpenSearch), sensitive field masking (`system prompt`, `tool args`).

**Phase 2 다른 audit 확장**:
- GDPR 익명화 (30일 후 IP 해시화, 90일 후 PII 삭제)
- S3/Glacier archival (90일 이후 cold storage)
- Elasticsearch export (real-time analytics)
- Sensitive field masking (system prompt, tool args)
- 배포 가이드 (향후 `docs/modules/deployment-operations.md` 혹은 ops 문서) 에 "프로덕션 배포 전 compliance review 필수" 게이트 주석.

---

## 7. 게이트웨이 통합 (미들웨어 체인) `[P1]`

### 7.1 `gadgetron-gateway` 미들웨어 체인

`docs/modules/gateway-routing.md §5.2` 에서 정의한 9-stage 체인 중 Phase 1 에 실제로 적용되는 3 stage:

```
 Request -> [AuthLayer] -> [QuotaLayer] -> [Router] -> Provider
                |              |                             |
                v              v                             v
          TenantContext   QuotaToken                   [AuditLayer]
          (extensions)    (extensions)                       |
                                                             v
                                                  AuditWriter.write()
```

- **AuthLayer**: `gadgetron_xaas::auth::middleware::AuthLayer`
  - `Authorization: Bearer <raw_key>` 추출
  - `KeyValidator::validate()` 호출
  - 성공 시 `TenantContext` 를 `Request::extensions_mut()` 에 삽입
  - 실패 시 401 (missing) / 403 (invalid)

- **QuotaLayer**: `gadgetron_xaas::quota::middleware::QuotaLayer`
  - Upstream `TenantContext` 추출
  - `QuotaEnforcer::check_pre()` → `QuotaToken` 삽입
  - 초과 시 429 + `Retry-After` 헤더
  - Response 후 `record_post()` 호출 (`tower::ServiceExt::map_result` 활용)

- **AuditLayer**: `gadgetron_xaas::audit::middleware::AuditLayer`
  - Response 완료 후 `AuditEntry` 생성
  - `AuditWriter::write()` (non-blocking)

### 7.2 Bearer 인증 프로토콜

```
HTTP/1.1 POST /v1/chat/completions
Authorization: Bearer gad_live_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
Content-Type: application/json

{ "model": "gpt-4", "messages": [...] }
```

- `gad_vk_` 인 경우: `tenant_short` 로부터 tenant_id prefix lookup (fast path)
- `gad_live_` / `gad_test_` 인 경우: 전체 hash 로 lookup

### 7.3 Error 매핑 (D-13 + D-20260411-08)

Phase 1 HTTP status 매핑 전체 테이블은 `docs/design/xaas/phase1.md §2.4.3` 에 OpenAI 호환 body schema 와 함께 명시. 요약:

| 내부 에러 | HTTP | OpenAI 호환 `error.type` |
|-----------|:---:|---|
| `TenantNotFound` (invalid/missing key) | 401 | `invalid_api_key` |
| `TenantNotFound` (scope denied, D-10) | 403 | `scope_forbidden` |
| `QuotaExceeded("rpm ...")` | 429 | `rate_limit_exceeded` |
| `QuotaExceeded("tpm ...")` | 429 | `token_limit_exceeded` |
| `Billing("...")` (Phase 2) | 402 | `billing_issue` |
| `DownloadFailed` (Phase 2) | 500 | `internal_error` |
| `HotSwapFailed` (Phase 2) | 503 | `service_unavailable` |
| `StreamInterrupted { reason }` (D-20260411-08) | 499 | `stream_interrupted` |
| `Provider(String)` | 502 | `provider_error` |
| `Config(String)` | 500 | `internal_error` (generic, PII-safe) |

`IntoResponse for GadgetronError` 구현은 `gadgetron-gateway/src/error.rs` 에 위치 (Phase 1 설계 문서 §2.4.3 sketch 참조).

---

## 8. 과금 엔진 `[P2]`

> **Phase 1 에서는 `cost_cents` 를 `audit_log` 에 기록만 합니다** (항상 0). 실제 계산 로직은 Phase 2 에서 추가.

### 8.1 LedgerEntry 모델 (D-8)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub request_id: Uuid,
    pub input_token_cost_cents: i64,    // D-8
    pub output_token_cost_cents: i64,
    pub gpu_compute_cost_cents: i64,
    pub vram_cost_cents: i64,
    pub qos_surcharge_cents: i64,
    pub mig_surcharge_cents: i64,
    pub total_cost_cents: i64,
    pub created_at: DateTime<Utc>,
}
```

**중요**: 모든 필드가 `i64` (센트). `f64` 금지. 예: $1.50 = `150`.

### 8.2 Phase 2 PostgreSQL 스키마 (미리보기)

```sql
-- Phase 2: billing_ledger.sql
CREATE TABLE billing_ledger (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    request_id               UUID NOT NULL,
    input_token_cost_cents   BIGINT NOT NULL DEFAULT 0,
    output_token_cost_cents  BIGINT NOT NULL DEFAULT 0,
    gpu_compute_cost_cents   BIGINT NOT NULL DEFAULT 0,
    vram_cost_cents          BIGINT NOT NULL DEFAULT 0,
    qos_surcharge_cents      BIGINT NOT NULL DEFAULT 0,
    mig_surcharge_cents      BIGINT NOT NULL DEFAULT 0,
    total_cost_cents         BIGINT NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX billing_ledger_tenant_created_idx
    ON billing_ledger(tenant_id, created_at DESC);
```

### 8.3 요금 계산 공식 (i64 산술)

```
total_cost_cents =
      input_tokens  * input_rate_millicents_per_ktoken  / 1000
    + output_tokens * output_rate_millicents_per_ktoken / 1000
    + gpu_seconds   * gpu_hourly_cents                  / 3600
    + vram_mb * gpu_seconds * vram_mb_hourly_cents      / (3600 * 1024)
    + qos_multiplier_scaled (i64, basis points)
    + mig_surcharge_scaled   (i64, basis points)
```

모든 곱셈은 `i64::checked_mul` 로 감쌀 것. Overflow 시 `GadgetronError::Billing("overflow in cost calc")` (D-13).

### 8.4 단가 테이블 (i64 cents)

```rust
pub struct BillingRates {
    // 1,000 tokens 당 cents (Phase 2 hot-loaded from config)
    pub input_cents_per_ktoken:  HashMap<String, i64>,  // model_id -> cents
    pub output_cents_per_ktoken: HashMap<String, i64>,

    // GPU type -> cents per hour
    pub gpu_hourly_cents: HashMap<String, i64>,

    // basis points (10000 = 1.0x)
    pub qos_multiplier_bp: HashMap<QosTier, u32>,
    pub mig_surcharge_bp:  HashMap<MigProfile, u32>,
}
```

예시:
- $0.005 per 1K input tokens = `500` millicents per ktoken = **실제 저장 시 `5` cents per ktoken 이면 해상도 부족 → millicent 필요**
- 따라서 rate 스토리지는 `i64` millicent (1 cent = 1000 mc), `total_cost_cents` 계산 시 `/1000` 으로 cent 변환
- `billing_ledger.*_cents` 는 cent 단위로 저장

### 8.5 Billing 에러 (D-13)

```rust
// Phase 2 예시
if total_cost_cents < 0 {
    return Err(GadgetronError::Billing(
        format!("negative cost for req={request_id}")
    ));
}
```

---

## 9. HuggingFace 카탈로그 `[P2]`

> **Phase 1 에서는 model 이름을 검증하지 않고 raw string 으로 처리**. 카탈로그 통합은 Phase 2.

### 9.1 Catalog 테이블 (Phase 2 미리보기)

```sql
-- Phase 2: model_catalog.sql
CREATE TABLE model_catalog (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_id         VARCHAR(255) NOT NULL,         -- "meta-llama/Llama-3.1-70B"
    architecture     VARCHAR(64) NOT NULL,
    params_billion   NUMERIC(6,2) NOT NULL,         -- display only
    quantization     VARCHAR(32) NOT NULL,
    engine           VARCHAR(32) NOT NULL,
    license          VARCHAR(64) NOT NULL,
    vram_mb          INT NOT NULL,                  -- 추정 VRAM
    downloaded       BOOLEAN NOT NULL DEFAULT FALSE,
    local_path       TEXT,
    checksum_sha256  CHAR(64),
    file_size_bytes  BIGINT,
    huggingface_id   VARCHAR(255),
    is_gated         BOOLEAN NOT NULL DEFAULT FALSE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(model_id, quantization, engine)
);

CREATE INDEX model_catalog_arch_idx ON model_catalog(architecture);
CREATE INDEX model_catalog_hf_idx   ON model_catalog(huggingface_id);
```

### 9.2 DownloadManager 타입 (재사용)

`DownloadState` 는 `gadgetron-core::model::DownloadState` 를 재사용 (D-12, C-5 해결). 별도 정의 금지.

```rust
// gadgetron-core (기존)
pub enum DownloadState {
    Queued,
    Downloading { bytes_downloaded: u64, bytes_total: u64 },
    Verifying,
    Completed { path: PathBuf },
    Failed { error: String },
    Cancelled,
}
```

`gadgetron-xaas::catalog::download::DownloadManager` (Phase 2) 는 다음 실패 경로에서 `GadgetronError::DownloadFailed(String)` (D-13) 반환:
- HuggingFace API 401/403
- 체크섬 불일치
- 네트워크 타임아웃
- 디스크 full

### 9.3 호환성 검사 (Phase 2, 요약)

`HfCompatibilityChecker` 가 Phase 2 에서 다음 4가지 체크:
1. 아키텍처 → 엔진 support 매트릭스
2. 양자화 포맷 → 엔진 호환
3. VRAM 요구량 vs 가용 VRAM (`gadgetron-core::model::estimate_vram_mb()`)
4. 라이선스 + gate 상태

상세는 Phase 2 설계 문서 `docs/design/xaas/phase2-catalog.md` (추후 작성).

---

## 10. AgentaaS `[P2]`

> **Phase 1 에는 포함되지 않음**. 여기는 Phase 2 확장을 위한 **개념적 설계**만 서술.

### 10.1 에이전트 생명주기 FSM (보존)

```
                    +---------+
                    | CREATED |
                    +----+----+
                         | configure
                    +----v-------+
          +-------->|CONFIGURED  |
          |         +----+-------+
          |              | start
          |         +----v------+
          |         | RUNNING   |<---+
          |         +----+------+    |
          |              |           | resume
          |         +----v------+    |
          |         | PAUSED    |----+
          |         +----+------+
          |              | destroy
          |         +----v------+
          |         | DESTROYED |
          |         +-----------+
          |
          |  +-------+
          |  | ERROR |
          +--+-------+
```

### 10.2 Phase 2 테이블 (미리보기)

```sql
-- Phase 2: agents.sql
CREATE TABLE agents (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id        UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name             VARCHAR(255) NOT NULL,
    state            VARCHAR(16) NOT NULL DEFAULT 'created',
    model_id         VARCHAR(255) NOT NULL,
    system_prompt    TEXT NOT NULL,
    temperature      DOUBLE PRECISION NOT NULL DEFAULT 0.7,   -- 모델 temperature, 과금 아님
    max_tokens       INT NOT NULL DEFAULT 2048,
    tools            JSONB NOT NULL DEFAULT '[]',
    memory_config    JSONB NOT NULL DEFAULT '{}',
    sandbox_profile  VARCHAR(64),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX agents_tenant_idx ON agents(tenant_id);

-- Phase 2: agent_memory.sql
CREATE TABLE agent_memory (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id         UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    conversation_id  UUID NOT NULL,
    role             VARCHAR(16) NOT NULL,        -- 'system' | 'user' | 'assistant' | 'tool'
    content          TEXT NOT NULL,
    tool_call_id     UUID,
    token_count      INT NOT NULL DEFAULT 0,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX agent_memory_conv_idx ON agent_memory(conversation_id, created_at);

-- Phase 2: agent_tools.sql
CREATE TABLE agent_tools (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id         UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    tool_name        VARCHAR(128) NOT NULL,
    risk_level       VARCHAR(16) NOT NULL,        -- 'safe' | 'moderate' | 'dangerous' | 'critical'
    requires_approval BOOLEAN NOT NULL DEFAULT FALSE,
    allowed          BOOLEAN NOT NULL DEFAULT TRUE
);
```

### 10.3 장기 메모리

- Vector store: **Qdrant** 또는 **pgvector** (Phase 2 선택)
- 임베딩 모델: `BAAI/bge-small-en-v1.5` (기본)
- Top-K: 5, similarity threshold: 0.7
- 저장 전략: episodic / semantic / hybrid

### 10.4 도구 호출 브릿지

- Tool-call 요청 → 권한 검사 → 샌드박스 실행 → 결과 반환
- 위험 등급별 샌드박스 프로파일 (default / data-analysis / code-exec / web-agent)
- 타임아웃 · CPU · 메모리 · 디스크 · 네트워크 제한

### 10.5 멀티 에이전트 오케스트레이션

5가지 패턴 지원: Sequential / Parallel / Hierarchical / Pipeline / Debate.

상세는 Phase 2 설계 문서 `docs/design/xaas/phase2-agent.md` (추후 작성).

---

## 11. GPUaaS (MIG, 타임슬라이싱, 예약) `[P2]`

> **Phase 1 에는 포함되지 않음**. `gadgetron-scheduler` + `gadgetron-node` 가 기본 GPU 할당을 담당하고, `gadgetron-xaas::gpuaas` 는 Phase 2 에 multi-tenant wrapper + 예약 시스템으로 추가.

### 11.1 할당 API (Phase 2 미리보기)

`REST` 엔드포인트 (D-7 네임스페이스 준수):

```
POST   /api/v1/xaas/gpu/allocate        → GPU 할당 요청 (Phase 2)
DELETE /api/v1/xaas/gpu/allocations/{id} → GPU 할당 해제
GET    /api/v1/xaas/gpu/allocations/{id} → 할당 상세 조회
GET    /api/v1/xaas/gpu/allocations      → 할당 목록 조회
GET    /api/v1/xaas/gpu/cluster/status   → 클러스터 상태 조회
```

> **D-5**: Phase 2 에서 gRPC 를 추가할 때 동일 endpoint 를 `tonic` proto 로 미러링. Phase 1 은 REST 전용.

### 11.2 할당 요청 body (Phase 2)

```json
{
  "gpu_type": "NVIDIA_A100_80GB",
  "vram_mb": 40960,
  "gpu_count": 1,
  "mig_profile": "1g.10gb",
  "qos_tier": "standard",
  "strategy": "best_fit",
  "priority": 5,
  "timeout_ms": 30000,
  "reservation_id": null
}
```

### 11.3 QoS 등급별 정책 (보존)

| QoS 등급 | 최소 보장 비율 | 최대 허용 비율 | 선점 가능 | VRAM 보장 |
|---------|-------------|-------------|----------|----------|
| DEDICATED | 100% | 100% | No | 전체 |
| STANDARD | 50% | 80% | No | 요청량 보장 |
| BURSTABLE | 0% | 100% | Yes | 최소 4GB |

### 11.4 MIG 프로파일 자동 선택 (Phase 2)

```rust
pub fn select_mig_profile(
    gpu_type: GpuType,
    workload: &WorkloadCharacteristics,
) -> MigProfile {
    // A100/H100: vram 기반 선택
    //   <= 5 GB    -> 1g.5gb
    //   <= 10 GB   -> 1g.10gb
    //   <= 20 GB   -> 2g.20gb
    //   <= 40 GB   -> 3g.40gb
    //   full       -> 7g.80gb
}
```

### 11.5 GPU 쿼터 테이블 (Phase 2)

```sql
-- Phase 2: gpu_quotas.sql
CREATE TABLE gpu_quotas (
    tenant_id         UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    gpu_type          VARCHAR(64) NOT NULL,
    max_count         INT NOT NULL,
    max_vram_mb       BIGINT NOT NULL,
    max_gpu_hours     INT NOT NULL,          -- display; i64 hours
    qos_tier          VARCHAR(16) NOT NULL DEFAULT 'standard',
    mig_enabled       BOOLEAN NOT NULL DEFAULT FALSE,
    time_slice_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, gpu_type)
);

-- Phase 2: gpu_usage.sql
CREATE TABLE gpu_usage (
    id               BIGSERIAL PRIMARY KEY,
    tenant_id        UUID NOT NULL REFERENCES tenants(id),
    allocation_id    UUID NOT NULL,
    gpu_type         VARCHAR(64) NOT NULL,
    vram_mb          INT NOT NULL,
    started_at       TIMESTAMPTZ NOT NULL,
    ended_at         TIMESTAMPTZ,                    -- NULL = active
    gpu_seconds      BIGINT GENERATED ALWAYS AS (
        CASE WHEN ended_at IS NOT NULL
             THEN EXTRACT(EPOCH FROM (ended_at - started_at))::BIGINT
             ELSE 0 END
    ) STORED,
    qos_tier         VARCHAR(16) NOT NULL,
    cost_cents       BIGINT NOT NULL DEFAULT 0     -- D-8
);

CREATE INDEX gpu_usage_tenant_idx ON gpu_usage(tenant_id);
CREATE INDEX gpu_usage_time_idx   ON gpu_usage(started_at, ended_at);
```

### 11.6 예약 시스템 (Phase 2)

우선순위 큐 기반:
- `ReservationPriority`: `Low` / `Normal` / `High` / `Critical`
- `WaitPolicy`: `Queue` / `Fail` / `BestEffort`
- Recurring reservations: iCalendar RRULE 형식

상세는 Phase 2 설계 문서 `docs/design/xaas/phase2-gpuaas.md` (추후 작성).

---

## 12. 부록: 설정 스키마

### 12.1 Phase 1 `gadgetron-xaas` TOML 섹션

```toml
# gadgetron.toml (Phase 1)

[xaas.database]
url             = "postgres://gadgetron:${PGPASSWORD}@localhost:5432/gadgetron"
max_connections = 20
migrate_on_start = true

[xaas.auth]
# Phase 1 default. 관리자는 `gadgetron xaas keys create` CLI 로 발급.
allow_test_keys  = true                      # gad_test_* 허용
default_key_ttl  = "365d"

[xaas.quota.defaults]
rpm = 60
tpm = 1_000_000
concurrent_max = 10

[xaas.audit]
# D-20260411-09: 채널 용량 1024 → 4096 (4배 확대)
channel_capacity = 4096       # mpsc buffer size
batch_size       = 100
flush_interval_ms = 1000
# 배포 전 compliance review 필수. Phase 2 에 WAL/S3 fallback (§6.5).
```

### 12.2 Phase 2 추가 TOML (예고)

```toml
# Phase 2 에만 추가됨

[xaas.billing]
currency = "USD"
billing_cycle = "monthly"
invoice_day = 1

[xaas.billing.rates]
# millicents per 1K tokens
"gpt-4".input  = 30_000
"gpt-4".output = 60_000

[xaas.catalog.huggingface]
api_base_url = "https://huggingface.co/api"
max_concurrent_downloads = 3
verify_checksums = true
default_token_path = "/etc/gadgetron/hf-token"

[xaas.agent]
max_concurrent_per_tenant = 50
default_timeout_ms = 120000
sandbox_default = "default"

[xaas.agent.memory]
default_short_term_messages = 50
default_short_term_tokens = 8192
long_term_default_store = "qdrant"
embedding_model = "BAAI/bge-small-en-v1.5"

[xaas.gpuaas]
cluster_name = "gadgetron-cluster-01"
time_slicing_enabled = true
mig_auto_partition = true
```

### 12.3 환경 변수

```bash
# Phase 1 (D-4 준수)
DATABASE_URL=postgres://gadgetron:xxx@localhost:5432/gadgetron

# API 키 prefix (D-11)
# 문서화 목적 — 실제 prefix 는 자동 생성
# gad_live_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
# gad_test_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
# gad_vk_<tenant_short>_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

# Phase 2 에 추가
HF_TOKEN=hf_xxxxxxxxxxxxxxxxxxxx
QDRANT_URL=http://qdrant:6333
```

### 12.4 Docker Compose (Phase 1 만)

```yaml
version: "3.9"

services:
  gadgetron:
    image: gadgetron/gadgetron:latest
    ports: ["8080:8080"]
    volumes:
      - ./gadgetron.toml:/etc/gadgetron/gadgetron.toml:ro
    environment:
      - DATABASE_URL=postgres://gadgetron:devpass@postgres:5432/gadgetron
      - RUST_LOG=info
    depends_on:
      - postgres

  postgres:
    image: postgres:16-alpine
    environment:
      - POSTGRES_USER=gadgetron
      - POSTGRES_PASSWORD=devpass
      - POSTGRES_DB=gadgetron
    volumes:
      - pg-data:/var/lib/postgresql/data
    ports:
      - "5432:5432"

volumes:
  pg-data:
```

**주의**: SQLite volume 금지 (D-4). 이전 draft 의 `sqlite:///data/gadgetron/gadgetron.db` 는 완전히 제거되었습니다.

---

## 13. 에러 처리 (D-13 + D-20260411-08)

`gadgetron-xaas` 는 `gadgetron-core::GadgetronError` 6개의 추가 variant 에 의존합니다 (`chief-architect` 트랙):

```rust
pub enum GadgetronError {
    // ... 기존 variants ...
    Billing(String),                     // D-13: 과금 오버플로우, rate 조회 실패
    TenantNotFound(String),              // D-13: tenant_id 미존재, invalid api key, scope denied
    QuotaExceeded(String),               // D-13: RPM/TPM/concurrent 초과
    DownloadFailed(String),              // D-13: HuggingFace download 실패 (Phase 2)
    HotSwapFailed(String),               // D-13: 핫 스왑 실패 (Phase 2 scheduler 용도)
    StreamInterrupted { reason: String}, // D-20260411-08: SSE 스트림 중단
}
```

**주의**:
- D-13 5개 + D-20260411-08 1개 = 총 6개 variant 가 `gadgetron-core::error` 에 추가되기 전까지는 `gadgetron-xaas` 컴파일 블로커.
- `StreamInterrupted` 는 `gadgetron-gateway/src/sse.rs` 가 client abort/network timeout 감지 시 생성.
- Track 1 (`chief-architect` — `docs/design/core/types-consolidation.md`) 의 Round 1 retry A4 가 이 variant 추가를 담당.

---

## 14. 관찰성 (Observability)

### 14.1 Tracing span 이름 (Phase 1)

| span | level | fields |
|------|-------|--------|
| `xaas.auth.validate` | debug | `tenant_id`, `key_kind` |
| `xaas.quota.check_pre` | debug | `tenant_id`, `rpm_remaining`, `tpm_remaining` |
| `xaas.quota.record_post` | trace | `tenant_id`, `actual_tokens` |
| `xaas.audit.write` | trace | `request_id`, `tenant_id` |
| `xaas.audit.batch_flush` | info | `batch_size`, `lag_ms` |

### 14.2 Prometheus 메트릭 (Phase 1)

```
gadgetron_xaas_auth_requests_total{kind="live|test|virtual", result="ok|invalid|expired"}
gadgetron_xaas_quota_requests_total{tenant_id, result="ok|rpm_exceeded|tpm_exceeded"}
gadgetron_xaas_audit_batch_latency_ms_bucket{le="..."}
gadgetron_xaas_audit_channel_capacity{}   # full, unfilled
gadgetron_xaas_audit_dropped_total{}
```

### 14.3 Phase 2 메트릭 (예고)

```
gadgetron_xaas_billing_ledger_cents_total{tenant_id}
gadgetron_xaas_gpuaas_allocations{gpu_type, qos_tier}
gadgetron_xaas_agent_state{state}
```

---

## 15. 단위 테스트 / 통합 테스트 개요

Phase 1 의 상세 테스트 계획은 `docs/design/xaas/phase1.md §4-5` 참조.

### 15.1 Phase 1 단위 테스트 대상
- `ApiKey::parse` 포맷 케이스 10+
- `PgKeyValidator` round-trip (testcontainers Postgres)
- `TokenBucket` refill 로직 (`tokio::time::pause`)
- `QuotaEnforcer::{check_pre, record_post}` 상태머신
- `PgAuditWriter` batch flush / overflow / backpressure
- `AuthLayer` middleware 200/401/403

### 15.2 Phase 1 통합 테스트 시나리오 (`gadgetron-testing` harness)
- Bearer → AuthLayer → handler → AuditLayer full flow
- Rapid request → 429 (RPM)
- Large token → 429 (TPM)
- Virtual key → tenant resolution
- PG migration round-trip

---

## 16. 오픈 이슈 / Phase 전환 준비

| ID | 항목 | 대응 Phase |
|----|------|-----------|
| X1 | Redis 도입 여부 (rate limit 분산) | Phase 2 (decision log) |
| X2 | OAuth2/OIDC integration | Phase 2 |
| X3 | Ledger → 외부 accounting (Stripe/QuickBooks) | Phase 3 |
| X4 | pgvector vs Qdrant 최종 선택 | Phase 2 |
| X5 | Sandbox profile enforcement (cgroups/nsjail) | Phase 2 |
| X6 | Audit S3 archival 형식 (JSONL vs Parquet) | Phase 2 |

---

## 17. 문서 이력

| 버전 | 날짜 | 변경 내용 | 작성자 |
|------|------|----------|--------|
| 1.0.0-draft | 2026-04-11 | 초안 작성 (SQLite 기반) | Platform Lead |
| 2.0.0-draft | 2026-04-11 | **전면 재작성**: D-4/D-8/D-11/D-13/D-20260411-01/D-20260411-03 반영 (PostgreSQL, i64 cents, gad_ prefix, 경량 Phase 1, gadgetron-xaas crate) | @xaas-platform-lead |
| 2.0.1-draft | 2026-04-11 | **Round 1 retry**: D-20260411-08 (`StreamInterrupted`), D-20260411-09 (audit 채널 4096 + Phase 2 WAL/S3 fallback §6.5), D-20260411-10 (`Scope` enum + `api_keys.scopes TEXT[]` schema + 기본 scope 매트릭스). HTTP status 매핑 테이블 확장, graceful shutdown drain, `IF NOT EXISTS` 마이그레이션 idempotency. 상세: `docs/design/xaas/phase1.md` §2.1.4/§2.2.4/§2.4.3. | @xaas-platform-lead |

---

## 관련 문서

- [`docs/design/xaas/phase1.md`](../design/xaas/phase1.md) — Phase 1 크레이트 상세 설계 (공개 API · 스키마 · 테스트)
- [`docs/modules/gateway-routing.md`](./gateway-routing.md) — 미들웨어 체인 연결 지점
- [`docs/reviews/pm-decisions.md`](../reviews/pm-decisions.md) — D-1 ~ D-13 확정 결정
- [`docs/reviews/round1-pm-review.md`](../reviews/round1-pm-review.md) — Round 1 이슈 (O-4, O-6, O-9)
- [`docs/reviews/round2-platform-review.md`](../reviews/round2-platform-review.md) — Round 2 BLOCKER 목록
- [`docs/process/04-decision-log.md`](../process/04-decision-log.md) — D-20260411-01 ~ 05 승인 결정
