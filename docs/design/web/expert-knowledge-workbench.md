# Gadgetron Expert Knowledge Workbench UI

> **담당**: @ux-interface-lead
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-web`, `gadgetron-gateway`, `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-penny`
> **Phase**: [P2A] / [P2B] / [P2C]
> **관련 문서**: `docs/00-overview.md`, `docs/architecture/glossary.md`, `docs/design/phase2/01-knowledge-layer.md`, `docs/design/phase2/02-penny-agent.md`, `docs/design/phase2/03-gadgetron-web.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/11-raw-ingestion-and-rag.md`, `docs/design/core/knowledge-plug-architecture.md`, `docs/manual/web.md`

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 기능이 해결하는 문제

현재 `/web` 경험은 `assistant-ui` 기본 채팅 셸에 가까워서, Gadgetron 의 본질인 지식 협업 플랫폼보다 "일반 사용자용 AI 채팅 데모"처럼 보인다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1` 은 Gadgetron 을 "지식 협업 플랫폼"으로 정의한다. Penny 는 중심이지만, Penny 자체가 제품의 전부는 아니다. 사용자는 Penny 의 답변만 보는 것이 아니라 다음을 함께 다뤄야 한다.

- 어떤 knowledge source 와 plug 가 현재 세션의 근거가 되는지
- 어떤 tool 이 호출되었는지
- 어떤 사실이 write-back 후보인지
- 현재 시스템이 healthy / degraded / blocked 중 어디에 있는지
- 권한 승인이나 ACL 이 어떤 제약을 주는지
- 어떤 bundle / plug / gadget 이 자기 전용 시각 surface 를 기여하는지

즉, Penny 는 유일한 제어 표면이 아니라 **하나의 제어 표면**이다. 전문가 사용자는 대화로 조작할 수도 있고, 같은 capability 를 bundle viewer 에서 직접 조작할 수도 있어야 한다.

따라서 Web UI 의 정체성도 "예쁜 채팅 앱"이 아니라 **전문가용 knowledge workbench** 여야 한다.

이 문서는 `docs/design/phase2/03-gadgetron-web.md` 의 embed / CSP / build 파이프라인 설계를 뒤집지 않는다. 대신 그 위에 올라가는 사용자 경험 계약을 고정한다. 즉:

- `03-gadgetron-web.md` 는 **전달 수단**
- 이 문서는 **제품 표면과 정보 구조**

를 정의한다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 설명 | 채택하지 않은 이유 |
|------|------|--------------------|
| A. 현재 채팅 UI 에 색/카드만 손보기 | 가장 빠르다 | 구조가 그대로여서 여전히 싸구려 AI 데모 느낌이 남고, 지식 계층/근거/실패 상태가 보이지 않는다 |
| B. 운영 콘솔형 대시보드로 전면 전환 | 상태/메트릭은 잘 보인다 | Gadgetron 의 1차 사용 흐름은 Penny 와 지식 작업이므로, ops dashboard 가 전면에 오면 제품 정체성이 어긋난다 |
| C. 지식 작업대 중심의 3-panel workbench | 대화, 근거, 지식 상태, 권한을 함께 보여줄 수 있다 | 정보 밀도와 구현 난이도가 올라가지만 제품 철학과 가장 일치한다 |
| D. 위키 에디터 중심 UI | knowledge source 를 드러내기 쉽다 | Penny, tool trace, approval, runtime health 를 한 화면에서 보기가 어렵다 |

채택: **C. 지식 작업대 중심의 3-panel workbench**

### 1.4 핵심 설계 원칙과 trade-off

1. **Knowledge-first, not chat-first**
   대화는 중심 흐름이지만, 근거와 knowledge state 가 같은 위계로 보여야 한다.
2. **Evidence must stay adjacent**
   답변과 citations / tool trace / write-back 후보를 분리된 화면이나 modal 뒤에 숨기지 않는다.
3. **Operational clarity over AI theatrics**
   "멋져 보이는" 효과보다 상태 구분, 실패 복구, provenance 노출을 우선한다.
4. **Expert density with clean hierarchy**
   전문가 사용자는 높은 정보 밀도를 감당할 수 있다. 단, 정보 밀도는 곧잡음이 아니라 명확한 계층과 필터를 전제로 한다.
5. **Commandable surface**
   자연어 입력만이 아니라 slash command, 키보드 shortcut, saved panel state 같은 능동적 surface 를 제공한다.
6. **No AI-template aesthetics**
   과한 라운드 카드, 무의미한 보라/청록 glow, "무엇이든 물어보세요" 식 범용 카피, 데모풍 빈 상태를 금지한다.
7. **Failure is first-class**
   degraded / auth failure / knowledge stale / approval required 상태를 숨기지 않고, 원인과 복구 경로를 함께 보여준다.
8. **Capabilities may contribute views**
   weather, server, infra 같은 domain capability 는 자기 성격에 맞는 시각 surface 를 workbench 에 등록할 수 있어야 한다. 단, P2B 는 schema-driven view model 로 제한하고 arbitrary frontend code injection 은 허용하지 않는다.
9. **Dual-surface parity**
   Penny 가 다룰 수 있는 operator-facing control 은 bundle viewer 에서도 직접 실행 가능해야 한다. 반대로 UI 직접 조작은 Penny path 와 다른 privileged bypass 가 되어서는 안 된다.
10. **System captures, Penny curates**
   시스템은 이벤트의 사실성, 순서, 권한, audit, deterministic projection 을 소유한다. Penny 는 그 위에서 의미를 해석하고, 요약하고, 지식으로 승격하고, 다음 행동을 제안하는 더 주된 semantic steward 역할을 맡는다.

Trade-off:

- 일반 사용자에게는 다소 차갑고 학습 곡선이 높게 느껴질 수 있다.
- 그러나 현재 주 사용자는 전문가에 가깝고, Gadgetron 의 본질도 지식 작업이기 때문에 이 trade-off 는 의도적으로 수용한다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

이 설계의 공개 surface 는 네 층으로 나뉜다.

- [P2A] 기존 `/v1/chat/completions`, `/v1/models`, `/health` 를 유지하면서 프런트엔드 구조만 재구성
- [P2B] workbench shell 이 안정적으로 표시할 read model 을 `gadgetron-gateway` 에 추가
- [P2B] bundle / plug / gadget 이 기여하는 capability view registry 를 추가
- [P2B] Penny 와 동일 capability 를 직접 실행하는 action registry 를 추가

