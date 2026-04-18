# 13 — Penny Shared Surface Awareness Loop

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-core`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-knowledge`, `gadgetron-web`
> **Phase**: [P2B] primary / [P2C] push refresh / [P3] learned prioritization
> **관련 문서**: `docs/design/phase2/02-penny-agent.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/core/knowledge-candidate-curation.md`, `docs/design/gateway/workbench-projection-and-actions.md`, `docs/design/web/expert-knowledge-workbench.md`, `docs/design/phase2/12-external-gadget-runtime.md`, `docs/process/04-decision-log.md`

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백

2026-04-18 기준 `origin/main` 은 이미 다음 세 가지를 갖고 있다.

1. `W3-WEB-1` 3-panel workbench shell 이 main 에 머지되었다.
2. `docs/design/gateway/workbench-projection-and-actions.md` 가 gateway read model, direct action, evidence route group 을 Approved 로 고정했다.
3. `docs/design/core/knowledge-candidate-curation.md` 가 direct action 이후의 capture plane, candidate disposition, canonical writeback 규칙을 Approved 로 고정했다.

하지만 Penny 쪽 문서는 여전히 subprocess/session/tool dispatch 중심이다. 즉, 최근 승인된 workbench/activity/candidate 계약이 **어떻게 Penny 의 다음 turn 문맥으로 들어오는지**, 그리고 그 과정에서 **숨겨진 사적 memory path 를 만들지 않는지**가 아직 authoritative 문서로 닫혀 있지 않다.

이 공백이 남아 있으면 네 가지 문제가 생긴다.

1. direct action parity 가 문구로만 존재하고, 실제로는 Penny 가 UI 직접 조작 사실을 모를 수 있다.
2. Penny awareness 를 구현하려고 할 때 `~/.gadgetron/penny` 나 Claude session resume 을 사실상의 private memory 로 오용할 유인이 생긴다.
3. workbench 와 Penny 가 서로 다른 activity/candidate projection 을 읽으면 operator 는 두 surface 중 무엇을 믿어야 하는지 알 수 없게 된다.
4. approval pending / degraded / materialization failed 상태가 Penny 응답에서 누락되면 "Knowledge-first, not chat-first" 원칙이 깨진다.

