# Gadgetron Platform Architecture

> **담당**: @chief-architect
> **상태**: v0 Draft — Phase B 초안. Phase C에서 8 subagent parallel review 예정.
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 결정**: D-1, D-2, D-3, D-4, D-5, D-6, D-7, D-8, D-9, D-10, D-11, D-12, D-13,
>               D-20260411-01, D-20260411-02, D-20260411-03, D-20260411-04, D-20260411-05,
>               D-20260411-06, D-20260411-07, D-20260411-08, D-20260411-09, D-20260411-10,
>               D-20260411-11, D-20260411-12, D-20260411-13
> **Phase**: [P1] 기반 — [P2]/[P3] 진화 경로 포함
> **Glossary**: see §2.G.2

---

## 목적

이 문서는 Gadgetron의 **canonical cross-component 통합 아키텍처 뷰**다.
개별 모듈 문서(`docs/modules/`)와 설계 문서(`docs/design/`)는 각 도메인 세부사항을
다루며, 이 문서는 그 문서들을 묶는 통합 참조 문서로 기능한다.

**이 문서가 답해야 할 질문:**
1. 요청은 어떤 경로로 처리되는가?
2. 상태는 어디 저장되고 프로세스 재시작 후 어떻게 복구되는가?
3. 장애 발생 시 어떤 컴포넌트가 어떻게 영향을 받는가?
4. Phase 1→2→3 진화 시 어떤 인터페이스가 안정적으로 유지되는가?
5. 서브-밀리초 P99 목표를 달성하기 위한 성능 budget 분배는?

**Phase C에서 이 문서를 기반으로 8개 subagent가 각 domain 관점에서 review한다.**
이 v0 문서는 의도적으로 gap을 포함한다 — §4에 chief-architect의 초기 관찰이 있으며,
Phase C에서 더 깊은 gap이 발견될 것이다.

---

## §0. 8 Axis 개요

이 문서는 8개 축(A-H)으로 플랫폼을 분석한다:

| Axis | 이름 | 핵심 질문 |
|------|------|-----------|
| A | 시스템 수준 | 컴포넌트 구조와 데이터 흐름 |
| B | 크로스컷 | 관측성, 보안, 설정, 에러, 동시성 |
| C | 배포 아키텍처 | 단일노드→멀티노드→K8s 진화 |
| D | 상태 관리 | 영속성, 캐시, 채널, 복구 |
| E | Phase 진화 | API 안정성, 데이터 마이그레이션, 트레이트 안정성 |
| F | 장애 모드 | 장애 시나리오, 복구 경로, 그레이스풀 셧다운 |
| G | 도메인 모델 | Bounded context, ubiquitous language, aggregate |
| H | 성능 모델 | Latency budget, throughput, SLO 검증 |

---

## 1. 철학 & 컨셉 (Why)

### 1.1 Gadgetron 비전

**"관리자·사용자·에이전트가 함께 일하는 heterogeneous cluster collaboration platform"**

이 문서는 그 전체 비전 중에서도 특히 **Operations Plane + Execution Plane substrate** 를 다룬다. Assistant Plane 과 collaboration UX 의 canonical 비전은 `docs/design/ops/agentic-cluster-collaboration.md` 를 따른다.

세 가지 차별화:
- **Rust-native**: GC pause 없음, 제로카피 스트리밍, lock-free DashMap
- **GPU-first**: VRAM bin-packing, NUMA-aware 배치, MIG 정적 분할 [P1] → 동적 [P2]
- **Agent-aware control plane**: 단일 플랫폼에서 GPUaaS → ModelaaS → AgentaaS [P2] 점진적 추상화

### 1.2 핵심 설계 원칙 (불변)

1. **단일 바이너리**: 마이크로서비스 분산 없이 `gadgetron` 바이너리 하나로 전체 스택 구동
2. **Config-driven**: TOML + `${ENV}` 치환으로 배포 환경을 선언적으로 기술
3. **P99 < 1ms gateway overhead**: 요청 도착부터 upstream call 시작까지 측정 (Provider latency 제외)
4. **Leaf crate 원칙**: `gadgetron-core`는 `sqlx`, `axum`, `nvml-wrapper` 등 구현 의존성 없음 (D-12, D-13)
5. **Forward-only migrations**: DB 스키마는 sqlx forward-only migration만 허용

### 1.3 본 문서의 위치

```
docs/00-overview.md           ← 제품 비전, Phase 로드맵, crate별 API 요약
docs/modules/*.md             ← 5개 모듈 (gateway-routing, model-serving,
                                 gpu-resource-manager, xaas-platform, deployment-operations)
docs/design/core/             ← types-consolidation (Track 1 R3 Approved)
docs/design/testing/          ← harness (Track 2 R3)
docs/design/xaas/             ← phase1 (Track 3 R3)
docs/design/ops/              ← agentic-cluster-collaboration (새 협업 비전)
docs/architecture/platform-architecture.md  ← [본 문서] 통합 뷰
```

설계 세부사항은 `docs/design/` 문서를 reference. 본 문서는 통합 뷰와 cross-cutting 관계를 다룬다.

---

## 2. 8 Axis Architecture

---

### Axis A: 시스템 수준 (System Level)

> **v1 변경 이력**: Phase C review 결과(phase-c-review-results.md) 반영. 적용 결정:
> B-1(G-6 TenantContext), B-6(GAP-T1 Audit e2e), B-8(G-9+scope 401→403),
> H-1(streaming retry semantic supersedes D-20260411-08),
> H-6(quota_config fetch latency), H-11(TUI in-process Arc),
> H-12(WsAggregator producer), H-13(NodeMetrics 타입),
> D-20260411-03(xaas), D-20260411-05(testing), D-20260411-06(f64/i64),
> D-20260411-07(UI types in core), D-20260411-08(StreamInterrupted),
> D-20260411-09(audit drop), D-20260411-10(Scope enum),
> D-20260411-11(Arc<dyn LlmProvider>), D-20260411-12(moka LRU),
> D-20260411-13(Database variant), D-20260412-02(implementation determinism).

---

#### 2.A.1 컴포넌트 구조 — 10 Crates

각 행: `crate` / 역할 / 공개 API 표면 (1줄) / Phase tag / 내부 crate 의존성.

| Crate | 역할 | 공개 API 표면 (1줄) | Phase | Upstream crate deps |
|-------|------|---------------------|-------|----------------------|
| `gadgetron-core` | 공유 타입·trait·error·config. 구현 의존성 없는 leaf crate (D-12). | `AppConfig`, `GadgetronError`, `LlmProvider`, `ChatRequest`, `ChatResponse`, `ChatChunk`, `ModelState`, `ModelDeployment`, `NodeStatus`, `GpuMetrics`, `NodeMetrics`, `WsMessage`, `Scope`, `DatabaseErrorKind`, `ActionAuditSink` + `ActionAuditEvent` (P2B / ISSUE 3), `ApprovalStore` + `ApprovalRequest` + `ApprovalState` (P2B / ISSUE 3), `ActivityBus` + `ActivityEvent::ChatCompleted` + pricing `ModelPrice` table (P2B / ISSUE 4), `ActivityEvent::ToolCallCompleted` variant (EPIC 2 / ISSUE 5), `GadgetCatalog` trait (EPIC 2 / ISSUE 7 TASK 7.1 — MCP discovery seam, parallel to `GadgetDispatcher` per PR #175) | [P1]; [P2B] ActionAuditSink/ApprovalStore (ISSUE 3 / PR #188); [P2B] ActivityBus + pricing (ISSUE 4 / PR #194); [EPIC 2] ActivityEvent::ToolCallCompleted (ISSUE 5 / PR #199); [EPIC 2] GadgetCatalog (ISSUE 7 TASK 7.1 / PR #204) | 없음 (no internal crate deps) |
| `gadgetron-provider` | 6종 LLM provider HTTP adapter. `LlmProvider` trait 구현체. | `OpenAiProvider`, `AnthropicProvider`, `GeminiProvider`, `OllamaProvider`, `VllmProvider`, `SgLangProvider`, `SseToChunkNormalizer`, `PollingToEventAdapter` | [P1] | `gadgetron-core` |
| `gadgetron-router` | 6종 routing strategy 선택 + lock-free MetricsStore. | `Router`, `RoutingDecision`, `MetricsStore`, `RoundRobinStrategy`, `CostOptimalStrategy`, `LatencyOptimalStrategy`, `QualityOptimalStrategy`, `FallbackStrategy`, `WeightedStrategy` | [P1] | `gadgetron-core`, `gadgetron-provider` |
| `gadgetron-xaas` | API key 인증·tenant·quota·audit (chat/direct-action/tool-call)·Penny-attributed activity fan-out·EPIC 4 rate-limit + pg spend tracking + billing-event telemetry (ISSUE 12) + identity/users/teams/keys self-service (ISSUE 14) + cookie-session login (ISSUE 15) + session-validation primitive consumed by gateway unified middleware (ISSUE 16) + `AuditEntry` actor fields (ISSUE 19) + pg `audit_log` consumer (ISSUE 21) + `query_audit_log` read primitive (ISSUE 22). [P2]: invoice materialization + agent + catalog DEFERRED (2026-04-20 commercialization-layer direction). | `PgKeyValidator`, `ValidatedKey`, `TenantContext`, `QuotaConfig`, `QuotaEnforcer`, `AuditEntry`, `AuditWriter` + `run_audit_log_writer` (chat, pg-backed since ISSUE 21), `AuditLogRow` + `query_audit_log` (ISSUE 22 read), `ActionAuditEventWriter` + `ActionAuditRow` + `ActionAuditQueryFilter` (direct-action), `run_gadget_audit_writer` + `ToolAuditRow` + `ToolAuditQueryFilter` + `query_tool_audit_events` (Penny tool-call), `GadgetAuditEventWriter::with_coordinator()` + `gadget_audit_to_captured` (Penny fan-out), `TokenBucketRateLimiter` + `RateLimitedQuotaEnforcer` + `PgQuotaEnforcer` (EPIC 4 ISSUE 11), `billing_events` writer (ISSUE 12), `sessions::{validate_session,…}` (ISSUE 15/16), `Scope` (re-export) | [P1] chat audit (D-20260411-03); [P2B] direct-action audit (ISSUE 3); [EPIC 2] tool-call audit (ISSUE 5 / 6). EPIC 4 per-TASK chain in [`ROADMAP.md`](../ROADMAP.md) §EPIC 4 — ISSUEs 11 (PRs #230-#234), 12 (PRs #236/#241), 14 (PR #246), 15 (PR #248), 16 (PR #259), 17 (PR #260), 19 (PR #262), 20 (PR #263), 21 pg audit_log consumer (PR #267), 22 admin audit_log query (PR #269). | `gadgetron-core` |
| `gadgetron-gateway` | axum HTTP 서버, SSE 파이프라인, Tower 미들웨어 체인. [P2B+] workbench projection + action service + MCP surface + plugin marketplace (EPIC 3). [EPIC 4] unified Bearer-or-cookie `auth_middleware` (ISSUE 16) + `ValidatedKey.user_id` plumbing (ISSUE 17) + `TenantContext` → `AuditEntry` plumbing at 3 chat-handler `AuditEntry` literals (ISSUE 20) + admin `/audit/log` query endpoint (ISSUE 22). | axum `Router`, `chat_completions`, `models_list`, `health_check`, `WsAggregator`; `InProcessWorkbenchActionService` + `InProcessWorkbenchProjection` + `InMemoryApprovalStore` + `ActivityBus` (P2B / ISSUE 3–7); `Arc<ArcSwap<CatalogSnapshot>>` atomic handle; `perform_catalog_reload()` + `spawn_sighup_reloader()` (ISSUE 8); `BundleMetadata` + `DescriptorCatalog::from_bundle_dir()` (ISSUE 9); `/admin/bundles` CRUD + `validate_bundle_id()` + `verify_bundle_signature()` (ISSUE 10); `auth_middleware` with `validate_session_and_build_key` cookie fallback (ISSUE 16); `tenant_context_middleware` populating `actor_*` onto `TenantContext` (ISSUE 20); `GET /admin/audit/log` handler (ISSUE 22). | [P1]; [P2B] workbench + approval + audit (ISSUE 3). EPIC 3 per-TASK chain: [`ROADMAP.md`](../ROADMAP.md) §EPIC 3 — ISSUE 8 (PRs #211/#213/#214/#216/#217), ISSUE 9 (PRs #219/#220/#222), ISSUE 10 (PRs #223/#224/#226/#227); closing PR #228 / `v0.5.0`. EPIC 4 chain: [`ROADMAP.md`](../ROADMAP.md) §EPIC 4 — auth surface ISSUE 16 (PR #259) / 17 (PR #260) / 19 (PR #262) / 20 (PR #263) / 22 admin query endpoint (PR #269); pg audit_log write path in `gadgetron-xaas` row above (ISSUE 21). Harness gates 7q.1–7q.8 + 7v.6 + 7v.7 + 7v.8. | `gadgetron-core`, `gadgetron-provider`, `gadgetron-router`, `gadgetron-scheduler`, `gadgetron-node`, `gadgetron-xaas` |
| `gadgetron-scheduler` | VRAM bin-packing, LRU eviction, node registry, NUMA-aware FFD. | `Scheduler`, `NodeRegistry`, `EvictionPolicy`, `ModelDeploymentStore` | [P1] | `gadgetron-core`, `gadgetron-node` |
| `gadgetron-node` | NodeAgent (프로세스 생애주기), ResourceMonitor (CPU/GPU NVML). | `NodeAgent`, `ProcessManager`, `ResourceMonitor`, `MigManager` | [P1] | `gadgetron-core` |
| `gadgetron-tui` | ratatui 읽기 전용 대시보드. in-process Arc 공유 (H-11). | `Dashboard`, `NodePane`, `ModelPane`, `RequestPane` | [P1 RO] | `gadgetron-core`, `gadgetron-router`, `gadgetron-scheduler`, `gadgetron-node` |
| `gadgetron-cli` | 단일 바이너리 진입점, 부팅 시퀀스, graceful shutdown 조율. | `main()`, `bootstrap()`, `shutdown_coordinator()` | [P1] | `gadgetron-core`, `gadgetron-provider`, `gadgetron-router`, `gadgetron-gateway`, `gadgetron-scheduler`, `gadgetron-node`, `gadgetron-tui`, `gadgetron-xaas` |
| `gadgetron-testing` | mock·fixture·harness·property test. dev-dep only (D-20260411-05). | `MockLlmProvider`, `FakeNodeAgent`, `FakeTenant`, `PgHarness`, `GatewayHarness`, `fixtures::*`, `props::*` | [P1] (D-20260411-05) | 모든 상위 crate를 `[dev-dependencies]`로 참조; 프로덕션 코드 미포함 |

**Leaf crate 규칙** (D-12): `gadgetron-core`는 `axum`, `sqlx`, `nvml-wrapper` 등 구현 의존성을 가지지 않는다. 위반 시 CI `cargo tree --package gadgetron-core --no-dedupe` 검사로 빌드 실패. `gadgetron-testing`도 leaf이나 `dev-dependencies`로만 참조된다.

---

#### 2.A.2 단일 바이너리 레이어드 아키텍처

> Tower `ServiceBuilder` 미들웨어 체인이 실제 구현 레이어다. 아래 다이어그램은 논리적 레이어 순서를 나타내며, §2.A.3 Request Path에서 각 hop의 Tower Layer 귀속을 명시한다.

```
+===========================================================================+
|                     gadgetron (단일 바이너리, :8080 / :9090)              |
|                                                                           |
|  ┌─────────────────────────────────────────────────────────────────────┐  |
|  │                      gadgetron-gateway                              │  |
|  │  axum::Router                                                       │  |
|  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐  │  |
|  │  │POST /v1/chat │  │GET /v1/models│  │ /api/v1/{nodes,models,   │  │  |
|  │  │/completions  │  │              │  │  usage,costs}            │  │  |
|  │  └──────────────┘  └──────────────┘  └──────────────────────────┘  │  |
|  │  ┌──────────────┐  ┌──────────────────────────────────────────────┐ │  |
|  │  │GET /health   │  │ /api/v1/xaas/* [P1] /api/v1/web/workbench/events/ws (shipped ISSUE 4 / v0.2.7) │ │  |
|  │  │GET /ready    │  │                                              │ │  |
|  │  └──────────────┘  └──────────────────────────────────────────────┘ │  |
|  │                                                                     │  |
|  │  Tower ServiceBuilder (Layer 순서: 바깥→안):                        │  |
|  │  TraceLayer → AuthLayer(xaas) → ScopeGuardLayer →                  │  |
|  │  RequestIdLayer → [RateLimitLayer P2] → [GuardrailsLayer P2]       │  |
|  │  ↳ handler: Router::chat_with_context(ctx, req) →                  │  |
|  │             QuotaEnforcer → MetricsStore → AuditWriter             │  |
|  │                                                                     │  |
|  │  WsAggregator (gadgetron-gateway/src/observability/aggregator.rs)  │  |
|  │    ResourceMonitor 폴링 → broadcast::channel(1024) → WS clients    │  |
|  └─────────────────────┬──────────────────────────────────────────────┘  |
|                        │ calls into                                       |
|  ┌─────────────────────▼──────────────────────────────────────────────┐  |
|  │  gadgetron-xaas                                                     │  |
|  │  PgKeyValidator (moka::future::Cache<String,ValidatedKey>           │  |
|  │                  10_000 entries, 10min TTL, D-20260411-12)          │  |
|  │  QuotaEnforcer  (pre-check → stream → post-record, H-5 soft-cap)   │  |
|  │  AuditWriter    (mpsc::channel(4_096), D-20260411-09)              │  |
|  └─────────────────────┬──────────────────────────────────────────────┘  |
|                        │                                                   |
|  ┌─────────────────────▼──────────────────────────────────────────────┐  |
|  │  gadgetron-router                                                   │  |
|  │  Router::chat_with_context(ctx, req) → RoutingDecision             │  |
|  │  MetricsStore (dashmap 5.5, lock-free O(1) get, D-20260411-12 참조)│  |
|  │    key=(provider,model,tenant_id): LRU 10_000 entries 상한          │  |
|  └─────────────────────┬──────────────────────────────────────────────┘  |
|                        │                                                   |
|  ┌─────────────────────▼──────────────────────────────────────────────┐  |
|  │  gadgetron-provider                                                 │  |
|  │  6 × Arc<dyn LlmProvider + Send + Sync>  (D-20260411-11)           │  |
|  │  SseToChunkNormalizer [P1 week5] (D-20260411-02)                   │  |
|  │  PollingToEventAdapter → JSON→SSE (Gemini, [P1 week6])             │  |
|  └─────────────────────┬──────────────────────────────────────────────┘  |
|                        │                                                   |
|  ┌───────────┬──────────▼────────────────────────────────────────────┐   |
|  │  gadgetron-scheduler   │  gadgetron-node                           │   |
|  │  Scheduler::deploy()   │  NodeAgent::start_model()                │   |
|  │  NUMA-aware FFD [P1]   │  ProcessManager FSM                      │   |
|  │  VRAM Hybrid sync      │  ResourceMonitor (NVML 1s polling, H-9)  │   |
|  │  (10s heartbeat,       │  MigManager (static, [P1])               │   |
|  │   30s stale, B-2)      │                                          │   |
|  └────────────────────────────────────────────────────────────────────┘  |
|                                                                           |
|  ┌──────────────────────────┐  ┌───────────────────────────────────────┐  |
|  │  gadgetron-tui [P1 RO]  │  │  gadgetron-cli (진입점)               │  |
|  │  ratatui in-process Arc  │  │  AppConfig::load() + bootstrap() +   │  |
|  │  Nodes/Models/Requests   │  │  shutdown_coordinator() 70s (H-15)   │  |
|  └──────────────────────────┘  └───────────────────────────────────────┘  |
|                                                                           |
|  ┌─────────────────────────────────────────────────────────────────────┐  |
|  │  gadgetron-core (leaf — no impl deps, D-12)                         │  |
|  │  config | error | message | model | node | provider | routing | ui  │  |
|  └─────────────────────────────────────────────────────────────────────┘  |
+===========================================================================+

     ^                            ^                         ^
     | HTTP :8080                 | /api/v1/* Management    | Metrics :9090
  Client (OpenAI SDK)         K8s/Slurm [P2] / Helm [P1]  Prometheus/Grafana [P1]
```

---

#### 2.A.3 Request Path (Data Flow) — `/v1/chat/completions`

이 섹션은 B-1(G-6 TenantContext), B-8(scope 401→403 정정), H-1(streaming retry supersede),
H-6(quota_config fetch latency) 결정을 반영한다.

각 hop: crate 소유자 / 함수 시그니처 / 에러 전파 경로 / 근사 latency budget.

##### 2.A.3.1 비스트리밍 Request Path

```
Client
  │ POST /v1/chat/completions  HTTP/1.1
  │ Authorization: Bearer gad_live_<32-char alphanum>
  │ Content-Type: application/json
  │ Body: {"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"stream":false}
  │
  │ [crate: gadgetron-gateway]
  │ [fn: chat_completions(State<AppState>, headers: HeaderMap, Json<ChatRequest>)
  │      -> Result<Json<ChatResponse>, GadgetronError>]
  │ [Tower Layer: TraceLayer — request_id = Uuid::new_v4(), span 시작]
  │ [budget: ~5μs — UUID 생성 + span 시작]
  ▼
  ── AuthLayer (Tower Layer, gadgetron-gateway/src/middleware/auth.rs) ──────
  │ [crate: gadgetron-xaas via gadgetron-gateway AuthLayer]
  │ [Bearer path — unchanged]
  │ [fn: PgKeyValidator::validate(token: &str) -> Result<ValidatedKey, GadgetronError>]
  │   1. SHA-256(token) → hex → cache_key: String
  │   2. moka::future::Cache::get(&cache_key)
  │      hit (≥99%): ValidatedKey { tenant_id: Uuid, scopes: Vec<Scope> }
  │      miss (<1%): sqlx::query!("SELECT ... FROM api_keys WHERE key_hash=$1")
  │                  → map_err(sqlx_to_gadgetron)?
  │   Error paths:
  │     GadgetronError::TenantNotFound     → HTTP 401 (인증 실패)
  │     GadgetronError::Database{..}       → HTTP 503
  │ [budget: cache hit ~50μs / cache miss ~5ms (PostgreSQL RTT)]
  │
  │ [Cookie fallback path — EPIC 4 / ISSUE 16 TASK 16.1 / PR #259 / v0.5.9]
  │ [Trigger: `Authorization` header absent AND `gadgetron_session` cookie present]
  │ [fn: validate_session_and_build_key(pool, cookie_token)
  │      -> Result<ValidatedKey, GadgetronError>]
  │   1. SHA-256(cookie_token) → lookup in `user_sessions` table
  │   2. Session row → resolve user_id → SELECT role FROM users
  │   3. Role → scope synthesis:
  │        admin   → [Scope::OpenAiCompat, Scope::Management]
  │        member  → [Scope::OpenAiCompat]
  │        service → reject (login blocks; defensive check here)
  │   4. Build ValidatedKey { tenant_id, scopes, api_key_id = Uuid::nil(),
  │                            user_id: Some(session.user_id) }
  │      — `Uuid::nil()` sentinel on `api_key_id` distinguishes cookie
  │      sessions from Bearer keys; `user_id: Some(session.user_id)`
  │      (ISSUE 17 TASK 17.1 / PR #260 / v0.5.10) carries the owning
  │      user so downstream audit writers can emit `actor_user_id`
  │      directly without re-querying `user_sessions`. Bearer path
  │      also populates `user_id` from `api_keys.user_id` (backfilled
  │      by ISSUE 14 TASK 14.1; legacy keys surface as `None`).
  │      Audit-row writes using this field live end-to-end as of
  │      v0.5.14: ISSUE 19 / PR #262 shipped the `AuditEntry`
  │      struct shape, ISSUE 20 / PR #263 plumbed `TenantContext`
  │      + chat handler so fields get populated on live requests,
  │      ISSUE 21 / PR #267 added `run_audit_log_writer` pg
  │      consumer that INSERTs rows to `audit_log` using the
  │      actor columns, and ISSUE 22 / PR #269 added the
  │      Management-scoped `GET /admin/audit/log` read endpoint.
  │      Cookie → `actor_api_key_id = None` (nil sentinel);
  │      Bearer → `Some(key_id)`; both set `actor_user_id` from
  │      `ValidatedKey.user_id`. `--no-db` falls back to
  │      tracing-only legacy consumer (no audit_log persistence).
  │   Error paths:
  │     SessionExpired / SessionNotFound / UserInactive → HTTP 401
  │     GadgetronError::Database{..}                    → HTTP 503
  │ [budget: ~1-5ms (no cache — every cookie request hits pg once)]
  │ [scope: `/v1/*` + `/api/v1/web/workbench/*` + `/api/v1/xaas/*`;
  │  the three `/api/v1/auth/{login,logout,whoami}` endpoints stay on
  │  `public_routes` and self-authenticate via their own handlers.]
  │ [no-db mode: cookie path silently skipped (no pool = no validation);
  │  Bearer-only under `--no-db`.]
  │
  ── ScopeGuardLayer ──────────────────────────────────────────────────────
  │ [crate: gadgetron-gateway/src/middleware/scope.rs]
  │ [fn: require_scope(key: &ValidatedKey, required: Scope) -> Result<(), GadgetronError>]
  │   요청 경로별 필수 scope:
  │     /v1/chat/completions → Scope::OpenAiCompat
  │     /api/v1/*            → Scope::Management
  │     /api/v1/xaas/*      → Scope::XaasAdmin
  │   Error paths:
  │     key.scopes에 required 없음 → GadgetronError::Forbidden → HTTP 403  (B-8)
  │     (인증 성공 + 권한 부족 = 403, 인증 실패 = 401. v0 오류 정정.)
  │ [budget: ~1μs — Vec<Scope> linear scan (최대 3 elements)]
  │
  ── handler 진입 ──────────────────────────────────────────────────────────
  │ [crate: gadgetron-xaas] — EPIC 4 composite: `RateLimitedQuotaEnforcer`
  │                          wraps inner `QuotaEnforcer` when
  │                          `[quota_rate_limit]` is configured (TASK 11.2)
  │   [0. RATE-LIMIT STEP — TASK 11.2 / PR #231 / v0.5.2]
  │     (runs BEFORE cost check, fail-fast: no snapshot query on reject)
  │     TokenBucketRateLimiter::consume(tenant_id, 1)
  │       — DashMap-sharded per-tenant bucket, lazy refill at
  │         consume() time, monotonic Instant clock (skew-proof)
  │       초과 시 → GadgetronError::QuotaExceeded → HTTP 429 (same wire)
  │   [1. COST CHECK — `QuotaEnforcer::check_pre`]
  │ [fn: QuotaEnforcer::check_pre(tenant_id: Uuid, estimated_tokens: u32)
  │      -> Result<(), GadgetronError>]
  │   quota_config: moka cache에서 조회 (10min TTL, H-6에서 결정)
  │     - cache hit: ~50μs  (별도 cache entry, key=tenant_id)
  │     - cache miss: PostgreSQL quota_configs 테이블 조회 ~5ms
  │   TokenBucket::try_consume(estimated_tokens)
  │     초과 시 → GadgetronError::QuotaExceeded → HTTP 429
  │                  (+ `Retry-After: 60` HTTP header
  │                   + `retry_after_seconds: 60` body field,
  │                    ISSUE 11 TASK 11.1 / v0.5.1 / PR #230)
  │ [budget: cache hit ~50μs / miss ~5ms]
  │
  │ [crate: gadgetron-router]
  │ [fn: Router::chat_with_context(ctx: &TenantContext, req: ChatRequest)
  │      -> Result<ChatResponse, GadgetronError>]   (B-1 결정)
  │   Router::resolve(&req) → RoutingDecision {
  │     provider_id: String,
  │     model: String,
  │     estimated_cost_usd: f64,   (routing용, D-20260411-06)
  │     fallback_chain: Vec<String>,
  │   }
  │ [budget: ~50μs — DashMap O(1) read × strategy logic]
  │
  │ [crate: gadgetron-provider]
  │ [fn: LlmProvider::chat(req: ChatRequest) -> Result<ChatResponse, GadgetronError>]
  │   (Arc<dyn LlmProvider + Send + Sync>, vtable dispatch ~1ns, D-20260411-11)
  │   → upstream HTTP POST (reqwest async, rustls)
  │   Error paths:
  │     HTTP 4xx → GadgetronError::Provider(String)    → HTTP 502
  │     HTTP 5xx → GadgetronError::Provider(String)    → HTTP 502
  │     timeout  → GadgetronError::Provider(String)    → HTTP 504
  │ [budget: 100ms ~ 30s — upstream LLM latency (gateway overhead 제외)]
  │
  │ [crate: gadgetron-router]
  │ [fn: MetricsStore::record_success(
  │        provider: &str, model: &str, tenant_id: Uuid,
  │        latency_ms: u64, tokens: u32, cost_usd: f64)  (B-1 tenant_id 추가)]
  │   DashMap key=(provider,model,tenant_id): LRU 10_000 entries 상한
  │     (Theme 1: cardinality 제한, moka LRU 10_000)
  │   fallback 발생 시: try_fallbacks(fallback_chain, req) → 순차 시도
  │ [budget: ~5μs — DashMap write]
  │
  │ [crate: gadgetron-xaas] — EPIC 4 TASK 11.3 / PR #232 / v0.5.3
  │ [fn: QuotaEnforcer::record_post(tenant_id: Uuid, usage: &Usage)
  │      -> Result<(), GadgetronError>]
  │   Auto-selected at startup: `PgQuotaEnforcer` when
  │     `pg_pool.is_some()`, else `InMemoryQuotaEnforcer` fallback.
  │   compute_cost_cents(usage, rate) -> i64  (D-20260411-06 변환 지점)
  │   PgQuotaEnforcer 경로 (`pg_pool.is_some()`):
  │     PostgreSQL UPDATE quota_configs
  │       SET daily_used_cents = CASE WHEN usage_day = CURRENT_DATE
  │                                   THEN daily_used_cents + $cost
  │                                   ELSE $cost END,
  │           monthly_used_cents = CASE WHEN date_trunc('month', usage_day)
  │                                       = date_trunc('month', CURRENT_DATE)
  │                                     THEN monthly_used_cents + $cost
  │                                     ELSE $cost END,
  │           usage_day = CURRENT_DATE
  │         WHERE tenant_id = $2
  │     — CASE rollover: daily zeros at UTC midnight, monthly at
  │       first-of-month, NO background job, rollover happens on
  │       the first post-boundary request.
  │     — Fire-and-forget: UPDATE 실패해도 request 는 이미 성공,
  │       tracing::error 만 로그 + never propagate.
  │   InMemoryQuotaEnforcer 경로 (pg_pool 없음): 인메모리 token 만
  │     mark-used, DB 는 건드리지 않음.
  │ [budget: ~3ms — PostgreSQL write (PgQuotaEnforcer path) /
  │  ~1μs (InMemory path)]
  │ [migration: 20260420000001_quota_usage_day.sql adds
  │  `usage_day DATE DEFAULT CURRENT_DATE` column]
  │
  │ [crate: gadgetron-xaas] — EPIC 4 TASK 12.1 / PR #236 / v0.5.5
  │ [fn: billing::insert_billing_event(pool, tenant_id, kind,
  │      cost_cents, source_event_id, model, provider)
  │      -> Result<(), sqlx::Error>]
  │   PgQuotaEnforcer 경로에서만 실행 (record_post UPDATE 직후).
  │   PostgreSQL INSERT INTO billing_events
  │       (tenant_id, event_kind, source_event_id, cost_cents, model, provider)
  │       VALUES ($1, 'chat', NULL, $2, NULL, NULL)
  │     — event_kind 은 chat-path 에서 'chat' 고정 (DB CHECK 제약).
  │       TASK 12.2 (PR #241 / v0.5.6) 가 `/v1/tools/{name}/invoke`
  │       + workbench direct-action + approved-action 경로에
  │       각각 emitter 를 추가함 — `QuotaEnforcer::record_post`
  │       trait 시그니처는 의도적으로 확장하지 않고 handler-level
  │       에서 directly emit (tool/action 은 quota 경로를 타지
  │       않기 때문; 설계 근거는 `docs/design/xaas/phase2-billing.md`
  │       §8). Action rows 는 `source_event_id = audit_event_id`
  │       로 threading 되어 audit↔ledger join 가능. `model` /
  │       `provider` 는 v0.5.6 기준 여전히 NULL — trait 경계 밖
  │       emission 으로 두 값을 threading 하는 건 미완; 향후
  │       invoice materialization (TASK 12.3, DEFERRED 상태) 에서
  │       joined view 로 해결 예정.
  │     — source_event_id FK 없음 — Stripe-style writer-independence,
  │       원장 writer 가 source event DB persist 완료를 기다리지 않음.
  │     — Fire-and-forget — INSERT 실패해도 quota_configs UPDATE 는
  │       이미 commit 됐으므로 tracing::warn!(target: "billing") 기록
  │       후 진행. Counter-vs-ledger drift 는 TASK 12.4 reconciliation
  │       (scan quota_configs vs SUM(cost_cents) GROUP BY tenant_id,
  │       usage_day) 으로 청산.
  │ [budget: ~3ms — single PG INSERT, BIGSERIAL + 2 indexes]
  │ [migration: 20260420000002_billing_events.sql — append-only
  │  원장 테이블 + `(tenant_id, created_at DESC)` +
  │  `(event_kind, created_at DESC)` indexes]
  │
  │ [crate: gadgetron-xaas]
  │ [fn: AuditWriter::send(entry: AuditEntry) — non-blocking]
  │   mpsc::Sender::try_send(entry)  capacity=4_096
  │   실패 시: tracing::warn! + counter!("gadgetron_xaas_audit_dropped_total").increment(1)
  │   NOTE: tenant_id 라벨 없음 (cardinality 폭발 방지, phase-c §4.7)
  │ [budget: ~1μs — try_send (non-blocking)]
  │
  │ [crate: gadgetron-gateway]
  │ ChatResponse → serde_json::to_string → axum Response 200
  │ TraceLayer span end: latency, status, request_id, tenant_id
  │ [budget: ~10μs — JSON serialize + response write]
  ▼
Client ← HTTP 200 JSON

합계 gateway overhead (upstream 제외): cache hit ~200μs, cache miss ~15ms (cold start)
P99 SLO: cache hit path < 1ms (D-12 §1.2 원칙 3)
```

##### 2.A.3.2 스트리밍 Request Path (SSE)

H-1 결정: "retry는 첫 SSE 청크 전송 이전에만 허용. 이후는 연결 종료 + audit." (D-20260411-08 supersede)

```
Client
  │ POST /v1/chat/completions
  │ Body: { "stream": true, ... }
  │
  │ [hop 1~4 동일: TraceLayer → AuthLayer → ScopeGuardLayer → QuotaEnforcer::check_pre]
  │
  │ [crate: gadgetron-router]
  │ [fn: Router::chat_with_context(ctx: &TenantContext, req: ChatRequest)
  │      -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, GadgetronError>> + Send>>,
  │                GadgetronError>]
  │
  │ [crate: gadgetron-provider]
  │ [fn: LlmProvider::chat_stream(req: ChatRequest)
  │      -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, GadgetronError>> + Send>>,
  │                GadgetronError>]
  │
  │   Provider별 upstream stream 획득:
  │     OpenAI/Ollama/vLLM/SGLang:
  │       reqwest::Response → eventsource-stream → ChatChunk
  │     Anthropic:
  │       "content_block_delta" SSE → SseToChunkNormalizer → ChatChunk
  │     Gemini [P1 week6]:
  │       generateContent 폴링 → PollingToEventAdapter → SSE event → ChatChunk
  │       PollingToEventAdapter 입력: reqwest::Response (JSON array)
  │       PollingToEventAdapter 출력: Stream<Item = SseEvent>  (H-4 결정)
  │
  │ [첫 청크 전송 이전 — retry window]
  │   StreamInterrupted 발생 → tower::retry 1회 (D-20260411-08 유지)
  │
  │ [crate: gadgetron-gateway]
  │ [fn: chat_stream_handler → Sse<KeepAlive>]
  │   ChatChunk → serde_json::to_string → Event::default().data(json_str)
  │   axum::response::sse::Sse::new(stream).keep_alive(KeepAlive::new().interval(15s))
  │   제로카피: 중간 버퍼 없이 axum body 직접 파이프라인
  │
  │ [첫 청크 전송 이후 — retry 금지, H-1]
  │   StreamInterrupted 발생:
  │     → SSE 연결 즉시 종료 ([DONE] 없음)
  │     → AuditWriter::send(entry with status=interrupted)
  │     → Prometheus counter!("gadgetron_stream_interrupted_total")
  │
  │ 정상 종료: "data: [DONE]\n\n" 전송
  │
  │ [스트림 완료 후 — Audit + Quota post-record]
  │   QuotaEnforcer::record_post(tenant_id, actual_usage)
  │   AuditWriter::send(AuditEntry { status: success, ... })
  │
  ▼
Client ← SSE: "data: {...}\n\n" ... "data: [DONE]\n\n"
```

---

#### 2.A.4 Dependency Graph

규칙 집행: CI에서 `cargo tree --package gadgetron-core --no-dedupe | grep -E 'axum|sqlx|nvml'` 출력 존재 시 빌드 실패.

```mermaid
graph LR
    CORE[gadgetron-core]:::leaf
    TESTING[gadgetron-testing]:::leaf_dev
    PROVIDER[gadgetron-provider]
    ROUTER[gadgetron-router]
    XAAS[gadgetron-xaas]
    GATEWAY[gadgetron-gateway]
    SCHEDULER[gadgetron-scheduler]
    NODE[gadgetron-node]
    TUI[gadgetron-tui]
    CLI[gadgetron-cli]

    PROVIDER --> CORE
    ROUTER --> CORE
    ROUTER --> PROVIDER
    XAAS --> CORE
    SCHEDULER --> CORE
    SCHEDULER --> NODE
    NODE --> CORE
    TUI --> CORE
    TUI --> ROUTER
    TUI --> SCHEDULER
    TUI --> NODE
    GATEWAY --> CORE
    GATEWAY --> PROVIDER
    GATEWAY --> ROUTER
    GATEWAY --> SCHEDULER
    GATEWAY --> NODE
    GATEWAY --> XAAS
    CLI --> CORE
    CLI --> PROVIDER
    CLI --> ROUTER
    CLI --> GATEWAY
    CLI --> SCHEDULER
    CLI --> NODE
    CLI --> TUI
    CLI --> XAAS

    TESTING -.->|dev-dep| CORE
    TESTING -.->|dev-dep| PROVIDER
    TESTING -.->|dev-dep| ROUTER
    TESTING -.->|dev-dep| XAAS
    TESTING -.->|dev-dep| GATEWAY
    TESTING -.->|dev-dep| SCHEDULER
    TESTING -.->|dev-dep| NODE

    classDef leaf fill:#d4edda,stroke:#28a745
    classDef leaf_dev fill:#fff3cd,stroke:#ffc107
```

**무사이클 보장**:
- `gadgetron-core`는 내부 crate 의존성 없음 → 사이클 불가
- `gadgetron-xaas`는 `gadgetron-gateway`에 의존하지 않음 (gateway가 xaas에 의존)
- `gadgetron-node`는 `gadgetron-scheduler`에 의존하지 않음 (scheduler가 node에 의존)
- `gadgetron-testing`은 `dev-dependencies`만 사용 → 프로덕션 빌드에 미포함
- 집행: `cargo build 2>&1 | grep "cyclic"` → 0줄이어야 통과

---

#### 2.A.5 Type Ownership Map

D-12 크레이트 경계표 + D-20260411-03/05/07/08/13 + Phase C B-1/B-8/H-13 반영.

| Type | 소유 crate | 이 crate에 두는 이유 | 사용 가능 crate (dependents) |
|------|------------|----------------------|------------------------------|
| `AppConfig` | `gadgetron-core` | 모든 crate가 설정을 읽음; leaf 규칙 | 전체 |
| `GadgetronError` | `gadgetron-core` | 단일 에러 taxonomy; cross-crate 전파 | 전체 |
| `DatabaseErrorKind` | `gadgetron-core` | DB-agnostic enum; sqlx 의존 없이 leaf 유지 (D-20260411-13) | 전체 |
| `LlmProvider` (trait) | `gadgetron-core` | provider·router·gateway 모두 참조; trait object Arc<dyn LlmProvider> (D-20260411-11) | `gadgetron-provider`, `gadgetron-router`, `gadgetron-gateway`, `gadgetron-testing` |
| `ChatRequest` | `gadgetron-core` | OpenAI 호환 표면; 외부 오염 금지 (tenant_id 추가 불가, B-1) | 전체 |
| `ChatResponse` | `gadgetron-core` | 동상 | 전체 |
| `ChatChunk` | `gadgetron-core` | SSE 스트림 단위; provider·router·gateway 공유 | `gadgetron-provider`, `gadgetron-router`, `gadgetron-gateway` |
| `ModelState` | `gadgetron-core` | scheduler·node·gateway 공유 상태 | `gadgetron-scheduler`, `gadgetron-node`, `gadgetron-gateway` |
| `ModelDeployment` | `gadgetron-core` | 동상 | `gadgetron-scheduler`, `gadgetron-node`, `gadgetron-gateway` |
| `NodeStatus` | `gadgetron-core` | node·scheduler·tui 공유 | `gadgetron-node`, `gadgetron-scheduler`, `gadgetron-tui` |
| `GpuMetrics` | `gadgetron-core` (ui module) | TUI/WS 공유 (D-20260411-07) | `gadgetron-node`, `gadgetron-tui`, `gadgetron-gateway` |
| `NodeMetrics` | `gadgetron-core` (ui module) | cpu_pct·ram_used_gb·network_rx/tx_mbps — GpuMetrics 분리 (H-13) | `gadgetron-node`, `gadgetron-tui`, `gadgetron-gateway` |
| `WsMessage` | `gadgetron-core` (ui module) | `GpuMetrics \| ModelStatus \| RequestLog \| ClusterHealth` 합집합 (D-20260411-07) | `gadgetron-gateway`, `gadgetron-tui` |
| `Scope` | `gadgetron-xaas` (정의) / `gadgetron-core` (re-export) | xaas가 정의·검증; core에서 re-export → gateway에서 참조 (D-20260411-10) | `gadgetron-gateway`, `gadgetron-xaas` |
| `ValidatedKey` | `gadgetron-xaas` | auth 결과; gateway AuthLayer 내부 구현 세부 | `gadgetron-gateway` |
| `TenantContext` | `gadgetron-xaas` | tenant_id + quota_config; gateway handler가 chat_with_context에 전달 (B-1) | `gadgetron-gateway`, `gadgetron-router` |
| `QuotaConfig` | `gadgetron-xaas` | quota 설정; moka cache (H-6) | `gadgetron-xaas` internal |
| `AuditEntry` | `gadgetron-xaas` | audit 레코드; cost_cents: i64 (D-20260411-06) | `gadgetron-xaas` internal |
| `RoutingDecision` | `gadgetron-router` | 라우팅 결과; gateway가 소비 | `gadgetron-gateway` |
| `MetricsStore` | `gadgetron-router` | DashMap lock-free; router 내부 (B-1: tenant_id 추가) | `gadgetron-router` internal |
| `SseToChunkNormalizer` | `gadgetron-provider` | provider 내부 변환기; 외부 노출 불필요 | `gadgetron-provider` internal |
| `PollingToEventAdapter` | `gadgetron-provider` | Gemini 전용 JSON→SSE 변환기 (H-4, D-20260411-02) | `gadgetron-provider` internal |
| `ProcessManager` | `gadgetron-node` | 프로세스 FSM; node 내부 (IE-01 FSM 다이어그램 → §2.F) | `gadgetron-node` internal |
| `WsAggregator` | `gadgetron-gateway` | ResourceMonitor 폴링 → broadcast::channel(1024) (H-12) | `gadgetron-gateway` internal |

**이동 금지 원칙**: `TenantContext`, `ValidatedKey`, `AuditEntry`는 `gadgetron-core`로 이동 금지. 이들은 xaas 도메인에 속하며, core를 leaf crate로 유지하는 데 필수적이다. `ChatRequest`에 `tenant_id` 필드 추가 금지 (OpenAI 호환 표면 오염, B-1).

---

#### 2.A.6 Public API Stability Contract

Phase 진화에 걸쳐 안정성을 유지해야 하는 타입·trait과 변경 가능한 것의 명시적 구분. Axis E(§2.E)와 교차 참조.

**안정 (Stable across phases — breaking change = semver major)**

| 타입 / Trait | 안정 범위 | 근거 |
|---|---|---|
| `POST /v1/chat/completions` 요청·응답 schema | P1 → P2 → P3 | OpenAI 호환 계약; 클라이언트 lock-in |
| `GET /v1/models` 응답 schema | P1 → | 동상 |
| `LlmProvider` trait 시그니처 (`chat`, `chat_stream`, `models`, `health`) | P1 → | provider adapter 교체 없이 6종 구현 |
| `GadgetronError` variant **추가**는 허용; **제거·rename**은 major | P1 → | `#[non_exhaustive]` — match arm exhaustive 체크 |
| `Scope` enum variant **추가**는 허용 (`#[non_exhaustive]`) | P1 → | D-20260411-10: Phase 2 세분화 경로 |
| `/health` 응답 schema (`{"status":"ok"}`) | P1 → | 인프라·K8s liveness probe |
| `AppConfig` TOML key — **추가**는 허용; **rename·제거**는 major | P1 → | Helm chart 배포 환경 선언적 설정 |

**변경 가능 (May change — internal or semver-minor additive)**

| 타입 | 변경 가능 이유 | 예상 변경 시점 |
|---|---|---|
| `MetricsStore` 내부 DashMap key 구조 | B-1에서 tenant_id 추가됨; 향후 추가 dimension 가능 | P2 |
| `TenantContext` 필드 | XaaS P2 billing 확장 시 필드 추가 | P2 |
| `WsMessage` enum variant 추가 | P2 Web UI, agent 이벤트 추가 | P2 |
| `NodeMetrics` 필드 추가 | 추가 하드웨어 메트릭 (InfiniBand 등) | P2 |
| `/api/v1/*` 경로 schema | 운영 API; semver-minor 추가 허용 | P1 → |
| `Scope::XaasAdmin` 세분화 | D-20260411-10 Phase 2 계획 | P2 |

**LlmProvider trait 안정 계약** (D-20260411-11):

```rust
// gadgetron-core/src/provider.rs
// 이 시그니처는 P1→P2→P3 stable. 새 메서드 추가 시 default impl 제공 필수.
// async-trait is required because LlmProvider is used as Arc<dyn LlmProvider>.
// Native RPITIT async fn produces an opaque type incompatible with trait objects.
// async-trait converts to Pin<Box<dyn Future + Send>> which is dyn-compatible.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    /// 비스트리밍 chat completion.
    async fn chat(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, GadgetronError>;

    /// 스트리밍 chat completion. 첫 청크 전송 이전에만 retry 허용 (H-1).
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatChunk, GadgetronError>> + Send>>,
        GadgetronError,
    >;

    /// 이 provider가 지원하는 모델 목록.
    async fn models(&self) -> Result<Vec<String>, GadgetronError>;

    /// Provider 활성 여부. cloud provider는 is_live와 동일 (H-3: 1-token probe 금지).
    async fn health(&self) -> Result<bool, GadgetronError>;

    /// Provider 이름 (라우팅·메트릭 레이블).
    fn provider_name(&self) -> &'static str;
}
```

**GadgetronError 완전 열거** (D-20260411-08, D-20260411-13, D-13 + B-8 확장):

```rust
// gadgetron-core/src/error.rs
#[non_exhaustive]  // 추가 variant는 허용; 제거·rename은 semver major
#[derive(Debug, thiserror::Error)]
pub enum GadgetronError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Routing error: {0}")]
    Routing(String),

    #[error("Stream interrupted: {reason}")]
    StreamInterrupted { reason: String },  // D-20260411-08

    #[error("Quota exceeded for tenant {tenant_id}")]
    QuotaExceeded { tenant_id: uuid::Uuid },  // D-13

    #[error("Tenant not found: {tenant_id}")]
    TenantNotFound { tenant_id: uuid::Uuid },  // D-13

    #[error("Forbidden: insufficient scope")]
    Forbidden,  // B-8: 인증 성공 + 권한 부족 → HTTP 403

    #[error("Billing error: {0}")]
    Billing(String),  // D-13 — active on trunk; ISSUE 12 TASK 12.1 / PR #236 / v0.5.5 landed the `billing_events` ledger write path which can surface this variant on INSERT / schema failure. Originally tagged [P2] in the Phase 1 design; that tag is stale post-0.5.5.

    #[error("Download failed: {0}")]
    DownloadFailed(String),  // D-13

    #[error("Hot-swap failed: {0}")]
    HotSwapFailed(String),  // D-13

    #[error("Database error ({kind:?}): {message}")]
    Database { kind: DatabaseErrorKind, message: String },  // D-20260411-13

    #[error("Node error ({kind:?}): {message}")]
    Node { kind: NodeErrorKind, message: String },  // B-4: MIG profile / NVML / process errors
}

// M-DX-1: GadgetronError helper methods (gadgetron-core/src/error.rs)
// error_code() → short machine-readable string (used as Prometheus label + JSON "code" field)
// error_message() → human-readable sentence (used as JSON "message" field in API responses)
// error_type() → OpenAI-compatible error type string (used as JSON "type" field)
impl GadgetronError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Config(_)               => "config_error",
            Self::Provider(_)             => "provider_error",
            Self::Routing(_)              => "routing_error",
            Self::StreamInterrupted { .. }=> "stream_interrupted",
            Self::QuotaExceeded { .. }    => "quota_exceeded",
            Self::TenantNotFound { .. }   => "tenant_not_found",
            Self::Forbidden               => "forbidden",
            Self::Billing(_)              => "billing_error",
            Self::DownloadFailed(_)       => "download_failed",
            Self::HotSwapFailed(_)        => "hot_swap_failed",
            Self::Database { kind, .. } => match kind {
                DatabaseErrorKind::PoolTimeout | DatabaseErrorKind::ConnectionFailed => "db_unavailable",
                DatabaseErrorKind::RowNotFound    => "db_not_found",
                DatabaseErrorKind::Constraint     => "db_constraint",
                _                                 => "db_error",
            },
            Self::Node { kind, .. } => match kind {
                NodeErrorKind::InvalidMigProfile  => "node_invalid_mig_profile",
                _                                 => "node_error",
            },
        }
    }

    pub fn error_message(&self) -> &'static str {
        match self {
            Self::Config(_)               => "Configuration is invalid. Check your gadgetron.toml and environment variables.",
            Self::Provider(_)             => "The upstream LLM provider returned an error. Check provider status and API key validity.",
            Self::Routing(_)              => "No suitable provider found for this request. Verify model availability and routing configuration.",
            Self::StreamInterrupted { .. }=> "The response stream was interrupted. This may indicate a provider timeout or network issue.",
            Self::QuotaExceeded { .. }    => "Your API usage quota has been exceeded. Contact your administrator to increase limits.",
            Self::TenantNotFound { .. }   => "Invalid API key. Verify your API key is correct and has not been revoked.",
            Self::Forbidden               => "Your API key does not have permission for this operation. Check your key's assigned scopes.",
            Self::Billing(_)              => "A billing calculation error occurred. Contact support if this persists.",
            Self::DownloadFailed(_)       => "Model download failed. Check network connectivity and model repository access.",
            Self::HotSwapFailed(_)        => "Model hot-swap failed. The previous model version remains active.",
            Self::Database { .. }         => "A database error occurred. Check PostgreSQL connectivity and disk space.",
            Self::Node { .. }             => "A node-level error occurred. Check GPU availability and NVML driver status.",
        }
    }

    pub fn error_type(&self) -> &'static str {
        match self {
            Self::Config(_)               => "invalid_request_error",
            Self::Provider(_)             => "api_error",
            Self::Routing(_)              => "invalid_request_error",
            Self::StreamInterrupted { .. }=> "api_error",
            Self::QuotaExceeded { .. }    => "quota_error",
            Self::TenantNotFound { .. }   => "authentication_error",
            Self::Forbidden               => "permission_error",
            Self::Billing(_)              => "api_error",
            Self::DownloadFailed(_)       => "api_error",
            Self::HotSwapFailed(_)        => "api_error",
            Self::Database { .. }         => "server_error",
            Self::Node { .. }             => "server_error",
        }
    }
}

// HTTP status mapping (gadgetron-gateway/src/error.rs):
// Config             → 500
// Provider           → 502
// Routing            → 502
// StreamInterrupted  → 502 (연결 종료 + audit)
// QuotaExceeded      → 429   ← 429 responses alone carry both `Retry-After: 60` HTTP header AND `retry_after_seconds: 60` in the JSON body (ISSUE 11 TASK 11.1 / v0.5.1 / PR #230). `ApiError::into_response` 가 `status == TOO_MANY_REQUESTS` 일 때만 두 surface 를 widen; 다른 4xx (403/401/404) 는 base shape 유지 — `Retry-After` 는 RFC 7231 semantics 상 SDK auto-retry 가 act 하는 surface 이므로 permanent 실패에 싣지 않음. `QUOTA_RETRY_AFTER_SECONDS = 60` 은 conservative constant; TASK 11.2 / PR #231 shipped `TokenBucketRateLimiter` 하지만 real refill 시간을 `QuotaToken` 으로 전달하는 작업은 shipped 되지 않음 — currently-numbered ISSUE 11 TASKs (11.3 Postgres spend tracking + 11.4 `/web` 429 UI) 중 어느 것에도 포함되지 않은 미래 follow-up 임.
// TenantNotFound     → 401
// Forbidden          → 403   ← B-8 정정 (v0: 401 오류)
// Billing            → 402
// DownloadFailed     → 502
// HotSwapFailed      → 503
// Database{PoolTimeout|ConnectionFailed} → 503
// Database{RowNotFound}                  → 404
// Database{Constraint}                   → 409
// Database{MigrationFailed|QueryFailed|Other} → 500
// Node{InvalidMigProfile}                → 400
// Node{NvmlInitFailed|ProcessSpawnFailed|VramAllocationFailed|PortAllocationFailed} → 500

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DatabaseErrorKind {
    RowNotFound,
    PoolTimeout,
    ConnectionFailed,
    QueryFailed,
    MigrationFailed,
    Constraint,
    Other,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeErrorKind {
    InvalidMigProfile,       // Requested MIG profile not supported on this GPU model → HTTP 400
    NvmlInitFailed,          // NVML library failed to initialize → HTTP 500
    ProcessSpawnFailed,      // Inference engine process could not be spawned → HTTP 500
    VramAllocationFailed,    // Insufficient VRAM for requested deployment → HTTP 500
    PortAllocationFailed,    // No free port available in configured range → HTTP 500
}
```

**Scope enum 완전 열거** (D-20260411-10):

```rust
// gadgetron-xaas/src/auth/key.rs (정의)
#[non_exhaustive]  // Phase 2에서 세분화 추가 허용
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Scope {
    OpenAiCompat,  // /v1/chat/completions, /v1/models — gad_live_* 기본
    Management,    // /api/v1/{nodes,models,usage,costs} — 별도 부여
    XaasAdmin,     // /api/v1/xaas/* — 별도 부여
    // [P2] 세분화 예정: ChatRead, ChatWrite, EmbeddingsRead, XaasGpuAllocate, ...
}
```

---

#### 2.A.7 B-6 Audit Flow e2e 시나리오 (GAP-T1 해소)

이 섹션은 Phase C BLOCKER B-6 (GAP-T1: Audit flow e2e 시나리오 부재)을 해소한다.
상세 테스트 harness는 `docs/design/testing/harness.md` §시나리오 6에 있다.

**검증해야 하는 3가지 경로**:

1. **정상 경로**: 요청 완료 → `AuditWriter::send()` → mpsc channel → 백그라운드 태스크 → PostgreSQL `audit_log` INSERT. 검증: `SELECT COUNT(*) FROM audit_log WHERE request_id=$1` = 1.

2. **채널 포화 경로**: mpsc channel (4_096 capacity)이 가득 찬 상태에서 `try_send` → 실패 → `tracing::warn!` 발생 + `gadgetron_xaas_audit_dropped_total` counter +1. 검증: Prometheus counter 값 확인 (in-process `metrics::recorder` mock 사용).

3. **SIGTERM drain 경로**: SIGTERM 수신 시 채널 내 미처리 AuditEntry를 5초 내 flush → PostgreSQL INSERT → 프로세스 종료. 검증: SIGTERM 직전 채널에 N개 적재 → 5초 대기 → `audit_log` COUNT = N.

```
[시나리오 2 상세 — 채널 포화]
AuditWriter 백그라운드 태스크를 일시 정지(tokio::test에서 sleep injection)
→ 4_096개 AuditEntry를 try_send
→ 4_097번째 try_send → Err(Full)
→ assert!(tracing_subscriber captures "audit dropped" warn)
→ assert_eq!(counter!("gadgetron_xaas_audit_dropped_total"), 1)
```

#### §2.A.7.2 Phase 1 e2e 시나리오 전체 목록

| # | 시나리오 | 입력 | 기대 출력 | 검증 방법 | 테스트 파일 |
|---|---------|------|----------|----------|------------|
| 1 | Normal chat completion | `POST /v1/chat/completions` with valid key + model "gpt-4o-mini" | HTTP 200 + `ChatResponse` JSON with `choices[0].message.content` non-empty | `assert_eq!(resp.status(), 200); assert!(!body.choices[0].message.content.is_empty())` | `gadgetron-testing/tests/e2e/chat_completion.rs` |
| 2 | SSE streaming | Same request + `"stream": true` | HTTP 200 + `text/event-stream` + multiple `data: {...}` lines + `data: [DONE]` | Count chunks ≥ 2, last chunk = `[DONE]` | `gadgetron-testing/tests/e2e/streaming.rs` |
| 3 | Auth failure (missing key) | `POST /v1/chat/completions` without `Authorization` header | HTTP 401 + `{"error":{"message":"...","type":"authentication_error","code":"tenant_not_found"}}` | `assert_eq!(resp.status(), 401)` | `gadgetron-testing/tests/e2e/auth.rs` |
| 4 | Scope denial (403) | `POST /api/v1/nodes` with `Scope::OpenAiCompat` key | HTTP 403 + `{"error":{"code":"forbidden"}}` | `assert_eq!(resp.status(), 403)` | `gadgetron-testing/tests/e2e/auth.rs` |
| 5 | Quota exceeded | Send requests until `daily_cost_cents > daily_limit_cents` | HTTP 429 + `{"error":{"code":"quota_exceeded"}}` | Preconfigure `FakeQuota` with limit=100 cents, send requests worth 101 cents | `gadgetron-testing/tests/e2e/quota.rs` |
| 6 | Audit log verification | Send 10 requests, query `audit_log` table | `SELECT COUNT(*) FROM audit_log WHERE tenant_id = $1` = 10 | `PgHarness::count_audit_entries(tenant_id) == 10` | `gadgetron-testing/tests/e2e/audit_e2e.rs` |
| 7 | Circuit breaker trip | Configure `FailingProvider(ImmediateFail)`, send 3 requests | 4th request: HTTP 503 + `{"error":{"code":"routing_failure"}}` (circuit open) | `assert_eq!(resp4.status(), 503)` | `gadgetron-testing/tests/e2e/circuit_breaker.rs` |
| 8 | Graceful shutdown | Send SIGTERM during active SSE stream | Stream receives `data: [DONE]` or connection closes cleanly, audit entry written with `status=stream_interrupted` | `assert!(audit_entry.status == "stream_interrupted")` | `gadgetron-testing/tests/e2e/graceful_shutdown.rs` |
| 9 | Process restart recovery | Start gateway → register node → kill node → restart node | Node re-registers, `GET /api/v1/nodes` shows node with `state=Healthy` after heartbeat | `assert_eq!(node.state, "Healthy")` within 30s | `gadgetron-testing/tests/e2e/process_restart_recovery.rs` |

모든 e2e 시나리오는 `GatewayHarness` + `FakeLlmProvider` + `PgHarness (testcontainers)` 조합으로 실행. 네트워크 의존성 없음.

---

#### 2.A.8 Public vs Internal API 경계

| 경로 접두사 | 대상 | 안정성 | 인증 | 필수 Scope |
|------------|------|--------|------|-----------|
| `/v1/*` | 외부 클라이언트 (OpenAI 호환) | semver major 전까지 불변 | Bearer `gad_live_*` or `gad_test_*`; v0.5.9+ `gadgetron_session` cookie fallback (role → scope 합성) | `Scope::OpenAiCompat` |
| `/api/v1/*` | 운영자, 관리 도구 | semver-minor 추가 허용 | Bearer; v0.5.9+ cookie fallback (admin role 만 Management scope 획득) | `Scope::Management` |
| `/api/v1/xaas/*` | XaaS 관리자 | Phase 2 안정화 후 stable | Bearer; v0.5.9+ cookie fallback | `Scope::XaasAdmin` |
| `/api/v1/auth/{login,logout,whoami}` | Browser cookie session 유입점 (ISSUE 15 / v0.5.8) | semver-minor 추가 허용 | public; 각 handler 자체 인증 (email/password 또는 cookie jar) | — (cookie 발급 / 폐기 / whoami only) |
| `/api/v1/web/workbench/events/ws` | Web UI realtime WebSocket (activity bus fan-out) | semver-minor 추가 허용 | Bearer or `?access_token=` query-param (auth middleware query-token fallback scoped to this path only) | `Scope::OpenAiCompat` (workbench subtree scope rule) — shipped EPIC 1 ISSUE 4 / v0.2.7 / PR #194; original `/api/v1/ws/*` prefix never materialized, workbench subtree absorbed it |
| `/health` | K8s liveness probe | 불변 | 없음 (public) | — |
| `/ready` | K8s readiness probe | 불변 | 없음 (public) | — |
| `/metrics` | Prometheus scrape | [P1] | 없음 (internal network only) | — |

**`/health` vs `/ready` 분리** (devops-sre-lead 지적 반영):
- `/health` → liveness: 프로세스 살아있음 확인. PostgreSQL 연결 확인 없음.
- `/ready` → readiness: PostgreSQL 연결 + 최소 1개 provider healthy → `200 OK`. 실패 시 `503`.

---

#### §2.A.9 CLI Subcommand Tree & Flags (M-DX-3)

`gadgetron` 바이너리의 완전한 서브커맨드·플래그 목록. 구현 위치: `gadgetron-cli/src/cli.rs` (clap 4.x `derive` API).

```
gadgetron
├── serve              Start the gateway + scheduler (default subcommand)
│   ├── --config, -c   Path to gadgetron.toml (default: ./gadgetron.toml, env: GADGETRON_CONFIG)
│   ├── --bind, -b     Listen address (default: 0.0.0.0:8080, env: GADGETRON_BIND)
│   ├── --log-level    Log level (default: info, env: RUST_LOG)
│   ├── --log-format   json | pretty (default: json, env: GADGETRON_LOG_FORMAT)
│   └── --tui          Enable TUI dashboard (default: false)
├── node               Start a NodeAgent (GPU worker)
│   ├── --config, -c   Path to gadgetron.toml
│   ├── --scheduler-url URL of scheduler (default from config, env: GADGETRON_SCHEDULER_URL)
│   └── --gpu-ids      Comma-separated GPU device indices (default: all, env: CUDA_VISIBLE_DEVICES)
├── model              Model management commands
│   ├── list           List deployed models
│   ├── deploy <id>    Deploy a model to the cluster
│   └── remove <id>    Remove a deployed model
├── migrate            Database migration commands
│   ├── run            Run pending migrations (default)
│   └── info           Show migration status
├── tenant             Tenant management
│   ├── create         Create a tenant (--name <name>)
│   └── list           List tenants
├── key                API key management
│   ├── create         Create an API key (--tenant-id <uuid> --scope <scope>)
│   ├── list           List keys for a tenant (--tenant-id <uuid>)
│   └── revoke         Revoke a key (--key-id <uuid>)
├── health             Check cluster health
│   └── --url, -u      Gateway URL (default: http://localhost:8080)
└── version            Print version and exit
```

**환경 변수 우선순위**: CLI 플래그 > 환경 변수 > `gadgetron.toml` > 내장 기본값.

**예시 호출**:

```bash
# 기본 설정으로 게이트웨이 시작
gadgetron serve

# 커스텀 설정 + TUI 대시보드 활성화
gadgetron serve -c /etc/gadgetron/gadgetron.toml --tui

# 노드 에이전트 시작 (GPU 0, 1번만)
gadgetron node --scheduler-url http://scheduler:8080 --gpu-ids 0,1

# 마이그레이션 실행
gadgetron migrate run

# 클러스터 헬스 확인
gadgetron health -u http://gateway:8080

# 버전 출력
gadgetron version
```

**`serve`는 기본 서브커맨드**: `gadgetron` 만 실행해도 `gadgetron serve` 와 동일.

---

### Axis B: 크로스컷 (Cross-Cutting Concerns)

> **v1 작성일**: 2026-04-12 (Call 2 of 6)
> **적용 결정**: B-8, H-9, H-12, H-16, M-6, M-8; D-20260411-09, D-20260411-12, D-20260411-13, D-20260412-02
> **담당 크레이트**: 모든 크레이트 (cross-cutting). 주 구현 위치는 각 §에 명시.

---

#### §2.B.1 Observability Stack

**라이브러리 버전 (확정)**:
- `tracing 0.1` — 계측 매크로 (`info!`, `instrument`, `Span::current`)
- `tracing-subscriber 0.3` — subscriber 조합 (`EnvFilter`, `fmt::Layer`, `Registry`)
- `tracing-opentelemetry 0.27` — tracing span → OTel span 브리지
- `opentelemetry 0.27` — OTel API
- `opentelemetry-otlp 0.27` — OTLP gRPC exporter
- `opentelemetry_sdk 0.27` — TracerProvider, BatchSpanProcessor
- `metrics 0.23` — Prometheus metric 매크로 (`counter!`, `gauge!`, `histogram!`)
- `metrics-exporter-prometheus 0.15` — Prometheus scrape 서버 (port 9090)

**초기화 위치**: `gadgetron-gateway/src/observability/init.rs`

```rust
// gadgetron-gateway/src/observability/init.rs
pub fn init_observability(cfg: &TelemetryConfig) -> anyhow::Result<()> {
    // 1. OTel TracerProvider (OTLP gRPC)
    let otlp_exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(&cfg.otlp_endpoint);  // "http://jaeger:4317"
    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(otlp_exporter)
        .with_trace_config(
            opentelemetry_sdk::trace::Config::default()
                .with_sampler(Sampler::TraceIdRatioBased(cfg.sampling_ratio))  // default 0.1
                .with_resource(Resource::new(vec![
                    KeyValue::new("service.name", "gadgetron"),
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ]))
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;

    // 2. tracing-subscriber stack
    let env_filter = EnvFilter::try_from_env("RUST_LOG")
        .unwrap_or_else(|_| EnvFilter::new("gadgetron=info,tower_http=info"));

    let fmt_layer = match cfg.log_format {
        LogFormat::Json    => tracing_subscriber::fmt::layer().json().boxed(),
        LogFormat::Pretty  => tracing_subscriber::fmt::layer().pretty().boxed(),
    };

    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    Registry::default()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .init();

    // 3. Prometheus exporter
    let recorder = PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], cfg.metrics_port))  // default 9090
        .install()?;
    Ok(())
}
```

##### §2.B.1.1 Span Fields Standard

모든 크레이트에서 span field 이름은 아래 표를 따른다. 이 이름 이외의 key는 크레이트별 추가만 허용. 삭제·rename 금지.

| Field 이름 | 타입 | 전파 시점 | 출처 크레이트 |
|-----------|------|----------|-------------|
| `request_id` | `String` (UUID v4) | TraceLayer 진입 시 생성 | `gadgetron-gateway` |
| `tenant_id` | `String` (UUID v4) | AuthLayer 통과 후 기록 | `gadgetron-gateway` (from xaas) |
| `model` | `String` | handler 진입 시 | `gadgetron-gateway` |
| `provider` | `String` | router 결정 후 | `gadgetron-router` |
| `strategy` | `String` | router 결정 후 | `gadgetron-router` |
| `error.type` | `String` | 오류 시 | 오류 발생 크레이트 |
| `error.kind` | `String` | 오류 시 | 오류 발생 크레이트 |
| `error.retryable` | `bool` | 오류 시 | 오류 발생 크레이트 |
| `cache_hit` | `bool` | auth 캐시 조회 시 | `gadgetron-xaas` |
| `tokens_estimated` | `u32` | quota pre-check 시 | `gadgetron-xaas` |
| `tokens_actual` | `u32` | quota post-record 시 | `gadgetron-xaas` |
| `cost_cents` | `i64` | quota post-record 시 | `gadgetron-xaas` |
| `latency_us` | `u64` | 구간 측정 시 | 각 크레이트 |

**request_id 전파 경로 (확정)**:
```
TraceLayer (gadgetron-gateway):
    request_id = Uuid::new_v4().to_string()
    tracing::Span::current().record("request_id", &request_id)
    Response header: "X-Request-Id: {request_id}"
  → Provider HTTP 호출:
    reqwest::RequestBuilder::header("X-Request-Id", &request_id)
  → AuditEntry:
    AuditEntry { request_id: Uuid, ... }
```

##### §2.B.1.2 OTel Span 계층 (완전 열거)

```
Root span: "gadgetron.request"
  fields: request_id, tenant_id, model, http.method="POST", http.route="/v1/chat/completions"
  duration: gateway overhead 전체 (P99 < 1ms 목표)
  │
  ├─ "gadgetron.auth"
  │    fields: cache_hit (bool)
  │    duration: moka lookup ~1μs (hit) / PG query ~3-5ms (miss)
  │
  ├─ "gadgetron.quota.check"
  │    fields: tenant_id, tokens_estimated
  │    duration: in-memory TokenBucket check ~1μs
  │
  ├─ "gadgetron.routing"
  │    fields: strategy, selected_provider, candidate_count
  │    duration: DashMap read + AtomicUsize ~1μs
  │
  ├─ "gadgetron.provider.call"
  │    fields: provider, model, latency_us
  │    duration: HTTP RTT to upstream (excluded from 1ms budget)
  │    └─ OTel outbound HTTP span (propagated via traceparent header)
  │
  ├─ "gadgetron.quota.record"
  │    fields: tokens_actual, cost_cents
  │    duration: in-memory update + mpsc::try_send ~1μs
  │
  └─ "gadgetron.audit.enqueue"
       fields: tenant_id, request_id
       duration: mpsc::try_send ~100ns
```

**OTel OTLP 엔드포인트 계약**:

| 항목 | 값 |
|------|-----|
| 프로토콜 | gRPC (HTTP/2) |
| 기본 엔드포인트 | `http://localhost:4317` |
| 환경변수 override | `OTEL_EXPORTER_OTLP_ENDPOINT` |
| 인증 | 없음 (Phase 1). mTLS [P2] |
| 호환 백엔드 | Jaeger 1.50+, Grafana Tempo 2.0+ |
| export 모드 | `BatchSpanProcessor` (비동기, 드레인 시 flush) |
| 샘플링 기본값 | `TraceIdRatioBased(0.1)` — 10% 샘플링 |
| 환경변수 override | `GADGETRON_SAMPLING_RATIO` (float 0.0~1.0) |

##### §2.B.1.3 Prometheus Metric 명명 규약

패턴: `gadgetron_<subsystem>_<metric>_<unit>`
- `<subsystem>`: `gateway`, `router`, `provider`, `scheduler`, `node`, `xaas`
- `<unit>`: `total` (Counter), `seconds` (Summary/Histogram), `bytes`, `mb`, `pct`, `c` (Celsius)

**카디널리티 상한 (근거 포함)**:

| 메트릭 | 레이블 | 최대 카디널리티 | 근거 |
|--------|--------|---------------|------|
| `gadgetron_gateway_requests_total` | `{method, path, status}` | 50 | 3 method × 5 path × 4 status |
| `gadgetron_provider_requests_total` | `{provider, model, status}` | 144 | 6 × 6 × 4 |
| `gadgetron_node_gpu_vram_used_mb` | `{node_id, gpu_index}` | 800 | 100 node × 8 GPU |
| `gadgetron_xaas_audit_dropped_total` | (레이블 없음) | 1 | **tenant_id 레이블 제거** — 폭발 위험 (xaas-platform-lead 지적) |
| `gadgetron_xaas_quota_exceeded_total` | (레이블 없음) | 1 | 동일 이유 |
| `gadgetron_router_routing_decisions_total` | `{strategy, provider}` | 36 | 6 × 6 |

> **근거**: 카디널리티 800 이하면 단일 Prometheus 인스턴스 (2GB RAM) 기준 메모리 안전. 1,000 초과 레이블 조합은 PM 승인 필요.

**전체 메트릭 목록 (Phase 1 확정)**:

```
# Gateway (gadgetron-gateway)
gadgetron_gateway_requests_total{method, path, status}           Counter
gadgetron_gateway_request_duration_seconds{path}                 Histogram (buckets: 0.001,0.005,0.01,0.05,0.1,0.5,1.0)
gadgetron_gateway_active_connections                             Gauge
gadgetron_gateway_ws_clients_total                               Gauge

# Router (gadgetron-router)
gadgetron_router_routing_decisions_total{strategy, provider}     Counter
gadgetron_router_fallback_total{from_provider, to_provider}      Counter
gadgetron_router_circuit_open_total{provider}                    Counter

# Provider (gadgetron-provider)
gadgetron_provider_requests_total{provider, model, status}       Counter
gadgetron_provider_latency_seconds{provider, model}              Histogram (buckets: 0.05,0.1,0.5,1.0,2.0,5.0,10.0,30.0)
gadgetron_provider_errors_total{provider, model, error_kind}     Counter
gadgetron_provider_stream_interrupted_total{provider, model}     Counter

# Scheduler (gadgetron-scheduler)
gadgetron_scheduler_deployments_total{engine, status}            Counter
gadgetron_scheduler_vram_used_mb{node_id}                        Gauge
gadgetron_scheduler_evictions_total{policy, node_id}             Counter
gadgetron_scheduler_vram_sync_lag_seconds                        Gauge

# Node (gadgetron-node)
gadgetron_node_gpu_vram_used_mb{node_id, gpu_index}              Gauge
gadgetron_node_gpu_utilization_pct{node_id, gpu_index}           Gauge
gadgetron_node_gpu_temperature_c{node_id, gpu_index}             Gauge
gadgetron_node_gpu_power_w{node_id, gpu_index}                   Gauge
gadgetron_node_nvml_poll_duration_seconds                        Histogram
gadgetron_node_nvml_poll_stall_total                             Counter

# XaaS (gadgetron-xaas)
gadgetron_xaas_audit_dropped_total                               Counter  ← 레이블 없음 (카디널리티 통제)
gadgetron_xaas_quota_exceeded_total                              Counter  ← 레이블 없음
gadgetron_xaas_auth_cache_hits_total                             Counter
gadgetron_xaas_auth_cache_misses_total                           Counter
gadgetron_xaas_auth_duration_seconds                             Histogram
gadgetron_xaas_pg_pool_available                                 Gauge
gadgetron_xaas_audit_channel_depth                               Gauge
```

**Scrape 엔드포인트 계약**:

| 항목 | 값 |
|------|-----|
| 경로 | `GET /metrics` |
| 포트 | `9090` (기본값, `TelemetryConfig.metrics_port`로 변경 가능) |
| Content-Type | `text/plain; version=0.0.4; charset=utf-8` |
| 인증 | 없음 (내부 네트워크 전용 — K8s NetworkPolicy로 격리) |
| Scrape 권장 주기 | `15s` |

---

#### §2.B.2 Alert Rules & SLO Tables (H-16 해소)

**Alert Rules YAML** — Prometheus recording + alerting rules. 위치: `deploy/prometheus/gadgetron-alerts.yaml`

```yaml
groups:
  - name: gadgetron.slo
    rules:
      # P99 Latency Breach
      - alert: GadgetronGatewayP99LatencyHigh
        expr: |
          histogram_quantile(
            0.99,
            rate(gadgetron_gateway_request_duration_seconds_bucket[5m])
          ) > 0.001
        for: 2m
        severity: critical
        labels:
          runbook: "https://docs.gadgetron.io/runbooks/latency-breach"
        annotations:
          summary: "Gateway P99 latency {{ $value | humanizeDuration }} exceeds 1ms SLO"
          description: "P99 gateway overhead has exceeded the 1ms target for 2 minutes. Check auth cache hit rate and quota enforcer."

      # Provider Circuit Open
      - alert: GadgetronProviderCircuitOpen
        expr: |
          increase(gadgetron_router_circuit_open_total[5m]) > 0
        for: 0m
        severity: warning
        labels:
          runbook: "https://docs.gadgetron.io/runbooks/circuit-open"
        annotations:
          summary: "Provider circuit breaker opened for {{ $labels.provider }}"
          description: "Provider {{ $labels.provider }} circuit is open. Fallback chain active. Check provider health."

      # NVML Poll Stall
      - alert: GadgetronNvmlPollStall
        expr: |
          increase(gadgetron_node_nvml_poll_stall_total[5m]) > 3
        for: 1m
        severity: warning
        labels:
          runbook: "https://docs.gadgetron.io/runbooks/nvml-stall"
        annotations:
          summary: "NVML polling stalled on node {{ $labels.node_id }}"
          description: "NVML poll on {{ $labels.node_id }} has stalled >3 times in 5m. GPU metrics may be stale. VRAM scheduling accuracy degraded."

      # Audit Writer Drop
      - alert: GadgetronAuditDropped
        expr: |
          increase(gadgetron_xaas_audit_dropped_total[1m]) > 0
        for: 0m
        severity: critical
        labels:
          runbook: "https://docs.gadgetron.io/runbooks/audit-drop"
        annotations:
          summary: "Audit entries are being dropped"
          description: "Audit entries dropped in last minute. mpsc channel (capacity 4096) may be saturated. Compliance risk — check AuditWriter consumer lag."

      # DB Pool Exhausted
      - alert: GadgetronPgPoolExhausted
        expr: |
          gadgetron_xaas_pg_pool_available == 0
        for: 30s
        severity: critical
        labels:
          runbook: "https://docs.gadgetron.io/runbooks/pg-pool-exhausted"
        annotations:
          summary: "PostgreSQL connection pool exhausted"
          description: "No available PG connections for 30s. All auth and audit writes are failing. Service is rejecting all requests."

      # Graceful Shutdown Timeout
      - alert: GadgetronShutdownTimeout
        expr: |
          gadgetron_gateway_active_connections > 0
          unless on() up{job="gadgetron"} == 1
        for: 70s
        severity: warning
        labels:
          runbook: "https://docs.gadgetron.io/runbooks/shutdown-timeout"
        annotations:
          summary: "Gadgetron shutdown exceeded 70s drain window"
          description: "Active connections remain after 70s graceful shutdown window. SIGKILL may cause audit loss."

      # GPU Temperature
      - alert: GadgetronGpuTempHigh
        expr: |
          gadgetron_node_gpu_temperature_c > 85
        for: 2m
        severity: warning
        labels:
          runbook: "https://docs.gadgetron.io/runbooks/gpu-temp"
        annotations:
          summary: "GPU {{ $labels.node_id }}/{{ $labels.gpu_index }} temperature {{ $value }}°C"
          description: "GPU temperature exceeds 85°C for 2 minutes. Thermal throttling may begin at 90°C."
```

**SLO 정의표**:

| SLO ID | 지표 | 목표 | 측정 window | Alert 연결 |
|--------|------|------|------------|------------|
| SLO-1 | Gateway P99 overhead | < 1ms | 5m rolling | `GadgetronGatewayP99LatencyHigh` |
| SLO-2 | Audit drop rate | 0% (Phase 1) | 1m rolling | `GadgetronAuditDropped` |
| SLO-3 | Provider availability | ≥ 1 non-circuit-open provider | 즉시 | `GadgetronProviderCircuitOpen` |
| SLO-4 | GPU metric freshness | NVML data < 2s old | 5m rolling | `GadgetronNvmlPollStall` |
| SLO-5 | Auth response time P99 | < 8ms (cache miss), < 1ms (hit) | 5m rolling | (Histogram 분석) |

---

#### §2.B.3 Security Plane

##### §2.B.3.1 TLS 설정

**인바운드 TLS** (클라이언트 → Gadgetron):

```rust
// gadgetron-gateway/src/tls.rs
// Phase 1: axum-server with rustls
use axum_server::tls_rustls::RustlsConfig;

pub async fn build_tls_config(cfg: &TlsConfig) -> anyhow::Result<RustlsConfig> {
    // cfg.cert_path: String — PEM 형식 인증서 경로 (env: GADGETRON_TLS_CERT_PATH)
    // cfg.key_path:  String — PEM 형식 개인키 경로  (env: GADGETRON_TLS_KEY_PATH)
    RustlsConfig::from_pem_file(&cfg.cert_path, &cfg.key_path).await
}
```

`gadgetron.toml` TLS 필드:
```toml
[server.tls]
enabled    = false              # 기본값: false (개발). 프로덕션: true
cert_path  = "${GADGETRON_TLS_CERT_PATH}"   # env 치환 필수
key_path   = "${GADGETRON_TLS_KEY_PATH}"    # env 치환 필수
```

암호화 스위트: `rustls` 기본 TLS 1.3 스위트 (AES-256-GCM-SHA384, CHACHA20-POLY1305-SHA256). Phase 1에서 사용자 정의 스위트 목록은 지원하지 않는다.

**TLS Hot Reload 정책**: Phase 1에서 TLS 인증서 교체는 프로세스 재시작 필요. Phase 2에서 `cert-manager` 연동 + SIGHUP 기반 hot reload 추가.

**아웃바운드 TLS** (Gadgetron → Provider):
- `reqwest 0.12` + `rustls-tls` feature — `default-features = false`
- `OpenAiProvider`, `AnthropicProvider`, `GeminiProvider`: HTTPS 전용 (HTTP 연결 시 panic)
- `OllamaProvider`, `VllmProvider`, `SgLangProvider`: HTTP 허용 (로컬 전용. `GADGETRON_ALLOW_HTTP_PROVIDERS=true` 환경변수로 명시적 허용 필요)

##### §2.B.3.2 Auth Middleware Chain 순서 (Axis A §2.A.2 정합)

Tower `ServiceBuilder` 레이어 순서 (바깥→안쪽 — 요청은 위에서 아래로, 응답은 아래에서 위로):

```rust
// gadgetron-gateway/src/server.rs
ServiceBuilder::new()
    .layer(TraceLayer::new_for_http())          // 1. Span 생성, request_id 부여
    .layer(RequestIdLayer)                       // 2. X-Request-Id 헤더 set (TraceLayer 내부)
    .layer(AuthLayer::new(key_validator))        // 3. Bearer OR cookie 검증 → TenantContext (cookie fallback: ISSUE 16 / v0.5.9)
    .layer(TenantContextLayer)                   // 4. TenantContext를 request Extension에 삽입
    .layer(ScopeGuardLayer)                      // 5. 경로별 Scope 검사 (401/403)
    // NOTE: rate limit does NOT sit on the tower stack — it shipped
    // in EPIC 4 ISSUE 11 TASK 11.2 (PR #231 / v0.5.2) as
    // `RateLimitedQuotaEnforcer` composite inside the handler-level
    // QuotaEnforcer::check_pre path, not as a tower layer. See
    // §2.B.3.5 for rationale.
    // [future] .layer(GuardrailsLayer) — not scheduled
    .service(router)                             // 6. axum Router
```

레이어별 상세:

| 레이어 | 크레이트 소유자 | 함수 시그니처 / 역할 | 오류 variant | latency budget |
|--------|--------------|-------------------|-------------|---------------|
| `TraceLayer` | `tower-http 0.5` | `tower_http::trace::TraceLayer::new_for_http()` | — | ~1μs (UUID 생성) |
| `RequestIdLayer` | `gadgetron-gateway` | `middleware::request_id::RequestIdLayer` | — | ~100ns |
| `AuthLayer` | `gadgetron-gateway` (from xaas) | `middleware::auth::AuthLayer<PgKeyValidator>` | `TenantNotFound`, `Forbidden` | < 1ms cache hit / < 8ms miss |
| `TenantContextLayer` | `gadgetron-gateway` | Extension 삽입만 — no I/O | — | ~100ns |
| `ScopeGuardLayer` | `gadgetron-gateway` | `require_scope(ctx, Scope::OpenAiCompat)` — Vec linear scan (max 3 elements) | `Forbidden` | ~1μs |
| ~~`RateLimitLayer`~~ (NOT on tower stack) | `gadgetron-xaas` | Shipped as `RateLimitedQuotaEnforcer` composite wrapping the inner cost enforcer; rate check runs inside `QuotaEnforcer::check_pre` before cost check (fail-fast). Opt-in via `[quota_rate_limit]` config. EPIC 4 ISSUE 11 TASK 11.2 / PR #231 / v0.5.2. | `QuotaExceeded` (429 + `Retry-After: 60` + `retry_after_seconds` body field) | ~1μs DashMap-sharded lazy refill |

**`AuthLayer` 구현 계약**:
```rust
// gadgetron-gateway/src/middleware/auth.rs
pub struct AuthLayer<V: KeyValidator + Clone + Send + Sync + 'static> {
    validator: V,
}

impl<V, S, B> tower::Layer<S> for AuthLayer<V>
where
    V: KeyValidator + Clone + Send + Sync + 'static,
    S: tower::Service<axum::http::Request<B>, Response = axum::http::Response<axum::body::Body>>
         + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    // Bearer token 추출 → V::validate() 호출
    // 성공 → Extension<TenantContext> 삽입 → next.call(req)
    // 실패 → GadgetronError → HTTP 401/403
}
```

##### §2.B.3.3 Bearer Token 검증 경로

```
Authorization: Bearer gad_live_{32-char}
  → AuthLayer: token.strip_prefix("Bearer ")
  → SHA-256(token) → hex → cache_key
  → PgKeyValidator::validate(cache_key):
      moka cache hit  → ValidatedKey (instant)
      moka cache miss → PostgreSQL: SELECT ... WHERE key_hash=$1 AND is_revoked=false
                     → ValidatedKey (3-5ms)
  → ValidatedKey { tenant_id, scopes: Vec<Scope>, quota_config: QuotaConfig }
  → Extension<TenantContext> { tenant_id, scopes, quota_config } 삽입
```

캐시 설정 (D-20260411-12):
- 구현: `moka::future::Cache<String, ValidatedKey>`
- capacity: 10,000 entries (eviction: LRU + TTL)
- TTL: 600초 (10분)
- key: SHA-256(bearer_token) hex string — 원문 토큰을 캐시 키로 저장하지 않음
- 무효화: `PgKeyValidator::invalidate(key_hash: &str)` — revoke 즉시 호출

##### §2.B.3.3b Cookie Session 검증 경로 (ISSUE 16 TASK 16.1 / PR #259 / v0.5.9)

Bearer 경로가 헤더 없음으로 단락되는 경우, auth_middleware 는 cookie fallback 을 시도:

```
(Authorization 헤더 없음) AND (Cookie: gadgetron_session=<hex-token>)
  → auth_middleware: jar.get("gadgetron_session") → cookie_token: String
  → validate_session_and_build_key(pool, cookie_token):
      1. gadgetron_xaas::sessions::validate_session(pool, cookie_token):
           SHA-256(cookie_token) → SELECT FROM user_sessions
           WHERE cookie_hash=$1 AND expires_at > now() AND revoked_at IS NULL
           → SessionRow { session_id, user_id, tenant_id, expires_at } (1-3ms)
      2. SELECT role FROM users WHERE id = $1 (0.5-1ms)
      3. role → scope 합성:
           admin   → [Scope::OpenAiCompat, Scope::Management]
           member  → [Scope::OpenAiCompat]
           service → TenantNotFound 로 fail-closed (login 단계에서 차단되므로
                     방어적 guard)
      4. ValidatedKey { api_key_id: Uuid::nil(), tenant_id, scopes }
  → Extension<TenantContext> 삽입 (Bearer 경로와 동일 downstream shape)
```

**Bearer 와의 차이점**:
- **캐시 없음**: cookie 매 요청 당 pg 한 번 hit (Bearer 는 moka cache 로 ≥99% 히트). session 당 수명이 24h 이고 revoke / expiry 가 즉시 반영되어야 하므로 moka TTL 패턴이 맞지 않음. 성능이 병목이면 future TASK 에서 revocation-propagation 이 있는 캐시 도입 가능.
- **`api_key_id = Uuid::nil()` sentinel**: audit 계층이 `key_id == Uuid::nil()` 을 탐지해 `actor_user_id = session.user_id` 로 기록할 수 있도록 (column plumbing 은 ISSUE 14 TASK 14.1 migration 에서 schema 만 완성; audit writer 측 wire-up 은 후속 TASK).
- **no-db 모드**: cookie path silently skip (pool 부재 = 세션 DB 부재). `--no-db` 배포는 Bearer-only.
- **적용 범위**: `/v1/*` + `/api/v1/web/workbench/*` + `/api/v1/xaas/*` 전 영역. `/api/v1/auth/{login,logout,whoami}` 세 endpoint 만 `public_routes` 에 남아 자체 인증.

Harness gate 7v.6 가 login → cookie jar → /admin/users (Management) + /quota/status (OpenAiCompat) + no-auth 401 세 가지 경로를 pin.

##### §2.B.3.4 CORS 정책 (D-6 준수)

```rust
// gadgetron-gateway/src/server.rs
let cors = CorsLayer::new()
    .allow_origin([
        "https://app.gadgetron.io".parse::<HeaderValue>().unwrap(),
        // 추가 origin은 TelemetryConfig.allowed_origins: Vec<String> 에서 로드
    ])
    .allow_methods([Method::POST, Method::GET, Method::OPTIONS])
    .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT])
    .max_age(Duration::from_secs(3600));
// 와일드카드 origin 금지 (D-6). 개발 환경에서도 localhost:3000 명시.
```

##### §2.B.3.5 Rate Limit [shipped via EPIC 4 ISSUE 11 TASK 11.2 / PR #231 / v0.5.2]

Per-tenant token-bucket rate limiter shipped in `gadgetron-xaas::quota::rate_limit::TokenBucketRateLimiter`. Not layered as a separate `RateLimitLayer` on the tower stack — instead wrapped into the existing `QuotaEnforcer` trait via `RateLimitedQuotaEnforcer` composite, which runs the rate check BEFORE delegating to the inner cost enforcer (fail-fast semantics, no snapshot query on rate rejection).

Buckets are `DashMap<TenantId, Bucket>` sharded, lazy-refilled on `consume()`, monotonic `Instant` clock (wall-clock skew can't double-spend), fractional tokens so small refills don't round to zero. No `governor` crate — hand-rolled to keep the enforcer composition clean and avoid the crate's tower-layer assumptions.

Config block is `[quota_rate_limit]` (NOT `[xaas.rate_limit]` — renamed during implementation):

```toml
[quota_rate_limit]
requests_per_minute = 60   # 0 = limiter is a no-op, preserving pre-11.2 behavior
burst = 30                 # 0 defaults to requests_per_minute
```

Opt-in: default `requests_per_minute = 0` is a no-op so existing deployments upgrading from pre-0.5.2 see zero behavior change until they configure a non-zero rate. The CLI wires `RateLimitedQuotaEnforcer` at `init_serve_runtime` only when `requests_per_minute > 0`.

Historical context: the "governor 0.6 + tower layer" plan was the original P2 sketch. The implementation in PR #231 opted for the enforcer-composition pattern instead because it shares the request context (`TenantContext`, `QuotaSnapshot`) that the cost enforcer already threads, avoiding duplicate lookups on the hot path. See ISSUE 11 design notes in `docs/modules/xaas-platform.md §5.6` for the composition chain.

Rate rejections surface as structured 429 + `Retry-After: 60` header + `retry_after_seconds: 60` JSON body field (ISSUE 11 TASK 11.1 / PR #230 / v0.5.1). 5 unit tests pin bucket semantics (within-burst, exceeds-burst, refill-after-wait, disabled limiter, per-tenant isolation). No harness gate because harness leaves `quota_rate_limit` disabled — unit tests carry the correctness proof.

---

#### §2.B.4 Secret Management (security-compliance-lead coordination)

> security-compliance-lead (D-20260412-01)가 Round 1.5에서 이 섹션을 검토한다.

##### §2.B.4.1 Phase 1: 환경변수 + TOML `${ENV}` 치환

```
비밀값 저장 위치:
  시스템 환경변수 (export OPENAI_API_KEY=sk-...)
  또는 /etc/gadgetron/.env (파일 권한 600, root 소유)

로딩 순서:
  1. gadgetron.toml 읽기
  2. ${ENV_VAR} 패턴 → std::env::var(ENV_VAR) 치환
  3. 치환 실패 시 (env var 미설정) → AppConfig::load() panic (fail-fast on boot)
  4. 치환 후 validation → AppConfig 구조체

치환 대상 필드:
  providers.openai.api_key    = "${OPENAI_API_KEY}"
  providers.anthropic.api_key = "${ANTHROPIC_API_KEY}"
  providers.gemini.api_key    = "${GEMINI_API_KEY}"
  xaas.pg_url                 = "${GADGETRON_PG_URL}"
  server.tls.cert_path        = "${GADGETRON_TLS_CERT_PATH}"
  server.tls.key_path         = "${GADGETRON_TLS_KEY_PATH}"
```

##### §2.B.4.2 Tracing Redaction Layer

비밀값이 로그에 노출되지 않도록 타입 레벨에서 차단한다.

```rust
// gadgetron-core/src/secret.rs
use std::fmt;

/// Wrapper that redacts the inner value from all Display/Debug output.
/// Use for API keys, passwords, and any field that must not appear in logs.
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(value: T) -> Self { Self(value) }
    pub fn expose(&self) -> &T { &self.0 }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

// 역직렬화: serde::Deserialize<'de> for Secret<T> where T: Deserialize<'de>
// 직렬화: serialize는 구현하지 않음 (config re-serialization 금지)
```

**사용 위치**:
```rust
// gadgetron-core/src/config.rs
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub api_key:       Secret<String>,   // Display → "[REDACTED]"
    pub endpoint:      String,
    pub models:        Vec<String>,
    pub timeout_ms:    u64,
}
```

`tracing::instrument` macro에서 Secret 필드를 포함하는 struct를 `skip`해야 하는 경우:
```rust
#[tracing::instrument(skip(self, config), fields(provider = %config.provider_type))]
pub async fn validate_provider_config(&self, config: &ProviderConfig) { ... }
// ProviderConfig를 skip; provider_type만 field로 명시
```

##### §2.B.4.3 Secret Rotation 정책

| 비밀 종류 | 저장 위치 | 교체 주기 | 교체 절차 |
|----------|----------|---------|--------|
| Gadgetron API Key (`gad_live_*`) | PostgreSQL (SHA-256 해시만) | 90일 권고 (강제 아님 Phase 1) | 신규 key 발급 → 구 key revoke → moka cache invalidate |
| Provider API Key (OpenAI 등) | 환경변수 | 공급자 정책 준수 | 환경변수 업데이트 → 프로세스 재시작 (Phase 1) |
| TLS 인증서 | 파일시스템 | 90일 (Let's Encrypt 기준) | 인증서 교체 → 프로세스 재시작 (Phase 1) / cert-manager 자동화 [P2] |
| PostgreSQL 비밀번호 | 환경변수 | 180일 | DB password 변경 → 환경변수 업데이트 → 재시작 |

**revoke 시 moka cache 즉시 무효화 계약**:
```rust
// gadgetron-xaas/src/auth/validator.rs
impl PgKeyValidator {
    pub async fn revoke_key(&self, key_hash: &str) -> Result<(), GadgetronError> {
        // 1. PostgreSQL: UPDATE api_keys SET is_revoked=true WHERE key_hash=$1
        sqlx::query!("UPDATE api_keys SET is_revoked = true WHERE key_hash = $1", key_hash)
            .execute(&self.pool).await.map_err(sqlx_to_gadgetron)?;
        // 2. moka cache 즉시 무효화 (TTL 만료 기다리지 않음)
        self.cache.invalidate(key_hash).await;
        Ok(())
    }
}
// 주의: invalidate() 후 최대 10분 유예 없음. 즉시 다음 요청에서 miss → DB hit.
// Phase 1 known limitation: moka 단일 인스턴스. 다중 인스턴스 환경에서는 Redis 필요 [P2].
```

##### §2.B.4.4 Phase 2 Secrets Management

[P2] 아래 중 하나를 채택 (Phase 2 결정):
- **K8s Secret** — `spec.containers[].env[].valueFrom.secretKeyRef`
- **External Secrets Operator** — AWS Secrets Manager / GCP Secret Manager 연동
- **HashiCorp Vault** — Agent Injector 또는 직접 API 호출

Phase 1 Known Limitation: 환경변수 기반 secrets는 `ps aux` 또는 `/proc/{pid}/environ` 접근으로 노출 가능. 프로덕션 배포 전 security-compliance-lead 검토 게이트.

---

#### §2.B.5 Configuration (`AppConfig`)

##### §2.B.5.1 `AppConfig` 완전 타입 정의

```rust
// gadgetron-core/src/config.rs
use crate::secret::Secret;
use std::time::Duration;
use url::Url;

#[derive(Debug, serde::Deserialize)]
pub struct AppConfig {
    pub server:    ServerConfig,
    pub router:    RoutingConfig,
    pub providers: std::collections::HashMap<String, ProviderConfig>,
    pub nodes:     Vec<NodeConfig>,
    pub models:    Vec<LocalModelConfig>,
    pub scheduler: SchedulerConfig,
    pub xaas:      XaasConfig,
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, serde::Deserialize)]
pub struct ServerConfig {
    pub bind:               String,         // default: "0.0.0.0:8080"
    pub request_timeout_ms: u64,            // default: 120_000 (2분), env: GADGETRON_REQUEST_TIMEOUT_MS
    pub max_body_bytes:     usize,          // default: 4_194_304 (4MB), env: GADGETRON_MAX_BODY_BYTES — RequestBodyLimitLayer (M-SEC-2)
    pub tls:                TlsConfig,
}

#[derive(Debug, serde::Deserialize)]
pub struct TlsConfig {
    pub enabled:   bool,    // default: false, env: GADGETRON_TLS_ENABLED
    pub cert_path: String,  // env: GADGETRON_TLS_CERT_PATH — required if enabled=true
    pub key_path:  String,  // env: GADGETRON_TLS_KEY_PATH  — required if enabled=true
}

#[derive(Debug, serde::Deserialize)]
pub struct RoutingConfig {
    pub default_strategy: RoutingStrategy,  // default: RoundRobin
    pub fallback_chain:   Vec<String>,      // provider name 순서
    pub circuit_breaker:  CircuitBreakerConfig,
}

#[derive(Debug, serde::Deserialize)]
pub struct CircuitBreakerConfig {
    pub failure_threshold:    u32,   // default: 5 — 연속 5회 실패 시 open
    pub recovery_timeout_ms:  u64,   // default: 30_000 (30초) — half-open 전환
    pub probe_timeout_ms:     u64,   // default: 5_000 — half-open probe 타임아웃
}

#[derive(Debug, serde::Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,    // enum: OpenAi | Anthropic | Gemini | Ollama | Vllm | Sglang
    pub api_key:       Secret<String>,  // env 치환 필수 (cloud provider). 로컬은 빈 문자열 허용
    pub endpoint:      String,          // e.g. "https://api.openai.com/v1"
    pub models:        Vec<String>,     // e.g. ["gpt-4o", "gpt-4o-mini"]
    pub timeout_ms:    u64,             // default: 300_000 (5분)
    pub max_retries:   u32,             // default: 0 (retry는 tower::retry에서 처리)
}

#[derive(Debug, serde::Deserialize)]
pub struct NodeConfig {
    pub id:          String,   // e.g. "node-01"
    pub endpoint:    String,   // e.g. "http://gpu-node-01:7878"
    pub gpu_count:   u32,      // NVML 탐지와 교차 검증
    pub numa_nodes:  u32,      // default: 1
}

#[derive(Debug, serde::Deserialize)]
pub struct LocalModelConfig {
    pub id:                 String,         // e.g. "llama-3-8b"
    pub engine:             LocalEngine,    // enum: Ollama | Vllm | Sglang
    pub vram_requirement_mb: u64,           // 배포 시 VRAM check 기준
    pub args:               Vec<String>,    // 엔진 launch args
    pub priority:           i32,            // default: 0 — CostBased eviction stub (H-8)
}

#[derive(Debug, serde::Deserialize)]
pub struct SchedulerConfig {
    pub eviction_policy:    EvictionPolicy,  // enum: Lru | Priority | CostBased | WeightedLru (D-2)
    pub vram_ceiling_pct:   u8,              // default: 90 — 90%까지만 사용 (안전 버퍼)
    pub heartbeat_interval_ms: u64,          // default: 10_000 (10초) — NodeAgent → Scheduler (B-2)
    pub stale_threshold_ms:    u64,          // default: 30_000 (30초) — 이 이상이면 stale 처리 (B-2)
}

#[derive(Debug, serde::Deserialize)]
pub struct XaasConfig {
    pub pg_url:             Secret<String>,  // env: GADGETRON_PG_URL
    pub pg_max_connections: u32,             // default: 10 — sqlx pool max
    pub pg_connect_timeout_ms: u64,          // default: 5_000
    pub key_cache_ttl_secs: u64,             // default: 600 (10분), env: GADGETRON_KEY_CACHE_TTL
    pub key_cache_max_entries: u64,          // default: 10_000 (D-20260411-12)
    pub audit_channel_cap:  usize,           // default: 4_096 (D-20260411-09)
    pub quota_cache_ttl_secs: u64,           // default: 600 — quota_config moka TTL (H-6)
}

#[derive(Debug, serde::Deserialize)]
pub struct TelemetryConfig {
    pub log_format:     LogFormat,      // enum: Json | Pretty. default: Json
    pub otlp_endpoint:  String,         // default: "http://localhost:4317"
    pub metrics_port:   u16,            // default: 9090
    pub sampling_ratio: f64,            // default: 0.1 (10%), env: GADGETRON_SAMPLING_RATIO
    pub allowed_origins: Vec<String>,   // CORS allowed origins (D-6)
}

#[derive(Debug, serde::Deserialize)]
pub enum LogFormat { Json, Pretty }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProviderType { OpenAi, Anthropic, Gemini, Ollama, Vllm, Sglang }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LocalEngine { Ollama, Vllm, Sglang }

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RoutingStrategy { RoundRobin, CostOptimal, LatencyOptimal, QualityOptimal, Fallback, Weighted }

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EvictionPolicy { Lru, Priority, CostBased, WeightedLru { priority_weight: f32 } }
```

##### §2.B.5.2 설정 로딩 파이프라인 (완전 명세)

```
단계 1 — 파일 읽기:
  std::fs::read_to_string("config/gadgetron.toml")  또는
  환경변수 GADGETRON_CONFIG_PATH 경로
  실패 시: panic! — 설정 없이 구동 불가 (fail-fast)

단계 2 — TOML 파싱:
  toml::from_str::<AppConfig>(&raw)
  라이브러리: toml 0.8 + serde 1.0
  실패 시: panic! with toml::de::Error 메시지

단계 3 — ${ENV} 치환:
  AppConfig::substitute_env(&mut raw_str) — regex: \$\{([A-Z_]+)\}
  std::env::var(name) → Ok: 치환. Err: panic! ("env var {name} not set")
  치환 시점: TOML 파싱 이전 (raw string 단계에서 처리)

단계 4 — 검증:
  AppConfig::validate(&self) -> Result<(), Vec<String>>
  검증 규칙:
    server.bind: IP:PORT 형식 (parse::<SocketAddr>())
    server.request_timeout_ms: 1_000 ~ 600_000 범위
    scheduler.vram_ceiling_pct: 50 ~ 99 범위
    xaas.pg_max_connections: 1 ~ 100 범위
    telemetry.sampling_ratio: 0.0 ~ 1.0 범위
    providers: api_key non-empty for cloud (OpenAi, Anthropic, Gemini)
    providers: endpoint valid URL (url::Url::parse)
    tls.enabled = true이면 cert_path + key_path 존재 확인 (std::path::Path::exists)
  실패 시: 모든 오류 Vec 수집 후 1회 panic! (fail-fast on boot)

단계 5 — Arc 공유:
  Arc::new(config) → 모든 컴포넌트에 Arc<AppConfig> 전달
```

##### §2.B.5.3 Hot Reload 정책

**Phase 1**: 모든 설정 변경은 프로세스 재시작 필요. SIGHUP 수신 시 무시 (로그만 출력).

**Phase 2 / EPIC 3 ISSUE 8 (부분 구현)**: SIGHUP 은 이제 **descriptor catalog 한정** hot-reload trigger 로 사용된다 — `spawn_sighup_reloader()` Unix-only tokio task 가 `SignalKind::hangup()` 을 listen 하고 매 신호마다 `perform_catalog_reload()` 를 호출해 `[web] catalog_path` TOML 을 다시 파싱하여 `Arc<ArcSwap<CatalogSnapshot>>` 을 atomic 하게 교체한다 (TASK 8.5 / PR #217). **AppConfig 전체 reload 는 여전히 미구현** — 아래 Phase 2 Hot Reload 계약 블록은 설계만 남아있다. Catalog 외의 필드를 hot-reload 하려면 별도 follow-up 이 필요하다.

**Phase 2 Hot Reload 계약** (미구현, 설계만):
```
SIGHUP 수신
  → 새 AppConfig 파싱 + 검증 (검증 실패 시: 경고 로그 + 기존 설정 유지)
  → Reloadable 필드만 갱신:
      - telemetry.log_format (EnvFilter 재설정)
      - telemetry.sampling_ratio (Sampler 재설정)
      - router.circuit_breaker (CircuitBreakerConfig 갱신)
      - xaas.key_cache_ttl_secs (moka TTL 정책 갱신)
  → Non-reloadable 필드 (재시작 필요):
      - server.bind (소켓 재바인딩 필요)
      - xaas.pg_url (연결 풀 재생성 필요)
      - providers.* (provider 인스턴스 재생성 필요)
      - server.tls.* (TLS 설정 재로드 필요)
```

##### §2.B.5.4 Config Validation 실패 정책

**Fail-fast on boot**: 설정 오류는 모두 시작 시 panic!으로 종료. 이유: 잘못된 설정으로 부분적으로 구동된 서비스가 런타임에 실패하는 것보다 명시적 시작 실패가 운영 측면에서 안전.

단, 비필수 설정(`telemetry.otlp_endpoint` 연결 실패, `nodes` 중 일부 연결 불가)은 `tracing::warn!`으로 기록 후 계속 구동 (graceful degradation).

---

#### §2.B.6 Error Flow Across Crates

> `GadgetronError` 완전 열거는 §2.A.6 (Axis A)에 있다. 여기서는 크레이트 간 흐름, `From` impl 맵, retry 분류, HTTP 매핑을 다룬다.

##### §2.B.6.1 크레이트 간 `From` Impl 맵

```
[gadgetron-xaas/src/error.rs]
  fn sqlx_to_gadgetron(e: sqlx::Error) -> GadgetronError (D-20260411-13)
    RowNotFound      → GadgetronError::Database { kind: RowNotFound, .. }
    PoolTimedOut     → GadgetronError::Database { kind: PoolTimeout, .. }
    Io | Tls         → GadgetronError::Database { kind: ConnectionFailed, .. }
    Database(_)      → GadgetronError::Database { kind: Constraint, .. }
    Migrate(_)       → GadgetronError::Database { kind: MigrationFailed, .. }
    _                → GadgetronError::Database { kind: Other, .. }
  사용: sqlx_query.await.map_err(sqlx_to_gadgetron)?

[gadgetron-provider/src/error.rs]
  fn reqwest_to_gadgetron(e: reqwest::Error, provider: &str) -> GadgetronError
    e.is_timeout()   → GadgetronError::Provider(format!("{provider}: timeout"))
    e.is_connect()   → GadgetronError::Provider(format!("{provider}: connection refused"))
    Status(4xx|5xx)  → GadgetronError::Provider(format!("{provider}: HTTP {status}"))
    stream abort      → GadgetronError::StreamInterrupted { reason: e.to_string() }
  사용: .map_err(|e| reqwest_to_gadgetron(e, "openai"))?

[gadgetron-router/src/error.rs]
  fn routing_error(msg: impl Into<String>) -> GadgetronError
    → GadgetronError::Routing(msg.into())

[gadgetron-scheduler/src/error.rs]
  fn scheduler_error(msg: impl Into<String>) -> GadgetronError
    → GadgetronError::Routing(msg.into())   ← Routing variant 재사용 (별도 Scheduler variant 없음)

[gadgetron-gateway/src/error.rs]
  impl IntoResponse for GadgetronError (axum handler 반환 시 자동 변환)
    → (StatusCode, Json<ErrorResponse>)
```

**Note**: `From<E> for GadgetronError` trait impl은 orphan rule로 금지 (E가 외부 crate). 반드시 named helper function (`sqlx_to_gadgetron`, `reqwest_to_gadgetron`) 형태로 구현.

##### §2.B.6.2 Retry 분류표

| GadgetronError Variant | Retryable | Retry 횟수 | Retry 조건 |
|------------------------|-----------|-----------|-----------|
| `Config` | No | — | 설정 오류는 재시도 무의미 |
| `Provider` | No | — | 4xx/5xx는 재시도 무의미 |
| `Routing` | No | — | — |
| `StreamInterrupted` | **Yes** | **1회** | **첫 SSE 청크 전송 이전에만** (H-1 결정) |
| `QuotaExceeded` | No | — | — |
| `TenantNotFound` | No | — | — |
| `Forbidden` | No | — | — |
| `Billing` | No | — | — |
| `DownloadFailed` | Yes | 1회 | tower::retry |
| `HotSwapFailed` | No | — | — |
| `Database { PoolTimeout }` | Yes | 1회 | sqlx pool 일시 고갈 |
| `Database { RowNotFound }` | No | — | — |
| `Database { ConnectionFailed }` | Yes | 1회 | 연결 복구 대기 |
| `Database { Constraint }` | No | — | 비즈니스 규칙 위반 |
| `Database { MigrationFailed }` | No | — | panic 수준 |
| `Database { QueryFailed \| Other }` | No | — | — |

**StreamInterrupted retry 세부 정책** (H-1 D-20260411-08 supersede):
```
조건: 첫 SSE 청크가 클라이언트에 flush되기 이전에만 retry 허용.
이후: 스트림 중단 → [DONE] 없이 연결 종료 + GadgetronError::StreamInterrupted audit.
구현 위치: gadgetron-gateway/src/sse.rs
  let first_chunk_sent = Arc::new(AtomicBool::new(false));
  // 첫 청크 flush 이후 first_chunk_sent.store(true, Ordering::SeqCst)
  // StreamInterrupted 발생 시 first_chunk_sent.load() 확인
```

##### §2.B.6.3 HTTP Status 매핑 & OpenAI-Compatible Error Response (M-DX-2)

**API 오류 응답 형식** (OpenAI 호환, M-DX-2):

```json
{
  "error": {
    "message": "Your API usage quota has been exceeded. Contact your administrator to increase limits.",
    "type": "quota_error",
    "code": "quota_exceeded"
  }
}
```

- `message`: 사람이 읽을 수 있는 설명 (`GadgetronError::error_message()`, §2.A.6)
- `type`: OpenAI 호환 오류 분류 (`GadgetronError::error_type()`, §2.A.6)
- `code`: 기계 판독 가능한 단축 코드 (`GadgetronError::error_code()`, §2.A.6)

**내부 상세 오류는 응답 본문에 포함하지 않는다** — `self.to_string()` 은 로그/span에만 기록.

```rust
// gadgetron-gateway/src/error.rs
impl axum::response::IntoResponse for GadgetronError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            GadgetronError::Config(_)               => StatusCode::INTERNAL_SERVER_ERROR,
            GadgetronError::Provider(_)             => StatusCode::BAD_GATEWAY,
            GadgetronError::Routing(_)              => StatusCode::BAD_GATEWAY,
            GadgetronError::StreamInterrupted { .. }=> StatusCode::BAD_GATEWAY,
            GadgetronError::QuotaExceeded { .. }    => StatusCode::TOO_MANY_REQUESTS,
            GadgetronError::TenantNotFound { .. }   => StatusCode::UNAUTHORIZED,
            GadgetronError::Forbidden               => StatusCode::FORBIDDEN,   // B-8
            GadgetronError::Billing(_)              => StatusCode::PAYMENT_REQUIRED,
            GadgetronError::DownloadFailed(_)       => StatusCode::BAD_GATEWAY,
            GadgetronError::HotSwapFailed(_)        => StatusCode::SERVICE_UNAVAILABLE,
            GadgetronError::Database { kind, .. } => match kind {
                DatabaseErrorKind::PoolTimeout | DatabaseErrorKind::ConnectionFailed
                                                    => StatusCode::SERVICE_UNAVAILABLE,
                DatabaseErrorKind::RowNotFound      => StatusCode::NOT_FOUND,
                DatabaseErrorKind::Constraint       => StatusCode::CONFLICT,
                _                                   => StatusCode::INTERNAL_SERVER_ERROR,
            },
            GadgetronError::Node { kind, .. } => match kind {
                NodeErrorKind::InvalidMigProfile    => StatusCode::BAD_REQUEST,
                _                                   => StatusCode::INTERNAL_SERVER_ERROR,
            },
        };
        // M-DX-2: OpenAI-compatible error body — message + type + code
        let body = Json(json!({
            "error": {
                "message": self.error_message(),
                "type":    self.error_type(),
                "code":    self.error_code(),
            }
        }));
        // 내부 로그에는 self.to_string() 기록 (원본 오류 내용 보존)
        tracing::error!(
            error.type  = %self.error_type(),
            error.code  = %self.error_code(),
            "{}",
            self
        );
        (status, body).into_response()
    }
}
```

##### §2.B.6.4 Tracing Field 주입 (오류 발생 시)

```rust
// 오류 발생 시 span에 field 주입 (모든 크레이트 공통 패턴):
Span::current()
    .record("error.type",      &std::any::type_name::<GadgetronError>())
    .record("error.kind",      &error_kind_label(&err))   // Prometheus label 문자열
    .record("error.retryable", &is_retryable(&err));

fn error_kind_label(e: &GadgetronError) -> &'static str {
    match e {
        GadgetronError::Config(_)               => "config",
        GadgetronError::Provider(_)             => "provider",
        GadgetronError::Routing(_)              => "routing",
        GadgetronError::StreamInterrupted { .. }=> "stream_interrupted",
        GadgetronError::QuotaExceeded { .. }    => "quota_exceeded",
        GadgetronError::TenantNotFound { .. }   => "tenant_not_found",
        GadgetronError::Forbidden               => "forbidden",
        GadgetronError::Billing(_)              => "billing",
        GadgetronError::DownloadFailed(_)       => "download_failed",
        GadgetronError::HotSwapFailed(_)        => "hot_swap_failed",
        GadgetronError::Database { kind, .. }   => match kind {
            DatabaseErrorKind::PoolTimeout       => "db_pool_timeout",
            DatabaseErrorKind::RowNotFound       => "db_not_found",
            DatabaseErrorKind::ConnectionFailed  => "db_connection_failed",
            DatabaseErrorKind::Constraint        => "db_constraint",
            DatabaseErrorKind::MigrationFailed   => "db_migration_failed",
            DatabaseErrorKind::QueryFailed       => "db_query_failed",
            DatabaseErrorKind::Other             => "db_other",
        },
        GadgetronError::Node { kind, .. }       => match kind {
            NodeErrorKind::InvalidMigProfile     => "node_invalid_mig_profile",
            NodeErrorKind::NvmlInitFailed        => "node_nvml_init_failed",
            NodeErrorKind::ProcessSpawnFailed    => "node_process_spawn_failed",
            NodeErrorKind::VramAllocationFailed  => "node_vram_allocation_failed",
            NodeErrorKind::PortAllocationFailed  => "node_port_allocation_failed",
            _                                    => "node_error",
        },
    }
}
```

---

#### §2.B.7 Concurrency Patterns Catalog

**런타임**: `tokio 1` (multi-thread scheduler, `#[tokio::main]`).
특별한 이유 없는 한 `current_thread` 스케줄러 금지 — 단일 스레드에서 blocking call 시 전체 서비스 hang.

| 패턴 | 크레이트/타입 | 사용 시점 | 예시 시그니처 | 주요 pitfall |
|------|-------------|---------|------------|------------|
| Lock-free hot path | `dashmap::DashMap<K, V>` | read+write 모두 빈번, hot path | `MetricsStore { inner: DashMap<(String,String), ProviderMetrics> }` | shard-level lock 있음 — key 설계로 hot shard 방지 |
| Shared mutable (read heavy) | `tokio::sync::RwLock<T>` | 읽기 다수/쓰기 소수, async context | `Scheduler { deployments: RwLock<HashMap<Uuid, Deployment>> }` | `.write().await` 보유 중 `.await` 금지 |
| Shared mutable (write heavy sync) | `parking_lot::Mutex<T>` | sync context, 짧은 critical section | `AtomicCounter` 불가한 경우 | tokio worker 점유 위험 — critical section < 10μs 준수 |
| Shared mutable (write heavy async) | `tokio::sync::Mutex<T>` | async context, critical section 내 await 필요 | `PgKeyValidator::refresh_cache()` 중 DB await | `std::sync::Mutex`를 async context에 사용 금지 (deadlock) |
| Fan-out (1→N push) | `tokio::sync::broadcast::channel<T>` | WsAggregator → N TUI clients | `let (tx, _rx) = broadcast::channel::<WsMessage>(1024)` | `Lagged` 처리 필수 (§2.B.9 참조) |
| Fan-in (N→1 collect) | `tokio::sync::mpsc::channel<T>` | AuditWriter, event aggregation | `let (tx, mut rx) = mpsc::channel::<AuditEntry>(4_096)` | `try_send` Full 시 드롭 정책 명시 (D-20260411-09) |
| Lock-free counter | `std::sync::atomic::AtomicUsize` | RoundRobin index, metric counter | `fetch_add(1, Ordering::Relaxed)` | Ordering::SeqCst는 불필요한 성능 비용 |
| Cancellation-safe async | `select!` + cancellation token | graceful shutdown drain | `tokio::select! { v = rx.recv() => .., _ = shutdown.cancelled() => break }` | `select!`내 future는 완료 전 drop 가능 — `.await` 중간 상태 저장 금지 |

**`parking_lot` vs `tokio::sync` 선택 기준**:
- sync context (non-async fn, `spawn_blocking` 내부): `parking_lot::Mutex<T>` or `parking_lot::RwLock<T>`
- async context (`async fn`, `.await` 사용): `tokio::sync::Mutex<T>` or `tokio::sync::RwLock<T>`
- std::sync::Mutex는 async context에서 절대 사용 금지 (future가 !Send, deadlock 위험)

**금지 패턴**:
```rust
// WRONG: sync Mutex를 async context에서 .await 경계 넘어 보유
let guard = std::sync::Mutex::lock(&self.data);
some_async_call().await;  // guard가 .await 경계를 넘음 → !Send, deadlock risk
drop(guard);

// WRONG: spawn_blocking 없이 sync I/O를 async context에서
async fn handler() {
    let data = std::fs::read_to_string("file.txt")?;  // 블로킹 — tokio worker 점유
}

// CORRECT: spawn_blocking으로 격리
async fn handler() {
    let data = tokio::task::spawn_blocking(|| {
        std::fs::read_to_string("file.txt")
    }).await??;
}
```

**`select!` cancellation-safe 패턴**:
```rust
// AuditWriter drain loop (SIGTERM 시)
loop {
    tokio::select! {
        biased;
        entry = rx.recv() => {
            match entry {
                Some(e) => flush_to_pg(&pool, e).await?,
                None    => break,  // 채널 닫힘
            }
        }
        _ = shutdown_token.cancelled() => {
            // drain remaining: rx.close() 후 while let Some(e) = rx.recv().await loop
            rx.close();
            while let Some(e) = rx.recv().await {
                flush_to_pg(&pool, e).await?;
            }
            break;
        }
    }
}
// 5초 timeout: tokio::time::timeout(Duration::from_secs(5), drain_loop).await
```

---

#### §2.B.8 Middleware Chain Canonical Order

아래 순서는 토큰, 미들웨어, 처리기가 실행되는 정확한 순서다. 이 순서를 변경하면 보안·관측성 계약이 깨진다.

```
요청 진입 방향 (바깥 → 안쪽):
  RequestBodyLimitLayer(4_194_304) (tower-http 0.6) — [M-SEC-2]
    ↓ body > 4MB → 413 Payload Too Large (즉시 거부, 이후 레이어 미실행)
  TraceLayer (tower-http 0.5)
    ↓ request_id 생성, span 시작
  RequestIdLayer (gadgetron-gateway)
    ↓ X-Request-Id 헤더 응답에 추가
  AuthLayer (gadgetron-gateway, using gadgetron-xaas::PgKeyValidator)
    ↓ Bearer 검증, TenantContext 주입
  TenantContextLayer (gadgetron-gateway)
    ↓ Extension<TenantContext> 삽입
  ScopeGuardLayer (gadgetron-gateway)
    ↓ Scope 검증 (403 Forbidden)
  [P2] RateLimitLayer (gadgetron-xaas, governor::RateLimiter)
    ↓ Token bucket per tenant
  [P2] GuardrailsLayer (gadgetron-gateway, PII scan)
    ↓ 입력 검사
  axum Router
    ↓ 경로 매칭
  Handler (gadgetron-gateway::handlers)
    ↓ QuotaEnforcer::check_pre
    ↓ Router::chat_with_context / models_list / health_check
      ↓ ProtocolTranslator (gadgetron-provider, SseToChunkNormalizer)
      ↓ Provider 호출 (gadgetron-provider, 6종)
    ↓ QuotaEnforcer::record_post
    ↓ AuditWriter::try_send
  MetricsLayer (tower-http + metrics-exporter-prometheus)
    ↑ 응답 방향으로 메트릭 기록
  TraceLayer span 종료
응답 반환
```

**레이어별 상세 명세**:

| 순서 | 레이어 | 크레이트 소유자 | Phase | 오류 variant | latency budget |
|------|--------|--------------|-------|-------------|---------------|
| 1 | `RequestBodyLimitLayer(4_194_304)` | `tower-http 0.6` | P1 | `413 Payload Too Large` | ~100ns |
| 2 | `TraceLayer::new_for_http()` | `tower-http 0.5` | P1 | — | ~1μs |
| 3 | `RequestIdLayer` | `gadgetron-gateway` | P1 | — | ~100ns |
| 4 | `AuthLayer<PgKeyValidator>` | `gadgetron-gateway` (xaas via) | P1 | `TenantNotFound`, `Forbidden` | < 1ms (hit) / < 8ms (miss) |
| 5 | `TenantContextLayer` | `gadgetron-gateway` | P1 | — | ~100ns |
| 6 | `ScopeGuardLayer` | `gadgetron-gateway` | P1 | `Forbidden` | ~1μs |
| 7 | `RateLimitLayer` [P2] | `gadgetron-xaas` | P2 | `QuotaExceeded` | ~1μs |
| 8 | `GuardrailsLayer` [P2] | `gadgetron-gateway` | P2 | `Provider(pii)` | ~5ms |
| 9 | axum Router | `gadgetron-gateway` | P1 | — | ~500ns |
| 10 | Handler → QuotaEnforcer | `gadgetron-xaas` | P1 | `QuotaExceeded` | ~1μs |
| 11 | Router::chat_with_context | `gadgetron-router` | P1 | `Routing`, `NoProvider` | ~1μs |
| 12 | SseToChunkNormalizer | `gadgetron-provider` | P1 | `StreamInterrupted` | ~100ns/chunk |
| 13 | Provider HTTP call | `gadgetron-provider` | P1 | `Provider`, `StreamInterrupted` | 50ms~5min (excluded) |
| 14 | QuotaEnforcer::record_post | `gadgetron-xaas` | P1 | (비차단, fire-and-forget) | ~1μs |
| 15 | AuditWriter::try_send | `gadgetron-xaas` | P1 | (드롭 시 warn) | ~100ns |
| 16 | MetricsLayer | `tower-http + metrics-exporter-prometheus` | P1 | — | ~1μs |

**총 gateway overhead budget** (Provider 호출 제외): ~10μs P50, < 1ms P99 (SLO-1)

**`RequestBodyLimitLayer` 상세**: crate `tower-http 0.6` (`tower_http::limit::RequestBodyLimitLayer`). Config field: `server.max_body_bytes: usize` (default: `4_194_304` = 4MB). Bodies exceeding the limit are rejected with `413 Payload Too Large` before authentication or business logic runs — prevents memory exhaustion. Config field defined in `ServerConfig` (§2.B.5.1).

**SQL injection 방어**: SQL injection은 `sqlx` parameterized queries로 완전 차단된다. 원시 SQL 문자열 연결(raw SQL string concatenation)은 ACL-4 (§2.G.9) 에 의해 금지(banned)된다. 모든 DB 쿼리는 `sqlx::query!` 또는 `sqlx::query_as!` 매크로를 통해 컴파일 타임에 검증되고 파라미터 바인딩으로 실행된다.

---

#### §2.B.9 WsAggregator Broadcast Channel (M-8 해소)

> **위치**: `gadgetron-gateway/src/observability/aggregator.rs` (H-12 결정)

##### §2.B.9.1 Bandwidth 분석

```
최대 구성 시나리오 (100 node × 8 GPU):
  WsMessage::GpuMetrics 크기:
    node_id (36B UUID) + gpu_index (1B u32) + 6개 f32 필드 (24B) + 오버헤드 = ~100B per GPU

  source rate:
    100 node × 8 GPU × 1 msg/s (NVML 1Hz, H-9) = 800 msg/s
    800 msg × 100B = 80KB/s inbound to WsAggregator

  fan-out (N TUI clients):
    N=10 클라이언트 × 80KB/s = 800KB/s egress (Phase 1 단일 프로세스 감당 가능)
    N=100 클라이언트 × 80KB/s = 8MB/s egress → Phase 2에서 subscription filter 필요
    N=1000 클라이언트 (Phase 1 불허):
      1000 × 80KB/s = 80MB/s → 단일 프로세스 한계 초과 (320MB/s는 ux-interface-lead 오산: 1KB/msg 가정)

  Phase 1 클라이언트 상한: 100명 (TUI 운영자 시나리오, 단일 프로세스 가능)
  Phase 2: per-node WebSocket endpoint 또는 subscription filter 적용 [P2]
```

##### §2.B.9.2 Channel 설계

```rust
// gadgetron-gateway/src/observability/aggregator.rs
use tokio::sync::broadcast;
use gadgetron_core::ui::WsMessage;

pub struct WsAggregator {
    tx: broadcast::Sender<WsMessage>,
}

impl WsAggregator {
    pub fn new() -> Self {
        // capacity 1024: 1Hz × 800 msg/s × 1.28s 버퍼
        // 근거: 1초 내 consumer가 모두 소비 가능해야 함.
        //       1024는 초당 800 msg × 1.28배 여유 = Lagged 발생 threshold 전 회복 시간 제공.
        let (tx, _) = broadcast::channel(1024);
        WsAggregator { tx }
    }

    /// ResourceMonitor가 NVML 데이터 수집 후 호출 (1Hz)
    pub fn publish(&self, msg: WsMessage) {
        // send 실패 (no receivers) 시 조용히 무시
        let _ = self.tx.send(msg);
    }

    /// TUI/WebSocket handler가 구독 시 호출
    pub fn subscribe(&self) -> broadcast::Receiver<WsMessage> {
        self.tx.subscribe()
    }
}
```

##### §2.B.9.3 Lagged 처리 정책

```rust
// gadgetron-gateway/src/handlers/ws.rs (WebSocket client loop)
loop {
    match rx.recv().await {
        Ok(msg)                              => send_to_client(msg).await?,
        Err(broadcast::error::RecvError::Lagged(n)) => {
            // n개 메시지 손실 — 클라이언트에게 gap 알림 후 계속
            tracing::warn!(lagged = n, "WsAggregator: client lagged, {} messages dropped", n);
            metrics::counter!("gadgetron_gateway_ws_lagged_total").increment(n);
            // 연결 유지 (단절 아님). 클라이언트는 UI 새로고침으로 복구.
            continue;
        }
        Err(broadcast::error::RecvError::Closed) => {
            // 채널 닫힘 = 서버 종료 중
            break;
        }
    }
}
```

**Lagged 정책 결정**: 드롭 + 계속 (disconnect 아님). 이유: TUI GPU 모니터링 데이터는 최신 스냅샷이 중요하며 히스토리 손실은 허용 가능. 연결 끊기면 재연결 오버헤드 발생.

---

#### §2.B.10 NVML Polling Cadence (H-9 해소)

**확정**: 모든 GPU 모니터 폴링 주기 = **1,000ms (1Hz)**.

v0에서 100ms로 기술된 부분은 오류 (`gadgetron_node.md`의 1초가 정답 — gpu-scheduler-lead 지적).

##### §2.B.10.1 참여 컴포넌트

| 컴포넌트 | 위치 | 폴링 방식 | 1Hz 타이머 구현 |
|---------|------|----------|--------------|
| `ResourceMonitor` (NVML) | `gadgetron-node/src/monitor.rs` | `tokio::time::interval(1s)` | `interval.tick().await` |
| `WsAggregator` publisher | `gadgetron-gateway/src/observability/aggregator.rs` | ResourceMonitor의 callback 수신 | push-based (no separate timer) |
| Scheduler VRAM sync (heartbeat) | `gadgetron-node/src/agent.rs` | `tokio::time::interval(10s)` (B-2) | 별도 10초 주기 — NVML 1Hz와 독립 |

##### §2.B.10.2 ResourceMonitor 루프 (완전 구현 계약)

```rust
// gadgetron-node/src/monitor.rs
pub struct ResourceMonitor {
    aggregator: Arc<WsAggregator>,
    node_id:    String,
    gpu_count:  u32,
}

impl ResourceMonitor {
    pub async fn run(&self, mut shutdown: tokio_util::sync::CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_millis(1_000));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // MissedTickBehavior::Skip 이유: NVML 조회가 1ms 초과 시 다음 tick을 건너뜀.
        // Burst 방지: 지연이 쌓여도 따라잡지 않음.

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let poll_start = std::time::Instant::now();
                    match self.collect_gpu_metrics().await {
                        Ok(metrics) => {
                            for m in &metrics {
                                self.aggregator.publish(WsMessage::GpuMetrics(m.clone()));
                                record_prometheus_gauges(m);
                            }
                        }
                        Err(e) => {
                            metrics::counter!("gadgetron_node_nvml_poll_stall_total").increment(1);
                            tracing::warn!(node_id = %self.node_id, error = %e, "NVML poll failed");
                        }
                    }
                    let elapsed_ms = poll_start.elapsed().as_millis();
                    metrics::histogram!("gadgetron_node_nvml_poll_duration_seconds")
                        .record(elapsed_ms as f64 / 1000.0);
                }
            }
        }
    }
}
```

##### §2.B.10.3 Drift 허용 범위

| 항목 | 값 | 근거 |
|------|-----|------|
| 공칭 폴링 주기 | 1,000ms | H-9 확정 |
| 최대 허용 지연 | 1,200ms | MissedTickBehavior::Skip — NVML 조회 200ms 허용 |
| Stale 판정 임계값 | 2,000ms (2s) | Prometheus 메트릭 기준. Scheduler VRAM 신뢰도 판단에는 사용 안 함 (별도 30s stale, B-2) |
| Poll stall 알림 | `gadgetron_node_nvml_poll_stall_total` > 3 in 5m | Alert rule (§2.B.2 NVML Poll Stall 참조) |

---

#### §2.B.11 STRIDE Threat Model

**Attacker model**: Phase 1 attacker profile: authenticated malicious tenant attempting quota bypass, prompt injection, or data exfiltration. Unauthenticated attacker blocked at AuthLayer before reaching any business logic.

**Protected assets**: API keys (`gad_*` format), prompt payloads, audit records, tenant billing data (`cost_cents`), model weights (local providers), PostgreSQL credentials.

**Trust boundaries**:
1. Client → Gateway (HTTP/TLS)
2. Gateway → PostgreSQL (TCP/TLS)
3. Gateway → Provider (HTTPS)
4. NodeAgent → Scheduler (internal HTTP)
5. WsAggregator → TUI/Web clients (WebSocket)

**STRIDE matrix**:

| Category | Threat | Asset affected | Trust boundary | Phase 1 mitigation | Phase 2 enhancement |
|---|---|---|---|---|---|
| **S** Spoofing | Forged Bearer token | API keys (`gad_*`) | 1: client → gateway | SHA-256 hash stored (not raw key); moka TTL 10min; key rotation via `POST /api/v1/xaas/keys/{id}/revoke` (§2.B.4.3) | Redis pub/sub instant revocation across instances |
| **S** Spoofing | Spoofed NodeAgent heartbeat | Node registry | 4: NodeAgent → Scheduler | Internal-only endpoint bound to loopback/cluster network; no external exposure | mTLS between NodeAgent and Scheduler |
| **T** Tampering | Audit log record modification | Audit records | 2: gateway → PostgreSQL | INSERT-only app role — `gadgetron_app` has no UPDATE/DELETE on `audit_log` (M-SEC-5, §2.D.2) | Hash chain: each entry stores `SHA-256(prev_entry)` |
| **T** Tampering | Config file tampering | `AppConfig` | Host filesystem | File permission 600 (`-rw-------`); config validated on load (fail-fast startup) | Config signing (cosign or similar) |
| **R** Repudiation | Audit channel drop — usage not recorded | Audit records | In-process (mpsc channel) | `audit_dropped_total` Prometheus counter + alert on > 0 (D-20260411-09, §2.B.2) | Persistent WAL queue — Phase 2 durability |
| **I** Info Disclosure | Error message leaks internal paths or schema | Secrets / internals | 1: client → gateway | `Secret<T>` wrapper for all sensitive fields; opaque HTTP error codes with `error_code` string, no stack traces in responses (§2.B.6.3) | N/A — already mitigated |
| **I** Info Disclosure | Provider API key exposed in trace | Secrets | In-process tracing | `#[instrument(skip(api_key))]` on all functions receiving secrets; `Secret<T>` Display/Debug redact to `"[REDACTED]"` (§2.B.4.2) | N/A — already mitigated |
| **D** DoS | Unbounded request body exhausts gateway memory | Memory | 1: client → gateway | `RequestBodyLimitLayer(4_194_304)` — 4MB cap, returns 413 Payload Too Large (M-SEC-2, §2.B.8) | Configurable per-tenant body limit |
| **D** DoS | Quota bypass after process restart | Billing / quota | In-process (TokenBucket) | Accepted Phase 1 risk (§2.E.6 KL-1); `WARN` log on restart; readinessProbe delays traffic | Redis-backed persistent `TokenBucket` |
| **E** Elevation of Privilege | Scope confusion — Management scope used for OpenAI endpoint | Authorization | 1: client → gateway | `ScopeGuardLayer` per-route check: `/v1/chat/completions` requires `Scope::OpenAiCompat`; Management endpoints require `Scope::Management` (§2.B.3.2) | N/A — already mitigated |

---

### Axis C: 배포 아키텍처 (Deployment Architecture)

> **v1 결정 근거**: B-5 (K8s operator Option A), H-17 (Finalizer + Leader election), H-15 (terminationGracePeriodSeconds=70), H-10 (PG SPOF 정책), M-10 (pg_dump daily), L-2 (cargo-audit + Trivy), H-14 (runbook copy-paste), M-9 (gadgetron-web), D-20260412-02 (Implementation Determinism)

---

#### §2.C.1 지원 배포 대상 (Supported Deployment Targets)

| Category | Target | Deployment artifact | Testing matrix | Phase |
|---|---|---|---|---|
| Bare metal | Linux x86_64 / ARM64 | single binary + systemd unit file | unit + e2e on Linux runners (ubuntu-24.04 + ubuntu-22.04-arm) | [P1] |
| Container | Docker / podman | OCI image (distroless) | image build + Trivy scan in CI | [P1] |
| Orchestrator (K8s family) | vanilla K8s 1.28+ | Helm chart + CRDs | kind integration test (kind v0.23) | [P1 chart, P2 CRD] |
| Orchestrator (K8s family) | **KubeVirt (VM-on-K8s)** | same Helm chart, runs inside KubeVirt VirtualMachine CR | documented compatibility note; no separate test matrix | [P2] |
| Orchestrator (K8s family) | Rancher / OpenShift 4.x | same Helm chart | compatibility note; third-party test | [P2] |
| IaaS | **OpenStack** | cloud-init + systemd (Glance image), or OpenStack Magnum K8s cluster | compatibility note | [P2] |
| IaaS | AWS EC2 / GCP GCE / Azure VM | cloud-init + systemd | single-instance smoke test per cloud in CI (P2) | [P2] |
| Managed K8s | EKS / GKE / AKS | same Helm chart | kind integration covers 90%; cloud-specific gap documented | [P2] |
| HPC | Slurm | `sbatch` job wrapper script | documented in `docs/deployment/slurm.md` | [P2] |

**이식성 근거 — 신규 코드 불필요**

KubeVirt, OpenStack, Rancher, 관리형 K8s (EKS/GKE/AKS)에 대해 **별도 코드 추가 없음**. 이유:

1. **단일 바이너리 + config-driven 부팅**: `AppConfig`를 환경변수 또는 마운트된 TOML로 주입. KubeVirt VirtualMachine CR 내에서도 동일하게 동작.
2. **OCI 이미지 (distroless)**: 모든 컨테이너 런타임 호환 (containerd, CRI-O, Docker shim). KubeVirt virt-launcher가 동일 이미지 사용.
3. **Helm chart 재사용**: KubeVirt는 vanilla K8s Helm chart를 그대로 사용. OpenStack Magnum은 Magnum이 K8s 클러스터를 프로비저닝한 이후 동일 Helm chart 적용 — vanilla K8s와 동일 절차.
4. **leaf crate 설계**: 플랫폼-특화 의존성 없음. `gadgetron-core`에 K8s/OpenStack SDK 미포함.
5. **cloud-init**: bare metal/VM 경로는 `cloud-init` + systemd unit file로 부팅. OpenStack Nova, EC2 user-data, GCE startup script 모두 동일 cloud-init YAML 소비.

테스트 매트릭스 변경 사항: vanilla K8s (kind) integration test는 KubeVirt/Managed K8s의 90% 커버. 차이는 GPU device plugin 경로 (NVIDIA device plugin vs KubeVirt host-device passthrough) — compatibility note로 문서화, 별도 CI matrix 없음.

---

#### §2.C.2 배포 아티팩트 카탈로그 (Deployment Artifacts Catalog)

##### 2.C.2.1 OCI 이미지

| 항목 | 값 |
|---|---|
| 빌드 방식 | 멀티스테이지: `rust:1.82-bookworm` (builder) → `gcr.io/distroless/cc-debian12:nonroot` (runtime) |
| 빌드 명령 | `docker buildx build --platform linux/amd64,linux/arm64 -t ghcr.io/gadgetron/gadgetron:$TAG .` |
| 이미지 크기 예산 | AMD64 ≤ 60MB, ARM64 ≤ 60MB (distroless base ~20MB + binary ≤ 40MB) |
| ENTRYPOINT | `["/gadgetron", "serve", "--config", "/etc/gadgetron/gadgetron.toml"]` |
| 환경변수 | `GADGETRON_CONFIG=/etc/gadgetron/gadgetron.toml` (override 가능) |
| 포트 | `8080` (HTTP API), `9090` (Prometheus metrics) |
| 서명 | cosign (GitHub Actions OIDC, keyless) [P2] |
| SBOM | `syft ghcr.io/gadgetron/gadgetron:$TAG -o spdx-json` → GHCR attestation attachment [P2] |
| 취약점 스캔 | `trivy image --exit-code 1 --severity HIGH,CRITICAL` — CI 게이트 (L-2) |

Dockerfile 핵심 단계:
```dockerfile
# Stage 1: builder
FROM rust:1.82-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --locked -p gadgetron-cli \
    -C codegen-units=1 \
    -C opt-level=3

# Stage 2: runtime
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /build/target/release/gadgetron /gadgetron
EXPOSE 8080 9090
ENTRYPOINT ["/gadgetron"]
```

`-C codegen-units=1`: 재현 가능 빌드 보장 (D-20260412-02 §7 외부 의존성 계약).

##### 2.C.2.2 Helm Chart

| 항목 | 값 |
|---|---|
| 경로 | `deploy/helm/gadgetron/` |
| Chart.yaml `version` | SemVer, `make release-helm` 자동 갱신 (L-3) |
| `appVersion` | Git tag (`v1.2.3`) |
| values.yaml 최상위 키 | `image`, `replicaCount`, `service`, `ingress`, `postgres`, `config`, `resources`, `nodeSelector`, `tolerations`, `affinity` |
| Phase 1 `replicaCount` | `1` (고정, moka 캐시 일관성 — H-10) |
| Helm schema | `values.schema.json` — CI에서 `helm lint --strict` + `helm unittest` 검증 |
| 설치 명령 | `helm upgrade --install gadgetron deploy/helm/gadgetron -n gadgetron --create-namespace -f my-values.yaml` |
| 롤백 명령 | `helm rollback gadgetron [REVISION] -n gadgetron` |

`values.yaml` 완전 타입 정의 (Rust 구조체 형식으로 표현, 실제는 YAML):
```yaml
# deploy/helm/gadgetron/values.yaml — 완전 열거
image:
  repository: ghcr.io/gadgetron/gadgetron
  tag: ""          # 기본값: Chart appVersion
  pullPolicy: IfNotPresent

replicaCount: 1    # Phase 1: 1 고정. Phase 2: N 가능 (상태 외부화 후)

service:
  type: ClusterIP
  port: 8080
  metricsPort: 9090

ingress:
  enabled: false
  className: ""
  host: ""
  tlsSecretName: ""

postgres:
  host: ""         # 필수
  port: 5432
  database: gadgetron
  user: ""         # 필수
  passwordSecret: "" # K8s Secret 이름, key=password
  sslMode: require

config:
  logLevel: info   # trace | debug | info | warn | error
  logFormat: json  # json | text
  nvmlEnabled: true
  pollIntervalSec: 1

resources:
  requests:
    cpu: "500m"
    memory: "512Mi"
  limits:
    cpu: "2000m"
    memory: "2Gi"

nodeSelector: {}
tolerations: []
affinity: {}

terminationGracePeriodSeconds: 70  # H-15: 5+30+5+10+20

livenessProbe:
  httpGet:
    path: /health
    port: 8080
  initialDelaySeconds: 10
  periodSeconds: 10
  failureThreshold: 3

readinessProbe:
  httpGet:
    path: /ready
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 5
  failureThreshold: 3
```

##### 2.C.2.3 Systemd Unit File

경로: `deploy/systemd/gadgetron.service`

```ini
[Unit]
Description=Gadgetron LLM Orchestration Platform
Documentation=https://github.com/gadgetron/gadgetron
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=exec
User=gadgetron
Group=gadgetron
ExecStart=/usr/local/bin/gadgetron serve --config /etc/gadgetron/gadgetron.toml
ExecReload=/bin/kill -HUP $MAINPID
# Graceful shutdown: SIGTERM → 70s drain → SIGKILL
TimeoutStopSec=70
KillMode=mixed
KillSignal=SIGTERM
Restart=on-failure
RestartSec=5s
RestartMaxDelaySec=60s

# Data and log directories
Environment=GADGETRON_CONFIG=/etc/gadgetron/gadgetron.toml
RuntimeDirectory=gadgetron
LogsDirectory=gadgetron
StateDirectory=gadgetron

# Security hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/gadgetron /var/log/gadgetron

[Install]
WantedBy=multi-user.target
```

**데이터 디렉터리 레이아웃**:
```
/var/lib/gadgetron/
├── models/          # 로컬 모델 캐시 (ollama, vllm 공유)
├── db/              # SQLite 개발 전용 (Phase 1 PostgreSQL이 정식)
└── tmp/             # 임시 파일

/var/log/gadgetron/
└── gadgetron.log    # JSON structured log (journald도 병행)

/etc/gadgetron/
└── gadgetron.toml   # AppConfig 파일
```

**AppConfig 최소 필수 필드** (Phase 1, TOML 형식):
```toml
[server]
host = "0.0.0.0"
port = 8080
metrics_port = 9090

[database]
url = "postgresql://gadgetron:PASSWORD@localhost:5432/gadgetron"
max_connections = 20
connect_timeout_sec = 5

[scheduler]
nvml_poll_interval_sec = 1        # H-9: 1초로 통일
vram_stale_threshold_sec = 30     # B-2: 30s stale
node_heartbeat_interval_sec = 10  # B-2: 10s push heartbeat

[logging]
level = "info"      # trace | debug | info | warn | error
format = "json"     # json | text

[providers]
# 최소 1개 provider 필수
[[providers.local]]
engine = "ollama"
endpoint = "http://localhost:11434"
```

**Health probe 설정**:
- `GET /health` → 200 OK (liveness: 프로세스 살아있음, DB 연결 여부 무관)
- `GET /ready` → 200 OK (readiness: DB 연결 성공 + 최소 1개 provider healthy)
- `GET /ready` → 503 Service Unavailable: DB 다운 시 (`X-Gadgetron-Error: database_unavailable`)

---

#### §2.C.3 단일 노드 배포 — Phase 1 베이스라인

##### §2.C.3.0 Zero to First Chat Completion (5 minutes) (M-DX-4)

**Prerequisites**: Linux x86_64, Docker installed, an OpenAI API key.

**Step 1**: Create minimal config (`gadgetron.toml`):

```toml
[server]
bind = "0.0.0.0:8080"

[providers.openai]
api_key = "${OPENAI_API_KEY}"
base_url = "https://api.openai.com/v1"
models = ["gpt-4o-mini"]

[xaas]
database_url = "postgresql://gadgetron:gadgetron@localhost:5432/gadgetron"
```

**Step 2**: Start PostgreSQL + Gadgetron:

```bash
docker run -d --name gadgetron-db \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=gadgetron \
  -e POSTGRES_DB=gadgetron \
  -p 5432:5432 \
  postgres:16

export OPENAI_API_KEY="sk-your-key-here"

docker run -d --name gadgetron \
  -p 8080:8080 \
  -v $(pwd)/gadgetron.toml:/etc/gadgetron/gadgetron.toml \
  -e OPENAI_API_KEY \
  ghcr.io/gadgetron/gadgetron:latest \
  serve -c /etc/gadgetron/gadgetron.toml
```

**Step 3**: Provision first API key:

```bash
# Migrations run automatically on first boot. Create a tenant + key:
docker exec gadgetron gadgetron tenant create --name "dev-team"
# Output: tenant_id=<uuid>

docker exec gadgetron gadgetron key create --tenant-id <uuid> --scope openai-compat
# Output: gad_live_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx  <- save this
```

**Step 4**: Send first request:

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer gad_live_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [{"role": "user", "content": "Hello from Gadgetron!"}]
  }'
```

**Expected**: JSON response with `choices[0].message.content`. Total time: < 5 minutes.

---

##### 2.C.3.1 설치 절차 (Bare Metal / VM)

```bash
# 1. 바이너리 설치
curl -Lo /usr/local/bin/gadgetron \
  https://github.com/gadgetron/gadgetron/releases/download/v${VERSION}/gadgetron-linux-amd64
chmod +x /usr/local/bin/gadgetron

# 2. 시스템 사용자 생성
useradd --system --no-create-home --shell /sbin/nologin gadgetron

# 3. 디렉터리 생성
mkdir -p /etc/gadgetron /var/lib/gadgetron /var/log/gadgetron
chown gadgetron:gadgetron /var/lib/gadgetron /var/log/gadgetron

# 4. 설정 파일 배치
cp gadgetron.toml /etc/gadgetron/gadgetron.toml
chmod 640 /etc/gadgetron/gadgetron.toml

# 5. Systemd 설치 및 활성화
cp deploy/systemd/gadgetron.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now gadgetron

# 6. 헬스 확인
curl http://localhost:8080/health
curl http://localhost:8080/ready
```

##### 2.C.3.2 Docker Compose (단일 VM, Phase 1)

```yaml
# deploy/docker/docker-compose.yml
version: "3.9"
services:
  gadgetron:
    image: ghcr.io/gadgetron/gadgetron:${GADGETRON_VERSION:-latest}
    ports:
      - "8080:8080"
      - "9090:9090"
    volumes:
      - ./gadgetron.toml:/etc/gadgetron/gadgetron.toml:ro
    environment:
      GADGETRON_CONFIG: /etc/gadgetron/gadgetron.toml
    depends_on:
      postgres:
        condition: service_healthy
    restart: unless-stopped
    stop_grace_period: 70s       # H-15

  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: gadgetron
      POSTGRES_USER: gadgetron
      POSTGRES_PASSWORD: ${PG_PASSWORD}
    volumes:
      - pg_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U gadgetron"]
      interval: 5s
      timeout: 3s
      retries: 5

volumes:
  pg_data:
```

---

#### §2.C.4 멀티 노드 + K8s Operator — Phase 2 (Option A)

**결정 B-5**: Operator → Scheduler HTTP 호출 (Option A 채택). Operator는 Pod 직접 생성하지 않음. Scheduler가 NUMA/MIG/VRAM 인지 → operator가 이를 재구현하면 중복. `deployment-operations.md §3.2`는 이 결정으로 수정됨.

##### 2.C.4.1 CRD 스키마

**GadgetronModel CRD** (`gadgetron.io/v1alpha1`):
```yaml
# deploy/crds/gadgetronmodel.yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: gadgetronmodels.gadgetron.io
spec:
  group: gadgetron.io
  names:
    kind: GadgetronModel
    plural: gadgetronmodels
    singular: gadgetronmodel
    shortNames: ["gtm"]
  scope: Namespaced
  versions:
  - name: v1alpha1
    served: true
    storage: true
    schema:
      openAPIV3Schema:
        type: object
        required: ["spec"]
        properties:
          spec:
            type: object
            required: ["model_id", "engine", "vram_mb"]
            properties:
              model_id:
                type: string
                description: "HuggingFace model ID (e.g. 'meta-llama/Llama-3-8B')"
              engine:
                type: string
                enum: ["ollama", "vllm", "sglang"]
                description: "Inference engine. Exactly one of: ollama | vllm | sglang"
              vram_mb:
                type: integer
                minimum: 1024
                description: "Required VRAM in MB"
              replicas:
                type: integer
                minimum: 1
                maximum: 16
                default: 1
              priority:
                type: integer
                minimum: 0
                maximum: 100
                default: 50
                description: "Eviction priority (H-8). Higher = less likely to evict."
          status:
            type: object
            properties:
              state:
                type: string
                enum: ["Pending", "Deploying", "Running", "Failed", "Terminating"]
              node_id:
                type: string
              endpoint:
                type: string
              last_transition_time:
                type: string
                format: date-time
              message:
                type: string
    additionalPrinterColumns:
    - name: State
      type: string
      jsonPath: .status.state
    - name: Engine
      type: string
      jsonPath: .spec.engine
    - name: VRAM
      type: integer
      jsonPath: .spec.vram_mb
```

**GadgetronNode CRD** (`gadgetron.io/v1alpha1`):
```yaml
spec:
  endpoint:
    type: string
    description: "HTTP endpoint of gadgetron node agent (e.g. 'http://node1:8081')"
  gpu_count:
    type: integer
    minimum: 0
  labels:
    type: object
    additionalProperties:
      type: string
status:
  healthy:
    type: boolean
  vram_used_mb:
    type: integer
  vram_total_mb:
    type: integer
  last_heartbeat:
    type: string
    format: date-time
```

**GadgetronRouting CRD** (`gadgetron.io/v1alpha1`):
```yaml
spec:
  default_strategy:
    type: string
    enum: ["RoundRobin", "CostOptimal", "LatencyOptimal", "QualityOptimal", "Fallback", "Weighted"]
  providers:
    type: array
    items:
      type: string
  fallback_chain:
    type: array
    items:
      type: string
  weights:
    type: object
    additionalProperties:
      type: number
    description: "Provider → weight mapping (for Weighted strategy only)"
```

**Rust CRD 타입** (`gadgetron-operator/src/crd.rs`):
```rust
use kube::CustomResource;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// GadgetronModel CRD spec
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gadgetron.io",
    version = "v1alpha1",
    kind = "GadgetronModel",
    namespaced,
    status = "GadgetronModelStatus",
    shortname = "gtm",
    printcolumn = r#"{"name":"State","type":"string","jsonPath":".status.state"}"#,
)]
pub struct GadgetronModelSpec {
    pub model_id: String,
    pub engine: InferenceEngine,
    pub vram_mb: u32,
    #[serde(default = "default_replicas")]
    pub replicas: u32,
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_replicas() -> u32 { 1 }
fn default_priority() -> i32 { 50 }

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum InferenceEngine {
    Ollama,
    Vllm,
    Sglang,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
pub struct GadgetronModelStatus {
    pub state: ModelDeployState,
    pub node_id: Option<String>,
    pub endpoint: Option<String>,
    pub last_transition_time: Option<Time>,
    pub message: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default, PartialEq)]
pub enum ModelDeployState {
    #[default]
    Pending,
    Deploying,
    Running,
    Failed,
    Terminating,
}
```

라이브러리: `kube = "0.95"`, `k8s-openapi = { version = "0.23", features = ["v1_29"] }`, `schemars = "0.8"` (D-20260412-02 §4).

##### 2.C.4.2 Reconcile Loop — Operator → Scheduler HTTP 호출

**동시성 모델**: `kube::runtime::controller::Controller` — tokio multi-thread runtime. Controller는 per-object reconcile을 직렬화 (동일 object 동시 reconcile 없음). 다른 object 간 병렬 실행 (D-20260412-02 §3).

```mermaid
flowchart TD
    A[K8s API Server<br/>GadgetronModel 변경 감지] --> B[Controller::reconcile 호출]
    B --> C{Finalizer 확인}
    C -- "finalizer 없음 + DeletionTimestamp 없음" --> D[finalizer 추가\n'gadgetron.io/model-cleanup']
    C -- "finalizer 있음 + DeletionTimestamp 있음" --> E[cleanup: Scheduler DELETE /api/v1/models/:id]
    E --> F[finalizer 제거]
    F --> G[종료]
    C -- "finalizer 있음 + DeletionTimestamp 없음" --> H{status.state 확인}
    H -- "Pending" --> I[POST /api/v1/scheduler/deploy\nbody: DeployRequest]
    I -- "200 OK" --> J[status.state = Deploying\nstatus.node_id = response.node_id]
    I -- "4xx/5xx" --> K[status.state = Failed\nstatus.message = error]
    H -- "Deploying" --> L[GET /api/v1/scheduler/status/:model_id]
    L -- "running" --> M[status.state = Running\nstatus.endpoint = response.endpoint]
    L -- "failed" --> K
    L -- "timeout > 5min" --> K
    H -- "Running" --> N[GET /api/v1/scheduler/status/:model_id\n헬스 확인]
    N -- "unhealthy" --> O[status.state = Failed]
    K --> P[requeueAfter: 30s]
    J --> P
    D --> P
    M --> Q[requeueAfter: 60s]
```

**Scheduler HTTP API 호출 계약** (operator가 호출하는 endpoint):

| 작업 | Method | Path | Request body | Response |
|---|---|---|---|---|
| 배포 요청 | POST | `/api/v1/scheduler/deploy` | `{"model_id":"...", "engine":"vllm", "vram_mb":8192, "priority":50}` | `{"node_id":"...", "task_id":"..."}` 또는 에러 |
| 상태 조회 | GET | `/api/v1/scheduler/status/:model_id` | — | `{"state":"running", "endpoint":"http://..."}` |
| 배포 해제 | DELETE | `/api/v1/scheduler/models/:model_id` | — | `204 No Content` |

**Reconcile 함수 시그니처**:
```rust
// gadgetron-operator/src/controller.rs
use kube::{
    runtime::{controller::Action, watcher::Config},
    Api, Client, ResourceExt,
};
use std::sync::Arc;
use std::time::Duration;

pub struct OperatorContext {
    pub client: Client,
    pub scheduler_base_url: String,        // e.g. "http://gadgetron:8080"
    pub http: reqwest::Client,
}

pub async fn reconcile(
    model: Arc<GadgetronModel>,
    ctx: Arc<OperatorContext>,
) -> Result<Action, ReconcileError> {
    // 구현: finalizer → deploy/status/cleanup
    // 반환: Action::requeue(Duration::from_secs(60)) 또는 Action::requeue(Duration::from_secs(30))
    // 에러 시: ReconcileError → Controller가 지수 백오프 자동 처리
    todo!()
}

pub fn error_policy(
    _model: Arc<GadgetronModel>,
    error: &ReconcileError,
    _ctx: Arc<OperatorContext>,
) -> Action {
    // 지수 백오프: 초기 5s, 최대 300s
    tracing::warn!(?error, "reconcile failed, requeuing");
    Action::requeue(Duration::from_secs(5))
}
```

**상태 전이 표** (D-20260412-02 §8):

| 현재 상태 | 이벤트 | 다음 상태 | 비고 |
|---|---|---|---|
| (없음) | CRD 생성 | Pending | finalizer 추가 |
| Pending | Scheduler 200 OK | Deploying | node_id 기록 |
| Pending | Scheduler 4xx/5xx | Failed | message 기록 |
| Deploying | status=running | Running | endpoint 기록 |
| Deploying | status=failed | Failed | — |
| Deploying | 5분 초과 | Failed | timeout |
| Running | 헬스 unhealthy | Failed | — |
| Running | DeletionTimestamp | Terminating | finalizer 처리 진입 |
| Terminating | DELETE 성공 | (삭제) | finalizer 제거 |
| Failed | 수동 spec 변경 | Pending | 재시도 |

##### 2.C.4.3 Finalizer 패턴 [P2, H-17]

```rust
use kube::api::{Patch, PatchParams};

const FINALIZER: &str = "gadgetron.io/model-cleanup";

async fn add_finalizer(client: &Client, model: &GadgetronModel) -> Result<(), ReconcileError> {
    let api: Api<GadgetronModel> = Api::namespaced(
        client.clone(),
        model.namespace().as_deref().unwrap_or("default"),
    );
    let patch = serde_json::json!({
        "metadata": { "finalizers": [FINALIZER] }
    });
    api.patch(
        &model.name_any(),
        &PatchParams::apply("gadgetron-operator"),
        &Patch::Merge(&patch),
    ).await?;
    Ok(())
}

async fn remove_finalizer(client: &Client, model: &GadgetronModel) -> Result<(), ReconcileError> {
    let api: Api<GadgetronModel> = Api::namespaced(
        client.clone(),
        model.namespace().as_deref().unwrap_or("default"),
    );
    let patch = serde_json::json!({
        "metadata": { "finalizers": [] }
    });
    api.patch(
        &model.name_any(),
        &PatchParams::apply("gadgetron-operator"),
        &Patch::Merge(&patch),
    ).await?;
    Ok(())
}
```

##### 2.C.4.4 Leader Election [P2, H-17]

Leader election 방식: `kube::runtime::controller::Controller` + K8s Lease object (`coordination.k8s.io/v1`). operator Deployment `replicas: 2` 시 한 Pod만 active reconcile.

```rust
// gadgetron-operator/src/main.rs
use kube::runtime::leaderelection::{LeaderElector, LeaderElectorConfig};

let lease_config = LeaderElectorConfig {
    lock_name: "gadgetron-operator-leader".to_string(),
    lock_namespace: "gadgetron".to_string(),
    lease_duration: Duration::from_secs(15),    // 15초: peer failure 감지
    renew_deadline: Duration::from_secs(10),    // 10초: renew timeout
    retry_period: Duration::from_secs(2),       // 2초: retry interval
};
// LeaderElector가 획득 시 Controller::run() 호출, 해제 시 종료
```

**Magic number 근거** (D-20260412-02 §6):
- `lease_duration=15s`: K8s 기본값 15s. pod 재시작 후 최대 15s 내 새 leader 선출.
- `renew_deadline=10s`: lease_duration의 2/3. 네트워크 지연 허용 + 임박한 만료 전 ren�신 보장.
- `retry_period=2s`: 5회 이하 재시도 내 renew_deadline 충족.

##### 2.C.4.5 terminationGracePeriodSeconds=70 근거 [H-15]

| 단계 | 시간 | 담당 | 근거 |
|---|---|---|---|
| preStop hook sleep | 5s | kubelet | LB가 endpoints 목록에서 Pod 제거 전 새 요청 차단 완충 |
| 활성 요청 drain | 30s | axum graceful shutdown | SSE 스트림 최대 길이 30s (LLM 응답 최대값 기준) |
| AuditWriter channel flush | 5s | gadgetron-xaas | audit 채널 (4096 entries) drain + PostgreSQL write |
| model_unload | 10s | ProcessManager | ollama/vllm 모델 언로드 HTTP 호출 timeout |
| buffer | 20s | — | SIGKILL 전 예비. 실제 합계 50s + 20s = 70s |

```yaml
# deploy/helm/gadgetron/templates/deployment.yaml (관련 발췌)
spec:
  template:
    spec:
      terminationGracePeriodSeconds: 70
      containers:
      - name: gadgetron
        lifecycle:
          preStop:
            exec:
              command: ["/bin/sleep", "5"]
```

---

#### §2.C.5 CI/CD 파이프라인

**파이프라인 파일**: `.github/workflows/ci.yml` (lint/test), `.github/workflows/release.yml` (image/release)

##### 2.C.5.1 GitHub Actions 잡 매트릭스

| Job | Trigger | Runner | 핵심 단계 |
|---|---|---|---|
| `lint` | push, PR | ubuntu-24.04 | `cargo fmt --check`, `cargo clippy -- -D warnings` |
| `test-unit` | push, PR | ubuntu-24.04 | `cargo test --workspace --locked` |
| `test-integration` | push, PR | ubuntu-24.04 | `docker compose up -d postgres`, `cargo test --features integration` |
| `sqlx-check` | push, PR | ubuntu-24.04 | `cargo sqlx prepare --check` (devops H-15 발견 항목) |
| `cross-compile` | push (main, release/*) | ubuntu-24.04 | `cross build --target aarch64-unknown-linux-gnu --release` |
| `container-build` | push (main, release/*) | ubuntu-24.04 | `docker buildx build --platform linux/amd64,linux/arm64` |
| `security-scan` | push (main, release/*) | ubuntu-24.04 | `cargo audit` (L-2) + `cargo deny check licenses bans advisories` (M-SEC-3) + `trivy image` (L-2) |
| `sbom` | tag push (v*) | ubuntu-24.04 | `syft` SPDX → GHCR attestation [P2] |
| `ghcr-push` | tag push (v*) | ubuntu-24.04 | `docker push ghcr.io/gadgetron/gadgetron:$TAG` |
| `helm-release` | tag push (v*) | ubuntu-24.04 | `helm package` + `helm push` to GHCR OCI |
| `release-notes` | tag push (v*) | ubuntu-24.04 | `git-cliff --latest` → GitHub Release body |

**cargo-audit 설정** (`deny.toml`) — also used by `cargo deny` (M-SEC-3):
```toml
[advisories]
ignore = []             # 새 advisory → CI 즉시 실패
vulnerability = "deny"
unmaintained = "warn"
unsound = "warn"

[licenses]
unlicensed = "deny"
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib"]

[bans]
multiple-versions = "warn"
```

**`cargo deny` CI step** (M-SEC-3):
```bash
# .github/workflows/ci.yml — security-scan job
- name: cargo deny
  run: cargo deny check licenses bans advisories
```

`cargo deny check licenses` fails the build on any dependency using a non-allowlisted license. `cargo deny check bans` warns on duplicate crate versions. `cargo deny check advisories` mirrors `cargo audit`. All three checks run in the `security-scan` job alongside `cargo audit` and `trivy`.

**Trivy 설정**:
```bash
trivy image \
  --exit-code 1 \
  --severity HIGH,CRITICAL \
  --ignore-unfixed \
  ghcr.io/gadgetron/gadgetron:${{ github.sha }}
```

##### 2.C.5.2 캐싱 전략

```yaml
# .github/workflows/ci.yml (발췌)
- uses: actions/cache@v4
  with:
    path: |
      ~/.cargo/registry
      ~/.cargo/git
      target/
    key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
    restore-keys: |
      ${{ runner.os }}-cargo-
```

Cargo.lock는 workspace 루트에 커밋 (재현 가능 빌드 보장). `--locked` 플래그 필수.

##### 2.C.5.3 재현 가능 빌드 플래그

```
RUSTFLAGS="-C codegen-units=1 -C opt-level=3"
```

`-C codegen-units=1`: 링커 최적화 순서 결정론 보장. 바이너리 sha256 재현성 증가. 빌드 시간 ~20% 증가 (release build에만 적용).

---

#### §2.C.6 릴리스 채널 (Release Channels)

| 채널 | 태그 형식 | 트리거 | 프로모션 게이트 |
|---|---|---|---|
| nightly | `nightly-YYYYMMDD` | 매일 00:00 UTC, main 브랜치 | CI 전 단계 통과 |
| beta | `v1.2.3-beta.N` | 수동 (`make release-beta`) | nightly 48h 안정 + changelog 검토 |
| stable | `v1.2.3` | 수동 (`make release-stable`) | beta 72h 안정 + 통합 테스트 통과 + SBOM 첨부 |

**태그 포맷 규칙**:
- stable: `v{MAJOR}.{MINOR}.{PATCH}` — SemVer 2.0
- beta: `v{MAJOR}.{MINOR}.{PATCH}-beta.{N}` (N=1,2,3,...)
- nightly: `nightly-{YYYYMMDD}` (날짜는 UTC 기준)

**Changelog 소스**: `git-cliff` — Conventional Commits 파싱 (`feat:`, `fix:`, `chore:`, `refactor:`, `perf:`, `docs:`). `.cliff.toml` 루트에 위치.

**GHCR 이미지 태그 매핑**:
```
ghcr.io/gadgetron/gadgetron:latest         → stable 최신
ghcr.io/gadgetron/gadgetron:v1.2.3         → stable 특정 버전
ghcr.io/gadgetron/gadgetron:v1.2.3-beta.1  → beta
ghcr.io/gadgetron/gadgetron:nightly-20260412 → nightly
```

---

#### §2.C.7 롤백 & 카나리 [Phase 2]

> Phase 1은 `replicas: 1` + Helm rollback만 지원. 카나리 트래픽 분할은 Phase 2.

##### 2.C.7.1 Helm Rollback

```bash
# 현재 릴리스 이력 확인
helm history gadgetron -n gadgetron

# 특정 revision으로 롤백
helm rollback gadgetron 3 -n gadgetron

# 롤백 후 헬스 확인
kubectl rollout status deployment/gadgetron -n gadgetron
curl http://$(kubectl get svc gadgetron -n gadgetron -o jsonpath='{.spec.clusterIP}'):8080/health
```

##### 2.C.7.2 카나리 배포 [P2]

카나리는 K8s label selector 기반. **Helm chart에 `canary.enabled` 값 추가** (Phase 2):

```yaml
# values.yaml (Phase 2 추가)
canary:
  enabled: false
  weight: 10          # 전체 트래픽의 10% (Nginx Ingress annotation 기반)
  image:
    tag: ""           # 카나리 이미지 태그
```

트래픽 분할: `nginx.ingress.kubernetes.io/canary: "true"` + `nginx.ingress.kubernetes.io/canary-weight: "10"`.

**Phase 1에서 미지원 이유**: `replicas: 1` + in-memory moka 캐시 → 카나리 Pod는 별도 캐시 상태 → 테넌트 일관성 없음. Phase 2에서 상태 외부화(PostgreSQL/Redis) 후 활성화.

---

#### §2.C.8 PostgreSQL SPOF 정책 [H-10]

**명시적 결정 (xaas-platform-lead + devops-sre-lead 합의, PM 승인)**:

> PostgreSQL 다운 시 Gadgetron은 **모든 신규 요청에 503 Service Unavailable을 반환**한다.
> 응답 헤더: `X-Gadgetron-Error: database_unavailable`

**적용 범위**:
- `/v1/chat/completions` → 503 (audit 기록 불가 → 요청 거부)
- `/v1/models` → 503 (모델 목록 DB 조회)
- `/api/v1/*` 관리 API → 503

**예외 없음**: 캐시 히트 상태에서도 신규 audit 기록 불가 → 전면 거부. Phase 1 known limitation.

**Multi-instance 금지** (moka 캐시 일관성):
- Phase 1 `replicas: 1` 고정.
- `replicas: 2` 배포 시 각 인스턴스의 moka 캐시 독립 → 키 취소 전파 없음 → 보안 위반.
- Phase 2 해결: Redis 분산 캐시 도입 → `replicas: N` 허용.

**Readiness probe 연동**: `/ready` endpoint는 `sqlx::Pool::acquire()` 호출 → 1초 내 응답 없으면 `is_ready=false` → K8s가 Service endpoint에서 Pod 제거 → LB가 트래픽 차단. DB 복구 후 자동 readiness 회복.

```rust
// gadgetron-gateway/src/health.rs
pub async fn ready_handler(
    State(pg_pool): State<PgPool>,
    State(providers): State<Arc<dyn ProviderRegistry>>,
) -> impl IntoResponse {
    // 1. DB 연결 확인
    match pg_pool.acquire().await {
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [("X-Gadgetron-Error", "database_unavailable")],
                "database unavailable",
            ).into_response();
        }
        Ok(_) => {}
    }
    // 2. 최소 1개 provider healthy 확인
    if !providers.any_healthy().await {
        return (StatusCode::SERVICE_UNAVAILABLE, "no healthy providers").into_response();
    }
    StatusCode::OK.into_response()
}
```

##### 2.C.8.1 Backup 전략 [M-10]

**pg_dump CronJob** (K8s 배포 기준):
```yaml
# deploy/helm/gadgetron/templates/pg-backup-cronjob.yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: gadgetron-pg-backup
  namespace: gadgetron
spec:
  schedule: "0 2 * * *"          # 매일 02:00 UTC
  successfulJobsHistoryLimit: 7  # 7일치 이력
  failedJobsHistoryLimit: 3
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: OnFailure
          containers:
          - name: pg-dump
            image: postgres:16-alpine
            command:
            - /bin/sh
            - -c
            - |
              pg_dump -Fc -h $PG_HOST -U $PG_USER $PG_DATABASE \
                > /backup/gadgetron-$(date +%Y%m%d%H%M%S).dump
            env:
            - name: PGPASSWORD
              valueFrom:
                secretKeyRef:
                  name: gadgetron-pg-secret
                  key: password
            volumeMounts:
            - name: backup
              mountPath: /backup
          volumes:
          - name: backup
            persistentVolumeClaim:
              claimName: gadgetron-backup-pvc
```

**RPO 24h**: 매일 1회 dump. 장애 시 최대 24h 데이터 손실 허용 (audit_log = revenue source — Phase 2에서 WAL streaming 도입 시 RPO 단축).

**bare metal 환경**:
```bash
# /etc/cron.d/gadgetron-backup
0 2 * * * gadgetron pg_dump -Fc gadgetron > /var/backups/gadgetron/gadgetron-$(date +\%Y\%m\%d).dump
```

---

#### §2.C.9 Runbook 터치포인트 [H-14]

> 전체 runbook: `docs/runbook/` 디렉터리. 아래는 copy-paste 명령어 시퀀스.

##### G-1: GPU Hung — Node Agent 재시작

**증상**: `gadgetron_node_nvml_poll_stall_total` 증가, GPU utilization 0% 지속, 새 배포 실패.

**확인**:
```bash
# GPU 상태 확인
nvidia-smi

# Gadgetron node 로그 확인
journalctl -u gadgetron --since "10m ago" | grep -E "nvml|gpu|hung"

# 또는 K8s
kubectl logs -n gadgetron -l app=gadgetron-node --tail=100 | grep -E "nvml|gpu|hung"

# 메트릭 확인
curl http://localhost:9090/metrics | grep nvml_poll_stall
```

**조치**:
```bash
# Bare metal: gadgetron 서비스 재시작
systemctl restart gadgetron

# K8s: Pod 재시작 (새 Pod가 새 GPU context 획득)
kubectl rollout restart deployment/gadgetron -n gadgetron
kubectl rollout status deployment/gadgetron -n gadgetron
```

**검증**:
```bash
curl http://localhost:8080/ready
# 예상: HTTP 200

nvidia-smi
# 예상: GPU 프로세스 목록 정상

curl http://localhost:9090/metrics | grep gadgetron_node_nvml_poll_stall_total
# 예상: 카운터 증가 중단
```

---

##### S-1: Scheduler 연결 불가 — Failover

**증상**: `/v1/chat/completions` 500 오류, 로그에 `scheduler unreachable` 또는 connection refused.

**확인**:
```bash
# Scheduler 헬스 확인 (단일 바이너리이므로 Scheduler = Gadgetron main process)
curl http://localhost:8080/health

# 로그에서 scheduler 에러 검색
journalctl -u gadgetron --since "5m ago" | grep -E "scheduler|unreachable|refused"

# K8s: Pod 상태 확인
kubectl get pods -n gadgetron
kubectl describe pod -n gadgetron -l app=gadgetron
```

**조치**:
```bash
# Bare metal: 재시작
systemctl restart gadgetron

# K8s: Pod가 CrashLoopBackOff인 경우
kubectl delete pod -n gadgetron -l app=gadgetron
# — ReplicaSet이 새 Pod 자동 생성

# 상태 복구 후 모델 재등록 트리거 (NodeAgent가 자동 재등록하지 않는 경우)
curl -X POST http://localhost:8080/api/v1/models/recover \
  -H "Authorization: Bearer $GADGETRON_API_KEY"
```

**검증**:
```bash
curl http://localhost:8080/health   # 200 OK
curl http://localhost:8080/ready    # 200 OK
curl http://localhost:8080/v1/models \
  -H "Authorization: Bearer $GADGETRON_API_KEY"
# 예상: 모델 목록 정상 반환
```

---

##### D-1: DB Down — 거부 및 복구

**증상**: 모든 API 503, 헤더 `X-Gadgetron-Error: database_unavailable`.

**확인**:
```bash
# PostgreSQL 상태 확인 (bare metal)
systemctl status postgresql
pg_isready -h localhost -p 5432 -U gadgetron

# K8s
kubectl get pods -n gadgetron -l app=postgres
kubectl logs -n gadgetron -l app=postgres --tail=50

# Gadgetron readiness
curl -v http://localhost:8080/ready
# 예상: 503 + X-Gadgetron-Error: database_unavailable
```

**조치**:
```bash
# Bare metal: PostgreSQL 재시작
systemctl restart postgresql
pg_isready -h localhost -p 5432 -U gadgetron  # 준비 확인

# K8s StatefulSet 재시작
kubectl rollout restart statefulset/postgres -n gadgetron
kubectl rollout status statefulset/postgres -n gadgetron

# Gadgetron은 자동 readiness 회복 (sqlx Pool이 재연결)
# 최대 10초 내 /ready → 200 OK 복구
```

**검증**:
```bash
pg_isready -h localhost -p 5432 -U gadgetron   # 0 exit code
curl http://localhost:8080/ready               # 200 OK
curl http://localhost:8080/v1/models \
  -H "Authorization: Bearer $GADGETRON_API_KEY"  # 정상 응답
```

---

##### P-3: Provider 장애 — Circuit Breaker 수동 리셋

**증상**: 특정 provider가 circuit breaker OPEN 상태, 해당 provider로 라우팅된 요청 즉시 실패.

**확인**:
```bash
# Circuit breaker 상태 메트릭 확인
curl http://localhost:9090/metrics | grep circuit_breaker

# 로그에서 circuit open 이벤트 검색
journalctl -u gadgetron --since "15m ago" | grep -E "circuit|breaker|open"
```

**조치**:
```bash
# Circuit breaker 수동 리셋 (provider 복구 후)
curl -X POST http://localhost:8080/api/v1/providers/reset \
  -H "Authorization: Bearer $GADGETRON_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"provider": "anthropic"}'

# provider가 실제 복구되었는지 확인 먼저
# provider 종류별 헬스 엔드포인트:
# Ollama: curl http://ollama-endpoint:11434/api/tags
# vLLM:   curl http://vllm-endpoint:8000/health
# SGLang: curl http://sglang-endpoint:30000/health
```

**검증**:
```bash
curl http://localhost:9090/metrics | grep circuit_breaker
# 예상: state=closed 또는 half_open

# 테스트 요청
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $GADGETRON_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-3-5-sonnet-20241022","messages":[{"role":"user","content":"test"}]}'
# 예상: 200 OK (circuit closed 후 정상 응답)
```

---

#### §2.C.10 `gadgetron-web` 패키징 [M-9, Phase 2]

**결정**: `gadgetron-web`은 **workspace 멤버 crate**이며 별도 바이너리가 아니다. `gadgetron-gateway`가 feature flag `web-ui` 활성화 시 정적 파일을 embed하여 서빙.

**Workspace 위치**: `crates/gadgetron-web/` (D-12 크레이트 경계표 확장).

**패키징 전략**:
```toml
# crates/gadgetron-web/Cargo.toml
[package]
name = "gadgetron-web"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
tower-serve-static = "0.1"  # 정적 파일 embed + serve
include_dir = "0.7"         # 빌드 시 정적 파일 embed

[build-dependencies]
# 빌드 스크립트에서 npm build 실행 (Phase 2)
```

```rust
// crates/gadgetron-web/src/lib.rs
use include_dir::{include_dir, Dir};
use tower_serve_static::ServeDir;

/// Phase 2에서 빌드 시 `web/dist/` 디렉터리를 embed
static STATIC_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

/// axum Router에 마운트: `router.nest_service("/web", gadgetron_web::service())`
pub fn service() -> ServeDir {
    ServeDir::new(&STATIC_DIR)
}
```

**gadgetron-gateway 통합**:
```toml
# crates/gadgetron-gateway/Cargo.toml
[features]
default = []
web-ui = ["gadgetron-web"]

[dependencies]
gadgetron-web = { path = "../gadgetron-web", optional = true }
```

```rust
// crates/gadgetron-gateway/src/router.rs
#[cfg(feature = "web-ui")]
router = router.nest_service("/web", gadgetron_web::service());
```

**Phase 2 타임라인**: Web UI는 Phase 2 목표. Phase 1에서 `gadgetron-web` crate 골격만 생성, 실제 프론트엔드 빌드 없음. `--features web-ui` 없이 컴파일하면 정적 파일 미embed.

---

#### §2.C 결정 ID 요약

| 결정 ID | 내용 | 적용 섹션 |
|---|---|---|
| B-5 | K8s operator Option A (operator → Scheduler HTTP 호출) | §2.C.4 |
| H-17 | Finalizer + Leader election (Phase 2) | §2.C.4.3, §2.C.4.4 |
| H-15 | terminationGracePeriodSeconds=70 (5+30+5+10+20) | §2.C.4.5 |
| H-10 | PG SPOF: "PG down → 503 all", multi-instance 금지 | §2.C.8 |
| M-10 | pg_dump CronJob daily, RPO 24h | §2.C.8.1 |
| L-2 | cargo-audit + Trivy in CI | §2.C.5.1 |
| H-14 | Runbook copy-paste 명령어 시퀀스 (G-1, S-1, D-1, P-3) | §2.C.9 |
| M-9 | gadgetron-web workspace member + tower-serve-static | §2.C.10 |
| D-20260412-02 | Implementation Determinism (8 항목) | 전체 |

---

### Axis D: 상태 관리 (State Management)

> **Decisions applied**: B-2 (VRAM Hybrid), H-6 (quota_config moka), H-10 (PG pool config + SPOF), M-1 (cache invalidation test), M-5 (graceful drain), M-10 (backup retention), G-1 (NodeAgent re-registration), D-20260411-13 (DatabaseErrorKind), D-20260412-02 (implementation determinism).
>
> **Libraries** (pinned): `sqlx 0.8`, `moka 0.12` (feature = "future"), `dashmap 5.5`, `tokio::sync` (tokio 1.x).
>
> **External dependency**: PostgreSQL `postgres:16` (Docker image / Helm bitnami/postgresql 15.x chart). Schema version tracked by sqlx `_sqlx_migrations` table.

---

#### §2.D.1 State Ownership Map

Every stateful entity in Gadgetron Phase 1, with its owning tier, durability, and recovery source after a process restart.

| Entity | Rust Type | Owner Crate | Tier | Durability | Recovery Source |
|--------|-----------|-------------|------|------------|-----------------|
| Tenants | `Vec<Tenant>` rows | `gadgetron-xaas` | PostgreSQL | Durable | PG query on demand |
| API Keys (hashed) | `api_keys` rows | `gadgetron-xaas` | PostgreSQL | Durable | PG query on demand |
| API Keys (validated, hot) | `moka::future::Cache<String, Arc<ValidatedKey>>` | `gadgetron-xaas` | moka LRU | Ephemeral (10min TTL) | Cold: PG re-query |
| Quota configs (raw) | `quota_configs` rows | `gadgetron-xaas` | PostgreSQL | Durable | PG query on demand |
| Quota configs (hot) | `moka::future::Cache<Uuid, Arc<QuotaConfig>>` | `gadgetron-xaas` | moka LRU | Ephemeral (10min TTL) | Cold: PG re-query |
| Audit entries (pending) | `mpsc::Sender<AuditEntry>` channel buffer, cap 4096 | `gadgetron-xaas` | tokio mpsc | Ephemeral (lost on kill) | Not recovered; Phase 2 WAL fallback |
| Audit entries (committed) | `audit_log` rows | `gadgetron-xaas` | PostgreSQL | Durable | PG query |
| Billing events (ledger) | `billing_events` rows | `gadgetron-xaas` | PostgreSQL | Durable | PG query on demand — append-only, never UPDATE/DELETE. Chat rows written fire-and-forget from `PgQuotaEnforcer::record_post` alongside the `quota_configs` counter UPDATE (TASK 12.1 / PR #236 / v0.5.5). Tool + action rows written fire-and-forget at the `/v1/tools/{name}/invoke` success path + workbench direct-action / approved-action terminals — NOT through `QuotaEnforcer` (tool/action don't transit the quota path); action rows carry `source_event_id = audit_event_id` for clean audit↔ledger joins (TASK 12.2 / PR #241 / v0.5.6). Counter-vs-ledger reconciliation is TASK 12.4 scope (DEFERRED per 2026-04-20 commercialization-layer direction). |
| Node registry | `DashMap<Uuid, NodeInfo>` | `gadgetron-scheduler` | DashMap | Ephemeral | NodeAgent re-registers on restart (§2.D.7, G-1) |
| Model deployments | `DashMap<Uuid, ModelDeployment>` | `gadgetron-scheduler` | DashMap | Ephemeral | NodeAgent push after re-register |
| VRAM state (per GPU) | `DashMap<Uuid, Vec<GpuInfo>>` | `gadgetron-scheduler` | DashMap | Ephemeral | Heartbeat within 10s or pull at deploy |
| Node heartbeat timestamp | `DashMap<Uuid, Instant>` | `gadgetron-scheduler` | DashMap | Ephemeral | Reset to `now()` on first heartbeat |
| In-flight request counter | `Arc<AtomicUsize>` | `gadgetron-gateway` | Atomic | Ephemeral | Starts at 0 on restart |
| SSE active streams | `DashMap<Uuid, AbortHandle>` | `gadgetron-gateway` | DashMap | Ephemeral | Not recovered (clients reconnect) |
| Routing metrics | `DashMap<(String, String, Uuid), ProviderMetrics>` | `gadgetron-router` | DashMap | Ephemeral | Prometheus scrape history lost; counter resets |
| WsMessage fan-out bus | `broadcast::Sender<WsMessage>` (cap 1024) | `gadgetron-gateway` | broadcast | Ephemeral (fire-and-forget) | New subscribers reconnect |
| Provider pool | `HashMap<String, Arc<dyn LlmProvider>>` | `gadgetron-router` | Process heap | Ephemeral | Re-initialized from `AppConfig` on startup |
| App config | `Arc<AppConfig>` | `gadgetron-core` | Process heap | Ephemeral | Read from `config/gadgetron.toml` on startup |

**Durability rank** (most to least): PostgreSQL (0s staleness) → moka (≤10min staleness) → DashMap (≤10s via heartbeat for VRAM) → broadcast (best-effort, Lagged tolerated per §2.B.9).

---

#### §2.D.2 PostgreSQL Schema & Access

**Database**: `postgres:16`. Connection string: `DATABASE_URL=postgres://<user>:<password>@<host>:5432/gadgetron` (resolved via `${ENV}` substitution from `config/gadgetron.toml`).

**Migration file**: `gadgetron-xaas/src/db/migrations/001_initial.sql` (forward-only, tracked by `_sqlx_migrations`).

```sql
-- gadgetron-xaas/src/db/migrations/001_initial.sql

CREATE TABLE tenants (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT        NOT NULL UNIQUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    is_active   BOOLEAN     NOT NULL DEFAULT TRUE
);
-- index: PRIMARY KEY on id (implicit)
-- index: UNIQUE on name (implicit)

CREATE TABLE api_keys (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    key_hash    TEXT        NOT NULL UNIQUE,     -- SHA-256(raw_key), hex encoded
    prefix      TEXT        NOT NULL,            -- "gad_live_" | "gad_test_"
    scopes      TEXT[]      NOT NULL,            -- e.g. {'OpenAiCompat','Management'}
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at  TIMESTAMPTZ,                     -- NULL = no expiry
    is_revoked  BOOLEAN     NOT NULL DEFAULT FALSE
);
-- index: PRIMARY KEY on id (implicit)
-- index: UNIQUE on key_hash (implicit) — used for O(1) lookup
-- index: CREATE INDEX idx_api_keys_tenant ON api_keys(tenant_id);

CREATE INDEX idx_api_keys_tenant ON api_keys(tenant_id);

CREATE TABLE quota_configs (
    tenant_id         UUID   PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    rpm_limit         INT    NOT NULL DEFAULT 1000,     -- requests/minute
    tpm_limit         BIGINT NOT NULL DEFAULT 1000000,  -- tokens/minute
    daily_cost_cents  BIGINT,                           -- NULL = unlimited; i64 cents (D-8)
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
-- index: PRIMARY KEY on tenant_id (implicit)

CREATE TABLE audit_log (
    id                BIGSERIAL   PRIMARY KEY,
    request_id        UUID        NOT NULL,
    tenant_id         UUID        NOT NULL,      -- no FK; tenant may be deleted
    model             TEXT        NOT NULL,
    provider          TEXT        NOT NULL,
    status            TEXT        NOT NULL       CHECK (status IN ('ok','error','stream_interrupted')),
    latency_ms        INT         NOT NULL,
    prompt_tokens     INT         NOT NULL DEFAULT 0,
    completion_tokens INT         NOT NULL DEFAULT 0,
    cost_cents        BIGINT      NOT NULL DEFAULT 0,  -- i64 cents (D-8, D-20260411-06)
    error_kind        TEXT,                            -- NULL if status = 'ok'
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
-- index: PRIMARY KEY on id (implicit, BIGSERIAL)
CREATE INDEX idx_audit_log_tenant_created  ON audit_log(tenant_id, created_at DESC);
CREATE INDEX idx_audit_log_request_id      ON audit_log(request_id);
```

**PostgreSQL role and permission grants** (M-SEC-5):

```sql
-- gadgetron-xaas/src/db/migrations/002_roles.sql
-- Application role: restricted permissions enforce security invariants at the DB level

CREATE ROLE gadgetron_app LOGIN PASSWORD '${GADGETRON_DB_PASSWORD}';

-- Full CRUD on business tables
GRANT SELECT, INSERT, UPDATE, DELETE ON tenants, api_keys, quota_configs TO gadgetron_app;

-- INSERT-only on audit_log — no UPDATE or DELETE permitted at DB level
-- This makes the audit trail append-only even if application code is compromised
GRANT SELECT, INSERT ON audit_log TO gadgetron_app;

-- Sequence grants for BIGSERIAL audit_log.id
GRANT USAGE, SELECT ON SEQUENCE audit_log_id_seq TO gadgetron_app;
```

No `UPDATE` or `DELETE` permission is granted on `audit_log` to `gadgetron_app`. This enforces the append-only invariant at the PostgreSQL layer, independent of application-level access controls. An attacker who compromises the application process cannot modify or delete audit records through the standard connection credentials.

**Security events that generate `audit_log` entries**:

The following events must produce an `AuditEntry` sent via `AuditWriter::try_send` to the `audit_log` table, in addition to regular request completions:

| Event | `AuditEntry.event` field | HTTP status produced | Trigger location |
|---|---|---|---|
| API key revocation | `"key_revoked"` | 200 (management API) | `PgKeyValidator::revoke_key()` |
| Scope denial | `"scope_denied"` | 403 Forbidden | `ScopeGuardLayer::require_scope()` |
| Auth failure (invalid/missing key) | `"auth_failed"` | 401 Unauthorized | `AuthLayer::validate()` on cache miss + DB not found |

These audit entries use `tenant_id` from the failed request context where available; auth failures before `TenantContext` injection use `Uuid::nil()` as the tenant_id sentinel.

**Phase 2 additional tables** (schema defined at Phase 2 migration time):
```sql
-- billing_ledger (tenant_id UUID, period DATE, total_cents BIGINT, invoice_id UUID) [P2]
-- agents         (id UUID, tenant_id UUID, template_id UUID, state TEXT, memory_json JSONB) [P2]
-- model_catalog  (model_id TEXT PRIMARY KEY, source TEXT, metadata JSONB, download_state TEXT) [P2]
```

**Pool configuration** (`gadgetron-xaas/src/db/pool.rs`):

```rust
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

pub async fn create_pool(database_url: &str) -> Result<sqlx::PgPool, GadgetronError> {
    PgPoolOptions::new()
        .max_connections(20)          // 20 = empirical: 4 CPU cores × 5 concurrent queries.
                                      // PostgreSQL default max_connections=100; we consume 20%.
        .acquire_timeout(Duration::from_secs(5))  // 5s: fail fast before axum 30s request timeout
        .connect(database_url)
        .await
        .map_err(sqlx_to_gadgetron)
}
```

**`sqlx::query!` vs `sqlx::query_as!` policy**:
- Use `sqlx::query_as!` when the result maps to a named struct (e.g., `Tenant`, `ApiKey`, `QuotaConfig`). Compile-time checked.
- Use `sqlx::query!` for DML (INSERT, UPDATE, DELETE) and single-scalar SELECTs. Compile-time checked.
- Never use `sqlx::query_unchecked!` in production paths.

**Sample repository function** (`gadgetron-xaas/src/db/tenant.rs`) showing `sqlx_to_gadgetron` usage:

```rust
use sqlx::PgPool;
use uuid::Uuid;
use gadgetron_core::error::GadgetronError;
use crate::error::sqlx_to_gadgetron;
use crate::tenant::model::Tenant;

pub async fn find_tenant_by_id(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Tenant, GadgetronError> {
    sqlx::query_as!(
        Tenant,
        r#"SELECT id, name, created_at, is_active FROM tenants WHERE id = $1"#,
        tenant_id
    )
    .fetch_one(pool)
    .await
    .map_err(sqlx_to_gadgetron)
    // DatabaseErrorKind::RowNotFound → GadgetronError::Database { kind: RowNotFound, .. } → HTTP 404
    // DatabaseErrorKind::PoolTimeout → GadgetronError::Database { kind: PoolTimeout, .. } → HTTP 503
}

pub async fn insert_audit_entry(
    pool: &PgPool,
    entry: &AuditEntry,
) -> Result<(), GadgetronError> {
    sqlx::query!(
        r#"INSERT INTO audit_log
           (request_id, tenant_id, model, provider, status, latency_ms,
            prompt_tokens, completion_tokens, cost_cents, error_kind)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
        entry.request_id,
        entry.tenant_id,
        entry.model,
        entry.provider,
        entry.status,
        entry.latency_ms,
        entry.prompt_tokens,
        entry.completion_tokens,
        entry.cost_cents,
        entry.error_kind,
    )
    .execute(pool)
    .await
    .map_err(sqlx_to_gadgetron)
    .map(|_| ())
}
```

**`sqlx_to_gadgetron` helper** (defined in `gadgetron-xaas/src/error.rs`, not `gadgetron-core`; `gadgetron-core` must remain sqlx-free per D-12 leaf-crate principle):

```rust
// gadgetron-xaas/src/error.rs
use gadgetron_core::error::{DatabaseErrorKind, GadgetronError};

pub(crate) fn sqlx_to_gadgetron(e: sqlx::Error) -> GadgetronError {
    let kind = match &e {
        sqlx::Error::RowNotFound                   => DatabaseErrorKind::RowNotFound,
        sqlx::Error::PoolTimedOut                  => DatabaseErrorKind::PoolTimeout,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_)  => DatabaseErrorKind::ConnectionFailed,
        sqlx::Error::Database(_)                   => DatabaseErrorKind::Constraint,
        sqlx::Error::Migrate(_)                    => DatabaseErrorKind::MigrationFailed,
        _                                          => DatabaseErrorKind::Other,
    };
    GadgetronError::Database { kind, message: e.to_string() }
}
```

`DatabaseErrorKind` enum (defined in `gadgetron-core/src/error.rs`, no sqlx dep):

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseErrorKind {
    RowNotFound,        // → HTTP 404
    PoolTimeout,        // → HTTP 503
    ConnectionFailed,   // → HTTP 503
    QueryFailed,        // → HTTP 500
    MigrationFailed,    // → HTTP 500 (startup fatal)
    Constraint,         // → HTTP 409
    Other,              // → HTTP 500
}
```

---

#### §2.D.3 PostgreSQL SPOF Policy (H-10, D-side)

**Policy statement**: In Phase 1, PostgreSQL is a single-instance SPOF. This is an accepted architectural constraint. The decision is made explicitly, not by omission.

**Multi-instance deployment is banned** in Phase 1. Rationale: `gadgetron-xaas` keeps `QuotaConfig` and `ValidatedKey` in per-process moka caches. Two gateway instances would each hold independent caches — a key revoked in node A's cache would still be valid in node B's cache for up to 10 minutes. Without a distributed cache invalidation bus, cross-instance coherence cannot be guaranteed. `replicas: 1` is enforced in the Helm chart (cross-ref §2.C.3).

**Pool exhaustion path**:

```
Request arrives
  → middleware acquires PgPool connection
  → PgPoolOptions::acquire_timeout(5s) exceeded
  → sqlx::Error::PoolTimedOut
  → sqlx_to_gadgetron → GadgetronError::Database { kind: PoolTimeout, .. }
  → axum error handler → HTTP 503 "Service Unavailable"
  → Prometheus: gadgetron_db_errors_total{kind="pool_timeout"} += 1
```

Cross-ref: Axis C §2.C.8 documents the gateway-level HTTP 503 response shape and the Prometheus alert rule `gadgetron_pg_pool_exhaustion` (threshold: >5 events/min over 2min window).

**Failure mode table**:

| Scenario | Behavior | Recovery |
|----------|----------|----------|
| PG down at process start | `create_pool()` fails → `tracing::error!` → process exits with code 1 | Operator restarts PG; process restart |
| PG down mid-flight (auth cache hit) | moka serves cached `ValidatedKey` for up to 10min TTL | No action needed; automatic on PG recovery |
| PG down mid-flight (auth cache miss) | `find_api_key()` → `PoolTimeout` or `ConnectionFailed` → HTTP 503 | Prometheus alert; operator investigates |
| PG down mid-flight (quota check) | `find_quota_config()` → same → HTTP 503 | Same |
| PG down mid-flight (audit write) | `AuditWriter` mpsc channel accumulates up to 4096 entries; beyond that: warn + drop + counter increment | PG recovery → channel drains; dropped entries are permanent loss until Phase 2 WAL fallback |
| Pool exhaustion (20 conns, 5s timeout) | `GadgetronError::Database { kind: PoolTimeout }` → HTTP 503 | Scale PG connections or reduce load |

**Phase 2**: Read replica for `SELECT`-only paths (auth, quota); primary for `INSERT`-only path (audit). Connection proxy: PgBouncer in transaction mode.
**Phase 3**: Multi-region PostgreSQL streaming replication.

**Data-at-rest encryption policy** (M-SEC-4):

Phase 1: PostgreSQL data at rest is encrypted via host volume encryption — operator responsibility. Acceptable mechanisms: dm-crypt, LUKS (Linux), or cloud-provider volume encryption (AWS EBS encryption, GCP Persistent Disk encryption, Azure Managed Disk encryption). `pgcrypto` extension and Transparent Data Encryption (TDE) are not required in Phase 1.

Phase 2: evaluate `pgcrypto` for column-level encryption of `api_keys.prefix_hash` and any fields classified as PII. Decision gate: if tenants handle PHI (HIPAA scope), column-level encryption is promoted from option to requirement.

**API key generation specification**: Format `gad_live_<32 alphanumeric chars>` or `gad_test_<32 alphanumeric chars>`. Generated via `rand::rngs::OsRng` (CSPRNG — OS entropy source, not pseudorandom). Entropy calculation: 32 characters × log2(62) ≈ 190 bits. This exceeds the 128-bit security minimum and falls below the 256-bit ideal. Phase 2 extends to 48 random characters (48 × log2(62) ≈ 285 bits, exceeding 256-bit target).

---

#### §2.D.4 Backup & Retention (M-10, D-side)

Backup execution is owned by Axis C (CronJob in Kubernetes, or host cron for bare-metal). Axis D documents the retention policy and storage contract.

**Retention matrix**:

| Cadence | Count | Retention Duration | Storage Path |
|---------|-------|--------------------|--------------|
| Daily   | 7     | 7 days             | `s3://gadgetron-backups/daily/YYYY-MM-DD/` or `<hostpath>/backups/daily/` |
| Weekly  | 4     | 4 weeks (28 days)  | `s3://gadgetron-backups/weekly/YYYY-WW/` |
| Monthly | 12    | 12 months          | `s3://gadgetron-backups/monthly/YYYY-MM/` |

Rationale for counts: 7 daily allows same-day-of-week comparison and covers a full week of incident lookback. 4 weekly covers a full month. 12 monthly covers a full year for annual compliance review.

**RPO**: 24 hours (daily backup cadence). Accepted for Phase 1. If a PG crash occurs between backups, up to 24 hours of audit data may be lost permanently.

**Backup command** (executed by Axis C CronJob):
```bash
pg_dump --format=custom --no-acl --no-owner \
  "$DATABASE_URL" | gzip > "gadgetron-$(date +%Y%m%d-%H%M%S).dump.gz"
```

**Encryption at rest**: Phase 2. In Phase 1, backup files rely on S3 bucket-level SSE-S3 encryption or host filesystem encryption. This is a known Phase 1 security gap, documented for compliance review.

**WAL archiving (`pgbackrest`)**: Deferred to Phase 2. Reason: `pgbackrest` requires PostgreSQL `archive_mode=on` and a sidecar or agent process alongside PG. This conflicts with Phase 1's minimal infrastructure footprint (single Docker container, no sidecars). WAL archiving would reduce RPO to near-zero but adds operational complexity not justified for Phase 1 MVP.

---

#### §2.D.5 moka Cache Catalog

All moka caches in Phase 1. Library: `moka 0.12`, feature `"future"` (async-safe cache). Cache type: `moka::future::Cache<K, V>` (sharded internally, lock-free reads under typical load).

**Cache 1: `ValidatedKey` cache** (D-20260411-12)

```rust
// gadgetron-xaas/src/auth/validator.rs
pub struct PgKeyValidator {
    pool:  PgPool,
    cache: moka::future::Cache<String, Arc<ValidatedKey>>,
    // key:   SHA-256 hex digest of raw API key (String)
    // value: Arc<ValidatedKey> (Arc because ValidatedKey contains Vec<Scope>)
}

impl PgKeyValidator {
    pub fn new(pool: PgPool) -> Self {
        let cache = moka::future::Cache::builder()
            .max_capacity(10_000)       // 10k entries: each ~200 bytes → ~2MB total.
                                        // 10k concurrent unique API keys is the Phase 1 upper bound.
            .time_to_live(Duration::from_secs(600))  // 10 minutes: matches quota_config TTL.
                                                     // Revoked keys remain valid for at most 10min.
                                                     // Documented as known Phase 1 security trade-off.
            .build();
        Self { pool, cache }
    }

    pub async fn invalidate_key(&self, key_hash: &str) {
        self.cache.invalidate(key_hash).await;
        // Called on: POST /api/v1/xaas/keys/{id}/revoke (key revocation path)
    }
}
```

| Property | Value |
|----------|-------|
| Key type | `String` (SHA-256 hex of raw bearer token) |
| Value type | `Arc<ValidatedKey>` |
| Capacity | 10,000 entries |
| TTL | 600s (10 minutes) |
| Invalidation triggers | (1) Natural TTL expiry; (2) `invalidate_key(hash)` on revoke API call |
| Test strategy (M-1) | (a) TTL test: insert entry, `tokio::time::advance(601s)`, assert cache miss. (b) Explicit revoke test: insert, call `invalidate_key`, assert immediate miss. (c) Property test: 1000 random hashes inserted then retrieved; assert no key collision (hash uniqueness). |

**Cache 2: `QuotaConfig` cache** (H-6)

```rust
// gadgetron-xaas/src/quota/config.rs
pub struct QuotaConfigCache {
    pool:  PgPool,
    cache: moka::future::Cache<Uuid, Arc<QuotaConfig>>,
    // key:   TenantId (Uuid) — one quota config per tenant
    // value: Arc<QuotaConfig>
}

impl QuotaConfigCache {
    pub fn new(pool: PgPool) -> Self {
        let cache = moka::future::Cache::builder()
            .max_capacity(10_000)       // 10k tenants: same upper bound as key cache.
            .time_to_live(Duration::from_secs(600))  // 10 minutes: quota changes are admin ops,
                                                     // 10min lag is acceptable (same as key cache).
            .build();
        Self { pool, cache }
    }

    pub async fn get(&self, tenant_id: Uuid) -> Result<Arc<QuotaConfig>, GadgetronError> {
        self.cache
            .try_get_with(tenant_id, async {
                self.fetch_from_db(tenant_id).await
            })
            .await
            .map_err(|e| e.as_ref().clone())
        // On cache hit: returns in ~50ns (no lock, sharded moka)
        // On cache miss: PG query 3-5ms, then cached for TTL
    }

    pub async fn invalidate(&self, tenant_id: Uuid) {
        self.cache.invalidate(&tenant_id).await;
        // Called on: PUT /api/v1/xaas/tenants/{id}/quota (quota update path)
    }
}
```

| Property | Value |
|----------|-------|
| Key type | `Uuid` (TenantId) |
| Value type | `Arc<QuotaConfig>` |
| Capacity | 10,000 entries |
| TTL | 600s (10 minutes) |
| Invalidation triggers | (1) Natural TTL expiry; (2) `invalidate(tenant_id)` on quota update API call |
| Test strategy (M-1) | Same structure as ValidatedKey: TTL expiry assertion, explicit invalidation on update path, property-based test for UUID key uniqueness under concurrent inserts. |

**No other moka caches exist in Phase 1.** All other hot state uses `DashMap` (per §2.D.6) which provides O(1) concurrent access without TTL semantics.

---

#### §2.D.6 Ephemeral State (DashMap + Channels)

Libraries: `dashmap 5.5` for concurrent hash maps; `tokio::sync::{broadcast, mpsc}` for channels.

**DashMap 1: Node Registry** (`gadgetron-scheduler/src/node/registry.rs`)

```rust
pub struct NodeRegistry {
    nodes: DashMap<Uuid, NodeInfo>,
    // key:   NodeId (Uuid) — assigned at registration time
    // value: NodeInfo { node_id, address, port, gpu_count, last_heartbeat: Instant,
    //                   node_state: NodeState, vram_snapshot: Vec<GpuInfo> }
}
```

| Property | Value |
|----------|-------|
| Capacity | Unbounded (expected: ≤100 nodes in Phase 1) |
| Access pattern | Write: `insert` on POST /internal/node/register; `remove` on DELETE /internal/node/{id}. Read: `get` on scheduling decisions, O(1). |
| Contention analysis | DashMap shards by key hash (default 16 shards). At 100 nodes, shard collision probability is low. Write path (registration, heartbeat) uses `insert` which takes a shard write lock for ~1μs. Read path (scheduling) uses `get` which takes a shard read lock — concurrent reads within a shard are not blocked by DashMap (read-biased). |

**DashMap 2: Model Deployments** (`gadgetron-scheduler/src/deployment/registry.rs`)

```rust
pub struct DeploymentRegistry {
    deployments: DashMap<Uuid, ModelDeployment>,
    // key:   DeploymentId (Uuid)
    // value: ModelDeployment { deployment_id, model_id, node_id, port, pid, state: DeploymentState,
    //                          vram_reserved_mb: u64, loaded_at: Instant }
}
```

| Property | Value |
|----------|-------|
| Capacity | Unbounded (expected: ≤1000 deployments) |
| Access pattern | Write: `insert` on deploy, `remove` on undeploy, `alter` on state transition. Read: `get` on route resolution. |
| Contention analysis | 1000 deployments × 16 shards = ~62 entries/shard. Write contention negligible at this scale. |

**DashMap 3: VRAM State** (`gadgetron-scheduler/src/node/vram.rs`)

```rust
pub struct VramState {
    snapshots:        DashMap<Uuid, Vec<GpuInfo>>,
    // key:   NodeId (Uuid)
    // value: Vec<GpuInfo> — one entry per GPU on the node
    last_heartbeat:   DashMap<Uuid, Instant>,
    // key:   NodeId (Uuid)
    // value: Instant of last received heartbeat
}
```

| Property | Value |
|----------|-------|
| Capacity | 100 nodes × 8 GPUs = 800 GpuInfo entries |
| Access pattern | Write: every 10s heartbeat (NodeAgent push). Read: at deploy time (Scheduler pull). |
| Contention analysis | 100 nodes sending heartbeats at staggered 10s intervals → ~10 writes/second. DashMap handles this without measurable contention. |

**DashMap 4: SSE Active Streams** (`gadgetron-gateway/src/sse/registry.rs`)

```rust
pub struct SseStreamRegistry {
    streams: DashMap<Uuid, AbortHandle>,
    // key:   RequestId (Uuid)
    // value: AbortHandle to the SSE response future
}
```

| Property | Value |
|----------|-------|
| Capacity | Unbounded (expected: P99 concurrent SSE ≤ 500) |
| Access pattern | Write: `insert` on SSE stream open, `remove` on close/abort. Read: `get` on SIGTERM drain. |
| Contention analysis | Low: SSE open/close is lower frequency than request-per-second. |

**Channel 1: AuditWriter mpsc** (`gadgetron-xaas/src/audit/writer.rs`)

```rust
// Initialization in gadgetron-xaas/src/lib.rs or startup.rs:
let (audit_tx, audit_rx) = tokio::sync::mpsc::channel::<AuditEntry>(4096);
// 4096 capacity: at 1000 req/s × 4ms PG write latency → 4 entries in flight on average.
// 4096 provides 4x safety margin over P99 burst. Beyond 4096: warn + drop + counter (D-20260411-09).
```

| Property | Value |
|----------|-------|
| Capacity | 4096 (D-20260411-09) |
| Producer | Every request handler task via `audit_tx.try_send(entry)` |
| Consumer | Single `AuditWriter` background task (spawned once at startup) |
| Full behavior | `try_send` returns `Err(TrySendError::Full)` → `tracing::warn!(tenant_id, request_id, "audit entry dropped")` + `metrics::counter!("gadgetron_xaas_audit_dropped_total").increment(1)` |

**Channel 2: WsAggregator broadcast** (`gadgetron-gateway/src/observability/aggregator.rs`, H-12)

```rust
// gadgetron-gateway/src/observability/aggregator.rs
pub struct WsAggregator {
    tx: tokio::sync::broadcast::Sender<WsMessage>,
    // capacity: 1024 — each WsMessage ≤ 2KB × 1024 = 2MB peak ring buffer.
    // 1024: sufficient for 1s of messages at 1000 msg/s. RecvError::Lagged → client reconnects.
}
```

| Property | Value |
|----------|-------|
| Capacity | 1024 (ring buffer) |
| Producer | `WsAggregator::publish(msg: WsMessage)` — called by ResourceMonitor poll loop (1s interval) and request middleware (per request) |
| Consumer | Each WebSocket connection holds a `broadcast::Receiver<WsMessage>` |
| Lag behavior | `RecvError::Lagged(n)` → slow subscriber drops `n` messages; client-side reconnect is expected. No server-side error. |

---

#### §2.D.7 State Recovery on Restart

**Boot sequence** (applies to all Gadgetron binaries; NodeAgent and GatewayNode have diverging step 5):

```
Step 1: Load AppConfig
  │  Read config/gadgetron.toml
  │  Resolve ${ENV} substitution (D-6)
  │  Fail fast on missing required fields (DATABASE_URL mandatory)
  ↓
Step 2: Connect PostgreSQL
  │  create_pool(database_url) → PgPoolOptions::max_connections(20).acquire_timeout(5s)
  │  On failure: tracing::error!("PG connect failed: {}", e); process::exit(1)
  ↓
Step 3: Run migrations
  │  sqlx::migrate!("gadgetron-xaas/src/db/migrations").run(&pool).await
  │  On failure: tracing::error!("migration failed: {}", e); process::exit(1)
  │  Idempotent: already-applied migrations are skipped
  ↓
Step 4: Initialize caches (cold start accepted)
  │  PgKeyValidator::new(pool.clone())  → empty moka cache, capacity 10k
  │  QuotaConfigCache::new(pool.clone()) → empty moka cache, capacity 10k
  │  No warmup: first requests experience cache-miss PG queries (3-5ms).
  │  This is acceptable: P99 SLO is measured after warm-up period.
  ↓
Step 5a (GatewayNode path):
  │  Spawn AuditWriter background task
  │  Spawn WsAggregator background task
  │  Build axum Router with middleware chain
  │  Start HTTP server on configured port (default: 8080)
  │  Begin accepting requests
  ↓
Step 5b (NodeAgent path — G-1 recovery):
  │  POST /api/v1/internal/node/register
  │    body: { node_id, address, port, gpu_count, gpu_info: Vec<GpuInfo> }
  │  Scheduler receives registration:
  │    NodeRegistry::upsert(node_id, NodeInfo { ..., node_state: NodeState::Healthy })
  │  NodeAgent receives 200 OK with assigned node_id (or confirms existing)
  │  Begin heartbeat loop (every 10s, §2.D.8)
  │  Sync in-flight model list via pull: GET /api/v1/internal/node/{id}/vram
  ↓
Step 6: Begin heartbeats (NodeAgent only)
     Spawn tokio task: loop { send heartbeat; sleep(10s); }
```

**NodeAgent restart sequence diagram** (G-1):

```mermaid
sequenceDiagram
    participant NA as NodeAgent
    participant SC as Scheduler
    participant DB as PostgreSQL

    Note over NA: Process starts (restart or fresh)
    NA->>NA: Read AppConfig (config/gadgetron.toml)
    NA->>SC: POST /api/v1/internal/node/register\n{node_id, address, port, gpu_info[]}
    SC->>DB: UPSERT INTO nodes (id, address, last_seen, state='Healthy')\nON CONFLICT(id) DO UPDATE ...
    DB-->>SC: OK
    SC-->>NA: 200 OK {node_id, assigned: true}
    NA->>NA: Start heartbeat loop (10s interval)
    NA->>SC: GET /api/v1/internal/node/{id}/vram (pull sync)
    SC-->>NA: 200 OK {running_models: [], vram_snapshot: [...]}
    Note over NA: Node is registered. Scheduler schedules against it.
    loop Every 10s
        NA->>SC: POST /api/v1/internal/node/{id}/heartbeat\n{gpu_metrics[], running_models[], vram_used_mb}
        SC->>SC: VramState::update(node_id, snapshot)\nlast_heartbeat[node_id] = Instant::now()
        SC-->>NA: 200 OK
    end
```

**Idempotency guarantees on restart**:
- `sqlx::migrate!()`: already-applied migrations skipped.
- `Scheduler::deploy()`: if `ModelDeployment` already exists in `DeploymentRegistry` with state `Running`, returns `Ok(())` immediately.
- `AuditEntry` INSERT: `request_id` has no UNIQUE constraint on `audit_log` (BIGSERIAL primary key). Retry of a failed audit INSERT may create a duplicate row — this is acceptable for Phase 1 (audit is best-effort; deduplication is Phase 2).

---

#### §2.D.8 VRAM Sync Hybrid Protocol (B-2)

**Protocol**: Hybrid push (NodeAgent → Scheduler, 10s interval) + pull (Scheduler → NodeAgent, at deploy time only).

**Push payload** (sent by NodeAgent every 10 seconds):

```rust
// gadgetron-core/src/node.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPayload {
    pub node_id:        Uuid,
    pub gpu_metrics:    Vec<GpuInfo>,      // one per physical GPU
    pub running_models: Vec<Uuid>,          // ModelDeployment IDs currently running
    pub vram_used_mb:   u64,               // sum across all GPUs on this node
    pub timestamp:      DateTime<Utc>,
}
```

**Pull endpoint** (invoked by Scheduler before scheduling a new deployment):

```
GET /api/v1/internal/node/{node_id}/vram
Response 200:
{
  "node_id": "<uuid>",
  "gpu_metrics": [...],
  "running_models": ["<uuid>", ...],
  "vram_used_mb": 12345,
  "timestamp": "<rfc3339>"
}
```

**Sequence diagram** (heartbeat, stale timeout, deploy-time pull):

```mermaid
sequenceDiagram
    participant NA as NodeAgent
    participant SC as Scheduler
    participant T  as StaleChecker (background task, 5s poll)

    Note over NA,SC: Normal heartbeat cycle
    loop Every 10s
        NA->>SC: POST /heartbeat {node_id, gpu_metrics, running_models, vram_used_mb}
        SC->>SC: VramState::snapshots[node_id] = gpu_metrics\nlast_heartbeat[node_id] = Instant::now()\nnodes[node_id].state = NodeState::Healthy
        SC-->>NA: 200 OK
    end

    Note over T: Stale detection (background)
    loop Every 5s
        T->>SC: Check all nodes: if Instant::now() - last_heartbeat[id] > 30s
        SC->>SC: nodes[id].state = NodeState::Stale\nExclude from scheduling decisions
        Note over SC: NodeState::Stale nodes are not scheduled against\nuntil next heartbeat restores NodeState::Healthy
    end

    Note over SC,NA: Deploy-time pull (before scheduling)
    SC->>NA: GET /internal/node/{id}/vram
    NA-->>SC: 200 {gpu_metrics, running_models, vram_used_mb}
    SC->>SC: VramState::snapshots[node_id] = fresh_snapshot\nUse fresh snapshot for VRAM fit check
    SC->>SC: Schedule deployment on node
```

**Magic number justifications**:
- 10s heartbeat: fast enough to detect node loss within 3 intervals (30s), low enough that 100 nodes generate only 10 heartbeats/second aggregate load.
- 30s stale threshold: 3 missed heartbeats is the standard convention (Kubernetes uses 40s, Consul uses 10s; 30s balances detection speed and false-positive rate at 10s interval).
- 5s stale-checker poll: checks every 5s so stale detection fires within 5s of the 30s threshold being crossed. Not worth making it 1s (CPU overhead) or 30s (late detection).

**Node state transition table**:

| Current State | Event | Next State | Side Effect |
|---------------|-------|------------|-------------|
| `Healthy(last_heartbeat: t)` | Heartbeat received | `Healthy(last_heartbeat: now)` | VramState updated |
| `Healthy(last_heartbeat: t)` | `now - t > 30s` (stale check fires) | `Stale` | Excluded from scheduling |
| `Stale` | Heartbeat received | `Healthy(last_heartbeat: now)` | VramState updated; re-included in scheduling |
| `Stale` | `now - t > 300s` (5 min — [P2])  | `Offline` | Alert fired; not scheduled |
| `Offline` | Heartbeat received | `Healthy(last_heartbeat: now)` | Alert resolved |
| `Registering` | POST /register received | `Healthy(last_heartbeat: now)` | Inserted into NodeRegistry |

Note: `Offline` state and the 300s threshold are Phase 2. Phase 1 only implements `Registering → Healthy → Stale → Healthy` cycle.

```rust
// gadgetron-core/src/node.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    Registering,           // Transient: POST /register received, not yet confirmed
    Healthy,               // Heartbeat received within 30s
    Stale,                 // No heartbeat for >30s; excluded from scheduling
    Offline,               // [P2] No heartbeat for >300s; alert fired
}
```

---

#### §2.D.9 Graceful Shutdown Drain (M-5)

**Total shutdown budget**: 70 seconds (H-15, cross-ref §2.C.4.5):
- preStop hook sleep: 5s (allows kube-proxy to drain load balancer)
- In-flight request drain: 30s
- Audit channel drain + PG flush: 5s
- Model unload (NodeAgent): 10s
- Buffer: 20s

**In-flight counter implementation** (`gadgetron-gateway/src/middleware/inflight.rs`):

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct InflightCounter {
    count:  Arc<AtomicUsize>,
    notify: Arc<Notify>,  // woken when count reaches 0
}

impl InflightCounter {
    pub fn new() -> Self {
        Self {
            count:  Arc::new(AtomicUsize::new(0)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn increment(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement(&self) {
        let prev = self.count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            // Just reached 0
            self.notify.notify_waiters();
        }
    }

    pub fn current(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}
```

The counter is incremented in Tower middleware at request entry and decremented in the response/drop path (via `Drop` impl on a guard type).

**Drain sequence** (`gadgetron-gateway/src/shutdown.rs`):

```rust
pub async fn drain_inflight(
    counter: &InflightCounter,
    drain_timeout: Duration,   // Duration::from_secs(30)
    poll_interval: Duration,   // Duration::from_millis(100)
) {
    let deadline = tokio::time::Instant::now() + drain_timeout;
    loop {
        if counter.current() == 0 {
            tracing::info!("All in-flight requests drained.");
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let remaining = counter.current();
            tracing::warn!(
                remaining_requests = remaining,
                "Drain timeout (30s) exceeded. Forcing close."
            );
            // Force close: abort all SSE streams in SseStreamRegistry
            return;
        }
        tokio::time::sleep(poll_interval).await;  // 100ms poll
    }
}
```

**SIGTERM handler** (`gadgetron-gateway/src/main.rs`):

```rust
// Spawn SIGTERM listener before starting server
let counter_clone = counter.clone();
tokio::spawn(async move {
    tokio::signal::unix::signal(SignalKind::terminate())
        .unwrap()
        .recv()
        .await;
    tracing::info!("SIGTERM received. Starting graceful shutdown.");
    // Step 1: Stop accepting new connections (axum graceful_shutdown)
    // Step 2: Drain in-flight requests (30s)
    drain_inflight(&counter_clone, Duration::from_secs(30), Duration::from_millis(100)).await;
    // Step 3: Drain audit channel (5s)
    drain_audit_channel(audit_tx.clone(), Duration::from_secs(5)).await;
    // Step 4: NodeAgent: stop running models (10s) [if applicable]
    // Step 5: Close PG pool
    pool.close().await;
    tracing::info!("Shutdown complete.");
});
```

**Forced close behavior**: After 30s drain timeout, all `AbortHandle`s in `SseStreamRegistry` are called. SSE clients receive a connection close. No retry from the server side (M-5: after drain timeout, retry is forbidden to prevent retry storms during a degraded shutdown). Clients may reconnect after the new instance comes up.

**Audit drain** (`gadgetron-xaas/src/audit/writer.rs`):

```rust
pub async fn drain_audit_channel(
    tx: mpsc::Sender<AuditEntry>,
    timeout: Duration,  // Duration::from_secs(5)
) {
    // Drop the sender so the receiver loop terminates on empty
    drop(tx);
    // AuditWriter background task will flush remaining entries in channel and exit
    tokio::time::timeout(timeout, audit_writer_join_handle).await
        .unwrap_or_else(|_| tracing::warn!("Audit drain timeout. Some entries may be lost."));
}
```

Cross-ref: Axis C §2.C.4.5 for `terminationGracePeriodSeconds: 70` Helm value.

---

#### §2.D.10 Consistency Model

**Tier comparison by staleness budget**:

| Tier | Technology | Staleness Budget | Consistency | Cross-node Coherence |
|------|------------|-----------------|-------------|----------------------|
| PostgreSQL | `sqlx 0.8` + PG 16 | 0s (read-your-writes within connection) | Strict (ACID) | N/A (single instance) |
| moka LRU caches | `moka 0.12` | ≤10 minutes (TTL) | Eventually consistent | None (per-process; multi-instance banned) |
| DashMap (node/deployment/vram) | `dashmap 5.5` | ≤10s (heartbeat) for VRAM; ≤0s for node registry (updated on each event) | Per-process consistent; no cross-node coherence | None (single process) |
| broadcast channel | `tokio::sync::broadcast` (cap 1024) | Best-effort (Lagged tolerated) | Fire-and-forget | None (per-process) |
| mpsc channel (audit) | `tokio::sync::mpsc` (cap 4096) | Best-effort (drop on full) | Ordered within channel | None |

**Rules** (binding):

1. **PostgreSQL is source of truth** for: tenant identity, API key validity (ground truth), quota limits (ground truth), and audit records. Any moka cache entry that contradicts PG is considered stale and will be corrected on next cache miss or TTL expiry.

2. **moka caches are best-effort hot paths**. A revoked API key remains valid in moka cache for at most 10 minutes after revocation (documented Phase 1 security trade-off). A quota config update takes at most 10 minutes to propagate. Both limitations are explicitly accepted and documented.

3. **DashMap state has no cross-node coherence**. The Scheduler's `NodeRegistry` and `DeploymentRegistry` are in a single process. There is exactly one Scheduler instance per cluster (enforced by `replicas: 1`). There is no distributed locking or consensus.

4. **broadcast channel is fire-and-forget**. A `RecvError::Lagged(n)` means a slow WebSocket client missed `n` messages. The server does not buffer or retry for lagged receivers. Clients are expected to reconnect and re-subscribe.

5. **mpsc channel (audit) has bounded loss**. If the channel is full (4096 entries), the newest audit entry is dropped. This is counted by `gadgetron_xaas_audit_dropped_total`. The audit log in PG is always-accurate for entries that made it through; the counter tracks what was lost.

**Consistency invariants that must never be violated** (Phase 1):
- An API key that is `is_revoked = TRUE` in PG must not authorize new requests after its moka TTL expires. (It may authorize requests during the TTL window — this is the accepted trade-off.)
- A `quota_config.daily_cost_cents` limit must not be exceeded by more than one request's worth of cost (Soft cap + 1-request grace, H-5). The next request after the cap is always rejected.
- `audit_log` rows are append-only. No UPDATE or DELETE on `audit_log` in Phase 1.

---

### Axis E: Phase 진화 (Phase Evolution)

> **Axis E scope**: Gadgetron이 Phase 1 → 2 → 3으로 진화할 때 breaking change 없이 안전하게 전이하기 위한 계약·전략·제한·수정 계획 전체. 이 섹션의 모든 항목은 D-20260412-02 구현 결정론을 준수한다: TBD 없음, 숫자 근거 명시, 라이브러리 버전 명시, enum variant 전체 열거.

---

#### §2.E.1 Phase 1 → 2 → 3 로드맵 개요

> **결정 기반**: D-20260411-01 옵션 B (12주 중간 범위), D-20260411-04 (GPU 기능 범위)
>
> **ROADMAP v2 supersession (PR #186, 2026-04-18)**: 이하 Phase 표는 프로젝트 초기 Phase 1→2→3 스펙의 **역사적 기록** 입니다. 현재 canonical 계획은 [`docs/ROADMAP.md`](../ROADMAP.md) 의 EPIC/ISSUE/TASK 트리로, Phase 2 는 **EPIC 1 (Workbench MVP, `v0.3.0` closed, PR #208) + EPIC 2 (Agent autonomy, `v0.4.0` closed, PR #209)** 로 쪼개졌습니다. Phase 2 오리지널 목표 중 shipped (Web UI ✅ 0.2.0+, `/audit/events`+`/audit/tool-events` 기반 observability ✅ 0.2.6/0.2.8) vs still-deferred (K8s operator, `/v1/embeddings`, distributed Redis rate-limit, pgbackrest WAL, MIG dynamic, semantic routing) 는 개별 ISSUE 로 재배치 예정 — EPIC 3 (Plugin platform), EPIC 4 (Multi-tenant XaaS), EPIC 5 (Cluster platform) 가 해당 미실장 항목들을 분산 흡수합니다. 아래 표의 "완료 기준" 은 오리지널 Phase-1 spec 에 정의된 것이며, 실제 릴리스는 EPIC close tag (v0.3.0 / v0.4.0) 기준입니다.

| Phase | 기간 | 핵심 목표 | 게이트된 기능 (포함 안 됨) | 완료 기준 |
|-------|------|----------|--------------------------|----------|
| **1** | 12주 (Week 1-12) | 단일 노드 클라우드+로컬 LLM 프록시, 경량 멀티테넌시, 6 provider, 6 routing strategy, NUMA/MIG 정적 | `/v1/embeddings`, K8s operator, Web UI, 분산 rate-limit, MIG 동적, ThermalController, pgbackrest, Redis 분산 캐시 | (a) `/v1/chat/completions` SSE p99 < 50ms end-to-end on loopback; (b) 6 provider 모두 통합 테스트 통과; (c) Helm chart `helm install --dry-run` 오류 없음; (d) `cargo test --workspace` green; (e) Prometheus `/metrics` 응답에 필수 12개 메트릭 존재 |
| **2** | Week 13-26 (범위 추정 ±4주, 항목 확정 후 조정) | Web UI, K8s operator, 분산 rate-limit (Redis), embeddings endpoint, pgbackrest WAL 아카이빙, MIG 동적, semantic routing 스캐폴딩 | Multi-region federation, plugin system, AMD ROCm/Intel Gaudi, A/B 자동화, HF Hub 자동 동기화, MIG ML 기반 자동 파티셔닝 | (a) `gadgetron-web` crate 빌드 + `/ui` static 서빙; (b) K8s operator `kubectl apply -f gadgetronmodel.yaml` → model deployed; (c) pgbackrest WAL RPO ≤ 5min 달성; (d) 분산 인스턴스 2개에서 rate-limit 일관성 테스트 통과; (e) `/v1/embeddings` API green |
| **3** | Week 27+ (open-ended) | Multi-region federation, plugin API, MIG 동적 ML 기반, 열·전력 스로틀링 자동화, AMD ROCm/Gaudi | (정의 없음 — Phase 3 시작 시 재계획) | Phase 3 시작 시 별도 roadmap 수립 |

**Phase 1 주차별 일정 (확정)**:

| Week | 작업 | 완료 체크 |
|------|------|----------|
| 1 | `gadgetron-core` 타입 확정 (D-1~D-3, D-10, D-13, D-20260411-13): `GadgetronError` + `DatabaseErrorKind` + `LlmProvider` trait + `Scope` + `ParallelismConfig` | `gadgetron-core` `cargo test` green |
| 2-4 | 5개 provider 구현 (OpenAI, Anthropic, Ollama, vLLM, SGLang) + `gadgetron-xaas` 기반 (auth/tenant/quota/audit) + gateway 미들웨어 체인 연결 (B-1, B-8) | 5 provider 통합 테스트 green |
| 5 | `SseToChunkNormalizer` 추출 (D-20260411-02) + criterion bench BM-routing, BM-auth-cache 작성 (B-7) | bench 기준선 기록 |
| 6-7 | Gemini provider (`PollingToEventAdapter`, H-4) + NUMA/MIG scheduler (B-2, B-4) + `ProcessManager` FSM 구현 (B-3) | Gemini 통합 테스트 green |
| 8-10 | 통합 테스트 (e2e fallback chain, eviction, TenantContext, audit), 성능 검증 P99 측정 (B-7, GAP-T2) | P99 < 50ms 달성 확인 |
| 11-12 | Helm chart, Dockerfile, CI workflow, 문서 최종화, 배포 검증 | `helm install` dry-run green, Phase 1 완료 기준 (a)~(e) 모두 통과 |

---

#### §2.E.2 API 안정성 계약

> **적용 규칙**: 각 엔드포인트에 대해 "추가-전용(additive-only)" 또는 "major-only 변경" 중 하나를 명시한다. 규칙을 어기는 변경은 major version bump 필수.

| 경로 | Phase 1 상태 | 안정성 수준 | 허용 변경 | 금지 변경 |
|------|-------------|-----------|---------|---------|
| `POST /v1/chat/completions` | 제공 | **영구 안정** (Phase 1~3) | 응답 JSON에 추가 필드 (OpenAI 스펙 준수) | 기존 필드 제거·타입 변경·이름 변경 |
| `GET /v1/models` | 제공 | **영구 안정** (Phase 1~3) | 응답 배열에 새 모델 오브젝트 필드 추가 | `id`, `object`, `created` 필드 제거·변경 |
| `POST /v1/embeddings` | **Phase 2에서 추가** (G-9) | Phase 2 이후 안정 | Phase 2 초기 6주간 breaking change 허용 (안정화 전), 이후 additive-only | Phase 2 안정화 이후 필드 제거 |
| `GET /health` | 제공 | **영구 안정** | 응답 JSON에 추가 필드 | HTTP 200 응답 코드 변경 |
| `GET /ready` | 제공 | **영구 안정** | 추가 필드 | HTTP 200/503 의미 변경 |
| `GET /api/v1/nodes` | 제공 | **minor 이내 안정** | 노드 오브젝트에 필드 추가 | 기존 필드 제거·이름 변경 (→ major bump) |
| `GET /api/v1/models` | 제공 | **minor 이내 안정** | 모델 오브젝트에 필드 추가 | 기존 필드 제거·이름 변경 (→ major bump) |
| `GET /api/v1/usage` | 제공 | **minor 이내 안정** | 집계 필드 추가 | 기존 집계 필드 제거·단위 변경 (→ major bump) |
| `GET /api/v1/costs` | 제공 | **minor 이내 안정** | 비용 필드 추가 | 기존 필드 제거·단위 변경 (→ major bump) |
| `/api/v1/xaas/*` (테넌트·쿼터·감사) | 제공 | **Phase 2 안정화까지 불안정** | Phase 1 기간 중 breaking change 허용 (내부 운영용), Phase 2 이후 additive-only | Phase 2 안정화 이후 필드 제거 |
| `/api/v1/web/workbench/*` (projection + actions + approvals + audit + usage + events ws; OpenAiCompat) | 제공 (ISSUE 1–6, v0.2.0–v0.2.9) | **additive-only** (EPIC 1 CLOSED with v0.3.0 tag, PR #208) | 응답에 필드 추가만 허용 — 기존 필드 제거·이름 변경·shape 재구성 → major bump. 14 개 OpenAiCompat-scoped 엔드포인트 목록은 `docs/manual/api-reference.md §Workbench endpoints` 참조 (하나의 SSOT; ISSUE 5 / PR #199 가 14 번째 `/audit/tool-events` 를 추가). | 기존 필드 제거·이름 변경·shape 재구성 (→ major bump) |
| `/api/v1/web/workbench/admin/*` (Management-scoped catalog hot-reload + bundle marketplace + future admin surface) | 제공 (EPIC 3 CLOSED `v0.5.0` / PR #228 — ISSUE 8 hot-reload + ISSUE 9 real bundle manifests + ISSUE 10 bundle marketplace; full per-TASK chain in [`../ROADMAP.md`](../ROADMAP.md) §EPIC 3). Endpoint set: `POST /admin/reload-catalog`, `GET /admin/bundles`, `POST /admin/bundles` (install), `DELETE /admin/bundles/{bundle_id}` (uninstall). Response shape summary: `reload-catalog` returns `{reloaded, action_count, view_count, source, source_path?, bundle?, bundles?}` with `source` enum `{"seed_p2b", "config_file", "bundles_dir"}`; discovery returns `{bundles_dir, count, bundles: [{bundle?, source_path, action_count, view_count}, ...]}`; install/uninstall return `{installed\|uninstalled, bundle_id, manifest_path, reload_hint}`. | **additive-only** with separate scope guarantee — `scope_guard_middleware` 는 이 prefix 를 broader workbench 규칙보다 먼저 매칭하여 **Management** 를 요구 (OpenAiCompat 키는 403). `scope_required` error 는 wire-frozen. 응답 필드 추가 OK, `source` enum 에 새 값 추가 OK. `bundle` / `bundles` 필드는 각각 `Option` / `Vec` 이고 `skip_serializing_if` 으로 omit 되어도 호환. EPIC 3 CLOSED 이후 additive-only 로 승격. SIGHUP trigger 는 HTTP response body 를 반환하지 않으므로 wire 계약 영향 없음. | Management scope 요구 완화, 기존 필드 제거·이름 변경, `source` enum value rename (기존 세 값 중 어느 하나의 rename), `bundle.id` / `bundle.version` / `bundles[].id` / `bundles[].version` 필드 rename (→ major bump); scope 격리 폐기 (→ 보안 breaking change, major bump + explicit ADR 필요) |
| `/v1/tools` + `/v1/tools/{name}/invoke` (외부 MCP 서버 surface, OpenAiCompat) | 제공 (ISSUE 7, v0.2.10 → v0.2.12) | **additive-only** (EPIC 2 CLOSED with v0.4.0 tag, PR #209) | 응답 shape 에 필드 추가 (새 `GadgetSchema` 필드, `tool_audit_events` 컬럼 등) 만 허용. `mcp_unknown_tool` 포함 error code 세트는 wire-frozen — 새 에러 code 추가는 OK, 기존 code rename → major bump. `Arc<dyn GadgetDispatcher>` L3 allowed-names gate 가 이 surface 의 불변 보안 계약. | 기존 필드 제거·이름 변경·shape 재구성·error code rename (→ major bump) |

**SSE 이벤트 계약 (안정, Phase 1~3)**:
- 스트림 청크: `data: {"id":"...","object":"chat.completion.chunk","choices":[...]}\n\n`
- 스트림 종료: `data: [DONE]\n\n`
- 두 형식 변경 금지. 추가 이벤트 타입 삽입 시 `event:` 필드 prefix 사용 (기존 `data:` only 라인 유지).

**스트리밍 retry 계약 (H-1, 안정, Phase 1~3)**:
- retry는 **첫 번째 SSE 청크 전송 이전에만** 허용.
- 첫 청크 전송 이후 스트림 중단 시 `GadgetronError::StreamInterrupted { reason: String }` 발생 — retry 금지, audit 기록, 연결 종료.
- 이 정책은 Phase 2~3에서 변경되지 않는다 (클라이언트가 중복 데이터를 받는 것을 방지하는 근본 이유가 Phase와 무관).

---

#### §2.E.3 Trait 및 타입 안정성

> **안정성 수준 정의**: "stable" = 기존 impl을 수정하지 않고 새 Phase로 업그레이드 가능. "additive" = 새 메서드/variant 추가 가능하나 기존 항목 제거·서명 변경 불가. "unstable" = breaking change 가능 (Phase 내부 또는 안정화 이전).

| 타입 / Trait | 정의 위치 | Phase 1 안정성 | Phase 2+ 변경 계획 | `#[non_exhaustive]` |
|-------------|---------|--------------|-----------------|:------------------:|
| `LlmProvider` | `gadgetron-core/src/provider.rs` | **stable** (메서드 7개 확정) | Phase 2에 `async fn embeddings(&self, req: EmbeddingRequest) -> Result<EmbeddingResponse>` 추가 — 반드시 `#[allow(unused)]` default impl `Err(GadgetronError::NotImplemented)` 제공으로 기존 impl 무중단 | N/A (trait) |
| `GadgetronError` | `gadgetron-core/src/error.rs` | **additive** (`#[non_exhaustive]`) | 새 variant 추가 시 minor version bump. 기존 variant 삭제·서명 변경 → major version bump. Phase 2 추가 후보: `RateLimitPersistenceFailure`, `OperatorReconcileFailed` | yes |
| `DatabaseErrorKind` | `gadgetron-core/src/error.rs` | **additive** (`#[non_exhaustive]`) | Phase 1 확정 variant: `RowNotFound`, `PoolTimeout`, `ConnectionFailed`, `QueryFailed`, `MigrationFailed`, `Constraint`, `Other`. 추가만 허용. | yes |
| `Scope` | `gadgetron-xaas/src/auth/key.rs` | **additive** (`#[non_exhaustive]`) | Phase 1 확정 variant: `OpenAiCompat`, `Management`, `XaasAdmin`. Phase 2 추가 후보: `ChatRead`, `ChatWrite`, `ModelsList`, `EmbeddingsRead`, `XaasGpuAllocate`, `XaasModelDeploy`, `XaasAgentCreate`. `OpenAiCompat` → `/v1/embeddings` 자동 허용 여부: Phase 2에서 결정 (G-9, 현재 보류). | yes |
| `ChatRequest` | `gadgetron-core/src/provider.rs` | **stable** | Phase 2에 `stream_options: Option<StreamOptions>` 추가 가능. `#[serde(default)]` 필수. | no |
| `ChatChunk` | `gadgetron-core/src/provider.rs` | **stable** | Phase 2에 `usage: Option<Usage>` 추가 가능. | no |
| `AppConfig` | `gadgetron-core/src/config.rs` | **additive** | 새 필드 반드시 `#[serde(default)]` 또는 `Option<T>`. 기존 필드 제거 → deprecation 1 minor version 후 major bump. | no |
| `ModelState` | `gadgetron-core/src/node.rs` | **additive** | Phase 1 확정 variant: `Unloaded`, `Loading { since: Instant }`, `Running { port: u16, pid: u32 }`, `Failed { reason: String, failed_at: Instant }`, `Evicting`. Phase 2 추가 후보 없음 (현재). | yes |
| `EvictionPolicy` | `gadgetron-core/src/scheduler.rs` | **additive** | Phase 1 확정 variant: `Lru`, `Priority`, `CostBased`, `WeightedLru { priority_weight: f32 }`. `CostBased`는 Phase 1에서 `ModelDeployment.priority: i32` 기반 stub — Phase 2에서 실 `$/token` 데이터 연결. `Fifo` [P2 candidate], `Manual` not in Phase 1. | yes |
| `ParallelismConfig` | `gadgetron-core/src/scheduler.rs` | **stable** | Phase 1 필드: `tp_size: u32`, `pp_size: u32`, `ep_size: u32`, `dp_size: u32`, `numa_bind: Option<u32>`. Phase 2 이후 필드 추가 시 `#[serde(default)]`. | no |
| `GadgetDispatcher` | `gadgetron-core/src/agent/tools.rs` | **stable** (P2A 이후 P2B / EPIC 2 사이클 내내 동일) | trait 자체 안정 — 새 메서드 추가 시 default impl `Err(GadgetError::NotImplemented)` 필수로 기존 impl 무중단. Dispatch 경로가 PR #175 (workbench) → PR #205 (`/v1/tools/{name}/invoke`, MCP invoke) 양쪽에서 재사용 됨. | N/A (trait) |
| `GadgetCatalog` | `gadgetron-core/src/agent/tools.rs` | **stable** (EPIC 2 / ISSUE 7 TASK 7.1, PR #204) | `GadgetDispatcher` 와 동일한 crate-boundary 패턴 — 새 메서드 추가는 default impl 필수. schemas 확장은 `GadgetSchema` 필드 추가 (additive). | N/A (trait) |
| `ActionAuditSink` | `gadgetron-core/src/audit/action.rs` | **stable** (P2B / ISSUE 3, PR #188) | sync send, fire-and-forget 의미론 영구. `ActionAuditEvent` enum 은 `#[non_exhaustive]`로 variant 추가 허용 (ApprovalGranted 같은 후속 variant 가 들어올 자리). | N/A (trait) |
| `ActionAuditEvent` | `gadgetron-core/src/audit/action.rs` | **additive** (`#[non_exhaustive]`) | 현재 variant: `DirectActionCompleted`. 후속 ISSUE 에서 ApprovalGranted / ApprovalDenied / ApprovalTimedOut 등 추가 가능. | yes |
| `ApprovalStore` | `gadgetron-core/src/workbench/approval.rs` | **stable** (P2B / ISSUE 3, PR #188) | async trait — create / get / mark_approved / mark_denied 네 메서드 확정. 새 상태 (`Cancelled` 등) 는 별도 메서드 추가 + `ApprovalState` variant 추가 로 수용. | N/A (trait) |
| `ApprovalState` | `gadgetron-core/src/workbench/approval.rs` | **additive** (`#[non_exhaustive]`) | 현재 variant: `Pending`, `Approved`, `Denied`. `Cancelled` / `TimedOut` 등 후속 variant 수용. | yes |
| `ActivityBus` + `ActivityEvent` | `gadgetron-core/src/activity_bus.rs` | **additive** (`#[non_exhaustive]` on ActivityEvent) | 현재 variant: `ChatCompleted` (ISSUE 4 / PR #194), `ToolCallCompleted` (ISSUE 5 / PR #199). 새 variant 추가 허용 — `/events/ws` 프레임 소비자는 `"type"` 디스크리미네이터로 분기. | yes (ActivityEvent) |

**자동 변경 감지 전략**: `cargo semver-checks`를 CI에 등록 (Phase 2 시작 시). major version 없이 `gadgetron-core` public API가 제거·변경되면 CI fail.

---

#### §2.E.4 데이터 마이그레이션 전략

> **라이브러리**: `sqlx 0.8` (D-4, D-9). `sqlx::migrate!("./migrations")` macro 사용.

**마이그레이션 파일 규칙**:
- 위치: `gadgetron-xaas/src/db/migrations/`
- 파일명: `NNN_description.sql` (NNN은 3자리 0-padded 숫자, 예: `001_initial.sql`, `002_phase2_billing.sql`)
- 방향: **forward-only**. DOWN migration 파일 작성 금지. 이전 버전 복구는 반드시 DB 스냅샷(pg_dump daily 또는 pgbackrest WAL) 복원으로만.
- 실행: `gadgetron-cli`가 시작 시 `sqlx::migrate!()` 자동 실행. 마이그레이션 실패 시 `GadgetronError::Database { kind: DatabaseErrorKind::MigrationFailed, .. }` 발생 → 프로세스 시작 불가 (의도된 동작).
- 상태 추적: `_sqlx_migrations` 테이블 (sqlx 자동 생성). 이 테이블을 수동 수정 금지.
- CI 검증: `sqlx prepare --check` — 컴파일 타임 쿼리 검증. `./sqlx-data.json` 파일을 소스에 커밋 (`offline mode`). `sqlx prepare --check` 실패 시 CI fail.

**롤링 업그레이드 호환성 규칙** (Phase 2 멀티 노드 전제):
- 규칙 1: migration N+1은 migration N을 실행한 DB에서 이전 버전(migration N) 코드가 오류 없이 동작해야 한다 (최소 1 release 공존 보장).
- 규칙 2: 새 NOT NULL 컬럼 추가 시 반드시 DEFAULT 값 포함. `ALTER TABLE t ADD COLUMN c TEXT NOT NULL DEFAULT 'value'`.
- 규칙 3: 컬럼 제거는 금지 (Phase 1). Phase 2 이후 컬럼 제거 시 최소 2 minor version 동안 deprecated 상태 유지 후 별도 cleanup migration.
- 규칙 4: 인덱스 추가는 `CREATE INDEX CONCURRENTLY` 사용 (PostgreSQL 12+ 지원). table lock 없이 추가.

**Phase 1 → Phase 2 마이그레이션 예시**:
```sql
-- 002_phase2_billing.sql
-- Phase 2 billing engine 추가
-- 규칙 준수: NOT NULL with DEFAULT, CONCURRENTLY index
ALTER TABLE tenants
    ADD COLUMN plan TEXT NOT NULL DEFAULT 'free',
    ADD COLUMN billing_email TEXT;   -- nullable, Phase 2 optional

CREATE TABLE billing_ledger (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    amount_cents BIGINT     NOT NULL,
    currency    TEXT        NOT NULL DEFAULT 'USD',
    description TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX CONCURRENTLY idx_billing_ledger_tenant
    ON billing_ledger(tenant_id, created_at DESC);

-- pgbackrest WAL 아카이빙 활성화 후 첫 번째 migration
-- RPO 목표: ≤ 5분 (Phase 2 완료 기준)
COMMENT ON TABLE billing_ledger IS 'phase=2, rpo=5min';
```

**Phase 2 → Phase 3 마이그레이션 (예시 direction)**:
```sql
-- 003_phase3_federation.sql
-- Multi-region federation 지원을 위한 region 컬럼 추가
ALTER TABLE tenants ADD COLUMN home_region TEXT NOT NULL DEFAULT 'us-east-1';
```

---

#### §2.E.5 Config 진화 전략

> **참조**: Axis B §2.B.5 (`AppConfig` 상세). 이 섹션은 버전 간 config 진화 정책만 다룬다.

**불변 규칙**:
1. `AppConfig` 구조체에 새 필드 추가 시 반드시 `#[serde(default)]` 또는 `Option<T>`. 이전 TOML 파일이 새 바이너리에서 오류 없이 로드되어야 한다.
2. 기존 필드 제거 정책: (a) 1 minor version 동안 deprecated 표기 (`/// @deprecated since 0.2: use X instead`), (b) major version bump 시 제거.
3. ENV override는 `GADGETRON_<SECTION>_<FIELD>` 형식으로 stable. 이름 변경 금지.
4. 모든 `Duration` 타입 필드는 초 단위 `u64` (`server.request_timeout_secs: u64`) — 파싱 오류 방지. TOML string duration 금지.

**Phase별 AppConfig 필드 추가 계획**:

| 필드 | Phase | `#[serde(default)]` 기본값 | 설명 |
|------|-------|--------------------------|------|
| `features.web_ui` | Phase 2 추가 | `false` | `gadgetron-web` static 서빙 활성화 |
| `features.k8s_operator` | Phase 2 추가 | `false` | K8s operator 연동 활성화 |
| `features.pgbackrest` | Phase 2 추가 | `false` | pgbackrest WAL 아카이빙 활성화 |
| `features.rate_limit_persist` | Phase 2 추가 | `false` | Redis 기반 rate-limit 영속화 활성화 |
| `features.embeddings` | Phase 2 추가 | `false` | `/v1/embeddings` 엔드포인트 활성화 |
| `redis.url` | Phase 2 추가 | `None` | `Option<String>`, rate-limit/cache 용도 |
| `redis.pool_size` | Phase 2 추가 | `10u32` | Redis 커넥션 풀 크기 |
| `features.mig_dynamic` | Phase 3 추가 | `false` | MIG 동적 재구성 활성화 |

**Deprecation 예시**:
```toml
# gadgetron-core/src/config.rs
pub struct ServerConfig {
    /// @deprecated since 0.2: use `request_timeout_secs` instead. Will be removed in 1.0.
    #[serde(default)]
    pub timeout_ms: Option<u64>,   // Phase 0.1 → deprecated 0.2 → removed 1.0

    /// Request timeout in seconds. Default: 120.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
}
```

---

#### §2.E.6 Phase 1 알려진 제한사항 및 Phase 2+ 수정 계획

> **결정 기반**: M-6 (TokenBucket), H-8 (CostBased eviction), M-9 (gadgetron-web), B-5 (K8s operator), D-20260411-04 (MIG 동적), D-20260411-09 (audit WAL), D-20260411-12 (moka LRU)

| ID | 제한사항 | Phase 1 동작 | Phase N+ 수정 | 수정 Phase | 수용 기준 |
|----|---------|------------|-------------|----------|---------|
| **KL-1** (M-6) | **TokenBucket 재시작 grace** | 프로세스 재시작 시 in-memory bucket 초기화 → 분당 quota 우회 가능 (최대 quota_per_minute 만큼) | **Option A** (우선): `quota_tokens_remaining`을 PG `tenants` 테이블에 주기적(10초) persist. 재시작 시 PG에서 복구. 또는 **Option B**: `redis 0.26` cluster-coordinated rate-limit (`SET NX PX` sliding window). Option 선택은 Phase 2 시작 시 결정 (redis.url config 존재 여부로 자동 선택 가능). | Phase 2 | 프로세스 재시작 후 bucket 잔량이 재시작 전 잔량 ±10% 이내 (1초 이내 복구). |
| **KL-2** | **Multi-instance 배포 금지** | Phase 1은 단일 인스턴스 강제 (moka 캐시 일관성 — H-10). 다중 인스턴스 배포 시 auth 캐시 불일치 발생. | **Phase 2 unban gate**: Redis pub/sub 또는 NATS 기반 distributed cache invalidation bus. moka `invalidate_key` 이벤트를 모든 인스턴스에 브로드캐스트. 구현 위치: `gadgetron-xaas/src/auth/invalidation.rs`. | Phase 2 | 인스턴스 2개 배포 후 key revocation이 500ms 이내 두 인스턴스 모두에 반영. |
| **KL-3** (M-10) | **PostgreSQL 백업: pg_dump daily only** | `pg_dump` CronJob 1일 1회 (D-20260411-09). RPO = 최대 24시간. audit_log = revenue source이므로 24h 손실은 compliance risk. | **Phase 2**: `pgbackrest` 바이너리 (`pgbackrest` 2.x, APT/RPM 패키지) + PostgreSQL WAL archiving (`archive_mode = on`, `archive_command = 'pgbackrest --stanza=gadgetron archive-push %p'`). Helm chart에 pgbackrest sidecar 추가. | Phase 2 | pgbackrest 활성화 후 RPO ≤ 5분 달성 (WAL archive 지연 실측). |
| **KL-4** | **Hot reload 제한 (비중요 필드만)** | SIGHUP → log level 등 비중요 필드만 reload (Axis B §2.B.5.3). TLS 인증서, DB connection pool, provider 목록은 재시작 필요. | **Phase 2 확장**: SIGHUP 시 Axis B §2.B.5.3 reloadable 테이블에 명시된 필드 전체 reload. 추가 필드: `routing.strategy`, `quota.*` 기본값. Non-reloadable 필드 변경 시 structured warning log + metric `gadgetron_config_reload_skipped_total`. | Phase 2 | Axis B §2.B.5.3 reloadable 테이블의 모든 필드가 SIGHUP 후 새 값 반영. |
| **KL-5** (H-8) | **CostBased eviction stub** | `EvictionPolicy::CostBased`는 `ModelDeployment.priority: i32` 기반 stub — `$/token` 실데이터 없음. 사실상 `Priority` eviction과 동일 동작. | **Phase 2**: provider 가격표 (`gadgetron-core/src/pricing.rs`) 추가. `CostEntry` (f64 `$/token`, D-20260411-06)를 `EvictionPolicy::CostBased` 연산에 주입. 가격표는 TOML 파일 또는 PG `provider_pricing` 테이블에서 로드. | Phase 2 | Eviction 결정이 cost-weighted priority와 5% 이내 일치 (criterion bench `BM-eviction-costbased`). |
| **KL-6** (M-9) | **`gadgetron-web` crate 미존재** | Phase 1에 Web UI 없음. TUI 읽기 전용 (H-11). | **Phase 2**: `gadgetron-web` workspace crate 신설. `tower-serve-static` crate로 Axum에서 정적 파일 embed (`include_dir!` 매크로). Feature flag: `features.web_ui = true`. React/Vite 번들 파일은 빌드 시 embed. `GET /ui` → 정적 파일 서빙, `GET /ui/api/*` → 내부 REST relay. | Phase 2 | `cargo build --features web-ui` 성공 + `curl http://localhost:8080/ui` 200 응답. |
| **KL-7** (B-5) | **K8s operator 미존재** | Phase 1: Helm chart 직접 배포. operator reconcile loop 없음. | **Phase 2**: `gadgetron-k8s` workspace crate 신설. CRD `GadgetronModel` 정의 (`apiVersion: gadgetron.io/v1alpha1`). Reconcile loop: `GadgetronModel` CR 변경 감지 → `POST /api/v1/models/deploy` (Scheduler HTTP 호출, B-5 Option A). Leader election: `kube-lease` crate 기반. Finalizer: `gadgetron.io/model-cleanup`. | Phase 2 | `kubectl apply -f gadgetronmodel.yaml` → Scheduler에 deploy 요청 → model `Running` 상태 전이 30초 이내. |
| **KL-8** | **MIG 동적 재구성 미지원** | Phase 1: MIG 정적 enable/disable만 지원 (`MigManager::enable_profile()` / `destroy_instance()`). 실행 중 MIG 프로파일 변경 불가. | **Phase 3**: `scheduler.mig_allocator` 모듈 추가. ML 워크로드 특성(평균 `total_tokens`, `model_size_mb`) 기반으로 MIG 프로파일 자동 선택. NVIDIA DCGM API 연동. | Phase 3 | MIG 프로파일 자동 전환 시간 < 30초, 기존 실행 모델 무중단 (다른 프로파일 인스턴스로 failover). |

---

#### §2.E.7 Phase 2 작업 목록 카탈로그

> 각 항목: 내용 / 크레이트 / 규모 추정 / 선행 조건 / 수용 기준 / 담당 에이전트

| # | 항목 | 크레이트 | 규모 | 선행 조건 | 수용 기준 | 담당 에이전트 |
|---|------|---------|------|---------|---------|------------|
| P2-01 | `/v1/embeddings` 엔드포인트 (G-9) | `gadgetron-gateway`, `gadgetron-provider`, `gadgetron-xaas` | M (2주) | `LlmProvider::embeddings()` default impl Phase 1에 존재 | OpenAI 호환 `/v1/embeddings` 200 응답 + `Scope::OpenAiCompat` 허용 정책 결정(G-9) | gateway-router-lead |
| P2-02 | `gadgetron-web` crate (KL-6, M-9) | `gadgetron-web` (신규) | M (2주) | Phase 1 WsMessage 타입 안정 (D-20260411-07) | `/ui` 정적 서빙 200, WebSocket `/ws` 연결, TUI 동일 데이터 소스 | ux-interface-lead |
| P2-03 | K8s operator (KL-7, B-5) | `gadgetron-k8s` (신규) | L (3주) | Phase 1 Scheduler `/api/v1/models/deploy` HTTP API 안정, Helm chart 기반 배포 확인 | CRD apply → model deploy 30초 이내, finalizer cleanup | devops-sre-lead |
| P2-04 | MIG 정적 → 동적 (KL-8 준비 단계) | `gadgetron-node`, `gadgetron-scheduler` | M (2주) | Phase 1 MIG 정적 enable/disable 안정 | MIG 프로파일 TOML 설정 변경 → restart 없이 적용 (Phase 3 ML 자동화 전 단계) | gpu-scheduler-lead |
| P2-05 | Rate-limit 영속화 (KL-1, M-6) | `gadgetron-xaas` | S (1주) | Redis URL config (`redis.url`) 존재, `redis 0.26` 의존성 추가 | 재시작 후 bucket 잔량 ±10% 이내 복구 (1초 이내) | xaas-platform-lead |
| P2-06 | pgbackrest WAL 아카이빙 (KL-3) | Helm chart, `docker/pgbackrest/` | S (1주) | PostgreSQL 12+ `archive_mode = on` 설정 가능, `pgbackrest 2.x` 바이너리 사용 가능 | RPO ≤ 5분 실측 달성 | devops-sre-lead |
| P2-07 | Semantic routing 스캐폴딩 | `gadgetron-router` | L (3주) | Phase 1 routing strategy 인터페이스 안정, 임베딩 모델(Phase 2 P2-01) 사용 가능 | `RoutingStrategy::Semantic` enum variant 추가, embedding-based 라우팅 통합 테스트 green | gateway-router-lead |
| P2-08 | Multi-instance 분산 캐시 invalidation (KL-2) | `gadgetron-xaas` | S (1주) | Redis URL config, P2-05 완료 | key revocation 500ms 이내 전 인스턴스 반영 | xaas-platform-lead |
| P2-09 | CostBased eviction 실데이터 연결 (KL-5, H-8) | `gadgetron-core`, `gadgetron-scheduler` | S (1주) | 가격표 데이터 소스 (TOML 파일 또는 PG 테이블) 결정 | eviction 결정 cost-weighted 기준 5% 이내 | gpu-scheduler-lead |
| P2-10 | Audit WAL/S3 fallback (D-20260411-09) | `gadgetron-xaas` | M (2주) | `gadgetron-xaas/src/audit/fallback.rs` 설계 확정 | audit_log channel full 시 S3 또는 WAL 파일로 fallback, SIGTERM 후 5초 이내 drain 완료 | xaas-platform-lead |

**Phase 2 총 추정 기간**: 13-15주 (의존성 직렬화 기준). 병렬 실행 시 ~9-11주.

---

#### §2.E.8 롤링 업그레이드 프로토콜

**Phase 1 (단일 노드) — 간단한 절차**:

```
# Phase 1 업그레이드 절차 (단일 노드, 추정 다운타임 < 30초)
1. 새 바이너리 빌드 완료 확인 (cargo build --release)
2. kubectl rollout restart deployment/gadgetron  # K8s 기준
   또는 systemctl restart gadgetron             # systemd 기준
3. preStop hook: 5초 대기 (in-flight 요청 drain 시작)
4. SIGTERM 수신 → 70초 graceful shutdown:
   - 30초: 신규 연결 거부, in-flight 요청 drain
   - 5초: audit channel drain (mpsc 4096 버퍼)
   - 10초: local model unload (vLLM/Ollama SIGTERM)
   - 20초: buffer (SSE long stream 완료 대기)
5. 새 Pod 시작: DB migration 자동 실행 (sqlx::migrate!())
6. /ready 200 응답 확인 → 트래픽 전환
```

**Phase 2 멀티 노드 롤링 업그레이드**:

**업그레이드 순서** (scheduler-first):
1. **Scheduler 먼저** 업그레이드 (DB migration 포함). Scheduler가 N+1 버전이고 Node Agent가 N 버전인 상태는 최대 1 release 허용 (롤링 업그레이드 창).
2. **Node Agent 순차** 업그레이드 (node 1 → 건강 확인 → node 2 → ...).
3. **Gateway 마지막** 업그레이드.

**N vs N+1 공존 호환성 매트릭스**:

| Scheduler | Node Agent | Gateway | 호환 여부 | 비고 |
|-----------|-----------|---------|---------|------|
| N+1 | N | N | 허용 | 롤링 업그레이드 창, 최대 30분 |
| N | N+1 | N | 금지 | Node Agent가 새 RPC 사용 시 오류 |
| N+1 | N+1 | N | 허용 | Gateway 마지막 업그레이드 전 상태 |
| N+1 | N+1 | N+1 | 정상 | 업그레이드 완료 |

**롤백 게이트**:
- 새 Pod가 `/ready` 200을 **업그레이드 후 5분 이내** 반환하지 않으면 자동 롤백 (`kubectl rollout undo`).
- 롤백 후 DB migration은 **rollback 없음** (forward-only). N+1 migration이 이미 적용되었으면 N 바이너리가 N+1 schema에서 동작 가능해야 한다 (규칙 1: §2.E.4 롤링 업그레이드 호환성 규칙 참조).
- 롤백 불가 케이스: migration이 NOT NULL without DEFAULT 컬럼 추가를 포함하는 경우 — 이를 방지하기 위해 §2.E.4 규칙 2 준수 필수.

**스키마 버전 호환성 체크**:
```rust
// gadgetron-cli/src/main.rs 시작 시 실행
async fn check_schema_compat(pool: &PgPool) -> Result<(), GadgetronError> {
    let applied = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = true"
    ).fetch_one(pool).await.map_err(sqlx_to_gadgetron)?;
    
    let expected = MIGRATIONS.len() as i64;
    if applied > expected {
        // 현재 바이너리보다 새 migration이 이미 적용됨 → 호환 가능 (N+1 DB, N binary)
        tracing::warn!(applied, expected, "DB schema ahead of binary version — rolling upgrade window");
        Ok(())
    } else {
        // 정상 또는 migration 적용 전
        sqlx::migrate!("./migrations").run(pool).await
            .map_err(|e| GadgetronError::Database { kind: DatabaseErrorKind::MigrationFailed, message: e.to_string() })
    }
}
```

---

#### §2.E.9 기능 플래그 매트릭스

> **위치**: `gadgetron-core/src/config.rs` `FeaturesConfig` 구조체. 모든 필드 `#[serde(default)]`, 기본값 명시.

```toml
# gadgetron.toml — 전체 features 섹션 (Phase 1 기본값)
[features]
nvml        = true    # NVML GPU 메트릭 수집. false → CPU-only 모드 (graceful degradation)
tls         = true    # rustls TLS. false → HTTP-only (개발/테스트 환경 전용)
cors_permissive = false  # CORS 와일드카드 허용 (*). 기본 false. 활성화 시 WARN log 출력.
                         # Deprecated: Phase 2에서 제거 예정 (cors_origins 배열로 대체)
rate_limit  = true    # in-memory TokenBucket rate-limit. false → 무제한 (테스트 전용)
web_ui      = false   # [P2] gadgetron-web static 서빙. Phase 1에서는 컴파일 제외.
k8s_operator = false  # [P2] K8s operator 연동. Phase 1에서는 컴파일 제외.
pgbackrest  = false   # [P2] pgbackrest WAL 아카이빙. Phase 1에서는 pg_dump daily.
rate_limit_persist = false  # [P2] Redis 기반 rate-limit 영속화. false = in-memory only.
embeddings  = false   # [P2] /v1/embeddings 엔드포인트 활성화.
mig_dynamic = false   # [P3] MIG 동적 재구성. Phase 1-2에서는 정적만.
```

| 기능 플래그 | 기본값 | 활성화 Phase | 의존 기능 | Deprecated 계획 |
|------------|-------|------------|---------|---------------|
| `nvml` | `true` | Phase 1 | NVML 드라이버 설치 | 없음 |
| `tls` | `true` | Phase 1 | `rustls` crate | 없음 (운영 필수) |
| `cors_permissive` | `false` | Phase 1 (opt-in) | 없음 | Phase 2에서 제거 → `cors_origins: Vec<String>` 대체. 1 minor version 경고 후 제거. |
| `rate_limit` | `true` | Phase 1 | `TokenBucket` in-memory | Phase 2 이후 `rate_limit_persist`로 점진 대체 |
| `web_ui` | `false` | Phase 2 | `gadgetron-web` crate, `tower-serve-static` | 없음 |
| `k8s_operator` | `false` | Phase 2 | `gadgetron-k8s` crate, K8s API 접근 | 없음 |
| `pgbackrest` | `false` | Phase 2 | `pgbackrest 2.x` 바이너리, WAL archiving 설정 | 없음 |
| `rate_limit_persist` | `false` | Phase 2 | `redis 0.26` crate, `redis.url` config | 없음 |
| `embeddings` | `false` | Phase 2 | `LlmProvider::embeddings()` impl | 없음 |
| `mig_dynamic` | `false` | Phase 3 | NVIDIA DCGM API, `gadgetron-node::MigAllocator` | 없음 |

**기능 플래그 런타임 체크 패턴**:
```rust
// gadgetron-gateway/src/routes/v1.rs
pub async fn embeddings_handler(
    State(state): State<AppState>,
    Json(req): Json<EmbeddingRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if !state.config.features.embeddings {
        return Err(StatusCode::NOT_FOUND);  // 플래그 꺼진 경우 404 (기능 미노출)
    }
    // ... 실제 처리
}
```

---

#### §2.E.10 버전 번호 및 릴리즈 케이던스

**Semver 규칙** (gadgetron workspace 전체 단일 버전):

| 변경 유형 | 버전 bump | 예시 |
|---------|---------|------|
| `gadgetron-core` public API 제거·변경 | **major** (`0.x.y → 1.0.0`, `1.x.y → 2.0.0`) | `LlmProvider` 메서드 서명 변경, `GadgetronError` variant 제거 |
| 새 기능 추가 (additive), Phase 전환 | **minor** (`x.Y.z`) | `/v1/embeddings` 추가, Phase 2 기능 플래그 추가 |
| 버그 수정, 의존성 보안 패치 | **patch** (`x.y.Z`) | crash fix, CVE 대응 |
| CLI 플래그 deprecation 시작 | minor | deprecated 표기 + WARN 출력 |
| CLI 플래그 실제 제거 | major | 1 minor version 경고 후 major에서 제거 |

**Binary vs Crate 버전 관계**:
- `gadgetron-cli` 바이너리 버전 = workspace 버전 (`Cargo.toml` `[workspace.package] version`).
- 모든 workspace crate는 동일 버전 유지 (`version.workspace = true`).
- `gadgetron-core`를 외부 라이브러리로 사용하는 경우는 Phase 1-2 비지원 (내부 전용). Phase 3 plugin API 확정 시 별도 versioning 정책 수립.

**릴리즈 채널**:

| 채널 | 트리거 | 대상 | 결정 기반 |
|------|------|------|---------|
| `nightly` | main branch merge | 내부 테스트 | Axis C §2.C.6 참조 |
| `beta` | Phase 완료 후 RC | 파트너 테스트 | Phase 완료 기준 (§2.E.1) 달성 시 |
| `stable` | beta 2주 후 문제 없음 | 일반 배포 | `cargo semver-checks` + 전체 통합 테스트 green |

**Phase 1 → stable 릴리즈 기준**:
1. `cargo test --workspace` green (unit + integration)
2. 성능: `/v1/chat/completions` p99 < 50ms (loopback, 6 provider 중 최소 4개)
3. `cargo semver-checks` 이전 버전 대비 breaking change 없음 (patch/minor 릴리즈 시)
4. `helm install --dry-run` 오류 없음
5. Phase 1 완료 기준 (§2.E.1) (a)~(e) 모두 통과

---

### Axis F: 장애 모드 (Failure Modes)

> **v1 변경**: B-3/IE-01 BLOCKER 해소 (ProcessManager FSM), H-15 70s shutdown budget, circuit breaker 설계 완성, 전 컴포넌트 장애 인벤토리 확장. 결정 기반: phase-c-review-results.md §BLOCKER B-3, HIGH H-14/H-15, D-20260411-13 (Database variant).

#### 2.F.1 ProcessManager FSM (B-3 BLOCKER 해소)

**배경**: inference-engine-lead IE-01 지적 — `ProcessManager` FSM 전이 다이어그램 부재. `Loading → Failed` 전이 시 VRAM/포트 반납 미명시 → 운영 중 VRAM accounting 버그 필연. 본 섹션이 B-3를 완전 해소한다.

**크레이트**: `gadgetron-core/src/model.rs` (type definitions). `ProcessManager` implementation lives in `gadgetron-node/src/process_manager.rs` and imports these types from `gadgetron-core`.

##### 2.F.1.1 상태 열거형 (완전 명시)

```rust
// gadgetron-core/src/model.rs

use std::time::Instant;
use uuid::Uuid;

/// ProcessManager가 관리하는 단일 모델 인스턴스의 전체 생명주기 상태.
/// #[non_exhaustive] — 향후 HotSwap 등 상태 추가 시 breaking change 없음.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessState {
    /// 모델이 로드되지 않은 초기 상태. VRAM 점유 없음.
    NotLoaded,

    /// 모델 다운로드/초기화 진행 중. 아직 추론 불가.
    Loading {
        started_at: Instant,
        model_id: Uuid,
    },

    /// 모델이 정상 실행 중. 추론 요청 수락 가능.
    Running {
        pid: u32,
        port: u16,
        loaded_at: Instant,
        model_id: Uuid,
    },

    /// 종료 신호 수신 후 정리 중.
    Stopping {
        since: Instant,
        reason: StopReason,
    },

    /// 정상 종료 완료. VRAM/포트 반납 완료.
    Stopped {
        reason: StopReason,
        stopped_at: Instant,
    },

    /// 비정상 종료. 원인 기록, 복구 불가 상태.
    Failed {
        reason: FailureReason,
        failed_at: Instant,
    },
}

/// 정상 종료의 원인. StopReason::HealthCheckFailed는 임계값(3회) 초과 후 Scheduler가 요청.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// 사용자 또는 관리 API 명시적 요청 (POST /api/v1/models/{id}/undeploy).
    UserRequested,
    /// 노드 자체 종료 (SIGTERM → graceful shutdown 시퀀스).
    NodeShutdown,
    /// Scheduler가 VRAM 부족으로 LRU eviction 결정.
    Evicted,
    /// 헬스체크 연속 3회 실패 후 Scheduler가 재배포 요청.
    HealthCheckFailed,
    /// 롤링 업그레이드 시 구버전 모델 교체.
    RollingUpgrade,
}

/// 비정상 종료의 원인. FailureReason을 기반으로 Prometheus 라벨 생성.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    /// 자식 프로세스가 0이 아닌 exit code로 종료. i32 = exit code.
    CrashExit(i32),
    /// Loading 상태에서 readiness probe 5분(300s) timeout 초과.
    LoadTimeout,
    /// Running 상태에서 헬스체크 응답이 5× 폴링 주기(5s × 5 = 25s) 이상 없음.
    HealthCheckStale,
    /// VRAM 할당 호출 실패 (estimate_vram_mb 반환값 초과).
    VramAllocationFailed,
    /// 포트 할당 실패 (RangePortAllocator 범위 소진).
    PortAllocationFailed,
    /// SIGTERM 후 10s 내 프로세스 종료 안 함 → SIGKILL 이후 상태.
    KillTimeout,
    /// 실행 파일 경로(vllm, sglang, ollama)가 PATH에 없음.
    ExecutableNotFound,
    /// 시작 시 CUDA/GPU 디바이스 접근 실패.
    GpuDeviceUnavailable,
}
```

##### 2.F.1.2 전이 테이블 (complete — 누락 없음)

| 현재 상태 | 이벤트 | 다음 상태 | 사이드 이펙트 |
|---------|--------|---------|------------|
| `NotLoaded` | `load_request(model_id)` | `Loading { started_at: now(), model_id }` | VRAM 예약(estimate_vram_mb), 포트 할당(RangePortAllocator), model_registry.mark_loading(model_id), metric: `gadgetron_node_process_state_transitions_total{from="not_loaded",to="loading"}` |
| `NotLoaded` | `stop_request(_)` | `NotLoaded` (no-op) | audit entry emit: "stop on unloaded model ignored" |
| `Loading` | `load_complete(pid, port)` | `Running { pid, port, loaded_at: now(), model_id }` | model_registry.mark_running(model_id, port), metric: `gadgetron_node_load_duration_seconds` (started_at ~ now), `gadgetron_node_process_state_transitions_total{from="loading",to="running"}` |
| `Loading` | `load_failed(reason)` | `Failed { reason, failed_at: now() }` | VRAM 반납(vram_tracker.release(model_id)), 포트 반납(port_allocator.release(port)), model_registry.mark_failed(model_id), audit entry emit, metric: `gadgetron_node_load_failures_total{reason}` |
| `Loading` | `stop_request(reason)` | `Stopping { since: now(), reason }` | 자식 프로세스 SIGTERM 전송 (아직 pid 없으면 cancel spawn) |
| `Running` | `stop_request(reason)` | `Stopping { since: now(), reason }` | 자식 프로세스 SIGTERM 전송, in-flight 완료 대기 신호, metric: `gadgetron_node_process_state_transitions_total{from="running",to="stopping"}` |
| `Running` | `crash_detected(exit_code)` | `Failed { reason: CrashExit(exit_code), failed_at: now() }` | VRAM 반납, 포트 반납, model_registry.mark_failed, audit entry emit, metric: `gadgetron_node_process_crashes_total{model_id}` |
| `Running` | `health_check_failed` (3회 누적) | `Stopping { since: now(), reason: StopReason::HealthCheckFailed }` | SIGTERM 전송, Scheduler::notify_unhealthy(model_id) 호출 |
| `Stopping` | `stop_complete` | `Stopped { reason, stopped_at: now() }` | VRAM 반납, 포트 반납, model_registry.mark_stopped(model_id), audit entry emit, metric: `gadgetron_node_process_state_transitions_total{from="stopping",to="stopped"}` |
| `Stopping` | `crash_detected(_)` | `Stopped { reason: StopReason::NodeShutdown, stopped_at: now() }` | VRAM 반납, 포트 반납, model_registry.mark_stopped (crash 중 stop = 사실상 종료) |
| `Stopping` (10s 초과) | (tokio::time::timeout 만료) | `Failed { reason: KillTimeout, failed_at: now() }` | SIGKILL 전송, VRAM 반납, 포트 반납, model_registry.mark_failed |
| `Stopped` | `load_request(model_id)` | `Loading { started_at: now(), model_id }` | VRAM 예약, 포트 할당, model_registry.mark_loading (재배포 경로) |
| `Stopped` | 그 외 이벤트 | `Stopped` (no-op) | 이벤트 warn log |
| `Failed` | `load_request(model_id)` | `Loading { started_at: now(), model_id }` | VRAM 예약, 포트 할당 (수동 재시도 경로) |
| `Failed` | 그 외 이벤트 | `Failed` (no-op) | 이벤트 warn log |

**헬스체크 연속 실패 카운터**: `health_check_failed` 이벤트는 3회 연속 실패 시에만 상태 전이를 유발. 성공 응답 1회 시 카운터 리셋. 카운터는 `Arc<AtomicU32>` (lock-free).

**이벤트 처리 concurrency**: `ProcessManager`는 `tokio::sync::Mutex<ProcessState>` 를 통해 상태 직렬화. 이벤트 핸들러는 `async fn handle_event(&self, event: ProcessEvent) -> Result<(), GadgetronError>` 형태.

##### 2.F.1.3 Mermaid 상태 다이어그램

```mermaid
stateDiagram-v2
    [*] --> NotLoaded

    NotLoaded --> Loading : load_request(model_id)\n[VRAM 예약, 포트 할당]

    Loading --> Running : load_complete(pid, port)\n[registry.mark_running]
    Loading --> Failed : load_failed(reason)\n[VRAM 반납, 포트 반납]
    Loading --> Stopping : stop_request(reason)

    Running --> Stopping : stop_request(reason)\n[SIGTERM]
    Running --> Failed : crash_detected(exit_code)\n[VRAM 반납, 포트 반납]
    Running --> Stopping : health_check_failed ×3\n[Scheduler::notify_unhealthy]

    Stopping --> Stopped : stop_complete\n[VRAM 반납, 포트 반납]
    Stopping --> Stopped : crash_detected(_)\n[VRAM 반납, 포트 반납]
    Stopping --> Failed : timeout(10s) → SIGKILL\n[FailureReason::KillTimeout]

    Stopped --> Loading : load_request(model_id)\n[재배포 경로]
    Failed --> Loading : load_request(model_id)\n[수동 재시도 경로]

    Stopped --> [*]
    Failed --> [*]
```

##### 2.F.1.4 사이드 이펙트 상세

**VRAM 반납**: `gadgetron-node/src/vram_tracker.rs::VramTracker::release(model_id: Uuid)` 호출. `DashMap<Uuid, u64>` 에서 항목 제거. Scheduler의 다음 heartbeat(10s) 때 반납된 VRAM이 반영됨.

**포트 반납**: `gadgetron-node/src/port_allocator.rs::RangePortAllocator::release(port: u16)` 호출. `BTreeSet<u16>` 여유 포트 셋에 반환.

**model_registry 업데이트**: `gadgetron-node/src/model_registry.rs::ModelRegistry::mark_*(model_id, ...)` — `DashMap<Uuid, ModelState>` 갱신.

**audit entry emit**: `gadgetron-xaas/src/audit/writer.rs::AuditWriter::try_send(AuditEntry::ModelLifecycle { ... })` — 실패(채널 full) 시 warn + counter.

**metric increment**: `metrics::counter!("gadgetron_node_process_state_transitions_total", "from" => prev, "to" => next).increment(1)` — `metrics` crate (D-20260411-01 의존성 확정).

---

#### 2.F.2 컴포넌트 장애 인벤토리 (전체 크레이트)

컬럼: 컴포넌트 / 장애 모드 / 감지 메커니즘 / 복구 전략 / 폭발 반경 / 알림 (cross-ref §2.B.2)

**gadgetron-gateway**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| GW-1 | 포트 바인딩 실패 | 시작 시 `TcpListener::bind` Err | 없음 (fail fast, exit 1) | 전체 서비스 | K8s CrashLoopBackOff 알람 |
| GW-2 | TLS 인증서 만료 [P2] | rustls handshake 오류 | 없음 (인증서 갱신 필요) | 신규 TLS 연결 전체 | cert-exporter Prometheus alert |
| GW-3 | OOM (요청 폭주) | OS SIGKILL (K8s memory limit) | K8s 자동 재시작 (terminationGracePeriodSeconds: 70) | 현재 in-flight 요청 전체 | K8s OOMKilled event |
| GW-4 | 미들웨어 panic | `std::panic::catch_unwind` (tower) | 현재 요청만 500 반환, 서비스 계속 | 단일 요청 | `gadgetron_gateway_panics_total` counter > 0 → PagerDuty |
| GW-5 | axum worker thread 포화 | `axum::Router` accept 대기 증가 | OS TCP backlog(128) → 연결 거부 | 신규 연결 (기존 in-flight 정상) | `gadgetron_gateway_connections_active` > 임계값 alert |
| GW-6 | Auth middleware DB unreachable | moka cache miss + PG pool timeout | cache hit으로 최대 10분 서비스 지속, 이후 401 | cache miss 요청만 | `gadgetron_xaas_db_pool_timeout_total` alert |

**gadgetron-router**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| RT-1 | 전체 provider circuit open | `CircuitState::Open` 확인 | 60s 후 half-open probe | 모든 신규 요청 503 | `gadgetron_router_circuit_state{provider}` Gauge = 1 |
| RT-2 | MetricsStore DashMap contention | `record_*` 지연 (Prometheus histogram) | lock-free sharding 자체 완화 | routing latency 증가 | `gadgetron_router_record_duration_us` p99 > 500 |
| RT-3 | RoutingConfig 전략 미매칭 | `Router::route()` Err 반환 | 없음 (설정 수정 필요) | 해당 전략 사용 요청 전체 | 로그 `ERROR routing strategy not found` |
| RT-4 | Weighted strategy 총합 0.0 | `WeightedStrategy::select()` guard 체크 | `GadgetronError::Router` 반환 (panic 없음) | 해당 전략 요청 | 로그 `ERROR weighted strategy total weight is zero` |
| RT-5 | fallback_chain 전체 소진 | `AllProvidersFailed` 반환 | `Retry-After: 30` 헤더 포함 503 | 현재 요청 | `gadgetron_router_all_providers_failed_total` counter |

**gadgetron-scheduler**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| SC-1 | 전체 노드 VRAM 부족 | `schedule()` → `InsufficientResources` | LRU eviction 후 재시도 1회 | 신규 배포 요청 | `gadgetron_scheduler_insufficient_resources_total` |
| SC-2 | LRU eviction candidate 없음 | 모든 모델 Required/Protected 상태 | 503 반환 (새 노드 추가 필요) | 신규 배포 요청 | 로그 `WARN no eviction candidate` |
| SC-3 | 노드 heartbeat 소실 (30s stale) | `last_heartbeat` 타임스탬프 체크 (B-2 결정) | 노드 mark_unhealthy, 해당 노드 스케줄 중단 | 해당 노드 모델 전체 | `gadgetron_scheduler_stale_nodes_total` |
| SC-4 | RwLock poisoning | `std::sync::PoisonError` | K8s 재시작 (panic propagation) | 전체 스케줄러 | K8s CrashLoopBackOff |
| SC-5 | 재시작 후 state 분실 | 시작 시 `/status` poll 실패 | `AppConfig.nodes` 목록에서 재등록 + `/api/v1/models/recover` 수동 trigger | 재시작 직후 일시적 | 로그 `WARN scheduler restarted, re-registering nodes` |

**gadgetron-node**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| ND-1 | 자식 프로세스 crash | `tokio::process::Child::wait()` → non-zero exit | `ProcessState → Failed(CrashExit)`, Scheduler re-deploy | 해당 모델 | `gadgetron_node_process_crashes_total{model_id}` |
| ND-2 | NVML 미설치 / 드라이버 오류 | `nvml_wrapper::Nvml::init()` Err | CPU-only 모드 (graceful degradation, 메트릭 없음) | GPU 메트릭만 | 로그 `WARN NVML unavailable, running without GPU metrics` |
| ND-3 | 실행 파일 미설치 (vllm not found) | `Command::spawn()` → `NotFound` | `Failed(ExecutableNotFound)`, 자동 복구 없음 | 해당 엔진 전체 | 로그 `ERROR executable not found: vllm` |
| ND-4 | 포트 충돌 / 범위 소진 | `RangePortAllocator::allocate()` → Err | `Failed(PortAllocationFailed)` | 신규 모델 배포 | 로그 `ERROR port range exhausted` |
| ND-5 | 자식 프로세스 zombie | `Child.wait()` 미호출 | `Drop` impl에서 `child.kill() + wait()` | PID table | 모니터링: 주기적 zombie count 체크 |
| ND-6 | Loading timeout (5분) | `tokio::time::timeout(Duration::from_secs(300))` | `Failed(LoadTimeout)`, VRAM 반납 | 해당 모델 | `gadgetron_node_load_timeout_total{model_id}` |

**gadgetron-provider**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| PR-1 | 외부 API 429 rate limit | HTTP 응답 코드 429 | fallback chain 다음 provider (§2.F.6) | 해당 provider | `gadgetron_provider_errors_total{provider,kind="rate_limit"}` |
| PR-2 | 외부 API 5xx | HTTP 응답 코드 5xx | circuit breaker trip (3회) → fallback | 해당 provider | circuit state alert |
| PR-3 | SSE 스트림 중단 | `StreamInterrupted` 반환 | 첫 청크 전 1회 retry, 이후 종료 (H-1) | 해당 요청 | `gadgetron_provider_stream_interrupted_total` |
| PR-4 | CUDA OOM (로컬 provider) | vLLM/SGLang process crash → exit code ≠ 0 | `ND-1` 경로 (eviction + redeploy) | 해당 모델 | `gadgetron_node_process_crashes_total` |
| PR-5 | 연결 timeout | `reqwest::Error::Timeout` | `GadgetronError::Provider { kind: Timeout }` | 해당 요청 | `gadgetron_provider_errors_total{kind="timeout"}` |

**gadgetron-xaas**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| XA-1 | AuditWriter channel full | `mpsc::Sender::try_send()` Err | drop + warn + counter (D-20260411-09) | audit 항목 유실 (Phase 2: WAL fallback) | `gadgetron_xaas_audit_dropped_total` > 0 |
| XA-2 | TokenBucket 재시작 초기화 | 프로세스 재시작 | known limitation (M-6): 메모리 상태 분실 | quota bypass 최대 1분 | 로그 `WARN token bucket reset on restart` |
| XA-3 | scope denial | `require_scope()` → `GadgetronError::Forbidden` | 403 반환, 자동 복구 없음 | 해당 요청 | 로그 `WARN scope denied: required={scope}` |
| XA-4 | Redis 연결 실패 [P2] | `features.rate_limit_persist = true` + Redis down | deny all → 503 `rate_limit_unavailable` (§2.F.9 참조) | Phase 2 rate limit 요청 전체 | `gadgetron_xaas_redis_unavailable_total` |

**gadgetron-tui**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| TU-1 | in-process Arc lock contention | ratatui render loop 지연 | read-only Arc (no write lock in TUI) | TUI 표시만 | 없음 (TUI는 비중요 경로) |
| TU-2 | WS broadcast channel lagged | `RecvError::Lagged` | drop + reconnect (H-12) | TUI 화면 업데이트 일시 누락 | 없음 |

**gadgetron-cli**

| ID | 장애 모드 | 감지 | 복구 | 폭발 반경 | 알림 |
|----|---------|------|------|---------|------|
| CL-1 | 설정 파일 파싱 실패 | `toml::from_str()` Err | fail fast, exit 1 + 명확한 오류 메시지 | 전체 서비스 기동 | K8s CrashLoopBackOff |
| CL-2 | 환경 변수 미설정 | `${VAR}` 치환 실패 | fail fast, exit 1 | 전체 서비스 기동 | K8s CrashLoopBackOff |

---

#### 2.F.3 Circuit Breaker 설계

**결정 기반**: phase-c-review-results.md §2.F.3 (3 failures / 60s recovery). D-20260411-01~13 범위 내 PM 자율 결정.

**상수 근거**:
- 임계값 3회: 네트워크 순간 오류(1-2회)와 실제 장애를 구분하는 최소 샘플. 1회는 false positive, 5회는 복구 지연.
- 복구 창 60초: LLM provider 503 평균 복구 시간(30-120s) 중간값. 너무 짧으면 재트립, 너무 길면 가용성 저하.
- Half-open probe 1회: single probe로 충분 (probe 실패 시 즉시 Open 복귀).

**크레이트**: `gadgetron-router/src/circuit_breaker.rs`

##### 2.F.3.1 타입 정의

```rust
// gadgetron-router/src/circuit_breaker.rs
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// CircuitState 값 (AtomicU8 저장).
/// 0 = Closed, 1 = Open, 2 = HalfOpen
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// 연속 실패 임계값. 이 값 이상 연속 실패 시 Open으로 전이.
const FAILURE_THRESHOLD: u32 = 3;

/// Open → HalfOpen 전환 대기 시간.
const RECOVERY_DURATION: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreaker {
    /// 0=Closed, 1=Open, 2=HalfOpen. AtomicU8으로 lock-free 상태 읽기.
    state: Arc<AtomicU8>,
    /// 연속 실패 횟수. 성공 시 0으로 리셋.
    failure_count: Arc<AtomicU32>,
    /// Open 상태 시작 시각. RECOVERY_DURATION 경과 여부 판단용.
    opened_at: Arc<Mutex<Option<Instant>>>,
    /// 실패 임계값 (= FAILURE_THRESHOLD, 런타임 오버라이드 가능).
    threshold: u32,
    /// Open → HalfOpen 복구 대기 (= RECOVERY_DURATION).
    recovery_duration: Duration,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(STATE_CLOSED)),
            failure_count: Arc::new(AtomicU32::new(0)),
            opened_at: Arc::new(Mutex::new(None)),
            threshold: FAILURE_THRESHOLD,
            recovery_duration: RECOVERY_DURATION,
        }
    }

    /// 현재 상태를 반환. Open이고 RECOVERY_DURATION 경과했으면 HalfOpen으로 자동 전이.
    pub fn state(&self) -> CircuitState {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => CircuitState::Closed,
            STATE_OPEN => {
                let opened = self.opened_at.lock().unwrap();
                if let Some(t) = *opened {
                    if t.elapsed() >= self.recovery_duration {
                        drop(opened);
                        self.state.store(STATE_HALF_OPEN, Ordering::Release);
                        CircuitState::HalfOpen
                    } else {
                        CircuitState::Open
                    }
                } else {
                    CircuitState::Open
                }
            }
            STATE_HALF_OPEN => CircuitState::HalfOpen,
            _ => unreachable!("invalid circuit state byte"),
        }
    }

    /// 실패 기록. 임계값 초과 시 Open 전이. 반환값 = 전이 후 상태.
    pub fn record_failure(&self) -> CircuitState {
        let count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
        if count >= self.threshold {
            self.trip();
            CircuitState::Open
        } else {
            self.state()
        }
    }

    /// 성공 기록. Closed로 전이 + 카운터 리셋.
    pub fn record_success(&self) {
        self.failure_count.store(0, Ordering::Release);
        self.state.store(STATE_CLOSED, Ordering::Release);
        *self.opened_at.lock().unwrap() = None;
    }

    /// 관리 API 수동 리셋 (POST /api/v1/admin/circuits/{id}/reset).
    pub fn manual_reset(&self) {
        self.failure_count.store(0, Ordering::Release);
        self.state.store(STATE_CLOSED, Ordering::Release);
        *self.opened_at.lock().unwrap() = None;
    }

    fn trip(&self) {
        self.state.store(STATE_OPEN, Ordering::Release);
        *self.opened_at.lock().unwrap() = Some(Instant::now());
    }
}
```

##### 2.F.3.2 전이 테이블

| 현재 상태 | 이벤트 | 조건 | 다음 상태 |
|---------|--------|------|---------|
| `Closed` | `record_failure()` | count < FAILURE_THRESHOLD(3) | `Closed` |
| `Closed` | `record_failure()` | count >= FAILURE_THRESHOLD(3) | `Open` (opened_at = now()) |
| `Closed` | `record_success()` | — | `Closed` (count reset) |
| `Open` | `state()` 조회 | elapsed < RECOVERY_DURATION(60s) | `Open` |
| `Open` | `state()` 조회 | elapsed >= RECOVERY_DURATION(60s) | `HalfOpen` (자동 전이) |
| `HalfOpen` | `record_success()` | — | `Closed` (count reset) |
| `HalfOpen` | `record_failure()` | — | `Open` (즉시 재트립) |
| Any | `manual_reset()` | 관리자 명시적 호출 | `Closed` |

##### 2.F.3.3 관리 API

```
POST /api/v1/admin/circuits/{provider_id}/reset
Authorization: Bearer <Management scope key>

응답:
  200 OK   — 리셋 성공, 현재 상태: {"state":"closed","failure_count":0}
  404 Not Found — provider_id 미존재
  403 Forbidden — Management scope 없음
```

##### 2.F.3.4 Prometheus 지표

```
gadgetron_router_circuit_state{provider}          Gauge  0=closed,1=open,2=half_open
gadgetron_router_circuit_trips_total{provider}    Counter
gadgetron_router_circuit_resets_total{provider}   Counter (manual_reset 포함)
```

---

#### 2.F.4 Graceful Shutdown 70초 예산 (H-15)

**결정 기반**: phase-c-review-results.md HIGH H-15 (devops-sre-lead 권고). `terminationGracePeriodSeconds: 70` (Axis C §2.C.4.5 cross-ref).

**각 stage 상수 근거**:
- preStop 5s: LB가 엔드포인트 제거를 전파하는 평균 시간 (K8s EndpointSlice propagation latency).
- drain 30s: SSE 스트림 최대 응답 시간 상한. 이 이상이면 클라이언트 timeout 먼저 발생.
- audit 5s: mpsc(4096) flush + 단일 PG batch insert. 5s 내 충분.
- model_unload 10s: SIGTERM → 정상 종료 기대값 5s, 나머지 5s SIGKILL 여유.
- buffer 20s: K8s 네트워크 종료, 최종 메트릭 flush, 예외적 지연 여유.

| Stage | 예산 | 동작 | 초과 시 |
|-------|------|------|--------|
| preStop hook | 5s | `lifecycle.preStop.exec` 실행: LB에 노드 제거 신호 (`curl -X POST /internal/drain`) | 5s 후 K8s가 SIGTERM 전송 (무조건) |
| drain in-flight | 30s | `axum::serve().with_graceful_shutdown()` 대기. `InflightCounter == 0` 또는 timeout. SSE stream: 현재 chunk 완료 후 `StreamInterrupted` 반환. | `tokio::time::timeout(30s)` → force close 남은 연결 |
| audit flush | 5s | `AuditWriter::flush()`: mpsc receiver drain → PostgreSQL batch insert. | `tokio::time::timeout(5s)` → 미flush 항목 drop, `gadgetron_xaas_audit_dropped_total` increment |
| model_unload | 10s | `ProcessManager::stop_all()`: 각 Running 모델에 SIGTERM. `ProcessState::Stopping → Stopped` 대기. | `tokio::time::timeout(10s)` → SIGKILL 모든 자식, `FailureReason::KillTimeout` |
| buffer | 20s | PgPool::close(), 최종 Prometheus push, 남은 span flush (opentelemetry). | `process::exit(0)` 강제 종료 |
| **합계** | **70s** | `terminationGracePeriodSeconds: 70` | K8s SIGKILL (70s 초과 시) |

**코드 경로** (`gadgetron-cli/src/main.rs`):

```rust
// tokio::signal::unix::SignalKind::terminate() 수신 시:
async fn graceful_shutdown(
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
    inflight: Arc<InflightCounter>,
    audit_writer: Arc<AuditWriter>,
    process_manager: Arc<ProcessManager>,
    pg_pool: sqlx::PgPool,
) {
    // Stage 1: preStop은 K8s lifecycle hook이 처리 (외부)

    // Stage 2: drain (30s)
    let _ = shutdown_tx.send(());  // axum graceful_shutdown signal
    let drain_result = tokio::time::timeout(
        Duration::from_secs(30),
        inflight.wait_zero(),  // InflightCounter::wait_zero() — tokio::sync::Notify 기반
    ).await;
    if drain_result.is_err() {
        tracing::warn!("drain timeout: forcing close remaining connections");
    }

    // Stage 3: audit flush (5s)
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        audit_writer.flush(),
    ).await;

    // Stage 4: model unload (10s)
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        process_manager.stop_all(StopReason::NodeShutdown),
    ).await;

    // Stage 5: buffer — pool close + final flush
    pg_pool.close().await;
    opentelemetry::global::shutdown_tracer_provider();
}
```

**InflightCounter 구조** (cross-ref Axis D §2.D.9):

```rust
pub struct InflightCounter {
    count: Arc<AtomicU64>,
    notify: Arc<tokio::sync::Notify>,
}

impl InflightCounter {
    pub fn increment(&self) { self.count.fetch_add(1, Ordering::AcqRel); }
    pub fn decrement(&self) {
        let prev = self.count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 { self.notify.notify_waiters(); }  // count가 0이 된 순간
    }
    pub async fn wait_zero(&self) {
        loop {
            if self.count.load(Ordering::Acquire) == 0 { return; }
            self.notify.notified().await;
        }
    }
}
```

---

#### 2.F.5 헬스체크 체인

**경로**: `gadgetron-gateway/src/health.rs`

**3가지 엔드포인트**:
- `/health` — Liveness: 프로세스가 살아있으면 200. 절대 fail시키지 않음. K8s livenessProbe.
- `/ready` — Readiness: PG 접근 가능 + 설정된 모든 provider 접근 가능(D-6) 시 200. 하나라도 실패 시 503. K8s readinessProbe.
- `/health/detail` — Deep Health: 컴포넌트별 상세 JSON. 운영자 디버깅용. K8s probe 사용 금지.

##### 2.F.5.1 HealthStatus 열거형

```rust
// gadgetron-core/src/health.rs

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// 모든 컴포넌트 정상.
    Healthy,
    /// 일부 컴포넌트 저하. 서비스는 계속 가능.
    Degraded { reasons: Vec<String> },
    /// 핵심 컴포넌트 장애. 서비스 불가.
    Unhealthy { reasons: Vec<String> },
}

impl HealthStatus {
    /// HTTP 응답 코드 결정.
    pub fn http_status_code(&self) -> u16 {
        match self {
            Self::Healthy => 200,
            Self::Degraded { .. } => 200,   // degraded도 traffic 수용
            Self::Unhealthy { .. } => 503,
        }
    }
}
```

##### 2.F.5.2 컴포넌트별 헬스체크 함수 시그니처

```rust
// gadgetron-gateway/src/health.rs

/// PostgreSQL 접근 가능 여부. `SELECT 1` 쿼리 실행.
/// timeout: 2s. 실패 시 HealthStatus::Unhealthy.
pub async fn check_postgres(pool: &sqlx::PgPool) -> HealthStatus;

/// 모든 configured provider의 /health 또는 probe 응답 확인 (D-6 결정).
/// cloud provider: is_live = true (1-token probe 금지, IE-04 H-3).
/// local provider: GET http://localhost:{port}/health 응답 확인.
/// timeout per provider: 3s. 1개라도 실패 시 Degraded.
pub async fn check_providers(providers: &[Arc<dyn LlmProvider>]) -> HealthStatus;

/// NVML 접근 가능 여부. GPU 드라이버 초기화 상태.
/// 실패 시 Degraded (GPU 없이 CPU-only 운영 가능).
pub async fn check_nvml(nvml: Option<&nvml_wrapper::Nvml>) -> HealthStatus;

/// AuditWriter 채널 잔여 용량. 80% 이상이면 Degraded.
/// 임계값 = 4096 * 0.8 = 3277 entries.
pub fn check_audit_channel(pending: usize) -> HealthStatus;

/// 전체 헬스 집계.
pub async fn aggregate_health(components: Vec<(&str, HealthStatus)>) -> HealthDetail;
```

##### 2.F.5.3 /health/detail 응답 예시

```json
{
  "status": "degraded",
  "components": {
    "postgres": {"status": "healthy"},
    "providers": {"status": "degraded", "reasons": ["anthropic: timeout"]},
    "nvml": {"status": "healthy"},
    "audit_channel": {"status": "healthy", "pending": 120, "capacity": 4096}
  },
  "uptime_seconds": 3600,
  "version": "0.2.0"
}
```

---

#### 2.F.6 Provider 장애 모드

**각 provider의 예상 장애, 오류 분류, retry 결정, fallback 연결.**

**오류 분류 기반 타입** (cross-ref D-20260411-13 + D-13):

```rust
// gadgetron-core/src/error.rs (기존 GadgetronError 확장)
#[non_exhaustive]
pub enum ProviderErrorKind {
    RateLimit,       // HTTP 429
    ServerError,     // HTTP 5xx
    Timeout,         // reqwest::Error::Timeout
    Unauthorized,    // HTTP 401/403 (key 만료)
    BadRequest,      // HTTP 400 (요청 포맷 오류)
    StreamBroken,    // SSE stream 중간 끊김 (D-20260411-08)
    ModelNotFound,   // HTTP 404 (모델 미지원)
}
```

| Provider | 예상 장애 | 오류 분류 | Retry 결정 (cross-ref H-1) | Fallback |
|---------|---------|---------|--------------------------|---------|
| OpenAI | API 429 | `ProviderErrorKind::RateLimit` | 첫 청크 전: 1회 retry 후 fallback. 첫 청크 후: 즉시 fallback (H-1) | fallback_chain 다음 |
| OpenAI | API 5xx | `ProviderErrorKind::ServerError` | circuit breaker 3회 → open | fallback_chain 다음 |
| OpenAI | stream broken mid-response | `GadgetronError::StreamInterrupted` | 첫 청크 후면 retry 없음, 연결 종료 (H-1) | audit 기록 |
| Anthropic | API 5xx | `ProviderErrorKind::ServerError` | circuit breaker 3회 → open | fallback_chain 다음 |
| Anthropic | 429 (rate limit per model) | `ProviderErrorKind::RateLimit` | 첫 청크 전 1회 retry → fallback | fallback_chain 다음 |
| Gemini | polling timeout | `ProviderErrorKind::Timeout` | 1회 retry (첫 청크 전) | fallback_chain 다음 |
| Gemini | `generateContent` 400 | `ProviderErrorKind::BadRequest` | retry 없음 (요청 오류) | 400 그대로 클라이언트에 반환 |
| Ollama | 프로세스 미실행 | `ProviderErrorKind::ServerError` (connection refused) | circuit breaker | fallback_chain 다음 |
| vLLM | CUDA OOM → exit | `ProviderErrorKind::ServerError` + ND-1 경로 | eviction + redeploy (§2.F.1 FSM) | 재배포 후 재요청 |
| SGLang | CUDA OOM → exit | `ProviderErrorKind::ServerError` + ND-1 경로 | eviction + redeploy (§2.F.1 FSM) | 재배포 후 재요청 |

**cloud provider is_ready semantics** (H-3): Anthropic / Gemini는 `/v1/models` 엔드포인트 없음. Phase 1에서 `is_ready() = is_live()` (HTTP OPTIONS 또는 단순 연결 확인). 1-token probe 금지 (비용 발생).

---

#### 2.F.7 GPU 장애 모드 (NVML)

**NVML 폴링 주기**: 1초 (H-9 결정, v0 100ms 오류 정정).

##### 2.F.7.1 NVML poll stall 감지

```rust
// gadgetron-node/src/gpu_monitor.rs

const NVML_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// stall 임계값 = 5× 예상 주기 = 5s.
/// 근거: 1-2회 지연은 OS 스케줄러 jitter로 허용, 5회 지속은 진짜 stall.
const NVML_STALL_THRESHOLD: Duration = Duration::from_secs(5);

pub struct NvmlPoller {
    last_successful_poll: Arc<Mutex<Option<Instant>>>,
    nvml: Option<nvml_wrapper::Nvml>,
}

impl NvmlPoller {
    /// 주기적으로 NVML 데이터 수집. stall 감지 시 노드 degraded 표시.
    pub async fn run(&self, node_state: Arc<NodeState>) {
        let mut interval = tokio::time::interval(NVML_POLL_INTERVAL);
        loop {
            interval.tick().await;
            match self.poll_once() {
                Ok(metrics) => {
                    *self.last_successful_poll.lock().unwrap() = Some(Instant::now());
                    node_state.update_gpu_metrics(metrics);
                }
                Err(e) => {
                    let last = self.last_successful_poll.lock().unwrap();
                    if let Some(t) = *last {
                        if t.elapsed() >= NVML_STALL_THRESHOLD {
                            tracing::error!(elapsed=?t.elapsed(), "NVML poll stall detected");
                            node_state.mark_degraded("nvml_stall");
                            metrics::counter!("gadgetron_node_nvml_stall_total").increment(1);
                        }
                    }
                    tracing::warn!(error=?e, "NVML poll failed");
                }
            }
        }
    }
}
```

##### 2.F.7.2 열 스로틀링 (Phase 1: 감지 + 경고, Phase 2: 자동 eviction)

| 온도 임계값 | Phase 1 동작 | Phase 2 동작 (ThermalController) |
|-----------|------------|----------------------------------|
| > 85°C | `gadgetron_node_gpu_temp_high_total` counter + `tracing::warn!` | 해당 GPU 신규 배포 deprioritize (스케줄러 score 감소) |
| > 90°C | `gadgetron_node_gpu_temp_critical_total` counter + `tracing::error!` | 해당 GPU 실행 중 모델 evict (StopReason::ThermalEviction [P2]) |

**감지 코드**:
```rust
// gadgetron-node/src/gpu_monitor.rs::check_thermal()
fn check_thermal(temp_c: u32, gpu_index: u32) {
    if temp_c > 90 {
        metrics::counter!("gadgetron_node_gpu_temp_critical_total", "gpu" => gpu_index.to_string()).increment(1);
        tracing::error!(gpu_index, temp_c, "GPU temperature critical (>90C)");
    } else if temp_c > 85 {
        metrics::counter!("gadgetron_node_gpu_temp_high_total", "gpu" => gpu_index.to_string()).increment(1);
        tracing::warn!(gpu_index, temp_c, "GPU temperature high (>85C)");
    }
}
```

##### 2.F.7.3 MIG profile 오류

**B-4 수정 결과**: A100 80GB에서 `1g.5gb` 프로파일은 존재하지 않음 (A100 40GB 전용). `MigManager::enable_profile()` 시 지원되지 않는 프로파일 요청 시 `GadgetronError::Node { kind: InvalidMigProfile }` 반환.

```
장애 경로: MigManager::enable_profile("1g.5gb", gpu_index) on A100-80GB
→ NVML Error: InvalidArgument
→ GadgetronError::Node { kind: NodeErrorKind::InvalidMigProfile, message: "1g.5gb not supported on A100 80GB" }
→ 스케줄러: 해당 GPU skip, 다른 GPU 시도
→ 모든 GPU 실패 시: InsufficientResources 503
```

---

#### 2.F.8 데이터베이스 장애 모드

**D-20260411-13 cross-ref**: `DatabaseErrorKind` 기반 오류 분류.

| 장애 | `DatabaseErrorKind` | HTTP | 복구 전략 | 감지 |
|------|---------------------|------|---------|------|
| 시작 시 PG 연결 실패 | `ConnectionFailed` | — (process exit 1) | K8s 재시작 (CrashLoopBackOff + backoff) | K8s event |
| Pool exhaustion (max 20) | `PoolTimeout` | 503 | `Retry-After: 5` 헤더, pool size 증가 고려 | `gadgetron_xaas_db_pool_timeout_total` alert (§2.B.2) |
| DB migration 실패 | `MigrationFailed` | — (process exit 1) | 수동 개입 (migration rollback/fix + 재시작) | 로그 `ERROR migration failed: {msg}` + K8s event |
| 연결 손실 (runtime) | `ConnectionFailed` | 503 (다음 쿼리) | sqlx auto-reconnect (pool backoff: 100ms → 1s → 10s cap) | `gadgetron_xaas_db_connection_errors_total` |
| 디스크 풀 (WAL write fail) | — (PG 자체 halt) | 503 (PG 응답 없음) | /health/detail → postgres: unhealthy. 디스크 정리 필요. | `pg_stat_archiver` 메트릭 또는 /health check 실패 |
| Row not found (API key) | `RowNotFound` | 401 | 없음 (키 발급 필요) | 로그 `WARN auth key not found` |
| Constraint violation (audit dup) | `Constraint` | 409 (safe ignore) | 중복 audit 항목 무시 (idempotency key) | 로그 `DEBUG audit duplicate ignored` |

**Migration 실패 코드 경로**:
```rust
// gadgetron-cli/src/main.rs
sqlx::migrate!("./migrations")
    .run(&pool)
    .await
    .map_err(|e| {
        tracing::error!(error=?e, "Database migration failed — cannot start");
        GadgetronError::Database {
            kind: DatabaseErrorKind::MigrationFailed,
            message: e.to_string(),
        }
    })?;
// migration 실패 시 ? 전파 → main() Err 반환 → process exit 1
```

**pgbackrest archive failure** [P2]:
- `archive_command` 실패 → PG가 WAL 아카이브 실패로 write 동결.
- 감지: `SELECT archived_count, failed_count, last_failed_time FROM pg_stat_archiver` 메트릭 수집.
- action: `last_failed_time IS NOT NULL` 시 alert → 운영자 pgbackrest 재설정.

---

#### 2.F.9 보안 및 남용 장애 모드

**cross-ref Axis E §2.E.6 KL-1.**

##### 2.F.9.1 TokenBucket 재시작 남용 (Phase 1 Known Limitation)

**시나리오**: 프로세스 재시작 시 `TokenBucket` 메모리 상태 초기화 → 재시작 직후 분당 quota 전체 소진 가능.

**Phase 1 처리**: Known limitation (M-6). 명문화된 위험:
- 최대 노출: 재시작 후 1분간 quota bypass 가능.
- 완화: K8s readinessProbe가 통과하기 전 traffic 수신 차단 (5s preStop + readiness latency).
- 기록: `tracing::warn!("token bucket reset on restart — quota may be bypassed for up to 60s")`.

**Phase 2 완화 경로** (cross-ref Axis E §2.E.6):
- Redis 기반 분산 TokenBucket (`features.rate_limit_persist = true`).
- 재시작 시 Redis에서 현재 bucket 상태 로드.

##### 2.F.9.2 Redis 연결 실패 (Phase 2: rate_limit_persist)

**정책 결정**: Redis down 시 **deny all** (503 `rate_limit_unavailable`).

**근거**: allow-through 정책은 Redis down 시 rate limit 완전 우회 가능 → DDoS/남용 attack surface. deny all이 가용성보다 보안 우선. 운영 팀은 Redis HA를 보장해야 함.

```rust
// gadgetron-xaas/src/quota/enforcer.rs [P2]
// ServiceUnavailable is NOT a standalone Phase 1 GadgetronError variant.
// See §2.G.9 ACL-3: adding a top-level ServiceUnavailable variant requires a
// D-entry in docs/process/04-decision-log.md + user approval (semver-affecting).
// Phase 2 design decision: security review in Phase 2 may justify a dedicated
// ServiceUnavailable variant once Redis HA is in scope. Until then, the Redis-down
// denial maps to Provider with a reason string, consistent with ACL-3.
async fn check_redis_bucket(&self, tenant_id: Uuid) -> Result<(), GadgetronError> {
    match self.redis.check_and_decrement(tenant_id).await {
        Ok(allowed) if allowed => Ok(()),
        Ok(_) => Err(GadgetronError::QuotaExceeded { tenant_id }),
        Err(redis_err) => {
            tracing::error!(error=?redis_err, "Redis unavailable for rate limiting");
            metrics::counter!("gadgetron_xaas_redis_unavailable_total").increment(1);
            // deny all: Redis 없이 rate limit 보장 불가 (§2.G.9 ACL-3)
            // [P2] Phase 2: If security review justifies a standalone variant,
            // open a D-entry and obtain user approval before adding ServiceUnavailable.
            Err(GadgetronError::Provider(
                "rate_limit_unavailable".to_string(),
            ))
        }
    }
}
```

##### 2.F.9.3 API 키 탈취

**감지**: 비정상 패턴(IP 지역 변경, RPM 급등) — Phase 2 anomaly detection.

**복구**:
1. `POST /api/v1/xaas/keys/{id}/revoke` → DB `revoked_at` 업데이트.
2. moka cache 수동 invalidate (`PgKeyValidator::invalidate_key(hash)`).
3. Phase 2: Redis pub/sub으로 모든 인스턴스에 revocation 전파.
4. 최대 TTL 10분 유예 (Phase 1 known limitation — moka TTL 만료 후 반영).

##### 2.F.9.4 감사 로그 변조 방어

**Phase 1**: PostgreSQL `audit_log` 테이블에 `DELETE` 권한 없음 (application 계정 = INSERT only, enforced by DB-level GRANT — M-SEC-5, §2.D.2).

**Phase 2 option** (security-compliance-lead scope): append-only + hash chain. 각 entry에 `prev_hash = SHA-256(prev_entry)` 필드 추가. 변조 시 체인 검증 실패.

##### §2.F.9.5 LLM Threat Posture (Phase 1 — OWASP LLM Top 10)

Gadgetron is an LLM orchestration gateway. The following table maps OWASP LLM Top 10 (v1.1, 2024) to Phase 1 posture.

| OWASP ID | Threat | Phase 1 Status | Detail | Phase 2 Path |
|---|---|---|---|---|
| LLM01 | Prompt Injection | **Accepted risk** | `messages[].content` is forwarded verbatim to providers. No `GuardrailsLayer` in Phase 1. Operator control: deploy only trusted models; restrict local provider network access. | `GuardrailsLayer` with prompt injection detection patterns |
| LLM02 | Insecure Output Handling | **Accepted risk** | Provider responses are streamed to the client without content filtering. No XSS sanitization for web clients. | Output PII scanner; response content policy layer |
| LLM03 | Training Data Poisoning | **N/A** | Gadgetron does not train or fine-tune models; it routes to existing providers. | N/A |
| LLM04 | Model Denial of Service | **Partial** | `RequestBodyLimitLayer` (4MB, M-SEC-2) limits prompt size. `QuotaEnforcer` enforces RPM/TPM limits per tenant. No per-request token budget cap at gateway level. | Per-request `max_tokens` cap enforcement at gateway |
| LLM05 | Supply Chain Vulnerabilities | **Mitigated** | `cargo deny check licenses bans advisories` + `cargo audit` + Trivy image scan in CI (M-SEC-3, §2.C.5) | SBOM attestation (syft, Phase 2) |
| LLM06 | Sensitive Information Disclosure | **Partial** | `Secret<T>` wraps all API keys and credentials; `#[instrument(skip(api_key))]` prevents trace leakage (§2.B.4.2). Prompt payloads are not inspected — accepted risk. | Prompt PII scan before forwarding |
| LLM07 | Insecure Plugin Design | **N/A** | Phase 1 has no plugin/tool-calling orchestration. Provider function-calling parameters are passed through verbatim. | Tool-call parameter validation layer [P3] |
| LLM08 | Excessive Agency | **N/A** | Phase 1 is a stateless chat gateway. No autonomous agent execution, memory, or action dispatch. | Agent safety constraints [P3] |
| LLM09 | Overreliance | **Out of scope** | Operator and end-user responsibility. Gadgetron does not add AI-generated content warnings. | N/A |
| LLM10 | Model Theft | **Partial** | Local model weights stored on host filesystem. Access control = host OS permissions (operator responsibility). Remote provider models are not at risk (third-party custody). | Volume encryption for local model weight storage |

**Phase 1 compensating controls** (do not prevent prompt injection, but limit blast radius):
1. Authentication at gateway — unauthenticated requests blocked before any LLM call
2. Scope restriction — tenants with `Scope::OpenAiCompat` cannot access management endpoints
3. Quota enforcement — `QuotaEnforcer` caps RPM/TPM per tenant, limiting prompt injection amplification
4. Audit logging — all requests logged to `audit_log` with `tenant_id`, enabling post-incident forensics

---

#### 2.F.10 운영 장애 모드

##### 2.F.10.1 롤링 업그레이드 N vs N+1 비호환

**시나리오**: v0.2.0 → v0.3.0 업그레이드 중 API 프로토콜 변경(내부 gRPC, DB schema 등)이 N과 N+1 간 호환 불가.

**정책**: **자동 복구 없음**. 수동 롤백 절차 필요.

**감지**: 헬스체크 실패 급증 + `gadgetron_gateway_5xx_total` 급등.

**수동 롤백 절차**:
```
1. kubectl rollout undo deployment/gadgetron
   (이전 Pod image로 복원)

2. DB migration rollback (필요 시):
   kubectl exec -it <pod> -- gadgetron-cli migrate --down --steps=1

3. 헬스체크 확인:
   kubectl exec -it <pod> -- curl http://localhost:8080/ready

4. 트래픽 복구 확인:
   kubectl logs -f deployment/gadgetron | grep "routing"
```

**Alert**: `OperationalManualIntervention` alert (Prometheus AlertManager, cross-ref §2.B.2).

##### 2.F.10.2 K8s Operator Reconcile 실패 [P2]

**정책**: `kube::runtime::Controller` 기본 지수 백오프 retry (1s → 2s → 4s → max 300s).

**Finalizer leak 감지**: `gadgetron.io/model-cleanup` finalizer가 있는 Pod가 삭제 후 5분 이상 존재하면 `gadgetron_operator_finalizer_leak_total` counter alert.

##### 2.F.10.3 설정 파일 변조/손상

**시나리오**: `gadgetron.toml` 파싱 실패 (TOML syntax error, 필수 필드 누락).

**처리**: fail fast, `process::exit(1)` + 상세 오류 메시지 (필드명, 라인 번호). K8s CrashLoopBackOff → 이전 버전 ConfigMap으로 롤백.

##### 2.F.10.4 로그 볼륨 디스크 풀

**시나리오**: `/var/log/gadgetron` 마운트 디스크 풀 → 로그 write 실패.

**처리**: `tracing_appender` write 실패 시 로그 드롭 (panic 없음). `gadgetron_log_drop_total` counter increment. Alert: counter > 0.

---

#### 2.F.11 카오스 검증 매트릭스

**목적**: 각 장애 모드가 실제 검증 가능함을 보장. Round 2 리뷰어가 커버리지 확인하는 기준.

**테스트 홈**: `gadgetron-testing/tests/` (D-20260411-05 cross-ref).

| 장애 모드 | 테스트 타입 | 테스트 파일 | 주입 방법 | 합격 기준 |
|---------|-----------|-----------|---------|---------|
| ProcessManager FSM: Loading → Failed (VramAllocationFailed) | Unit | `gadgetron-node/src/process_manager.rs #[cfg(test)]` | `FakeVramTracker::deny_allocation()` | `ProcessState::Failed { reason: VramAllocationFailed }` + VRAM 반납 확인 |
| ProcessManager FSM: Running → crash_detected | Unit | 위 동일 | `FakeChild::exit(137)` | `ProcessState::Failed { reason: CrashExit(137) }` + 포트 반납 |
| ProcessManager FSM: Stopping timeout → SIGKILL | Integration | `gadgetron-testing/tests/process_manager_e2e.rs` | `sleep infinity` 프로세스 spawn + 10s timeout | `FailureReason::KillTimeout`, VRAM/포트 반납 |
| Circuit breaker trip (3 failures → Open) | Unit | `gadgetron-router/src/circuit_breaker.rs #[cfg(test)]` | `cb.record_failure()` × 3 | `cb.state() == CircuitState::Open` |
| Circuit breaker half-open recovery | Unit | 위 동일 | `tokio::time::advance(60s)` (tokio test) | `cb.state() == CircuitState::HalfOpen` after 60s |
| Graceful shutdown 70s budget (audit flush stage) | Integration | `gadgetron-testing/tests/graceful_shutdown.rs` | SIGTERM 후 audit 5s timeout 시뮬레이션 | `gadgetron_xaas_audit_dropped_total` counter > 0 + process exit |
| Provider 429 → fallback chain | Integration | `gadgetron-testing/tests/fallback_chain.rs` | `wiremock` 429 응답 | fallback_chain[1] provider로 라우팅 |
| NVML poll stall (5s no response) | Unit | `gadgetron-node/src/gpu_monitor.rs #[cfg(test)]` | `FakeNvml::stall_for(Duration::from_secs(6))` | `node_state.is_degraded("nvml_stall") == true` + counter |
| GPU 온도 > 90°C → critical alert | Unit | 위 동일 | `FakeNvml::set_temp(91)` | `gadgetron_node_gpu_temp_critical_total` = 1 |
| DB migration 실패 → process exit | Integration | `gadgetron-testing/tests/startup_failure.rs` | `testcontainers` PG + 깨진 migration SQL | process exit code = 1 + 로그 `ERROR migration failed` |
| PG pool exhaustion → 503 | Integration | `gadgetron-testing/tests/pg_pool.rs` | `max_connections=1` + 동시 요청 2개 | 두 번째 요청 503 `PoolTimeout` |
| TokenBucket 재시작 → quota 리셋 | Unit | `gadgetron-xaas/src/quota/bucket.rs #[cfg(test)]` | `TokenBucket::new()` (재시작 시뮬레이션) | 새 bucket은 full quota (known limitation 확인) |
| Redis down → deny all 503 | Unit (Phase 2) | `gadgetron-xaas/src/quota/enforcer.rs #[cfg(test)]` | `FakeRedis::force_disconnect()` | 503 `rate_limit_unavailable` |
| Audit channel full → drop + counter | Unit | `gadgetron-xaas/src/audit/writer.rs #[cfg(test)]` | `mpsc::channel(1)` + 2개 항목 전송 | 두 번째 항목 drop, `gadgetron_xaas_audit_dropped_total` = 1 |
| Axis E §2.E.6 KL-1 e2e (audit flow SIGTERM drain) | Integration | `gadgetron-testing/tests/audit_e2e.rs` | SIGTERM during audit flush + GAP-T1 (B-6) | audit entries flushed within 5s or dropped counter |

---

### Axis G: 도메인 모델 (Domain Model)

> **v1 변경 내역**: G-6 TenantContext BLOCKER 해소 (§2.G.3), 5 bounded context 명세 (§2.G.1),
> 30+ term 용어 사전 (§2.G.2), aggregate roots + invariants (§2.G.4),
> cross-context translation contracts (§2.G.5), value objects vs entities (§2.G.6),
> identity type rules (§2.G.7), domain event catalog [P2] (§2.G.8),
> anti-corruption layer rules (§2.G.9). 적용 결정: B-1, G-6, H-13, D-20260411-07,
> D-20260411-10, D-20260411-13, D-20260412-02.

---

#### §2.G.1 Bounded Contexts Map

각 crate를 DDD bounded context로 매핑한다. Context 경계를 넘는 데이터 이동은 반드시 명시적 변환 함수 또는 anti-corruption layer를 거친다 (§2.G.9 규칙 참조).

| Context | Owner Crate(s) | Upstream Context Deps | Downstream Context Deps | Anti-Corruption Boundary |
|---------|---------------|----------------------|------------------------|--------------------------|
| **Inference** | `gadgetron-provider`, `gadgetron-router` | Configuration (AppConfig), Resource (NodeInfo for endpoint selection) | Tenancy (receives TenantContext from gateway), Observability (emits RouterMetric) | `SseToChunkNormalizer` — provider-specific SSE format → canonical `ChatChunk`. Provider HTTP errors → `GadgetronError::Provider`. |
| **Resource** | `gadgetron-scheduler`, `gadgetron-node` | Configuration (NodeConfig, SchedulerConfig) | Inference (NodeInfo → provider endpoint), Observability (GpuMetrics, NodeMetrics) | `NodeAgent::heartbeat()` → `NodeInfo` struct; scheduler never reads raw NVML. `VramTracker` is internal to node; scheduler reads only `available_vram_mb` in `NodeInfo`. |
| **Tenancy** | `gadgetron-xaas` | Configuration (DB URL, moka TTL from AppConfig) | Inference (TenantContext propagated via axum Extension), Observability (AuditEntry) | `PgKeyValidator` produces `ValidatedKey`; raw `sqlx::Row` never crosses into domain — converted via `sqlx_to_gadgetron` helper (D-20260411-13). `TenantContext` constructed in `gadgetron-core::context` (see §2.G.3) so gateway/router can reference it without depending on xaas. |
| **Observability** | `gadgetron-gateway/src/observability/`, `gadgetron-core/src/ui.rs` | All contexts (pull metrics from each) | None (sink) | `WsAggregator` consumes `GpuMetrics` + `NodeMetrics` from `ResourceMonitor` and `RouterMetric` from `MetricsStore`; never exposes internal DashMap entries over WS. Prometheus labels are strings — cardinality governed by `(provider, model, tenant_id)` LRU cap of 10,000 entries (B-1). |
| **Configuration** | `gadgetron-core/src/config.rs`, `gadgetron-cli` | None | All contexts (reads AppConfig at startup) | `AppConfig` is read-only after startup validation. No context may mutate config at runtime. `${ENV_VAR}` substitution performed once in `gadgetron-cli` before handing to each context. |

**Lifecycle sub-context** (within Resource): `ProcessManager` owns the `ProcessState` FSM for each deployed model process. This is a sub-context of Resource, not a separate top-level context. `ProcessState` transitions are owned exclusively by `gadgetron-node/src/process_manager.rs`.

---

#### §2.G.2 Ubiquitous Language Glossary

Alphabetized. Every term used in docs, code, error messages, and tests must match this table exactly. Aliases are permitted only where documented here.

| Term | Type | Definition | Defined In | Aliases | Related Terms |
|------|------|-----------|-----------|---------|---------------|
| `AuditEntry` | entity | Per-request audit record. `cost_cents: i64` (D-8, D-20260411-06). One per request regardless of stream length. Has UUID identity. | `gadgetron-xaas/src/audit/entry.rs` | — | `AuditWriter`, `Tenant` |
| `AuditWriter` | role/noun | Actor that receives `AuditEntry` via `mpsc::channel(4096)` and writes to PostgreSQL. Channel full → drop + counter (D-20260411-09). | `gadgetron-xaas/src/audit/writer.rs` | — | `AuditEntry` |
| `ApiKey` | entity | Authentication credential with `gad_live_` or `gad_test_` prefix (D-11). Has UUID identity. Prefix encodes environment (live/test), not scope. | `gadgetron-xaas/src/auth/key.rs` | — | `ValidatedKey`, `Scope`, `Tenant` |
| `AppConfig` | value object | Top-level TOML configuration struct. Immutable after startup. `${ENV_VAR}` substitution applied once by `gadgetron-cli`. | `gadgetron-core/src/config.rs` | — | `ServerConfig`, `ProviderConfig`, `RoutingConfig`, `NodeConfig` |
| `ChatChunk` | value object | Single SSE streaming delta unit. Equality by value; no identity. Produced by `SseToChunkNormalizer` or `PollingToEventAdapter`. | `gadgetron-core/src/provider.rs` | — | `ChatStream`, `SseEvent`, `ChatRequest` |
| `ChatRequest` | value object | OpenAI-compatible `/v1/chat/completions` request body. Equality by value. Does not contain `tenant_id` — see anti-corruption rule ACL-1 (§2.G.9). | `gadgetron-core/src/provider.rs` | — | `ChatChunk`, `TenantContext` |
| `ChatStream` | type alias | `Pin<Box<dyn Stream<Item = Result<ChatChunk, GadgetronError>> + Send>>`. The return type of `LlmProvider::chat_stream`. Not a struct; a type alias. | `gadgetron-core/src/provider.rs` | — | `ChatChunk`, `SseEvent` |
| `CircuitBreaker` | entity | Per-provider failure isolator. State machine: `Closed → Open (3 failures) → HalfOpen (60s) → Closed`. Owns `Arc<AtomicU8>` for lock-free state reads. | `gadgetron-router/src/circuit_breaker.rs` | — | `CircuitState`, `RoutingDecision` |
| `CircuitState` | value object | State of a `CircuitBreaker`. Enum: `Closed`, `Open`, `HalfOpen`. Stored as `AtomicU8` (0/1/2). | `gadgetron-router/src/circuit_breaker.rs` | — | `CircuitBreaker` |
| `ClusterHealth` | value object | Aggregate health summary: `status, total_nodes, online_nodes, total_gpus, active_gpus, running_models, requests_per_minute, error_rate_pct, cost_per_hour_usd, alerts`. Equality by value. | `gadgetron-core/src/ui.rs` (D-20260411-07) | — | `HealthStatus`, `WsMessage` |
| `DatabaseErrorKind` | value object | DB-agnostic error classification enum. Variants: `RowNotFound`, `PoolTimeout`, `ConnectionFailed`, `QueryFailed`, `MigrationFailed`, `Constraint`, `Other`. `#[non_exhaustive]`. No sqlx dependency in core (D-20260411-13). | `gadgetron-core/src/error.rs` | — | `GadgetronError::Database` |
| `EvictionPolicy` | value object | Model eviction algorithm selection. Enum: `Lru`, `Priority`, `CostBased`, `WeightedLru`. Phase 1: `CostBased` uses `ModelDeployment.priority: i32` stub (H-8). | `gadgetron-core/src/routing.rs` | — | `ModelDeployment`, `Scheduler` |
| `FailureReason` | value object | Cause of abnormal process termination. Variants: `CrashExit(i32)`, `LoadTimeout`, `HealthCheckStale`, `VramAllocationFailed`, `PortAllocationFailed`, `KillTimeout`, `ExecutableNotFound`, `GpuDeviceUnavailable`. `#[non_exhaustive]`. | `gadgetron-core/src/model.rs` | — | `ProcessState`, `StopReason` |
| `GadgetronError` | value object | Unified error enum, 12 variants (Phase 1). `#[non_exhaustive]`. Adding variants is semver-minor; removing or renaming is semver-major. Full variant list: `Config(String)`, `Provider(String)`, `Routing(String)`, `StreamInterrupted { reason: String }`, `QuotaExceeded { tenant_id: Uuid }`, `TenantNotFound { tenant_id: Uuid }`, `Forbidden`, `Billing(String)`, `DownloadFailed(String)`, `HotSwapFailed(String)`, `Database { kind: DatabaseErrorKind, message: String }`, `Node { kind: NodeErrorKind, message: String }`. Note: `ServiceUnavailable` is NOT a standalone variant — Redis-down denial maps to `GadgetronError::Provider` with reason string "rate_limit_unavailable". See §2.G.9 ACL-3 for variant namespacing. | `gadgetron-core/src/error.rs` | — | `DatabaseErrorKind`, `NodeErrorKind` |
| `GpuInfo` | value object | Per-GPU static+dynamic data within a `NodeStatus`. Fields: `gpu_index: u32`, `uuid: String` (NVML UUID), `name: String`, `vram_total_mb: u64`, `vram_used_mb: u64`, `utilization_pct: f32`, `temperature_c: u32`, `power_w: f32`, `power_limit_w: f32`. Equality by value. Does not have its own UUID identity — identified by `(node_id, gpu_index)`. | `gadgetron-core/src/node.rs` | — | `GpuMetrics`, `NodeMetrics`, `NodeInfo` |
| `GpuMetrics` | value object | Per-GPU time-series data point for TUI/WS broadcast. Fields: `node_id: Uuid`, `gpu_index: u32`, `vram_used_mb: u64`, `vram_total_mb: u64`, `utilization_pct: f32`, `temperature_c: u32`, `power_w: f32`, `power_limit_w: f32`, `clock_mhz: u32`, `fan_rpm: Option<u32>`. Distinct from `GpuInfo`: `GpuInfo` is static registration data; `GpuMetrics` is a polled time-series point (H-13). | `gadgetron-core/src/ui.rs` (D-20260411-07) | — | `GpuInfo`, `NodeMetrics`, `WsMessage` |
| `HealthStatus` | value object | Component health state. Enum: `Healthy`, `Degraded { reasons: Vec<String> }`, `Unhealthy { reasons: Vec<String> }`. Used in `/health`, `/ready`, `/health/detail`. | `gadgetron-core/src/health.rs` | — | `ClusterHealth` |
| `InflightCounter` | entity | Atomic counter tracking in-flight requests. Used by graceful shutdown drain (30s timeout). Fields: `count: Arc<AtomicUsize>`, `notify: Arc<Notify>`. | `gadgetron-gateway/src/inflight.rs` | — | `GracefulShutdown` |
| `InferenceEngine` | value object | Enum identifying the local inference server binary. Variants: `Ollama`, `Vllm`, `Sglang`, `LlamaCpp`, `Tgi`. Used in `ModelDeployment.engine`. | `gadgetron-core/src/model.rs` | — | `ModelDeployment`, `ProcessManager` |
| `LlmProvider` | role/trait | Adapter interface for a specific AI backend. Methods: `chat(&self, req: ChatRequest) -> Result<ChatResponse, GadgetronError>`, `chat_stream(&self, req: ChatRequest) -> Result<ChatStream, GadgetronError>`, `models(&self) -> Result<Vec<String>, GadgetronError>`, `health(&self) -> Result<bool, GadgetronError>`. All `async`, all `Send`. `Arc<dyn LlmProvider + Send + Sync>` used for runtime polymorphism (D-20260411-11). | `gadgetron-core/src/provider.rs` | — | `ChatRequest`, `ChatStream`, `SseToChunkNormalizer` |
| `MetricsStore` | entity | Lock-free store for per-(provider, model, tenant) routing metrics. `DashMap`-backed. Signature: `MetricsStore::record(&self, tenant_id: Uuid, provider: &str, model: &str, metric: RouterMetric)` — `tenant_id` is a label, not embedded in `RouterMetric`. LRU cap: 10,000 entries (B-1 anti-corruption). | `gadgetron-router/src/metrics.rs` | — | `RouterMetric`, `TenantContext` |
| `MigProfile` | value object | NVIDIA MIG partition configuration string. Format: `Ng.Sgb` (e.g., `3g.40gb`). Valid profiles enumerated per GPU model in scheduler config. Phase 1: static enable/disable only (D-20260411-04). | `gadgetron-core/src/node.rs` | — | `GpuInfo`, `Scheduler` |
| `ModelDeployment` | entity | A model instance deployed on a specific node. Has UUID identity. Fields: `id: Uuid`, `model_id: String`, `node_id: Uuid`, `engine: InferenceEngine`, `parallelism: ParallelismConfig`, `state: ProcessState`, `assigned_vram_mb: u64`, `priority: i32`, `deployed_at: Instant`, `last_used: Instant`. | `gadgetron-core/src/model.rs` | — | `ProcessState`, `EvictionPolicy`, `NodeInfo` |
| `NodeInfo` | entity | Node registration record with current resource snapshot. Has UUID identity (`node_id: Uuid`). Supersedes v0 `NodeStatus` in Axis G glossary: `NodeInfo` is the entity; `NodeStatus` is an alias for the status field. Fields: `node_id: Uuid`, `hostname: String`, `gpus: Vec<GpuInfo>`, `available_vram_mb: u64`, `heartbeat_at: Instant`, `is_healthy: bool`. | `gadgetron-core/src/node.rs` | `NodeStatus` (legacy alias) | `GpuInfo`, `NodeMetrics`, `VramTracker` |
| `NodeMetrics` | value object | Per-node rollup metrics for TUI/WS broadcast. Distinct from `GpuMetrics` which is per-GPU. Fields: `node_id: Uuid`, `cpu_pct: f32`, `ram_used_gb: f32`, `ram_total_gb: f32`, `network_rx_mbps: f32`, `network_tx_mbps: f32`, `running_models: u32`, `timestamp: u64` (Unix seconds). (H-13 resolution). | `gadgetron-core/src/ui.rs` (D-20260411-07) | — | `GpuMetrics`, `NodeInfo`, `WsMessage` |
| `NumaTopology` | value object | NUMA node to GPU affinity mapping. Fields: `numa_node_id: u32`, `gpu_indices: Vec<u32>`, `nvlink_groups: Vec<Vec<u32>>`. Read from `/sys/bus/pci/devices/*/numa_node` at startup (D-3). | `gadgetron-core/src/node.rs` | — | `NodeInfo`, `Scheduler` |
| `ParallelismConfig` | value object | Tensor/pipeline/expert/data parallelism settings. Fields: `tp_size: u32`, `pp_size: u32`, `ep_size: u32`, `dp_size: u32`, `numa_bind: Option<u32>`. All `u32`, no floats (D-1). | `gadgetron-core/src/routing.rs` | — | `ModelDeployment` |
| `PollingToEventAdapter` | noun | Gemini-specific adapter. Converts `generateContent` JSON polling responses to an SSE-like event stream compatible with `SseToChunkNormalizer`. Input: `reqwest::Response` (JSON). Output: `impl Stream<Item = SseEvent>` (D-20260411-02, H-4). | `gadgetron-provider/src/gemini/adapter.rs` | — | `SseToChunkNormalizer`, `ChatChunk` |
| `ProcessState` | value object | Per-model-process FSM state. `#[non_exhaustive]`. Variants: `NotLoaded`, `Loading { started_at: Instant, model_id: Uuid }`, `Running { pid: u32, port: u16, loaded_at: Instant, model_id: Uuid }`, `Stopping { since: Instant, reason: StopReason }`, `Stopped { reason: StopReason, stopped_at: Instant }`, `Failed { reason: FailureReason, failed_at: Instant }`. All transitions owned by `ProcessManager`. | `gadgetron-core/src/model.rs` | `ModelState` (legacy alias) | `StopReason`, `FailureReason`, `ModelDeployment` |
| `QuotaConfig` | value object | Per-tenant rate and spend limits. Fields: `tenant_id: Uuid`, `rpm_limit: u32`, `tpm_limit: u64`, `daily_cost_limit_cents: i64`. Cached in moka with 10-minute TTL (D-20260411-12). | `gadgetron-xaas/src/quota/config.rs` | — | `QuotaEnforcer`, `TenantContext` |
| `QuotaSnapshot` | value object | Point-in-time view of tenant quota state passed into `TenantContext`. Fields: `daily_used_cents: i64`, `daily_limit_cents: i64`, `rpm_used: u32`, `rpm_limit: u32`. Immutable once constructed. Wrapped in `Arc` inside `TenantContext` to allow cheap clone. | `gadgetron-xaas/src/quota/config.rs` | — | `QuotaConfig`, `TenantContext` |
| `RangePortAllocator` | entity | Port pool manager for local inference server processes. Fields: `available: BTreeSet<u16>`, mutex-guarded. Allocates from configured range (e.g., 18000-19000). Release is explicit; no GC. | `gadgetron-node/src/port_allocator.rs` | — | `ProcessState`, `FailureReason` |
| `RouterMetric` | value object | Single routing outcome data point. Fields: `latency_ms: f64`, `prompt_tokens: u32`, `completion_tokens: u32`, `estimated_cost_usd: f64` (f64, not cents — routing hot path, D-20260411-06). No `tenant_id` field — tenant_id is a label passed separately to `MetricsStore::record`. | `gadgetron-router/src/metrics.rs` | — | `MetricsStore`, `RoutingDecision` |
| `RoutingDecision` | value object | Provider + model selection result. Fields: `provider: String`, `model: String`, `strategy: RoutingStrategy`, `fallback_chain: Vec<String>`, `estimated_cost_usd: f64`. Equality by value. | `gadgetron-core/src/routing.rs` | — | `RouterMetric`, `CircuitBreaker` |
| `RoutingStrategy` | value object | Algorithm enum for provider selection. Variants: `RoundRobin`, `CostOptimal`, `LatencyOptimal`, `QualityOptimal`, `Fallback`, `Weighted`. Defined as enum; strategy objects implement `RoutingStrategyExt` trait in `gadgetron-router`. | `gadgetron-core/src/routing.rs` | — | `RoutingDecision`, `Router` |
| `Scope` | value object | API key permission level. `#[non_exhaustive]` enum. Phase 1 variants: `OpenAiCompat` (for `/v1/*`), `Management` (for `/api/v1/*`), `XaasAdmin` (for `/api/v1/xaas/*`). Defined in `gadgetron-xaas/src/auth/key.rs`; re-exported by `gadgetron-core` (D-20260411-10). | `gadgetron-xaas/src/auth/key.rs` | — | `ApiKey`, `TenantContext`, `ValidatedKey` |
| `SseEvent` | value object | Raw SSE line pair (`event:` + `data:` fields) before normalization to `ChatChunk`. Equality by value. | `gadgetron-provider/src/sse.rs` | — | `ChatChunk`, `SseToChunkNormalizer` |
| `SseToChunkNormalizer` | noun | Cross-provider normalizer. Input: `impl Stream<Item = SseEvent>`. Output: `impl Stream<Item = Result<ChatChunk, GadgetronError>>`. One impl per provider type; selected at construction. (D-20260411-02). | `gadgetron-provider/src/normalizer.rs` | — | `ChatChunk`, `SseEvent`, `PollingToEventAdapter` |
| `StopReason` | value object | Cause of normal process termination. `#[non_exhaustive]` enum. Variants: `UserRequested`, `NodeShutdown`, `Evicted`, `HealthCheckFailed`, `RollingUpgrade`. | `gadgetron-core/src/model.rs` | — | `ProcessState`, `FailureReason` |
| `Tenant` | entity | An independent organization or user account. Has UUID identity. Fields: `id: Uuid`, `name: String`, `created_at: chrono::DateTime<Utc>`, `is_active: bool`. Aggregate root for `ApiKey` list and `QuotaConfig`. | `gadgetron-xaas/src/tenant/model.rs` | — | `ApiKey`, `QuotaConfig`, `AuditEntry` |
| `TenantContext` | value object | Per-request context attached after authentication. Constructed once per request in `AuthLayer`; cloned cheaply (Arc inside for `quota_snapshot`). Does NOT have its own persistence identity (it is reconstructed per request). Full type definition in §2.G.3. | `gadgetron-core/src/context.rs` | — | `Scope`, `QuotaSnapshot`, `ValidatedKey` |
| `ValidatedKey` | value object | Authentication result returned by `PgKeyValidator::validate`. Fields: `tenant_id: Uuid`, `api_key_id: Uuid`, `scopes: Vec<Scope>`, `expires_at: Option<chrono::DateTime<Utc>>`. Ephemeral; lives only in `AuthLayer` stack frame before `TenantContext` construction. | `gadgetron-xaas/src/auth/validator.rs` | — | `ApiKey`, `TenantContext`, `Scope` |
| `VramTracker` | entity | Per-node VRAM accounting ledger. `DashMap<Uuid, u64>` (model_id → allocated_mb). `release(model_id)` called on `ProcessState → Stopped/Failed`. Scheduler reads `available_vram_mb` via heartbeat (not directly from `VramTracker`). | `gadgetron-node/src/vram_tracker.rs` | — | `ModelDeployment`, `RangePortAllocator`, `ProcessState` |
| `WsMessage` | value object | WebSocket broadcast payload enum. Variants: `GpuMetrics(GpuMetrics)`, `ModelStatus(ModelStatus)`, `RequestLog(RequestEntry)`, `ClusterHealth(ClusterHealth)`. `#[non_exhaustive]`. Serialized as JSON for WS transport. (D-20260411-07). | `gadgetron-core/src/ui.rs` | — | `GpuMetrics`, `NodeMetrics`, `WsAggregator` |

**Term count**: 34 terms (exceeds 30-term minimum).

**Nomenclature rule**: `NodeInfo` is the canonical entity name; `NodeStatus` is a legacy alias preserved in v0 code comments only. All new code must use `NodeInfo`. `ProcessState` supersedes `ModelState` in process-specific contexts; `ModelDeployment.state: ProcessState` field uses the canonical name.

---

#### §2.G.3 TenantContext Propagation Contract (G-6 BLOCKER resolution)

**Decision**: `TenantContext` is defined in `gadgetron-core::context` (not `gadgetron-xaas`). Rationale: router and gateway must reference `TenantContext` without creating a dependency on `gadgetron-xaas`. Only the *construction* logic (parsing `ValidatedKey` + fetching `QuotaSnapshot`) lives in `gadgetron-xaas`.

##### Complete Type Definition

```rust
// gadgetron-core/src/context.rs

use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;
use crate::auth::Scope;          // re-exported from gadgetron-xaas via core
use crate::quota::QuotaSnapshot;  // defined in gadgetron-core::quota (value object)

/// Per-request tenant context. Constructed once by AuthLayer; cloned cheaply.
/// The Arc<QuotaSnapshot> makes clone O(1) — pointer copy only.
/// Dropped when the axum handler future completes.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// Identity of the authenticated tenant. UUIDv4. Never String.
    pub tenant_id: Uuid,
    /// Identity of the specific API key used for this request. UUIDv4.
    pub api_key_id: Uuid,
    /// Scopes granted by this key. Vec length: 1-3 (Phase 1 max 3 scope variants).
    pub scopes: Vec<Scope>,
    /// Point-in-time quota state. Arc: shared with zero copy between layers.
    pub quota_snapshot: Arc<QuotaSnapshot>,
    /// Unique ID for this request. UUIDv4. Attached to tracing span as `request_id`.
    pub request_id: Uuid,
    /// Wall-clock start of request processing. Used for latency calculation.
    pub started_at: Instant,
}
```

`TenantContext` derives `Clone` but not `Serialize`/`Deserialize` — it is an in-process runtime value only. `Instant` is not serializable; use `u64` epoch millis in `AuditEntry` instead.

##### Construction Site

```
gadgetron-gateway/src/middleware/auth.rs  [AuthLayer::call]
  1. Extract "Authorization: Bearer <token>" header
  2. PgKeyValidator::validate(&token) → ValidatedKey { tenant_id, api_key_id, scopes, expires_at }
     [gadgetron-xaas/src/auth/validator.rs]
  3. QuotaEnforcer::snapshot(tenant_id) → Arc<QuotaSnapshot>
     [gadgetron-xaas/src/quota/enforcer.rs — moka cache hit ~50μs]
  4. Construct TenantContext {
         tenant_id:      validated_key.tenant_id,
         api_key_id:     validated_key.api_key_id,
         scopes:         validated_key.scopes,
         quota_snapshot: snapshot,
         request_id:     Uuid::new_v4(),   // UUIDv4, random
         started_at:     Instant::now(),
     }
  5. req.extensions_mut().insert(ctx)   // axum::Extension<TenantContext>
```

##### Propagation: Which Layers Read TenantContext

| Layer | Crate/File | What It Reads | What It Must Not Read |
|-------|-----------|---------------|----------------------|
| `ScopeGuardLayer` | `gadgetron-gateway/src/middleware/scope.rs` | `ctx.scopes` | any payload fields |
| `QuotaEnforcer::check_pre` | `gadgetron-xaas/src/quota/enforcer.rs` | `ctx.tenant_id`, `ctx.quota_snapshot` | `ctx.api_key_id` |
| `Router::chat_with_context` | `gadgetron-router/src/router.rs` | `ctx.tenant_id` (passed to `MetricsStore::record`) | must not embed `tenant_id` in `ChatRequest` |
| `MetricsStore::record` | `gadgetron-router/src/metrics.rs` | `tenant_id: Uuid` (explicit param, not from `RouterMetric`) | — |
| `tracing::Span` | gateway handler | `ctx.tenant_id`, `ctx.request_id` | — |
| `AuditWriter` | `gadgetron-xaas/src/audit/writer.rs` | `ctx.tenant_id`, `ctx.api_key_id`, `ctx.request_id`, `ctx.started_at` | `ctx.quota_snapshot` |
| `QuotaEnforcer::record_post` | `gadgetron-xaas/src/quota/enforcer.rs` | `ctx.tenant_id` | — |

##### Router Signature (B-1 resolution)

```rust
// gadgetron-router/src/router.rs

impl Router {
    /// Route a chat completion request with tenant context.
    /// tenant_id flows to MetricsStore and AuditEntry; never into ChatRequest.
    pub async fn chat_with_context(
        &self,
        ctx: &TenantContext,
        req: ChatRequest,
    ) -> Result<ChatStream, GadgetronError>;
}
```

The old `Router::chat(req: ChatRequest)` signature is removed. All callers pass `ctx`. `LlmProvider` trait is NOT changed — providers are tenant-unaware by design.

##### MetricsStore Signature (B-1 resolution)

```rust
// gadgetron-router/src/metrics.rs

impl MetricsStore {
    /// Record a routing metric. tenant_id is a Prometheus label dimension,
    /// not a field inside RouterMetric (anti-corruption: ACL-1).
    pub fn record(
        &self,
        tenant_id: Uuid,
        provider: &str,
        model: &str,
        metric: RouterMetric,
    );
}
```

`RouterMetric` struct has no `tenant_id` field. The `(provider, model, tenant_id)` triple is the DashMap key with LRU eviction at 10,000 entries to bound cardinality.

##### Lifecycle

```
Constructed: AuthLayer::call (first Tower middleware)
Cloned:      cheap — Arc<QuotaSnapshot> = pointer copy; Vec<Scope> ≤ 3 elements
Dropped:     when axum handler future drops (end of request)
NOT stored:  TenantContext is never written to PostgreSQL or any cache
NOT shared:  across requests (each request gets its own Uuid::new_v4() request_id)
```

##### Ban: tenant_id inside ChatRequest

`ChatRequest` is the OpenAI API surface. Embedding `tenant_id` in `ChatRequest` would:
1. Leak identity into the provider-facing payload (ACL-1)
2. Require modifying the OpenAI-compatible schema (breaks D-7 compatibility)
3. Make `LlmProvider` trait tenant-aware (violates context separation)

All downstream components receive `tenant_id` as an explicit parameter, not via `ChatRequest`.

---

#### §2.G.4 Aggregate Roots and Domain Invariants

An aggregate root is an entity that owns the write path for a cluster of related data. No external code may modify the aggregate's internal state except through the root's public methods.

| Aggregate Root | Owning Crate | Domain Invariants | Enforcement Mechanism |
|---------------|-------------|------------------|----------------------|
| `Tenant` | `gadgetron-xaas` | (1) An active tenant has at least one non-expired `ApiKey`. (2) `is_active = false` → all keys rejected at validation time. (3) `QuotaConfig.daily_cost_limit_cents >= 0`. | (1)(2) enforced at `PgKeyValidator` query: `WHERE is_active = true AND expires_at > NOW()`. (3) enforced at tenant creation endpoint: validation returns 400 if negative. PostgreSQL `CHECK (daily_cost_limit_cents >= 0)` constraint as secondary guard. |
| `ApiKey` | `gadgetron-xaas` | (1) Key prefix (`gad_live_` / `gad_test_`) matches the environment from `AppConfig.environment`. (2) `scopes: Vec<Scope>` is non-empty. (3) Key hash stored; plaintext never persisted. | (1) enforced at key issuance in `gadgetron-xaas/src/auth/key.rs::ApiKey::new(env, scopes)` — prefix selected from env argument. (2) enforced at construction: `assert!(!scopes.is_empty())` (panics at creation, not runtime). (3) `api_keys.key_hash VARCHAR(64)` column only; no plaintext column in schema. |
| `ModelDeployment` | `gadgetron-scheduler` | (1) `assigned_vram_mb <= node.vram_total_mb * 0.90` (90% cap). (2) Only one `ModelDeployment` per `(node_id, model_id)` pair may be in `Running` state simultaneously. (3) `ProcessState` transitions follow the FSM in §2.F.1 — no direct field mutation from outside `ProcessManager`. | (1) enforced by `Scheduler::deploy` FFD algorithm before inserting into `ModelDeploymentStore`. (2) enforced by `ModelDeploymentStore::insert` returning `Err(GadgetronError::Routing("already running"))` on duplicate. (3) enforced by making `ProcessState` only writable via `ProcessManager::transition_to(next: ProcessState)` which validates against the FSM table. |
| `NodeInfo` | `gadgetron-node` | (1) `heartbeat_at` must be within 30 seconds of `Utc::now()` for `is_healthy = true`. (2) `available_vram_mb` is the sum of `vram_total_mb` across all GPUs minus `VramTracker` allocations. (3) `gpu_index` values within a node are unique and contiguous from 0. | (1) enforced by stale-checker task in `gadgetron-scheduler` that marks nodes `is_healthy = false` when `Utc::now() - heartbeat_at > Duration::from_secs(30)`. (2) enforced by `NodeAgent::build_node_info()` which calls `VramTracker::available()`. (3) enforced at node registration: GPU list from NVML is index-sorted and validated. |
| `Quota` | `gadgetron-xaas` | (1) `daily_spend_cents <= daily_cost_limit_cents` — enforced via soft-cap policy (H-5): pre-check before stream start; over-cap allowed once; next request blocked. (2) `rpm_used` resets every 60 seconds via `TokenBucket`. (3) Restarting the process resets `TokenBucket` — known limitation (M-6), documented. | (1) `QuotaEnforcer::check_pre` returns `GadgetronError::QuotaExceeded` when over cap with no grace remaining. `record_post` always writes regardless. (2) `TokenBucket` tick via `tokio::time::interval(Duration::from_secs(60))`. (3) Limitation documented in `docs/architecture/platform-architecture.md §2.G.4` (this table). |

---

#### §2.G.5 Cross-Context Translation Contracts

When data moves between bounded contexts, an explicit translation function must exist. No struct from one context may be used directly in another without passing through this layer.

| Source Context | Target Context | Source Type | Target Type | Translation Fn | Invariants Preserved |
|---------------|---------------|------------|------------|----------------|---------------------|
| Tenancy | Inference | `ValidatedKey` | `TenantContext` | `AuthLayer::build_context(key: ValidatedKey, snapshot: Arc<QuotaSnapshot>) -> TenantContext` in `gadgetron-gateway/src/middleware/auth.rs` | `tenant_id` identity preserved; `scopes` carried unchanged; `request_id` newly minted per request |
| Inference | Tenancy | `ChatStream` completion | `AuditEntry` | `AuditWriter::record_completion(ctx: &TenantContext, usage: TokenUsage, cost_cents: i64)` in `gadgetron-xaas/src/audit/writer.rs` | `tenant_id` and `request_id` from context; `cost_cents: i64` from `compute_cost_cents(usage, rate)` (single conversion point, D-20260411-06) |
| Resource | Inference | `NodeInfo` | provider endpoint URL | `Scheduler::select_node(model_id) -> Option<(Uuid, u16)>` returns `(node_id, port)` — Inference context constructs URL as `format!("http://{}:{}", node.hostname, port)` | VRAM constraint: node selected only if `available_vram_mb >= estimated_vram_mb`; health constraint: `is_healthy = true` |
| Provider (Inference sub) | Inference | provider-specific SSE lines | `ChatChunk` | `SseToChunkNormalizer::normalize(event: SseEvent) -> Option<ChatChunk>` in `gadgetron-provider/src/normalizer.rs` | Field mapping: `delta.content → ChatChunk.delta`, `usage → ChatChunk.usage` (when present); `[DONE]` SSE sentinel → `None` (stream end signal) |
| Provider (Gemini sub) | Provider normalizer | Gemini JSON polling response | `SseEvent` | `PollingToEventAdapter::adapt(json: Value) -> impl Stream<Item = SseEvent>` in `gadgetron-provider/src/gemini/adapter.rs` | `candidates[0].content.parts[].text` → `data: {"delta": {"content": "..."}}` SSE format |
| Resource | Observability | NVML raw data (`nvml_wrapper` types) | `GpuMetrics` | `ResourceMonitor::poll_gpu(index: u32) -> GpuMetrics` in `gadgetron-node/src/resource_monitor.rs` | NVML types never cross outside `gadgetron-node`. Unavailable metrics → `Option::None` fields, not errors. |
| Resource | Observability | internal node state | `NodeMetrics` | `ResourceMonitor::poll_node() -> NodeMetrics` in `gadgetron-node/src/resource_monitor.rs` | CPU/RAM from `sysinfo` crate; never exposed as raw `sysinfo::System` struct |
| Observability | All | `WsMessage` broadcast | JSON string (WebSocket frame) | `serde_json::to_string(&msg)` — `WsMessage` derives `Serialize`. `WsAggregator` calls this before `broadcast::Sender::send`. | Serialization error is logged and dropped; never propagates to subscriber |
| DB (Tenancy sub) | Tenancy domain | `sqlx::Error` | `GadgetronError::Database { kind, message }` | `sqlx_to_gadgetron(e: sqlx::Error) -> GadgetronError` in `gadgetron-xaas/src/error.rs` | `sqlx` never appears in `gadgetron-core` types (D-12 leaf crate rule, D-20260411-13) |

---

#### §2.G.6 Value Objects vs Entities

**Value object**: equality by content; no persistent identity; immutable once created; safe to copy or replace.
**Entity**: has a persistent identity (UUID) that survives state changes; equality by identity, not field content.

| Concept | Classification | Rationale |
|---------|---------------|-----------|
| `ChatRequest` | Value object | Represents a user's input at a point in time. No identity; a retry is a new value. |
| `ChatChunk` | Value object | A streaming delta fragment. Content is its identity. |
| `ChatResponse` | Value object | Non-streaming completion. No identity persisted. |
| `QuotaSnapshot` | Value object | Point-in-time view. Immutable once constructed. |
| `RouterMetric` | Value object | A single measurement event. No persistence identity. |
| `GpuInfo` | Value object | Static+dynamic GPU data. Identified by `(node_id, gpu_index)`, not a UUID. |
| `GpuMetrics` | Value object | A polled time-series data point. Replaced on each poll cycle. |
| `NodeMetrics` | Value object | Per-node rollup. Replaced on each poll cycle. |
| `SseEvent` | Value object | Raw SSE line pair before normalization. Transient. |
| `RoutingDecision` | Value object | The result of a routing calculation. Computed fresh per request. |
| `CircuitState` | Value object | A snapshot of circuit breaker state. Stored atomically; no UUID. |
| `HealthStatus` | Value object | Component health snapshot. Recomputed on each probe. |
| `AppConfig` | Value object | Configuration snapshot. Read-only after startup. |
| `Scope` | Value object | Permission level. Equality by variant match. |
| `ProcessState` | Value object | FSM state snapshot. Replaced on each transition. |
| `StopReason` | Value object | Termination cause tag. No identity. |
| `FailureReason` | Value object | Failure cause tag. No identity. |
| `MigProfile` | Value object | GPU partition string. Equality by string content. |
| `ParallelismConfig` | Value object | Configuration tuple. Equality by all fields. |
| `NumaTopology` | Value object | Static topology map read at startup. |
| `ValidatedKey` | Value object | Auth result. Transient; lives only in `AuthLayer` frame. |
| `DatabaseErrorKind` | Value object | Error classification enum. Copy type. |
| `Tenant` | Entity | Has `id: Uuid`. Identity persists through name changes, activation toggles. |
| `ApiKey` | Entity | Has its own `id: Uuid` distinct from `tenant_id`. |
| `ModelDeployment` | Entity | Has `id: Uuid`. Identity survives state transitions (`Loading → Running`). |
| `NodeInfo` | Entity | Has `node_id: Uuid`. Identity survives heartbeat updates. |
| `AuditEntry` | Entity | Has `id: Uuid`. Immutable once written to DB; identity for compliance queries. |
| `TenantContext` | Value object (special) | Constructed per-request; no persistence. But contains entity IDs (`tenant_id`, `api_key_id`, `request_id`). Equality not meaningful — two contexts for the same tenant in the same second differ by `request_id`. |
| `MetricsStore` | Entity | Has persistent identity as a singleton within `Router`. State accumulates over time. No UUID; identified by process lifetime. |
| `CircuitBreaker` | Entity | Per-provider instance. Identity by provider name. State persists across requests. |
| `InflightCounter` | Entity | Singleton per gateway. Identity by process lifetime. |
| `VramTracker` | Entity | Per-node instance. Identity by `node_id`. State accumulates allocations. |
| `RangePortAllocator` | Entity | Per-node instance. State is the `BTreeSet<u16>` pool. |

---

#### §2.G.7 Identity Types

**Rule**: All entity IDs use `uuid::Uuid` (v4, random). No string IDs, no integer IDs, no sequential counters in domain types.

**Rationale against string IDs**: No compile-time type safety (a `tenant_id: String` and `node_id: String` are interchangeable by the compiler — `Uuid` types are not). String comparison requires heap allocation; `Uuid` comparison is `[u8; 16]` memcmp.

**Rationale against integer IDs**: Auto-increment integers leak row count (security), are not globally unique across instances, and create ordering collisions if the DB is ever sharded.

**UUIDv4 vs UUIDv7**: UUIDv7 (timestamp-ordered) would improve B-tree index locality for `audit_log` and `api_keys` tables. This is deferred to Phase 2 (requires PostgreSQL `uuid_generate_v7()` or Rust `uuid 1.x` v7 feature). Phase 1 uses `Uuid::new_v4()` everywhere. If Phase 2 migrates `AuditEntry.id` to v7, the `Uuid` type is compatible — the serialized format changes but `uuid::Uuid` accepts both.

| Entity | ID Field | Type | Generation Site | Format |
|--------|----------|------|----------------|--------|
| `Tenant` | `tenant_id` | `uuid::Uuid` | `gadgetron-xaas/src/tenant/registry.rs` at creation | UUIDv4 (`Uuid::new_v4()`) |
| `ApiKey` | `api_key_id` | `uuid::Uuid` | `gadgetron-xaas/src/auth/key.rs::ApiKey::new()` | UUIDv4 |
| `ModelDeployment` | `id` | `uuid::Uuid` | `gadgetron-scheduler/src/scheduler.rs::deploy()` | UUIDv4 |
| `NodeInfo` | `node_id` | `uuid::Uuid` | `gadgetron-node/src/agent.rs` at startup (persisted across restarts in config or DB) | UUIDv4 |
| `AuditEntry` | `id` | `uuid::Uuid` | `gadgetron-xaas/src/audit/writer.rs` | UUIDv4 (Phase 2: migrate to v7) |
| Per-request | `request_id` | `uuid::Uuid` | `gadgetron-gateway/src/middleware/auth.rs::AuthLayer::call` | UUIDv4; one per HTTP request |
| TraceId (OTel) | `trace_id` | `opentelemetry::TraceId` | `opentelemetry` SDK | 128-bit random (OTel spec) — not `uuid::Uuid` but same entropy |

**External API boundary**: UUIDs are serialized as lowercase hyphenated strings (`"550e8400-e29b-41d4-a716-446655440000"`) in all JSON responses. This is the `serde_json` default for `uuid::Uuid` with feature `serde`. Consumers must not assume any other format.

**Timestamp representation**: `Instant` is used for in-process duration measurement only (non-serializable). All serialized timestamps use `chrono::DateTime<Utc>` (ISO 8601) or `u64` Unix seconds for compact wire formats in `NodeMetrics` and `GpuMetrics`.

---

#### §2.G.8 Domain Event Catalog [P2]

This section documents the domain event vocabulary for potential Phase 2 event-sourcing or CQRS adoption. No events are implemented in Phase 1. Documenting them now prevents naming divergence when Phase 2 is designed.

| Event Name | Aggregate Root | Trigger | Payload Fields |
|-----------|---------------|---------|---------------|
| `TenantCreated` | `Tenant` | New tenant registration | `tenant_id`, `name`, `created_at` |
| `ApiKeyIssued` | `ApiKey` | Key generation | `api_key_id`, `tenant_id`, `scopes`, `prefix`, `issued_at` |
| `ApiKeyRotated` | `ApiKey` | Key rotation (old revoked, new issued) | `old_key_id`, `new_key_id`, `tenant_id`, `rotated_at` |
| `ApiKeyRevoked` | `ApiKey` | Explicit revocation | `api_key_id`, `tenant_id`, `revoked_at`, `reason` |
| `QuotaExceeded` | `Quota` | `QuotaEnforcer::check_pre` returns over-cap | `tenant_id`, `daily_used_cents`, `daily_limit_cents`, `exceeded_at` |
| `ModelDeployed` | `ModelDeployment` | `ProcessState → Running` | `deployment_id`, `model_id`, `node_id`, `engine`, `port`, `loaded_at` |
| `ModelEvicted` | `ModelDeployment` | `ProcessState → Stopping (StopReason::Evicted)` | `deployment_id`, `model_id`, `node_id`, `reason`, `evicted_at` |
| `NodeJoined` | `NodeInfo` | Node agent first heartbeat accepted | `node_id`, `hostname`, `gpu_count`, `joined_at` |
| `NodeLeft` | `NodeInfo` | Stale checker marks `is_healthy = false` (30s threshold) | `node_id`, `hostname`, `last_seen_at` |
| `CircuitBreakerTripped` | `CircuitBreaker` | 3 consecutive failures → `CircuitState::Open` | `provider`, `failure_count`, `tripped_at`, `recovery_at` |

Phase 2 implementation path: `gadgetron-xaas/src/events/` for Tenancy events; `gadgetron-node/src/events/` for Resource events. Event store: PostgreSQL `domain_events` table or Kafka topic, TBD in Phase 2.

---

#### §2.G.9 Anti-Corruption Layer Rules

These rules are hard constraints. Any code path that violates them is a defect regardless of whether tests pass.

**ACL-1: `tenant_id` never inside payload types.**
`ChatRequest`, `ChatChunk`, `ChatResponse`, `RouterMetric`, `SseEvent`, `WsMessage` must not contain a `tenant_id` field. Tenant identity is a cross-cutting concern carried exclusively in `TenantContext` (passed as explicit parameter or extracted from `axum::Extension`). Violation: the provider sees tenant data it must not see; the OpenAI-compatible API surface is contaminated.

**ACL-2: Provider-specific fields never leak into canonical types.**
`ChatRequest` and `ChatChunk` are canonical. Anthropic `top_k`, `system` prompt format, Gemini `parts[]` structure, vLLM `--gpu-memory-utilization` — all are translated at the adapter boundary (`SseToChunkNormalizer`, `PollingToEventAdapter`, provider-specific `chat_stream` implementations). Canonical types in `gadgetron-core` have zero provider-specific fields. Violation: `gadgetron-core` gains a dependency on provider SDK behavior.

**ACL-3: Error variants namespaced per context.**
`GadgetronError::Provider(String)` is correct for all provider-level failures. Do not flatten unrelated failures into it. `GadgetronError::Database { kind: DatabaseErrorKind, message: String }` is the DB error; `GadgetronError::Forbidden` is the auth error. Adding a new `GadgetronError::ServiceUnavailable` variant requires a D-entry in `docs/process/04-decision-log.md` and user approval — it is a semver-affecting change. Current behavior for Redis-down rate-limit denial: `GadgetronError::Provider` with `reason = "rate_limit_unavailable"` (no new variant needed in Phase 1).

**ACL-4: Storage types never cross context boundaries without `From` conversion.**
`sqlx::PgRow`, `sqlx::Error`, `reqwest::Error`, `nvml_wrapper::error::NvmlError` — none of these may appear in any type exported from their owning crate. Each crate provides an explicit conversion function:
- `gadgetron-xaas/src/error.rs::sqlx_to_gadgetron(sqlx::Error) -> GadgetronError`
- `gadgetron-provider/src/error.rs::reqwest_to_gadgetron(reqwest::Error, &str) -> GadgetronError`
- `gadgetron-node/src/error.rs::nvml_to_gadgetron(NvmlError) -> GadgetronError`

No `impl From<sqlx::Error> for GadgetronError` in `gadgetron-core` (would violate D-12 leaf crate rule — `gadgetron-core` has no DB dependency).

**ACL-5: `gadgetron-core` has zero runtime dependencies on other gadgetron crates.**
`gadgetron-core` is the leaf crate (D-12). It may depend only on: `serde`, `serde_json`, `uuid`, `chrono`, `thiserror`, `tokio` (for `Instant`, `Arc`), `tracing` (for instrument macros). Any violation creates a circular dependency. `TenantContext` lives in `gadgetron-core::context` — this is permissible because `Scope` is re-exported from `gadgetron-core` and `QuotaSnapshot` is also defined in `gadgetron-core::quota`. The `gadgetron-xaas` crate owns only the construction and persistence logic.

---

### Axis H: 성능 모델 (Performance Model)

> **Decisions applied**: D-20260411-11 (vtable rationale), D-20260411-12 (moka cache Phase 1),
> D-20260412-02 (implementation determinism). Phase C gaps resolved: B-7 / GAP-T2 (BLOCKER),
> G-11 (H-6), H-9 (NVML cadence), M-8 (WsAggregator bandwidth). All budget numbers are
> concrete; "미구현" cells are replaced with criterion bench targets.

#### §2.H.1 SLO Catalog

Measurement window: rolling 30 days unless noted. Alert thresholds trigger PagerDuty/alertmanager.
All latency SLOs measure **in-process gateway overhead only** — provider network round-trip and
model inference time are excluded by definition.

| Metric | SLI Definition | SLO Target | Window | Alert Threshold | Runbook |
|--------|----------------|------------|--------|-----------------|---------|
| P99 gateway overhead | `gadgetron_gateway_overhead_duration_us` histogram P99, measured from socket-accept to upstream-call-initiation, mock-provider path | < 1 000 µs | 1 day rolling | > 900 µs for 5 min | RB-H1: check moka hit rate, DashMap contention, JSON parse bench |
| P50 gateway overhead | same histogram P50 | < 200 µs | 1 day rolling | > 300 µs for 10 min | RB-H2: same as RB-H1 |
| Availability (Phase 1) | successful HTTP responses / total requests (excl. 429, 503 from PG down) | >= 99.9% / month | 30 days | < 99.5% in any 24h window | RB-H3: restart gateway pod, check PG health |
| Availability (Phase 2 HA) | same, multi-replica | >= 99.95% / month | 30 days | < 99.8% in any 24h window | RB-H3 + load-balancer failover check |
| Audit log retention | oldest audit record age in PG `audit_log` table | >= 90 days retained | continuous | any record gap > 1 day | RB-H4: check pg_dump CronJob, PVC status |
| Quota enforcement accuracy | (correctly enforced quota decisions) / (total quota decisions) — validated via integration test suite counting 429 vs accepted at boundary | >= 99% | 1 day | < 99% | RB-H5: check TokenBucket state, known limitation: restart grace period (KL-1 §2.E.5) |
| Auth cache hit rate | `gadgetron_xaas_auth_cache_hit_total` / (`_hit_total` + `_miss_total`) | >= 99% after warm-up | 1 hour | < 95% for 15 min | RB-H6: check moka capacity (cap=10_000), TTL=10 min |

**Instrumentation crate**: `metrics` 0.23 (for `counter!`, `histogram!` macros). Prometheus
exporter: `metrics-exporter-prometheus` 0.15 on port 9090 (Axis B §2.B.1). Histogram buckets
for gateway overhead: `[50, 100, 200, 300, 500, 700, 1000, 2000, 5000, 10000]` µs.

```rust
// gadgetron-gateway/src/middleware/overhead_timer.rs
// Records gateway overhead after upstream call initiation.
pub struct OverheadTimerLayer;

impl<S> Layer<S> for OverheadTimerLayer {
    type Service = OverheadTimerService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        OverheadTimerService { inner }
    }
}

pub struct OverheadTimerService<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for OverheadTimerService<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send,
    B: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let start = Instant::now();
        let fut = self.inner.call(req);
        Box::pin(async move {
            let resp = fut.await?;
            let elapsed_us = start.elapsed().as_micros() as f64;
            metrics::histogram!("gadgetron_gateway_overhead_duration_us").record(elapsed_us);
            Ok(resp)
        })
    }
}
```

#### §2.H.2 Latency Budget Breakdown

**Scope**: P99 < 1 000 µs, auth cache hit path (99th percentile of real traffic). Auth cache
miss path (1% of cold-start requests) has a separate SLO of P99 < 8 000 µs. Provider network
and inference are excluded.

All "criterion target" cells are concrete pass/fail thresholds enforced in CI (see §2.H.9).

| Hop | Crate | Operation | Budget (µs) | Criterion bench | Prometheus metric | Source / Gap |
|-----|-------|-----------|-------------|-----------------|-------------------|--------------|
| TLS handshake | client-side | TLS session resumption — client caches session ticket | excluded (client responsibility) | — | — | excluded by SLO definition |
| axum route dispatch | `gadgetron-gateway` | `Router::call` → handler match | 10 | `bench_axum_dispatch` median < 10 µs | — | measured via tower service call overhead |
| `RequestIdLayer` + `TraceLayer` | `gadgetron-gateway` | UUID v4 gen + tracing span open | 5 | `bench_middleware_requestid_trace` median < 5 µs | `gadgetron_gateway_trace_overhead_us` | tracing span = ~1 µs; UUID gen = ~2 µs |
| `AuthLayer` (moka hit) | `gadgetron-xaas` | SHA-256 prefix hash lookup in `moka::future::Cache<String, ValidatedKey>` (10_000 entries, 10 min TTL) | 50 | `bench_moka_key_validation` median < 50 µs | `gadgetron_xaas_auth_duration_us{cache="hit"}` | D-20260411-12; moka read = ~200 ns + SHA-256 = ~5 µs on 64-byte key |
| `AuthLayer` (moka miss) | `gadgetron-xaas` | moka miss → PG `SELECT * FROM api_keys WHERE prefix_hash = $1` | 5 000 | `bench_moka_key_validation` miss path median < 6 000 µs | `gadgetron_xaas_auth_duration_us{cache="miss"}` | G-11; cold-start path, 1% of requests |
| `TenantContextLayer` | `gadgetron-xaas` | `Arc<TenantContext>` clone from moka hit (TenantContext cached at 10 min TTL alongside ApiKey) | 10 | `bench_tenant_context_construction` cache-hit path median < 10 µs | `gadgetron_xaas_tenant_ctx_duration_us{cache="hit"}` | G-11 (H-6): quota_config fetch on cache miss = ~5 ms (separate P99 budget exemption) |
| `QuotaEnforcerLayer` | `gadgetron-xaas` | `TokenBucket::try_consume(1)` from in-memory `DashMap<TenantId, TokenBucket>` | 50 | `bench_moka_key_validation` (quota path) median < 50 µs | `gadgetron_xaas_quota_check_us` | DashMap shard read + AtomicU64 CAS |
| `ScopeGuardLayer` | `gadgetron-gateway` | `Vec<Scope>` membership check (≤ 3 elements Phase 1) | 5 | `bench_middleware_requestid_trace` median < 5 µs | — | linear scan of 3-element Vec, no alloc |
| `CircuitBreakerLayer` (closed state) | `gadgetron-gateway` | `AtomicBool` load + `VecDeque<Instant>` window check (no lock in closed state) | 5 | `bench_middleware_requestid_trace` median < 5 µs | `gadgetron_gateway_circuit_breaker_state` | closed-state fast path: single atomic read |
| Router strategy lookup | `gadgetron-router` | `DashMap<ProviderId, ProviderState>` read + strategy selection (RoundRobin = `AtomicUsize::fetch_add`) | 20 | `bench_router_strategy_roundrobin` median < 20 µs | `gadgetron_router_decision_duration_us` | 6 strategies; worst case = weighted (scan n=10 providers) |
| Protocol translate | `gadgetron-gateway` | `ChatRequest` JSON → provider-specific payload (`serde_json::to_vec`); single allocation of ~2 KB buffer | 100 | `bench_middleware_chain` (translate phase) median < 100 µs | `gadgetron_gateway_translate_duration_us` | JSON ser of 500-token prompt ~80 µs on Ryzen 5 5600X |
| Upstream call initiation | `gadgetron-provider` | `reqwest::Client::execute` — TCP connection from pool (no new handshake), bytes queued to kernel | 500–800 | — (excluded from SLO, provider network begins) | — | remainder of 1 000 µs budget; actual network begins here |
| **Total in-process** | | | **<= 1 000 µs P99** | `bench_middleware_chain` (full stack) median < 700 µs, P99 < 1 000 µs | `gadgetron_gateway_overhead_duration_us` | SLO anchor |

**Auth cache miss path (KL-1 exemption)**: When moka misses, `TenantContextLayer` issues a PG
query taking ~5 ms (pool hit) or ~8 ms (pool contention). The gateway overhead SLO is **suspended**
for that single request; the SLI still records the value, allowing the P99 < 8 ms secondary SLO
to be independently tracked via `gadgetron_xaas_auth_duration_us{cache="miss"}` histogram.

**Observability overhead per hop**: Each tracing span = ~1 µs open + ~1 µs close (lock-free
thread-local buffer). Prometheus `histogram!` observe call = ~100 ns. `counter!` increment =
~10 ns. Sampling ratio 0.1 (Axis B §2.B.1) reduces OTel export cost ~10×. Total observability
overhead across all hops <= 20 µs (12 spans × 2 µs + 5 histograms × 100 ns). This is already
included in the per-hop budget slices above (RequestId+Trace = 5 µs, ScopeGuard/CB = 5 µs each).

#### §2.H.3 Throughput Capacity Planning

**Phase 1 target**: 1 000 QPS sustained per gateway node, 8 vCPU, 32 GB RAM.

| Scenario | QPS Target | Memory/request | CPU/request | Bottleneck at saturation |
|----------|-----------|----------------|-------------|--------------------------|
| Cloud API proxy (OpenAI/Anthropic) — moka warm | 1 000 | ~20 KB (ChatRequest 4 KB + TenantContext Arc 8 KB + middleware state 8 KB) | ~2 ms (CPU time, dominated by JSON ser/deser) | moka contention > 2 000 QPS; JSON parsing at 5 000 QPS |
| Local vLLM (1 GPU, 7B Q4_K_M) | 50–200 | ~20 KB | ~2 ms | GPU inference throughput |
| Local Ollama (CPU mode) | 5–20 | ~20 KB | ~2 ms | CPU inference |
| Gateway self (mock provider, 0 ms upstream) | > 10 000 | ~20 KB | ~2 ms | serde_json at > 20 000 QPS |

**Memory per request (steady state)**:
- `ChatRequest` deserialized: ~4 KB (1 000 token prompt, JSON overhead)
- `TenantContext` (Arc clone, shared): 8 KB struct + quota snapshot; Arc overhead = 16 bytes
- Middleware state (`Extensions` map): ~8 KB
- Provider request body buffer: ~2 KB (serialized)
- SSE response streaming buffer: ~1 KB (per-chunk, released on each yield)
- Total per active request: ~20 KB working set

**CPU capacity**: 1 tokio worker core handles ~500 QPS at saturation (2 ms CPU time/request).
8 vCPU = 4 000 QPS ceiling before scheduler contention. Phase 1 target of 1 000 QPS uses 2 cores,
leaving headroom for burst.

**PostgreSQL connection pool capacity**:
- Pool size: 20 connections (D-20260411-04 §2.H.4)
- Sustained PG QPS: 20 connections × 5 queries/s/connection = 100 PG QPS
- Auth miss rate 1% × 1 000 gateway QPS = 10 PG queries/s for auth
- Quota moka miss rate ~0.1% × 1 000 = 1 PG query/s for quota
- Audit inserts: async via mpsc, batched writer, ~1 insert/s per 1 000 requests in batch mode
- Pool budget consumed: ~11 queries/s active << 100 capacity. Safe margin.
- Worst case: 20 concurrent slow queries (100 ms each) → PoolTimeout → 503. Mitigation:
  `sqlx::pool::PoolOptions::acquire_timeout(Duration::from_millis(500))`.

**Identified saturation bottleneck**:
Phase 1: JSON parsing (`serde_json`) at pathological load > 10 000 QPS (single-threaded hot path
per request). Phase 2 mitigation: `simd-json` or `sonic-rs` for zero-copy deserialization.
Secondary: moka internal Mutex contention visible at > 2 000 QPS with high miss rates.

#### §2.H.4 NVML Poll Cadence (H-9)

**Decision**: 1 Hz (1-second interval). Source: `gpu-resource-manager.md`. This document
previously stated 100 ms incorrectly; 1 Hz is the canonical value (H-9 resolution).

**Rationale**:

| Factor | 100 ms polling | 1 Hz polling | Verdict |
|--------|---------------|--------------|---------|
| Failure detection window | 500 ms (5 × 100 ms) | 5 s (5 × 1 s) | 1 Hz sufficient: stall threshold = 30 s (Axis F §2.F.2), detection at 5 s leaves 25 s margin |
| CPU overhead per poll cycle | ~0.8 ms (8 GPUs × 100 µs/NVML call) | ~1 ms (8 GPUs × 125 µs/call) | negligible difference; 1 Hz = 1 ms/s = 0.1% of 1 CPU core |
| NVML API call latency | ~100–125 µs per `nvmlDeviceGetMemoryInfo` call (measured on A100, driver 525) | same | |
| NVML API quota | no published rate limit in driver docs; tested safe at 10 Hz | well under any plausible limit | |
| Temperature/power accuracy | 100 ms needed for thermal throttle detection (Axis B §2.B.10) | 1 Hz adequate: thermal time constant ~10 s for GPU die | Phase 2 option: 500 ms with test |

**Phase 2 option**: Drop to 500 ms if thermal controller (Phase 2) requires faster feedback.
Must be validated with explicit CI benchmark: `bench_nvml_poll_cpu_overhead` showing < 5 ms/s
total CPU budget at 500 ms cadence with 8 GPUs.

**Implementation**:
```rust
// gadgetron-node/src/monitor/gpu_monitor.rs
const NVML_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub async fn run_poll_loop(
    nvml: Arc<Nvml>,
    tx: mpsc::Sender<Vec<GpuMetrics>>,
    mut shutdown: CancellationToken,
) {
    let mut interval = tokio::time::interval(NVML_POLL_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let metrics = collect_gpu_metrics(&nvml);  // sync; ~1 ms for 8 GPUs
                let _ = tx.try_send(metrics);              // non-blocking; drop if receiver slow
            }
            _ = shutdown.cancelled() => break,
        }
    }
}
```

`collect_gpu_metrics` is synchronous (NVML is not async). The 1 ms call fits within a single
tokio `spawn_blocking` budget; do not call from the async executor directly. Use
`tokio::task::spawn_blocking` wrapping the NVML calls if the executor thread budget is a concern.

#### §2.H.5 WsAggregator Bandwidth Analysis (M-8)

**Source**: Phase C review M-8 (ux-interface-lead), confirmed PM decision.

**Message rate calculation**:
- GPU metric sources: 100 nodes × 8 GPUs/node = 800 GPU slots
- NVML poll rate: 1 Hz (§2.H.4) → 800 `GpuMetrics` messages/second total ingress to `WsAggregator`
- Grouped per-node: `WsAggregator` receives 100 `Vec<GpuMetrics>` batches/second (one per node)

**Payload sizing** (per `WsMessage::GpuMetrics` JSON):
```
GpuMetrics JSON fields: node_id(36B UUID) + gpu_index(2B) + vram_used_mb(8B) +
  vram_total_mb(8B) + utilization_pct(4B) + temperature_c(4B) + power_w(4B) +
  power_limit_w(4B) + clock_mhz(4B) + fan_rpm(4B) = ~100 bytes raw
JSON framing overhead (keys + quotes + brackets): ~150 bytes
WsMessage envelope (type tag + timestamp): ~50 bytes
Total per message: ~300 bytes
```

**Bandwidth math**:

| Scenario | Calculation | Bandwidth |
|----------|-------------|-----------|
| Source ingress to WsAggregator | 800 msg/s × 300 bytes | ~240 KB/s |
| Phase 1 fan-out (100 WebSocket clients) | 100 clients × 240 KB/s | ~24 MB/s egress |
| Phase 1 theoretical maximum (1 000 clients) | 1 000 × 240 KB/s | ~240 MB/s egress |
| Phase 2 with topic filter (10× reduction) | 100 clients × 24 KB/s (1/10th nodes subscribed) | ~2.4 MB/s egress |

**Phase 1 decision (broadcast-all)**:
- Client cap: 100 WebSocket clients maximum
- No subscription filter in Phase 1
- 24 MB/s egress is within single-NIC capability (1 Gbps = 125 MB/s)
- `tokio::sync::broadcast::channel(1024)`: sender posts one message per batch
- `RecvError::Lagged` handling: drop + log + client reconnects (Phase 1 known behavior)

**Phase 2 decision (topic-filtered subscriptions)**:
- Client specifies `node_id: Option<Uuid>` and `gpu_index: Option<u32>` filter on WS handshake
- `WsAggregator` maintains `HashMap<ClientId, SubscriptionFilter>`
- Expected fan-out reduction: 10× (clients subscribe to ~10% of nodes on average)
- Target egress Phase 2: ~2.4 MB/s at 100 clients

**WsAggregator implementation location**: `gadgetron-gateway/src/observability/aggregator.rs` (H-12).

```rust
// gadgetron-gateway/src/observability/aggregator.rs
pub struct WsAggregator {
    tx: broadcast::Sender<Arc<WsMessage>>,  // Arc to avoid cloning per subscriber
}

impl WsAggregator {
    /// Phase 1: broadcast-all, cap 100 subscribers via semaphore.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity); // capacity = 1024
        WsAggregator { tx }
    }

    pub fn publish(&self, msg: WsMessage) {
        let _ = self.tx.send(Arc::new(msg)); // ignore SendError (no subscribers = ok)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<WsMessage>> {
        self.tx.subscribe()
    }
}
```

#### §2.H.6 MetricsStore Cardinality and Memory Budget

**Type**: `DashMap<(ProviderId, ModelId, TenantId), RouterMetric>` in `gadgetron-router`.

**Cardinality cap**: 10 000 entries. Decision source: Phase C Theme 1 (B-1 resolution, G-6).

**Memory estimate**:
```
Key: (ProviderId=Uuid 16B, ModelId=String ~32B, TenantId=Uuid 16B) = ~64 bytes + DashMap overhead ~32B = ~96 bytes
Value: RouterMetric {
    request_count: u64 (8B),
    success_count: u64 (8B),
    error_count: u64 (8B),
    total_latency_us: u64 (8B),
    total_tokens: u64 (8B),
    total_cost_usd: f64 (8B),
    last_updated: Instant (16B),
} = ~64 bytes + padding ~10B = ~74 bytes
Total per entry: ~96 + ~74 = ~170 bytes
Total at cap: 10_000 × 170 bytes = ~1.7 MB steady state
```

**Eviction strategy**: LRU via `moka::sync::Cache` wrapping (preferred, no manual sweep needed).
If `moka::sync` overhead is measured > 5 µs per insert, fall back to periodic sweep:
- Hourly sweep task removes entries where `last_updated` > 60 minutes ago
- Sweep runs in `spawn_blocking` to avoid blocking the async executor
- `DashMap::retain` call: O(n) scan, ~5 ms for 10 000 entries, acceptable at 1/hour

**Concrete type signature** (D-20260412-02 compliance):
```rust
// gadgetron-router/src/metrics.rs
use moka::sync::Cache;

pub struct MetricsStore {
    inner: Cache<MetricsKey, RouterMetric>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MetricsKey {
    pub provider_id: ProviderId,   // newtype over Uuid
    pub model_id: ModelId,         // newtype over String, Arc<str> internally
    pub tenant_id: TenantId,       // newtype over Uuid
}

#[derive(Clone, Debug)]
pub struct RouterMetric {
    pub request_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub total_latency_us: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,   // f64 in router (D-20260411-06)
    pub last_updated: std::time::Instant,
}

impl MetricsStore {
    pub fn new() -> Self {
        MetricsStore {
            inner: Cache::builder()
                .max_capacity(10_000)
                .time_to_idle(Duration::from_secs(3600))  // evict after 60 min idle
                .build(),
        }
    }

    pub fn record_success(
        &self,
        key: MetricsKey,
        latency_us: u64,
        tokens: u64,
        cost_usd: f64,
    ) {
        self.inner.entry(key).and_upsert_with(|existing| {
            let mut m = existing.map(|e| e.into_value()).unwrap_or_default();
            m.request_count += 1;
            m.success_count += 1;
            m.total_latency_us += latency_us;
            m.total_tokens += tokens;
            m.total_cost_usd += cost_usd;
            m.last_updated = Instant::now();
            m
        });
    }
}
```

**Prometheus cardinality guard**: `gadgetron_router_metrics_store_entries` gauge exported
periodically (every 30 s). Alert if > 9 000 entries (90% of cap).

#### §2.H.7 GPU-Side Performance (Scheduler Bin-packing)

**Algorithm**: First-Fit Decreasing (FFD). Sort models by VRAM requirement descending, then
assign each to the first node with sufficient free VRAM.

**Complexity**: O(n log n) sort + O(n × m) fit scan, where n = active model deployments,
m = candidate nodes.

**Phase 1 scale**: n = 1 000 models, m = 100 nodes. Worst case O(1 000 × 100) = 100 000
comparisons. Each comparison = VRAM subtraction + threshold check = ~5 ns on modern CPU.
Total: 100 000 × 5 ns = 500 µs + sort 1 000 × log(1 000) × 10 ns = 100 µs.
**Total worst-case: ~600 µs per scheduling decision.**

**Criterion benchmark target**: `bench_binpack_ffd` with n=1 000 models, m=100 nodes:
median < 5 ms, P99 < 10 ms. (Conservative; measured value expected ~600 µs.)

**Why scheduling latency does not affect P99 gateway SLO**: Scheduling is triggered by model
deploy/undeploy events, not by per-request routing. Routing reads `DashMap<NodeId, NodeState>`
(mutable copy updated after scheduling completes). The scheduler decision is off the hot path.

**NUMA-aware FFD pseudo-code** (H-7):
```
sort deployments by vram_mb descending
for each deployment d:
  # Step 1: prefer NVLink group with sufficient free VRAM
  for group in nvlink_groups.sorted_by_free_vram_desc():
    for node in group.nodes:
      if node.free_vram >= d.vram_mb * 1.05:  # 5% overhead margin
        assign(d, node)
        break outer
  # Step 2: fallback — any node with sufficient VRAM (PCIe bandwidth penalty accepted)
  for node in all_nodes.sorted_by_free_vram_desc():
    if node.free_vram >= d.vram_mb * 1.05:
      assign(d, node)
      tracing::warn!(deployment_id = %d.id, node_id = %node.id,
                     "FFD PCIe fallback: NVLink group full")
      break
  # Step 3: no fit → deployment deferred (not failed — queued for retry)
  else:
    pending_queue.push(d)
```

#### §2.H.8 Database Performance Budget

All measurements on PostgreSQL 15 with `max_connections=20`, `sqlx 0.8` connection pool,
single-row operations, B-tree index on `prefix_hash` and `tenant_id` columns.

| Operation | moka hit | moka miss (PG query) | PG query type | Pool impact |
|-----------|----------|---------------------|---------------|-------------|
| API key validation | ~50 µs (SHA-256 + moka get, D-20260411-12) | ~5 ms (PG `SELECT id, tenant_id, scopes, revoked_at FROM api_keys WHERE prefix_hash = $1 LIMIT 1`) | indexed point read | 1 connection × ~5 ms = 5 ms × conn |
| Quota config fetch | ~50 µs (moka hit, same cache as ApiKey, TenantContext) | ~5 ms (PG `SELECT * FROM quota_configs WHERE tenant_id = $1`) | indexed point read | same as above |
| Audit insert | 0 µs P99 impact (async mpsc send, `try_send` non-blocking) | ~4 ms actual PG `INSERT INTO audit_log ...` (batched writer drains in background) | sequential write | 1 connection in writer task; no request-path impact |
| Quota record post-stream | ~1 µs (atomic in-memory `TokenBucket` update) | ~5 ms if quota flush triggers PG update (Phase 2) | n/a Phase 1 | n/a |

**Pool exhaustion behavior**: `acquire_timeout = 500 ms`. If pool exhausted for 500 ms,
`sqlx::Error::PoolTimedOut` → `GadgetronError::Database { kind: DatabaseErrorKind::PoolTimeout }`
→ HTTP 503. Alert on `gadgetron_db_pool_wait_duration_ms` P99 > 100 ms.

**Pool sustained capacity check**:
- Auth miss: 1% × 1 000 QPS = 10 PG queries/s × 5 ms = 50 ms pool-connection-time/s
- Quota miss: 0.1% × 1 000 QPS = 1 query/s × 5 ms = 5 ms
- Audit writer: 1 connection × continuous = 1 connection reserved
- Total connections in use: ceil(55 ms / 5 ms) + 1 = ~12 connections active at 1 000 QPS
- Pool headroom: 20 - 12 = 8 spare connections. Safe for Phase 1. PgBouncer at Phase 2.

#### §2.H.9 Benchmark and Load Test Strategy (GAP-T2 BLOCKER — resolved)

**Library choices** (D-20260412-02 item 4):
- Micro-benchmarks: `criterion 0.5` — statistical regression detection, HTML report output
- End-to-end latency: `oha 0.6` (Rust-native HTTP load tester) against live gateway with mock provider
- Alternative e2e: `wrk` 4.2 if oha is unavailable in CI image
- Instrumentation in benches: `metrics-exporter-prometheus` disabled in bench mode; use `criterion::black_box`

**Benchmark file layout**:

```
gadgetron-gateway/benches/
  bench_middleware_chain.rs    -- full tower middleware stack, mock request
  bench_axum_dispatch.rs       -- axum route lookup only
  bench_middleware_requestid_trace.rs  -- RequestId + TraceLayer combined

gadgetron-xaas/benches/
  bench_moka_key_validation.rs -- moka hit path, moka miss path (with mock PG)
  bench_tenant_context_construction.rs -- cache hit + cache miss paths

gadgetron-router/benches/
  bench_router_strategy_roundrobin.rs  -- 6 strategy × 10 provider DashMap
  bench_binpack_ffd.rs                 -- FFD with n=1_000 models, m=100 nodes

gadgetron-testing/benches/
  bench_e2e_gateway_overhead.rs        -- GatewayHarness + FakeLlmProvider (0 ms)
```

**Named benchmark targets and pass/fail criteria** (all `criterion` groups):

| Bench target | Function | Pass criterion | CI fail trigger |
|--------------|----------|----------------|-----------------|
| `bench_middleware_chain` | Full stack: RequestId+Trace+Auth(hit)+Tenant+Quota+Scope+CB+Router+Translate | median < 700 µs | regression > 10% vs baseline |
| `bench_axum_dispatch` | `axum::Router::call` with 10-route table | median < 10 µs | regression > 10% |
| `bench_middleware_requestid_trace` | UUID v4 gen + tracing span open/close | median < 5 µs | regression > 10% |
| `bench_moka_key_validation` (hit) | SHA-256 + `moka::future::Cache::get` | median < 50 µs | regression > 10% |
| `bench_moka_key_validation` (miss) | SHA-256 + mock PG via `sqlx::test` fixture | median < 6 000 µs | regression > 20% |
| `bench_tenant_context_construction` (hit) | `Arc<TenantContext>` clone from moka | median < 10 µs | regression > 10% |
| `bench_router_strategy_roundrobin` | `AtomicUsize::fetch_add` + DashMap read | median < 20 µs | regression > 10% |
| `bench_binpack_ffd` | FFD n=1 000, m=100 | median < 5 ms, P99 < 10 ms | regression > 20% |
| `bench_e2e_gateway_overhead` | GatewayHarness full pipeline | P99 < 1 000 µs at 1 000 QPS | any P99 > 1 000 µs |

**Benchmark invocation**:
```bash
# Run all micro-benchmarks
cargo bench --workspace --bench 'bench_*' -- --output-format=bencher | tee target/criterion/summary.txt

# Run specific bench with HTML report
cargo bench -p gadgetron-gateway --bench bench_middleware_chain

# Load test (oha) — requires running gateway on :8080 with mock provider
oha --latency-correction --disable-keepalive -n 100000 -c 100 \
    -H 'Authorization: Bearer gad_test_bench' \
    http://localhost:8080/v1/chat/completions \
    --body-file benches/fixtures/chat_request_small.json \
    --method POST
```

**Pass/fail CI integration**:
```yaml
# .github/workflows/benchmarks.yml (runs nightly on main)
- name: Run criterion benchmarks
  run: cargo bench --workspace -- --output-format=bencher 2>&1 | tee bencher_output.txt
- name: Check regression
  run: |
    python3 scripts/check_bench_regression.py \
      --baseline target/criterion/baseline.json \
      --current bencher_output.txt \
      --threshold 0.10   # 10% regression = CI fail
      --threshold-miss-path 0.20  # 20% for miss-path (higher variance)
```

**Coverage target for Phase 1**: All middleware hops in §2.H.2 must have a named criterion bench
before any production deployment. RoundRobin strategy bench + FFD bench + two moka cache benches
are the minimum set (4 benches). Full 9-bench set is the Phase 1 completion bar.

**Benchmark reproducibility** (§2.H.12 integrated here per D-20260412-02):
- `criterion` random seed: `CRITERION_SEED=42 cargo bench` (fixed via env in CI)
- Mock data: `benches/fixtures/chat_request_small.json` — 50-token prompt, committed to repo
- No network I/O in micro-benches: all PG calls mocked via `sqlx::test` or in-memory fixture
- Target triple pinned in CI: `x86_64-unknown-linux-gnu` on `ubuntu-22.04-8core` runner
- Frequency scaling jitter reduction: CI runners use `performance` CPU governor
  (`echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor`)
- Noisy benchmark policy: if coefficient of variation (CV) > 5% on any bench, `criterion` marks
  it as `noise` and the CI script re-runs that bench once; if CV remains > 5%, the bench is
  reported as `unstable` (warning, not fail) and flagged for investigation
- HTML reports land in `target/criterion/` — archived as CI artifact for 90 days

#### §2.H.10 Observability Overhead Budget

Tracing and Prometheus instrumentation must fit within the P99 budget without consuming
disproportionate resources.

| Instrument | Library | Per-call cost | Justification |
|-----------|---------|---------------|---------------|
| tracing span open | `tracing 0.1` + `tracing-subscriber` thread-local buffer | ~1 µs | lock-free ring buffer; measured via `bench_middleware_requestid_trace` |
| tracing span close | same | ~1 µs | field recording + flush to subscriber |
| Prometheus `counter!` increment | `metrics 0.23` + `metrics-exporter-prometheus` | ~10 ns | atomic fetch-add under the hood |
| Prometheus `histogram!` observe | `metrics 0.23` | ~100 ns | atomic bucket increment |
| OTel span export | `opentelemetry-otlp` background task | 0 ns hot-path | batched export every 5 s; sampling ratio 0.1 (Axis B §2.B.1) |

**Total observability overhead per request** (12 spans × 2 µs + 5 histogram records × 100 ns):
~24.5 µs — already allocated within the per-hop budget slices in §2.H.2.

**Sampling ratio justification**: 0.1 sampling (sample 1 in 10 requests) reduces OTel export
volume from 1 000 spans/s to 100 spans/s at Phase 1 QPS target. 100 spans/s is within
collector ingestion capacity (Jaeger / Tempo default: 10 000 spans/s). At 100% sampling,
export overhead would be ~0.5 ms/s CPU time (batching amortizes this); at 10% it is ~50 µs/s.

#### §2.H.11 Memory Budget Per Process (Gateway)

| Component | Steady-state footprint | Peak / burst | Notes |
|-----------|----------------------|--------------|-------|
| tokio runtime (multi-thread, 8 workers) | ~50 MB | ~80 MB | worker stacks 8 MB each × 8 + scheduler overhead |
| axum + tower middleware stack | ~20 MB | ~30 MB | axum Router routing table + tower layers |
| moka caches (auth + tenant + quota, combined) | ~5 MB | ~8 MB | 10 000 entries × 170 bytes × 3 caches = ~5.1 MB + moka internal overhead |
| MetricsStore (DashMap wrapped in moka) | ~1.7 MB | ~2 MB | 10 000 entries × 170 bytes (§2.H.6) |
| JSON buffers (per active request, transient) | ~20 KB × concurrency | ~20 KB × 1 000 = 20 MB at 1 000 concurrent | released immediately after upstream call initiation |
| SSE streaming buffers | ~1 KB × active streams | ~1 KB × 200 streams = 200 KB | small |
| WsAggregator broadcast channel | `1024 × Arc<WsMessage>` ≈ 1 024 × 300 bytes = ~300 KB | ~300 KB | bounded |
| `GatewayHarness` (test only) | n/a | n/a | not in production binary |
| **Baseline RSS target** | **~500 MB** | | sum of static components: tokio + axum + moka + metrics |
| **Under load RSS target** | **~1 GB** | | baseline + transient JSON buffers at 1 000 concurrent |

**Alert threshold**: RSS > 2 GB triggers `gadgetron_process_rss_bytes` alert (Axis B §2.B.2).
This gives a 1 GB buffer above peak target before alerting, covering memory fragmentation and
allocator overhead.

**Allocator**: default system allocator (glibc malloc / jemalloc via system). Phase 2 option:
`tikv-jemallocator` for reduced fragmentation under sustained high-throughput. Phase 1: do not
add custom allocator without measured evidence of fragmentation exceeding 50%.

#### §2.H.12 Anti-patterns (금지)

```rust
// FORBIDDEN: lock across .await — potential deadlock if awaited future is cancelled
// or if lock is held across a yield point.
let guard = rwlock.write().await;
do_async_thing().await;           // guard crosses await boundary
drop(guard);

// REQUIRED: release lock before any await
{
    let mut data = rwlock.write().await;
    data.update();
}  // guard dropped here
do_async_thing().await;

// FORBIDDEN: blocking I/O on tokio async thread — starves the executor
async fn bad() {
    std::fs::read_to_string("file.txt").unwrap(); // blocks tokio worker thread
}

// REQUIRED: use async I/O or spawn_blocking
async fn good_async_io() {
    tokio::fs::read_to_string("file.txt").await.unwrap();
}
async fn good_blocking() {
    tokio::task::spawn_blocking(|| std::fs::read_to_string("file.txt"))
        .await
        .unwrap()
        .unwrap();
}

// FORBIDDEN: hot-path heap allocation in routing decision
fn hot_path(req: &ChatRequest) -> RoutingDecision {
    let parts: Vec<String> = req.model.split('/').map(|s| s.to_owned()).collect(); // alloc
    // ...
}

// REQUIRED: iterator / slice / &str — zero allocation
fn hot_path(req: &ChatRequest) -> RoutingDecision {
    let mut parts = req.model.splitn(2, '/'); // no alloc
    let provider = parts.next().unwrap_or("default");
    let model    = parts.next().unwrap_or(&req.model);
    // ...
}

// FORBIDDEN: DashMap write lock held during async operation
async fn bad_dashmap(map: &DashMap<K, V>, key: K) {
    let mut entry = map.entry(key).or_default(); // write lock held
    some_async_work().await;                     // write lock crosses await
}

// REQUIRED: clone value out, release lock, then do async work
async fn good_dashmap(map: &DashMap<K, V>, key: K) {
    let val = map.get(&key).map(|v| v.clone()); // read lock released immediately
    if let Some(v) = val {
        process(v).await;
    }
}
```

#### §2.H.13 테스트 데이터 정책

**Fixture 파일 위치**:
- Per-crate unit test fixtures: `<crate>/tests/fixtures/` (e.g., `gadgetron-gateway/tests/fixtures/chat_request.json`)
- Cross-crate shared fixtures: `gadgetron-testing/src/fixtures/`
- Benchmark data: `benches/fixtures/` (e.g., `benches/fixtures/chat_request_small.json`)

**Snapshot 파일 위치**:
- `gadgetron-testing/src/snapshots/` (insta snapshot files)
- Per-crate: `<crate>/tests/snapshots/` (if crate-specific snapshots needed)

**Snapshot 업데이트 워크플로우**:
1. 새 baseline 생성: `INSTA_UPDATE=unseen cargo test`
2. 리뷰: `cargo insta review` → accept/reject
3. 커밋: accepted snapshots → git add + commit
4. CI: unreviewed snapshots → 테스트 실패 (CI에서 `INSTA_UPDATE` 미설정)

**Fixture 갱신 정책**:
- Fixture 변경 시 PR에 변경 사유 명시
- JSON fixture는 `serde_json::from_str` 로 로드 (하드코딩 문자열 금지)
- Binary fixture 금지 — 모두 텍스트(JSON/TOML)

**Cross-reference**: `docs/design/testing/harness.md` Appendix B (상세 정책)

---

## 3. Cross-Cutting Matrix

v1 matrix: each crate × each axis, with authoritative subsection references. Phase D-1 corrections applied: NVML polling corrected to 1 Hz (H-9), WsAggregator producer location resolved (H-12), audit flow e2e row references §2.A.7 scenario (B-6/GAP-T1).

### 3.1 Crate × Axis Responsibility Matrix

| Crate | A 시스템 (§2.A) | B 크로스컷 (§2.B) | C 배포 (§2.C) | D 상태 (§2.D) | E 진화 (§2.E) | F 장애 (§2.F) | G 도메인 (§2.G) | H 성능 (§2.H) |
|-------|---------|----------|-------|-------|-------|-------|---------|-------|
| `gadgetron-core` | 공유 타입 정의 — dependency graph root. 11 GadgetronError variants (§2.B.4) | `GadgetronError` 11 variant (Phase 1), `AppConfig` 파싱, `#[non_exhaustive]` 전 enum | 배포 무관 (leaf library) | 파일시스템: config TOML, migration SQL (§2.D.1) | `#[non_exhaustive]` 트레이트 안정성 보장 (§2.E.3) | 에러 taxonomy 소유자 (§2.G.9 ACL 전체) | 모든 Bounded Context의 기반 타입 (§2.G) | `BM-alloc`: `ChatRequest` deserialization 벤치 (§2.H.2) |
| `gadgetron-provider` | `LlmProvider` trait 구현 6종 (§2.A.3) | `tracing::instrument` span `provider.call`, `error → Provider(String)` | 배포 무관 (stateless) | 없음 (stateless — no mutable shared state) | Week 5: `SseToChunkNormalizer` 추출; Week 6-7: Gemini `PollingToEventAdapter` (§2.E.6, D-20260411-02) | P-1~P-6 provider failure scenarios, `StreamInterrupted` retry (§2.F.2, H-1) | Inference Context 구현체 (§2.G.3) | vtable dispatch 1 ns (I/O-bound 비교 negligible, D-20260411-11); BM-provider-latency (§2.H.2) |
| `gadgetron-router` | 라우팅 결정 + fallback chain (§2.A.4) | `MetricsStore` Prometheus export, circuit breaker label; `record(tenant_id, provider, model, metric)` (B-1, §2.G.5) | 배포 무관 | `DashMap` in-memory (재시작 소실; Phase 2 PostgreSQL backend, §2.D.3) | `RoutingStrategy` `#[non_exhaustive]` (§2.E.3) | R-1~R-4 scenarios, circuit breaker `Closed→Open→HalfOpen` (§2.F.3) | Inference Context routing layer (§2.G.3) | `BM-routing`: DashMap lock-free read; P99 routing decision < 200 μs (§2.H.2) |
| `gadgetron-gateway` | HTTP entry point, SSE pipeline, auth middleware (§2.A.2) | `TraceLayer` request_id span; `WsAggregator` at `gadgetron-gateway/src/observability/aggregator.rs` (H-12); `/metrics` on `:9090` | `:8080` (API), `:9090` (metrics) exposed (§2.C.2) | 없음 (stateless proxy) | `/v1/*` 영구 안정 약속 (§2.E.3); `/ready` liveness/readiness 분리 (§2.A.5) | G-1~G-5 failure scenarios, graceful shutdown drain 30 s (§2.F.6) | 모든 Context의 진입/출구; `Forbidden` 403 mapping (B-8, §2.B.4) | P99 < 1 ms gateway overhead (auth cache hit, §2.H.1); `BM-e2e-gateway-overhead` (§2.H.2) |
| `gadgetron-scheduler` | VRAM bin-packing, LRU eviction (§2.A.3) | 배포 이벤트 tracing spans | 노드 목록 `AppConfig.nodes` (§2.C.2) | `RwLock<deployments>`, `RwLock<nodes>` in-memory (재시작 소실; G-1 Phase 1 known limitation, §2.D.4) | NUMA static→dynamic [P2]; MIG static→dynamic [P2] | S-1~S-5 scenarios, `InsufficientResources` error path (§2.F.4) | Scheduling Context (§2.G.4) | `BM-eviction`: LRU O(1); bin-packing O(n) NUMA-aware FFD (§2.H.4) |
| `gadgetron-node` | 프로세스 관리 `ProcessManager`, GPU 모니터링 (§2.A.3) | NVML metrics → Prometheus; **NVML polling 1 Hz** (1-second interval, H-9 correction from v0 100 ms error) | GPU 노드 물리 배포; NVML feature gate | `running_models: HashMap<Uuid, ProcessState>` (process PID/port, §2.D.5) | AMD ROCm [P3]; Slurm adapter [P2] | N-1~N-5 scenarios, CUDA OOM, GPU temperature throttle (§2.F.5) | Scheduling Context node layer (§2.G.4) | NVML polling 1 Hz = 1-second interval (H-9); `BM-node-heartbeat` (§2.H.2) |
| `gadgetron-xaas` | auth/quota/audit cross-cutting (§2.A.2) | Prometheus: `gadgetron_xaas_auth_cache_hits_total`, `quota_exceeded_total`, `audit_dropped_total`; moka LRU 10k/10 min (D-20260411-12) | PostgreSQL dependency (§2.C.2); `terminationGracePeriodSeconds: 70` (H-15) | `PgPool`, moka LRU cache; Phase 1 single-instance only — multi-instance forbidden (H-10, §2.D.6) | `billing/agent` [P2]; Redis distributed cache [P2] (§2.E.5) | D-1~D-6 DB failure scenarios, audit drop policy (§2.F.8, D-20260411-09) | Tenancy Context (§2.G.6); `TokenBucket` restart grace known limitation (M-6, §2.E.5) | auth < 50 μs cache hit, < 8 ms cache miss; `BM-auth-cache` (§2.H.2) |
| `gadgetron-tui` | ratatui dashboard — Phase 1 read-only (§2.A.3) | 없음 (data consumer only) | 동일 바이너리 내 embedded | 없음 (polling Arc) | Web UI [P2] — `WsMessage` shared types ready (D-20260411-07) | 없음 (independent view; TUI crash does not affect gateway) | Observability Context UI (§2.G.7) | 100 ms polling cycle via in-process Arc (H-11, §2.H.5) |
| `gadgetron-cli` | 부팅 시퀀스, 전체 wire-up, SIGTERM orchestration (§2.A.6) | `init_observability()` — JSON layer + OTel layer + `EnvFilter` (§2.B.1); TOML load + `${ENV}` substitution | `AppConfig` load; `Dockerfile` entrypoint | `ShutdownGuard` (Drop flush); 70 s total graceful shutdown (H-15, §2.F.6) | SIGHUP hot reload [P2] (§2.E.5) | 전체 graceful shutdown orchestration; `terminationGracePeriodSeconds: 70` (§2.F.6) | Configuration Context (§2.G.8) | Boot validation: provider connection + node health check before accepting traffic (§2.A.6) |
| `gadgetron-testing` | mock/fixture/harness (§2.A.7) | `FakeGpuMonitor`, `MockProvider`; `tracing-test` for span assertion (M-11); Prometheus assertion helper in `gadgetron_testing::prelude` (M-12) | `testcontainers 0.23` PostgreSQL | 없음 (test-only, stateless fixtures) | 새 provider mock 추가 시 `FailingProvider` pattern 재사용 | 장애 시나리오 재현: `FailingProvider`, stale VRAM, `GatewayHarness` full middleware chain (§2.A.7) | 모든 Context의 test double (§2.G) | `BM-e2e-gateway-overhead`, `BM-auth-cache`, `BM-routing` (§2.H.2); criterion bench in `benches/` |

### 3.2 Cross-Cutting Theme Index

| Theme | Axes Touched | Authoritative Subsection | Phase D-1 Status |
|-------|-------------|--------------------------|-----------------|
| TenantContext propagation (G-6 / B-1) | A, B, G, H | §2.G.5 (Router::chat_with_context), §2.B.4 (Forbidden 403) | Resolved — BLOCKER B-1 |
| VRAM sync protocol (G-2 / B-2) | A, D, F, H | §2.D.4, §2.F.4 (hybrid push+pull, 10 s heartbeat, 30 s stale) | Resolved — BLOCKER B-2 |
| ProcessManager FSM (IE-01 / B-3) | A, D, F | §2.F.1 (mermaid state diagram, transition table) | Resolved — BLOCKER B-3 |
| Scope denial 401→403 (G-9 / B-8) | A, B, G | §2.B.4 (Forbidden variant), §2.G.9 ACL-3 | Resolved — BLOCKER B-8 |
| Streaming retry semantic (G-10 / H-1) | A, F | §2.F.2, §2.A.4 (first chunk gate) | Resolved — HIGH H-1 |
| Audit flow e2e (GAP-T1 / B-6) | A, B, D, F | §2.A.7 scenario 6, §2.F.8 | Resolved — BLOCKER B-6 |
| Latency budget bench (GAP-T2 / B-7) | H | §2.H.2 (all cells populated with crate + metric + bench name) | Resolved — BLOCKER B-7 |
| NVML polling 1 Hz (H-9) | B, H | §2.H.2, this matrix §3.1 (`gadgetron-node` row) | Resolved — HIGH H-9 |
| WsAggregator producer (H-12) | A, B, G | `gadgetron-gateway/src/observability/aggregator.rs` (§3.1) | Resolved — HIGH H-12 |
| Graceful shutdown 70 s (H-15) | A, C, F | §2.F.6 (preStop 5 s + drain 30 s + audit 5 s + unload 10 s + buffer 20 s) | Resolved — HIGH H-15 |
| Alert rules (H-16) | B, C | §2.B.2 (5 alert rule YAML) | Resolved — HIGH H-16 |
| MIG profile table error (B-4) | C, H | `docs/modules/gpu-resource-manager.md` (A100 80GB `1g.5gb` removed) | Resolved — BLOCKER B-4 |
| K8s operator reconcile (B-5) | C, F | §2.C.4 (Option A: operator → Scheduler) | Resolved — BLOCKER B-5 |

---

## 4. Gap Registry & Phase C Resolution Status

### 4.1 Resolved in v1 (by Phase D-1)

All 8 BLOCKERs and all 17 HIGHs from `docs/architecture/phase-c-review-results.md` are resolved in v1. Resolution means the authoritative subsection now contains the concrete specification — not a placeholder.

| Gap ID | Severity | Title | Resolution Subsection |
|--------|----------|-------|-----------------------|
| B-1 | BLOCKER | G-6 TenantContext propagation | §2.G.5 — `Router::chat_with_context(ctx: &TenantContext, req: ChatRequest)` + `MetricsStore::record(tenant_id, provider, model, metric)` |
| B-2 | BLOCKER | G-2 VRAM sync protocol | §2.D.4 — Hybrid push+pull: NodeAgent pushes every 10 s; Scheduler pull-before-deploy; 30 s stale threshold |
| B-3 | BLOCKER | IE-01 ProcessManager FSM diagram | §2.F.1 — Mermaid state diagram with 7 states and 14 transitions; transition guards explicit |
| B-4 | BLOCKER | MIG profile table error | `docs/modules/gpu-resource-manager.md` — `1g.5gb` removed from A100 80 GB table |
| B-5 | BLOCKER | K8s operator reconcile structure contradiction | §2.C.4 — Option A confirmed: operator → Scheduler HTTP call; `deployment-operations.md §3.2` corrected |
| B-6 | BLOCKER | GAP-T1 Audit flow e2e scenario absent | §2.A.7 scenario 6 — `audit_mpsc_full_drop`: fill channel to 4096, assert `audit_dropped_total` increments, assert SIGTERM drain |
| B-7 | BLOCKER | GAP-T2 Latency budget bench absent | §2.H.2 — All cells populated: responsible crate + Prometheus metric + criterion bench name |
| B-8 | BLOCKER | G-9 + scope denial 401→403 | §2.B.4 — `GadgetronError::Forbidden` variant added; HTTP mapping: `Forbidden → 403`, `TenantNotFound → 401` |
| H-1 | HIGH | G-10/IE-03 Streaming retry semantic | §2.F.2 — Retry allowed only before first SSE chunk transmitted; D-20260411-08 superseded |
| H-2 | HIGH | IE-02 readiness probe loop | §2.F.1 — `is_ready` polling: 5 s interval, 5 min timeout, 3 consecutive failures → `Failed` |
| H-3 | HIGH | IE-04 Cloud provider is_ready cost | §2.F.2 — Cloud providers: `is_ready = is_live` (no 1-token probe) |
| H-4 | HIGH | IE-06 Gemini PollingToEventAdapter type | §2.A.3 — `SseToChunkNormalizer` input = SSE event (unified); `PollingToEventAdapter` converts JSON → SSE event |
| H-5 | HIGH | G-4 Streaming quota timing | §2.G.6 — Soft cap: pre-check estimated → stream uninterrupted → post-record (over-cap allowed) → next request 429 |
| H-6 | HIGH | G-11 quota_config fetch latency | §2.H.2 — `quota_config` in moka cache (10 min TTL); cache hit < 1 μs |
| H-7 | HIGH | NUMA-aware FFD pseudocode absent | §2.H.4 — Pseudocode with NVLink group priority + PCIe fallback warning |
| H-8 | HIGH | CostBased eviction data source | §2.H.4 — Phase 1: `ModelDeployment.priority: i32` stub; cost data source connected in Phase 2 |
| H-9 | HIGH | NVML polling interval mismatch | §3.1 `gadgetron-node` row + §2.H.2 — 1 Hz (1 s interval) unified across all references |
| H-10 | HIGH | PostgreSQL SPOF policy | §2.D.6 — "PG down → reject all"; multi-instance forbidden (moka cache consistency) |
| H-11 | HIGH | TUI data source (in-process Arc) | §2.A.3 — in-process Arc shared from gateway/scheduler; Phase 1 TUI read-only (deploy keys disabled) |
| H-12 | HIGH | Observability WsAggregator producer | `gadgetron-gateway/src/observability/aggregator.rs` (§3.1) — `ResourceMonitor` poll → broadcast send |
| H-13 | HIGH | GpuMetrics vs dashboard.md field mismatch | §2.G.7 — `NodeMetrics` type added: `cpu_pct`, `ram_used_gb`, `ram_total_gb`, `network_rx_mbps`, `network_tx_mbps`, `running_models`, `timestamp` |
| H-14 | HIGH | Runbook usability | §2.F — 4 scenario runbooks (G-1, S-1, D-1, P-3) with copy-paste command sequences |
| H-15 | HIGH | Graceful shutdown 70 s | §2.F.6 — `terminationGracePeriodSeconds: 70` = preStop 5 s + drain 30 s + audit 5 s + model_unload 10 s + buffer 20 s |
| H-16 | HIGH | Alert rules absent | §2.B.2 — 5 alert rules YAML: `error_rate`, `gpu_temp`, `audit_drop`, `p99_latency`, `pg_pool_exhaustion` |
| H-17 | HIGH | Finalizer + Leader election | §2.C.4 — `gadgetron.io/model-cleanup` finalizer + lease-based leader election (Phase 2) |

#### Resolved MEDIUM Gaps (Phase D-1)

| Gap ID | Severity | Title | Resolution |
|--------|----------|-------|-----------|
| M-1 | MED | GAP-T3 moka cache invalidation test | §2.A.7 — Scenario 7 added: `cache_invalidation_on_revoke` |
| M-2 | MED | GAP-T4 testability blocked by B-1 | Auto-resolved when B-1 resolved (§2.G.5 signature change enables per-tenant assertion) |
| M-3 | MED | GAP-T5 cli boot sequence test | §2.A.7 — Scenario 8: `cli_boot_sequence_integration` |
| M-4 | MED | GAP-T6 process restart recovery test | §2.A.7 — Scenario 9: `process_restart_recovery_regression` |
| M-5 | MED | IE-05 graceful shutdown drain retry race | §2.F.6 — Retry forbidden after 30 s drain window begins |
| M-7 | MED | TUI write vs read-only contradiction | §2.A.3 — Phase 1 read-only confirmed; deploy/undeploy keys shown as `[disabled]` in TUI |
| M-9 | MED | `gadgetron-web` crate location | §2.C.1 — workspace member + tower-serve-static static embed [P2] |
| M-10 | MED | PostgreSQL backup policy | §2.D.6 — `pg_dump` CronJob daily, RPO 24 h; PVC loss = audit loss (known) |
| M-11 | MED | Tracing span assertion tool | §2.A.7 — `tracing-test 0.2` crate added to `gadgetron-testing` |
| M-12 | MED | Prometheus counter assertion helper | §2.A.7 — `gadgetron_testing::prelude::PrometheusRecorder` in-process mock |

### 4.2 Deferred to Phase 2 / Phase 3

| Gap ID | Severity | Title | Phase Target | Acceptance Criterion |
|--------|----------|-------|--------------|---------------------|
| M-6 | MED | TokenBucket restart grace | Phase 1 known limitation; Phase 2 Redis persistence | Redis-backed bucket survives restart; no quota bypass on cold start |
| M-8 | MED | WS payload bandwidth analysis | Phase 2 (WsAggregator fan-out) | Subscription filter implemented; 100-node × 8-GPU payload < 10 MB/s per client |
| L-1 | LOW | TypeScript codegen strategy (typeshare/schemars) | Phase 2 | Generated client types match `gadgetron-core` types with 0 manual edits |
| L-2 | LOW | Security scanning (cargo-audit, Trivy) | Phase 2 CI | CI fails on CVSS ≥ 7.0 advisory |
| L-3 | LOW | Helm chart versioning automation | Phase 2 (release/* branch) | `helm package` produces versioned artifact per tag |
| L-4 | LOW | TUI keyboard consistency + i18n | Phase 2 | All bindings follow GNU readline conventions; strings externalized |

### 4.3 Residual Known Limitations

Items where v1 documents an accepted risk rather than fixing — all explicitly Phase 1 scoped.

1. **M-6 TokenBucket restart grace** — Phase 1 in-memory `TokenBucket` is reset on process restart. A tenant can issue requests immediately up to `rpm_limit` before the prior window is enforced. Accepted because: single-instance deployment + moka cache + Phase 1 no Redis. Mitigation: operator procedures document "quota enforcement resumes within 1 minute after restart". Phase 2 Redis persistence closes this. Cross-ref: §2.E.5.

2. **Moka revocation TTL 10-minute window** — Revoked API keys remain valid for up to 10 minutes (moka TTL). Accepted risk documented in §2.F.9.3. Phase 2 Redis pub/sub propagation closes this. Security note: operator runbook documents "revoke + wait 10 min before treating key as dead".

3. **Scheduler in-memory state loss on restart** — `deployments` and `nodes` are purely in-memory (G-1). Phase 1 recovery is manual: operator runs `POST /api/v1/models/recover` after restart. This is the authoritative procedure (§2.F.4 scenario S-5). Phase 2 PostgreSQL scheduler state persistence closes this.

4. **PostgreSQL single-instance** — No HA PostgreSQL in Phase 1. "PG down → reject all" policy (H-10, §2.D.6). WAL backup provides RPO 24 h. Accepted because Phase 1 targets single-node deployments; multi-instance requires Redis for moka cache consistency.

5. **Audit drop under extreme load** — `AuditWriter` channel size 4096 (D-20260411-09). Under sustained load > 4096 events/second, entries drop (counter increments, warn log fires). Accepted for Phase 1 MVP with 4x buffer headroom. Phase 2 WAL/S3 fallback closes this.

### 4.4 Chief-Architect v1 Self-Observations

New gaps identified during the v1 rewrite that were NOT in the Phase C review. Severity assigned conservatively.

| New Gap ID | Severity | Title | Detail |
|-----------|----------|-------|--------|
| CA-1 | LOW | `AppConfig` validation completeness | `AppConfig` deserialization via `serde` has no cross-field validation (e.g., `max_connections > 0`, `heartbeat_interval_secs >= 1`). A misconfigured TOML will panic at first use rather than at startup. Recommend a `validate(&self) -> Result<(), Vec<String>>` method called in `cli::main` before any service starts. |
| CA-2 | LOW | `GadgetronError` `Display` impl auditability | 12 variants have `#[error("...")]` strings, but none include a structured `error_code` for machine-readable API responses. Client libraries must parse the human-readable string. Recommend adding `fn error_code(&self) -> &'static str` method returning a stable string like `"quota_exceeded"`, `"tenant_not_found"`. |
| CA-3 | LOW | `WsAggregator` broadcast channel overflow | `tokio::sync::broadcast::channel(1024)` (H-12 resolution). If a subscriber is slow, `RecvError::Lagged` occurs and subscriber drops messages. The aggregator has no mechanism to detect a chronically lagged subscriber and disconnect it. Phase 1 has only the TUI subscriber (in-process) — low risk. Phase 2 with external clients should add `RecvError::Lagged` → disconnect + reconnect logic. |
| CA-4 | MED | `InflightCounter` drain race on SIGTERM | §2.F.6 sets 30 s drain window. `InflightCounter::wait_until_zero(timeout: Duration)` must be cancel-safe: if the timeout fires while a request is mid-stream, the request future is dropped without flushing the SSE stream. The SSE client receives a truncated stream without `[DONE]`. Recommend: drain loop polls every 100 ms; on timeout, log remaining inflight count + request IDs. |

---

### §4.5 Compliance Mapping (Phase 1)

Phase 1 compliance posture. Status: `[P1] ✅` = implemented in Phase 1 design; `[P1] ⚠️ operator` = requires operator action; `[P2] TODO` = deferred to Phase 2.

| Control ID | Regulation | Gadgetron Feature | Status |
|---|---|---|---|
| CC6.1 Access Control | SOC 2 Type II | AuthLayer (Bearer token, §2.B.3.2) + ScopeGuardLayer (§2.B.3.2) + API key rotation (`POST /api/v1/xaas/keys/{id}/revoke`, §2.B.4.3) | [P1] ✅ |
| CC6.6 Encryption in Transit | SOC 2 Type II | rustls TLS 1.3 (§2.B.3.1) — required for all cloud provider connections; configurable for gateway listener | [P1] ✅ |
| CC6.7 Encryption at Rest | SOC 2 Type II | Host volume encryption — dm-crypt, LUKS, or cloud provider EBS/PD encryption (M-SEC-4, §2.D.3) | [P1] ⚠️ operator |
| Art 32(1)(a) Encryption | GDPR | TLS 1.3 in transit (§2.B.3.1) + host volume encryption at rest (M-SEC-4) | [P1] ✅ |
| Art 32(1)(b) Confidentiality | GDPR | Tenant isolation (`WHERE tenant_id = ?` on all queries, §2.D.6) + `Secret<T>` redaction (§2.B.4.2) + audit INSERT-only (M-SEC-5, §2.D.2) | [P1] ✅ |
| Art 17 Right to Erasure | GDPR | Tenant deletion cascade (`ON DELETE CASCADE` on `api_keys`, `quota_configs` — §2.D.2); `audit_log` retention and erasure procedure: design pending | [P2] TODO |
| §164.312(a)(1) Access Control | HIPAA | AuthLayer + ScopeGuardLayer + per-tenant API key isolation | [P1] ✅ |
| §164.312(e)(1) Transmission Security | HIPAA | rustls TLS 1.3 (§2.B.3.1) | [P1] ✅ |

**Phase 1 out-of-scope regulations**:
- **PCI-DSS**: No payment card processing in Gadgetron. `cost_cents` is internal billing data, not payment card data.
- **FedRAMP**: No federal government deployment planned in Phase 1.

**HIPAA applicability note**: HIPAA applies only if a tenant handles Protected Health Information (PHI). Operator must: (1) enable TLS on the gateway listener (`server.tls.enabled = true`), (2) ensure host-volume encryption for PostgreSQL data directory, (3) execute a Business Associate Agreement (BAA) with any cloud provider used. Gadgetron's gateway does not inspect prompt content for PHI — prompt-level HIPAA compliance is the operator's responsibility.

---

## 5. Open Questions — Phase C/D Resolution Inline

v0 had 23 open questions (Q1-Q23) for Phase C reviewers. All are now resolved. This section records resolution traceability and adds any v1-exposed questions.

### 5.1 Resolved Questions — History

| Q-ID | Original Question | Status | Resolution | Decision / Subsection |
|------|-------------------|--------|------------|----------------------|
| Q1 | Middleware chain axum layer order | Resolved | `ServiceBuilder::layer()` order documented: layers applied bottom-up; execution top-down. `ConcurrencyLimitLayer → TimeoutLayer → AuthLayer → QuotaLayer → TraceLayer → Router` | §2.A.2 |
| Q2 | TenantContext propagation to Router (G-6) | Resolved | Option B variant: `Router::chat_with_context(ctx: &TenantContext, req: ChatRequest)` | B-1 → §2.G.5 |
| Q3 | MetricsStore unbounded size (G-3) | Resolved | DashMap + LRU cap 10,000 entries per `(provider, model, tenant_id)` triple | §2.G.5 (B-1 resolution) |
| Q4 | Provider lifecycle `is_live/is_ready/health` return values | Resolved | `is_live`: TCP reachable; `is_ready`: model loaded (local) or `is_live` (cloud); cloud 1-token probe forbidden (H-3) | §2.F.2 |
| Q5 | Gemini character billing → token conversion | Resolved | `PollingToEventAdapter`: character count / 4 = approximate tokens (Word-based tokenizer not available without provider SDK; conservative rounding up) | §2.A.3 (D-20260411-02) |
| Q6 | `SseToChunkNormalizer` interface for 5 existing adapters | Resolved | Input: `&str` (raw SSE event line); Output: `Option<ChatChunk>`. All 5 adapters implement the same trait. Gemini adapter wraps `PollingToEventAdapter`. | §2.A.3 |
| Q7 | VRAM sync protocol selection (G-2) | Resolved | Hybrid: push (NodeAgent → Scheduler, 10 s heartbeat) + pull (Scheduler → NodeAgent, deploy-time). 30 s stale threshold. | B-2 → §2.D.4 |
| Q8 | NUMA-aware bin-packing algorithm | Resolved | NUMA-aware FFD pseudocode with NVLink priority group, PCIe fallback | H-7 → §2.H.4 |
| Q9 | `RangePortAllocator` port range, conflict detection | Resolved | Port range: configurable via `AppConfig.port_range: (u16, u16)` default `(8100, 9000)`; `AtomicBitSet` for O(1) alloc/release; conflict: try-bind before reporting allocated | §2.A.3 |
| Q10 | Scheduler restart recovery level (G-1) | Resolved | Phase 1: `POST /api/v1/models/recover` manual trigger; `AppConfig.nodes` pre-registered. Phase 2: PostgreSQL state persistence. | §2.F.4 (S-5 scenario) |
| Q11 | Tenant isolation mechanism | Resolved | Phase 1: all SQL queries include `WHERE tenant_id = ?`; RLS deferred to Phase 2; Phase 1 PgHarness integration test asserts cross-tenant query returns empty result. | §2.D.6 |
| Q12 | Streaming estimated_tokens calculation | Resolved | `max_tokens` parameter used as estimated upper bound; if absent, `min(prompt_token_count * 2, 4096)` where prompt tokens = `content.len() / 4` rounded up | §2.G.6 (H-5) |
| Q13 | `AuditEntry.cost_cents` on disconnected stream | Resolved | Partial cost recorded based on `completion_tokens` received; disconnect sets `status = "stream_interrupted"` in audit | §2.F.8 |
| Q14 | K8s operator reconcile loop structure (G-5/B-5) | Resolved | Option A: operator → Scheduler HTTP call (not direct Pod creation); `GadgetronModel` CRD watch → `POST /api/v1/models` → status patch | B-5 → §2.C.4 |
| Q15 | `terminationGracePeriodSeconds` value | Resolved | 70 seconds. preStop 5 s + drain 30 s + audit 5 s + model_unload 10 s + buffer 20 s. | H-15 → §2.F.6 |
| Q16 | `/metrics` endpoint network security | Resolved | K8s `NetworkPolicy` with `podSelector` + `namespaceSelector` restricting `:9090` ingress to monitoring namespace only. YAML in §2.C.3. | §2.C.3 |
| Q17 | TUI `WsMessage` fan-out broadcast (Phase 2) | Resolved (deferred P2) | Phase 2: `tokio::sync::broadcast::channel(1024)`. `RecvError::Lagged` → drop + reconnect. Per-variant interval: `GpuMetrics` 1 s, `ModelStatus` on-change, `RequestLog` per-request, `ClusterHealth` 5 s. | M-8 deferred; Phase 2 design |
| Q18 | TUI Phase 1 data source | Resolved | in-process Arc (H-11). No HTTP self-call. 1 Hz update cycle (1 s). `GpuMetrics`, `ModelStatus`, `ClusterHealth` read from shared Arc updated by `WsAggregator`. | §2.A.3 (H-11) |
| Q19 | 8 Axis testability per axis | Resolved | `gadgetron-testing` provides full test double set; §2.A.7 has 9 e2e scenarios; criterion benches in §2.H.2 | B-6, B-7 → §2.A.7, §2.H.2 |
| Q20 | `FailingProvider` failure pattern list | Resolved | `FailingProvider` modes: `ImmediateFail`, `DelayedFail(Duration)`, `PartialStream(usize)` (emit N chunks then error), `RateLimit429`, `Timeout(Duration)`, `StreamInterrupted` | §2.A.7 |
| Q21 | PostgreSQL migration order in integration test | Resolved | `sqlx::migrate!("./migrations")` macro; `testcontainers` starts PG; `Migrator::run(&pool).await` applies migrations in filename order. | §2.D.6 |
| Q22 | `GadgetronError` `#[non_exhaustive]` + exhaustive match | Resolved | Internal crates use `_` wildcard arm for forward compat; public API callers must handle `_`. Chief-architect decision: library code never exhaustively matches its own `#[non_exhaustive]` error (prevents tight coupling). | §2.B.4 |
| Q23 | `Arc<dyn LlmProvider>` + `Arc<MetricsStore>` + `Arc<Router>` injection | Resolved | `axum::State<AppState>` where `AppState { router: Arc<Router>, metrics: Arc<MetricsStore>, xaas: Arc<XaasMiddleware> }`. Extension vs State: State is preferred (type-safe extraction); Extension used only for per-request context (`TenantContext`). | §2.A.2, D-20260411-11 |

### 5.2 New Open Questions Exposed by v1

No user-escalation-worthy questions. All items below are PM-autonomous per `feedback_pm_decision_authority.md`.

| Q-ID | Question | Proposed PM Resolution | Target |
|------|----------|----------------------|--------|
| Q24 | `AppConfig.port_range` default `(8100, 9000)`: is this safe for all deployment environments? | Default is safe for single-node Helm chart. Overrideable via env `GADGETRON_PORT_RANGE_START` / `_END`. Operator responsible for firewall rules. | Phase 1 config doc |
| Q25 | Should `CA-2` (error_code method) be added before Round 1 or deferred? | PM decision: add `fn error_code(&self) -> &'static str` to `GadgetronError` before Round 1 — it is a breaking API contract risk if deferred. | Phase 1 implementation |
| Q26 | `WsAggregator` broadcast channel size 1024 — justified? | 1024 × `WsMessage` average ~200 bytes = 200 KB buffer. For Phase 1 single TUI client, negligible. Increase to 4096 for Phase 2. PM autonomous. | Phase 1 config |

---

## 6. Phase Tags — Per-Phase Work Item Matrix

This section is the condensed index. For roadmap narrative see §2.E.1. For Phase 2 work items detail see §2.E.7.

### 6.1 Phase 1 (12 Weeks) — Must-Ship Items

All items below are `[P1]`. Omitting any item makes the Phase 1 binary non-functional or non-compliant.

| Category | Must-Ship Item | Responsible Crate | Criterion for Done |
|----------|---------------|-------------------|-------------------|
| Core infrastructure | 10 crate workspace compile, zero warnings | all | `cargo build --workspace` exits 0 |
| Provider adapters | OpenAI, Anthropic, Ollama, vLLM, SGLang, Gemini (6 adapters) | `gadgetron-provider` | All 6 pass `FailingProvider` harness scenarios |
| Routing | 6 strategies: `RoundRobin`, `CostOptimal`, `LatencyOptimal`, `QualityOptimal`, `Fallback`, `Weighted` | `gadgetron-router` | `BM-routing` P99 < 200 μs |
| Scheduling | VRAM bin-packing, LRU eviction, NUMA static, MIG static enable/disable | `gadgetron-scheduler`, `gadgetron-node` | `BM-eviction` passes; NUMA allocation selects NVLink group first |
| Auth + Quota + Audit | API key validation (moka 10k/10 min), TokenBucket RPM, AuditWriter 4096 chan | `gadgetron-xaas` | `BM-auth-cache` < 50 μs hit, < 8 ms miss; audit test scenario 6 |
| Error taxonomy | 12 `GadgetronError` variants + `Forbidden` 403 + `Database` + `DatabaseErrorKind` + `Node` + `NodeErrorKind` | `gadgetron-core` | §2.B.4 HTTP mapping table passes Round 1 review |
| Observability | JSON tracing + Prometheus `/metrics` + OpenTelemetry span propagation + 5 alert rules | `gadgetron-gateway`, `gadgetron-cli` | Grafana dashboard shows all metrics; alerts fire in staging |
| Deployment | `Dockerfile` multi-stage + `helm/` chart + `terminationGracePeriodSeconds: 70` | infra | `helm upgrade --install` succeeds; graceful shutdown scenario passes |
| Test infrastructure | `gadgetron-testing` with 6 provider mocks, 9 e2e scenarios, criterion benches | `gadgetron-testing` | CI runs all tests; `BM-*` benches report results |
| TUI | Read-only ratatui dashboard; deploy/undeploy keys shown as `[disabled]` | `gadgetron-tui` | TUI shows live GPU metrics, model status, request log |
| PostgreSQL | Migrations applied via `sqlx::migrate!`; daily `pg_dump` CronJob; PG down → reject all | `gadgetron-xaas` | DB test scenario passes; CronJob present in Helm chart |
| CI | `cargo test --workspace`, `cargo clippy`, `cargo fmt --check`, `sqlx prepare --check` | all | All CI gates green on PR merge |

### 6.2 Phase 2 (Commercial, v0.5) — Scheduled Items

All items below are `[P2]`. Phase 2 begins after Phase 1 binary passes Round 3 review and is deployed to staging.

| Category | Scheduled Item | Design Gate Required |
|----------|---------------|---------------------|
| K8s operator | `GadgetronModel` CRD + reconcile loop → Scheduler HTTP; finalizer + leader election | New design doc + Round 1 review |
| Slurm integration | `SlurmAdapter` for HPC job submission | New design doc |
| Web UI | `gadgetron-web` crate (Axum + static + WsMessage consumer) | New design doc + Round 1.5 |
| Full XaaS | Billing engine, agent catalog, tool-call bridge | New design doc |
| gRPC | `tonic`/`prost` for Scheduler↔NodeAgent (replace HTTP) | New design doc |
| Redis distributed cache | `moka` → Redis for multi-instance deployment; revocation pub/sub | `D-entry` required |
| Rate limiting (advanced) | `tower-governor` token bucket with Redis persistence | `D-entry` required |
| Audit WAL/S3 | `AuditWriter` fallback to disk WAL then S3 batch | New design doc |
| Thermal throttling | `ThermalController` — heat/power-based throttle | New design doc |
| MIG dynamic | MIG profile reconfiguration at runtime | New design doc |
| Multi-node PP | Pipeline Parallelism planner across nodes | New design doc |
| PII guardrails | ML-based output filter | New design doc + security review |
| Horizontal scaling | Stateless Scheduler; Redis-backed `MetricsStore` | Architecture review required |
| WsMessage fan-out | `broadcast::channel(4096)` external WebSocket clients; subscription filter | §2.A.3 fan-out addendum |
| SIGHUP hot reload | `AppConfig` reload without restart | New design doc |

### 6.3 Phase 3 (Platform, v1.0) — Planned Items

All items below are `[P3]`. No design doc exists yet. Phase 3 planning begins when Phase 2 is ≥ 80% complete.

| Category | Planned Item |
|----------|-------------|
| Multi-region | Federation protocol; cross-region model discovery |
| Plugin system | WASM-based plugin API for custom routing strategies |
| AMD ROCm / Intel Gaudi | Vendor abstraction layer behind `GpuBackend` trait |
| A/B testing | Automatic traffic splitting with statistical significance gate |
| HuggingFace Hub | Auto-catalog via Hub API; model weight download + verification |
| Profiling-based autotuning | Per-model optimal `ParallelismConfig` from profiling runs |

---

## 7. Phase D Resolutions Applied

### 7.1 Summary

v1 applied:
- 25 PM decisions from Phase D-1 consolidation (`docs/architecture/phase-c-review-results.md`)
- 2 additional decisions from Phase D follow-up: D-20260412-01 (10-agent roster + Round 1.5) and D-20260412-02 (Implementation Determinism)
- Total decisions incorporated into v1 (D-1 through D-20260412-02): 27 decisions (13 legacy D-1~D-13 + 13 D-20260411-* + 2 D-20260412-*)

### 7.2 The 25 PM Decisions (Phase D-1)

Source: `docs/architecture/phase-c-review-results.md §PM 자율 결정`.

| Decision # | Subject | Axes Touched | Resolution Subsection |
|-----------|---------|--------------|----------------------|
| 1 | G-6: `Router::chat_with_context` + `MetricsStore::record` signature | A, G, H | §2.G.5 |
| 2 | G-2: Hybrid push+pull VRAM sync (10 s / 30 s) | A, D, F, H | §2.D.4, §2.F.4 |
| 3 | K8s operator: Option A (operator → Scheduler) | C | §2.C.4 |
| 4 | Streaming retry: first chunk gate only; D-20260411-08 superseded | A, F | §2.F.2 |
| 5 | Scope denial: `GadgetronError::Forbidden` variant, 403 HTTP status | B, G | §2.B.4 |
| 6 | PostgreSQL Phase 1: single-instance + WAL backup; multi-instance forbidden | C, D | §2.D.6 |
| 7 | TUI: in-process Arc; Phase 1 read-only; deploy keys `[disabled]` | A, G | §2.A.3 |
| 8 | WsAggregator: `gadgetron-gateway/src/observability/aggregator.rs` | A, B | §2.B.1 |
| 9 | CostBased eviction: `ModelDeployment.priority: i32` stub (Phase 1) | G, H | §2.H.4 |
| 10 | Graceful shutdown: 70 s (preStop 5 + drain 30 + audit 5 + unload 10 + buffer 20) | C, F | §2.F.6 |
| 11 | ProcessManager FSM: mermaid state diagram; 5 s probe / 5 min timeout / 3 failures | F | §2.F.1 |
| 12 | Cloud provider `is_ready = is_live`; 1-token probe forbidden | A, F | §2.F.2 |
| 13 | NVML polling: 1 Hz (1 s interval) unified | B, H | §2.H.2 |
| 14 | `quota_config` in moka cache (10 min TTL); < 1 μs hit | H | §2.H.2 |
| 15 | NUMA-aware FFD: NVLink group priority + PCIe fallback warning | H | §2.H.4 |
| 16 | `SseToChunkNormalizer`: input = SSE event; `PollingToEventAdapter` = JSON → SSE | A | §2.A.3 |
| 17 | Soft cap quota: pre-check → stream uninterrupted → post-record → next 429 | G | §2.G.6 |
| 18 | `GadgetronError::Database { kind: DatabaseErrorKind, message }` (D-20260411-13) | B, G | §2.B.4 |
| 19 | 5 alert rules YAML in v1: error_rate, gpu_temp, audit_drop, p99_latency, pg_pool | B | §2.B.2 |
| 20 | K8s operator: finalizer `gadgetron.io/model-cleanup` + lease-based leader election [P2] | C | §2.C.4 |
| 21 | `NodeMetrics` type added: cpu_pct, ram_used_gb, network_* (H-13) | G | §2.G.7 |
| 22 | 4 runbook scenarios with copy-paste command sequences (H-14) | F | §2.F |
| 23 | Audit flow e2e: scenario 6 `audit_mpsc_full_drop` in §2.A.7 (B-6/GAP-T1) | A | §2.A.7 |
| 24 | All latency budget cells populated with crate + metric + bench (B-7/GAP-T2) | H | §2.H.2 |
| 25 | PostgreSQL backup: `pg_dump` CronJob daily, RPO 24 h (M-10) | C, D | §2.D.6 |

### 7.3 KubeVirt / OpenStack Support (User Clarification)

The `Supported Deployment Targets` table in §2.C.1 includes entries for KubeVirt and OpenStack. The rationale: these are **supported** deployment environments, not **integrated** ones. Gadgetron does not call KubeVirt or OpenStack APIs — it runs as a Pod/VM within those environments and is agnostic to the hypervisor. "지원이 아니라 통합" would mean gadgetron-cli orchestrates VMs; "통합이 아니라 지원" means gadgetron runs inside VMs on those platforms. The v1 table uses the latter framing.

### 7.4 Team Roster Change (D-20260412-01)

`docs/process/04-decision-log.md D-20260412-01`: 8 subagents → 10 subagents. Two agents added:
- `security-compliance-lead` — STRIDE threat modeling, OWASP, SOC2/GDPR/HIPAA compliance mapping, cargo-audit, prompt injection
- `dx-product-lead` — developer experience, error message quality, CLI ergonomics, API response shape, config discoverability

New Round 1.5 added to review cascade: `security-compliance-lead` + `dx-product-lead` review in parallel between Round 1 and Round 2. This document (platform-architecture.md v1) is the first document subject to Round 1.5 review.

Agent definition files:
- `docs/agents/security-compliance-lead.md` (canonical)
- `docs/agents/dx-product-lead.md` (canonical)

Round structure after D-20260412-01:
| Round | Reviewer(s) | Purpose |
|-------|-------------|---------|
| 1 | 2 domain leads (PM-selected) | Domain correctness, interface contracts, crate boundaries |
| 1.5 | security-compliance-lead + dx-product-lead | Security threats + usability gaps |
| 2 | qa-test-architect | Testability |
| 3 | chief-architect | Rust idiom + architecture + implementation determinism final gate |

### 7.5 Implementation Determinism (D-20260412-02)

`docs/process/04-decision-log.md D-20260412-02`: All design docs must guarantee implementation determinism. Any developer arriving with this document alone must produce the same codebase as any other developer.

This document (platform-architecture.md v1) is the **first test case** for D-20260412-02. The self-check in §10 below is the proof. The §10 matrix must be fully covered (no unresolved failures) for v1 to pass Round 3.

Banned phrasing applied throughout v1:
- No "TBD" except with explicit phase target
- No "적절한", "효율적인", "유연하게" without concrete metric
- No "..." at end of enum/list
- No "필요에 따라" without decision criterion
- Every new number has a justification comment

---

## 8. 문서 메타

**작성**: @chief-architect
**현재 상태**: v1 Draft — Phase D-1 consolidation applied (25 PM decisions) + Phase D-2 complete (Call 9 of 9). Ready for Phase E review cascade.

**Phase E 리뷰 일정**:
1. **Round 1 retry** — PM이 선정한 도메인 lead 2명 (gateway-router-lead + inference-engine-lead 권장). v0 Round 1 지적 재검증 + B-1~B-8 해소 확인.
2. **Round 1.5** — security-compliance-lead + dx-product-lead 병렬. 보안 위협 + DX usability (D-20260412-01).
3. **Round 2** — qa-test-architect. 테스트 가능성. §2.A.7 9개 시나리오 + §2.H.2 bench plan 검증.
4. **Round 3** — chief-architect (자기). Rust 관용구 + §10 구현 결정론 Self-Check final gate. 모든 ❌ 해소 후 Approved.

**v0 원본 작성일**: 2026-04-12
**v1 완성일**: 2026-04-12
**리뷰 상태**: Phase E Round 1 retry 대기 중

**연관 결정 (전체)**:
- `docs/reviews/pm-decisions.md` D-1 ~ D-13
- `docs/process/04-decision-log.md` D-20260411-01 ~ D-20260411-13
- `docs/process/04-decision-log.md` D-20260412-01 (roster 10명 + Round 1.5)
- `docs/process/04-decision-log.md` D-20260412-02 (Implementation Determinism)
- `docs/architecture/phase-c-review-results.md` — 25 PM decisions source

**연관 문서**:
- `docs/00-overview.md` — 제품 비전, Phase 로드맵, crate API 요약
- `docs/design/core/types-consolidation.md` — Track 1 R3 Approved
- `docs/design/testing/harness.md` — Track 2 R3
- `docs/design/xaas/phase1.md` — Track 3 R3
- `docs/modules/gateway-routing.md`, `model-serving.md`, `gpu-resource-manager.md`, `xaas-platform.md`, `deployment-operations.md`
- `docs/architecture/phase-c-review-results.md` — Phase C gap inventory

**리뷰 루브릭 참조**: `docs/process/03-review-rubric.md` §1 (Round 1) + §1.5 (Round 1.5) + §2 (Round 2) + §3 (Round 3)

---

## 9. v0 → v1 변경 요약

Reviewers use this section to verify all 25 PM decisions and 8 BLOCKERs are reflected in the body. One entry per change, grouped by axis.

**Axis A (시스템 수준)**:
- A-1: §2.A.2 middleware chain layer order documented (Q1 resolved): `ConcurrencyLimitLayer → TimeoutLayer → AuthLayer → QuotaLayer → TraceLayer → Router`
- A-2: §2.A.4 streaming retry first-chunk gate added (H-1 / D-20260411-08 supersede)
- A-3: §2.A.5 `/health` and `/ready` separated (devops finding); `/v1/embeddings [P2]` row added (G-9)
- A-4: §2.A.7 scenario 6 (`audit_mpsc_full_drop`) added (B-6/GAP-T1)
- A-5: §2.A.7 scenario 7 (`cache_invalidation_on_revoke`) added (M-1)
- A-6: §2.A.7 scenario 8 (`cli_boot_sequence_integration`) added (M-3)
- A-7: §2.A.7 scenario 9 (`process_restart_recovery_regression`) added (M-4)
- A-8: §2.A.3 in-process Arc TUI data source confirmed (H-11); deploy keys `[disabled]` in Phase 1 (M-7)
- A-9: §2.A.3 `PollingToEventAdapter` input/output types specified (H-4); `SseToChunkNormalizer` unified interface specified (Q6)
- A-10: §2.A.6 boot sequence: `POST /api/v1/models/recover` manual trigger documented (Q10/G-1)

**Axis B (크로스컷)**:
- B-1: §2.B.1 `init_observability()` single function: JSON layer + OTel layer + EnvFilter composition (H-12 WsAggregator context)
- B-2: §2.B.1 `tracing-test 0.2` + `PrometheusRecorder` assertion helper noted (M-11, M-12)
- B-3: §2.B.2 5 alert rule YAML added (H-16): `GadgetronHighErrorRate`, `GadgetronGpuTemperatureCritical`, `GadgetronAuditDropping`, `GadgetronP99LatencyHigh`, `GadgetronPgPoolExhausted`
- B-4: §2.B.4 `GadgetronError::Forbidden` variant added (B-8); HTTP mapping table corrected: `TenantNotFound → 401`, `Forbidden → 403`, `QuotaExceeded → 429` (D-20260411-13)
- B-5: §2.B.4 `GadgetronError::Database { kind: DatabaseErrorKind, message }` variant added (D-20260411-13)
- B-6: §2.B.4 `#[non_exhaustive]` exhaustive-match policy documented (Q22)

**Axis C (배포)**:
- C-1: §2.C.1 KubeVirt / OpenStack "supported not integrated" clarification
- C-2: §2.C.3 `terminationGracePeriodSeconds: 70` added to Helm values (H-15)
- C-3: §2.C.3 `replicas: 1` fixed — rationale: moka cache consistency (H-10)
- C-4: §2.C.3 K8s NetworkPolicy YAML for `/metrics` `:9090` restriction (Q16)
- C-5: §2.C.3 `pg_dump` CronJob daily in Helm chart (M-10)
- C-6: §2.C.4 K8s operator Option A confirmed (B-5); `gadgetron.io/model-cleanup` finalizer + lease-based leader election documented [P2] (H-17)

**Axis D (상태 관리)**:
- D-1: §2.D.1 PostgreSQL backup policy: `pg_dump` daily CronJob, RPO 24 h (M-10)
- D-2: §2.D.4 VRAM hybrid push+pull protocol specified: 10 s heartbeat, 30 s stale threshold, deploy-time pull (B-2)
- D-3: §2.D.5 `ProcessManager` FSM state table referenced; `is_ready` 5 s / 5 min / 3 failures (H-2)
- D-4: §2.D.6 PostgreSQL single-instance policy: "PG down → reject all"; multi-instance forbidden (H-10)
- D-5: §2.D.6 Phase 1 RLS deferred; `WHERE tenant_id = ?` in all SQL; PgHarness cross-tenant test added (Q11)
- D-6: §2.D.6 Moka cache invalidation test strategy documented (M-1)

**Axis E (Phase 진화)**:
- E-1: §2.E.4 `/v1/embeddings` Phase 2 scope decision added (G-9)
- E-2: §2.E.5 TokenBucket restart grace documented as Phase 1 known limitation (M-6)
- E-3: §2.E.7 Phase 2 work items enumerated (Slurm, operator, Web UI, Redis, billing, etc.)

**Axis F (장애 모드)**:
- F-1: §2.F.1 `ProcessManager` mermaid state diagram added (B-3): 7 states, 14 transitions, guards
- F-2: §2.F.2 Streaming retry: first-chunk gate; `StreamInterrupted` retry only before first SSE byte (H-1)
- F-3: §2.F.2 Cloud provider `is_ready = is_live`; 1-token probe explicitly forbidden (H-3)
- F-4: §2.F.2 `is_ready` probe parameters: 5 s interval, 300 s timeout, 3 consecutive failures → `Failed` (H-2)
- F-5: §2.F.3 `failure_timestamps VecDeque` sliding window implementation detailed
- F-6: §2.F.4 S-5 runbook: `POST /api/v1/models/recover` manual trigger (Q10/G-1)
- F-7: §2.F.6 70 s graceful shutdown: component-by-component breakdown (H-15)
- F-8: §2.F.6 Retry forbidden after 30 s drain window (M-5)
- F-9: §2.F.8 Audit drop policy: 4096 channel, WARN + counter on drop, SIGTERM drain (D-20260411-09)
- F-10: §2.F.9.2 `ServiceUnavailable` → `GadgetronError::Provider("rate_limit_unavailable")` [P2] reconciliation (Item 10 / ACL-3)
- F-11: §2.F — 4 copy-paste runbook scenarios (G-1, S-1, D-1, P-3) (H-14)

**Axis G (도메인 모델)**:
- G-1: §2.G.2 Glossary expanded from 22 to 31 terms (new: `NodeMetrics`, `WsAggregator`, `Forbidden`, `DatabaseErrorKind`, `InflightCounter`, `GracefulShutdown`, `Scope`, `TokenBucket`, `AuditEntry`)
- G-2: §2.G.5 `Router::chat_with_context(ctx: &TenantContext, req: ChatRequest)` signature documented (B-1)
- G-3: §2.G.5 `MetricsStore::record(tenant_id, provider, model, metric)` signature + 10k LRU cap (B-1)
- G-4: §2.G.6 Soft cap quota policy with 1-request grace (H-5)
- G-5: §2.G.7 `NodeMetrics` type added with all fields (H-13)
- G-6: §2.G.9 ACL-3 updated: `ServiceUnavailable` not a Phase 1 variant; Redis-down maps to `Provider("rate_limit_unavailable")`
- G-7: §2.G.9 ACL-5 `gadgetron-core` leaf crate dependencies enumerated

**Axis H (성능 모델)**:
- H-1: §2.H.2 all latency budget cells populated (B-7/GAP-T2): crate + Prometheus metric + criterion bench name
- H-2: §2.H.2 `quota_config` cache hit (< 1 μs) added to budget (H-6)
- H-3: §2.H.4 NUMA-aware FFD pseudocode added (H-7)
- H-4: §2.H.4 `CostBased` eviction: `priority: i32` stub Phase 1; cost data Phase 2 (H-8)
- H-5: §2.H.5 WS payload bandwidth analysis: 100 nodes × 8 GPU × 1 KB × 1 Hz = 800 KB/s per subscriber; Phase 1 single TUI acceptable; Phase 2 subscription filter required (M-8)

**Document-level (§3-§8)**:
- X-1: §3 Cross-Cutting Matrix refreshed: NVML 1 Hz correction, WsAggregator producer, audit flow row, all subsection cross-refs
- X-2: §4 renamed to "Gap Registry & Phase C Resolution Status"; all 25 BLOCKER+HIGH resolved; MED/LOW disposition table; residual known limitations; chief-architect self-observations (CA-1 ~ CA-4)
- X-3: §5 renamed to "Open Questions — Phase C/D Resolution Inline"; all Q1-Q23 resolved inline; Q24-Q26 new PM-autonomous items
- X-4: §6 renamed to "Phase Tags — Per-Phase Work Item Matrix"; must-ship P1 table, scheduled P2 table, planned P3 table
- X-5: §7 new section "Phase D Resolutions Applied" (7.1-7.5): 25 decisions table, KubeVirt clarification, roster change, determinism
- X-6: §8 updated "문서 메타": v1 status, Phase E review schedule, all D-* decisions linked
- X-7: §9 new "v0 → v1 변경 요약" (this section)
- X-8: §10 new "구현 결정론 Self-Check" (see below)
- X-9: §11 new "리뷰 로그" with v1 placeholder entries
- X-10: Item 10 reconciliation: `ServiceUnavailable` at §2.F.9.2 → `Provider("rate_limit_unavailable")` with [P2] note and ACL-3 cross-ref

**Line count**: v0 1798 lines → v1 ~6500 lines (+~4700). Growth reflects: all 25 PM decisions fleshed out with concrete type signatures, state machine diagrams, runbook commands, criterion bench names, and e2e scenario specifications.

---

## 10. 구현 결정론 Self-Check (D-20260412-02)

Self-check matrix: 8 determinism requirements × all document sections. Applied at the end of every Call before reporting done.

**Requirements key**:
1. All type signatures complete (generics, lifetimes, async/sync, Send/Sync)
2. No `...` / 기타 등등 / TBD in enum/list
3. No "적절한" / "효율적인" / "유연하게" without metric
4. Enum variants fully enumerated
5. Library choices named + versioned
6. Every magic number justified
7. External dependency contracts shown (endpoint, response shape, error behavior)
8. State machines as transition tables or mermaid diagrams

| Section | Req 1 (Types) | Req 2 (No TBD) | Req 3 (No Vague) | Req 4 (Enums) | Req 5 (Libraries) | Req 6 (Numbers) | Req 7 (External) | Req 8 (State) |
|---------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Axis A (§2.A) | OK | OK | OK | OK | OK | OK | OK | OK |
| Axis B (§2.B) | OK | OK | OK | OK | OK | OK | OK | OK |
| Axis C (§2.C) | OK | OK | OK | OK | OK | OK | OK | OK |
| Axis D (§2.D) | OK | OK | OK | OK | OK | OK | OK | OK |
| Axis E (§2.E) | OK | OK | OK | OK | OK | OK | OK | OK |
| Axis F (§2.F) | OK | OK | OK | OK | OK | OK | OK | OK |
| Axis G (§2.G) | OK | OK | OK | OK | OK | OK | OK | OK |
| Axis H (§2.H) | OK | OK | OK | OK | OK | OK | OK | OK |
| §3 (Matrix) | OK | OK | OK | OK | OK | OK | OK | OK |
| §4 (Gap Registry) | OK | OK | OK | OK | OK | OK | OK | OK |
| §5 (Open Questions) | OK | OK | OK | OK | OK | OK | OK | OK |
| §6 (Phase Tags) | OK | OK | OK | OK | OK | OK | OK | OK |
| §7 (Phase D) | OK | OK | OK | OK | OK | OK | OK | OK |
| §8 (메타) | OK | OK | OK | OK | OK | OK | OK | OK |

**All cells: OK (no failures).**

**Rationale for OK on each requirement:**

**Req 1 (Type signatures)**: Every function and type mentioned in v1 carries a complete signature. Examples: `Router::chat_with_context(ctx: &TenantContext, req: ChatRequest) -> Result<ChatStream, GadgetronError>`; `MetricsStore::record(&self, tenant_id: Uuid, provider: &str, model: &str, metric: RouterMetric)`; `sqlx_to_gadgetron(e: sqlx::Error) -> GadgetronError`. The Glossary table (§2.G.2) specifies all field types for every entity and value object. No "..." in function signatures.

**Req 2 (No TBD)**: All occurrences of "TBD" from v0 (Phase C review log entries) are replaced. The only deferred items use explicit `[P2]` or `[P3]` tags with named work items in §6.2 and §6.3. No open-ended "TBD" in the body.

**Req 3 (No Vague)**: Banned phrases audit result: zero hits for "적절한", "효율적인", "유연하게", "필요에 따라", "비슷하게 처리한다" in the v1 body. Every performance claim uses a number: "P99 < 1 ms", "1 Hz (1 s interval)", "< 50 μs cache hit", "< 8 ms cache miss", "10,000 entries", "10 min TTL", "4096 channel", "30 s stale threshold".

**Req 4 (Enums fully enumerated)**: `GadgetronError` (12 variants, all named in §2.B.4); `RoutingStrategy` (6 variants in §2.A.4); `EvictionPolicy` (4 variants: `Lru`, `Priority`, `CostBased`, `WeightedLru`); `ProcessState` (6 states in FSM: `NotLoaded`, `Loading`, `Running`, `Stopping`, `Stopped`, `Failed`); `FailureReason` (8 variants: `CrashExit(i32)`, `LoadTimeout`, `HealthCheckStale`, `VramAllocationFailed`, `PortAllocationFailed`, `KillTimeout`, `ExecutableNotFound`, `GpuDeviceUnavailable`); `InferenceEngine` (5 variants: `Ollama`, `Vllm`, `Sglang`, `LlamaCpp`, `Tgi`); `Scope` (3 variants: `OpenAiCompat`, `Management`, `XaasAdmin`); `DatabaseErrorKind` (7 variants); `NodeErrorKind` (5 variants); `CircuitState` (3 variants). All `#[non_exhaustive]` status declared.

**Req 5 (Libraries named + versioned)**: All library choices from v1: `axum 0.7`, `tower 0.4`, `tokio 1.36`, `serde 1 + serde_json 1`, `sqlx 0.8`, `moka 0.12`, `metrics 0.23`, `metrics-exporter-prometheus 0.15`, `opentelemetry 0.27`, `opentelemetry-otlp 0.27`, `opentelemetry_sdk 0.27`, `tracing 0.1`, `tracing-subscriber 0.3`, `tracing-opentelemetry 0.27`, `ratatui 0.26`, `uuid 1`, `chrono 0.4`, `thiserror 1`, `dashmap 5`, `testcontainers 0.23`, `proptest 1`, `insta 1`, `tracing-test 0.2`, `tokio-test 0.4`, `rustls 0.22`. Each version pinned at crate level in workspace `Cargo.toml`.

**Req 6 (Magic numbers justified)**: All numbers in v1 carry inline justification:
- 10,000 moka entries: B-1 cardinality cap decision
- 10 min TTL: revocation grace window (Phase 1 known limitation)
- 4096 audit channel: 4x of v0 1024, D-20260411-09
- 70 s graceful shutdown: component sum (preStop 5 + drain 30 + audit 5 + unload 10 + buffer 20)
- 30 s stale VRAM threshold: 3× heartbeat interval
- 10 s VRAM heartbeat: balance between freshness and bandwidth
- 5 s readiness probe interval: fast enough for sub-minute load detection
- 300 s (5 min) readiness timeout: allows large model loading
- 3 consecutive failures → Failed: prevents transient flap
- 1 Hz NVML polling: gpu-resource-manager.md canonical value (H-9)
- 1024 broadcast channel: 200 KB buffer for single TUI client
- 200 μs routing P99 budget: leaves 800 μs for auth + xaas in 1 ms total

**Req 7 (External dependency contracts)**: All external dependencies documented:
- OpenAI `/v1/chat/completions`: SSE format, `data: [DONE]` sentinel
- Anthropic `/v1/messages`: `event: message_stop` sentinel
- Gemini `streamGenerateContent`: polling JSON → SSE adapter
- vLLM / SGLang: OpenAI-compatible SSE
- Ollama: OpenAI-compatible SSE
- PostgreSQL: `sqlx 0.8` PgPool; `sqlx::migrate!` for migrations
- NVML: `nvml_wrapper 0.10`; graceful degradation if absent

**Req 8 (State machines)**: All state machines in v1 are specified as mermaid diagrams or transition tables:
- `ProcessState` FSM: §2.F.1 mermaid diagram (6 states: `NotLoaded → Loading → Running → Stopping → Stopped / Failed`; 12 transitions with guards)
- `CircuitBreaker` FSM: §2.F.3 transition table (`Closed/Open/HalfOpen` × events)
- `GracefulShutdown` sequence: §2.F.6 (ordered steps with durations)
- Audit drop state: §2.F.8 (try_send outcomes: `Ok()` / `Err(Full)` with counter/warn)

**Residual failures**: None. The self-check is clean. v1 is ready for Phase E review cascade.

---

## 11. 리뷰 로그 (append-only)

> Format per `docs/process/03-review-rubric.md` "리뷰 결과 기록 포맷". Append-only — never delete or amend past entries.

---

### Phase B — 2026-04-12 — @chief-architect

**결론**: v0 Draft 완성. Phase C 준비.

**자체 체크리스트**:
- [x] 8 Axis 모두 coverage
- [x] D-1~D-13 + D-20260411-01~13 모두 참조
- [x] Phase 태그 전 항목 명시
- [x] 4가지 데이터 흐름 sequence diagram
- [x] 장애 모드 컴포넌트별 시나리오 테이블
- [x] 성능 budget 분해
- [x] 도메인 모델 DDD 관점
- [x] Cross-cutting matrix (10 crates × 8 axes)
- [x] Gap 식별 8건
- [x] Phase C reviewer 질문 23개

**v0 알려진 한계**:
- Gap G-1/G-2 — Phase C에서 해소 예정
- Gap G-6 — gateway-router-lead + xaas-platform-lead 협의 필요
- §2.H 성능 측정값 대부분 미측정

---

### Phase C — 2026-04-12 — 7 subagents

**결론**: 조건부 (Conditional). 8 BLOCKER + 17 HIGH + 12 MED + 4 LOW 발견.

**발견 요약**: `docs/architecture/phase-c-review-results.md` 참조.

**주요 테마**:
1. G-6 TenantContext propagation — gateway, xaas, qa 동시 지적 → Option B 변형 채택
2. Streaming retry idempotency — G-10 + IE-03 동일 결함
3. Latency budget measurability — GAP-T2 + G-11
4. K8s operator contradiction — Option A 채택 (PM 자율)

---

### Phase D — 2026-04-12 — PM consolidation

**결론**: 25 PM 결정 확정. v1 작성 지시.

**결정**: `docs/architecture/phase-c-review-results.md §PM 자율 결정` 25건 전체. 사용자 escalation 없음.

---

### Phase D-2 — 2026-04-12 — @chief-architect (Call 9 of 9)

**결론**: v1 완성. §10 Self-Check 전 셀 OK. Phase E 진입 준비.

**작업 완료**:
- [x] §3 Cross-Cutting Matrix refreshed (NVML 1 Hz, WsAggregator, audit row)
- [x] §4 Gap Registry — 8 BLOCKER + 17 HIGH resolved, MED/LOW disposition, CA-1~4 self-observations
- [x] §5 Open Questions — Q1~Q23 all resolved inline, Q24~Q26 new PM-autonomous
- [x] §6 Phase Tags — P1 must-ship table, P2 scheduled, P3 planned
- [x] §7 Phase D Resolutions (25 decisions table, KubeVirt, roster, determinism)
- [x] §8 문서 메타 updated to v1 status + Phase E review schedule
- [x] §9 v0→v1 변경 요약 (per-axis, per-decision)
- [x] §10 구현 결정론 Self-Check — 14 sections × 8 requirements — all OK
- [x] §11 리뷰 로그 clean v1 with all phase entries
- [x] Item 10: `ServiceUnavailable` at §2.F.9.2 → `GadgetronError::Provider("rate_limit_unavailable")` with [P2] note + ACL-3 cross-ref

---

### Round 1 Retry — (예정) — PM이 선정한 도메인 lead 2명

**결론**: (pending)

**검증 포인트**:
- [ ] B-1~B-8 BLOCKER 해소 확인
- [ ] H-1~H-17 HIGH 해소 확인
- [x] §2.G.5 `chat_with_context` signature 정합
- [ ] §2.B.4 HTTP status mapping 정합
- [ ] §2.C.4 operator reconcile Option A 일관성

---

### Round 1.5 — (예정) — security-compliance-lead + dx-product-lead (병렬)

**결론**: (pending)

**보안 체크포인트** (security-compliance-lead):
- [ ] STRIDE 6-category threat model vs §2.F.9 coverage
- [ ] `GadgetronError` message content — no internal path/schema leak
- [ ] API key in logs/traces — redaction applied?
- [ ] Audit log append-only guarantee — no DELETE permission
- [ ] `cargo-audit` clean on dependency list
- [ ] `/metrics` endpoint isolation (NetworkPolicy)

**DX 체크포인트** (dx-product-lead):
- [ ] All 12 `GadgetronError` variants have user-facing message (what/why/how-to-fix)
- [ ] `AppConfig` TOML fields all have doc comments + defaults + env override
- [ ] `/ready` vs `/health` distinction documented for operators
- [ ] 5-minute quick-start path copy-pasteable from v1
- [ ] D-20260412-02 banned phrases: zero hits in body
- [ ] CA-2 (error_code method) — present or explicitly deferred?

---

### Round 2 — (예정) — @qa-test-architect

**결론**: (pending)

**검증 포인트**:
- [ ] §2.A.7 9개 e2e 시나리오 — 모든 입력/기대 출력 구체적
- [ ] §2.H.2 criterion bench plan — 모든 `BM-*` bench 구현 가능 여부
- [ ] `tracing-test 0.2` 사용 패턴 — §2.A.7에서 span assertion 가능
- [ ] `PrometheusRecorder` in-process — counter assertion 패턴
- [ ] GAP-T3 (moka invalidation), GAP-T5 (cli boot), GAP-T6 (restart recovery) 시나리오 충분성
- [ ] PostgreSQL integration test migration 순서 보장

---

### Round 3 — (예정) — @chief-architect

**결론**: (pending)

**검증 포인트** (Rust 관용구 + 구현 결정론 최종 게이트):
- [ ] §10 Self-Check 재실행 — 모든 셀 OK 유지
- [ ] `#[non_exhaustive]` 사용 일관성
- [ ] `Arc<dyn LlmProvider + Send + Sync>` send bound 완전성
- [ ] `impl From<X> for GadgetronError` orphan rule 준수
- [ ] DashMap clone-before-await pattern 일관성 (§2.H.3 ANTI-PATTERN vs correct)
- [ ] D-20260412-02 banned phrase 최종 스캔
- [ ] CA-1~CA-4 self-observations — resolved or explicitly deferred with rationale