#### 2.1.1 [P2B] Gateway read-model API

```rust
// crates/gadgetron-gateway/src/web/workbench.rs

use axum::{extract::{Path, State}, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use gadgetron_core::{
    error::GadgetronError,
    workbench::{
        WorkbenchActionDescriptor,
        WorkbenchActionResult,
        WorkbenchActivityEntry,
        WorkbenchViewDescriptor,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchHealthState {
    Healthy,
    Degraded,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchBootstrapResponse {
    pub instance_label: String,
    pub api_base_path: String,
    pub default_model: String,
    pub health: WorkbenchHealthState,
    pub degraded_reasons: Vec<String>,
    pub canonical_knowledge_plug: String,
    pub search_plugs: Vec<String>,
    pub relation_plugs: Vec<String>,
    pub pending_approvals: u32,
    pub pending_writebacks: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeIndexStatus {
    pub plug: String,
    pub healthy: bool,
    pub stale: bool,
    pub last_indexed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchKnowledgeStatusResponse {
    pub canonical_plug: String,
    pub write_consistency: String,
    pub indexes: Vec<KnowledgeIndexStatus>,
    pub acl_mode: String,
    pub last_ingest_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchCitation {
    pub title: String,
    pub locator: String,
    pub source_kind: String,
    pub freshness: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchToolTrace {
    pub tool_name: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchWritebackCandidate {
    pub path: String,
    pub summary: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRequestEvidenceResponse {
    pub request_id: Uuid,
    pub citations: Vec<WorkbenchCitation>,
    pub tool_traces: Vec<WorkbenchToolTrace>,
    pub writeback_candidates: Vec<WorkbenchWritebackCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRegisteredViewsResponse {
    pub views: Vec<WorkbenchViewDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRegisteredActionsResponse {
    pub actions: Vec<WorkbenchActionDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeWorkbenchActionRequest {
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeWorkbenchActionResponse {
    pub result: WorkbenchActionResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActivityResponse {
    pub entries: Vec<WorkbenchActivityEntry>,
}

pub async fn get_workbench_bootstrap(
    State(state): State<GatewayState>,
) -> Result<Json<WorkbenchBootstrapResponse>, GadgetronError>;

pub async fn get_workbench_knowledge_status(
    State(state): State<GatewayState>,
) -> Result<Json<WorkbenchKnowledgeStatusResponse>, GadgetronError>;

pub async fn get_workbench_request_evidence(
    State(state): State<GatewayState>,
    Path(request_id): Path<Uuid>,
) -> Result<Json<WorkbenchRequestEvidenceResponse>, GadgetronError>;

pub async fn list_workbench_views(
    State(state): State<GatewayState>,
) -> Result<Json<WorkbenchRegisteredViewsResponse>, GadgetronError>;

pub async fn list_workbench_actions(
    State(state): State<GatewayState>,
) -> Result<Json<WorkbenchRegisteredActionsResponse>, GadgetronError>;

pub async fn invoke_workbench_action(
    State(state): State<GatewayState>,
    Path(action_id): Path<String>,
    Json(request): Json<InvokeWorkbenchActionRequest>,
) -> Result<Json<InvokeWorkbenchActionResponse>, GadgetronError>;

pub async fn list_workbench_activity(
    State(state): State<GatewayState>,
) -> Result<Json<WorkbenchActivityResponse>, GadgetronError>;
```

Route contract:

- `GET /api/v1/web/workbench/bootstrap`
- `GET /api/v1/web/workbench/knowledge-status`
- `GET /api/v1/web/workbench/requests/:request_id/evidence`
- `GET /api/v1/web/workbench/views`
- `GET /api/v1/web/workbench/actions`
- `POST /api/v1/web/workbench/actions/:action_id`
- `GET /api/v1/web/workbench/activity`

Authentication:

- 기존 Bearer auth 를 그대로 사용한다.
- 새 read-model endpoint 는 default-deny 이며, `/health` 처럼 public endpoint 로 취급하지 않는다.

Error contract:

- 응답 shape 는 기존 관리 API 와 동일하게 `{ error: { message, type, code } }` 를 유지한다.
- `KnowledgeErrorKind`, `WikiErrorKind`, `PennyErrorKind` 의 구체 문자열은 operator-facing 이되 secret / path leak 는 금지한다.

#### 2.1.2 [P2B] Capability surface registry contract

분산 단위는 gadget 이 아니라 **bundle** 이다. Bundle 이 plug / gadget 을 묶어 배포 단위가 되기 때문이다. 다만 descriptor 에는 어떤 plug / gadget 이 이 surface 를 대표하는지 메타데이터로 남긴다.

핵심 원칙:

- bundle viewer 는 Penny 와 별도의 특권 경로가 아니다
- direct UI action 과 Penny gadget 은 같은 capability 를 여는 두 개의 front door 여야 한다
- 두 경로 모두 동일한 `AuthenticatedContext`, approval gate, audit log, knowledge reflection 경로를 사용한다
- bundle author 는 business logic 를 gadget 쪽과 UI 쪽에 중복 구현하지 않는다
- 시스템은 immutable event 와 deterministic state projection 을 먼저 만든다
- Penny 는 그 이벤트를 읽고 semantic reflection 과 follow-up orchestration 을 주도한다

