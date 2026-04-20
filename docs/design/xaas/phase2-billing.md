# gadgetron-xaas Phase 2 — integer-cent billing ledger (ISSUE 12)

> **담당**: @xaas-platform-lead
> **상태**: TASK 12.1 shipped (PR #236 / 0.5.5) + TASK 12.2 shipped (PR #241 / 0.5.6, tool + action emission — ISSUE 12 closed at telemetry scope) + **ISSUE 23 per-user attribution shipped (PR #271 / 0.5.15)** — `actor_user_id` column + tenant-first composite index + 8-arg `insert_billing_event`. ISSUE 24 is the queued follow-up that threads real `user_id` through `QuotaToken` + `AuthenticatedContext` so chat + action paths populate the column (tool path populates today). TASKs 12.3 (invoice materialization), 12.4 (counter/ledger reconciliation), 12.5 (Stripe ingest) DEFERRED per 2026-04-20 commercialization-layer direction — designed in this doc (§6–§8), not scheduled into an active ISSUE.
> **작성일**: 2026-04-19
> **관련 크레이트**: `gadgetron-xaas` (새 `billing` 모듈), `gadgetron-gateway`, `gadgetron-cli`
> **Phase**: [P2] — extends Phase 1 quota infrastructure (`phase1.md`)
> **반영 결정**: D-4 (PostgreSQL), D-8 (i64 cents, no floats)
> **선행 TASKs**: ISSUE 11 전체 (quota pipeline이 record_post에서 billing_events를 발행함)

---

## 1. 철학 & 컨셉 (Why)

### 1.1 해결하는 문제

**ISSUE 11이 "얼마 남았나"의 실시간 조회는 해결했지만, "지난 달에 무슨 일이 일어났는가"는 대답 못함.** 지금 `quota_configs.daily_used_cents / monthly_used_cents`는 카운터일 뿐 이력이 없다. 월말에 인보이스를 뽑으려면 (a) 이벤트 단위로 append-only 기록이 있어야 하고, (b) 모델/프로바이더/비용을 조회할 수 있어야 하고, (c) 과거 데이터를 의도치 않게 덮어쓰지 않아야 한다. 그게 이 ISSUE의 scope.

### 1.2 제품 비전과의 연결

- ROADMAP EPIC 4 (Multi-tenant XaaS) → v1.0.0의 전제. 빌링 원장이 없으면 진짜 SaaS라 부를 수 없음.
- ADR-D-8 (integer cents, i64): BIGINT로 `cost_cents` 기록, 변환/반올림 없음.
- `docs/modules/xaas-platform.md` 의 "metering pipeline → invoice materialization" 연결선의 맨 아래 절반.

### 1.3 고려한 대안 / 채택하지 않은 이유

| 대안 | 기각 이유 |
|------|----------|
| **ClickHouse / TimescaleDB** | 현 Postgres 스택에 신규 의존성 추가 비용 > 당장의 분석 쿼리 성능 이득. 이벤트 볼륨 초기엔 IOPS 평범한 테이블 + 인덱스로 충분. |
| **Kafka / 외부 큐** | fire-and-forget DB insert 하나보다 복잡. at-least-once 보장이 필요해지면 재검토. 현재는 ISSUE 11 record_post 경로와 같은 트랜잭션 경계로 충분. |
| **audit_log 확장 (열 추가)** | `audit_log`는 S/T 위협 모델에서 INSERT-only + UPDATE/DELETE/TRUNCATE 금지로 잠긴 보안 자산. 빌링은 invoice 정정(크레딧/환불)이 향후 생길 수 있고, 이를 위해 별도 테이블이 맞다. |
| **Floating point decimals** | **금지 (D-8)**. 세금 계산/합산에서 반올림 오차가 원장과 인보이스 불일치로 이어짐. |
| **Soft delete (is_deleted 플래그)** | append-only 원장이 목표. 정정이 필요하면 음수 `cost_cents` 보정 이벤트를 새로 INSERT. |

### 1.4 핵심 설계 원칙

1. **Append-only ledger**: `billing_events`는 INSERT만. 수정은 보정 행(음수 코스트)으로.
2. **Fire-and-forget on hot path**: `record_post`에서 INSERT 실패는 WARN 로그, 요청 성공은 유지. 빌링 ≠ 요청 gate.
3. **Integer cents everywhere**: `cost_cents BIGINT`. API 응답도 숫자 그대로 노출 (String 변환 없음).
4. **Tenant-boundary enforcement in handler**: `/admin/billing/events`는 쿼리 파라미터가 무엇이든 caller의 `tenant_id`로만 조회. 크로스-테넌트 조회 불가.
5. **Source-of-truth는 이벤트별 audit 테이블**: `source_event_id`는 `audit_log` / `tool_audit_events` / `action_audit_events`의 ID를 느슨하게 참조 (FK 없음) — 빌링이 먼저 INSERT되는 레이스를 막지 않기 위함.

### 1.5 Trade-offs

- **FK 없는 `source_event_id`**: 참조 무결성은 포기, 대신 write-path가 부모 테이블과 독립적. 트레이드오프: 고아 이벤트가 이론상 가능 (audit가 실패했는데 billing은 성공). 상쇄: audit 실패 시 요청 자체가 이미 실패했을 확률이 큼.
- **INSERT 실패를 WARN으로만**: 요청 성공을 지키기 위한 선택. 상쇄: 드문 경우 quota 카운터는 전진했는데 원장엔 없는 상황. §7 재조정(reconciliation)에서 다룸.
- **TASK 12.1에서는 chat만**: tool / action 이벤트는 다른 파이프라인을 타므로 별도 TASK(12.2 / 12.3). 당장의 wire-contract 안정화가 우선.

### 1.6 Threat Model (STRIDE, delta from phase1.md)

| Category | Threat | Asset | Mitigation | Phase |
|:---:|---|---|---|:---:|
| **T** (Tampering) | 다른 테넌트의 빌링 이벤트를 읽거나 수정 | `billing_events` | `/admin/billing/events` 핸들러가 ctx.tenant_id로 WHERE 절 고정; UPDATE/DELETE API 없음; Management scope 필수 | P2 |
| **I** (Info Disclosure) | 비-Management key로 인보이스 데이터 접근 | `billing_events` | `/admin/*` prefix로 RBAC 분기; OpenAiCompat key → 403 (harness gate 7k.7) | P2 |
| **R** (Repudiation) | INSERT 실패 시 활동 흔적 없음 | `billing_events` | `tracing::warn!(target: "billing")` + quota 카운터는 이미 전진 (reconciliation 수단으로 사용) | P2 accepted |
| **D** (DoS) | 대용량 쿼리로 DB 스트레스 | `billing_events` | `limit.clamp(1, 500)` + `(tenant_id, created_at DESC)` 인덱스 | P2 |

---

## 2. Data Model (SQL — 배포 상태)

```sql
-- migrations/20260420000002_billing_events.sql
CREATE TABLE IF NOT EXISTS billing_events (
    id                BIGSERIAL PRIMARY KEY,
    tenant_id         UUID         NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_kind        TEXT         NOT NULL CHECK (event_kind IN ('chat', 'tool', 'action')),
    source_event_id   UUID,
    cost_cents        BIGINT       NOT NULL,
    model             TEXT,
    provider          TEXT,
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS billing_events_tenant_created_idx
    ON billing_events (tenant_id, created_at DESC);

CREATE INDEX IF NOT EXISTS billing_events_kind_created_idx
    ON billing_events (event_kind, created_at DESC);

-- migrations/20260420000005_billing_events_actor_user_id.sql  (ISSUE 23)
ALTER TABLE billing_events
    ADD COLUMN IF NOT EXISTS actor_user_id UUID;

-- Tenant-first composite — forces per-user spend queries to pin
-- tenant_id (cross-tenant leakage defense per security review).
CREATE INDEX IF NOT EXISTS billing_events_tenant_actor_user_idx
    ON billing_events (tenant_id, actor_user_id, created_at DESC);
```

**Notes**
- `BIGSERIAL`: i64로 충분. 테넌트당 초당 1만건 기준 ~29만년치 용량.
- `ON DELETE CASCADE` on `tenant_id`: 테넌트 삭제 시 원장도 따라감. 컴플라이언스상 문제가 되면 ADR로 분리해 `RESTRICT`로 바꾸는 것을 추후 검토.
- `event_kind` CHECK: 3가지로 잠금. 새 종류 추가 시 migration 필요 — 이게 feature, bug 아님.
- `source_event_id`에 FK 없음: §1.5 참조.
- **`actor_user_id` (ISSUE 23)**: nullable, **FK 없음**. caller 가 heterogeneous sources (ValidatedKey.user_id / TenantContext.actor_user_id / AuthenticatedContext placeholder) 에서 populate 하므로 strict FK 는 buggy caller 를 silent INSERT failure 로 만들 위험이 있다. 운영자 reconciliation 쿼리는 read-time `LEFT JOIN users(id)` 로 병합. ISSUE 23 migration comment 가 이 결정을 authoritative 로 기록.

---

## 3. Public API 시그니처 (Rust — 배포 상태)

```rust
// crates/gadgetron-xaas/src/billing/events.rs
pub enum BillingEventKind { Chat, Tool, Action }

impl BillingEventKind {
    pub fn as_str(&self) -> &'static str { … } // "chat" | "tool" | "action"
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct BillingEventRow {
    pub id: i64,
    pub tenant_id: Uuid,
    pub event_kind: String,
    pub source_event_id: Option<Uuid>,
    pub cost_cents: i64,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// ISSUE 23 — denormalized projection of `audit_log.actor_user_id`
    /// (audit_log = source of truth). Nullable; see §1.5 + migration.
    pub actor_user_id: Option<Uuid>,
}

// Owned, 'static-safe insert payload — callers can move into
// tokio::spawn closures without lifetime gymnastics.
#[derive(Debug, Clone)]
pub struct BillingEventInsert {
    pub tenant_id: Uuid,
    pub kind: BillingEventKind,
    pub cost_cents: i64,
    pub source_event_id: Option<Uuid>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub actor_user_id: Option<Uuid>, // ISSUE 23
}

impl BillingEventInsert {
    // Typed constructors encode the kind + default cost invariant.
    pub fn chat(tenant_id: Uuid, cost_cents: i64) -> Self;
    pub fn tool(tenant_id: Uuid, gadget_name: String) -> Self;
    pub fn action(tenant_id: Uuid, audit_event_id: Uuid, gadget_name: Option<String>) -> Self;
    // Optional-field builders.
    pub fn with_actor_user(self, actor_user_id: Option<Uuid>) -> Self;
}

pub async fn insert_billing_event(
    pool: &PgPool,
    event: BillingEventInsert,
) -> Result<(), sqlx::Error>;

pub async fn query_billing_events(
    pool: &PgPool,
    tenant_id: Uuid,
    since: Option<chrono::DateTime<chrono::Utc>>,
    limit: i64, // already clamped at handler
) -> Result<Vec<BillingEventRow>, sqlx::Error>;
```

---

## 4. Hot-path 통합 (enforcer 확장)

```rust
// crates/gadgetron-xaas/src/quota/enforcer.rs PgQuotaEnforcer::record_post
// (after the existing UPDATE quota_configs …)
// ISSUE 24 will flip chat to `.with_actor_user(Some(token.user_id))`
// once QuotaToken carries user_id. Today the typed constructor
// defaults all optionals (source_event_id, model, provider,
// actor_user_id) to None.
let ins = crate::billing::insert_billing_event(
    &self.pool,
    crate::billing::BillingEventInsert::chat(token.tenant_id, actual_cost_cents),
).await;
if let Err(e) = ins {
    tracing::warn!(target: "billing", tenant_id = %token.tenant_id, error = %e,
        "failed to persist billing_events row — counter ahead of ledger until reconciled");
}
```

**Why no threading of model/provider/source_event_id now**: `QuotaEnforcer` trait는 현재 `actual_cost_cents: i64`만 받는다. TASK 12.2에서 이 trait 시그니처를 확장하면서 동시에 tool / action 이벤트 경로도 추가. 한 번의 wire change로 3가지 값을 threading — 한 TASK 한 wire change.

**Per-path `actor_user_id` nullability contract (ISSUE 23)**:

| 경로 | source | 현재 값 | ISSUE 24 이후 |
|------|--------|---------|---------------|
| chat | `PgQuotaEnforcer::record_post` (이 §) | `None` | `Some(token.user_id)` — `QuotaToken` 확장 |
| tool | `handlers.rs` tool billing | `Some(ctx.actor_user_id)` ✅ | unchanged |
| action | `action_service::emit_action_billing` | `None` — `AuthenticatedContext.user_id` 가 api_key_id placeholder 라 security review 가 반전 | `Some(actor.real_user_id)` — `AuthenticatedContext` 에 `real_user_id` 필드 추가 |

Harness Gate 7k.6b (PR #271) 이 per-kind population 을 직접 Postgres query 로 assert: chat=NULL + tool≥1 NOT NULL + action=NULL.

---

## 5. HTTP API (배포 상태)

```
GET /api/v1/web/workbench/admin/billing/events
    ?since=<ISO-8601>   (optional lower bound)
    &limit=<1..=500>    (default 100)

Auth: Bearer <Management-scope API key> (OpenAiCompat → 403)
Response 200:
  {
    "events": [
      {
        "id": 42,
        "tenant_id": "…uuid…",
        "event_kind": "chat",
        "source_event_id": null,
        "cost_cents": 17,
        "model": null,
        "provider": null,
        "created_at": "2026-04-19T18:12:03.441Z"
      },
      …
    ],
    "returned": 1
  }
```

**Tenant boundary**: 핸들러가 `ctx.tenant_id`로 WHERE 절을 고정. `since`/`limit` 외의 필터 파라미터는 wire-받지 않음 (향후 `event_kind` 필터는 TASK 12.2에서 추가).

---

## 6. Testability (Round 2 test plan)

### 6.1 Unit tests (in-module)

- `BillingEventKind::as_str` 문자열이 변하지 않음을 pinning (`wire_frozen`). **이미 landed**.
- `insert_billing_event` + `query_billing_events` 라운드트립 — `gadgetron-testing`의 `PostgresFixture` 사용. **TASK 12.2에서 추가** (현 TASK는 harness 로 cover).
- `query_billing_events` 의 `since` 필터가 경계치(`=` 포함)에서 올바른지 proptest. **TASK 12.2**.

### 6.2 E2E harness gates (이미 배포)

**Gate 7k.6** `/workbench/admin/billing/events`:
  - 비-스트리밍 chat 직후 `sleep 1` 후 `/admin/billing/events?limit=5` 호출
  - `.events[] | select(.event_kind == "chat") | length >= 1` 검증
  - `{events: array, returned: number}` shape 검증
  - cost_cents는 >= 0 (mock 모델은 0, 실모델은 양수)

**Gate 7k.7** RBAC 403:
  - OpenAiCompat scope API key로 `/admin/billing/events` 호출 → 403 기대

### 6.3 향후 load/property tests (TASK 12.2+)

- `proptest!` — (tenant, kind, cost_cents) 조합으로 INSERT 후 query round-trip
- ledger drift 테스트: 100개 chat 요청 후 `SUM(cost_cents) FROM billing_events` == `quota_configs.daily_used_cents`
- concurrent INSERT 100개 동시 발행 후 count 무결성

---

## 7. Reconciliation & 운영 (P2 scope)

`record_post`의 INSERT가 실패하면 카운터는 전진, 원장엔 없음 (§1.5). 이를 감지하고 보정하는 경로:

1. 일 1회 `cron` — `SELECT tenant_id, SUM(cost_cents) FROM billing_events WHERE created_at::date = CURRENT_DATE - 1 GROUP BY tenant_id`
2. `quota_configs.daily_used_cents` (어제 자정 직전 snapshot)와 비교
3. drift가 있으면 해당 테넌트에 대한 보정 이벤트 (`event_kind='chat', source_event_id=NULL, cost_cents = drift, model='__reconciliation__'`)를 INSERT

**당장 배포하는 것은 detection만** — 자동 보정은 TASK 12.4. 수동 SQL로 먼저 돌려봐야 실제 drift 크기를 안다.

---

## 8. 남은 TASKs (로드맵)

### TASK 12.2 — tool/action 이벤트 확장 (IN-FLIGHT, this cycle)

**설계 결정 (12.1 design에서 수정):** `QuotaEnforcer` trait 시그니처는 건드리지
않는다. 이유는 두 가지다.

1. chat 경로는 이미 `record_post`를 통해 발행하고 있고, 이 지점에 `kind/model/
   provider`를 threading 하려면 `QuotaToken`까지 건드려야 해서 wire-change가
   커진다.
2. tool / action 경로는 quota 파이프라인을 거치지 않는다 (tool은 MCP 권한,
   action은 워크벤치 approval 게이트). 거기에 quota trait를 끼워 넣는 건 잘못된
   추상화.

**대신 — 각 성공 경로에서 `insert_billing_event`를 직접 호출한다.**
동일한 fire-and-forget 원칙 (audit sink 패턴과 동치).

#### 시그니처 및 호출 지점

```rust
// 현재 시그니처 (struct-based after the 0.5.16 refactor — §3 참조):
pub async fn insert_billing_event(
    pool: &PgPool,
    event: BillingEventInsert,
) -> Result<(), sqlx::Error>;
```

**Tool (MCP /v1/tools/{name}/invoke)** — `crates/gadgetron-gateway/src/handlers.rs`
의 `invoke_tool_handler` 끝부분. `GadgetCallOutcome::Success`일 때만 발행:
```rust
if matches!(outcome, GadgetCallOutcome::Success) {
    if let Some(pool) = state.pg_pool.clone() {
        let tenant_id = ctx.tenant_id;
        let gadget_name = name.clone();
        let actor_user_id = ctx.actor_user_id;
        tokio::spawn(async move {
            let _ = gadgetron_xaas::billing::insert_billing_event(
                &pool,
                gadgetron_xaas::billing::BillingEventInsert::tool(tenant_id, gadget_name)
                    .with_actor_user(actor_user_id),
            ).await;
        });
    }
}
```

source_event_id는 현재 None — `tool_audit_events.id`가 UUID가 아니라 BIGSERIAL.
12.4 reconciliation 에서 tenant+gadget+timestamp로 조인하는 편법 사용. 깨끗한
UUID 링크는 tool_audit_events 스키마 확장이 필요해서 별도 ADR 건.

**Action (workbench direct-action)** — `crates/gadgetron-gateway/src/web/action_service.rs`
의 두 성공 경로 (직접 dispatch success @ line 447, approved dispatch success @ line 541).
여기서 `audit_event_id: Uuid`가 이미 생성돼 있으므로 **source_event_id에 그대로
사용 가능 (clean join!)**. action_service에 `pg_pool: Option<Arc<PgPool>>` 필드
추가하고 `new_full` 생성자에 파라미터로 받는다. CLI 초기화에서 wire.

```rust
// action_service.rs — direct success @ ~line 447 이후:
if let Some(pool) = self.pg_pool.as_ref() {
    let pool = pool.clone();
    let tenant_id = actor_tenant_id;
    let gadget_name = Some(descriptor.gadget_name.clone());
    let actor_user_id = actor.actor_user_id;
    tokio::spawn(async move {
        let _ = gadgetron_xaas::billing::insert_billing_event(
            &pool,
            gadgetron_xaas::billing::BillingEventInsert::action(
                tenant_id, audit_event_id, gadget_name,
            )
            .with_actor_user(actor_user_id),
        ).await;
    });
}
```

**cost_cents = 0 for tool/action in this TASK**: 현재 dispatcher 반환값에 cost가
없다. 0으로 기록하는 것은 "이벤트는 있었으나 금전 비용은 미할당" 상태로 유효.
원가 모델(per-call base fee, or token-count forward) 은 TASK 12.3 invoice
materialization 시점에 이벤트 테이블을 업데이트하지 않고 invoice line
materializer에서 조회 시 규정표를 적용하도록 계획.

#### Harness gates

- **Gate 7i.5 — tool billing emission**: Gate 7i.3에서 이미 성공하는
  `/v1/tools/wiki.list/invoke` 호출 이후, `sleep 1` → `/admin/billing/events`
  조회 → `.events[] | select(.event_kind == "tool") | length >= 1` 검증.
- **Gate 7h.X — action billing emission**: workbench 액션 실행 성공 gate
  직후 (harness에서 현재 `capture_action` 등 성공 경로가 있다면) billing에
  action-kind 행이 있는지 확인.

#### 버전

**이번 cycle에서 workspace 버전은 올리지 않는다** — ISSUE 12 close 시점의 단일 PR
에서 한 번에 0.5.5 → 0.5.6으로 bump. (feedback memory: PR granularity is ISSUE)

### TASK 12.3 — 인보이스 materialization

`invoices` + `invoice_lines` 테이블; 월말 요약 뷰. billing_events를 invoice_lines로
집계하는 쿼리는 per-kind 기본료 + 변수 비용 (chat은 cost_cents 합, tool은 건당
base fee, action은 operator 정책).

### TASK 12.4 — reconciliation cron + 자동 보정

- 일 1회 chat billing sum vs quota_configs.daily_used_cents 비교
- drift >0 이면 보정 이벤트 (model='__reconciliation__') INSERT
- tool/action의 source_event_id 누락 문제는 여기서 (tenant, gadget, timestamp)
  lookup으로 보완

### TASK 12.5 — Stripe webhook ingest

외부 결제 성공/환불 이벤트를 ledger에 반영 (음수 cost_cents for refund).

---

## 9. 참조
- Phase 1 foundation: `docs/design/xaas/phase1.md`
- ADR-D-8 (integer cents)
- ROADMAP §EPIC 4 / ISSUE 12
- Implementation code: `crates/gadgetron-xaas/src/billing/`, `crates/gadgetron-xaas/migrations/20260420000002_billing_events.sql`
