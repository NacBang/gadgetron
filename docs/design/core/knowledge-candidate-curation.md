# Knowledge Candidate Lifecycle & Penny Curation Loop

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-web`, `gadgetron-xaas`
> **Phase**: [P2B] primary / [P2C] richer candidate heuristics / [P3] automation candidates
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

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

```rust
// gadgetron-core::activity / gadgetron-core::knowledge::candidate

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    error::GadgetronError,
    identity::AuthenticatedContext,
    knowledge::{KnowledgeDocumentWrite, KnowledgePath},
};

pub type CaptureResult<T> = Result<T, GadgetronError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOrigin {
    UserDirect,
    Penny,
    System,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeCandidateDisposition {
    PendingPennyDecision,
    PendingUserConfirmation,
    Accepted,
    Rejected,
}

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
    pub proposed_path: Option<KnowledgePath>,
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
pub trait ActivityCaptureStore: Send + Sync {
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
}

#[async_trait]
pub trait KnowledgeCandidateCoordinator: Send + Sync {
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
    ) -> CaptureResult<KnowledgePath>;
}
```

설계 규칙:

- bundle / gadget / external runtime 은 `CandidateHint` 만 반환할 수 있다.
- `KnowledgeCandidateDisposition` 초기값은 `PendingPennyDecision` 또는 `PendingUserConfirmation` 뿐이다.
- `Accepted` / `Rejected` 로의 전환은 항상 `decide_candidate()` 경로를 통해 기록된다.
- canonical write 는 `materialize_accepted_candidate()` 를 통해서만 실행된다.

### 2.2 내부 구조

구조는 다섯 계층으로 분리한다.

1. **Capture ingress**
   gateway, Penny, workbench action host, bundle runtime 이 `CapturedActivityEvent` 입력을 만든다.
2. **Append-only event store**
   activity event 와 candidate decision event 를 영속화한다.
3. **Candidate projection**
   현재 disposition, latest rationale, proposed path, acceptance 가능 여부를 계산한다.
4. **Penny curation loop**
   Penny 가 pending candidate 를 읽고 accept/reject/escalate 결정을 내리거나 사용자 confirmation 을 요청한다.
5. **Canonical knowledge materializer**
   `Accepted` 된 candidate 만 `KnowledgeService::write` 로 승격한다.

데이터 흐름:

```text
direct action / Penny tool / system event
                |
                v
       CapturedActivityEvent append
                |
                v
         CandidateHint normalization
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
                v
      canonical knowledge write
```

핵심 불변식:

- append-only event store 는 수정되지 않는다.
- candidate 의 현재 disposition 은 projection 결과이지, 원본 사실 이벤트의 rewrite 가 아니다.
- canonical knowledge write 이전에는 candidate 는 citation 대상이 아니다.
- Penny 실패와 무관하게 capture plane 은 완료되어야 한다.

동시성 모델:

- capture append 는 request-local 트랜잭션으로 처리한다.
- candidate projection 갱신은 idempotent upsert 또는 event-sourced reducer 로 구현할 수 있으나, source of truth 는 append-only decision log 다.
- same candidate 에 대한 중복 accept/reject race 는 optimistic version check 로 막는다.

### 2.3 설정 스키마

```toml
[knowledge.curation]
enabled = true
capture_retention_days = 90
candidate_retention_days = 30
max_candidates_per_request = 8
auto_prompt_penny = true
require_user_confirmation_for = ["org_write", "policy_note", "destructive_action"]