```rust
// crates/gadgetron-core/src/workbench.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    error::GadgetronError,
    mcp::auth::AuthenticatedContext,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchViewPlacement {
    LeftRail,
    CenterTab,
    RightPane,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchRendererKind {
    StatGrid,
    StatusBoard,
    TimeSeries,
    Table,
    Timeline,
    Markdown,
}

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchViewData {
    pub view_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActionPlacement {
    ViewToolbar,
    ViewInline,
    RightPane,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActionKind {
    Button,
    ConfirmButton,
    Form,
    Toggle,
    Select,
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
    pub input_schema: Value,
    pub destructive: bool,
    pub requires_approval: bool,
    pub knowledge_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActivityOrigin {
    UserDirect,
    Penny,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActivityKind {
    Prompt,
    ToolCall,
    DirectAction,
    Approval,
    KnowledgeWriteback,
    StatusChange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActivityEntry {
    pub id: Uuid,
    pub origin: WorkbenchActivityOrigin,
    pub kind: WorkbenchActivityKind,
    pub title: String,
    pub summary: String,
    pub request_id: Option<Uuid>,
    pub audit_event_id: Option<Uuid>,
    pub knowledge_candidate_ids: Vec<Uuid>,
    pub canonical_knowledge_paths: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeCandidateDisposition {
    PendingPennyDecision,
    PendingUserConfirmation,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeCandidate {
    pub id: Uuid,
    pub source_action_id: Option<String>,
    pub source_activity_id: Uuid,
    pub summary: String,
    pub proposed_path: Option<String>,
    pub disposition: KnowledgeCandidateDisposition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActionResult {
    pub action_id: String,
    pub status: String,
    pub summary: String,
    pub approval_id: Option<Uuid>,
    pub audit_event_id: Uuid,
    pub activity_event_id: Uuid,
    pub knowledge_candidates: Vec<KnowledgeCandidate>,
    pub refresh_view_ids: Vec<String>,
}

#[async_trait]
pub trait WorkbenchViewProvider: Send + Sync {
    fn descriptors(&self) -> Vec<WorkbenchViewDescriptor>;

    async fn load(
        &self,
        actor: &AuthenticatedContext,
        view_id: &str,
    ) -> Result<WorkbenchViewData, GadgetronError>;
}

#[async_trait]
pub trait WorkbenchActionProvider: Send + Sync {
    fn descriptors(&self) -> Vec<WorkbenchActionDescriptor>;

    async fn invoke(
        &self,
        actor: &AuthenticatedContext,
        action_id: &str,
        args: Value,
    ) -> Result<WorkbenchActionResult, GadgetronError>;
}
```

규칙:

- P2B 는 arbitrary JS / React component injection 을 금지한다
- bundle 은 `WorkbenchRendererKind` + JSON payload 를 등록한다
- web shell 은 first-party renderer 집합으로만 이를 렌더링한다
- direct action 은 같은 bundle 이 제공한 service call 을 타야 하며, 동일 capability 의 gadget 과 비즈니스 로직을 공유해야 한다
- direct action 은 항상 append-only audit event 와 `WorkbenchActivityEntry` 를 남긴다
- direct action 결과가 장기 참고 가치가 있으면 시스템은 `KnowledgeCandidate` 를 생성하되 disposition 은 `PendingPennyDecision` 또는 `PendingUserConfirmation` 으로만 시작한다
- Penny 는 이후 turn 에 recent activity / knowledge reflection 을 참고할 수 있어야 한다

예:

- `weather` bundle
  - view: `weather.current` -> `StatusBoard`
  - view: `weather.hourly` -> `TimeSeries`
  - action: `weather.refresh_now` -> `Button`
- `server` bundle
  - view: `server.fleet` -> `StatusBoard`
  - view: `server.events` -> `Timeline`
  - action: `server.restart_service` -> `ConfirmButton`
- `infra` bundle
  - view: `infra.gpu-capacity` -> `StatGrid`
  - view: `infra.deployments` -> `Table`
  - action: `infra.scale_deployment` -> `Form`

#### 2.1.3 [P2A] Frontend route contract

`/web` 는 여전히 기본 진입점으로 유지한다. 라우팅은 늘리지 않고, shell 안에서 정보 구조를 재배치한다.

```tsx
// crates/gadgetron-web/web/app/page.tsx

export default function WorkbenchPage(): React.ReactElement;
```

화면 surface:

- `TopStatusStrip`
- `LeftWorkspaceRail`
- `ActivityStream`
- `EvidencePane`
- `CommandComposer`
- `FailurePanel`
- `CapabilityViewHost`
- `CapabilityActionHost`

#### 2.1.4 [P2A] Client preference schema

서버 설정과 별개로, workbench 는 사용자별 panel state 를 origin-scoped `localStorage` 에 저장한다.

```ts
export type WorkbenchDensity = "compact" | "comfortable";
export type WorkbenchRightPane = "evidence" | "sources" | "writeback";

export interface WorkbenchPrefs {
  density: WorkbenchDensity;
  rightPane: WorkbenchRightPane;
  leftRailCollapsed: boolean;
  showReasoning: boolean;
  showToolDetails: boolean;
}
```

검증 규칙:

- 잘못된 JSON 이거나 enum 값이 아니면 전체를 버리고 기본값으로 복구
- API key 와 panel prefs 는 서로 다른 storage key 로 분리
- prefs 는 prompt/response 본문을 저장하지 않는다

### 2.2 내부 구조

#### 2.2.1 [P2A] 화면 정보 구조

