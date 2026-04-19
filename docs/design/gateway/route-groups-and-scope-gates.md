# Gateway Route Groups & Scope Gate Contract

> **담당**: @gateway-router-lead
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-20 — `/api/v1/web/workbench/admin/*` Management-scope sub-tree 이 ISSUE 8–10 의 full lifecycle 을 지원하도록 확장됨 (ISSUE 8 TASK 8.2 / PR #213 `POST /admin/reload-catalog`, ISSUE 10 TASK 10.1 / PR #223 `GET /admin/bundles`, ISSUE 10 TASK 10.2 / PR #224 `POST + DELETE /admin/bundles/{bundle_id}` with `validate_bundle_id()` path-traversal guard, ISSUE 10 TASK 10.4 / PR #227 Ed25519 `signature_hex` request field + `[web.bundle_signing]` trust anchors). **EPIC 4 신규 route**: `/api/v1/web/workbench/quota/status` (ISSUE 11 TASK 11.4 / PR #234 / v0.5.4) 는 `OpenAiCompat` scope 하에 tenant 가 자기 쿼터를 Management 권한 없이 조회 — 기존 `/api/v1/web/workbench/*` OpenAiCompat 예외 규칙으로 커버됨 (별도 scope 분기 불필요). Scope gate 순서 불변: `/admin/*` (Management) 가 `/api/v1/web/workbench/*` (OpenAiCompat) 보다 먼저 매칭되어야 `/admin/` 이 자기 prefix 로 OpenAiCompat 키를 잘못 허용하는 일이 없음.
> **관련 크레이트**: `gadgetron-gateway`, `gadgetron-core`, `gadgetron-xaas`, `gadgetron-web`, `gadgetron-knowledge`
> **Phase**: [P1] primary / [P2A] embedded web mount / [P2B] workbench scope exception / [P2B-EPIC3] workbench admin sub-tree (Management-scoped catalog hot-reload, PR #213)
> **관련 문서**: `docs/design/gateway/wire-up.md`, `docs/design/gateway/workbench-projection-and-actions.md`, `docs/design/core/workbench-shared-types.md`, `docs/design/web/expert-knowledge-workbench.md`, `docs/design/phase2/03-gadgetron-web.md`, `docs/process/04-decision-log.md` D-20260414-02, D-20260418-14, `docs/manual/api-reference.md` §POST /api/v1/web/workbench/admin/reload-catalog, `docs/reviews/pm-decisions.md` D-6/D-7/D-11/D-12
> **보정 범위**: 2026-04-18 기준 `origin/main` 의 실제 gateway router/build/scope contract 는 이 문서가 authoritative 이다. `docs/design/gateway/wire-up.md` 는 Sprint-era wide draft 로 유지하되, 현재 trunk 의 route-group 분류, scope gate (2026-04-19 기준 4 개 prefix — admin sub-tree 포함), 413 shaping, `AppState` optional field semantics 는 본 문서를 우선한다.

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백 [P1] [P2A] [P2B]

2026-04-18 기준 `origin/main` 은 gateway 쪽에서 세 가지 중요한 변화를 이미 landed 했다.

1. `build_router()` 가 `/v1/*`, `/api/v1/*`, `/api/v1/web/workbench/*`, `/health`, `/ready` 를 서로 다른 인증/권한 규칙으로 조립한다.
2. `build_router_with_web()` 가 `web-ui` feature + `web.enabled` 설정을 기준으로 `/web/*` subtree 를 public static surface 로 마운트한다.
3. `W3-WEB-2` 가 `/api/v1/web/workbench/*` 를 `/api/v1/*` 아래에 두면서도 `Scope::Management` 가 아니라 `Scope::OpenAiCompat` 로 처리하는 예외를 도입했다.

하지만 현재 문서 구조는 이 trunk reality 를 한 곳에서 설명하지 않는다.

- `docs/design/gateway/wire-up.md` 는 load-bearing 아이디어를 많이 담고 있지만 Sprint 3 wide draft 로 남아 있고, 실제 `AppState` shape 와 layer stack 이 현재 코드와 1:1로 맞지 않는다.
- `docs/design/gateway/workbench-projection-and-actions.md` 는 workbench read/action semantics authority 이지만, gateway 전체 route group 분류와 scope gate ordering 자체를 소유하지는 않는다.
- `docs/design/phase2/03-gadgetron-web.md` 는 `/web` embed 와 CSP 를 정의하지만, `/web` 이 public asset surface 이고 `/api/v1/web/workbench/*` 는 authenticated data plane 이라는 경계를 gateway runtime 관점에서 닫지 않는다.
- 코드 주석(`server.rs`, `scope.rs`, `web/workbench.rs`) 만 읽어도 구현은 이해할 수 있지만, cross-review 가능한 production doc 는 없다.

즉, 최근 mainline 이 노출한 가장 큰 문서 공백은 다음이다.

> **어떤 route group 이 public 이고, 어떤 route group 이 auth 를 요구하며, 어떤 prefix 가 어떤 scope 로 매핑되는가, 그리고 그 규칙이 어떤 middleware order 와 `AppState` wiring 에 의해 유지되는가**

이 문서는 그 seam 을 닫는다.

### 1.2 제품 비전과의 연결 [P1] [P2A] [P2B]

`docs/00-overview.md §1`, `docs/reviews/pm-decisions.md` D-6/D-7/D-11, 그리고 최신 P2A/P2B 문서는 Gadgetron gateway 를 단순 reverse proxy 가 아니라 다음 두 역할을 동시에 수행하는 choke-point 로 정의한다.

- OpenAI-compatible data plane (`/v1/*`) 와 operator/admin plane (`/api/v1/*`) 를 분리하는 namespace gate
- 같은 capability 를 다른 surface 로 노출하더라도 auth, scope, audit, error envelope 을 일관되게 유지하는 policy gate

이 원칙에서 중요한 점은 두 가지다.

1. `D-7` 은 namespace 를 분리했지만, namespace 분리는 곧바로 "전부 management" 를 뜻하지 않는다.
2. `W3-WEB-2` 이후 `/api/v1/web/workbench/*` 는 path prefix 상 관리 namespace 내부에 있지만, 실제 권한 기준은 `/v1/*` 와 같은 `OpenAiCompat` base scope 를 사용한다.

즉, current gateway contract 는 다음 문장으로 요약된다.

> **`/web` 은 public shell 이고, `/v1/*` 와 `/api/v1/web/workbench/*` 는 operator-facing authenticated plane 이며, 나머지 `/api/v1/*` 는 privileged management plane 이다.**

### 1.3 고려한 대안과 채택하지 않은 이유 [P1] [P2B]

| 대안 | 설명 | 채택하지 않은 이유 |
|---|---|---|
| A. `docs/design/gateway/wire-up.md` wide draft 만 계속 authority 로 둔다 | 문서 수가 늘지 않는다 | 현재 trunk 의 route matrix, `AppState` optional fields, `openai_shape_413`, `metrics_middleware`, workbench scope exception 이 분산돼 drift 위험이 크다 |
| B. `/api/v1/web/workbench/*` 를 `/v1/web/*` 로 옮긴다 | scope 규칙이 단순해진다 | current landed code, same-origin `/web` shell, workbench design docs, D-20260418-14 실행 순서와 충돌한다 |
| C. scope 예외를 handler 내부 `if path == ...` 식으로 처리한다 | middleware 규칙은 단순해 보인다 | authz truth 가 분산되고, route 추가 시 review 누락으로 bypass 위험이 생긴다 |
| D. focused route-group + scope-gate contract 를 별도 문서로 고정한다 | current code 와 review rubric 을 1:1로 맞출 수 있다 | 문서가 하나 늘지만 trunk reality 와 review traceability 를 가장 잘 보존한다 |

채택: **D. focused gateway route-group and scope-gate contract**

### 1.4 핵심 설계 원칙과 trade-off [P1] [P2A] [P2B]

1. **Route group policy is declared at router assembly time**
   handler 내부에서 "이 엔드포인트는 사실 management" 를 다시 판단하지 않는다. route prefix 와 middleware stack 이 1차 권한 경계를 소유한다.
2. **Authenticated routes are default-deny**
   `/health`, `/ready`, `/web/*` 같은 명시적 public surface 를 제외한 gateway data plane 은 Bearer auth 를 요구한다.
3. **Scope exceptions must be explicit and prefix-ordered**
   `/api/v1/web/workbench/*` 예외는 `/api/v1/*` catch-all 보다 먼저 평가되어야 한다. 순서가 깨지면 즉시 privilege regression 이다.
4. **Shared state stays in `AppState`, request identity stays in extensions**
   key validator, quota enforcer, audit writer, provider map, optional workbench service 는 `AppState` 가 소유한다. `request_id`, `ValidatedKey`, `TenantContext` 는 middleware 가 요청 extension 으로 전달한다.
5. **Pre-handler failures still get a coherent operator-facing envelope**
   body-size rejection처럼 handler 이전에 발생하는 실패도 가능하면 OpenAI-compatible JSON shape 로 정규화한다. 단, `/health`와 `/ready` 같은 public probe 는 단순 status body 를 유지한다.
6. **Public shell, authenticated data plane**
   `/web/*` asset mount 는 public 이지만, shell 이 읽는 `/v1/*` 와 `/api/v1/web/workbench/*` API 는 auth/scope gate 뒤에 있다.
7. **Workbench stays a gateway leaf concern**
   `GatewayWorkbenchService`, `WorkbenchProjectionService`, `WorkbenchHttpError` 같은 route-local behavior 는 `gadgetron-gateway` 에 남긴다. `gadgetron-core` 는 shared DTO 와 context 만 소유한다.

Trade-off:

- route group 규칙이 단순 path-prefix table 이상으로 진화했다.
- 대신 이 복잡도를 router assembly 와 scope middleware 에 모아둬야 `/web`, `/v1`, `/api/v1`, workbench, future XaaS surface 가 서로 다른 authz truth 를 갖는 drift 를 막을 수 있다.

### 1.5 Operator Touchpoint Walkthrough [P1] [P2A] [P2B]

1. 운영자는 먼저 `GET /health` 또는 `GET /ready` 로 프로세스/DB readiness 를 확인한다. 이 두 경로는 public probe 이다.
2. 애플리케이션 클라이언트는 `gad_live_*` 또는 `gad_test_*` 키로 `GET /v1/models`, `POST /v1/chat/completions` 를 호출한다.
3. 같은 키로 `GET /api/v1/nodes` 를 호출하면 403 이다. 이 경로는 `Scope::Management` 를 요구한다.
4. 사용자가 `/web` 을 열면 static shell 은 공개적으로 내려가지만, shell 이 호출하는 `/api/v1/web/workbench/bootstrap` 등은 same-origin Bearer auth 를 요구한다.
5. `OpenAiCompat` 키는 `/api/v1/web/workbench/*` 에 접근할 수 있어야 하고, 같은 키가 일반 `/api/v1/*` 관리 경로까지 열어서는 안 된다.
6. oversized request 는 handler 이전에 413 으로 막히지만, body 는 OpenAI-shaped JSON 으로 재작성되어 SDK client 가 plain-text parsing failure 를 겪지 않는다.
7. `state.workbench` 가 아직 wiring 되지 않은 build 에서 workbench API 를 호출하면 current trunk 기준 OpenAI-shaped config error(400)를 받는다. 이것이 숨겨진 404 나 unauthenticated bypass 로 바뀌면 안 된다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API [P1] [P2A] [P2B]

#### 2.1.1 Shared gateway state (`AppState`) [P1] [P2A] [P2B]

```rust
use std::{collections::HashMap, sync::Arc};

use gadgetron_core::{provider::LlmProvider, ui::WsMessage};
use gadgetron_router::Router as LlmRouter;
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::KeyValidator,
    quota::enforcer::QuotaEnforcer,
};
use tokio::sync::broadcast;

use crate::web::workbench::GatewayWorkbenchService;

#[derive(Clone)]
pub struct AppState {
    pub key_validator: Arc<dyn KeyValidator + Send + Sync>,
    pub quota_enforcer: Arc<dyn QuotaEnforcer + Send + Sync>,
    pub audit_writer: Arc<AuditWriter>,
    pub providers: Arc<HashMap<String, Arc<dyn LlmProvider + Send + Sync>>>,
    pub router: Option<Arc<LlmRouter>>,
    pub pg_pool: Option<sqlx::PgPool>,
    pub no_db: bool,
    pub tui_tx: Option<broadcast::Sender<WsMessage>>,
    pub workbench: Option<Arc<GatewayWorkbenchService>>,
    // P2B observability (PSL/KC cycles — D-20260418-14..22):
    pub penny_shared_surface: Option<Arc<dyn PennySharedSurfaceService>>,
    pub penny_assembler: Option<Arc<dyn PennyTurnContextAssembler>>,
    pub agent_config: Arc<AgentConfig>,
    pub activity_capture_store: Option<Arc<dyn ActivityCaptureStore>>,
    pub candidate_coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
}
```

계약:

- `providers` 는 provider registry visibility 와 test harness setup 을 위해 남긴다. 실제 routing hot path 는 `router: Option<Arc<LlmRouter>>` 를 사용한다.
- `router = None` 은 legacy unit fixture 나 일부 handler-isolated test 에서 허용된다. routing-dependent handler 는 fail-closed 해야 한다.
- `pg_pool` 와 `no_db` 는 한 쌍이다. `no_db = true` 면 `/ready` 는 DB probe 없이 200 을 반환한다.
- `tui_tx` 는 optional observability surface 이다. 없을 때 `metrics_middleware` 는 no-op 이다.
- `workbench` 는 optional gateway leaf service 이다. current trunk 는 route subtree 를 항상 mount 하지만, service 미주입 시 route handler 가 OpenAI-shaped config error 로 fail-closed 한다. `knowledge_service` 가 없어도 `build_workbench` 는 `Some(degraded_workbench)` 를 반환하므로 endpoint 들은 항상 mount 된다 — bootstrap 응답의 `degraded_reasons` 로 상태가 operator 에 노출된다.
- `penny_shared_surface` / `penny_assembler` / `activity_capture_store` / `candidate_coordinator` 는 PSL-1 ~ KC-1c 에서 순차 landed 됐다. **boot-gating rule**:
  - `[knowledge]` 섹션 없음 → 5 필드 모두 `None`, P1 legacy serve 경로 보존
  - `[knowledge]` 있고 `[knowledge.curation].enabled = false` → `workbench` 만 Some, candidate plane 은 None
  - `[knowledge]` 있고 `enabled = true` → 5 필드 모두 wired (Pg backing if `pg_pool.is_some()`, else InMemory + operator-visible `tracing::warn!`)
- `agent_config` 는 required (non-optional) Arc 이다. `AgentConfig::default()` 가 안전한 기본값을 제공하며, chat handler 가 `[agent.shared_context]` 를 소비할 때 이 Arc 를 읽는다.
- 전체 matrix 의 의미 / source / `None` 동작은 `docs/design/gateway/workbench-projection-and-actions.md §2.1.3` 에 표로 정리돼 있다. 본 문서는 route gating 관점 (auth / scope / boot-gating) 만 다룬다.

#### 2.1.2 Router entrypoints and route groups [P1] [P2A] [P2B]

```rust
pub fn build_router(state: AppState) -> Router;

#[cfg(feature = "web-ui")]
pub fn build_router_with_web(
    state: AppState,
    web_cfg: &gadgetron_core::config::WebConfig,
) -> Router;
```

Route group matrix:

| Route group | Auth | Required scope | Owner | Phase | Notes |
|---|---|---|---|---|---|
| `/health` | 없음 | 없음 | gateway runtime | [P1] | liveness probe |
| `/ready` | 없음 | 없음 | gateway runtime | [P1] | DB/no-db readiness probe |
| `/web/*` | 없음 | 없음 | embedded `gadgetron-web` | [P2A] | static shell only, feature+config gated |
| `/v1/*` | Bearer | `Scope::OpenAiCompat` | OpenAI-compatible data plane | [P1] | `/v1/chat/completions`, `/v1/models` |
| `/api/v1/web/workbench/admin/*` | Bearer | `Scope::Management` | workbench admin plane — catalog hot-reload (`POST /admin/reload-catalog`, TASK 8.2), bundle discovery (`GET /admin/bundles`, TASK 10.1), install + uninstall (`POST + DELETE /admin/bundles/{bundle_id}`, TASK 10.2 with `validate_bundle_id()` path-traversal guard), Ed25519 signature verification on install (TASK 10.4 `signature_hex` request field + `[web.bundle_signing]` trust anchors) | [EPIC 3 complete] | **must be matched BEFORE** the broader workbench rule — shipped across PRs #213 / #223 / #224 / #227 as ISSUE 8–10 rolled in |
| `/api/v1/web/workbench/*` | Bearer | `Scope::OpenAiCompat` | workbench read/action plane | [P2B] | explicit exception under `/api/v1/*` |
| `/api/v1/xaas/*` | Bearer | `Scope::XaasAdmin` | XaaS control plane | [P2+] | reserved in scope table even before all routes land |
| 기타 `/api/v1/*` | Bearer | `Scope::Management` | admin/operator API | [P1] | nodes, deploy, usage, costs |

Current nested workbench registration:

```rust
let authenticated_routes = Router::new()
    .route("/v1/chat/completions", post(chat_completions_handler))
    .route("/v1/models", get(list_models_handler))
    .nest("/api/v1/web/workbench", workbench_routes())
    .route("/api/v1/nodes", get(list_nodes_handler))
    .route("/api/v1/models/deploy", post(deploy_model_handler))
    .route("/api/v1/models/{id}", delete(undeploy_model_handler))
    .route("/api/v1/models/status", get(model_status_handler))
    .route("/api/v1/usage", get(usage_handler))
    .route("/api/v1/costs", get(costs_handler));
```

규칙:

- `/web/*` 와 `/api/v1/web/workbench/*` 는 이름이 비슷해도 전혀 다른 surface 다.
- `/web/*` 는 public asset tree 이고, `/api/v1/web/workbench/*` 는 authenticated JSON API tree 다.
- `build_router_with_web()` 는 항상 `build_router()` 를 먼저 조립한 뒤 base router 위에 `/web` subtree 를 덧씌운다.

#### 2.1.3 Scope gate mapping [P1] [P2B] [P2B-EPIC3]

```rust
let required_scope: Option<Scope> = if path.starts_with("/v1/") {
    Some(Scope::OpenAiCompat)
} else if path.starts_with("/api/v1/xaas/") {
    Some(Scope::XaasAdmin)
} else if path.starts_with("/api/v1/web/workbench/admin/") {
    // [P2B-EPIC3] ISSUE 8 TASK 8.2 (PR #213): admin sub-tree under the
    // workbench namespace is privileged (catalog hot-reload today; future
    // bundle install/uninstall). Must be matched BEFORE the broader
    // workbench rule so OpenAiCompat callers get 403 on /admin/*.
    Some(Scope::Management)
} else if path.starts_with("/api/v1/web/workbench/") {
    Some(Scope::OpenAiCompat)
} else if path.starts_with("/api/v1/") {
    Some(Scope::Management)
} else {
    None
};
```

계약:

- `/api/v1/web/workbench/admin/` 분기는 `/api/v1/web/workbench/` 일반 workbench 분기보다 앞에 와야 한다 — 순서가 뒤집히면 OpenAiCompat 키가 `/admin/reload-catalog` 같은 Management-only 엔드포인트에 접근할 수 있는 즉각적인 privilege regression 이다. [P2B-EPIC3]
- `/api/v1/web/workbench/` 분기는 `/api/v1/` catch-all 보다 앞에 와야 한다.
- `ScopeGuard` 는 authenticated route stack 안에서만 실행된다. `/health`, `/ready`, `/web/*` 는 이 middleware 에 도달하지 않는다.
- `TenantContext` 가 request extensions 에 없으면 fail-closed 한다. 이는 layer-ordering 위반 방어장치다.
- scope denial(403)은 audit entry 를 남겨야 한다. 401 auth failure 와 달리 여기서는 tenant/api_key/request_id 가 이미 확정돼 있다.

### 2.2 내부 구조 [P1] [P2A] [P2B]

#### 2.2.1 Middleware stack ordering [P1] [P2B]

Authenticated route stack 의 inbound 순서는 다음과 같다.

```text
[map_response openai_shape_413]
        |
[RequestBodyLimitLayer 4 MiB]
        |
[TraceLayer]
        |
[request_id_middleware]
        |
[auth_middleware]
        |
[tenant_context_middleware]
        |
[scope_guard_middleware]
        |
[metrics_middleware]
        |
[handler / workbench_routes()]
```

설계 이유:

- `RequestBodyLimitLayer` 는 body type 을 `Limited<Body>` 로 감싼다. 따라서 auth/scope/metrics middleware 와 같은 `Body`-typed middleware 는 분리된 `.layer(...)` 체인으로 유지해야 한다.
- `openai_shape_413` 는 가장 바깥에서 413 응답만 가로채 body 를 JSON 으로 바꾼다. non-413 응답은 그대로 통과한다.
- `metrics_middleware` 는 handler 직전/직후를 감싸야 latency 와 status 를 가장 정확히 기록할 수 있다.

#### 2.2.2 Request context flow [P1]

```text
request_id_middleware
  -> inserts `Uuid` into extensions + `x-request-id` header

auth_middleware
  -> parses `Authorization: Bearer gad_*`
  -> validates via `KeyValidator`
  -> inserts `Arc<ValidatedKey>` into extensions

tenant_context_middleware
  -> reuses request_id if present
  -> builds `TenantContext`
  -> inserts `TenantContext` into extensions

scope_guard_middleware
  -> reads `TenantContext.scopes`
  -> enforces required scope

metrics_middleware
  -> reads `TenantContext` + request_id
  -> optionally emits `WsMessage::RequestLog`
```

규칙:

- request-scoped identity 는 `AppState` 로 올리지 않는다.
- `TenantContext` 는 Phase 1 에서 unlimited placeholder `QuotaSnapshot` 을 담지만, route group contract 자체는 변하지 않는다.
- raw token 은 `ApiKey::parse()` 이후 hash 기반 validation 으로만 흐른다. tracing/audit/error message 에 raw bearer string 을 남기지 않는다.

#### 2.2.3 Workbench read-plane seam [P2B]

이 문서는 workbench read/action semantics 자체를 다시 정의하지 않는다. 그 authority 는 `docs/design/gateway/workbench-projection-and-actions.md` 와 `docs/design/core/workbench-shared-types.md` 이다. 본 문서가 고정하는 것은 **router + scope + state wiring seam** 이다.

Current trunk contract:

- workbench subtree 는 authenticated_routes 아래에 nested 된다.
- base scope 는 `Scope::OpenAiCompat` 다.
- current mounted read-only slice 는 `GET /bootstrap`, `GET /activity`, `GET /requests/{request_id}/evidence` 다.
- `GatewayWorkbenchService`, `WorkbenchProjectionService`, `WorkbenchHttpError` 는 gateway-local 구현이다.
- shared DTO 는 `gadgetron-core::workbench` 에 남고, route-local behavior 는 gateway leaf crate 에 남는다.

중요한 current-code detail:

- `state.workbench = None` 이면 `require_workbench()` 는 `WorkbenchHttpError::Core(GadgetronError::Config(...))` 를 반환한다.
- 따라서 current trunk 의 not-wired behavior 는 **OpenAI-shaped 400 config error** 다.
- 이것을 503 service unavailable 로 올리고 싶다면 `GadgetronError` taxonomy 또는 gateway-local mapping 을 바꾸는 별도 설계 PR 이 필요하다. 본 문서는 current landed behavior 를 과장 없이 문서화한다.

#### 2.2.4 Embedded web mount seam [P2A]

`build_router_with_web()` 는 다음 규칙을 따른다.

```rust
let base = build_router(state);
if !web_cfg.enabled {
    return base;
}
base.route("/web/", get(web_trailing_slash_redirect))
    .route("/favicon.ico", get(web_favicon_handler))
    .nest("/web", web_router)
```

계약:

- `web_cfg.enabled = false` 면 `/web/*` subtree 는 아예 mount 되지 않는다.
- `/web/` 는 canonical `/web` base path 로 redirect 된다.
- `/favicon.ico` 는 204 로 응답한다.
- `/web/*` asset 경로는 public 이다. 실제 데이터 호출은 browser 가 `web.api_base_path` 를 사용해 `/v1/*` 와 workbench route group 으로 별도 인증 요청을 보낸다.

### 2.3 설정 스키마 [P1] [P2A]

이 문서는 새 config field 를 추가하지 않는다. 다만 route group contract 를 좌우하는 기존 runtime input 은 다음이다.

```toml
[server]
bind = "0.0.0.0:8080"

[web]
enabled = true
api_base_path = "/v1"
```

Runtime input rules:

- `server.bind`
  기본값: `"0.0.0.0:8080"`
  override: `GADGETRON_BIND`, `gadgetron serve --bind`
- database runtime
  source: `GADGETRON_DATABASE_URL` 및 `gadgetron serve` 의 DB mode resolution
  override: `gadgetron serve --no-db`
- `web.enabled`
  기본값: `true`
  `false` 이면 `/web/*` subtree 전체가 mount 되지 않는다
- `web.api_base_path`
  기본값: `"/v1"`
  validation:
  - `/` 로 시작해야 한다
  - `;`, `\n`, `\r`, `<`, `>`, `"`, `'`, backtick 을 포함하면 안 된다

운영자 의미:

- `--no-db` 또는 DB URL 부재는 route group 자체를 바꾸지 않는다. 다만 `/ready` behavior 를 "probe actual PG" 에서 "always ready" 로 전환한다.
- `web.enabled` 는 `/web/*` asset subtree 만 토글한다. `/v1/*` 또는 `/api/v1/web/workbench/*` auth/scope contract 는 변하지 않는다.

### 2.4 에러 & 로깅 [P1] [P2A] [P2B]

주요 error/logging contract:

| 위치 | 조건 | HTTP | 응답 shape | 감사 로그 |
|---|---|---|---|---|
| `auth_middleware` | Authorization header 없음 / malformed / revoked | 401 | OpenAI-shaped `TenantNotFound` | 예 (`Uuid::nil()` ids) |
| `scope_guard_middleware` | scope 부족 | 403 | OpenAI-shaped `Forbidden` | 예 (real tenant/api_key/request_id) |
| `openai_shape_413` | body > 4 MiB | 413 | OpenAI-shaped `{error:{code:"request_too_large",...}}` | 아니오 |
| `/ready` | DB 미도달 또는 pool 없음 (`no_db = false`) | 503 | `{"status":"unavailable"}` | 아니오 |
| `workbench request not found` | request evidence 대상 없음/비가시성 | 404 | OpenAI-shaped `workbench_request_not_found` | workbench-specific path 에서 처리 |
| `state.workbench = None` | workbench service 미주입 | 400 | OpenAI-shaped `Config` | 아니오 |

로깅 규칙:

- raw bearer token, DB credential, config secret 은 tracing field 로 남기지 않는다.
- `scope denied` warning 은 `tenant_id`, `required_scope`, `path` 까지만 남긴다.
- `/ready` 실패는 DB 에러를 tracing 으로 남기되, 응답 본문에는 `{"status":"unavailable"}` 만 준다.
- TUI metrics emit 는 best-effort 이다. receiver 부재나 lag 는 hot path failure 로 승격하지 않는다.

STRIDE 요약:

| 자산 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| `gad_*` API key | client -> authenticated route stack | Spoofing | `ApiKey::parse` + hash validation + 401 audit |
| management namespace | `TenantContext` -> `ScopeGuard` | Elevation of Privilege | ordered prefix match: `/api/v1/web/workbench/admin/*` (Management, [P2B-EPIC3] PR #213) 먼저 → `/api/v1/web/workbench/*` (OpenAiCompat) 다음 → `/api/v1/*` (Management) catch-all + regression tests (harness Gate 7q.2 = OpenAiCompat 키가 `/admin/reload-catalog` 호출 시 403) |
| error envelope | body-limit layer -> SDK client | Information Disclosure / DX breakage | `openai_shape_413` 가 plain-text 413 을 JSON 으로 정규화 |
| audit trail | auth/scope failure path -> `AuditWriter` | Repudiation | 401/403 failure audit entry |
| gateway memory/CPU | external request body | DoS | 4 MiB body limit at outer edge |
| workbench service visibility | authenticated route -> `state.workbench` | Tampering / misconfiguration confusion | fail-closed config error, no silent fallback to public data |

### 2.5 의존성 [P1] [P2A] [P2B]

추가 dependency 는 없다. current contract 는 기존 workspace dependency 만 사용한다.

- `axum 0.8` — router, extractor, middleware glue
- `tower 0.5`, `tower-http 0.6` — limit/trace layers
- `tokio 1` — async runtime, broadcast channel
- `sqlx 0.8` — readiness probe `PgPool`
- `moka 0.12` — key validator cache (via `gadgetron-xaas`)

의존성 경계:

- auth/quota/audit 는 `gadgetron-xaas`
- shared scope/context/config/ui types 는 `gadgetron-core`
- routing hot path 는 `gadgetron-router`
- workbench projection backing source 는 `gadgetron-knowledge`
- static `/web` asset service 는 optional `gadgetron-web`

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 상하위 의존 구조 [P1] [P2A] [P2B]

```text
Client / Browser
      |
      v
gadgetron-gateway::server::{build_router, build_router_with_web}
      |
      +--> middleware::{request_id, auth, tenant_context, scope, metrics}
      |         |
      |         +--> gadgetron-core::{Scope, TenantContext, WsMessage, WebConfig}
      |         +--> gadgetron-xaas::{KeyValidator, QuotaEnforcer, AuditWriter}
      |
      +--> handlers::{chat_completions, list_models, admin stubs}
      |         |
      |         +--> gadgetron-router::Router
      |         +--> gadgetron-provider implementations
      |
      +--> web::workbench::{workbench_routes, GatewayWorkbenchService}
      |         |
      |         +--> gateway-local projection/error traits
      |         +--> gadgetron-core::workbench shared DTO
      |         +--> gadgetron-knowledge::KnowledgeService
      |
      +--> gadgetron-web service (optional `/web/*`)
```

### 3.2 데이터 흐름 다이어그램 [P1] [P2B]

```text
Bearer request
  -> request_id
  -> auth (ValidatedKey)
  -> tenant_context (TenantContext + request_id)
  -> scope_gate (route prefix -> required Scope)
  -> metrics
  -> handler/workbench
  -> response

Oversize request
  -> RequestBodyLimitLayer 413
  -> openai_shape_413 rewrites body
  -> response
```

### 3.3 타 도메인과의 인터페이스 계약 [P1] [P2A] [P2B]

- `@xaas-platform-lead`
  `KeyValidator`, `QuotaEnforcer`, `AuditWriter` 를 통해 auth/scope/audit 가 gateway stack 에 주입된다.
- `@devops-sre-lead`
  `/health`, `/ready`, tracing, body-limit, public `/web` exposure 가 운영 surface 로 이어진다.
- `@dx-product-lead`
  OpenAI-shaped error envelope, `request_too_large` message, `/web` public shell vs authenticated data plane walkthrough 을 소비한다.
- `@qa-test-architect`
  scope regression, 413 rewrite, `/ready`, `/web` mount, workbench not-wired behavior 를 integration contract 로 검증한다.
- `@chief-architect`
  `gadgetron-core` 에는 shared DTO/context 만 남기고, gateway-local service/error/router assembly 는 leaf crate 에 유지한다.

### 3.4 D-12 크레이트 경계 준수 여부 [P1] [P2B]

준수한다.

- `Scope`, `TenantContext`, `WebConfig`, `WsMessage`, workbench shared DTO 는 `gadgetron-core`
- `KeyValidator`, `AuditWriter`, `QuotaEnforcer` 는 `gadgetron-xaas`
- `GatewayWorkbenchService`, `WorkbenchProjectionService`, `WorkbenchHttpError`, `build_router*` 는 `gadgetron-gateway`

즉, current trunk 는 "shared types in core, route-local behavior in leaf crate" 원칙을 그대로 따른다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위 [P1] [P2A] [P2B]

| 대상 | 검증할 invariant |
|---|---|
| `format_body_limit`, `openai_shape_413` | 413 응답이 plain text 가 아니라 OpenAI-shaped JSON 으로 정규화된다 |
| `auth_middleware` | missing/malformed/revoked auth 가 401 + audit 로 fail-closed 한다 |
| `tenant_context_middleware` | `ValidatedKey` 를 `TenantContext` 로 변환하고 request_id 를 재사용한다 |
| `scope_guard_middleware` | `/v1/*`, `/api/v1/xaas/*`, `/api/v1/web/workbench/admin/*` ([P2B-EPIC3]), `/api/v1/web/workbench/*`, `/api/v1/*` 순서가 정확히 적용된다. harness Gate 7q.2 가 `/admin/reload-catalog` 에 OpenAiCompat 키로 호출 시 403 을 확인하여 ordering regression 을 잡는다 |
| `metrics_middleware` | `tui_tx` 가 있을 때만 `WsMessage::RequestLog` emit, 없을 때 no-op |
| `workbench_routes()` | OpenAiCompat key 로 bootstrap 접근 가능, request evidence not found 가 404 envelope 로 직렬화된다 |
| `ready_handler` | `no_db` / missing pool / reachable PG 상태에 따라 200 또는 503 을 반환한다 |

### 4.2 테스트 하네스 [P1] [P2A] [P2B]

- `MockKeyValidator` / `NoopKeyValidator`
- `InMemoryQuotaEnforcer`
- `AuditWriter::new(TEST_AUDIT_CAPACITY)`
- `lazy_pool()` 또는 intentionally unreachable `PgPool`
- `broadcast::channel` 기반 TUI metrics 검증
- `GatewayWorkbenchService { projection: Arc<dyn WorkbenchProjectionService> }` stub

Property-based test:

- N/A + 이유: 이 문서는 deterministic route-prefix / middleware-order contract 가 핵심이며, current risk 는 fuzzed numeric domain 보다 explicit prefix regression 이다.

### 4.3 커버리지 목표 [P1] [P2A] [P2B]

- `gadgetron-gateway` router/middleware/workbench route contract 관련 line coverage 85%+
- scope prefix 분기 branch coverage 100%
- `openai_shape_413`, auth failure, scope failure, workbench exception 회귀 경로는 모두 branch-hit

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위 [P1] [P2A] [P2B]

함께 검증할 크레이트:

- `gadgetron-gateway`
- `gadgetron-core`
- `gadgetron-xaas`
- `gadgetron-web` (`web-ui` feature path)
- `gadgetron-knowledge` (workbench projection stub/in-process path)

핵심 시나리오:

1. `OpenAiCompat` key 로 `GET /v1/models` 는 200, `GET /api/v1/nodes` 는 403
2. 같은 `OpenAiCompat` key 로 `GET /api/v1/web/workbench/bootstrap` 는 200
3. oversized `/v1/chat/completions` request 는 413 + JSON error envelope
4. `/ready` 는 `no_db = true` 에서 200, `no_db = false && pg_pool missing/unreachable` 에서 503
5. `build_router_with_web()` 는 `/web` subtree 를 public 으로 mount 하고, API route 들은 계속 auth 를 요구
6. `state.workbench = None` 인 build 에서 workbench 호출은 current trunk contract 대로 400 config error 로 fail-closed

### 5.2 테스트 환경 [P1] [P2A] [P2B]

- 기본 경로는 로컬 unit/integration harness 만으로 충분하다
- real Postgres container 는 필수 아니다. current route contract 의 핵심 failure path 는 unreachable/missing pool 로 재현 가능하다
- `web-ui` feature 켠 default build 와 `build_router_with_web()` path 를 함께 검증한다
- headless/API-only build 는 `--no-default-features` compile smoke test 를 future CI 확장 항목으로 둔다

### 5.3 회귀 방지 [P1] [P2A] [P2B]

| 변경 사항 | 실패해야 하는 테스트 |
|---|---|
| `/api/v1/web/workbench/*` 가 `/api/v1/*` catch-all 에 잡혀 `Management` scope 를 요구 | scope regression integration test |
| 413 body 가 다시 plain text 로 내려감 | `openai_shape_413` unit/integration test |
| `/web/*` asset subtree 가 auth stack 뒤로 들어감 | web mount integration test |
| `metrics_middleware` 가 `tui_tx = None` 에서 panic 또는 error | metrics no-op test |
| `state.workbench = None` 에서 silent 404/200 fallback 발생 | workbench not-wired integration test |
| `/ready` 가 DB 미도달 상황에서도 200 을 반환 | readiness regression test |

---

## 6. Phase 구분

| 항목 | Phase |
|---|---|
| `/health`, `/ready`, `/v1/*`, 일반 `/api/v1/*`, auth/request_id/tenant_context/scope/metrics stack | [P1] |
| `openai_shape_413` 413 JSON normalization | [P1] |
| optional `/web/*` asset mount, `web.enabled`, `web.api_base_path` | [P2A] |
| `/api/v1/web/workbench/*` nesting + `OpenAiCompat` scope exception + optional `state.workbench` | [P2B] |
| `/api/v1/xaas/*` reserved scope row | [P2+] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | `state.workbench = None` 일 때 current trunk 는 `Config` 기반 400 을 반환한다. 의미상 503 이 더 자연스러울 수 있으나, 현재 구현/테스트/에러 taxonomy 는 400 에 맞춰져 있다. | A. current 400 유지 / B. gateway-local 503으로 승격 | A | 🟢 현행 trunk 계약 문서화 완료. 변경은 별도 설계 PR |
| Q-2 | `/api/v1/xaas/*` scope rule 은 이미 middleware table 에 있지만 route subtree 는 단계적으로만 land 될 수 있다. | A. reserved mapping 유지 / B. route land 시점까지 제거 | A | 🟢 D-7 namespace contract 에 따라 reserved 유지 |

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. wide Sprint draft 와 recent P2B/P2A code 사이에 비어 있던 current router/scope contract 를 독립 문서로 분리.

**체크리스트**:
- [x] 최근 `origin/main` 변화 반영
- [x] 기존 wide draft 와 current trunk 차이 식별
- [x] D-6 / D-7 / D-11 / D-12 연결
- [ ] cross-review annotations

### Round 1 — 2026-04-18 — @devops-sre-lead @xaas-platform-lead
**결론**: Pass

**체크리스트**:
- [x] 인터페이스 계약
- [x] 크레이트 경계
- [x] 타입 중복
- [x] 에러 반환
- [x] 동시성
- [x] 의존성 방향
- [x] Phase 태그
- [x] 레거시 결정 준수

**Action Items**:
- A1: `/web/*` public shell 과 `/api/v1/web/workbench/*` authenticated plane 을 route matrix 와 walkthrough 양쪽에서 분리해 명시
- A2: `AppState` 의 optional fields(`pg_pool`, `no_db`, `tui_tx`, `workbench`)가 route behavior 에 미치는 영향을 §2.1/§2.3 에 명시

**다음 라운드 조건**: A1, A2 반영 후 Round 1.5 진행

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**:
- [x] 위협 모델
- [x] 신뢰 경계 입력 검증
- [x] 인증·인가
- [x] 시크릿 관리
- [x] 공급망
- [x] 암호화
- [x] 감사 로그
- [x] 에러 정보 누출
- [ ] LLM 특이 위협 — N/A: router/scope contract 문서로 모델 추론 surface 자체를 다루지 않음
- [x] 컴플라이언스 매핑
- [x] 사용자 touchpoint 워크스루
- [x] 에러 메시지 3요소
- [ ] CLI flag — N/A: CLI spec 자체는 범위 밖, 단 `--no-db` 영향만 문서화
- [x] API 응답 shape
- [x] config 필드
- [x] defaults 안전성
- [x] 문서 5분 경로
- [x] runbook playbook
- [x] 하위 호환
- [x] i18n 준비

**Action Items**:
- A1: 401/403/413/ready/workbench-not-wired 경로를 표로 고정해 어떤 응답이 OpenAI-shaped 이고 어떤 응답이 probe-shape 인지 명시
- A2: workbench exception ordering 이 privilege regression 방지 포인트임을 STRIDE 와 회귀 테스트 계획에 반복 명시

**다음 라운드 조건**: A1, A2 반영 후 Round 2 진행

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**:
- [x] 단위 테스트 범위
- [x] mock 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 성능 검증
- [x] 회귀 테스트
- [x] 테스트 데이터

**Action Items**:
- A1: 413 rewrite, workbench scope regression, `state.workbench = None` current behavior 를 각각 별도 회귀 항목으로 분리
- A2: property-based test 가 N/A 인 이유를 명시하고 deterministic route-prefix regression test 로 대체한다고 적시

**다음 라운드 조건**: A1, A2 반영 후 Round 3 진행

### Round 3 — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**:
- [x] Rust 관용구
- [x] 제로 비용 추상화
- [x] 제네릭 vs 트레이트 객체
- [x] 에러 전파
- [x] 수명주기
- [x] 의존성 추가
- [x] 트레이트 설계
- [x] 관측성
- [x] hot path
- [x] 문서화

**Action Items**:
- A1: `GatewayWorkbenchService` / `WorkbenchProjectionService` 를 core 로 올리지 않고 gateway leaf crate 에 남기는 이유를 D-12 원칙 관점에서 명시
- A2: current trunk 의 `Config -> 400` behavior 를 normative contract 로 과장하지 말고 "현행 구현 문서화" 로 표현

**다음 라운드 조건**: 없음

### 최종 승인 — 2026-04-18 — PM
Round 1 / 1.5 / 2 / 3 action item 반영 완료. 이 문서는 current `gadgetron-gateway` router assembly, route-group 분류, scope gate ordering, `/web` public shell, `/api/v1/web/workbench/*` exception 의 authoritative reference 이며, broader historical draft 와 workbench feature 문서 사이의 runtime seam 을 닫는다.
