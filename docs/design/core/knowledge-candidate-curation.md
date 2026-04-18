# Knowledge Candidate Lifecycle & Penny Curation Loop

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18 (KC-1b conformance reconcile for recent `origin/main`)
> **관련 크레이트**: `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-web`, `gadgetron-xaas`
> **Phase**: [P2B] landed in-memory slice + canonical materialization on wired services / [P2B-next] startup wiring + persistent capture + materialization status / [P2C] richer heuristics + user-confirmation routing / [P3] automation candidates
> **관련 문서**: `docs/design/core/knowledge-plug-architecture.md`, `docs/design/web/expert-knowledge-workbench.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/11-raw-ingestion-and-rag.md`, `docs/design/phase2/12-external-gadget-runtime.md`, `docs/process/04-decision-log.md`

---

## 1. 철학 & 컨셉 (Why)

### 1.1 해결하는 문제

Gadgetron 에서는 Penny 가 중심 제어 표면이지만, 전문가 사용자는 bundle viewer 나 workbench action host 에서도 직접 제어를 수행한다. 문제는 "직접 제어"가 발생한 뒤 그 사실을 **어떻게 capture 하고, 언제 knowledge 로 승격하며, 누가 그 승격을 결정하는가**가 아직 core contract 로 고정돼 있지 않다는 점이다.

이 공백이 남아 있으면 다음 세 가지가 흔들린다.

1. direct action 이 Penny 와 무관한 별도 privileged path 로 분기될 수 있다.
2. 시스템이 관측한 사실과 Penny 가 해석한 의미가 뒤섞여 audit / ACL / replay 무결성이 깨질 수 있다.
3. 장기 참고 가치가 있는 사건이 즉시 canonical wiki 에 반영되거나, 반대로 아예 사라져 follow-up / citation / runbook 누적이 끊길 수 있다.

이 문서는 이 세 문제를 닫기 위한 **capture plane / semantic plane 분리 계약** 을 정의한다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1` 의 Gadgetron 은 "지식 협업 플랫폼" 이다. 즉, 시스템은 사용자의 조작과 자동화 결과를 단순 실행으로 끝내지 않고, 장기적으로 재사용 가능한 지식으로 다뤄야 한다.

동시에 사용자가 직접 지시한 원칙도 분명하다.

- Penny 만이 유일한 제어 표면이 아니다.
- bundle viewer 를 통한 직접 제어도 Penny 와 knowledge layer 가 인식하고 반영해야 한다.
- 그러나 그 모든 것을 무조건 자동 기록해서는 안 된다.
- Penny 가 더 주된 semantic steward 로서 기록 여부를 결정하거나 사용자에게 물을 수 있어야 한다.

따라서 채택할 구조는 다음 한 문장으로 요약된다.

> **System captures facts. Penny curates meaning. Canonical knowledge writes only after that curation loop resolves.**

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 장점 | 채택하지 않은 이유 |
|---|---|---|
| A. direct action 성공 시 즉시 canonical wiki 기록 | 구현 단순, 회고 누락 감소 | 잡음이 너무 많고, 의미 승격 판단이 없어 canonical knowledge 질이 무너진다 |
| B. direct action 은 audit 에만 남기고 knowledge 와 완전 분리 | 보안과 순서는 단순 | Penny 가 이후 turn 에 사건을 참고하거나 기록 여부를 묻는 경험이 끊긴다 |
| C. Penny 가 activity capture 와 knowledge write 를 모두 소유 | agent 중심 모델과 직관적 | fact capture 가 LLM 성공 여부에 종속되고, deterministic replay / audit 순서가 흔들린다 |
| D. 시스템이 append-only capture 를 소유하고, Penny 는 candidate disposition 을 맡는다 | 책임 경계가 명확, audit/replay 안정성 확보 | read model / candidate lifecycle 층이 추가된다 |

채택: **D. 시스템 capture + Penny curation + delayed canonical write**

### 1.4 핵심 설계 원칙과 trade-off

1. **Capture precedes curation**
   직접 제어, tool call, approval 결과, runtime 상태 변화는 먼저 append-only 사실 이벤트로 남는다.
2. **Penny is semantic steward, not fact owner**
   Penny 는 capture 된 이벤트를 읽고 요약, 연결, 기록 승인 제안, 사용자 질의를 담당한다. 이벤트 원본 자체를 바꾸지 않는다.
3. **Canonical knowledge is opt-in by disposition**
   `KnowledgeCandidate` 가 `Accepted` 되기 전에는 knowledge search / citation 의 canonical source 로 간주하지 않는다.
4. **Direct UI action and Penny tool call are dual surfaces**
   둘은 같은 capability 로 연결되며, 서로 다른 privileged bypass 가 아니다.
5. **Candidate hints are advisory, never authoritative**
   bundle / gadget / external runtime 이 후보 힌트를 줄 수는 있지만, 최종 disposition / canonical path / ACL 는 core 가 결정한다.
6. **Penny memory is shared-surface memory**
   Penny 는 특별한 사적 메모리를 갖지 않는다. activity event 와 candidate read model 을 통해 사건을 재사용한다.

Trade-off:

- 상태 모델이 단순 채팅 앱보다 무겁다.
- 대신 전문가 환경에서 요구되는 auditability, replay, shared awareness, selective write-back 을 한 구조 안에 담을 수 있다.

### 1.5 현재 trunk 기준 landed slice

2026-04-18 `origin/main` (`W3-KC-1` + `W3-KC-1b`, PR #85 / #87, `docs/process/04-decision-log.md` D-20260418-17 / D-20260418-18) 기준 trunk 에 실제로 들어온 것은 "contract + in-memory slice + optional canonical writeback + fixture/E2E gate" 까지다.

- [P2B] landed
  - `gadgetron-core::knowledge::candidate` 의 wire types / traits / serde contract
  - `ActivityCaptureStore::get_candidate()` fast-path 추가
  - `gadgetron-knowledge::candidate` 의 `InMemoryActivityCaptureStore`, `InProcessCandidateCoordinator`
  - builder-style `InProcessCandidateCoordinator::with_knowledge_service()` / `.with_path_rules()`
  - `ActivityKind` snake_case key 기반 `{date}` / `{topic}` / `{author}` `path_rules` expansion
  - coordinator 에 `KnowledgeService` 가 주입된 경우의 `materialize_accepted_candidate() -> KnowledgeService::write()` canonical writeback
  - `gadgetron-gateway::penny::shared_context` 의 wire-stable `pending_penny_decision` renderer
  - `crates/gadgetron-gateway/tests/kc1_fixture_diff.rs` 및 `crates/gadgetron-knowledge/tests/kc1b_canonical_write_e2e.rs`
- [P2B-next] not yet wired on real server boot
  - `AppState.activity_capture_store` / `candidate_coordinator` / `knowledge_service` / `path_rules` 를 실제 startup path 에서 연결하는 wiring
  - Penny gadget/runtime path 가 production boot 에서 PSL-1 stub 을 벗어나는 것
  - workbench/read-model 에 `canonical_path`, `materialization_status`, `audit_event_id` 를 영속적으로 반영하는 projection
  - config default `path_rules` 와 KC-1b runtime key-space(`direct_action`, `runtime_observation` 등) 의 정렬
- [P2C+] intentionally deferred
  - `PgActivityCaptureStore` / retention jobs / projection persistence
  - `require_user_confirmation_for` routing semantics
  - auto-prompt Penny behavior
  - audit correlation (`audit_event_id`) 의 full plumbing
  - richer candidate heuristics / clustering / scoring

따라서 이 문서는 "이상적인 최종 구조" 만 설명하지 않는다. **현재 trunk authority 와 KC-1b landed behavior, 그리고 KC-1c follow-up seam 을 함께 고정하는 문서**로 사용한다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

```rust
// gadgetron-core::knowledge::candidate