데스크톱 기본 레이아웃은 3-panel workbench 이다.

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Status strip: model | health | canonical plug | pending approvals         │
├───────────────┬──────────────────────────────────────┬─────────────────────┤
│ Left rail     │ Activity stream                      │ Evidence pane       │
│               │                                      │                     │
│ - Workspace   │ - request / response timeline        │ - citations         │
│ - Bundles     │ - tool / reasoning blocks            │ - tool trace        │
│ - Plugs       │ - write-back callouts                │ - write-back queue  │
│ - Views       │ - capability view + action host      │ - registered view   │
│ - Shortcuts   │ - failure panels                     │ - knowledge status  │
├───────────────┴──────────────────────────────────────┴─────────────────────┤
│ Command composer: slash commands, prompt, send, draft hints               │
└────────────────────────────────────────────────────────────────────────────┘
```

패널 역할:

- **TopStatusStrip**: 서버 건강 상태, 선택 모델, canonical knowledge plug, pending approval, pending write-back 를 한 줄에 요약
- **LeftWorkspaceRail**: 제품이 "채팅창"이 아니라 knowledge workspace 임을 각인시키는 정적 구조. bundle / plug / saved view / keyboard shortcut entry 를 둔다
- **ActivityStream**: 사용자 입력, Penny 응답, reasoning, tool 호출, failure, approval-required, write-back suggestion 을 시간순으로 보여주는 주 작업 영역
- **EvidencePane**: 선택된 응답 혹은 request 를 기준으로 citation / tool trace / knowledge status / write-back queue 를 탭 전환 없이 보여준다
- **CommandComposer**: 일반 텍스트 입력 + slash command + draft persistence + explicit action surface
- **CapabilityViewHost**: 선택된 bundle/plug/gadget 이 제공하는 registered view 를 workbench 내부 규칙으로 렌더링하는 영역
- **CapabilityActionHost**: 선택된 view 와 bundle 이 제공하는 direct action 을 toolbar / form / confirm surface 로 노출하고, 실행 결과를 activity/knowledge/evidence 에 다시 반영하는 영역

#### 2.2.2 [P2A] 프런트엔드 파일 분해

기존 `app/page.tsx` 단일 파일 구조를 해체해 책임을 분리한다.

```text
crates/gadgetron-web/web/app/
├── page.tsx
├── globals.css
├── components/
│   ├── shell/
│   │   ├── workbench-shell.tsx
│   │   ├── top-status-strip.tsx
│   │   ├── left-workspace-rail.tsx
│   │   ├── evidence-pane.tsx
│   │   ├── capability-view-host.tsx
│   │   ├── capability-action-host.tsx
│   │   └── failure-panel.tsx
│   ├── capability-views/
│   │   ├── stat-grid-view.tsx
│   │   ├── status-board-view.tsx
│   │   ├── time-series-view.tsx
│   │   ├── table-view.tsx
│   │   ├── timeline-view.tsx
│   │   └── markdown-view.tsx
│   ├── capability-actions/
│   │   ├── action-bar.tsx
│   │   ├── action-form.tsx
│   │   ├── action-toggle.tsx
│   │   └── action-confirm-dialog.tsx
│   ├── activity/
│   │   ├── activity-stream.tsx
│   │   ├── activity-message.tsx
│   │   ├── reasoning-block.tsx
│   │   ├── tool-trace-block.tsx
│   │   └── writeback-callout.tsx
│   └── composer/
│       ├── command-composer.tsx
│       ├── slash-command-list.tsx
│       └── draft-indicator.tsx
├── lib/
│   ├── workbench-prefs.ts
│   ├── workbench-bootstrap.ts
│   ├── request-evidence.ts
│   ├── registered-views.ts
│   ├── registered-actions.ts
│   ├── workbench-activity.ts
│   ├── error-presenter.ts
│   └── layout-mode.ts
```

규칙:

- route 파일은 orchestration 만 담당하고, 렌더링 로직은 `components/` 로 이동
- assistant-ui runtime adapter 는 유지하되, workbench shell 이 그 위에 상태 surface 를 씌운다
- data fetch 와 rendering 을 섞지 않는다
- capability view 는 descriptor 와 payload 를 받아 first-party renderer 로만 그린다
- capability action 은 descriptor 와 JSON schema 를 받아 first-party action renderer 로만 그린다

#### 2.2.3 [P2A] 상태 모델

UI 전체는 다음 상태 머신으로 본다.

```text
Booting
  -> NeedsApiKey
  -> Ready
Ready
  -> RunningRequest
  -> Degraded
  -> HardFailure
RunningRequest
  -> Ready
  -> Degraded
  -> HardFailure
Degraded
  -> Ready
  -> HardFailure
HardFailure
  -> Ready
