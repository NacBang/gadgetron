# Gadgetron — 전체 설계 개요

> **버전**: `0.5.12` — Phase 2 EPIC 1 + EPIC 2 + **EPIC 3 모두 CLOSED** (tag `v0.5.0`); **EPIC 4 Multi-tenant XaaS ACTIVE** (2026-04-20 승격, `v1.0.0` target). 요약은 아래 bullet list, full breakdown 은 [`docs/ROADMAP.md`](ROADMAP.md).
>
> **EPIC 1–3 shipping**:
>
> - **EPIC 1** Workbench MVP — CLOSED `v0.3.0` (PR #208). ISSUE 1–4 통합 (workbench projection + 직접 액션 승인 흐름 + dashboard observability).
> - **EPIC 2** Agent autonomy — CLOSED `v0.4.0` (PR #209). ISSUE 5–7 통합 (tool-call audit 영속화 + Penny fan-out + `/v1/tools` 외부 MCP surface).
> - **EPIC 3** Plugin platform — CLOSED `v0.5.0` (PR #228, 2026-04-20). ISSUE 8 hot-reload (5 TASKs, v0.4.1–v0.4.5), ISSUE 9 real bundle manifests (3 TASKs, v0.4.6–v0.4.8), ISSUE 10 bundle marketplace (4 TASKs, v0.4.9–v0.4.12).
>
> **EPIC 4 progress** (toward `v1.0.0`):
>
> - **ISSUE 11** quotas + rate limits ✅ — 4 TASKs across 0.5.1→0.5.4 (PRs #230/#231/#232/#234). Pipeline: rate-limit check → pg cost check → dispatch → pg `record_post` → `/quota/status` tenant introspection → rejections surface as structured 429 + `Retry-After: 60`. `TokenBucketRateLimiter` DashMap-sharded per-tenant (opt-in `[quota_rate_limit]`). `PgQuotaEnforcer` CASE-expression rollover on UTC midnight + first-of-month, fire-and-forget. Bootstrap-gap fallback uses schema defaults (1M daily / 10M monthly).
> - **ISSUE 12** integer-cent billing ✅ closed at telemetry scope (PR #241 / 0.5.6). TASK 12.1 (PR #236 / 0.5.5) append-only `billing_events` ledger + chat INSERT + Management-scoped `/admin/billing/events` query. TASK 12.2 (PR #241 / 0.5.6) widened to `/v1/tools/{name}/invoke` + workbench direct-action + approved-action terminals (kind=tool / kind=action; action rows carry `source_event_id = audit_event_id` for audit↔ledger join). Harness gates 7i.5 + 7h.1c.
> - **ISSUE 13** HuggingFace catalog + per-model cost attribution DEFERRED (commercialization layer, 2026-04-20 scope direction) — post-1.0.
> - **ISSUE 14** tenant self-service ✅ (PR #246 / 0.5.7) — users/teams/key rotation/CLI.
> - **ISSUE 15** cookie-session login API ✅ (PR #248 / 0.5.8) — `POST /auth/login` + `/logout` + `GET /whoami`.
> - **ISSUE 16** unified Bearer-or-cookie auth middleware ✅ (PR #259 / 0.5.9) — `auth_middleware` cookie fallback with role-derived scope synthesis (admin → `[OpenAiCompat, Management]`; member → `[OpenAiCompat]`); covers `/v1/*` + `/api/v1/web/workbench/*` + `/api/v1/xaas/*`.
> - **ISSUE 17** `ValidatedKey.user_id` plumbing ✅ (PR #260 / 0.5.10) — both auth paths carry owning user id for downstream audit/billing attribution without extra DB round-trips.
> - **ISSUE 18** web UI login form (React/Tailwind in `gadgetron-web`) ⏳ planned.
> - **ISSUE 19** `AuditEntry` actor fields structural ✅ (PR #262 / 0.5.11) — `actor_user_id` + `actor_api_key_id` columns added to the struct; 7 call sites default to `None` for now.
> - **ISSUE 20** TenantContext → AuditEntry plumbing ✅ (PR #263 / 0.5.12) — `TenantContext` gains `actor_*` fields populated by `tenant_context_middleware` from `ValidatedKey`; chat handler's 3 `AuditEntry` literals now read ctx fields. Cookie sessions (`Uuid::nil()` sentinel) resolve `actor_api_key_id = None`; Bearer callers get `Some(key_id)`.
> - **ISSUE 21** pg `audit_log` consumer ⏳ planned — background task drains `AuditWriter` mpsc channel and writes rows to `audit_log` using the new actor columns, plus a `GET /admin/audit/log` operator query endpoint.
> - **ISSUE 12.3 / 12.4 / 12.5** (invoice materialization, reconciliation, Stripe ingest) DEFERRED with ISSUE 13 as commercialization layer — post-1.0 patch / minor bumps once market pull justifies.
>
> **Harness**: 129 PASS, 0 FAIL (Gate 7v.6 added in PR #259; PRs #260 + #262 + #263 behavior-preserving, no new gates).
> **EPIC 4 close-for-1.0**: ISSUE 11 + 14 + 15 + 16 + 17 + 18 + 19 + 20 + 21 + core product surfaces (knowledge + Penny + bundle/plug + observability) meeting production bar.
> **EPIC 5** (Cluster platform) PLANNED post-1.0 → `v2.0.0`.
> **이전 tag**: `v0.5.0` (EPIC 3 closure, 2026-04-20), `v0.4.0` (EPIC 2 closure, 2026-04-19), `v0.3.0` (EPIC 1 closure, 2026-04-19), `v0.1.0-phase1` (역사).
> **버저닝 정책**: [`docs/process/06-versioning-policy.md`](process/06-versioning-policy.md)
> **에디션**: 2021
> **라이선스**: MIT
> **최소 Rust**: 1.80
> **바이너리 이름**: `gadgetron`

---

## 1. Gadgetron이란

Gadgetron은 지식 협업 플랫폼이다. 사용자의 로컬 지식과 웹 정보를 **지식 레이어**에 쌓고, Penny 가 그 레이어를 근거로 사용자의 요청을 해결한다. 기능은 **Bundle / Plug / Gadget** 구조로 확장한다.

### 1.1 작동 흐름

```
  사용자 요청
       │
       ▼
  ┌─────── Penny ───────┐
  │                     │
  │  1. 지식 레이어 조회   │
  │  2. 필요하면 웹 조사   │
  │  3. 결과 종합해 응답   │
  │  4. 유의미한 사실은    │──► 지식 레이어에 기록
  │     레이어에 기록     │
  └─────────────────────┘
```

동일한 질문에는 레이어에 이미 정리된 내용이 먼저 쓰인다.

### 1.2 Penny

Penny (Penny Brown) 는 사용자와 대화하며 Gadgetron 의 지식 레이어와 Gadgets 를 도구로 삼아 요청을 해결한다.

- **기본 런타임**: Claude Code CLI + Claude Opus. OAuth (`claude_max`) 또는 Anthropic API 키.
- **교체 가능**: 사용자 설정으로 다른 클라우드 모델 (OpenAI / Gemini 등) 또는 로컬 모델 (vLLM / SGLang / Ollama)로 바꿀 수 있다.
- **노출 방식**: OpenAI 호환 엔드포인트의 `model = "penny"`, Web UI (`/web`)에서 대화.
- 상세: [`docs/design/phase2/02-penny-agent.md`](design/phase2/02-penny-agent.md).

### 1.3 지식 레이어

Gadgetron 지식 레이어는 여러 구성요소로 이루어진다:

- **LLM Wiki** — Markdown + Obsidian 스타일 `[[link]]` + git 버전 관리 (모든 쓰기 = 자동 커밋). 사람이 읽을 수 있고 Penny 가 다룰 수 있는 구조.
- **웹 조사** — SearXNG 프록시 또는 선택 외부 검색. 조사 결과는 필요시 LLM Wiki 페이지로 정리되어 축적된다.
- **RAW 지식 입수 파이프라인** — 사용자가 특정 폴더(예: `knowledge/inbox/`)에 PDF · 텍스트 · 회의록을 드롭하면 백그라운드로 LLM Wiki 페이지를 생성. 원본은 보존.
- **검색 인덱스** — P2A는 인메모리 역인덱스, P2B+는 SQLite + `sqlite-vec` 벡터 / `tantivy` 전문검색.

쓰기 안전 장치 (PEM / AWS / GCP 자격증명 차단, 페이지 크기 상한, 경로 탈출 방지)는 `M1–M8` 위협 모델로 문서화되어 있다.

- 상세: [`docs/design/phase2/01-knowledge-layer.md`](design/phase2/01-knowledge-layer.md), [`docs/design/phase2/05-knowledge-semantic.md`](design/phase2/05-knowledge-semantic.md).

### 1.4 Bundle / Plug / Gadget

Gadgetron의 canonical extension vocabulary 는 **Bundle / Plug / Gadget** 다. Bundle 은 배포 단위, Plug 는 core 가 소비하는 Rust 구현, Gadget 은 Penny 가 호출하는 MCP tool 이다. 용어의 최종 기준은 [`ADR-P2A-10`](adr/ADR-P2A-10-bundle-plug-gadget-terminology.md) 이다.

Penny가 보는 도구 surface 는 MCP 로 노출되며, 새 Gadget 을 추가하려면 `GadgetProvider` 를 구현해 registry 에 등록하면 된다. Plug 는 Penny가 직접 보지 않고 core/service layer 가 소비한다.

| Bundle | 상태 | 제공 Gadgets / 역할 |
|---|---|---|
| Knowledge | P2A 출시 | `wiki.list` / `wiki.get` / `wiki.search` / `wiki.write` / `web.search` |
| 서버 운영 | 설계 중 | 서버 상태 조회, 로그 열람, 설정 변경, 위험 경고, 재시작 |
| 클러스터 관리 | 후속 | K8s / Slurm 잡 제출 · 스케일링 · drain |
| 작업 관리 | 후속 | 큐 / 스케줄 / SLA 추적 |

- 레지스트리 설계: [`docs/design/phase2/04-mcp-tool-registry.md`](design/phase2/04-mcp-tool-registry.md) *(legacy filename; Gadget registry spec)*
- 서버 운영 Bundle 설계: [`docs/design/ops/operations-tool-providers.md`](design/ops/operations-tool-providers.md)

### 1.5 구현 형태

- 기본 배포 단위는 **단일 Rust 바이너리** (`gadgetron serve`). 필요에 따라 프로세스 단위로 분리할 수 있다.
- 기본 설정은 **단일 TOML 파일**. 필요에 따라 분할한다.
- 스토리지는 **로컬 데스크톱 / 온프레미스 / 클라우드 (S3 · GCS 등)** 를 같은 코드 경로로 지원. 배포 형태는 §5 로드맵 참조.
- 게이트웨이 본체 오버헤드는 P99 서브밀리초 목표 (§7.1). GPU 자원(NVML · VRAM · NUMA · MIG)은 스케줄러가 직접 관리 (§7.2).
- Claude Code / assistant-ui / SearXNG / git2 / sqlite-vec 등 오픈소스 조각을 조립하고, 직접 구현은 최소화한다.

---

## 2. 아키텍처 다이어그램

### 2.1 크레이트 의존성 및 데이터 흐름

중요: 아래 다이어그램은 **현재 trunk crate layout** 을 설명한다. 구현자가 따라야 할 **canonical ownership** 은 `docs/process/04-decision-log.md` D-20260418-01 이며, 그 기준에서 `gateway` 는 core, `router/provider/scheduler` 는 `ai-infra` Bundle ownership, `node` 는 `server/gpu/ai-infra` Bundle split 대상이다.

```
                              +------------------+
                              |  gadgetron-cli   |
                              |  (bin: gadgetron)|
                              +--------+---------+
                                       |
          +------------+-------+-------+-------+---------+-----------+
          |            |       |               |         |           |
          v            v       v               v         v           v
  +------------+ +----------+ +----------+ +----------+ +--------+ +-----+
  |    core    | | provider | |  router  | | gateway  | |scheduler| | tui |
  +------------+ +----+-----+ +-----+----+ +-----+----+ +---+----+ +--+--+
       ^              |             |            |          |          |
       |              |             |            |          |          |
       |              v             v            |          v          v
       +----------<- core <----- provider        |     +-------+  +---+
                   ^   ^         ^    ^          |     | node  |  |core|
                   |   |         |    |          |     +---+---+  |router|
                   |   |         |    |          |         ^      |sched|
                   |   |         |    |          +---------+      |node |
                   |   +---------+    +----------+-+------+       +-----+
                   |                               |
                   +-------------------------------+

  실제 의존성 (Phase 1 기준, Cargo.toml):
    core        <- (외부 crate만)
    provider    <- core
    router      <- core, provider
    gateway     <- core, provider, router, scheduler, node
    scheduler   <- core, node
    node        <- core
    tui         <- core, router, scheduler, node
    xaas        <- core                                    (Phase 1 XaaS: auth/quota/audit)
    testing     <- core, provider, gateway, xaas           (Phase 1 E2E harness)
    cli         <- core, provider, router, gateway, scheduler, node, tui, xaas

  Phase 2 신규 (trunk 구현됨):
    knowledge   <- core                                    (Phase 2: wiki + search + stdio MCP server)
    penny      <- core, knowledge                         (Phase 2: LlmProvider impl → router 등록)
    web         <- (embedded static UI crate, gateway가 feature `web-ui`로 mount)
    cli         <- +knowledge, penny                      (`gadget serve` 서브커맨드 + penny/web wiring; `mcp serve` 는 deprecated alias per ADR-P2A-10)
```

### 2.2 런타임 컴포넌트 다이어그램

```
+=========================================================================+
|                        Gadgetron -- 단일 바이너리                        |
|                                                                         |
|  +-------------------------------------------------------------------+  |
|  |                    gadgetron-gateway (HTTP)                        |  |
|  |  +-------------+  +------------+  +-----------+  +-------------+  |  |
|  |  |/v1/chat     |  |/v1/models  |  |/api/v1/   |  |  SSE Stream  |  |  |
|  |  | completions |  |             |  |  nodes    |  |  (zero-copy) |  |  |
|  |  +-------------+  +------------+  |  deploy    |  +-------------+  |  |
|  |                                   |  status    |                   |  |
|  |  +-------------+  +------------+  +-----------+                   |  |
|  |  |  /health    |  |/api/v1/    |  +-----------+                   |  |
|  |  |             |  |  usage     |  |/api/v1/   |                   |  |
|  |  +-------------+  +------------+  |  costs     |                   |  |
|  |                                   +-----------+                   |  |
|  |  GatewayServer::new(addr).run()                                   |  |
|  |  Routes: axum::Router + CorsLayer + TraceLayer                    |  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                       gadgetron-router                             |  |
|  |  +--------------+ +--------------+ +-------------+ +------------+  |  |
|  |  | RoundRobin   | | CostOptimal  | | LatencyOpt  | | QualityOpt |  |  |
|  |  +--------------+ +--------------+ +-------------+ +------------+  |  |
|  |  +--------------+ +----------------------------------------------+ |  |
|  |  | Fallback     | | Weighted { weights: HashMap<String, f32> }  | |  |
|  |  +--------------+ +----------------------------------------------+ |  |
|  |                                                                   |  |
|  |  Router::resolve(&ChatRequest) -> RoutingDecision                 |  |
|  |  Router::chat(ChatRequest) -> ChatResponse (with fallback)        |  |
|  |  Router::chat_stream(ChatRequest) -> Pin<Box<Stream<ChatChunk>>>  |  |
|  |                                                                   |  |
|  |  +---------------------------------------------------------------+|  |
|  |  |           MetricsStore (DashMap<(String,String), ProviderMetrics>)|  |
|  |  |  record_success() / record_error() / get() / all_metrics()     ||  |
|  |  +---------------------------------------------------------------+|  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                     gadgetron-provider                             |  |
|  |  +----------+ +-----------+ +--------+ +--------+ +-------------+  |  |
|  |  | OpenAI   | | Anthropic | | Gemini | | Ollama | | vLLM/SGLang |  |  |
|  |  | Provider | | Provider  | | (TBD)  | |Provider| |  Providers  |  |  |
|  |  +----------+ +-----------+ +--------+ +--------+ +-------------+  |  |
|  |                                                                   |  |
|  |  LlmProvider trait:                                               |  |
|  |    async fn chat(&self, ChatRequest) -> Result<ChatResponse>      |  |
|  |    fn chat_stream(&self, ChatRequest) -> Pin<Box<dyn Stream>>     |  |
|  |    async fn models(&self) -> Result<Vec<ModelInfo>>               |  |
|  |    fn name(&self) -> &str                                         |  |
|  |    async fn health(&self) -> Result<()>                           |  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                     gadgetron-scheduler                            |  |
|  |  +------------------+  +--------------------+  +----------------+  |  |
|  |  | VRAM 기반        |  | 노드 등록/갱신     |  | 모델 배포/해제  |  |  |
|  |  | 스케줄링         |  | register_node()    |  | deploy()       |  |  |
|  |  |                  |  | update_node()      |  | undeploy()     |  |  |
|  |  +------------------+  +--------------------+  +----------------+  |  |
|  |  +------------------+                                              |  |
|  |  | LRU Eviction     |  Scheduler:                                      |  |
|  |  | find_eviction_   |    deployments: RwLock<HashMap<String, ModelDeployment>>
|  |  |   candidate()    |    nodes: RwLock<HashMap<String, NodeStatus>>     |  |
|  |  +------------------+                                              |  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                       gadgetron-node                               |  |
|  |  +----------------------+  +------------------------------------+  |  |
|  |  | NodeAgent            |  | ResourceMonitor                    |  |  |
|  |  |  start_model()       |  |   collect() -> NodeResources      |  |  |
|  |  |  stop_model()        |  |   +------+ +-----+ +------+ +---+ |  |  |
|  |  |  status()            |  |   | CPU  | | RAM | | GPU  | |온도||  |  |
|  |  |  collect_metrics()   |  |   +------+ +-----+ +------+ +---+ |  |  |
|  |  +----------------------+  |   sysinfo    NVML (feature gate)  |  |  |
|  +------------------------------------------------------------------+  |
|                                                                         |
|  +------------------------------------------------------------------+  |
|  |                        gadgetron-core                             |  |
|  |  config | error | message | model | node | provider | routing    |  |
|  +------------------------------------------------------------------+  |
|                                                                         |
|  +---------------------------+  +----------------------------------+  |
|  |     gadgetron-tui         |  |       gadgetron-cli              |  |
|  |  App::run()               |  |  main.rs -- 진입점               |  |
|  |  ratatui 대시보드         |  |  AppConfig::load()               |  |
|  |  Nodes | Models | Requests|  |  전체 크레이트 통합 부팅         |  |
|  +---------------------------+  +----------------------------------+  |
|                                                                         |
+=========================================================================+

           ^ 외부 API               ^ K8s/Slurm             ^ 모니터링
    +------+-------+        +-------+--------+     +-------+---------+
    |    Client    |        | 컨테이너 오케스트레이터|   |  Prometheus/   |
    |    (SDK)     |        | K8s/Slurm/Docker |   |  Grafana       |
    +-------------+        +------------------+   +----------------+
```

---

## 3. 크레이트별 요약

각 크레이트의 **상세 스펙**은 lead 가 소유하는 `docs/modules/*.md`에 있고,
**cross-cutting 관점**은 `docs/architecture/platform-architecture.md` Axis B에 통합되어 있습니다.
본 섹션은 크레이트 역할과 상세 SSOT 링크만 제공합니다 — 공개 API 시그니처를 여기서 반복하면 드리프트가 쌓입니다.

중요: 아래 표는 **현재 workspace crate layout** 을 설명한다. 구현자가 따라야 할 **canonical ownership** 은 `docs/process/04-decision-log.md` D-20260418-01 이며, 그 기준에서 `gateway` 는 core, `router/provider/scheduler` 는 `ai-infra` Bundle ownership, `node` 는 `server/gpu/ai-infra` Bundle split 대상이다.

| 크레이트 | 역할 한 줄 | 상세 SSOT |
|---|---|---|
| `gadgetron-core` | 공통 타입·트레이트·에러·설정 (`LlmProvider`, `GadgetronError`, `AppConfig`, `ChatRequest`, `ModelState`, `NodeResources` …) | [`docs/design/core/types-consolidation.md`](design/core/types-consolidation.md) |
| `gadgetron-provider` | 현재 top-level crate. Canonical ownership 은 `ai-infra` Bundle 의 provider Plug 집합 | [`docs/modules/model-serving.md`](modules/model-serving.md) §2–3 |
| `gadgetron-router` | 현재 top-level crate. Canonical ownership 은 `ai-infra` Bundle 의 routing service | [`docs/modules/gateway-routing.md`](modules/gateway-routing.md) §4 |
| `gadgetron-gateway` | axum 기반 OpenAI 호환 HTTP 서버 + `/api/v1/*` 관리 엔드포인트 + SSE 제로카피 변환 | [`docs/modules/gateway-routing.md`](modules/gateway-routing.md) §2 · [`docs/manual/api-reference.md`](manual/api-reference.md) |
| `gadgetron-scheduler` | 현재 top-level crate. Canonical ownership 은 `ai-infra` Bundle 의 scheduling service | [`docs/modules/gpu-resource-manager.md`](modules/gpu-resource-manager.md) §6 · [`docs/modules/model-serving.md`](modules/model-serving.md) §5 |
| `gadgetron-node` | 현재 top-level crate. Canonical ownership 은 `server/gpu/ai-infra` Bundle split 대상 | [`docs/modules/gpu-resource-manager.md`](modules/gpu-resource-manager.md) §1 · [`docs/modules/deployment-operations.md`](modules/deployment-operations.md) |
| `gadgetron-xaas` | Multi-tenancy / auth / quota / audit — PostgreSQL + sqlx 런타임 쿼리 | [`docs/modules/xaas-platform.md`](modules/xaas-platform.md) · [`docs/design/xaas/phase1.md`](design/xaas/phase1.md) |
| `gadgetron-testing` | Mock / fake 프로바이더, fake GPU 노드, E2E 하네스 (PostgreSQL 필요) | [`docs/design/testing/harness.md`](design/testing/harness.md) |
| `gadgetron-tui` | ratatui 기반 3x2 터미널 대시보드 (`gadgetron serve --tui`) | [`docs/manual/tui.md`](manual/tui.md) · [`docs/ui-ux/dashboard.md`](ui-ux/dashboard.md) |
| `gadgetron-cli` | 단일 바이너리 entry point (`gadgetron` 바이너리) + `serve` / `init` / `doctor` / `tenant` / `key` / `gadget serve` 서브커맨드 (`mcp serve` 는 deprecated alias — ADR-P2A-10, v0.5.x 에서 warn-log 후 dispatch) | [`docs/manual/quickstart.md`](manual/quickstart.md) · [`docs/manual/configuration.md`](manual/configuration.md) |
| `gadgetron-knowledge` *(P2A)* | wiki (md + git2 + Obsidian `[[link]]`) + SearXNG 프록시 + `KnowledgeGadgetProvider` | [`docs/design/phase2/01-knowledge-layer.md`](design/phase2/01-knowledge-layer.md) |
| `gadgetron-penny` *(P2A)* | Claude Code subprocess 브릿지 + `GadgetRegistry` + `LlmProvider` 구현 (model = `penny`) | [`docs/design/phase2/02-penny-agent.md`](design/phase2/02-penny-agent.md) · [`docs/design/phase2/04-mcp-tool-registry.md`](design/phase2/04-mcp-tool-registry.md) |
| `gadgetron-web` *(P2A)* | assistant-ui 기반 embedded Web UI (`/web`) + approval/report UX substrate | [`docs/design/phase2/03-gadgetron-web.md`](design/phase2/03-gadgetron-web.md) · [`docs/manual/web.md`](manual/web.md) |

크레이트 의존성 그래프는 §2.1에, Cargo.toml 버전과 Feature Gate 상세는 §6에 있습니다. `LlmProvider` 트레이트가 모든 프로바이더 다형성의 기반이며 시그니처는 §7.4에 재언급됩니다.

---

## 4. 데이터 흐름: ChatRequest -> Router -> Provider -> Response

### 4.1 비스트리밍 요청 흐름

```
  Client
    |
    |  POST /v1/chat/completions
    |  { model: "gpt-4", messages: [...], temperature: 0.7 }
    v
+----------------------------------------------+
|  gadgetron-gateway                           |
|  GatewayServer.run()                         |
|  1. axum Router가 요청 수신                   |
|  2. ChatRequest 역직렬화 (serde_json)        |
|  3. API Key 검증 (ServerConfig.api_key)      |
|  4. 타임아웃 설정 (request_timeout_ms)        |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-router                            |
|  Router::resolve(&ChatRequest)               |
|  4. RoutingConfig.default_strategy 확인      |
|  5. 프로바이더 목록에서 전략 적용             |
|     - RoundRobin: AtomicUsize 순환            |
|     - CostOptimal: cheapest_provider()        |
|     - LatencyOptimal: fastest_provider()      |
|     - QualityOptimal: prefer_provider()       |
|     - Fallback: chain 순차 검색              |
|     - Weighted: weighted_provider()           |
|  6. RoutingDecision 생성                      |
|     { provider, model, strategy,             |
|       estimated_cost_usd, fallback_chain }   |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-provider                          |
|  LlmProvider::chat(ChatRequest)              |
|  7. 선택된 프로바이더 호출                    |
|     - OpenAI: Bearer 토큰, /v1/chat/completions
|     - Anthropic: x-api-key, /v1/messages     |
|     - Ollama: 인증 없음, /v1/chat/completions|
|     - vLLM: Bearer (선택), /v1/chat/completions
|     - SGLang: Bearer (선택), /v1/chat/completions
|  8. HTTP 응답 -> ChatResponse 역직렬화       |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-router (사후 처리)                 |
|  9. 성공 시:                                 |
|     MetricsStore::record_success(             |
|       provider, model, latency_ms,           |
|       prompt_tokens, completion_tokens, cost) |
|  10. 실패 시:                                |
|     MetricsStore::record_error(               |
|       provider, model, latency_ms, error)     |
|     try_fallbacks() -> fallback_chain 순차 시도|
|     모두 실패 -> AllProvidersFailed 에러      |
+----------------------+-----------------------+
                       |
                       v
  Client <- ChatResponse {
    id, object: "chat.completion",
    model, choices: [Choice { message, finish_reason }],
    usage: Usage { prompt_tokens, completion_tokens, total_tokens }
  }
```

### 4.2 스트리밍 요청 흐름

```
  Client
    |
    |  POST /v1/chat/completions  (stream: true)
    v
+----------------------------------------------+
|  gadgetron-gateway                           |
|  SSE 스트리밍 파이프라인:                    |
|                                              |
|  LlmProvider::chat_stream(req)               |
|       -> Pin<Box<dyn Stream<Item=Result<ChatChunk>>>>  |
|       -> chat_chunk_to_sse()                 |
|       -> Sse<KeepAlive<15s>>                 |
|       -> axum response body                  |
|                                              |
|  제로카피 변환:                              |
|  ChatChunk -> serde_json::to_string()        |
|            -> Event::default().data(json)    |
|            -> SSE frame ("data: ...\n\n")    |
|                                              |
|  프로바이더별 SSE 처리:                      |
|  - OpenAI/vLLM/SGLang/Ollama:                |
|    "data: {json}\n\n" ... "data: [DONE]\n\n" |
|  - Anthropic:                                |
|    "data: {type:content_block_delta}\n\n"    |
|    "data: {type:message_stop}\n\n"           |
|    -> ChatChunk으로 정규화 후 재변환         |
+----------------------------------------------+
```

### 4.3 로컬 모델 배포 흐름

```
  POST /api/v1/models/deploy
    { model_id, engine, vram_requirement_mb }
    |
    v
+----------------------------------------------+
|  gadgetron-scheduler                         |
|  Scheduler::deploy(model_id, engine, vram_mb)|
|                                              |
|  1. 기존 배포 확인 (멱등성)                  |
|     if ModelState::Running -> return Ok(())  |
|                                              |
|  2. 가용 노드 탐색                           |
|     nodes.values()                           |
|       .filter(|n| n.healthy)                 |
|       .find(|n| n.resources.available_vram_mb() >= vram_mb)
|                                              |
|  3. 미충족 시 find_eviction_candidate()      |
|     -> LRU 기반 모델 해제 후 재시도          |
|                                              |
|  4. ModelDeployment 생성                     |
|     { status: Loading, assigned_node, ... }  |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-node                              |
|  NodeAgent::start_model(&deployment)         |
|                                              |
|  5. InferenceEngine별 프로세스 시작           |
|     - Ollama: POST /api/generate (keep_alive)|
|     - Vllm:  tokio::process::Command         |
|     - SGLang: tokio::process::Command        |
|     - LlamaCpp: tokio::process::Command      |
|     - TGI: tokio::process::Command           |
|                                              |
|  6. 포트 할당 및 running_models 갱신         |
|  7. ModelState::Running { port } 전환        |
+----------------------------------------------+
```

### 4.4 VRAM 추정 공식

```rust
// model.rs
pub fn estimate_vram_mb(params_billion: f64, quantization: Quantization) -> u64 {
    let gb_per_billion = match quantization {
        Quantization::Fp16    => 2.0,
        Quantization::Fp8     => 1.0,
        Quantization::Q8_0    => 1.1,
        Quantization::Q5_K_M  => 0.7,
        Quantization::Q4_K_M  => 0.6,
        Quantization::Q3_K_M  => 0.45,
        Quantization::GgufAuto => 0.6,
    };
    ((params_billion * gb_per_billion * 1024.0) + 1024.0) as u64
}
```

예시: Llama-3-70B Q4_K_M -> `70 * 0.6 * 1024 + 1024` = 44,032 MB

---

## 5. 로드맵 — Foundation (Phase 1) / Assistant & Collaboration (Phase 2) / Cluster Ops Expansion (Phase 3)

### Phase 1 — Operations + Execution Substrate (v0.1.0-phase1, **완료**)

**목표**: 단일 노드에서 OpenAI 호환 게이트웨이로 다중 프로바이더를 라우팅하고, 기본 XaaS (auth/quota/audit) 제공.

**크레이트 (10개, 모두 구현 완료)**:
- `gadgetron-core` — 공통 타입·트레이트·에러 (12 GadgetronError variants)
- `gadgetron-provider` — 6 provider adapter (OpenAI/Anthropic/Gemini/Ollama/vLLM/SGLang)
- `gadgetron-router` — 6 routing strategy + MetricsStore
- `gadgetron-gateway` — axum HTTP server + middleware chain + SSE
- `gadgetron-scheduler` — VRAM-aware + LRU/Priority eviction + bin packing
- `gadgetron-node` — NodeAgent (process mgmt) + ResourceMonitor
- `gadgetron-tui` — Ratatui 대시보드 (`serve --tui`)
- `gadgetron-cli` — 단일 바이너리 entry point + `doctor`/`init`/`tenant`/`key` 서브커맨드
- `gadgetron-xaas` — auth + tenant + quota + audit (PostgreSQL + sqlx runtime queries)
- `gadgetron-testing` — mock providers + fake GPU node + E2E harness

**완료된 기능**:
- OpenAI 호환 `/v1/chat/completions` (stream + non-stream)
- 6 provider / 6 routing strategy / Bearer auth (moka cached) / in-memory quota
- Audit broadcast channel + optional PostgreSQL 영속화 + no-db 모드
- TUI 실시간 대시보드 / graceful shutdown / `gadgetron doctor` 사전 점검
- Phase 1 v0.1.0 manual QA Tests 1-9 전부 통과 (P99 bench: 65ns resolve, 50µs middleware chain, 20-15000× SLO 여유)
- Hotfix PRs #7-10: streaming audit latency Phase 1 semantics 명확화, 401 `invalid_api_key` + 413 `request_too_large` OpenAI-compat, `--tui` TTY pre-check, 매뉴얼 동기화

**Phase 1 알려진 제약 (Phase 2 진행 상황)**:
- ~~Streaming audit `latency_ms=0` (dispatch-time only)~~ — **해결 (P2B / ISSUE 4 PR #194)**. `StreamEndGuard` Drop 경로에서 amendment `AuditEntry` 를 보내 실제 `latency_ms` + `output_tokens` + `cost_cents` 를 기록 (`crates/gadgetron-gateway/src/stream_end_guard.rs:241-285`). 남은 한계: streaming 의 `input_tokens` 는 여전히 `0` (provider 가 delta stream 에 prompt token 을 재보고하지 않음 — router upstream rework 필요한 별도 ISSUE).
- Audit `provider=None` (실제 라우팅된 provider 미기록) — 여전히 유효. `RoutingDecision` 을 handler 까지 surface 하는 rework 이 별도 ISSUE 로 남아 있음.

### Phase 2 — Knowledge Layer + Penny + Bundle Surfaces + Multi-tenant XaaS (v0.2 → **v0.5.12**, EPIC 1 + EPIC 2 + EPIC 3 CLOSED; EPIC 4 **ACTIVE** — ISSUE 11 + ISSUE 12 (telemetry) + ISSUE 14 (self-service) + ISSUE 15 (cookie login) + ISSUE 16 (unified auth middleware) + ISSUE 17 (`ValidatedKey.user_id` plumbing) + ISSUE 19 (`AuditEntry` actor fields structural) + ISSUE 20 (TenantContext → AuditEntry plumbing) complete; 12.3 / 12.4 / 12.5 + 13 cost-attribution DEFERRED per 2026-04-20 commercialization-layer direction; ISSUE 13 discovery/pinning + ISSUE 18 web UI login form + ISSUE 21 pg audit_log consumer remain)

**목표**: Phase 1 substrate 위에 지식 레이어(LLM Wiki + 웹 조사 + RAW 입수 + 검색 인덱스), Penny, 그리고 Bundle/Plug/Gadget 확장 surface를 올려 지식 협업 플랫폼을 구성.

**진행 상태 (2026-04-19)**:
- EPIC 1 (Workbench MVP) — **CLOSED**, tag `v0.3.0`, PR #208. ISSUE 1–4 통합 (workbench projection + 직접 액션 승인 흐름 + dashboard observability).
- EPIC 2 (Agent autonomy) — **CLOSED**, tag `v0.4.0`, PR #209. ISSUE 5–7 통합 (tool-call audit 영속화 + Penny fan-out + `/v1/tools` 외부 MCP surface).
- EPIC 3 (Plugin platform) — **CLOSED (v0.5.0, PR #228, 2026-04-20)**. **ISSUE 8 (DescriptorCatalog hot-reload) 기능적 완료**: TASK 8.1 ✅ (PR #211 `Arc<ArcSwap<DescriptorCatalog>>` 플러밍 substrate, 0.4.1), TASK 8.2 ✅ (PR #213 `POST /admin/reload-catalog` Management-scoped HTTP endpoint, 0.4.2), TASK 8.3 ✅ (PR #214 `CatalogSnapshot { catalog, validators }` bundling — 핸들이 `Arc<ArcSwap<CatalogSnapshot>>` 로 확장되어 reload 가 catalog + pre-compiled JSON-schema validators 를 lockstep 으로 교체, TASK 8.2 validator-rebuild 제한 해소, 0.4.3), TASK 8.4 ✅ (PR #216 file-based catalog source — `[web] catalog_path` 새 설정 키로 operator 가 TOML 파일을 지정, `DescriptorCatalog::from_toml_file()` 로 매 reload 마다 파싱 + atomic swap; 파싱 실패는 500 으로 surface 되고 running snapshot 은 교체되지 않아 잘못된 edit 이 workbench 를 다운시키지 못함; response 에 `source: "config_file"` + `source_path` 필드 추가, 0.4.4), TASK 8.5 ✅ (PR #217 POSIX `SIGHUP` reloader — `spawn_sighup_reloader()` Unix-only tokio task 가 `SignalKind::hangup()` 리스너를 설치하고 매 signal 마다 `perform_catalog_reload()` 공유 helper 호출; HTTP path 와 동일한 `ReloadCatalogResponse` + tracing telemetry; operator workflow `kill -HUP <pid>`; fs-watcher 는 TASK 8.6 로 deferred — demand 에 따라 follow-up, 0.4.5). **ISSUE 9 (real bundle manifests) 도 기능적 완료**: TASK 9.1 ✅ (PR #219 bundle metadata + first-party bundle file — `BundleMetadata { id, version }` 구조체가 `DescriptorCatalog` 에 optional `[bundle]` TOML 테이블로 연결됨; reload response 가 `bundle: Option<BundleMetadata>` 필드로 확장 (None 시 `skip_serializing_if` 로 JSON 에서 생략); first-party bundle file `bundles/gadgetron-core/bundle.toml` 이 `seed_p2b()` 과 동일한 catalog 를 TOML 로 표현하고, drift-test 가 두 source 간 action id 집합 동일성을 보장, 0.4.6), TASK 9.2 ✅ (PR #220 multi-bundle aggregation — `[web] bundles_dir` 새 설정 키 + `DescriptorCatalog::from_bundle_dir()` 가 subdirectory 별 `<name>/bundle.toml` 을 스캔해 하나의 catalog 로 merge, `bundle.toml` 이 없는 subdir 은 silently skip, 정렬 순서는 알파벳 path 기준 — reload 가 idempotent; 중복 action OR view id 는 hard `Config` error 로 reject 하며 id + 두 bundle id 를 메시지에 포함, running snapshot 은 교체되지 않음 — TASK 8.4 parse-failure 동일 보장; `allow_direct_actions` 는 bundle 간 OR-fold; reload response 가 `bundles: Vec<BundleMetadata>` 필드로 확장 (`skip_serializing_if = "Vec::is_empty"` 으로 single-bundle / seed_p2b 시 생략); precedence `bundles_dir` > `catalog_path` > `seed_p2b` fallback; 3 new unit tests, 0.4.7), TASK 9.3 ✅ (PR #222 bundle-driven harness default — E2E harness 의 fixture template `scripts/e2e-harness/fixtures/gadgetron-test.toml.tmpl` 이 `[web] bundles_dir = "@BUNDLES_DIR@"` 를 포함하도록 변경, `run.sh` 가 `@BUNDLES_DIR@` 를 repo-root 의 `bundles/` 절대 경로로 치환; Gate 7q.1 이 `source == "bundles_dir"` 으로 retarget 되어 bundle loader 가 end-to-end 로 연결되었음을 입증; 새 Gate 7q.3 이 `.bundles[0].id == "gadgetron-core"` 를 pin 해서 first-party bundle rename 이나 `bundles` field serde regression 을 잡음 — harness PASS 83 → 84; `source` enum 이 이제 `{"seed_p2b", "config_file", "bundles_dir"}` 세 값; `seed_p2b()` 는 unit-test fixture 와 drift-guard reference 로 유지되지만 production 기본 source 는 아님, 0.4.8). **ISSUE 10 (bundle marketplace) 도 기능적 완료**: TASK 10.1 ✅ (PR #223 bundle-discovery endpoint — `GET /api/v1/web/workbench/admin/bundles` Management-scoped, read-only 방식으로 `bundles_dir` subdirectory 를 스캔해 각 bundle 의 metadata + action/view count + absolute manifest path 를 enumerate; 순서는 `from_bundle_dir` 과 동일하게 subdir path 기준 정렬되어 tooling 이 deterministic 하게 의존할 수 있음; `bundles_dir` 가 wire 되지 않은 deployment 는 503 `Config` 로 fail; live catalog 의 `ArcSwap` 을 건드리지 않으므로 in-flight 요청 중에도 safe 하게 polling 가능; Gate 7q.4 shape + `gadgetron-core` enumeration + Gate 7q.5 RBAC 403; harness PASS 84 → 86, 0.4.9), TASK 10.2 ✅ (PR #224 install + uninstall runtime — `POST /api/v1/web/workbench/admin/bundles` 이 `{bundle_toml: "<text>"}` body 를 받아서 `[bundle]` 테이블 id 를 `^[a-zA-Z0-9_-]{1,64}$` regex 로 validate 후 `{bundles_dir}/{id}/bundle.toml` 에 기록하고 `{installed, bundle_id, manifest_path, reload_hint}` 반환; `DELETE /api/v1/web/workbench/admin/bundles/{bundle_id}` 가 같은 id guard 로 directory 를 `remove_dir_all`; install 은 disk 에 쓰기만 하고 live catalog 는 swap 하지 않음 — operator 가 별도로 `POST /admin/reload-catalog` 또는 SIGHUP 으로 activate; collision 은 4xx `Config` 로 reject (no silent overwrites, `from_bundle_dir` 중복-id hard-error 와 동일); `validate_bundle_id()` helper 가 path-traversal 을 disk write 이전에 차단; Gates 7q.6 (install → discovery), 7q.7 (`id = "../etc/passwd"` → 4xx), 7q.8 (uninstall → discovery); harness PASS 86 → 91, 0.4.10), TASK 10.3 ✅ (PR #226 per-bundle scope isolation — `BundleMetadata.required_scope: Option<Scope>` 새 serde-optional TOML field 가 `[bundle]` level 에 선언되면 `DescriptorCatalog::from_toml_file` post-parse pass 가 모든 view + action 을 walk 하면서 자기 scope 가 없는 descriptor 에 bundle scope 를 inherit; narrower explicit scope 는 유지 (narrower wins); bundle 이 scope 를 선언하지 않으면 zero-overhead (default None, inheritance pass 생략); `from_bundle_dir` 도 per-manifest `from_toml_file` 을 호출하므로 동일 behavior; effective scope 는 descriptor 자체에 land 되어 downstream audit/log 가 bundle 을 re-read 하지 않고도 introspect 가능; unit test 가 양방향 (OpenAiCompat actor 가 Management-floored action 을 못 봄 + Management actor 는 봄) pin; 새 harness gate 없음 — reload + discovery 기존 gate 가 scope inheritance 와 무관하게 계속 hold, harness PASS 그대로 91, 0.4.11). TASK 10.4 ✅ (PR #227 / 0.4.12 — Ed25519-signed manifests, `[web.bundle_signing]` 설정 + `signature_hex` request field + `verify_bundle_signature` TOML parse 이전 실행, 6-branch policy matrix). EPIC 3 CLOSED (PR #228, 2026-04-20, `0.4.12 → 0.5.0`, git tag `v0.5.0`).
- EPIC 4 (Multi-tenant business ops / XaaS) — **ACTIVE (2026-04-20 승격, `v1.0.0` target)**. **ISSUE 11 (quotas + rate limits) 완료** (PRs #230/#231/#232/#234 across 0.5.1→0.5.4): TASK 11.1 ✅ (구조화된 429 + `Retry-After: 60` 헤더 + `retry_after_seconds: 60` body 필드, SDK 가 backoff 를 deterministic 하게), TASK 11.2 ✅ (`TokenBucketRateLimiter` per-tenant 버킷 + `RateLimitedQuotaEnforcer` composite, opt-in `[quota_rate_limit]`, rate 체크가 cost 체크보다 먼저 실행 fail-fast), TASK 11.3 ✅ (`PgQuotaEnforcer` 가 `quota_configs` 에 CASE-expression rollover UPDATE 로 daily + monthly 지출 추적, background job 없이 첫 post-boundary 요청에서 rollover, fire-and-forget), TASK 11.4 ✅ (`GET /api/v1/web/workbench/quota/status` OpenAiCompat tenant 내부 조회 엔드포인트, `{usage_day, daily, monthly}` 각 `{used_cents, limit_cents, remaining_cents}`, SQL 이 CASE rollover 를 SELECT 에 inline 으로 project, `quota_configs` row 없을 시 schema defaults fallback). Full pipeline: rate-limit check → pg cost check → dispatch → pg record_post (quota_configs counter + billing_events ledger INSERT) → tenant 내부 조회 via /quota/status → rejection 은 구조화된 429. **ISSUE 12 (integer-cent billing) telemetry scope 에서 close (PR #241 / 0.5.6)** — TASK 12.1 ✅ (PR #236 / 0.5.5) append-only `billing_events` ledger + chat INSERT + `GET /admin/billing/events` 쿼리; TASK 12.2 ✅ (PR #241 / 0.5.6) tool + action emission (harness gates 7i.5 + 7h.1c). TASKs 12.3 (invoice materialization), 12.4 (reconciliation), 12.5 (Stripe ingest) 는 ISSUE 13 (HuggingFace model catalog), ISSUE 14 (tenant self-service) 와 함께 **commercialization layer, 2026-04-20 scope direction 에 따라 DEFERRED** — `v1.0.0` cut 의 gate 가 아님. EPIC 4 "close for 1.0" = ISSUE 11 + core product surfaces 가 production bar 에 도달하면 출시. EPIC close → `0.5.x` → `v1.0.0` (major bump — API stabilizes).

**신규 크레이트 (3개)**:
- `gadgetron-knowledge` — 지식 레이어 구성요소 (wiki / SearXNG 프록시 / stdio Gadget 서버).
- `gadgetron-penny` — Claude Code subprocess 브릿지 + `GadgetRegistry` + `LlmProvider` 구현 (`model = "penny"`).
- `gadgetron-web` — embedded assistant-ui Web UI (`/web`).

**서브-스프린트 4종 (P2A→P2D, 각 4주)**, 배포 형태 3종 (Local / On-prem / Cloud, 같은 코드 경로, 스토리지만 swap), 보안 위협 모델 M1–M8은 [`docs/design/phase2/00-overview.md`](design/phase2/00-overview.md)에 통합되어 있다. 핵심 ADR은 [`docs/adr/README.md`](adr/README.md) 참조.

### Phase 3 — Cluster Ops Hardening & Rich Automation (EPIC 5 / `v2.0.0` — PLANNED, post-1.0)

**목표**: Phase 2 collaboration entry point 를 프로덕션급 멀티 노드/멀티 테넌트 cluster operations platform 으로 승격하고, infra/scheduler/cluster toolization 과 richer automation 으로 확장.

**스코프 정정 (2026-04-20 ROADMAP v2 이후)**: 이 섹션은 원래 Phase 1 에서 Phase 3 로 직접 점프하는 2-phase 모델을 전제로 작성되었으나, ROADMAP v2 (PR #186) 에서 EPIC 단위로 리프레임되면서 아래 표 항목들 중 일부가 EPIC 3/4 (Phase 2 내부) 로 재배치되었다:
- **Bundle 시스템** → EPIC 3 CLOSED `v0.5.0` (ISSUE 8 hot-reload + ISSUE 9 manifests + ISSUE 10 marketplace, 2026-04-20). Phase 2 끝에서 완료.
- **멀티 테넌시 강화 / GPUaaS / ModelaaS / AgentaaS** → EPIC 4 ACTIVE (ISSUE 11 quotas ✅, ISSUE 12 billing telemetry ✅ (invoicing DEFERRED), ISSUE 14 self-service ✅ (PR #246), ISSUE 15 cookie-session login API ✅ (PR #248), ISSUE 16 unified Bearer-or-cookie middleware ✅ (PR #259 / v0.5.9), ISSUE 17 `ValidatedKey.user_id` plumbing ✅ (PR #260 / v0.5.10), ISSUE 19 `AuditEntry` actor fields structural ✅ (PR #262 / v0.5.11), ISSUE 20 TenantContext → AuditEntry plumbing ✅ (PR #263 / v0.5.12); ISSUE 13 HF catalog discovery/pinning + **ISSUE 18** web UI login form (React/Tailwind in `gadgetron-web`) + **ISSUE 21** pg `audit_log` consumer (drains `AuditWriter` mpsc channel → writes rows to `audit_log` using new actor columns; operator `GET /admin/audit/log` query endpoint) 남음, 닫으면 `v1.0.0` first production release).
- **HuggingFace Hub 통합** → EPIC 4 ISSUE 13 (per-model cost attribution 포함, billing pipeline 과 연동).
- **핫 리로드** (DescriptorCatalog) → EPIC 3 ISSUE 8 완료 (`POST /admin/reload-catalog` + SIGHUP, CatalogSnapshot ArcSwap). AppConfig 전체 hot-reload 는 여전히 Phase 3 로 deferred.

따라서 아래 표의 "Phase 3" 항목 중 `v2.0.0` EPIC 5 에 실제로 남는 것은 **K8s 통합, Slurm 통합, NUMA/MIG/인터커넥트 인식 스케줄링, 열/전력 스로틀링, 영구 메트릭 (Prometheus), 벤더 마이그레이션 (클라우드 ↔ 로컬 failover), OpenTelemetry 분산 트레이싱** — 즉 순수 cluster-ops layer 의 것들이다. 다른 항목들 (Bundle / 멀티 테넌시 / HF / 핫 리로드) 은 이미 Phase 2 (EPIC 3/4) 로 흡수되어 여기 표에서는 참고용 역사적 기록이다.

| 항목 | 설명 | 관련 크레이트 |
|------|------|-------------|
| K8s 통합 | CRD 기반 모델 배포, HPA/VPA 오토스케일링 | 신규 `gadgetron-k8s` |
| Slurm 통합 | HPC 클러스터 잡 서브미션 | 신규 `gadgetron-slurm` |
| NUMA/MIG/인터커넥트 인식 스케줄링 | NVLink/NVSwitch 토폴로지 + NUMA 선호도 기반 텐서 병렬 배치 | `gadgetron-scheduler` |
| 열/전력 스로틀링 | `GpuInfo::temperature_c`, `power_draw_w` 기반 요청 제한 | `gadgetron-scheduler` + `gadgetron-node` |
| 핫 리로드 | TOML 설정 변경 시 무중단 반영 | `gadgetron-core` |
| 영구 메트릭 | Prometheus 익스포터, 시계열 DB 저장 | `gadgetron-router` |
| GPUaaS / ModelaaS / AgentaaS | 계층형 추상화 (Phase 2 Penny는 AgentaaS의 첫 소비자) | `gadgetron-xaas` 확장 |
| 벤더 마이그레이션 | 클라우드 ↔ 로컬 자동 페일오버 | `gadgetron-router` + `gadgetron-scheduler` |
| 멀티 테넌시 강화 | 조직 격리, 리소스 쿼터, 사용량 청구 (Phase 2 PennyManager 기반) | `gadgetron-xaas` + `gadgetron-penny` |
| Bundle 시스템 | 커스텀 provider / routing / Gadget surface | 워크스페이스 확장 |
| HuggingFace Hub 통합 | 모델 카탈로그/다운로드 | `gadgetron-scheduler` |
| OpenTelemetry | 분산 트레이싱 | `gadgetron-gateway` |
| 글로벌 분산 | 리전 간 라우팅, 지연 최적화 | `gadgetron-router` |

---

## 6. 기술 스택과 의존성 맵

### 6.1 워크스페이스 의존성

| 계층 | 크레이트 | 버전 | 사용 크레이트 |
|------|---------|------|-------------|
| 비동기 런타임 | `tokio` | 1 (full) | core, provider, router, gateway, scheduler, node, tui, cli |
| 웹 프레임워크 | `axum` | 0.8 (macros) | gateway |
| 미들웨어 | `tower` | 0.5 | gateway |
| | `tower-http` | 0.6 (cors, trace) | gateway |
| HTTP 클라이언트 | `reqwest` | 0.12 (stream, json, rustls-tls) | provider, node |
| 직렬화 | `serde` | 1 (derive) | 전 크레이트 |
| | `serde_json` | 1 | 전 크레이트 |
| | `toml` | 0.8 | core, cli |
| 스트리밍 | `futures` | 0.3 | core, provider, router, gateway |
| | `tokio-stream` | 0.1 | provider, gateway, tui |
| | `eventsource-stream` | 0.2 | provider |
| | `async-stream` | 0.3.6 | provider |
| 에러 | `thiserror` | 2 | core, provider, router, gateway, scheduler, node |
| | `anyhow` | 1 | gateway, tui, cli |
| 로깅 | `tracing` | 0.1 | 전 크레이트 |
| | `tracing-subscriber` | 0.3 (env-filter, json) | gateway, cli |
| 시스템 모니터링 | `sysinfo` | 0.33 | node |
| GPU 모니터링 | `nvml-wrapper` | 0.10 (optional) | node |
| 동시 컬렉션 | `dashmap` | 6 | router, scheduler |
| TUI | `ratatui` | 0.29 | tui |
| | `crossterm` | 0.28 (event-stream) | tui |
| 유틸리티 | `uuid` | 1 (v4) | core, provider, gateway |
| | `chrono` | 0.4 (serde) | core, provider, router, gateway, scheduler, node, tui |
| | `async-trait` | 0.1 | core, provider, router, scheduler, node |
| 난수 | `rand` | 0.10.0 | router |

### 6.2 의존성 행렬 (실제 Cargo.toml 기준)

| 크레이트 | core | provider | router | gateway | scheduler | node | tui | cli |
|----------|:----:|:--------:|:------:|:-------:|:---------:|:----:|:---:|:---:|
| **core** | -- | | | | | | | |
| **provider** | O | -- | | | | | | |
| **router** | O | O | -- | | | | | |
| **gateway** | O | O | O | -- | O | O | | |
| **scheduler** | O | | | | -- | O | | |
| **node** | O | | | | | -- | | |
| **tui** | O | | O | | O | O | -- | |
| **cli** | O | O | O | O | O | O | O | -- |

> O = 직접 의존. 간접 의존은 표기하지 않음.

### 6.3 Feature Gates

| 크레이트 | Feature | 의존성 | 설명 |
|---------|---------|--------|------|
| `gadgetron-node` | `nvml` | `nvml-wrapper = "0.10"` | NVIDIA GPU 메트릭 수집 (GPU 이름, VRAM, 사용률, 온도, 전력). 비활성 시 `gpus: Vec::new()` |

---

## 7. 핵심 설계 원칙

### 7.1 Sub-ms P99 오버헤드

Rust 네이티브 + GC 없음 + 제로카피 스트리밍으로 게이트웨이 자체 오버헤드를 서브밀리초 수준으로 유지합니다.

- **GC-free**: Rust의 소유권 기반 메모리 관리로 GC pause 원천 차단
- **제로카피 스트리밍**: `LlmProvider::chat_stream()`이 `Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>`를 반환하고, `chat_chunk_to_sse()`가 이를 SSE `Event`로 직접 변환. 중간 버퍼링 없이 axum response body로 파이프라인
- **lock-free 메트릭**: `MetricsStore`가 `DashMap` 기반으로 읽기/쓰기 시 뮤텍스 없이 동시 접근
- **비동기 I/O**: `tokio` 런타임 기반으로 파일/네트워크 I/O가 논블로킹

### 7.2 GPU-first 스케줄링

모델 배치 결정에 GPU 리소스를 1순위로 고려합니다.

- **VRAM 인식**: `NodeResources::available_vram_mb()`로 가용 VRAM 계산, `estimate_vram_mb()`로 모델 요구량 추정
- **LRU Eviction**: `Scheduler::find_eviction_candidate()`가 `ModelDeployment::last_used` 기준으로 최소 사용 모델 우선 해제
- **NVML 메트릭**: `GpuInfo` 구조체가 `temperature_c`, `power_draw_w`, `utilization_pct` 제공 (Phase 2 스로틀링 기반)
- **NUMA/인터커넥트**: Phase 2에서 NVLink 토폴로지, NUMA 선호도 기반 배치 예정

### 7.3 Config-driven 단일 바이너리

```toml
[server]
bind = "0.0.0.0:8080"
api_key = "${OPENAI_API_KEY}"
request_timeout_ms = 30000

[router]
default_strategy = "cost_optimal"

[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4", "gpt-4o"]

[providers.anthropic]
type = "anthropic"
api_key = "${ANTHROPIC_API_KEY}"
models = ["claude-sonnet-4-20250514"]

[providers.ollama]
type = "ollama"
endpoint = "http://localhost:11434"

[providers.vllm-local]
type = "vllm"
endpoint = "http://localhost:8000"

[[nodes]]
id = "node-01"
endpoint = "http://localhost:11434"
gpu_count = 2

[[models]]
id = "meta-llama/Meta-Llama-3-70B"
engine = "vllm"
vram_requirement_mb = 42000
priority = 10
args = ["--tensor-parallel-size", "2"]
```

- `AppConfig::load()`가 TOML 파싱 + `${ENV}` 환경변수 치환
- 단일 `gadgetron` 바이너리로 전체 스택 구동
- 마이크로서비스 분할 없이 `gadgetron-cli`가 모든 크레이트를 정적 링크

### 7.4 LlmProvider 트레이트 기반 다형성

모든 프로바이더가 동일한 트레이트를 구현하여 `Router`가 프로바이더 타입을 알 필요 없이 라우팅합니다.

```rust
// 프로바이더 등록 패턴
let providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::from([
    ("openai".into(), Arc::new(OpenAiProvider::new(key, None))),
    ("anthropic".into(), Arc::new(AnthropicProvider::new(key, None).with_models(models))),
    ("ollama".into(), Arc::new(OllamaProvider::new(endpoint))),
]);

let router = Router::new(providers, config, metrics);
```

- 클라우드 프로바이더 (OpenAI, Anthropic): HTTP API + 각자의 인증 헤더
- 로컬 프로바이더 (Ollama, vLLM, SGLang): OpenAI 호환 API 또는 전용 엔드포인트
- Anthropic: `to_anthropic_request()`/`from_anthropic_response()`로 프로토콜 변환 계층 적용

### 7.5 단일 에러 타입

`GadgetronError` 열거형이 모든 크레이트의 에러를 통합합니다. `thiserror` 기반 `Display` 구현으로 에러 체인 추적이 용이합니다.

```rust
pub type Result<T> = std::result::Result<T, GadgetronError>;
```

각 변형은 발생 도메인을 명확히 표시합니다: `Provider`, `Router`, `Scheduler`, `Node`, `Config`, `NoProvider`, `AllProvidersFailed`, `InsufficientResources`, `ModelNotFound`, `NodeNotFound`, `HealthCheckFailed`, `Timeout`.

### 7.6 스트리밍 일관성

모든 프로바이더가 `chat_stream()`을 동일한 시그니처로 구현합니다:

- **OpenAI/vLLM/SGLang/Ollama**: SSE `data: {json}\n\n` ... `data: [DONE]\n\n` 파싱
- **Anthropic**: `content_block_delta` / `message_stop` 이벤트를 `ChatChunk`로 정규화 후 통일된 SSE 출력
- **버퍼링**: 모든 프로바이더가 `\n\n` 기준 버퍼 분할로 불완전 청크 처리
- **에러 전파**: 스트림 내 에러도 `Result<ChatChunk>`로 전달, 강제 종료 없이 graceful degradation

### 7.7 구현-ready 타입 시그니처 요약

| 크레이트 | 타입 | 주요 메서드 |
|---------|------|-----------|
| core | `LlmProvider` | `chat()`, `chat_stream()`, `models()`, `name()`, `health()` |
| core | `AppConfig` | `load(path) -> Result<Self>` |
| core | `NodeResources` | `available_vram_mb()`, `memory_available_bytes()` |
| core | `ModelDeployment` | `is_available()` |
| core | `estimate_vram_mb(params_billion, quantization) -> u64` |
| router | `Router` | `new()`, `resolve()`, `chat()`, `chat_stream()`, `list_models()`, `health_check_all()` |
| router | `MetricsStore` | `new()`, `record_success()`, `record_error()`, `get()`, `all_metrics()` |
| gateway | `GatewayServer` | `new(addr)`, `run()` |
| gateway | `chat_chunk_to_sse()` | `impl Stream<Item=Result<ChatChunk>> -> Sse<...>` |
| scheduler | `Scheduler` | `new()`, `deploy()`, `undeploy()`, `get_status()`, `list_deployments()`, `find_eviction_candidate()`, `register_node()`, `update_node()`, `list_nodes()` |
| node | `NodeAgent` | `new(config)`, `id()`, `endpoint()`, `collect_metrics()`, `status()`, `start_model()`, `stop_model()` |
| node | `ResourceMonitor` | `new()`, `collect() -> NodeResources` |
| tui | `App` | `new()`, `run()` |