[knowledge.curation.path_rules]
operations = "ops/journal/%Y/%m/%d"
incident = "ops/incidents/%Y/%m/%d"
research = "research/notes/%Y/%m/%d"
```

검증 규칙:

- `enabled=false` 여도 audit capture 는 비활성화할 수 없다. 꺼지는 것은 candidate generation / Penny curation loop 뿐이다.
- `capture_retention_days` 는 7 이상이어야 한다. 그보다 짧으면 incident review 가 불가능해진다.
- `candidate_retention_days` 는 `capture_retention_days` 보다 클 수 없다.
- `max_candidates_per_request` 는 1 이상 32 이하로 제한한다.
- `require_user_confirmation_for` 의 항목은 미리 정의된 enum set 과 일치해야 한다.
- `path_rules.*` 는 canonical knowledge root escape 를 허용하지 않는 템플릿만 통과한다.

운영자 touchpoint:

- `gadgetron workbench activity --request <id>` 는 capture plane 사실 이벤트를 보여준다.
- `gadgetron knowledge candidates --pending` 는 disposition 이 unresolved 인 후보를 보여준다.
- `gadgetron knowledge candidate decide <id> --accept|--reject` 는 Penny 없이도 same contract 로 개입하는 operator path 다.

### 2.4 이벤트와 후보 생성 규약

candidate 생성은 bundle 반환을 그대로 신뢰하지 않는다.

정책:

1. bundle/runtime 이 `CandidateHint` 를 0..N 개 반환할 수 있다.
2. core 는 actor, tenant, source capability, audit correlation 을 다시 주입한다.
3. `proposed_path` 는 sanitize + ACL + path template 규칙을 통과한 뒤에만 유지한다.
4. 힌트가 없더라도, 특정 activity kind 는 서버가 자체적으로 candidate 를 만들 수 있다.
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
- Penny accept 도 caller 권한을 넘지 못한다. caller 가 쓸 수 없는 scope/path 는 accept 실패다.
- Penny 의 rationale 은 candidate decision event 에 남지만, canonical knowledge 자체는 별도 write path 로 기록된다.

same-turn 처리 원칙:

- direct action 직후 activity event 는 즉시 read model 에 반영된다.
- Penny 스트림이 이미 열려 있다면, 다음 tool reasoning 단계에서 pending candidate 목록을 볼 수 있다.
- Penny 스트림이 끝난 뒤에도 next-turn 에 candidate inbox 로 재진입할 수 있다.

### 2.6 Canonical write 승격 규칙

`Accepted` 후보만 canonical write 로 승격한다.

승격 순서:

1. candidate latest disposition = `Accepted` 확인
2. actor 가 target scope/path 에 write 가능한지 재검증
3. `KnowledgeDocumentWrite` 생성
4. `KnowledgeService::write` 호출
5. 성공 시 activity stream 에 `KnowledgeWriteback` 이벤트 추가
6. candidate projection 에 `canonical_path` 와 write correlation 추가

중요한 점:

- candidate acceptance 와 canonical write 는 같은 개념이 아니다.
- accept 는 semantic approval 이고, write 는 별도의 ACL / conflict / store write 단계다.
- write 실패 시 candidate 는 `Accepted` 상태를 유지하되, `materialization_status=failed` read model 을 노출한다.

이유:

- 사람이 보기에 "기록해야 한다" 와 "지금 저장 인프라가 성공했다" 는 다른 축이다.
- write 실패를 이유로 candidate 를 다시 `PendingPennyDecision` 로 되돌리면 의미 결정과 인프라 실패가 섞인다.

### 2.7 에러 & 로깅

주요 에러:

- `GadgetronError::ActivityCapture`
  invalid candidate hint, missing actor, malformed provenance, duplicate decision race
- `GadgetronError::Knowledge`
  canonical write ACL failure, path validation failure, store write failure
- `GadgetronError::Penny`
  candidate summarization/decision 요청 실패

operator-facing error 원칙:

- 무엇이 일어났는지: "지식 후보를 기록할 수 없습니다"
- 왜인지: "caller 권한으로는 해당 경로에 쓸 수 없거나 후보 상태가 아직 승인되지 않았습니다"
- 무엇을 할지: "candidate 상태와 대상 scope/path 를 확인한 뒤 다시 시도하세요"

필수 tracing/audit events:

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
| candidate summary / path | bundle hint -> core candidate store | Tampering | hint normalize, path sanitize, authoritative disposition 금지 |
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
  - 기존 `KnowledgeService` 와 연결
- `gadgetron-xaas`
  - activity/candidate persistence schema 와 projection 지원
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
          ActivityCaptureStore append-only log
                     |
                     v
          KnowledgeCandidate projection/read model
               |                         |
               v                         v
         Penny curation             bundle viewer UI
               |                         |
               +------------+------------+
                            |
                            v
                 KnowledgeService::write
                            |
                            v
                  canonical store / index / relation
```

### 3.2 타 모듈과의 인터페이스 계약

- `10-penny-permission-inheritance.md`
  Penny 의 decision path 역시 caller inheritance 를 그대로 따라야 한다.
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
  coordinator, knowledge write integration, disposition policy
- `gadgetron-gateway`
  actor resolution, request correlation, read/write endpoints
- `gadgetron-web`
  workbench projection 렌더링
- `gadgetron-xaas`
  persistence, projection, retention jobs

즉, core 는 contract 를 소유하고, orchestration/persistence/rendering 은 각 책임 계층에 남긴다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

- `candidate_hint_path_escape_rejected`
- `candidate_initial_disposition_is_never_accepted`
- `candidate_decision_race_returns_conflict`
- `accepted_candidate_requires_second_acl_check`
- `materialization_failure_does_not_revert_acceptance`
- `same_request_duplicate_hints_are_deduped`
- `pending_candidates_filtered_by_actor_scope`
- `external_runtime_hint_cannot_set_authoritative_fields`

### 4.2 테스트 하네스

- in-memory append-only event store fake
- deterministic projection reducer fixture
- fake `KnowledgeService` materializer
- tempdir canonical path validator
- property-based test:
  direct action -> Penny decision -> materialization sequence permutation 에서 불변식 유지

### 4.3 커버리지 목표

- `gadgetron-core::activity` / candidate types: branch 90% 이상
- `gadgetron-knowledge` candidate coordinator: branch 90% 이상
- projection reducer / ACL re-check path: branch 95% 이상

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

1. direct workbench action -> capture -> pending candidate -> Penny accept -> canonical write
2. direct workbench action -> capture -> Penny escalate -> user confirm -> canonical write
3. direct workbench action -> capture -> Penny reject -> no canonical write
4. external runtime action -> candidate hint -> normalized candidate -> acceptance
5. accepted candidate + write failure -> read model surfaces failed materialization

### 5.2 테스트 환경

- gateway auth fixture with multiple users/teams
- Postgres testcontainers for append-only event tables + projection tables
- fake Penny decision provider
- web read-model contract tests for pending/accepted/rejected rendering

### 5.3 회귀 방지

- direct action 이 capture 없이 실행되면 실패해야 한다
- candidate 가 승인 전 canonical search/citation 에 노출되면 실패해야 한다
- Penny accept 가 caller write 권한을 우회하면 실패해야 한다
- write 실패가 candidate history 를 덮어쓰거나 삭제하면 실패해야 한다
- external bundle 이 `Accepted` candidate 를 직접 반환해도 시스템이 받아들이면 실패해야 한다

---

## 6. Phase 구분

### 6.1 [P2B]

- append-only activity capture
- `KnowledgeCandidate` lifecycle
- Penny accept/reject/escalate loop
- workbench + CLI read model 노출
- canonical writeback 연결

### 6.2 [P2C]

- richer candidate heuristics (incident/runbook/policy taxonomy)
- relation-aware candidate summarization
- candidate clustering / duplicate merge

### 6.3 [P3]

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