```

상태 의미:

- `Booting`: localStorage / health / bootstrap read model 초기 로딩
- `NeedsApiKey`: API key 부재. 단순 입력 카드가 아니라 origin / 저장 범위 / 회수 경로를 함께 설명
- `Ready`: 정상 작업 가능
- `RunningRequest`: Penny 응답 중. 상태 strip 과 activity stream 에 즉시 반영
- `Degraded`: 일부 기능은 가능하지만 knowledge index stale, gateway 503, approval backlog 등 운영상 주의 필요
- `HardFailure`: 채팅 입력을 막거나, 요청 자체를 전송할 수 없는 상태

별도 capability view 상태:

- `ViewIdle`
- `ViewLoading`
- `ViewReady`
- `ViewError`

개별 view 오류는 shell 전체를 죽이지 않는다. 예를 들어 weather view 가 깨져도 chat/evidence 는 계속 동작해야 한다.

별도 direct action 상태:

- `ActionReady`
- `ActionSubmitting`
- `ActionAwaitingApproval`
- `ActionSucceeded`
- `ActionFailed`

direct action 이 성공하면 activity stream 에 `DirectAction` entry 가 추가되고, evidence pane 이 즉시 다시 조회된다. knowledge 승격 여부는 그 다음 Penny 판단 단계에서 정해진다.

#### 2.2.4 [P2A] 반응형 전략

- `>= 1440px`: 3-panel 고정
- `1024px - 1439px`: left rail 축소, evidence pane 는 360px 고정 폭
- `< 1024px`: activity stream 우선. left/evidence pane 는 drawer 또는 segmented panel 로 전환
- `< 640px`: status strip 은 2줄까지 허용하지만 중요 순서는 `health > model > canonical plug > approvals`

모바일은 데스크톱을 단순 압축하지 않는다. 가장 중요한 작업 단위인 `현재 응답`, `현재 실패`, `현재 근거`를 먼저 유지하고, 부가 패널은 접는다.

#### 2.2.5 [P2A] 시각 시스템

폰트:

- 본문/레이블: `IBM Plex Sans KR` self-host
- 메타데이터/코드/경로: `JetBrains Mono` 유지

색채:

- 기본 테마는 **paper-light** 로 시작한다. 배경은 완전한 흰색이 아니라 약간 따뜻한 중립색
- dark mode 는 제공하되 기본값으로 두지 않는다
- 강조색은 상태 중심으로만 사용
  - info: 청록/청회색 계열
  - caution: amber
  - failure: oxide red
  - success: muted green

금지 규칙:

- 광범위한 radial glow
- 보라색 계열 브랜드 없는 하이라이트
- 카드마다 큰 radius 를 주는 consumer-SaaS 미감
- "AI", "magic", "smart" 같은 범용 브랜딩 문구

표면 규칙:

- 바탕은 조용하게, 정보는 선명하게
- 깊이는 그림자보다 border contrast 와 layer stacking 으로 표현
- 상태 변화는 색만이 아니라 label / icon / 위치 변화로도 표시

#### 2.2.6 [P2A] 상호작용 규칙

- slash command 는 `search`, `cite`, `pin`, `writeback`, `sources`, `help` 를 우선 지원
- `Cmd/Ctrl+K`: command palette
- `Cmd/Ctrl+.`: evidence pane focus
- `Cmd/Ctrl+Shift+L`: left rail collapse
- `Esc`: drawer / palette close

입력부 규칙:

- placeholder 는 범용 문구를 쓰지 않는다
- 기본 문구: `질문하거나 /command 를 입력하세요`
- draft 는 새로고침에 복원하되 전송 후 즉시 삭제

#### 2.2.7 [P2B] 근거와 지식 상태의 합류

`EvidencePane` 는 단순 citation list 가 아니라 request-scoped read model 을 보여준다.

패널 구성:

- `Sources`: citation title, locator, freshness, source kind
- `Tool Trace`: tool name, success/failure, duration, 요약
- `Write-back Queue`: 후보 문서 경로, 요약, pending/applied/rejected 상태
- `Knowledge Status`: canonical plug, search plug health, relation plug availability, ACL mode

`RequestEvidenceResponse` 는 메시지 버블에 흩어진 내부 metadata 를 한 곳에 모아 UI 가 안정적으로 렌더링할 수 있게 만든다.

#### 2.2.8 [P2B] 등록형 capability view

workbench 는 단일 고정 화면이 아니라, bundle 이 등록한 view 를 담는 컨테이너이기도 하다.

원칙:

- view 등록 주체는 bundle
- descriptor 는 `owner_bundle`, `source_kind`, `source_id` 를 반드시 명시
- 시각 언어는 shell 이 소유하고, bundle 은 데이터와 의도만 제공
- 한 view 는 left rail entry, center tab, right pane widget 중 하나 이상의 placement 를 가질 수 있다

예시:

- 날씨 gadget 을 가진 bundle 은 summary weather board 와 hourly chart 를 등록할 수 있다
- server monitoring plug 를 가진 bundle 은 host status board 와 incident timeline 을 등록할 수 있다
- graph knowledge plug 는 relation graph summary 를 right pane widget 으로 등록할 수 있다

왜 arbitrary microfrontend 를 바로 허용하지 않는가:

- CSP/Trusted Types 모델이 깨진다
- bundle 품질이 shell 전체 품질을 끌어내릴 수 있다
- version skew 와 hydration 오류가 커진다

따라서 P2B 는 typed renderer 만 허용하고, truly custom surface 가 필요하면 P3 에 sandboxed iframe/web-component 전략을 별도 설계한다.

#### 2.2.9 [P2B] direct control recognition loop

사용자가 bundle viewer 에서 직접 제어를 실행해도, 그것은 Penny 와 knowledge layer 바깥에서 일어나는 일이 아니다. 다만 순서는 `시스템 계측 순서` 그대로가 아니라, **capture plane** 과 **semantic plane** 을 구분해 설계해야 한다.

역할 분리:

- **시스템**
  - actor 확정
  - append-only audit event 기록
  - 현재 상태 projection 갱신
  - activity feed 생성
  - approval / ACL / idempotency / retry 같은 기계적 invariant 보장
- **Penny**
  - 어떤 이벤트가 장기 지식 가치가 있는지 판정
  - 요약/링크/후속질문/추천행동 생성
  - 기록할지 말지 결정하거나 사용자에게 물어봄
  - write-back candidate 또는 canonical note 승격 주도
  - 이후 대화에서 해당 사건을 문맥으로 재사용

권장 반응 루프:

1. 사용자가 direct action 실행
2. gateway 가 `AuthenticatedContext::from_auth_context(...)` 로 actor 확정
3. `WorkbenchActionProvider::invoke` 가 bundle service 호출
4. 시스템이 append-only audit event 와 `WorkbenchActivityEntry { origin = UserDirect, kind = DirectAction }` 를 즉시 생성
5. activity stream / evidence pane / registered view 가 먼저 refresh 된다
6. Penny 는 same-turn 또는 next-turn 문맥에서 이 activity event 를 즉시 볼 수 있다
7. deterministic 한 사실 변화는 상태 read model / index 에 즉시 반영된다
8. semantic reflection 이 필요한 경우 Penny 가 기록 여부를 결정하거나, 사용자에게 기록할지 묻는다
9. 기록 후보는 먼저 `KnowledgeCandidate` 로 남고, disposition 은 `PendingPennyDecision` 또는 `PendingUserConfirmation` 이다
10. Penny 또는 사용자가 기록을 승인하면 그때 `Accepted` 로 전환되고 canonical knowledge write 가 수행된다
11. Penny 또는 사용자가 기록하지 않기로 하면 disposition 은 `Rejected` 로 남고 canonical store 는 건드리지 않는다
12. 승격된 knowledge entry 는 이후 검색, citation, follow-up planning 에 재사용된다

즉, 기본 계약은 `viewer action -> system capture -> immediate Penny awareness -> deterministic state reflection -> semantic knowledge reflection` 이다.

반영 규칙:

- direct action 은 chat message 가 아니어도 activity stream 에 남는다
- Penny 가 수행한 tool call 과 동일한 축에서 정렬된다
- bundle 이 반환한 `knowledge_candidates` 는 evidence pane 에 먼저 노출되고, 실제로 `Accepted` 된 뒤에만 knowledge search/citation 대상이 된다
- Penny 는 별도 privileged memory 가 아니라 동일 knowledge/event surface 를 통해 이를 본다
- "무조건 자동 위키 기록"이 아니라, 사실 캡처와 의미 승격을 구분한다
- canonical knowledge 기록 여부의 최종 steward 는 Penny 또는 Penny 가 사용자에게 물어본 결과다

예:

- 사용자가 `server.restart_service` 버튼 클릭
- UI 는 결과 토스트만 띄우고 끝나지 않는다
- activity stream 에 `서비스 재시작 요청` entry 가 즉시 생성된다
- 상태 보드에는 서비스 상태 변화가 먼저 반영된다
- Penny 는 이후 turn 에 이 직접 제어 사실을 문맥으로 사용할 수 있다
- Penny 가 "이 변경을 운영 journal 에 남길까요?" 라고 물을 수 있다
- 사용자가 승인하면 그때 운영 journal / runbook note 후보가 canonical knowledge 로 남는다

#### 2.2.10 [P2C] knowledge plug 탐색 확장

`docs/design/core/knowledge-plug-architecture.md` 와 정합하게, UI 는 knowledge layer 를 단일 wiki 엔진으로 가정하지 않는다.

- canonical store 는 하나의 기본 truth 로 보이되
- search plug / relation plug 는 별도 badge 와 status 로 분리
- graph/relation 탐색은 evidence pane 안의 별도 relation view 로 노출

즉, UI 는 "지식 레이어 = LLM Wiki" 가 아니라 "지식 레이어 = canonical store + derived/query plugs" 라는 모델을 시각적으로 반영한다.

### 2.3 설정 스키마

#### 2.3.1 [P2A] 서버 설정

P2A 에서는 새 `gadgetron.toml` 필드를 추가하지 않는다.

이유:

- visual density, pane open state, reasoning visibility 같은 항목은 인스턴스 전역 설정보다 사용자별 선호에 가깝다
- Phase 2A 목표는 정보 구조 교정이지 운영자 설정 축 확대가 아니다

#### 2.3.2 [P2B] 선택적 인스턴스 기본값

P2B 부터 운영자가 인스턴스 기본값을 제어할 수 있도록 `WebConfig` 하위에 선택 필드를 추가한다.

```toml
[web.workbench]
default_density = "compact"
show_reasoning_by_default = false
show_writeback_queue = true
show_relation_panel = false
enabled_surface_families = ["core", "knowledge", "ops"]
allow_direct_actions = true
```

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

- `default_density = "compact"`
- `show_reasoning_by_default = false`
- `show_writeback_queue = true`
- `show_relation_panel = false`
- `enabled_surface_families = ["core", "knowledge", "ops"]`
- `allow_direct_actions = true`

검증 규칙:

- enum 값이 아니면 config load 실패
- `show_relation_panel = true` 이면서 relation plug 가 하나도 없으면 경고 로그를 남기고 UI 에서는 비활성 badge 로 렌더
- `enabled_surface_families` 에 없는 bundle 의 descriptor 와 action 은 노출하지 않는다
- `allow_direct_actions = false` 이면 action descriptor 는 로드하되 UI 에서 disabled explanation 과 함께 숨김 처리한다
- 등록된 view descriptor 가 `renderer`/`placement` enum 을 벗어나면 해당 view 만 drop 하고 경고 로그를 남긴다

우선순위:

1. 서버 기본값
2. 사용자 localStorage override

### 2.4 에러 & 로깅

#### 2.4.1 에러 모델

P2A 는 새 Rust 에러 variant 를 추가하지 않는다. 기존 에러를 workbench 맥락에 맞게 해석한다.

- API key 오류: HTTP 401 -> `FailurePanel(kind = "auth")`
- 권한 부족: HTTP 403 -> `FailurePanel(kind = "forbidden")`
- Penny subprocess/stream 실패: `GadgetronError::Penny`
- knowledge/read-model 실패: `GadgetronError::Knowledge` 또는 `GadgetronError::Wiki`
- registered view payload load 실패: `CapabilityViewHost(kind = "view_error")`
- direct action invoke 실패: `CapabilityActionHost(kind = "action_error")`
- direct action approval 대기: `CapabilityActionHost(kind = "awaiting_approval")`
- 네트워크 단절/서버 종료: `FailurePanel(kind = "offline")`

사용자 메시지는 항상 3가지를 포함한다.

1. 무엇이 일어났는가
2. 왜 그런가
3. 사용자가 무엇을 해야 하는가

예:

- `인증이 거부되었습니다. 저장된 API 키가 만료되었거나 revoke 되었습니다. Settings 에서 새 gad_live_* 키를 저장하세요.`
- `지식 인덱스가 최신이 아닙니다. 응답은 계속 가능하지만 citation 정확도가 떨어질 수 있습니다. 재색인 상태를 확인하세요.`

#### 2.4.2 tracing

새 서버 read-model endpoint 는 다음 span 을 사용한다.

- `web.workbench.bootstrap`
- `web.workbench.knowledge_status`
- `web.workbench.request_evidence`
- `web.workbench.list_views`
- `web.workbench.load_view`
- `web.workbench.list_actions`
- `web.workbench.invoke_action`
- `web.workbench.activity`

필드:

- `request_id`
- `conversation_id` (해당 시)
- `knowledge_canonical_plug`
- `pending_approvals`
- `pending_writebacks`
- `degraded`
- `http.status_code`
- `view_id` (해당 시)
- `owner_bundle` (해당 시)
- `action_id` (해당 시)
- `activity_event_id` (해당 시)

로그 금지 항목:

- API key 원문
- prompt/response 본문 전체
- 로컬 파일 절대 경로
- citation 의 민감 원문 스니펫

#### 2.4.3 STRIDE 요약

| 자산 | 신뢰 경계 | 위협 | 완화 |
|------|-----------|------|------|
| API key (`localStorage`) | 브라우저 origin | Spoofing / theft | 전용 origin 유지, no inline script, CSP 유지, key 는 별도 storage key 에 저장 |
| workbench read-model 응답 | Browser ↔ Gateway | Tampering | TLS, Bearer auth, CSP, JSON schema 검증 |
| tool trace / citations | Penny / Knowledge ↔ Gateway | Repudiation | request_id 기반 append-only tracing, audit correlation 유지 |
| knowledge status | Gateway ↔ Knowledge plug | Information disclosure | operator-facing 메시지에만 plug id 노출, 민감 경로/본문 비노출 |
| bootstrap polling | Browser ↔ Gateway | DoS | low-frequency polling, exponential backoff, degraded mode 전환 |
| relation/write-back controls | Browser ↔ Gateway | Elevation of privilege | approval-required action 분리, default-deny endpoint, existing auth 재사용 |
| registered capability view payload | Bundle ↔ Gateway ↔ Browser | Tampering / XSS | arbitrary HTML 금지, typed JSON payload 만 허용, shell-owned renderer 만 사용 |
| direct bundle action | Browser ↔ Gateway ↔ Bundle service | Elevation of privilege / repudiation | `AuthenticatedContext` 강제, same approval gate, append-only audit, `origin = UserDirect` activity event |

### 2.5 의존성

Rust:

- [P2A] 추가 없음
- [P2B] 추가 없음. read-model endpoint 는 기존 `axum`, `serde`, `uuid`, `tracing` 만 사용

Frontend runtime:

- [P2A] 추가 없음. 기존 `Next.js`, `React`, `assistant-ui`, `lucide-react`, `shadcn` 만 사용

Frontend dev/test:

- `docs/design/phase2/03-gadgetron-web.md §22` 의 Vitest 기반 테스트 스택을 현재 패키지에 맞게 복원한다
- 새 라이브러리 추가보다 현재 스택 정리와 컴포넌트 분해를 우선한다

자산:

- `IBM Plex Sans KR` self-host font 파일 추가

정당화:

- 새 상태관리/애니메이션 라이브러리를 넣지 않고도 이번 범위는 충분히 구현 가능하다
- 런타임 의존성을 늘리지 않는 것이 bundle 크기와 CSP 운영에 유리하다

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 연결

```text
gadgetron-core
  ├─ WebConfig / WebWorkbenchConfig
  ├─ GadgetronError::{Penny, Wiki, Knowledge}
  ├─ WorkbenchViewProvider / WorkbenchActionProvider traits
  ├─ KnowledgeCandidate type
  └─ shared auth/config primitives
        │
        ▼