use std::collections::BTreeMap;
use std::fmt::Debug;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{error::GadgetronError, knowledge::AuthenticatedContext};

pub type CaptureResult<T> = Result<T, GadgetronError>;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOrigin {
    UserDirect,
    Penny,
    System,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    DirectAction,
    GadgetToolCall,
    ApprovalDecision,
    RuntimeObservation,
    KnowledgeWriteback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedActivityEvent {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub request_id: Option<Uuid>,
    pub origin: ActivityOrigin,
    pub kind: ActivityKind,
    pub title: String,
    pub summary: String,
    pub source_bundle: Option<String>,
    pub source_capability: Option<String>,
    pub audit_event_id: Option<Uuid>,
    pub facts: Value,
    pub created_at: DateTime<Utc>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeCandidateDisposition {
    PendingPennyDecision,
    PendingUserConfirmation,
    Accepted,
    Rejected,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDecisionKind {
    Accept,
    Reject,
    EscalateToUser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeCandidate {
    pub id: Uuid,
    pub activity_event_id: Uuid,
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub summary: String,
    pub proposed_path: Option<String>,
    pub provenance: BTreeMap<String, String>,
    pub disposition: KnowledgeCandidateDisposition,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateHint {
    pub summary: String,
    pub proposed_path: Option<String>,
    pub tags: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateDecision {
    pub candidate_id: Uuid,
    pub decision: CandidateDecisionKind,
    pub decided_by_user_id: Option<Uuid>,
    pub decided_by_penny: bool,
    pub rationale: Option<String>,
}

#[async_trait]
pub trait ActivityCaptureStore: Send + Sync + Debug {
    async fn append_activity(
        &self,
        actor: &AuthenticatedContext,
        event: CapturedActivityEvent,
    ) -> CaptureResult<()>;

    async fn append_candidate(
        &self,
        actor: &AuthenticatedContext,
        activity_event_id: Uuid,
        hint: CandidateHint,
    ) -> CaptureResult<KnowledgeCandidate>;

    async fn decide_candidate(
        &self,
        actor: &AuthenticatedContext,
        decision: CandidateDecision,
    ) -> CaptureResult<KnowledgeCandidate>;

    async fn list_candidates(
        &self,
        actor: &AuthenticatedContext,
        limit: usize,
        only_pending: bool,
    ) -> CaptureResult<Vec<KnowledgeCandidate>>;

    async fn get_candidate(
        &self,
        actor: &AuthenticatedContext,
        id: Uuid,
    ) -> CaptureResult<Option<KnowledgeCandidate>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDocumentWrite {
    pub path: String,
    pub content: String,
    pub provenance: BTreeMap<String, String>,
}

#[async_trait]
pub trait KnowledgeCandidateCoordinator: Send + Sync + Debug {
    async fn capture_action(
        &self,
        actor: &AuthenticatedContext,
        event: CapturedActivityEvent,
        hints: Vec<CandidateHint>,
    ) -> CaptureResult<Vec<KnowledgeCandidate>>;

    async fn materialize_accepted_candidate(
        &self,
        actor: &AuthenticatedContext,
        candidate_id: Uuid,
        write: KnowledgeDocumentWrite,
    ) -> CaptureResult<String>;
}
```

설계 규칙:

- bundle / gadget / external runtime 은 `CandidateHint` 만 반환할 수 있다.
- [P2B] landed slice 에서 `KnowledgeCandidateDisposition` 초기값은 `PendingPennyDecision` 이다. `PendingUserConfirmation` 은 decision path 를 통해서만 만든다.
- `Accepted` / `Rejected` 로의 전환은 항상 `decide_candidate()` 경로를 통해 기록된다.
- `list_candidates(actor, limit, only_pending)` 는 newest-first projection read 의 유일한 공용 진입점이다.
- `get_candidate(actor, id)` 는 materialization precondition 이 `list_candidates(usize::MAX, false)` 전체 스캔에 의존하지 않도록 만든 fast-path 이다. absent row 와 backend failure 를 구분해야 한다.
- `materialize_accepted_candidate()` 는 **stable seam** 이다. [P2B] 현재 trunk 는 먼저 `Accepted` 여부를 확인한 뒤, coordinator 에 `knowledge_service` 가 있으면 `KnowledgeService::write` 를 호출하고, 없으면 KC-1a synthetic fallback 을 유지한다.
- `proposed_path` 와 `KnowledgeDocumentWrite` 는 현재 trunk 에서 계속 `String` 기반 wire shape 로 유지한다. typed `KnowledgePath` narrowing 은 아직 landed 하지 않았고 [P2C+] follow-up 으로 남는다.

### 2.2 내부 구조

구조는 현재 trunk authority 기준으로 **네 계층 + 하나의 optional materialization seam** 으로 분리한다.

1. **Capture ingress** `[P2B landed]`
   gateway, Penny, workbench action host, bundle runtime 이 `CapturedActivityEvent` 입력을 만든다.
2. **Append-only event store** `[P2B landed, in-memory only]`
   현재 trunk 는 activity event / candidate / decision 을 `tokio::sync::Mutex` 로 감싼 in-memory store 에 둔다. 영속 스토어는 아직 아니다.
3. **Candidate projection** `[P2B landed]`
   현재 disposition, latest rationale, proposed path, acceptance 가능 여부를 계산한다.
4. **Penny curation loop bridge** `[P2B landed at service/test level, startup wiring is deferred]`
   Penny shared-surface service 가 pending candidate 를 읽고 accept/reject/escalate 결정을 기록한다.
5. **Canonical knowledge materializer** `[P2B landed when coordinator is wired; production startup wiring deferred]`
   `Accepted` 된 candidate 는 `KnowledgeService::write` 로 승격할 수 있다. 다만 coordinator 에 service 가 주입되지 않은 경로에서는 synthetic path fallback 을 유지한다.

데이터 흐름:

```text
direct action / Penny tool / system event
                |
                v
       CapturedActivityEvent append
                |
                v
CandidateHint normalization + optional path_rules expansion
                |
                v
        KnowledgeCandidate projection
                |
      +---------+----------+
      |                    |
      v                    v
 Penny decide         User confirm
      |                    |
      +---------+----------+
                |
                v
           Accepted / Rejected
                |
                +--> [P2B landed, service wired] materialize_accepted_candidate()
                |        -> KnowledgeService::write
                |        -> canonical store / index / relation
                |
                +--> [P2B landed fallback] materialize_accepted_candidate()
                         -> `proposed_path` or synthesized fallback String
```

핵심 불변식:

- append-only event store 는 수정되지 않는다.
- candidate 의 현재 disposition 은 projection 결과이지, 원본 사실 이벤트의 rewrite 가 아니다.
- canonical knowledge write 이전에는 candidate 는 citation 대상이 아니다.
- Penny 실패와 무관하게 capture plane 은 완료되어야 한다.

동시성 모델:

- [P2B] current trunk 는 `tokio::sync::Mutex` 로 append / decide / list 를 직렬화한다.
- source of truth 는 여전히 append-only candidate + decision append 순서다. projection 은 read-side convenience 이다.
- [P2B] 현재 landed slice 는 optimistic version check 를 구현하지 않는다. 중복 decision 은 mutex 순서대로 기록된다.
- [P2C / KC-1c] Postgres backing 이 들어오면 row-level locking 또는 explicit version check 로 conflict semantics 를 강화한다.

### 2.3 설정 스키마

```toml
[knowledge.curation]
enabled = true
capture_retention_days = 90
candidate_retention_days = 30
max_candidates_per_request = 8
auto_prompt_penny = true
require_user_confirmation_for = ["org_write", "policy_note", "destructive_action"]

# 현재 `KnowledgeCurationConfig::default()` 가 실제로 제공하는 기본값
[knowledge.curation.path_rules]
operations = "ops/journal/%Y/%m/%d"
incident = "ops/incidents/%Y/%m/%d"
research = "research/notes/%Y/%m/%d"

# KC-1b coordinator runtime semantics (startup wiring landed 후 의도되는 키 공간)
# direct_action = "ops/journal/{date}/{topic}"
# gadget_tool_call = "ops/tools/{date}/{author}"
# runtime_observation = "ops/runtime/{date}/{topic}"
```

검증 규칙:

- `enabled=false` intent 는 "candidate generation / Penny curation loop 비활성화" 다. 다만 [P2B] 현재 trunk 는 이 플래그를 production startup wiring 에 아직 연결하지 않았다.
- `capture_retention_days` 는 7 이상이어야 한다. 그보다 짧으면 incident review 가 불가능해진다.
- `candidate_retention_days` 는 `capture_retention_days` 보다 클 수 없다.
- `max_candidates_per_request` 는 1 이상 32 이하로 제한한다.
- `require_user_confirmation_for` 는 [P2B] 현재 비어 있지 않은 string label 만 요구한다. enum narrowing 은 [P2C] user-confirmation semantics 와 함께 굳힌다.
- `path_rules.*` 는 [P2B] 현재 validation 시 parent traversal (`../`, `..` segment) 만 거부한다. placeholder vocabulary, key-space(`operations` vs `direct_action`) 는 아직 validate 하지 않는다.
- `InProcessCandidateCoordinator::with_path_rules()` 로 주입된 map 은 [P2B] 현재 `ActivityKind` snake_case key (`direct_action`, `gadget_tool_call`, `runtime_observation`, ...) 를 기준으로 `{date}` / `{topic}` / `{author}` 만 확장한다.
- 따라서 current trunk 는 **config default surface 와 coordinator runtime semantics 가 아직 완전히 합쳐지지 않았다**. production startup wiring 이 없기 때문에 operator-facing drift 는 아직 격리되어 있지만, live boot enable 전 정렬이 필요하다.
- `auto_prompt_penny`, `require_user_confirmation_for`, `path_rules` application 은 현재 startup validation surface 까지 landed 되었고, live request flow 연결은 후속이다.

운영자 touchpoint:

- `GET /activity` 와 `GET /request_evidence` workbench surface 가 capture/evidence read model 을 노출한다.
- Penny gadget 의 `workbench.candidates_pending` / `workbench.candidate_decide` 가 같은 shared-surface contract 를 소비한다.
- [P2B] 현재 production boot 는 candidate plane 을 기본 wiring 하지 않으므로, live server 는 여전히 PSL-1 stub behavior 를 보일 수 있다. 현재 landed verification gate 는 `kc1_fixture_diff` 와 `kc1b_canonical_write_e2e` 두 축이다.

### 2.4 이벤트와 후보 생성 규약

candidate 생성은 bundle 반환을 그대로 신뢰하지 않는다.

정책:

1. bundle/runtime 이 `CandidateHint` 를 0..N 개 반환할 수 있다.
2. core 는 actor, tenant, source capability, audit correlation 을 다시 주입한다.
3. [P2B] 현재 landed slice 는 `proposed_path` 를 advisory `Option<String>` 으로 보존한다. authoritative path sanitize / ACL re-check 는 materialization boundary 의 책임이며, `path_rules` template expansion 은 capture 시점에 hint path 가 비어 있을 때만 적용된다.
4. [P2B] 현재 coordinator 는 전달된 힌트를 clamp + append 한다. hint 의 `proposed_path` 가 비어 있으면 `path_rules` 를 조회하고, 매칭이 없으면 `ops/journal/<YYYY-MM-DD>/<activity_kind>` fallback string 을 넣는다. 힌트가 전혀 없는 activity 에 대한 server-generated candidate 는 [P2B-next / P2C] 로 미룬다.
5. external runtime 은 candidate 를 `Accepted` 상태로 만들 수 없다.

자동 candidate 생성 예:

- `server.restart_service` direct action 성공 -> 운영 journal candidate
- approval 을 동반한 destructive action -> change note candidate
- `wiki.import` 성공 -> import summary candidate 또는 citation-ready summary candidate
- repeated runtime failure -> incident/runbook delta candidate

후보 미생성 원칙:

- purely ephemeral read action
- 동일 request 안에서 이미 중복된 요약
- caller 권한으로 나중에 재사용 가치가 거의 없는 telemetry spam

### 2.5 Penny curation loop

Penny 의 책임은 세 가지뿐이다.

1. `PendingPennyDecision` 후보를 읽는다.
2. `Accept` / `Reject` / `EscalateToUser` 중 하나를 선택한다.
3. 필요하면 사용자에게 "기록할까요?" 를 묻는다.

Penny 권한 경계:

- Penny 는 `AuthenticatedContext` 를 caller 로 상속한다.
- Penny 는 candidate 원문 사실 이벤트를 수정하지 못한다.
- [P2B] 현재 accept 는 disposition update 만 수행한다. canonical write 는 이후 별도의 `materialize_accepted_candidate()` 호출에서만 일어나며, coordinator 에 service 가 없으면 synthetic fallback 으로 끝난다.
- [P2B] landed materialization 경로에서도 Penny accept 는 caller 권한을 넘지 못한다. caller 가 쓸 수 없는 scope/path 는 materialization 실패로 surface 한다.
- Penny 의 rationale 은 candidate decision event 에 남지만, canonical knowledge 자체는 별도 write path 로 기록된다.

same-turn 처리 원칙:

- explicit wiring 이 있는 서비스 인스턴스에서는 direct action 직후 activity / candidate projection 이 즉시 read model 에 반영된다.
- Penny 스트림이 이미 열려 있다면, 다음 tool reasoning 단계에서 pending candidate 목록을 볼 수 있다.
- Penny 스트림이 끝난 뒤에도 next-turn 에 candidate inbox 로 재진입할 수 있다.
- 현재 trunk 의 production startup path 는 candidate plane 을 자동 wiring 하지 않으므로, 이 same-turn guarantee 는 test harness 와 future server wiring authority 를 정의하는 문장으로 본다.

### 2.6 Canonical write 승격 규칙

이 문서가 authoritative 로 고정하는 핵심은 **`Accepted` 와 `materialized` 는 같은 상태가 아니다** 라는 점이다. 2026-04-18 `origin/main` 기준으로는 두 단계가 모두 일부 landed 했지만, 후자의 operator-visible projection 은 아직 미완성이다.

[P2B] current trunk / KC-1b:

1. `materialize_accepted_candidate()` 는 `get_candidate()` fast-path 로 latest row 를 조회한다.
2. latest disposition 이 `Accepted` 가 아니면 `GadgetronError::Knowledge { kind = InvalidQuery }` 로 실패한다.
3. coordinator 에 `knowledge_service` 가 주입되어 있으면 `KnowledgeDocumentWrite` 를 `KnowledgePutRequest { path, markdown, create_only: false, overwrite: true }` 로 변환해 `KnowledgeService::write` 를 호출한다.
4. canonical write 성공 시 `KnowledgeWriteReceipt.path` 를 반환한다.
5. coordinator 에 `knowledge_service` 가 없으면 `candidate.proposed_path` 또는 synthetic fallback string 을 반환한다. 이 경로는 dev/test 및 아직 boot wiring 되지 않은 서버 경로 호환을 위한 것이다.
6. `KnowledgeWriteback` activity append, `materialization_status` persistence, `canonical_path` projection 은 아직 없다.

[P2B-next / KC-1c]:

1. startup path 가 coordinator + knowledge service + `path_rules` 를 한 번에 wiring 한다.
2. actor 가 target scope/path 에 write 가능한지 재검증하는 operator-visible failure model 을 projection 까지 연결한다.
3. canonical write 성공/실패가 activity stream, workbench projection, Penny follow-up 에 모두 반영된다.
4. persistence-backed candidate store 와 retry/reporting surface 가 추가된다.

중요한 점:

- candidate acceptance 와 canonical write 는 같은 개념이 아니다.
- accept 는 semantic approval 이고, write 는 별도의 ACL / conflict / store write 단계다.
- write 실패 시 candidate 는 `Accepted` 상태를 유지하되, `materialization_status=failed` read model 을 노출한다.

이유:

- 사람이 보기에 "기록해야 한다" 와 "지금 저장 인프라가 성공했다" 는 다른 축이다.
- write 실패를 이유로 candidate 를 다시 `PendingPennyDecision` 로 되돌리면 의미 결정과 인프라 실패가 섞인다.

### 2.7 에러 & 로깅

주요 에러:

- `GadgetronError::Knowledge`
  unknown candidate id, unsupported decision kind, non-accepted materialization, future canonical write ACL/path/store failure
- `GadgetronError::Penny`
  `ToolDenied` stub path, candidate decision request failure, future materialization follow-up reporting

operator-facing error 원칙:

- 무엇이 일어났는지: "지식 후보를 기록할 수 없습니다"
- 왜인지: "caller 권한으로는 해당 경로에 쓸 수 없거나 후보 상태가 아직 승인되지 않았습니다"
- 무엇을 할지: "candidate 상태와 대상 scope/path 를 확인한 뒤 다시 시도하세요"

필수 tracing/audit events:

- [P2B landed] typed `PennyCandidateDecisionReceipt` / fixture-diff output 으로 candidate state 전이를 검증한다.
- [P2B landed] `candidate.materialize` tracing span 이 `candidate_id`, `has_knowledge_service` 를 기록한다.
- [P2B-next] 다음 이벤트를 tracing / audit 에 추가한다:
  - `activity_capture_appended`
  - `knowledge_candidate_created`
  - `knowledge_candidate_decided`
  - `knowledge_candidate_escalated_to_user`
  - `knowledge_candidate_materialization_started`
  - `knowledge_candidate_materialization_failed`
  - `knowledge_candidate_materialized`

STRIDE 요약:

| 자산 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| direct action facts | UI/bundle -> gateway | Spoofing | actor 는 gateway `AuthenticatedContext` 로만 확정 |
| candidate summary / path | bundle hint -> core candidate store | Tampering | hint normalize, advisory-only `proposed_path`, authoritative disposition 금지 |
| operator history | capture store -> review UI | Repudiation | append-only activity + decision event log |
| pending candidates | candidate store -> Penny/UI | Information Disclosure | caller/tenant ACL 로 pending candidate 목록 필터 |
| candidate generation pipeline | high-volume runtime events | Denial of Service | per-request candidate cap, dedup, retention policy |
| canonical write path | Penny decision -> KnowledgeService | Elevation of Privilege | caller inheritance 재검증, Penny privileged bypass 금지 |

컴플라이언스 매핑:

- SOC2 CC6.6: direct action / candidate decision / writeback 감사 연쇄
- GDPR Art.32: tenant-scoped candidate visibility, least-privilege writeback
- HIPAA 164.312(b): who decided to record what, and when

### 2.8 의존성

- `gadgetron-core`
  - 기존 `serde`, `uuid`, `chrono`, `async-trait` 재사용
  - 신규 heavyweight dependency 추가 금지
- `gadgetron-knowledge`
  - [P2B] in-memory store / coordinator / curation config validation / optional `KnowledgeService` writeback
  - [P2B-next] startup wiring / persistence / projection
- `gadgetron-xaas`
  - [P2B-next / P2C] activity/candidate persistence schema 와 retention jobs 지원
- `gadgetron-web`
  - existing workbench read model / action host 소비

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 데이터 흐름

```text
user direct action / Penny gadget / system observation
                     |
                     v
                gateway auth
                     |
                     v
       [P2B] ActivityCaptureStore append-only log
                     |
                     v
  KnowledgeCandidate projection/read model + optional path_rules expansion
               |                         |
               v                         v
         Penny curation             bundle viewer UI
               |                         |
               +------------+------------+
                            |
                            v
        [P2B] materialize_accepted_candidate() seam
              |                               |
              v                               v
  [service wired] KnowledgeService::write   [service absent] synthetic path fallback
              |
              v
      canonical store / index / relation
```

### 3.2 타 모듈과의 인터페이스 계약

- `10-penny-permission-inheritance.md`
  Penny 의 decision path 역시 caller inheritance 를 그대로 따라야 한다.
- `13-penny-shared-surface-loop.md`
  candidate truth 는 shared bootstrap 이 아니라 candidate plane 이 소유하고, Penny 는 그 projection 을 읽는 소비자여야 한다.
- `knowledge-plug-architecture.md`
  accepted candidate 의 최종 목적지는 `KnowledgeService` canonical write path 이다.
- `11-raw-ingestion-and-rag.md`
  import 성공 후 생성되는 기록 후보와 citation-ready summary 는 이 lifecycle 을 따른다.
- `12-external-gadget-runtime.md`
  external bundle/runtime 이 candidate hint 를 반환할 수 있지만 authoritative disposition 은 없다.
- `expert-knowledge-workbench.md`
  UI 는 이 문서의 capture/candidate contract 를 렌더링하고 조작할 뿐, 별도 상태 모델을 만들지 않는다.

### 3.3 D-12 크레이트 경계 준수

- `gadgetron-core`
  activity/candidate type, trait, config, error surface
- `gadgetron-knowledge`
  [P2B] in-memory store / coordinator / disposition policy / optional knowledge write integration
  [P2B-next] startup wiring helper / persistence / projection
- `gadgetron-gateway`
  actor resolution, request correlation, Penny shared-surface adapter, fixture-diff integration gate
- `gadgetron-web`
  workbench projection 렌더링
- `gadgetron-xaas`
  [P2B-next / P2C] persistent store, projection, retention jobs

즉, core 는 contract 를 소유하고, orchestration/persistence/rendering 은 각 책임 계층에 남긴다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

- [P2B landed]
  - `knowledge_candidate_disposition_round_trips_snake_case`
  - `candidate_decision_kind_round_trips_snake_case`
  - `inmem_store_append_then_list`
  - `inmem_store_decide_updates_disposition_{accept,reject,escalate}`
  - `inmem_store_list_only_pending_filters`
  - `inmem_store_get_candidate_returns_exact_row`
  - `coordinator_capture_action_clamps_to_max_candidates`
  - `coordinator_capture_action_expands_path_rules_when_hint_path_missing`
  - `coordinator_materialize_errors_when_not_accepted`
  - `coordinator_materialize_returns_proposed_path_when_unwired`
  - `coordinator_materialize_writes_to_knowledge_service_when_wired`
  - gateway shared-surface pending/decide mapper tests
  - gateway shared-context renderer emits `pending_penny_decision`
- [P2B-next / P2C]
  - `candidate_hint_path_escape_rejected_at_materialization_boundary`
  - `accepted_candidate_requires_second_acl_check`
  - `materialization_failure_does_not_revert_acceptance`
  - `same_request_duplicate_hints_are_deduped`
  - `pending_candidates_filtered_by_actor_scope`
  - `external_runtime_hint_cannot_set_authoritative_fields`

### 4.2 테스트 하네스

- [P2B landed] in-memory append-only event store + fake workbench projection + deterministic fixture-diff normalizer + tempdir wiki-backed `KnowledgeService`
- [P2B-next] multi-user auth fixture + projection receipt assertions + canonical path validator
- [P2C] property-based test:
  direct action -> Penny decision -> materialization sequence permutation 에서 불변식 유지

### 4.3 커버리지 목표

- [P2B] `gadgetron-core::knowledge::candidate` / `gadgetron-knowledge::candidate`: branch 90% 이상
- [P2B-next] startup wiring / ACL re-check / projection path: branch 95% 이상

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

1. [P2B landed] `kc1_fixture_diff`: capture event -> 2 hints -> bootstrap render -> accept 1 candidate -> bootstrap render diff
2. [P2B landed] `kc1b_canonical_write_e2e`: capture -> path_rules expansion -> accept -> materialize -> `knowledge.search()` 가 canonical page 를 찾는다
3. [P2B-next] direct workbench action -> capture -> pending candidate -> Penny accept -> boot-wired canonical write
4. [P2B-next] direct workbench action -> capture -> Penny escalate -> user confirm -> canonical write
5. [P2B-next] direct workbench action -> capture -> Penny reject -> no canonical write
6. [P2C] external runtime action -> candidate hint -> normalized candidate -> acceptance
7. [P2B-next] accepted candidate + write failure -> read model surfaces failed materialization

### 5.2 테스트 환경

- [P2B landed] in-memory store + fake projection + shared-context renderer
- [P2B landed] tempdir wiki + `LlmWikiStore` + keyword index + `KnowledgeService`
- [P2B-next] gateway auth fixture with multiple users/teams + Postgres testcontainers for append-only event tables + projection tables
- [P2B-next] fake Penny decision provider + boot-wired `KnowledgeService`
- [P2B-next / P2C] web read-model contract tests for pending/accepted/rejected/materialization-warning rendering

### 5.3 회귀 방지

- direct action 이 capture 없이 실행되면 실패해야 한다
- candidate 가 승인 전 canonical search/citation 에 노출되면 실패해야 한다
- [P2B landed] wired materialization 이 canonical wiki 에 기록하지 못하면 실패해야 한다
- [P2B-next] Penny accept 가 caller write 권한을 우회하면 실패해야 한다
- [P2B-next] write 실패가 candidate history 를 덮어쓰거나 삭제하면 실패해야 한다
- external bundle 이 `Accepted` candidate 를 직접 반환해도 시스템이 받아들이면 실패해야 한다

---

## 6. Phase 구분

### 6.1 [P2B] landed on trunk

- append-only activity capture
- `KnowledgeCandidate` lifecycle
- in-memory store / coordinator
- `ActivityCaptureStore::get_candidate`
- builder-style `.with_knowledge_service()` / `.with_path_rules()`
- `path_rules` 3-variable expansion on hint-less candidates
- optional canonical materialization through `KnowledgeService::write`
- Penny shared-surface pending/decide adapter
- wire-stable `pending_penny_decision` renderer
- deterministic fixture-diff + canonical-write E2E exit gates
- local `String`-based `KnowledgeDocumentWrite` / `proposed_path`

### 6.2 [P2B-next] KC-1c-ready boot wiring

- real server startup wiring (`AppState.activity_capture_store` / `candidate_coordinator`)
- production config `path_rules` 와 runtime key-space 정렬
- `KnowledgeWriteback` / `canonical_path` / `materialization_status` projection
- Postgres-backed capture store / projection / retention
- workbench / Penny / web read model 이 같은 projection 을 소비하도록 정렬

### 6.3 [P2C] KC-1c and beyond

- `require_user_confirmation_for` routing logic
- typed `KnowledgePath` narrowing
- richer candidate heuristics (incident/runbook/policy taxonomy)
- relation-aware candidate summarization
- candidate clustering / duplicate merge
- audit correlation plumbing

### 6.4 [P3]

- automation/runbook candidate generation
- candidate quality scoring
- pluggable semantic curator beyond Penny

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | `Accepted` 이지만 materialization failed 인 후보의 UI 기본 노출 방식 | A. accepted+warning / B. retry queue 분리 | **A** | 🟡 |
| Q-2 | candidate retention 만료 시 projection 처리 | A. hide only / B. auto-reject | **A** | 🟡 |
| Q-3 | Penny auto-accept 허용 범위 | A. personal/private note only / B. wider org scope | **A** | 🟡 |
| Q-4 | candidate dedup key | A. request+capability+summary / B. semantic similarity | **A** first | 🟢 |

---

## 8. Out of scope

- full UI visual design 세부
- final Penny prompt wording
- long-term memory ranking / scoring engine
- runbook auto-execution / automation policy binding

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. capture plane / semantic plane 분리 및 candidate lifecycle 정리.

**체크리스트**:
- [x] direct action parity
- [x] candidate lifecycle
- [ ] final review annotations

### Round 1 — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Conditional Pass

**체크리스트**:
- [x] 인터페이스 계약
- [x] 크레이트 경계
- [x] 타입 중복 방지
- [x] 에러 반환
- [x] 동시성
- [x] 의존성 방향
- [x] Phase 태그
- [x] 레거시 결정 준수

**Action Items**:
- A1: bundle/runtime hint 와 authoritative candidate field 를 분리 명시
- A2: accepted 와 materialization failure 를 동일 상태로 취급하지 않음을 본문에 고정

**다음 라운드 조건**: A1, A2 반영

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**:
- [x] 위협 모델
- [x] 신뢰 경계 입력 검증
- [x] 인증·인가
- [x] 시크릿/에러 누출 방지
- [x] 감사 로그
- [x] 사용자 touchpoint 워크스루
- [x] defaults 안전성
- [x] runbook/operator path

**Action Items**:
- 없음

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**:
- [x] 단위 테스트 범위
- [x] mock 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 회귀 테스트

**Action Items**:
- 없음

### Round 3 — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**:
- [x] Rust 관용구
- [x] 제로 비용 추상화
- [x] 트레이트 설계
- [x] 에러 전파
- [x] 의존성 추가
- [x] 관측성

**Action Items**:
- 없음

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved. direct action, Penny curation, canonical writeback 의 공통 lifecycle 기준 문서로 사용 가능.

### Round 1 retry — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [x] 인터페이스 계약 — trunk 의 `ActivityCaptureStore` / `KnowledgeCandidateCoordinator` / gateway adapter 시그니처와 문서가 일치한다
- [x] 크레이트 경계 — in-memory impl 은 `gadgetron-knowledge`, persistent backing 은 `gadgetron-xaas` 후속으로 분리된다
- [x] 타입 중복 — `KnowledgeDocumentWrite` / `proposed_path: Option<String>` 임시 shape 가 문서 한 곳에서만 authoritative 하다
- [x] 에러 반환 — 현재 landed slice 는 `GadgetronError::Knowledge` / `Penny` 표면만 사용한다
- [x] 동시성 — in-memory mutex 직렬화와 KC-1b row-level semantics 가 분리 기재되었다
- [x] 의존성 방향 — gateway 는 candidate-plane trait 에만 의존하고 역방향 결합이 없다
- [x] Phase 태그 — P2B landed / P2B-next / P2C 가 명확히 구분되었다
- [x] 레거시 결정 준수 — D-12 crate boundary 와 충돌하지 않는다

**Action Items**:
- 없음

**Open Questions**:
- Q-1 ~ Q-4 유지

**다음 라운드 조건**: Round 1.5 retry on reconciled trunk snapshot

### Round 1.5 retry — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1.5` 기준)
- [x] 위협 모델 (필수) — capture path / candidate projection / canonical write seam 이 STRIDE 표에 반영되었다
- [x] 신뢰 경계 입력 검증 — `CandidateHint` 와 advisory `proposed_path` 의 authority boundary 가 분리되었다
- [x] 인증·인가 — Penny accept 와 future materialization 의 caller inheritance 재검증이 분리 명시되었다
- [x] 시크릿 관리 — 새 시크릿 surface 없음, error/log wording 에 secret leakage 경로 없음
- [x] 공급망 — 신규 dependency 요구사항 없음
- [x] 감사 로그 — append-only candidate / decision history 와 KC-1b event follow-up 이 분명하다
- [x] 에러 정보 누출 — operator-facing error 문구가 internal path leakage 없이 remediation 을 제공한다
- [x] 사용자 touchpoint 워크스루 — workbench endpoints, Penny gadgets, production boot stub 상태가 모두 문서화되었다
- [x] defaults 안전성 — `[knowledge.curation]` defaults 와 아직 미연결인 필드가 명확히 구분되었다
- [x] runbook/playbook — exit gate 가 `kc1_fixture_diff` 임을 명시해 oncall/operator 확인 경로가 있다

**Action Items**:
- 없음

**다음 라운드 조건**: Round 2 retry on landed-vs-follow-up test split

### Round 2 retry — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §2` 기준)
- [x] 단위 테스트 범위 — landed tests 와 KC-1b/P2C follow-up tests 가 분리되어 있다
- [x] mock 가능성 — in-memory store / fake projection / fake `KnowledgeService` strategy 가 명시되었다
- [x] 결정론 — fixture-diff timestamp normalization 과 deterministic ordering contract 가 반영되었다
- [x] 통합 시나리오 — current exit gate 와 future canonical write scenarios 가 모두 존재한다
- [x] CI 재현성 — in-memory landed path 와 Postgres testcontainers follow-up path 가 구분되었다
- [x] 성능 검증 — heavyweight persistence tests 를 landed slice 와 분리해 CI 비용이 통제된다
- [x] 회귀 테스트 — production stub behavior 와 future materialization regressions 둘 다 잡을 수 있다
- [x] 테스트 데이터 — fixture-diff output 과 fake projection usage 가 명확하다

**Action Items**:
- 없음

**다음 라운드 조건**: Round 3 retry on reconciled Rust/public-contract wording

### Round 3 retry — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §3` 기준)
- [x] Rust 관용구 — `#[non_exhaustive]`, `Result<T, GadgetronError>`, trait-object `Debug` 요구가 문서와 일치한다
- [x] 제로 비용 추상화 — P2B landed slice 가 in-memory mutex 기반임을 숨기지 않고 후속 비용 모델과 분리했다
- [x] 제네릭 vs 트레이트 객체 — public seam 이 trait object 기반이라는 점이 명확하다
- [x] 에러 전파 — `Knowledge` / `Penny` surfaces 만 사용한다는 설명이 trunk 와 일치한다
- [x] 수명주기 — no additional `'static` or lifetime claims beyond current API
- [x] 의존성 추가 — 신규 crate 요구 없음
- [x] 트레이트 설계 — `list_candidates` / `materialize_accepted_candidate` seam 이 trunk authority 와 정렬되었다
- [x] 관측성 — fixture-diff exit gate 와 future tracing events 가 단계별로 정리되었다
- [x] hot path — runtime writeback 미landing 사실을 명시해 과도한 performance claim 을 피했다
- [x] 문서화 — current trunk contract 와 KC-1b seam 이 명확히 분리되었다

**Action Items**:
- 없음

**다음 라운드 조건**: PM final approval on reconciled authority doc

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved (reconciled). 이 문서는 이제 KC-1 landed slice 와 KC-1b/c follow-up seam 을 동시에 설명하는 trunk-authoritative 설계 문서다.

### Round 1 (KC-1b Conformance Amendment) — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [x] 인터페이스 계약
- [x] 크레이트 경계
- [x] 타입 중복
- [x] 에러 반환
- [x] 동시성
- [x] 의존성 방향
- [x] Phase 태그
- [x] 레거시 결정 준수

**Action Items**:
- A1: `ActivityCaptureStore::get_candidate()` 와 builder-style coordinator API(`with_knowledge_service`, `with_path_rules`)를 본문 시그니처와 phase 설명에 반영했다.
- A2: KC-1a stub 설명을 제거하고, service wired / unwired 두 materialization path 를 같은 lifecycle 안에서 분리해 적었다.
- A3: config default `path_rules` 와 KC-1b runtime key-space drift 를 startup wiring 미landing 사실과 함께 명시했다.

**Open Questions**:
- Q-1 ~ Q-4 유지

**다음 라운드 조건**: 없음. Round 1.5 진행 가능.

### Round 1.5 (KC-1b Conformance Amendment) — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1.5` 기준)
- [x] 위협 모델 (필수)
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
- [x] CLI flag
- [x] API 응답 shape
- [x] config 필드
- [x] defaults 안전성
- [x] 문서 5분 경로
- [x] runbook playbook
- [x] 하위 호환
- [x] i18n 준비

**Action Items**:
- A1: operator-facing config surface 가 아직 startup 에 연결되지 않았다는 사실과, 연결 후 문제가 될 key-space drift 를 문서에 명확히 표기했다.
- A2: canonical write 가 caller inheritance 를 유지한 채 optional service wiring 으로만 일어난다는 점을 보안/사용성 양쪽에서 다시 고정했다.
- A3: shared-context renderer 의 `pending_penny_decision` wire string 이 audit/candidate plane 과 동일해야 한다는 계약을 반영했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 2 진행 가능.

### Round 2 (KC-1b Conformance Amendment) — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §2` 기준)
- [x] 단위 테스트 범위
- [x] mock 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 성능 검증
- [x] 회귀 테스트
- [x] 테스트 데이터

**Action Items**:
- A1: landed test surface 를 `kc1_fixture_diff` 와 `kc1b_canonical_write_e2e` 두 축으로 재정리했다.
- A2: unwired synthetic fallback path 와 wired canonical write path 를 별도 회귀 항목으로 분리했다.
- A3: future boot wiring / projection persistence 는 [P2B-next] 시나리오로 남기고, current trunk assertions 와 섞지 않도록 수정했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 3 진행 가능.

### Round 3 (KC-1b Conformance Amendment) — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §3` 기준)
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
- A1: current public trait surface(`get_candidate`, `materialize_accepted_candidate`)와 실제 builder API 를 trunk 기준으로 다시 고정했다.
- A2: 아직 landed 하지 않은 typed `KnowledgePath` narrowing 을 future phase 로 되돌려 현재 구현 약속과 분리했다.
- A3: persistence / row-locking 설명을 KC-1b 가 아니라 KC-1c follow-up 으로 재배치해 recent decision log 와 맞췄다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. 최종 승인 가능.

### 최종 승인 (KC-1b Conformance Amendment) — 2026-04-18 — PM
**결론**: Approved

기존 승인 로그는 append-only 로 유지하고, 이번 revision 은 `W3-KC-1b` landed state 와 recent `origin/main` authority 를 맞추기 위한 conformance amendment 로 별도 재검토했다. 본 문서는 이제 candidate lifecycle 의 current trunk behavior, optional canonical write path, 그리고 KC-1c follow-up boundary 를 서로 충돌 없이 설명한다.
