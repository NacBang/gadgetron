# Workbench Gateway Projection & Direct Action Wire-Up

> **담당**: @gateway-router-lead
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-19 — **ISSUE 8 functionally complete** (5 TASKs shipped): TASK 8.1 / PR #211 `Arc<ArcSwap<DescriptorCatalog>>` 플러밍 substrate, TASK 8.2 / PR #213 `POST /admin/reload-catalog` Management-scoped HTTP endpoint, TASK 8.3 / PR #214 `CatalogSnapshot { catalog, validators }` 번들링 — 핸들이 `Arc<ArcSwap<CatalogSnapshot>>` 로 확장되어 reload 가 catalog + pre-compiled JSON-schema validators 를 lockstep 으로 교체, TASK 8.4 / PR #216 file-based TOML source (`[web] catalog_path`), TASK 8.5 / PR #217 POSIX `SIGHUP` reloader (fs-watcher 는 TASK 8.6 로 deferred).
> **관련 크레이트**: `gadgetron-gateway`, `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-xaas`, `gadgetron-web`, `gadgetron-penny`
> **Phase**: [P2B] primary / [P2B-EPIC3] workbench admin sub-tree (catalog hot-reload, Management-scoped) / [P2C] subscriptions + incremental refresh
> **관련 문서**: `docs/design/gateway/wire-up.md`, `docs/design/gateway/route-groups-and-scope-gates.md` (scope gate ordering SSOT — admin sub-tree 포함), `docs/design/web/expert-knowledge-workbench.md`, `docs/design/core/knowledge-candidate-curation.md`, `docs/design/core/knowledge-plug-architecture.md`, `docs/design/phase2/08-identity-and-users.md`, `docs/design/phase2/09-knowledge-acl.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/12-external-gadget-runtime.md`, `docs/manual/api-reference.md` §POST /api/v1/web/workbench/admin/reload-catalog, `docs/reviews/pm-decisions.md`

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 기능이 해결하는 문제

`origin/main` 의 최신 상태는 이미 `W3-WEB-1` 3-panel shell 을 포함한다. 하지만 현재 trunk 에서 실제 gateway 계약은 여전히 다음 공백을 남긴다.

1. `crates/gadgetron-web/web/app/components/shell/status-strip.tsx` 는 `/api/v1/web/workbench/bootstrap` 이 P2B 에서 들어올 것을 전제로 하지만, `gadgetron-gateway` 에는 그 read model 조립 경로가 없다.
2. `docs/design/web/expert-knowledge-workbench.md` 는 `data_endpoint` 를 가진 registered view 를 정의하지만, 그 endpoint 를 누가 어떤 auth/scope 규칙으로 서빙하는지 gateway 문서가 없다.
3. `docs/design/core/knowledge-candidate-curation.md` 는 direct action 이 append-only capture, approval, candidate projection, Penny awareness, canonical writeback 과 연결돼야 한다고 규정하지만, 이 lifecycle 을 `/api/v1/web/workbench/actions/:action_id` 에 투영하는 구현 소유자가 없다.
4. 현재 `crates/gadgetron-gateway/src/middleware/scope.rs` 는 `/api/v1/*` 전체를 `Scope::Management` 로 묶는다. 그대로 두면 workbench read model 과 direct action 도 관리 전용 키를 요구하게 되고, 이는 "Penny 와 direct control 은 같은 capability 를 다른 surface 로 노출한다" 는 P2B 원칙과 충돌한다.