이 문서는 그 seam 을 닫는다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1`, `docs/design/web/expert-knowledge-workbench.md`, `docs/design/core/knowledge-candidate-curation.md` 가 합쳐서 규정하는 Gadgetron 의 핵심은 다음 한 문장으로 요약된다.

> **Penny 는 중심 surface 이지만, 진실의 소스는 shared activity / knowledge / approval surface 이다.**

즉, Penny 는 별도 사적 메모리 소유자가 아니라 다음 역할을 맡는다.

- shared surface 에서 사실을 읽는다
- pending candidate 의 의미를 해석한다
- 사용자에게 기록 여부를 묻거나 accept / reject / escalate 결정을 내린다
- direct action 과 Penny tool path 가 같은 capability lifecycle 위에 있음을 유지한다

이 문서는 `02-penny-agent.md` 를 대체하지 않는다. `02` 가 **subprocess lifecycle / Claude session / gadget dispatch / stderr redaction** 을 고정했다면, 이 문서는 그 위에 올라가는 **shared surface awareness contract** 를 고정한다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 설명 | 채택하지 않은 이유 |
|---|---|---|
| A. Penny 전용 private memory store 추가 | direct action 사실과 후보를 Penny 전용 DB/파일에 따로 축적 | workbench 와 진실의 소스가 갈라지고, cross-user isolation / audit / replay 설명이 어려워진다 |
| B. gateway 가 긴 prompt summary 만 매 turn 주입 | 구현은 단순 | deep-read / evidence drill-down / candidate disposition 변경을 전부 prompt formatting 에 의존하게 되어 typed contract 가 없다 |
| C. Penny 가 필요할 때마다 read gadget 만 호출 | source-of-truth 일관성은 좋음 | "다음 turn 에 바로 aware 해야 한다"는 parity 요구를 만족하지 못하고, 기본 응답 품질이 tool-calling heuristic 에 좌우된다 |
| D. per-turn bootstrap summary + typed deep-read gadget 조합 | 기본 awareness 와 drill-down 둘 다 확보 | 조립 계층이 하나 더 필요하지만 drift 와 hidden memory 를 동시에 막을 수 있다 |

채택: **D. per-turn bootstrap summary + typed deep-read gadget 조합**

### 1.4 핵심 설계 원칙과 trade-off

1. **Shared surface is the source of truth**
   Penny 는 activity, pending candidate, approval, evidence 의 truth 를 소유하지 않는다. truth 는 기존 workbench/candidate projection 이 소유한다.
2. **Every turn gets a fresh bootstrap**
   Claude session resume 가 켜져 있어도 shared context 는 매 요청마다 다시 조립한다. session store 는 대화 continuity 보조일 뿐, 사실 저장소가 아니다.
3. **Prompt carries only the digest, gadgets carry the detail**
   bootstrap 은 짧고 결정적인 digest 로 유지한다. 상세 근거나 disposition 변경은 typed gadget 으로만 수행한다.
4. **No silent degradation**
   shared context 일부가 비정상이면 Penny 는 계속 응답할 수 있어도, degraded 이유를 명시적으로 context 와 tracing 에 남긴다.
5. **Same fact, same policy, same audit**
   direct action 이든 Penny gadget 이든 같은 activity/candidate/audit chain 에 연결된다.
6. **Actor filtering happens before Penny sees anything**
   Penny 가 읽는 activity, candidate, evidence 는 전부 `AuthenticatedContext` 로 pre-filter 된 projection 이어야 한다.

Trade-off:

- gateway 와 Penny 사이에 bootstrap assembly 레이어가 하나 더 생긴다.
- 그러나 이 비용을 지불해야 workbench UI, CLI, Penny, future external runtime 이 같은 사실 모델을 공유할 수 있다.

### 1.5 Operator Touchpoint Walkthrough

1. 운영자가 `/web` 에서 direct action 을 실행한다.
2. gateway 는 action 결과를 audit, activity, candidate projection 으로 fan-out 한다.
3. 같은 사용자가 다음 turn 에 Penny 에게 "방금 뭐가 바뀌었지?" 라고 묻는다.
4. gateway 는 request auth 이후 `PennyTurnBootstrap` 을 새로 조립하고, 최근 activity / pending candidate / pending approval / degraded reason digest 를 Penny 입력 앞부분에 붙인다.
5. Penny 는 별도 사적 기억에 의존하지 않고 bootstrap 만으로 "직접 action 이 있었고, 후보가 pending 이며, 아직 canonical writeback 은 끝나지 않았다" 고 설명할 수 있다.
6. 더 자세한 근거가 필요하면 Penny 는 `workbench.request_evidence` 또는 `workbench.candidates_pending` gadget 을 호출한다.
7. Penny 가 candidate 를 accept/reject/escalate 하면 coordinator 가 same contract 로 disposition 을 갱신한다.
8. workbench 와 Penny 는 같은 projection 을 보므로, 사용자는 UI 와 chat 이 서로 다른 사실을 말하는 상황을 겪지 않는다.
9. shared context store 일부가 다운되어 있으면 Penny 응답 앞부분과 workbench 상태 strip 양쪽에 같은 degraded reason 이 나타난다. hidden fallback 은 없다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

이 문서의 공개 surface 는 세 층이다.

1. `gadgetron-core` 의 shared bootstrap / digest 타입
2. `gadgetron-gateway` 의 bootstrap assembler contract
3. `gadgetron-penny` 가 등록하는 typed deep-read / decision gadget contract

#### 2.1.1 Shared bootstrap types (`gadgetron-core`)

```rust
// gadgetron-core::agent::shared_context

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::GadgetronError,
    knowledge::candidate::{CandidateDecisionKind, KnowledgeCandidateDisposition},
    knowledge::AuthenticatedContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PennySharedContextHealth {
    Healthy,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyTurnBootstrap {
    pub request_id: Uuid,
    pub conversation_id: Option<String>,
    pub generated_at: DateTime<Utc>,
    pub health: PennySharedContextHealth,
    pub degraded_reasons: Vec<String>,
    pub recent_activity: Vec<PennyActivityDigest>,
    pub pending_candidates: Vec<PennyCandidateDigest>,
    pub pending_approvals: Vec<PennyApprovalDigest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyActivityDigest {
    pub activity_event_id: Uuid,
    pub request_id: Option<Uuid>,
    pub origin: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub source_bundle: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyCandidateDigest {
    pub candidate_id: Uuid,
    pub activity_event_id: Uuid,
    pub summary: String,
    pub disposition: KnowledgeCandidateDisposition,
    pub proposed_path: Option<String>,
    pub requires_user_confirmation: bool,
    pub materialization_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyApprovalDigest {
    pub approval_id: Uuid,
    pub title: String,
    pub risk_tier: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyCandidateDecisionRequest {
    pub candidate_id: Uuid,
    pub decision: CandidateDecisionKind,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyCandidateDecisionReceipt {
    pub candidate_id: Uuid,
    pub disposition: KnowledgeCandidateDisposition,
    pub canonical_path: Option<String>,
    pub materialization_status: Option<String>,
    pub activity_event_id: Uuid,
}

#[async_trait]
pub trait PennyTurnContextAssembler: Send + Sync {
    async fn build(
        &self,
        actor: &AuthenticatedContext,
        conversation_id: Option<&str>,
        request_id: Uuid,
    ) -> Result<PennyTurnBootstrap, GadgetronError>;
}
```

규칙:

- `PennyTurnBootstrap` 은 actor-filtered projection 이다. raw event row 나 secret-bearing fields 를 그대로 담지 않는다.
- `recent_activity` 는 newest-first 로 정렬한다.
- `pending_candidates` 는 `Accepted` / `Rejected` 완료 항목이 아니라 unresolved 및 materialization follow-up 이 필요한 항목만 담는다.
- `pending_approvals` 는 Penny 가 설명할 수 있어야 하는 operator-visible approval 만 담는다. raw approver secret / full rationale 전문은 담지 않는다.
- 현재 trunk 에는 `gadgetron-core::knowledge::AuthenticatedContext` placeholder 가 이미 존재한다. P2B landing 에서는 이 ZST carrier 를 `10-penny-permission-inheritance.md` 가 정의한 caller identity payload 로 승격하되, "모든 shared surface read/write 가 caller context 를 요구한다" 는 trait shape 는 유지한다.

#### 2.1.2 Gateway bootstrap assembly contract

```rust
// gadgetron-gateway::penny::shared_context

use async_trait::async_trait;
use uuid::Uuid;

use gadgetron_core::{
    agent::shared_context::PennyTurnContextAssembler,
    error::GadgetronError,
    knowledge::AuthenticatedContext,
    workbench::WorkbenchRequestEvidenceResponse,
};

#[async_trait]
pub trait PennySharedSurfaceService: Send + Sync {
    async fn recent_activity(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyActivityDigest>, GadgetronError>;

    async fn pending_candidates(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyCandidateDigest>, GadgetronError>;

    async fn pending_approvals(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyApprovalDigest>, GadgetronError>;

    async fn request_evidence(
        &self,
        actor: &AuthenticatedContext,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, GadgetronError>;

    async fn decide_candidate(
        &self,
        actor: &AuthenticatedContext,
        request: PennyCandidateDecisionRequest,
    ) -> Result<PennyCandidateDecisionReceipt, GadgetronError>;
}
```

설계 규칙:

- gateway 구현은 `docs/design/gateway/workbench-projection-and-actions.md` 가 이미 정의한 projection/action 서비스와 **같은 backing store / same filter rule** 을 재사용한다.
- Penny 전용 별도 DB 조회 경로를 만들지 않는다.
- assembler 는 `recent_activity`, `pending_candidates`, `pending_approvals` 세 read 를 병렬로 시도하되, 하나가 실패해도 전체 요청을 무조건 500 으로 만들지 않는다. 대신 `health = degraded` + `degraded_reasons += [...]` 로 표기한다.
- 단, `AuthenticatedContext` 가 없거나 actor filter 가 성립하지 않으면 fail-closed 한다. 이 경우 Penny request 자체를 진행하면 안 된다.

#### 2.1.3 Penny-facing typed gadget contract

Penny 는 bootstrap digest 만으로 충분하지 않을 때 다음 gadget 들을 사용한다.

| Gadget | Tier | 목적 |
|---|---|---|
| `workbench.activity_recent` | `Read` | 최근 shared activity 를 더 깊게 읽기 |
| `workbench.request_evidence` | `Read` | 특정 request 의 citation / tool trace / writeback candidate 조회 |
| `workbench.candidates_pending` | `Read` | pending candidate 상세 보기 |
| `workbench.candidate_decide` | `Write` | accept / reject / escalate 결정 기록 |

```rust
// gadgetron-penny::gadget::workbench_awareness

use async_trait::async_trait;
use serde_json::Value;

use gadgetron_core::agent::{
    GadgetError, GadgetProvider, GadgetResult, GadgetSchema,
};

pub struct WorkbenchAwarenessGadgetProvider<S> {
    actor: AuthenticatedContext,
    service: S,
}

#[async_trait]
impl<S> GadgetProvider for WorkbenchAwarenessGadgetProvider<S>
where
    S: PennySharedSurfaceService + Send + Sync + 'static,
{
    fn category(&self) -> &'static str { "workbench" }

    fn gadget_schemas(&self) -> Vec<GadgetSchema> { /* omitted */ }

    async fn call(
        &self,
        name: &str,
        args: Value,
    ) -> Result<GadgetResult, GadgetError> { /* uses self.actor-bound service */ }
}
```

추가 규칙:

- `WorkbenchAwarenessGadgetProvider` 는 process-global singleton 이 아니라 Penny request-local actor snapshot 을 가진 provider 로 바인딩한다.
- `workbench.candidate_decide` 는 candidate 를 authoritative 하게 overwrite 하지 않는다. 내부적으로는 `KnowledgeCandidateCoordinator::decide_candidate()` 로만 내려간다.
- `Accept` 는 canonical writeback 의 동의이지, wiki page write 성공 보장을 뜻하지 않는다.
- `materialization_status` 가 `failed` 이어도 disposition 은 rollback 하지 않는다. 이는 `knowledge-candidate-curation.md` 규칙을 그대로 따른다.

### 2.2 내부 구조

#### 2.2.1 Per-turn bootstrap assembly

요청 흐름:

```text
HTTP /v1/chat/completions
          |
          v
Auth + AuthenticatedContext
          |
          v
PennyTurnContextAssembler::build
   |            |             |
   |            |             +--> approval projection
   |            +-----------------> candidate projection
   +------------------------------> activity projection
          |
          v
PennyTurnBootstrap
          |
          v
PennyProvider::chat_stream
```

핵심 규칙:

1. `conversation_id` 가 있든 없든 bootstrap 은 **항상 재조립**한다.
2. `session_store` 나 Claude resume state 는 bootstrap 생략 근거가 될 수 없다.
3. bootstrap 입력은 동일 actor 가 workbench UI 에서 보게 될 projection 과 bit-identical 한 권한 경계를 사용한다.
4. assembler 는 request-local 이며 cross-request mutable cache 를 source of truth 로 취급하지 않는다.

#### 2.2.2 Prompt binding contract

Penny subprocess 로 넘어가기 직전, gateway 는 bootstrap 을 deterministic text block 으로 렌더링해 user turn 앞에 주입한다.

형식:

```text
<gadgetron_shared_context>
generated_at: 2026-04-18T14:03:00Z
health: degraded
degraded_reasons:
- pending approval store unavailable; counts may be stale
recent_activity:
- [direct_action] Restarted server bundle "prod-web" 4m ago
- [knowledge_writeback] Accepted runbook candidate for /ops/incidents/redis-timeout 12m ago
pending_candidates:
- [pending_user_confirmation] "Restart outcome should be recorded as runbook delta"
pending_approvals:
- [destructive] Drain node gpu-a100-07
</gadgetron_shared_context>
```

렌더링 규칙:

- summary 는 `digest_summary_chars` 로 잘라서 prompt 폭주를 막는다.
- 없음은 생략하지 말고 `recent_activity: []` 형태로 명시한다.
- degraded 이유는 반드시 plain language 로 적는다.
- raw JSON, 내부 file path, DB key, stack trace 는 넣지 않는다.

#### 2.2.3 Deep-read and decision flow

Penny 의 shared surface 사용 경로는 두 단계다.

1. **Bootstrap digest** — 기본 awareness 보장
2. **Typed gadget read/write** — 필요한 경우 drill-down 또는 disposition 변경

예시:

- "방금 뭐 바뀌었어?" → bootstrap 만으로 대답 가능
- "그 재시작 근거를 보여줘" → `workbench.request_evidence`
- "이 후보 기록해도 돼?" → `workbench.candidates_pending`
- "그 후보는 받아들이고, 이유는 운영 절차 반복 방지라고 남겨" → `workbench.candidate_decide`

중요:

- Penny 는 hidden file 또는 Claude local memory 를 조회해서 fact 를 복원하지 않는다.
- same-turn tool output 은 이미 Claude context 에 있으므로 bootstrap 은 pre-turn awareness 에만 집중한다.
- next-turn awareness 는 previous turn output 이 아니라 shared projection 재조회로 얻는다.

#### 2.2.4 Session resume와 shared surface의 관계

`docs/design/phase2/02-penny-agent.md` 의 session resume 는 유지한다. 다만 의미는 다음으로 제한한다.

- Claude 측 대화 continuity
- tool reasoning style continuity
- short-lived conversational convenience

아래는 금지한다.

- session resume 를 activity truth 로 사용하는 것
- `~/.gadgetron/penny/work` 를 operator 사실 저장소처럼 다루는 것
- "지난 turn 에 Claude 가 기억하겠지" 를 이유로 bootstrap 을 생략하는 것

즉:

> **resume 는 conversational convenience 이고, shared surface 는 operator truth 다.**

#### 2.2.5 Failure behavior

세 failure class 를 구분한다.

1. **Identity failure**
   `AuthenticatedContext` 조립 실패, tenant/user parse 실패, actor filter 불가능. 이 경우 fail-closed. Penny 요청 자체를 거부한다.
2. **Partial shared-surface failure**
   activity/candidate/approval 중 일부 read 실패. 이 경우 chat 요청은 계속 진행 가능하되, bootstrap `health = degraded` 와 명시적 `degraded_reasons` 를 주입한다.
3. **Decision path failure**
   `workbench.candidate_decide` 또는 `workbench.request_evidence` 가 실패. 이는 tool error 로 surface 하고, 원인과 remediation 을 포함한다.

degraded continuation 규칙:

- degraded 는 "조용히 무시"가 아니라 "제한된 awareness 로 계속 진행"이다.
- Penny 는 degraded 문맥을 숨기지 않는다.
- workbench status strip 과 Penny bootstrap 은 같은 degraded reason 문자열을 재사용해야 한다.

### 2.3 설정 스키마

새 top-level product feature toggle 는 만들지 않는다. direct action parity 와 shared surface awareness 는 optional 기능이 아니라 P2B Penny contract 일부다.

대신 additive tuning subsection 만 허용한다.

```toml
[agent.shared_context]
enabled = true                     # emergency-only rollback switch; see below
bootstrap_activity_limit = 6
bootstrap_candidate_limit = 4
bootstrap_approval_limit = 3
digest_summary_chars = 240
require_explicit_degraded_notice = true
```

필드 규칙:

- `enabled`
  - default: `true`
  - **Emergency rollback switch only.** `false` 로 설정하면 gateway 의 chat-completions handler 가 bootstrap 조립을 완전히 건너뛴다 — `<gadgetron_shared_context>` 블록이 주입되지 않고 Penny 는 shared surface 를 인식하지 못한 채 응답한다. "tuning" 으로 쓰는 것은 금지이며, PSL-1b 이 도입한 이유는 오직 "prod 에서 bootstrap 이 잘못 조립되어 사용자 요청을 깨고 있을 때 즉시 OFF 하기 위해서" 다. `require_explicit_degraded_notice` 와는 의미가 다르다 — 후자는 "No silent degradation" 을 강제하는 invariant, 전자는 incident 운영 놉 (knob).
  - 검증: `true` / `false` 모두 허용 (enforcement 는 operator-visible WARN 으로 대체 — `false` 설정 시 tracing::warn! 로 "Penny awareness is disabled" 경고)
  - Ref: D-20260418-16 (PSL-1b).
- `bootstrap_activity_limit`
  - default: `6`
  - 검증: `1..=20`
- `bootstrap_candidate_limit`
  - default: `4`
  - 검증: `1..=12`
- `bootstrap_approval_limit`
  - default: `3`
  - 검증: `0..=10`
- `digest_summary_chars`
  - default: `240`
  - 검증: `80..=512`
- `require_explicit_degraded_notice`
  - default: `true`
  - `false` 는 허용하지 않는다. 설정 파일에 들어오면 startup validation error 로 거부한다.

이유:

- operator 가 limit 조정은 할 수 있어야 한다.
- 하지만 degraded notice 를 끄는 것은 "No silent degradation" 원칙과 충돌한다.
- `enabled` 는 feature toggle 이 아니라 **incident 운영 switch** 다. P2B Penny contract 는 shared surface awareness 를 필수로 요구하지만, 버그가 prod 에 landed 된 순간 즉시 OFF 할 수 있는 경로가 없으면 blast radius 를 제어할 수 없다. doc 13 §1.3 대안 A 가 금지한 "silent private memory" 와 혼동하지 말 것 — `enabled = false` 는 Penny 가 shared surface 를 못 보게 만들 뿐이고, shared surface 자체 (workbench / audit / approval) 는 여전히 동작한다.

### 2.4 에러 & 로깅

#### 2.4.1 Error model

- bootstrap build partial failure: chat 자체는 지속 가능. `PennyTurnBootstrap.health = degraded`
- bootstrap build identity failure: `GadgetronError::Auth` 또는 동등 fail-closed 에러
- `workbench.request_evidence`: 없는 request 또는 actor 불가시 `404` 또는 `403`
- `workbench.candidate_decide`: validation 실패 `400`, 권한 실패 `403`, coordinator/materialization 문제 `500`
- Penny-facing tool error mapping 은 기존 `PennyErrorKind::{ToolDenied, ToolInvalidArgs, ToolExecution}` 경로를 재사용한다

모든 operator-facing 에러는 다음 세 가지를 답해야 한다.

1. 무엇이 실패했는가
2. 왜 지금 그 요청을 완료할 수 없는가
3. 사용자가 어떻게 복구하거나 다시 시도해야 하는가

#### 2.4.2 tracing / audit

필수 span / event:

- `penny_turn_bootstrap_build`
  - fields: `request_id`, `conversation_id`, `activity_count`, `candidate_count`, `approval_count`, `health`
- `penny_shared_context_partial_failure`
  - fields: `request_id`, `component`, `reason_code`
- `penny_candidate_decision`
  - fields: `request_id`, `candidate_id`, `decision`, `materialization_status`
- `penny_shared_context_rendered`
  - fields: `request_id`, `rendered_bytes`, `degraded`

감사 규칙:

- bootstrap read 자체는 별도 audit event 남발을 만들지 않는다.
- candidate decision 은 append-only decision event 와 audit trail 양쪽에 기록한다.
- degraded continuation 은 security/ops incident correlation 을 위해 structured tracing 으로 남긴다.

#### 2.4.3 STRIDE 요약

| 자산 / 경계 | 위협 | 설명 | 완화 |
|---|---|---|---|
| shared activity digest | Spoofing | 다른 사용자의 activity 를 Penny bootstrap 에 주입 | `AuthenticatedContext` 기반 pre-filter, cross-user regression test |
| candidate disposition path | Tampering | Penny 가 authoritative field 를 직접 overwrite | `decide_candidate()` 단일 진입점, direct DB write 금지 |
| audit/candidate history | Repudiation | direct action 이후 Penny decision 이 남지 않음 | append-only activity + decision event + audit correlation |
| pending candidate summary | Information Disclosure | private candidate/path 가 타 사용자 bootstrap 에 노출 | actor/tenant filter, summary-only digest, raw payload 제외 |
| bootstrap assembly | Denial of Service | giant activity backlog 가 prompt 폭주 유발 | limits + char caps + degraded summary |
| Penny path | Elevation of Privilege | shared surface gadget 이 admin-only path 를 우회 | `AuthenticatedContext` 상속, `workbench.candidate_decide` 는 same ACL/coordinator path 사용 |

컴플라이언스 매핑:

- SOC2 CC6.1: actor-filtered bootstrap, same auth boundary
- SOC2 CC6.7: candidate decision / direct action correlation audit
- GDPR Art.32: least-privilege projection, no cross-user context bleed

### 2.5 의존성

새 third-party crate 추가는 없다.

기존 의존성만 사용한다.

- `serde`, `chrono`, `uuid` — bootstrap / digest serialization
- `tracing` — degraded / decision telemetry
- `sqlx` / 기존 projection backing stores — activity/candidate lookup

즉, 이 문서는 새 dependency risk 를 만들지 않고 기존 seam 정렬만 수행한다.

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 책임 분리

| 크레이트 | 책임 |
|---|---|
| `gadgetron-core` | `PennyTurnBootstrap` 및 digest 타입, assembler trait, decision request/receipt 타입 |
| `gadgetron-gateway` | actor-filtered shared surface assembler 구현, workbench projection 재사용, request ingress orchestration |
| `gadgetron-penny` | bootstrap text binding, `workbench.*` gadget provider 등록, tool error propagation |
| `gadgetron-knowledge` | candidate projection, decision coordinator, materialization status source |
| `gadgetron-web` | 같은 projection 을 렌더링하는 별도 UI consumer |

`docs/reviews/pm-decisions.md` D-12 준수 요약:

- gateway-local projection builder 는 `gadgetron-gateway` 에 머문다.
- shared digest 타입만 `gadgetron-core` 로 올라간다.
- candidate truth / materialization 은 계속 `gadgetron-knowledge` 가 소유한다.
- Penny 는 consumer 이지 authoritative store 가 아니다.

### 3.2 데이터 흐름 다이어그램

```text
user direct action / Penny gadget / system event
                    |
                    v
        activity capture + candidate projection
                    |
                    +-------------------+
                    |                   |
                    v                   v
        workbench projection      PennyTurnContextAssembler
                    |                   |
                    |                   v
                    |          PennyTurnBootstrap digest
                    |                   |
                    v                   v
          /web workbench UI      PennyProvider -> Claude session
                                        |
                                        v
                           workbench.* deep-read gadgets
                                        |
                                        v
                           candidate decision / evidence lookup
```

### 3.3 타 도메인 인터페이스 계약

- `gateway-router-lead`
  - `/v1/chat/completions` request ingress 에서 assembler 호출 위치를 소유
  - workbench route group 과 Penny bootstrap 이 같은 projection source 를 쓰는지 보장
- `xaas-platform-lead`
  - actor / tenant / audit correlation 필드 정합성 보장
- `security-compliance-lead`
  - degraded continuation 이 cross-user leak 나 silent bypass 를 만들지 않는지 검증
- `qa-test-architect`
  - resume-turn refresh, cross-user isolation, decision/materialization 분리 회귀 시나리오 검증

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

테스트 대상과 invariant:

- `PennyTurnContextAssembler::build`
  - actor filter 가 workbench projection 과 일치해야 한다
  - newest-first ordering
  - configured limits/caps 적용
  - partial failure 시 degraded reason 포함
  - resume-turn 이라도 매 요청 재조립
- `render_shared_context_block`
  - deterministic formatting
  - empty list 표기 유지
  - `digest_summary_chars` cap 적용
  - raw path / secret / stack trace 미노출
- `WorkbenchAwarenessGadgetProvider`
  - unknown gadget → `UnknownGadget`
  - invisible request/candidate → 403/404
  - `candidate_decide` 가 coordinator 경로만 사용
- candidate decision receipt mapper
  - `Accepted` 와 `materialization_status = failed` 를 혼동하지 않음

### 4.2 테스트 하네스

- fake activity projection store
- fake candidate projection / coordinator
- fake approval projection store
- existing `PennyFixture` / fake Claude binary
- `tokio::time::pause()` 로 deterministic timeout / retry 검증

property-based test 도입:

- activity ordering / capping / dedup invariant
- candidate disposition + materialization status combination invariant

### 4.3 커버리지 목표

- `gadgetron-gateway` shared-context assembler: branch 90% 이상
- `gadgetron-penny` shared-context renderer / gadget provider: branch 90% 이상
- regression tests: cross-user isolation / resume refresh / degraded notice 각각 최소 1개 이상

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

함께 테스트할 크레이트:

- `gadgetron-gateway`
- `gadgetron-penny`
- `gadgetron-knowledge`
- `gadgetron-core`

e2e 시나리오:

1. **direct action -> next Penny turn awareness**
   workbench direct action 실행 -> activity/candidate 생성 -> 같은 user 의 다음 Penny turn bootstrap 에 digest 포함
2. **candidate accept -> workbench/Penny parity**
   Penny 가 `workbench.candidate_decide(accept)` 호출 -> candidate disposition 갱신 -> workbench pending count 와 Penny follow-up 이 같은 상태를 봄
3. **cross-user isolation**
   user A direct action / candidate 가 user B bootstrap 에 나타나지 않음
4. **resume-turn refresh**
   conversation resume 활성화 상태에서 out-of-band direct action 발생 -> 다음 resume turn 에 새 digest 가 붙음
5. **degraded continuation**
   approval projection 또는 candidate store 를 의도적으로 실패시킴 -> Penny bootstrap 에 degraded reason 포함, 요청 자체는 계속 가능

### 5.2 테스트 환경

- PostgreSQL: testcontainers 실 DB
- gateway harness: authenticated user / API key fixture
- Penny: fake Claude binary + existing `PennyFixture`
- workbench action path: fixture bundle 또는 gateway-local fake action provider

CI 재현성 규칙:

- wall-clock 의존 금지
- sorting/filtering/assertion 은 deterministic fixture timestamp 사용
- cross-user tests 는 고정된 `user_id`, `tenant_id`, `request_id` 로 수행

### 5.3 회귀 방지

다음 변화가 일어나면 테스트가 실패해야 한다.

- Penny 가 shared surface 대신 hidden local state 에 의존해도 테스트가 통과하는 상태
- resume-turn 에 bootstrap 재조립이 생략되는 리팩터
- user B 가 user A 의 activity/candidate digest 를 보는 leak
- `Accept` 직후 canonical write 실패를 `Rejected` 나 `Pending` 로 잘못 되돌리는 구현
- degraded component failure 가 Penny 와 workbench 사이에서 서로 다른 이유 문자열로 surface 되는 drift

---

## 6. Phase 구분

- [P2B]
  - per-turn bootstrap digest
  - `workbench.activity_recent`
  - `workbench.request_evidence`
  - `workbench.candidates_pending`
  - `workbench.candidate_decide`
  - explicit degraded notice
- [P2C]
  - push refresh or live invalidation when long-running Penny interaction needs mid-turn updates
  - prioritization / ranking improvements for digests
- [P3]
  - learned summarization / prioritization tuned by incident history
  - richer pending approval narratives and runbook candidate grouping

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | [P2C] mid-turn refresh 가 필요해질 때 transport 선택 | A. gateway SSE side-channel / B. 다음 turn bootstrap + explicit refresh gadget | **B** | 🟢 P2B scope 에서는 B로 닫음. 운영자 증거가 쌓일 때만 재개방 |

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. 최근 workbench/action/candidate 승인 이후 비어 있던 Penny awareness seam 을 독립 문서로 분리.

**체크리스트**:
- [x] 최근 `origin/main` 변화 반영
- [x] shared surface vs private memory 경계 명시
- [x] direct action parity 포함
- [ ] review annotations

### Round 1 — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Conditional Pass

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
- A1: workbench UI projection 과 Penny bootstrap 이 같은 backing service / actor filter 를 쓰도록 본문에 명시
- A2: session resume 가 켜져 있어도 bootstrap 재조립이 매 요청마다 수행됨을 load-bearing 규칙으로 승격

**다음 라운드 조건**: A1, A2 반영 후 Round 1.5 진행

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**:
- [x] 위협 모델
- [x] 신뢰 경계 입력 검증
- [x] 인증·인가
- [x] 시크릿 관리
- [x] 감사 로그
- [x] 에러 정보 누출
- [x] LLM 특이 위협
- [x] 사용자 touchpoint 워크스루
- [x] 에러 메시지 3요소
- [x] defaults 안전성
- [x] runbook/operator path
- [x] 하위 호환

**Action Items**:
- A1: degraded notice 를 optional config 로 만들지 말고 fail-closed validation 으로 고정
- A2: bootstrap summary 에 raw path / stack / secret 이 실리지 않음을 logging/error 섹션에 명시

**다음 라운드 조건**: A1, A2 반영 후 Round 2 진행

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**:
- [x] 단위 테스트 범위
- [x] mock 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 회귀 테스트
- [x] 테스트 데이터

**Action Items**:
- A1: resume-turn refresh 를 별도 회귀 시나리오로 고정
- A2: cross-user isolation 과 degraded continuation 을 integration plan 에 명시

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
- 없음

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved. Penny 는 이제 subprocess/session spec 위에 shared surface awareness contract 를 갖는다. direct action parity, candidate curation, workbench projection 이 같은 사실 모델 위에 정렬되었다.