gadgetron-gateway
  ├─ /v1/chat/completions
  ├─ /v1/models
  ├─ /health
  └─ /api/v1/web/workbench/* + surface registry + activity feed
        │
        ▼
gadgetron-web
  ├─ embedded Next.js workbench shell
  ├─ assistant-ui runtime adapter
  ├─ evidence / status / failure surfaces
  ├─ registered capability view renderers
  └─ direct action host + candidate review UI
        │
        ├──────────────► gadgetron-penny (chat stream, tool trace source)
        └──────────────► gadgetron-knowledge (knowledge status, citation, canonical write target)
```

### 3.2 데이터 흐름 다이어그램

```text
Browser
  ├─ GET /web/*                                -> embedded assets
  ├─ GET /api/v1/web/workbench/bootstrap       -> shell state
  ├─ GET /api/v1/web/workbench/knowledge-status-> knowledge read model
  ├─ GET /api/v1/web/workbench/views           -> registered capability views
  ├─ GET /api/v1/web/workbench/actions         -> direct action registry
  ├─ GET /api/v1/web/workbench/activity        -> direct + Penny activity feed
  ├─ POST /v1/chat/completions                 -> Penny streaming response
  ├─ POST /api/v1/web/workbench/actions/:id    -> direct action invoke
  └─ GET /api/v1/web/workbench/requests/:id/evidence
                                                -> citation / tool / candidate / canonical summary
```

응답 흐름:

1. 페이지 로드 시 bootstrap + health 를 먼저 로드
2. 등록된 capability view descriptor 를 함께 로드
3. 등록된 direct action descriptor 와 recent activity 를 함께 로드
4. 사용자가 요청 전송 또는 direct action 실행
5. `/v1/chat/completions` 스트림 또는 action result 가 activity stream 에 렌더
6. request id / activity id 확정 후 evidence endpoint 호출
7. evidence pane 이 citation / tool trace / knowledge candidate / canonical summary 를 채운다
8. 사용자가 특정 registered view 선택 시 해당 payload 를 로드한다

### 3.3 타 도메인과의 인터페이스 계약

- `gateway-router-lead`
  - 새 read-model endpoint 의 auth / error shape / status code 계약 소유
- `dx-product-lead`
  - empty state / failure copy / settings copy / runbook 링크 문구 소유
- `bundle authors`
  - 자기 bundle 이 제공하는 view/action descriptor 와 payload shape 소유
- `security-compliance-lead`
  - localStorage, origin isolation, approval/ACL surface, trace redaction 검토
- `qa-test-architect`
  - Playwright + mocked gateway harness + deterministic 상태 fixture 정의
- `chief-architect`
  - `WebWorkbenchConfig` 배치, 에러 variant 재사용, D-12 경계 검토

### 3.4 D-12 크레이트 경계 준수 여부

준수 방침:

- UI shell 구현은 `gadgetron-web` 안에만 둔다
- gateway read-model handler 와 응답 struct 는 `gadgetron-gateway` 에 둔다
- 지식 저장/검색/graph 논리는 `gadgetron-knowledge` 와 plug 구현체에 남긴다
- bundle 이 구현하는 공용 surface contract (`WorkbenchViewProvider`, `WorkbenchActionProvider`, 관련 descriptor 타입) 만 `gadgetron-core` 에 둔다
- `KnowledgeCandidate` 같은 canonical-write 전 단계 타입은 `gadgetron-core` 또는 `gadgetron-knowledge` shared layer 에 둔다
- 공통 설정 타입이 필요할 때만 `gadgetron-core` 로 올린다

즉, 이번 설계는 UI 를 위해 knowledge 로직을 `gadgetron-web` 로 끌어오지 않는다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

Frontend:

- `workbench-prefs.ts`
  - 손상된 localStorage 값이 기본값으로 복구되는지
- `registered-views.ts`
  - descriptor enum 검증과 fallback drop 이 정확한지
- `registered-actions.ts`
  - direct action descriptor 의 kind/schema/approval 힌트가 올바르게 정규화되는지
- `error-presenter.ts`
  - 401/403/503/Knowledge stale 상태가 올바른 panel variant 로 매핑되는지
- `layout-mode.ts`
  - viewport 폭에 따라 3-panel / condensed / drawer 모드가 결정되는지
- `top-status-strip.tsx`
  - degraded badge 와 pending counts 가 우선순위대로 렌더되는지
- `evidence-pane.tsx`
  - citation / tool trace / write-back / empty 상태 전환이 올바른지
- `capability-view-host.tsx`
  - renderer kind 별로 올바른 first-party component 에 dispatch 되는지
- `capability-action-host.tsx`
  - action kind 별로 올바른 first-party control 에 dispatch 되는지
- `candidate-review-list.tsx`
  - `PendingPennyDecision` / `PendingUserConfirmation` / `Accepted` / `Rejected` 가 올바르게 구분 렌더되는지
- `failure-panel.tsx`
  - "무엇/왜/어떻게" 3요소를 항상 포함하는지

Gateway:

- `get_workbench_bootstrap`
  - healthy / degraded / blocked 계산이 정확한지
- `get_workbench_knowledge_status`
  - stale index 와 relation plug availability 가 정확히 직렬화되는지
- `get_workbench_request_evidence`
  - request id 기준 citation / tool / write-back aggregation 이 정확한지
- `list_workbench_views`
  - bundle 제공 descriptor 가 placement/renderer 규칙대로 노출되는지
- `list_workbench_actions`
  - bundle 제공 action descriptor 가 approval/direct-action 정책대로 노출되는지
- `invoke_workbench_action`
  - audit event, activity event, `KnowledgeCandidate` 생성이 기대대로 연결되는지
- `list_workbench_activity`
  - `UserDirect` 와 `Penny` origin 이 동일 타임라인에 정렬되는지

### 4.2 테스트 하네스

Frontend:

- Vitest + React component tests
- mock fetch 응답은 deterministic fixture 사용
- localStorage 는 test double 로 대체

Gateway:

- `axum` handler unit tests
- `GatewayState` 는 fake penny / fake knowledge read-model 로 주입

property-based test:

- `WorkbenchPrefs` enum parsing 은 제한된 입력 공간이라 property-based test 불필요
- activity ordering 과 candidate disposition 전이는 [P2B] 에 한해 property-based test 도입 가능

### 4.3 커버리지 목표

- Frontend unit: line 80% 이상, branch 70% 이상
- Gateway read-model: line 85% 이상, branch 80% 이상
- 실패/복구 경로는 happy path 와 동일 비중으로 커버

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

- `gadgetron-web` + `gadgetron-gateway`
- [P2B] 에서 `gadgetron-knowledge`, `gadgetron-penny` read model 포함

핵심 시나리오:

1. **첫 실행 / 키 없음**
   `/web` 접속 -> generic hero 없이 key + origin 설명 + 다음 단계 노출
2. **정상 요청**
   키 저장 -> 모델 확인 -> 질문 전송 -> status strip 이 running 으로 바뀌고 tool trace 가 activity/evidence 양쪽에 반영
3. **등록형 view**
   weather 또는 server fixture bundle 활성화 -> left rail 에 새 view 가 나타나고, 선택 시 status board / chart / table 로 렌더
4. **직접 제어 + Penny 인지**
   server fixture bundle 의 `restart_service` direct action 실행 -> activity feed 에 `UserDirect` entry 생성 -> Penny 후속 질문 또는 candidate review 가 이어짐
5. **기록 후보 승인**
   direct action 결과로 `KnowledgeCandidate` 생성 -> Penny 가 기록 여부를 묻거나 사용자가 승인 -> 그 뒤에만 canonical knowledge path 가 evidence/search 에 나타남
6. **백엔드 다운**
   `/v1/chat/completions` 404/503 -> 빈 assistant bubble 이 아니라 failure panel + 복구 가이드 렌더
7. **knowledge stale**
   bootstrap 또는 knowledge-status 가 stale 반환 -> warning badge + evidence pane 에 stale notice
8. **좁은 화면**
   mobile viewport 에서 left/evidence pane 이 drawer 로 전환되고 current response 와 failure 는 유지

### 5.2 테스트 환경

Frontend E2E:

- Playwright
- mocked gateway fixture server 또는 dev harness route

Backend integration:

- `cargo test -p gadgetron-gateway`
- fake penny runtime + fake knowledge service

수동 QA:

- `/web` 접속 시 visual shell 이 3-panel 구조를 유지하는지
- empty state 문구가 범용 AI 카피로 회귀하지 않았는지
- right pane 없이 단일 채팅 레이아웃으로 퇴행하지 않았는지
- direct action 직후 candidate 가 즉시 canonical knowledge 로 보이지 않는지
- Penny 승인 전에는 candidate 상태가 `PendingPennyDecision` 또는 `PendingUserConfirmation` 으로 남는지

### 5.3 회귀 방지

다음 변경은 테스트를 실패시켜야 한다.

- evidence pane 이 DOM 에서 사라짐
- degraded / blocked 상태가 단순 토스트로 축소됨
- 모바일에서 failure panel 이 입력창 아래로 밀려 보이지 않음
- empty state 에 범용 문구("무엇이든 물어보세요")가 재도입됨
- relation/search/canonical plug 구분이 UI 에서 사라짐
- registered view 가 arbitrary HTML 을 직접 렌더하기 시작함
- direct action 이 Penny approval/curation 없이 즉시 canonical knowledge 를 써버림

---

## 6. Phase 구분

### [P2A]

- 3-panel workbench shell
- status strip / left rail / evidence pane / failure panel
- typography / color / spacing 전면 재정비
- empty / failure / auth / offline 상태 교정
- localStorage 기반 workbench prefs

### [P2B]

- gateway workbench read-model endpoint 집합
- knowledge status / write-back queue / citation / candidate surface 정식화
- `WebWorkbenchConfig` 선택적 기본값
- registered capability view registry
- registered direct action registry
- `KnowledgeCandidate` lifecycle 와 Penny curation loop
- first-party typed renderer set (`StatGrid`, `StatusBoard`, `TimeSeries`, `Table`, `Timeline`, `Markdown`)

### [P2C]

- relation panel
- graph/query knowledge plug 상태 표시
- saved knowledge views / persistent workspace customization
- sandboxed custom capability surface 필요 시 별도 설계

---

## 7. 오픈 이슈 / 의사결정 필요

| ID  | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| - | 현재 오픈 이슈 없음 | - | - | - |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-18 — @gateway-router-lead @dx-product-lead
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
- A1: direct action 과 Penny gadget 이 동일 capability 의 dual-surface 임을 본문에 명시
- A2: bundle-registered view 만이 아니라 action registry 도 API surface 에 포함

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
- [x] runbook playbook
- [x] 하위 호환
- [x] i18n 준비

**Action Items**:
- A1: arbitrary microfrontend 금지와 typed renderer 제한을 보안 완화책으로 명시
- A2: direct action 이 approval/audit 을 우회하지 않음을 STRIDE 와 error model 양쪽에 반영

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
- A1: direct action -> Penny awareness -> candidate acceptance 흐름을 통합 시나리오에 추가
- A2: candidate 상태 전이와 action registry 렌더 테스트를 단위 테스트 항목에 추가

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
- A1: `KnowledgeCandidate` 를 canonical write 전 단계 타입으로 분리
- A2: 시스템 capture 와 Penny curation 의 책임 구분을 명시

**다음 라운드 조건**: 없음

### 최종 승인 — 2026-04-18 — PM
Round 1 / 1.5 / 2 / 3 액션 아이템 반영 완료. 문서는 구현 착수 전제의 승인 상태다.