즉, 최근 web/knowledge 문서와 구현이 만든 가장 큰 문서 갭은 "UI shell 은 생겼고 domain contract 도 생겼는데, gateway 가 그것을 어떤 route, 어떤 auth, 어떤 projection, 어떤 error model 로 연결하는가" 이다. 이 문서는 그 seam 을 닫는다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1` 과 `docs/design/web/expert-knowledge-workbench.md` 가 정의한 Gadgetron 의 web surface 는 "채팅 앱"이 아니라 "지식 작업대"다. 따라서 gateway 는 단순 HTTP reverse proxy 가 아니라 다음 두 역할을 동시에 수행해야 한다.

- Penny 와 direct action, knowledge, approval, audit, runtime health 를 한 surface 로 묶는 projection 계층
- direct action 이 Penny path 와 다른 privileged bypass 가 되지 않도록 막는 policy choke-point

이 문서는 `docs/design/gateway/wire-up.md` 의 P1 미들웨어 체인을 대체하지 않는다. P1 문서가 **공통 auth/trace/body-limit 골격** 을 고정했다면, 이 문서는 그 위에 올라가는 **P2B workbench route group 과 actor-aware projection 규칙** 을 고정한다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 설명 | 채택하지 않은 이유 |
|---|---|---|
| A. `gadgetron-web` 이 bootstrap/activity/evidence/actions 를 여러 내부 endpoint 에서 직접 조립 | gateway 코드가 작아 보임 | auth/scope, error shape, stale/degraded 계산, approval/audit 연결이 프런트엔드와 각 서브시스템으로 분산된다 |
| B. Penny 스트림 하나만 authoritative source 로 사용 | 채팅 중심 UX 와 단순하게 맞음 | direct action, approval pending, system health, registered view payload 는 chat transcript 만으로 안정적으로 조립할 수 없다 |
| C. bundle 이 자기 HTTP endpoint 를 직접 `/api/v1/...` 에 등록 | bundle 자율성이 큼 | same-origin policy, scope 예외, view/action filtering, audit naming, error shape 가 번들마다 drift 한다 |
| D. gateway 가 workbench route group 을 소유하고, 내부 projection/action service 가 다른 모듈의 contract 를 조립 | route, auth, error, audit, approval, activity 를 한 곳에서 통제 가능 | gateway 내부 조립 계층이 하나 더 필요하다 |

채택: **D. gateway-owned projection and action wire-up**

### 1.4 핵심 설계 원칙과 trade-off

1. **Gateway projects, subsystems own truth**
   gateway 는 canonical knowledge, activity 사실 이벤트, approval 상태, bundle descriptor 의 truth 를 소유하지 않는다. 대신 그들을 actor-aware read model 로 조립한다.
2. **Workbench 는 `/api/v1/*` 이지만 관리 콘솔이 아니다**
   `/api/v1/web/workbench/*` 는 path prefix 상 관리 namespace 아래에 있지만, base scope 는 `Scope::OpenAiCompat` 다. finer-grained 제약은 descriptor metadata 와 downstream ACL/approval 이 담당한다.
3. **Descriptor listing must already be filtered**
   "보이면 써도 된다" 는 아니지만, "어차피 403 날 거니까 일단 다 보여준다" 도 금지한다. view/action descriptor 는 actor, enabled family, feature flag, runtime availability 를 반영한 뒤 UI 로 간다.
4. **Same capability, same policy**
   `POST /api/v1/web/workbench/actions/:action_id` 는 Penny gadget path 와 동일한 approval, audit, candidate capture, caller inheritance 규칙을 탄다.
5. **Same-origin typed view data**
   `WorkbenchViewDescriptor.data_endpoint` 는 임의의 bundle URL 이 아니라 gateway 소유의 same-origin route 로 정규화한다. P2B 는 typed payload 만 허용하고 arbitrary microfrontend 는 허용하지 않는다.
6. **Polling first, push later**
   P2B 는 deterministic polling read model 을 먼저 고정하고, subscription/WebSocket/SSE push 는 [P2C] 로 미룬다.
7. **Evidence is a first-class projection**
   direct action, Penny tool trace, citations, knowledge candidates 는 UI 가 내부 metadata 를 임의 해석해서 만든다기보다, gateway 가 안정된 `RequestEvidenceResponse` 로 조립해 준다.

Trade-off:

- gateway 가 단순 route tree 를 넘어 projection orchestration 을 갖게 된다.
- 그러나 이 복잡도를 gateway 에 모으는 편이 web, knowledge, Penny, bundle 들에 auth/policy/refresh 규칙을 흩뿌리는 것보다 훨씬 안전하다.

### 1.5 Operator Touchpoint Walkthrough

1. 사용자는 `/web` 에 접속하고 `gad_live_*` 또는 `gad_test_*` API key 를 저장한다.
2. shell 은 `GET /api/v1/web/workbench/bootstrap`, `GET /api/v1/web/workbench/activity`, `GET /api/v1/web/workbench/views`, `GET /api/v1/web/workbench/actions` 를 병렬로 호출한다.
3. 사용자가 left rail 에서 어떤 view 를 열면 shell 은 descriptor 의 `data_endpoint` 를 그대로 호출하는 것이 아니라, gateway 가 발급한 same-origin canonical route `GET /api/v1/web/workbench/views/:view_id/data` 를 호출한다.
4. 사용자가 direct action 을 누르면 gateway 는 actor 를 확정하고, 필요한 approval 을 걸고, 성공/대기/거부 결과를 `WorkbenchActionResult` 로 돌려주며, 동시에 audit/activity/candidate read model 을 갱신한다.
5. shell 은 `refresh_view_ids`, `activity_event_id`, `audit_event_id`, `request_id` 를 받아 evidence/activity/view data 를 재조회한다.
6. Penny 는 same-turn 또는 next-turn 문맥에서 이 direct action 사실을 activity feed 로 읽고, 필요하면 기록 여부를 묻거나 writeback candidate 를 승격한다.
7. 사용자는 degraded/stale/approval-required 상태를 failure panel 이나 status strip 에서 즉시 본다. hidden fallback 은 없다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

이 문서는 `expert-knowledge-workbench.md §2.1` 의 API 를 좁히고 보완한다. 핵심 보강점은 세 가지다.

1. `GET /api/v1/web/workbench/views/:view_id/data` 를 canonical data route 로 추가
2. `/api/v1/web/workbench/*` path 를 `Scope::Management` 예외로 두고 `Scope::OpenAiCompat` base scope 로 처리
3. descriptor metadata 에 actor-aware filtering 을 위한 최소 필드 추가

#### 2.1.1 Core descriptor refinement

`gadgetron-core::workbench` 의 descriptor 타입은 다음 필드를 additive 하게 확장한다.

```rust
use serde::{Deserialize, Serialize};

use crate::context::Scope;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchViewDescriptor {
    pub id: String,
    pub title: String,
    pub owner_bundle: String,
    pub source_kind: String,
    pub source_id: String,
    pub placement: WorkbenchViewPlacement,
    pub renderer: WorkbenchRendererKind,
    pub data_endpoint: String,
    pub refresh_seconds: Option<u32>,
    #[serde(default)]
    pub action_ids: Vec<String>,
    #[serde(default)]
    pub required_scope: Option<Scope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActionDescriptor {
    pub id: String,
    pub title: String,
    pub owner_bundle: String,
    pub source_kind: String,
    pub source_id: String,
    pub gadget_name: Option<String>,
    pub placement: WorkbenchActionPlacement,
    pub kind: WorkbenchActionKind,
    pub input_schema: serde_json::Value,
    pub destructive: bool,
    pub requires_approval: bool,
    pub knowledge_hint: String,
    #[serde(default)]
    pub required_scope: Option<Scope>,
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeWorkbenchActionRequest {
    pub args: serde_json::Value,
    #[serde(default)]
    pub client_invocation_id: Option<uuid::Uuid>,
}
```

설계 규칙:

- `required_scope = None` 은 `Scope::OpenAiCompat` base scope 만 요구한다.
- `required_scope = Some(Scope::Management)` 또는 `Some(Scope::XaasAdmin)` 인 descriptor 는 목록 단계에서 filter 된다.
- `disabled_reason` 은 actor privilege 문제가 아니라 instance config/runtime availability 때문이다.
- `client_invocation_id` 는 같은 actor/action 조합에 대한 duplicate click/retry dedupe 용이며, [P2B] 에서는 5분 TTL replay cache 로 처리한다.

#### 2.1.2 Gateway route and service contract

```rust
// crates/gadgetron-gateway/src/web/workbench.rs

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use gadgetron_core::{
    error::GadgetronError,
    identity::AuthenticatedContext,
    workbench::{
        InvokeWorkbenchActionRequest,
        InvokeWorkbenchActionResponse,
        WorkbenchActivityResponse,
        WorkbenchRegisteredActionsResponse,
        WorkbenchRegisteredViewsResponse,
        WorkbenchRequestEvidenceResponse,
        WorkbenchViewData,
    },
};

use crate::server::AppState;

#[derive(Clone)]
pub struct GatewayWorkbenchService {
    pub projection: Arc<dyn WorkbenchProjectionService>,
    pub actions: Arc<dyn WorkbenchActionService>,
}

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    #[serde(default = "default_activity_limit")]
    pub limit: u32,
}

fn default_activity_limit() -> u32 {
    50
}

#[async_trait]
pub trait WorkbenchProjectionService: Send + Sync {
    async fn bootstrap(
        &self,
        actor: &AuthenticatedContext,
    ) -> Result<WorkbenchBootstrapResponse, GadgetronError>;

    async fn knowledge_status(
        &self,
        actor: &AuthenticatedContext,
    ) -> Result<WorkbenchKnowledgeStatusResponse, GadgetronError>;

    async fn views(
        &self,
        actor: &AuthenticatedContext,
    ) -> Result<WorkbenchRegisteredViewsResponse, GadgetronError>;

    async fn view_data(
        &self,
        actor: &AuthenticatedContext,
        view_id: &str,
    ) -> Result<WorkbenchViewData, WorkbenchHttpError>;

    async fn actions(
        &self,
        actor: &AuthenticatedContext,
    ) -> Result<WorkbenchRegisteredActionsResponse, GadgetronError>;

    async fn activity(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<WorkbenchActivityResponse, GadgetronError>;

    async fn request_evidence(
        &self,
        actor: &AuthenticatedContext,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, GadgetronError>;
}

#[async_trait]
pub trait WorkbenchActionService: Send + Sync {
    async fn invoke(
        &self,
        actor: &AuthenticatedContext,
        action_id: &str,
        request: InvokeWorkbenchActionRequest,
    ) -> Result<InvokeWorkbenchActionResponse, WorkbenchHttpError>;
}

pub fn workbench_routes() -> Router<AppState> {
    Router::new()
        .route("/bootstrap", get(get_workbench_bootstrap))
        .route("/knowledge-status", get(get_workbench_knowledge_status))
        .route("/views", get(list_workbench_views))
        .route("/views/:view_id/data", get(load_workbench_view_data))
        .route("/actions", get(list_workbench_actions))
        .route("/actions/:action_id", post(invoke_workbench_action))
        .route("/activity", get(list_workbench_activity))
        .route("/requests/:request_id/evidence", get(get_workbench_request_evidence))
}

pub async fn get_workbench_bootstrap(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchBootstrapResponse>, WorkbenchHttpError>;

pub async fn get_workbench_knowledge_status(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchKnowledgeStatusResponse>, WorkbenchHttpError>;

pub async fn list_workbench_views(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchRegisteredViewsResponse>, WorkbenchHttpError>;

pub async fn load_workbench_view_data(
    State(state): State<AppState>,
    Path(view_id): Path<String>,
) -> Result<Json<WorkbenchViewData>, WorkbenchHttpError>;

pub async fn list_workbench_actions(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchRegisteredActionsResponse>, WorkbenchHttpError>;

pub async fn invoke_workbench_action(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
    Json(request): Json<InvokeWorkbenchActionRequest>,
) -> Result<Json<InvokeWorkbenchActionResponse>, WorkbenchHttpError>;

pub async fn list_workbench_activity(
    State(state): State<AppState>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<WorkbenchActivityResponse>, WorkbenchHttpError>;

pub async fn get_workbench_request_evidence(
    State(state): State<AppState>,
    Path(request_id): Path<Uuid>,
) -> Result<Json<WorkbenchRequestEvidenceResponse>, WorkbenchHttpError>;
```

`AppState` 확장:

```rust
#[derive(Clone)]
pub struct AppState {
    // existing fields...
    pub workbench: Option<Arc<GatewayWorkbenchService>>,
}
```

#### 2.1.3 `AppState` P2B observability matrix

14 cycle 을 거치면서 `AppState` 에 workbench / Penny shared-surface / candidate plane wiring 이 순차적으로 추가됐다. P2B-complete `AppState` 의 6 new fields 는 다음과 같다 (source: `crates/gadgetron-gateway/src/server.rs`):

| 필드 | 타입 | wiring 소스 | `Some` 의미 | `None` 의미 |
|---|---|---|---|---|
| `workbench` | `Option<Arc<GatewayWorkbenchService>>` | `build_workbench(knowledge_service, candidate_coordinator)` | workbench read endpoints live — `bootstrap` / `activity` / `views` / `actions` / `evidence` 전부 동작 | 5개 workbench endpoint 이 config-error(400) 응답 |
| `penny_shared_surface` | `Option<Arc<dyn PennySharedSurfaceService>>` | `build_penny_shared_context()` | Penny 가 `workbench.*` 가젯 dispatch 가능 (activity_recent / request_evidence / candidates_pending / candidate_decide) | 4개 가젯이 `ToolDenied` 반환 (graceful degrade) |
| `penny_assembler` | `Option<Arc<dyn PennyTurnContextAssembler>>` | 같은 helper | chat handler 가 매 turn `<gadgetron_shared_context>` 블록 주입 | PSL-1b 의 graceful-degrade branch — block 주입 skip, 원 request 그대로 전송 |
| `activity_capture_store` | `Option<Arc<dyn ActivityCaptureStore>>` | `build_candidate_plane(service, curation, pg_pool)` | capture_chat_completion 이 활동 이벤트 영속화 (Pg or InMemory) | capture hook fire-and-forget spawn 이 호출되지 않음 |
| `candidate_coordinator` | `Option<Arc<dyn KnowledgeCandidateCoordinator>>` | 같은 helper | Penny 가 candidate accept/reject 결정 기록 + materialize → `KnowledgeService::write` | `candidate_decide` 는 `ToolDenied`, `capture_action` spawn 생략 |
| `agent_config` | `Arc<AgentConfig>` (required, not Optional) | startup 에서 config 로드 | chat handler 가 `[agent.shared_context].enabled` 및 `digest_summary_chars` 등을 소비 | 존재하지 않는 상태 — `AppState::default()` 가 안전한 기본값 제공 |

**Boot-gating rule:**
- `knowledge_cfg.is_none()` → 모든 6 필드 default (None / Arc::new(Default)) — P1 legacy serve path 보존
- `knowledge_cfg.is_some() && !curation.enabled` → `workbench` + `agent_config` 만 wired, candidate plane 은 None
- `knowledge_cfg.is_some() && curation.enabled` → 6 필드 모두 wired (Pg backing if `pg_pool.is_some()`)

**Degraded mode:** `build_workbench(None, None)` 은 `None` 이 아니라 `Some(degraded_workbench)` 을 반환한다. 즉 `knowledge_service` 가 없어도 workbench endpoint 들은 mount 되며, bootstrap 응답이 `degraded_reasons = ["knowledge service not wired"]` 로 operator 에게 상태를 알린다. hidden 404 / unauthenticated bypass 가 되지 않도록 하는 PSL-1b graceful-degrade 계약의 일부 (D-20260418-19).

**pg_pool 없는데 curation.enabled=true:** serve startup 이 `tracing::warn!` 로 "활동 이벤트가 메모리에만 저장되고 restart 시 유실됩니다" 를 명시적으로 경고한다. hard-fail 은 아직 안 — dev 경로 (로컬 Postgres 없이 `gadgetron serve`) 를 보존하기 위함. operator 가 실제 prod 에서 이를 silent 하게 넘겨받지 않도록 `allow_inmemory_store` 옵트인 플래그로 후속 PR 에서 강화 예정 (drift-fix U-B, D-20260418-23).

경로/권한 규칙:

| Route | Base scope | 추가 제약 | 비고 |
|---|---|---|---|
| `GET /api/v1/web/workbench/bootstrap` | `OpenAiCompat` | actor 존재 필수 | shell 초기 상태 |
| `GET /api/v1/web/workbench/knowledge-status` | `OpenAiCompat` | knowledge ACL filter | stale/degraded 표시 |
| `GET /api/v1/web/workbench/views` | `OpenAiCompat` | `required_scope`, family filter | actor-aware descriptor 목록 |
| `GET /api/v1/web/workbench/views/:view_id/data` | `OpenAiCompat` | descriptor lookup + `required_scope` + provider ACL | same-origin data route |
| `GET /api/v1/web/workbench/actions` | `OpenAiCompat` | `required_scope`, family filter, availability | disabled_reason 포함 가능 |
| `POST /api/v1/web/workbench/actions/:action_id` | `OpenAiCompat` | descriptor lookup + `required_scope` + schema validation + approval + downstream ACL | direct action invoke |
| `GET /api/v1/web/workbench/activity` | `OpenAiCompat` | actor/tenant filtering | Penny + UserDirect feed |
| `GET /api/v1/web/workbench/requests/:request_id/evidence` | `OpenAiCompat` | actor/tenant/request correlation | request-scoped evidence |
| `POST /api/v1/web/workbench/admin/reload-catalog` | **`Management`** (admin sub-tree) | ArcSwap store on `descriptor_catalog` handle — 새 catalog 를 `into_snapshot()` 으로 validator 번들링한 뒤 `Arc<ArcSwap<CatalogSnapshot>>` 에 atomic 하게 교체 | ISSUE 8 TASK 8.2 / PR #213 endpoint + TASK 8.3 / PR #214 `CatalogSnapshot { catalog, validators }` 번들링 — catalog + pre-compiled JSON-schema validators 를 lockstep 으로 swap; OpenAiCompat 키는 403 |

`scope_guard_middleware` 수정 규칙 (2026-04-19 기준 trunk, ISSUE 8 TASK 8.2 / PR #213 반영 — 정본은 [`route-groups-and-scope-gates.md §2.1.3`](./route-groups-and-scope-gates.md)):

```rust
let required_scope: Option<Scope> = if path.starts_with("/v1/") {
    Some(Scope::OpenAiCompat)
} else if path.starts_with("/api/v1/xaas/") {
    Some(Scope::XaasAdmin)
} else if path.starts_with("/api/v1/web/workbench/admin/") {
    // ISSUE 8 TASK 8.2 (PR #213): catalog hot-reload + future bundle
    // install/uninstall. Must precede the broader workbench rule so
    // OpenAiCompat 키가 /admin/* 에 접근해 catalog 를 리로드할 수 없다.
    Some(Scope::Management)
} else if path.starts_with("/api/v1/web/workbench/") {
    Some(Scope::OpenAiCompat)
} else if path.starts_with("/api/v1/") {
    Some(Scope::Management)
} else {
    None
};
```

중요한 점:

- workbench 는 `/api/v1/` namespace 를 사용하지만, `management scope` 자체는 요구하지 않는다 — `/admin/` sub-tree 만 예외다 (EPIC 3 / ISSUE 8 TASK 8.2 이후).
- admin/ops 성격의 view/action 은 descriptor `required_scope` 로 개별 제어한다. 다만 catalog 자체를 교체하는 admin 작업 (reload/install/uninstall) 은 descriptor layer 가 아니라 path-prefix scope 로 격리한다 — catalog 가 없으면 descriptor 가 존재할 수 없으므로 descriptor-level scope 로는 self-reload 를 막을 수 없기 때문.
- 이 예외가 없으면 regular Penny operator 가 `/web` 에서 chat 은 가능하지만 workbench projection 은 볼 수 없는 모순이 생긴다.
- admin sub-tree 순서가 broader workbench rule 보다 먼저 매칭되어야 한다 — 순서가 뒤집히면 즉각 privilege regression. harness Gate 7q.2 가 OpenAiCompat 키로 `/admin/reload-catalog` 호출 → 403 을 검증하여 ordering regression 을 CI 에서 잡는다.

### 2.2 내부 구조

#### 2.2.1 파일 책임 분해

```text
crates/gadgetron-gateway/src/
├── server.rs                 -- workbench route group mount + AppState 확장
├── middleware/scope.rs       -- /api/v1/web/workbench/* scope 예외
└── web/
    ├── mod.rs
    ├── workbench.rs          -- routes + DTO re-export + WorkbenchHttpError
    ├── projection.rs         -- bootstrap/knowledge/activity/evidence 조립
    ├── catalog.rs            -- descriptor snapshot + family/scope filtering
    └── actions.rs            -- invoke flow + approval/audit/candidate capture
```

책임 경계:

- `projection.rs` 는 read-only projection 만 담당한다.
- `catalog.rs` 는 bundle registry 를 스캔해 actor-aware descriptor snapshot 을 만든다.
- `actions.rs` 는 schema validation, approval, audit, activity, candidate capture 를 한 트랜잭션적 flow 로 묶는다.
- `server.rs` 와 `scope.rs` 는 route tree 와 base scope 예외만 담당한다.

#### 2.2.2 Projection assembly

`GatewayWorkbenchService` 내부는 다섯 projector 로 나눈다.

1. **BootstrapProjector**
   - 입력: gateway health, `WebWorkbenchConfig`, knowledge topology, pending approval 수, pending candidate/writeback 수, default model, active surface family
   - 출력: `WorkbenchBootstrapResponse`
   - 캐시: `(tenant_id, user_id)` 기준 2초 TTL
2. **KnowledgeStatusProjector**
   - 입력: `KnowledgeService` health/readiness, canonical/search/relation plug 상태, ACL mode, 최근 ingest timestamp
   - 출력: `WorkbenchKnowledgeStatusResponse`
   - 캐시: 없음. 단일 요청 내에서만 memoize
3. **DescriptorCatalog**
   - 입력: `BundleRegistry`, `enabled_surface_families`, actor scope, bundle enabled state
   - 출력: filtered `WorkbenchViewDescriptor[]`, `WorkbenchActionDescriptor[]`
   - 저장: immutable snapshot 을 **`Arc<ArcSwap<CatalogSnapshot>>`** 로 보관 (EPIC 3 / ISSUE 8 — TASK 8.1 / PR #211 에서 `tokio::sync::RwLock<Arc<DescriptorSnapshot>>` → `Arc<ArcSwap<DescriptorCatalog>>` 로 전환, TASK 8.3 / PR #214 에서 `CatalogSnapshot { catalog, validators }` 로 rev — pre-compiled JSON-schema validators 가 catalog 와 함께 스냅샷에 번들되어 atomic swap 한 번으로 둘이 lockstep 으로 교체된다). 읽기 측 (`projection.rs` + `action_service.rs`) 은 매 요청마다 `load()` 로 `Arc<CatalogSnapshot>` 스냅샷을 획득하므로 in-flight 요청은 그 스냅샷 기준으로 마무리 (catalog 와 validator 양쪽 모두 같은 세대를 읽는다 — mismatched pair 관찰 불가), hot-reload 경로는 새 Catalog 를 만들어 `DescriptorCatalog::into_snapshot()` 으로 validator 를 컴파일해 묶은 뒤 `store(new)` 로 포인터를 atomic 하게 교체한다. RwLock 대비 writer 가 in-flight reader 를 block 하지 않는 `arc-swap` 의 hand-off 가 hot-reload 요구사항과 정확히 맞는다. **TASK 8.2–8.5 모두 shipped (ISSUE 8 functionally complete)**: TASK 8.2 / PR #213 — `POST /api/v1/web/workbench/admin/reload-catalog` Management-scoped endpoint 가 이 handle 에 `store()` 를 호출. TASK 8.3 / PR #214 — 위 `CatalogSnapshot` 번들링 으로 TASK 8.2 validator-rebuild 제한 해소. TASK 8.4 / PR #216 — file-based catalog source (`[web] catalog_path` + `DescriptorCatalog::from_toml_file()`) 로 operator 가 on-disk TOML edit 후 reload 가능, 파싱 실패 시 running snapshot 교체 안 됨. TASK 8.5 / PR #217 — POSIX `SIGHUP` trigger (`spawn_sighup_reloader()` Unix-only tokio task) 가 HTTP endpoint 와 동일한 `perform_catalog_reload()` 공유 helper 호출, `kill -HUP <pid>` 로 reload. fs-watcher 는 demand 에 따라 TASK 8.6 follow-up — SIGHUP 이 90% 사용례를 dep/background-thread 없이 커버하므로 우선 미구현.
4. **ActivityProjector**
   - 입력: Penny request timeline, direct action capture log, approval state changes, canonical writeback events
   - 출력: 시간 역순 `WorkbenchActivityEntry[]`
5. **EvidenceProjector**
   - 입력: request trace, citations, tool traces, direct action correlation, candidate projection, canonical path receipts
   - 출력: `WorkbenchRequestEvidenceResponse`

#### 2.2.3 View data route

`WorkbenchViewDescriptor.data_endpoint` 는 임의 문자열이 아니라 gateway canonical route 로 정규화한다.

규칙:

- 등록 단계에서 gateway 가 `"/api/v1/web/workbench/views/{view_id}/data"` 를 채운다.
- bundle/provider 는 자기 view data 를 직접 HTTP route 로 등록하지 않는다.
- view payload 는 `WorkbenchViewData { view_id, payload }` 로만 반환한다.
- payload 는 typed renderer 가 기대하는 shape 로 제한되며 raw HTML 은 금지한다.

이 설계가 필요한 이유:

- auth/scope/filtering 을 view data 요청에도 똑같이 적용할 수 있다.
- browser 가 bundle-specific path 나 cross-origin path 를 알 필요가 없다.
- tracing, metrics, error shape 가 모두 gateway 한 곳에 모인다.

#### 2.2.4 Direct action flow

`POST /api/v1/web/workbench/actions/:action_id` 는 다음 순서를 고정한다.

1. actor 를 `AuthenticatedContext` 로 확정한다.
2. descriptor snapshot 에서 action lookup 한다. 없으면 404.
3. `required_scope` 와 `disabled_reason` 을 확인한다.
4. `args` 를 descriptor `input_schema` 에 맞춰 검증한다.
5. `client_invocation_id` 가 있으면 `(tenant_id, user_id, action_id, client_invocation_id)` 키로 5분 replay cache 를 조회한다.
6. `requires_approval` 또는 destructive tier 규칙에 따라 approval gate 를 거친다.
7. 승인 완료 후 provider/action host 를 호출한다.
8. 결과를 audit event, activity entry, candidate hint capture 로 fan-out 한다.
9. `WorkbenchActionResult` 를 반환한다.
10. UI 는 `refresh_view_ids`, `activity_event_id`, `audit_event_id` 를 사용해 follow-up GET 을 호출한다.

approval pending 인 경우:

- provider 호출은 아직 일어나지 않는다.
- `WorkbenchActionResult.status = "pending_approval"` 을 반환한다.
- `approval_id`, synthetic `activity_event_id`, empty `knowledge_candidates`, conservative `refresh_view_ids` 만 채운다.

성공한 경우:

- `origin = UserDirect`, `kind = DirectAction` activity entry 가 반드시 남는다.
- knowledge candidate hint 가 있으면 `PendingPennyDecision` 또는 `PendingUserConfirmation` 으로 생성된다.
- Penny 는 이 activity 를 same-turn 또는 next-turn 에 읽을 수 있다.

#### 2.2.5 동시성 모델

- descriptor snapshot 은 read-heavy 이므로 write-on-refresh, read-many 구조를 사용한다.
- bootstrap cache 는 2초 TTL 이며 actor-scoped 이다. tenant 간 캐시 공유는 금지한다.
- activity/evidence 는 cache 없이 최신 값을 읽는다. stale hiding 보다 최신성 우선이다.
- replay cache 는 TTL 만료형 `moka` 또는 동등한 in-memory map 을 사용한다.
- action provider 호출은 gateway global mutex 를 사용하지 않는다. concurrency control 은 approval gate 와 대상 subsystem 이 담당한다.

#### 2.2.6 Config and feature gating

새 상위 TOML 섹션은 추가하지 않는다. gateway 는 이미 정의된 `[web.workbench]` 필드만 소비한다.

```toml
[web.workbench]
enabled_surface_families = ["core", "knowledge", "ops"]
allow_direct_actions = true
show_writeback_queue = true
show_relation_panel = false
```

gateway 가 읽는 의미:

- `enabled_surface_families`
  - descriptor catalog filter 기준
- `allow_direct_actions`
  - `false` 인 경우 `GET /actions` 는 descriptor 를 반환하되 `disabled_reason = "Direct actions are disabled by instance policy."` 를 채운다
  - 같은 경우 `POST /actions/:id` 는 `403 permission_error` 로 거부한다
- `show_writeback_queue`
  - bootstrap/status projection 에 pending writeback count 를 포함할지 결정
- `show_relation_panel`
  - relation view descriptor 노출 여부의 상위 gate

##### Orthogonal dual-gate: `required_scope` vs `allow_direct_actions`

이 두 gate 는 **서로 다른 계층에서 작동하며 결합되지 않는다** — 감사에서 "allow_direct_actions 가 Scope 에 bound 안됨" 으로 flag 될 수 있으나 spec-correct 이다 (drift-fix PR 4 / D-20260418-25 에서 close-out). 두 gate 는 다음과 같이 구분한다:

| Gate | Layer | Trigger | Effect | Rendered response |
|---|---|---|---|---|
| `required_scope` | **per-descriptor** (actor privilege) | actor 가 scope 를 보유하지 않음 | descriptor 가 **숨겨짐** (list 에서 strip) + direct `POST /actions/:id` 는 404 | descriptor 가 아예 없음 |
| `allow_direct_actions = false` | **per-instance** (config policy) | 인스턴스 정책으로 direct action 전역 비활성화 | descriptor 는 **노출되지만** `disabled_reason` 주입 + `POST /actions/:id` 는 403 | descriptor + `disabled_reason` + 403 forbidden |

Key rules (line 148 의 원칙 재확인):

- `disabled_reason` 은 **actor privilege 문제가 아니다** — instance config 또는 runtime availability 이슈에만 사용한다.
- actor 가 scope 를 보유하지 않은 경우 disabled_reason 을 채우지 말고 descriptor 자체를 strip 하라. disabled_reason 에 "you lack scope X" 를 쓰는 것은 privilege-disclosure 위반.
- 두 gate 는 AND 가 아니라 독립 적용 — actor 가 scope 를 가지더라도 `allow_direct_actions = false` 이면 `disabled_reason` + 403 으로 막는다.

검증 규칙:

- family 문자열이 등록되지 않은 bundle 을 가리켜도 config load 는 실패시키지 않는다. 단 warning log 를 남기고 해당 family descriptor 는 비어 있는 것으로 처리한다.
- `allow_direct_actions = false` 인 동안 direct action invoke 는 절대 approval gate 로 내려가지 않는다.
- hidden descriptor 에 대한 직접 `GET /views/:id/data` 또는 `POST /actions/:id` 시도는 404 또는 403 으로 종료한다. UI 가 알고 있던 오래된 descriptor id 가 새 정책에 의해 사라졌을 가능성을 고려한다.

### 2.3 설정 스키마

이 문서가 새 operator-facing config surface 를 만들지는 않는다. 대신 `docs/design/web/expert-knowledge-workbench.md §2.3.2` 의 `WebWorkbenchConfig` 중 gateway 가 실제 소비하는 하위 집합을 여기서 고정한다.

```rust
pub struct WebWorkbenchConfig {
    pub default_density: WorkbenchDensity,
    pub show_reasoning_by_default: bool,
    pub show_writeback_queue: bool,
    pub show_relation_panel: bool,
    pub enabled_surface_families: Vec<String>,
    pub allow_direct_actions: bool,
}
```

기본값:

- `show_writeback_queue = true`
- `show_relation_panel = false`
- `enabled_surface_families = ["core", "knowledge", "ops"]`
- `allow_direct_actions = true`

검증 규칙:

- `allow_direct_actions = false` 라도 descriptor catalog 자체는 생성한다. 단 disabled reason 을 반드시 채워야 한다.
- `show_relation_panel = true` 인데 relation engine 이 없으면 bootstrap 은 `degraded_reasons += ["relation panel configured but no relation plug is healthy"]` 를 추가한다.
- `enabled_surface_families` 에서 제외된 family 의 descriptor 는 list endpoint 에서 완전히 사라진다.
- internal limits (`activity_limit <= 100`, `evidence trace limit <= 64`, `bootstrap cache TTL = 2s`) 은 P2B 에서 코드 상수로 고정한다. operator tuning surface 는 [P2C] 이후 검토한다.

### 2.4 에러 & 로깅

#### 2.4.1 에러 모델

unknown view/action 404 와 같은 gateway-local case 는 `GadgetronError` 를 오염시키지 않기 위해 local wrapper 를 둔다.

```rust
pub enum WorkbenchHttpError {
    Core(GadgetronError),
    ViewNotFound { view_id: String },
    ActionNotFound { action_id: String },
}
```

OpenAI-shaped response 규칙:

| 상황 | HTTP | `type` | `code` | 사용자 메시지 |
|---|---:|---|---|---|
| view id 미등록/필터됨 | 404 | `invalid_request_error` | `workbench_view_not_found` | 요청한 view 가 현재 사용자에게 보이지 않거나 제거되었습니다. shell 을 새로고침하세요. |
| action id 미등록/필터됨 | 404 | `invalid_request_error` | `workbench_action_not_found` | 요청한 action 이 현재 사용자에게 보이지 않거나 제거되었습니다. shell 을 새로고침하세요. |
| `allow_direct_actions = false` | 403 | `permission_error` | `forbidden` | 이 인스턴스는 direct action 을 비활성화했습니다. Penny 대화 또는 관리자 정책을 확인하세요. |
| `required_scope` 불일치 | 403 | `permission_error` | `forbidden` | 현재 API key 에는 이 workbench 기능을 사용할 scope 가 없습니다. |
| schema validation 실패 | 400 | `invalid_request_error` | `workbench_action_invalid_args` | action 입력이 descriptor schema 와 일치하지 않습니다. 폼을 다시 확인하세요. |
| approval timeout/reject | 403 또는 504 | `permission_error` 또는 `server_error` | approval flow 표준 코드 | approval 상태를 다시 확인하세요. |
| knowledge backend/stale read 실패 | 기존 `KnowledgeErrorKind` 매핑 | 기존 매핑 | 기존 매핑 | backend health 와 index freshness 를 확인하세요. |

오류 메시지 3원칙:

- 무엇이 일어났는가
- 왜 그런가
- 사용자가 무엇을 해야 하는가

#### 2.4.2 tracing

필수 span/event:

- `web.workbench.bootstrap`
- `web.workbench.knowledge_status`
- `web.workbench.list_views`
- `web.workbench.load_view`
- `web.workbench.list_actions`
- `web.workbench.invoke_action`
- `web.workbench.activity`
- `web.workbench.request_evidence`

필수 필드:

- `tenant_id`
- `user_id`
- `api_key_id`
- `request_id`
- `view_id` 또는 `action_id`
- `owner_bundle`
- `required_scope`
- `approval_required`
- `client_invocation_id`
- `cache_hit` (`bootstrap` only)
- `candidate_count`
- `audit_event_id`

로그 보안 규칙:

- `args` 원문 전체를 trace field 에 남기지 않는다.
- evidence/tool trace/candidate summary 는 escaped text 로만 렌더하고 raw HTML 은 금지한다.
- `disabled_reason` 은 정책 설명용 짧은 문구만 허용한다. 내부 file path, SQL, secret ref, runtime token 을 포함하면 안 된다.

#### 2.4.3 STRIDE threat model

| 자산 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| workbench descriptor catalog | bundle registry -> gateway -> browser | Information Disclosure | `required_scope`, family filter, actor-aware list 단계 filtering |
| view payload | bundle/provider -> gateway -> browser | Tampering / XSS | same-origin route, typed renderer, raw HTML 금지, JSON payload 검증 |
| direct action args | browser -> gateway -> bundle service | Elevation of Privilege | `OpenAiCompat` base scope + descriptor `required_scope` + schema validation + approval gate + downstream ACL |
| action replay | browser retry / duplicate click | Repudiation / DoS | `client_invocation_id` replay cache, UI pending-state disable |
| approval id / audit id / request correlation | gateway -> UI -> follow-up fetch | Spoofing | actor/tenant-bound lookup, opaque UUID only, server-side ownership 재검증 |
| evidence projection | Penny/tool/bundle -> gateway -> browser | Information Disclosure / prompt injection reflection | escaped markdown/text only, no secret/log stderr raw dump, actor ACL filter |
| polling endpoints | browser -> gateway | DoS | bootstrap 2s cache, limit clamp, no N+1 per bubble, small payload cap |

컴플라이언스 매핑:

- SOC2 CC6.6: actor-aware route auth, same-origin view data, policy-based direct action
- SOC2 CC6.7: scope denial, approval decision, action invoke, writeback transition 전부 audit 가능
- GDPR Art.32: tenant/user-scoped projection filtering, least-privilege read/write paths

### 2.5 의존성

- `gadgetron-core`
  - existing `Scope`, workbench descriptor/result types, shared error taxonomy 재사용
- `gadgetron-gateway`
  - `tokio`, `axum`, `tower`, `moka` existing dependency 재사용
  - 신규 direct dependency: `jsonschema = "0.28"`
    - 이유: `InvokeWorkbenchActionRequest.args` 를 trust boundary 에서 descriptor `input_schema` 로 검증하기 위한 최소 의존성
    - 위치: `gadgetron-gateway` 전용. core 로 끌어올리지 않는다
- `gadgetron-knowledge`
  - `KnowledgeService`, index/relation status, candidate projection read model 소비
- `gadgetron-xaas`
  - approval store, audit writer, multi-user actor resolution/persistence
- `gadgetron-penny`
  - recent request/tool trace feed read model 제공

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 상위/하위 연결 구도

```text
gadgetron-web (/web shell)
        |
        v
gadgetron-gateway
  ├─ web::workbench routes
  ├─ scope exception for /api/v1/web/workbench/*
  ├─ projection service
  └─ action service
        |
        +--> gadgetron-core
        |     ├─ Scope
        |     ├─ Workbench* descriptor/result types
        |     └─ shared error taxonomy
        |
        +--> gadgetron-knowledge
        |     ├─ KnowledgeService
        |     ├─ index/relation health
        |     └─ candidate projection
        |
        +--> gadgetron-xaas
        |     ├─ auth/user resolution
        |     ├─ approval store
        |     └─ audit writer
        |
        +--> gadgetron-penny
              └─ recent request/tool trace projection
```

### 3.2 데이터 흐름 다이어그램

read path:

```text
browser /web
   |
   | GET /api/v1/web/workbench/bootstrap|views|actions|activity
   v
gateway auth + scope(OpenAiCompat)
   |
   v
GatewayWorkbenchService
   |-- bundle registry ----------> descriptor snapshot
   |-- knowledge service --------> plug/index health
   |-- xaas approval store ------> pending approvals
   |-- candidate projection -----> pending writebacks
   '-- penny runtime -----------> recent activity/tool traces
```

direct action path:

```text
browser POST /api/v1/web/workbench/actions/:id
   |
   v
gateway auth + scope(OpenAiCompat)
   |
   v
descriptor lookup + required_scope + schema validation
   |
   +--> approval gate (if required/destructive)
   |
   v
bundle/provider invoke
   |
   +--> audit writer
   +--> activity capture
   +--> candidate coordinator
   '--> refresh ids / evidence correlation
   |
   v
WorkbenchActionResult
```

### 3.3 타 모듈과의 인터페이스 계약

- `docs/design/web/expert-knowledge-workbench.md`
  - layout, empty/failure states, shell UX 의 authoritative source
  - 이 문서는 그 중 `gateway workbench endpoint 집합` 과 `load_view` 누락 seam 을 concrete route 로 고정한다
- `docs/design/core/knowledge-candidate-curation.md`
  - direct action -> activity -> candidate -> Penny curation -> canonical write 순서를 그대로 따른다
- `docs/design/phase2/09-knowledge-acl.md`
  - knowledge status, candidate visibility, evidence citations 는 actor/team/scope 필터를 준수해야 한다
- `docs/design/phase2/10-penny-permission-inheritance.md`
  - direct action 도 Penny 와 마찬가지로 caller inheritance 를 깨지 않는다
- `docs/design/gateway/wire-up.md`
  - body limit, request id, auth, trace 골격은 기존 문서가 authoritative 이고, 이 문서는 P2B route group 만 추가한다

### 3.4 D-12 크레이트 경계 준수 여부

| 책임 | 크레이트 | 이유 |
|---|---|---|
| `Scope`, workbench descriptor/result shared type | `gadgetron-core` | web/gateway/bundle 이 공통으로 쓰는 계약이기 때문 |
| route tree, scope exception, projection/action orchestration | `gadgetron-gateway` | HTTP surface 와 actor-aware 조립은 gateway 책임 |
| approval/audit persistence | `gadgetron-xaas` | multi-user identity, quota, audit 스택과 결합 |
| knowledge status, candidate/readback projection | `gadgetron-knowledge` | canonical/derived knowledge truth source |
| Penny request/tool trace projection | `gadgetron-penny` | assistant runtime event source |
| shell render, polling/requery choreography | `gadgetron-web` | browser UX 책임 |

새 core 타입은 additive metadata(`required_scope`, `disabled_reason`) 에 한정한다. gateway-specific route error 나 projector 구현 타입은 core 로 올리지 않는다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

필수 unit test:

- `scope_guard_maps_workbench_paths_to_openai_compat`
- `bootstrap_projection_marks_degraded_when_relation_panel_has_no_healthy_plug`
- `descriptor_catalog_filters_by_required_scope`
- `descriptor_catalog_filters_by_enabled_surface_family`
- `descriptor_catalog_populates_disabled_reason_when_direct_actions_disabled`
- `view_data_route_rejects_hidden_descriptor_with_404`
- `action_invoke_rejects_invalid_schema_before_provider_call`
- `action_invoke_replays_same_client_invocation_id_without_second_provider_call`
- `activity_query_limit_is_clamped_to_100`
- `request_evidence_orders_tool_traces_and_candidates_deterministically`
- `workbench_http_error_serializes_openai_shape`

### 4.2 테스트 하네스

- `MockWorkbenchProjectionService`
  - bootstrap/knowledge/activity/evidence canned responses
- `RecordingWorkbenchActionService`
  - provider call count, received args, returned `WorkbenchActionResult` 확인
- `FakeBundleRegistry`
  - family, required_scope, bundle enable/disable 조합 fixture
- `RecordingApprovalGate`
  - allow / pending / reject / timeout 분기 제어
- `InMemoryReplayCache`
  - `client_invocation_id` dedupe 검증
- `InMemoryCandidateProjection`
  - evidence projection 정렬과 candidate linking 검증

property-based test:

- actor scope 집합 × enabled family 집합 × descriptor metadata 조합에 대해 노출 descriptor 가 monotonic filter 를 만족하는지 검증
- activity/evidence ordering reducer 가 입력 순서와 무관하게 동일 출력 정렬을 만드는지 검증

### 4.3 커버리지 목표

- `crates/gadgetron-gateway/src/web/{workbench,projection,catalog,actions}.rs`
  - line coverage 90% 이상
  - branch coverage 85% 이상
- `crates/gadgetron-gateway/src/middleware/scope.rs`
  - `/api/v1/web/workbench/*` 예외 경로 100% line coverage
- replay cache / schema validation / 404 wrapper
  - branch coverage 90% 이상

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

함께 테스트할 크레이트:

- `gadgetron-gateway`
- `gadgetron-xaas`
- `gadgetron-knowledge`
- `gadgetron-web`
- `gadgetron-penny` (fake runtime projection only)

핵심 e2e 시나리오:

1. **OpenAI-compatible key can open workbench**
   - `Scope::OpenAiCompat` key 로 `bootstrap`, `views`, `actions`, `activity` 가 모두 200
2. **Workbench does not require management scope**
   - 같은 key 로 `/api/v1/nodes` 는 403 이지만 `/api/v1/web/workbench/bootstrap` 은 200
3. **Higher-scope descriptor is hidden**
   - regular key 에서는 `required_scope = Management` descriptor 가 list 에 없고 direct route 는 404
4. **Pending approval flow**
   - destructive action invoke -> `pending_approval` result -> provider call 없음 -> activity feed 에 approval placeholder 존재
5. **Successful direct action fan-out**
   - approve 후 action invoke -> audit row, `DirectAction` activity entry, candidate 생성, evidence endpoint 반영
6. **Direct actions disabled by policy**
   - `allow_direct_actions = false` -> action list 에 disabled_reason 포함 -> POST 403
7. **Knowledge stale signal reaches shell**
   - stale relation/index fixture -> bootstrap degraded + knowledge-status stale = true
8. **Web shell consumes live gateway routes**
   - Playwright 로 `/web` 실행 -> stub comment 제거 후 실제 bootstrap/actions/activity/evidence polling 이 DOM 에 반영

### 5.2 테스트 환경

- auth/audit/approval/candidate persistence:
  - `testcontainers` PostgreSQL
- bundle/view/action fixture:
  - in-process fake bundle registry with one `ops` family view, one `knowledge` family view, one management-only action
- Penny runtime:
  - fake tool trace/activity source
- browser e2e:
  - `build_router_with_web(...)` + Playwright

CI 재현성 규칙:

- wall-clock 의존이 있는 polling/replay-cache 테스트는 `tokio::time::pause()` 로 고정
- Playwright 는 mocked provider/knowledge runtime 로만 돌리고 외부 네트워크 금지
- approval 흐름은 deterministic fixture id 를 써서 UUID ordering flakes 를 막는다

### 5.3 회귀 방지

다음 변경은 반드시 이 테스트를 깨야 한다.

- `/api/v1/web/workbench/*` 가 다시 `Scope::Management` 로 빨려 들어가는 변경
- descriptor filtering 전에 list endpoint 가 raw bundle descriptor 를 그대로 노출하는 변경
- `data_endpoint` 가 arbitrary URL 을 허용하는 변경
- approval pending 인데 provider 가 먼저 실행되는 변경
- direct action 결과가 activity/evidence/candidate fan-out 없이 끝나는 변경
- `allow_direct_actions = false` 인데 POST 가 통과하는 변경
- hidden/removed descriptor id 에 대해 200 이 나오는 변경

---

## 6. Phase 구분

- [P2A]
  - 현재 shipped shell 의 `/health` polling + static plugs fixture
  - 이 문서의 full route group 구현 대상 아님
- [P2B]
  - `/api/v1/web/workbench/*` route group
  - `views/:view_id/data` canonical route
  - `/api/v1/web/workbench/*` -> `OpenAiCompat` scope 예외
  - actor-aware descriptor filtering
  - `client_invocation_id` dedupe
  - direct action -> approval/audit/activity/candidate fan-out
- [P2C]
  - bootstrap/activity/view data incremental refresh tokens
  - SSE/WebSocket subscription path
  - cross-tab sync, optimistic UI invalidation
- [P3]
  - sandboxed custom renderer/iframe/web-component 실험
  - collaborative multi-user presence/cursors

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | [P2C] push transport 를 무엇으로 둘 것인가 | A. short polling 유지 / B. SSE / C. WebSocket multiplex | **B** — activity/view/evidence 는 server push 가 유리하지만 P2B 는 polling 으로 먼저 고정 | ⚪ follow-up, P2B 비차단 |

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. 최신 `W3-WEB-1` shell 과 knowledge candidate 문서 사이의 gateway seam 을 독립 설계 문서로 분리.

**체크리스트**:
- [x] 최근 `origin/main` 변화 반영
- [x] gateway scope gap 식별
- [x] direct action + evidence + projection seam 포함
- [ ] review annotations

### Round 1 — 2026-04-18 — @xaas-platform-lead @ux-interface-lead
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
- A1: `/api/v1/web/workbench/*` 가 `/api/v1/* => Management` catch-all 에 빨려 들어가지 않도록 scope 예외를 명시
- A2: `data_endpoint` 의 same-origin canonical route 와 descriptor `required_scope` metadata 를 본문에 추가

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
- [x] LLM 특이 위협
- [x] 컴플라이언스 매핑
- [x] 사용자 touchpoint 워크스루
- [x] 에러 메시지 3요소
- [x] API 응답 shape
- [x] config 필드
- [x] defaults 안전성
- [x] 문서 5분 경로
- [x] runbook playbook
- [x] 하위 호환
- [x] i18n 준비

**Action Items**:
- A1: `client_invocation_id` 기반 replay cache 를 추가해 duplicate click/retry replay 를 막을 것
- A2: raw HTML 금지와 same-origin data route 원칙을 STRIDE 와 error/logging 양쪽에 명시할 것

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
- A1: `/api/v1/web/workbench/*` scope regression 을 명시적 통합 시나리오로 추가
- A2: approval pending 과 successful action fan-out 두 경로를 모두 통합 테스트 계획에 고정

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
- A1: gateway-local 404 (`WorkbenchHttpError`) 와 shared descriptor metadata (`required_scope`, `disabled_reason`) 의 경계를 본문에 명시
- A2: gateway-specific projector 타입이 `gadgetron-core` 로 새지 않도록 D-12 표를 보강

**다음 라운드 조건**: 없음

### 최종 승인 — 2026-04-18 — PM
Round 1 / 1.5 / 2 / 3 action item 반영 완료. 이 문서는 `workbench` route group 의 구현 착수 전제 문서이며, `docs/design/gateway/wire-up.md` 와 함께 gateway layer 의 P2B authoritative reference 로 사용한다.
