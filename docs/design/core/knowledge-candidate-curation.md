# Knowledge Candidate Lifecycle & Penny Curation Loop

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18 (PSL-1d chat-capture conformance reconcile for recent `origin/main`)
> **관련 크레이트**: `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-cli`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-web`, `gadgetron-xaas`
> **Phase**: [P2B] landed in-memory slice + canonical materialization + production startup wiring + first chat-completion capture hook / [P2B-next] additional capture owners + projection hardening / [P2C] richer heuristics + user-confirmation routing / [P3] automation candidates
> **관련 문서**: `docs/design/core/knowledge-plug-architecture.md`, `docs/design/web/expert-knowledge-workbench.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/11-raw-ingestion-and-rag.md`, `docs/design/phase2/12-external-gadget-runtime.md`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/process/04-decision-log.md`

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

2026-04-18 `origin/main` (`W3-KC-1` + `W3-KC-1b` + `W3-PSL-1c` + `W3-PSL-1d`, PR #85 / #87 / #89 / #91, `docs/process/04-decision-log.md` D-20260418-17 / D-20260418-18 / D-20260418-19 / D-20260418-20) 기준 trunk 에 실제로 들어온 것은 "contract + in-memory slice + optional canonical writeback + production startup wiring + first request-owner capture hook + smoke gates" 까지다.

- [P2B] landed
  - `gadgetron-core::knowledge::candidate` 의 wire types / traits / serde contract
  - `ActivityCaptureStore::get_candidate()` fast-path 추가
  - `gadgetron-knowledge::candidate` 의 `InMemoryActivityCaptureStore`, `InProcessCandidateCoordinator`
  - builder-style `InProcessCandidateCoordinator::with_knowledge_service()` / `.with_path_rules()`
  - `ActivityKind` snake_case key 기반 `{date}` / `{topic}` / `{author}` `path_rules` expansion
  - coordinator 에 `KnowledgeService` 가 주입된 경우의 `materialize_accepted_candidate() -> KnowledgeService::write()` canonical writeback
  - `gadgetron-cli::init_serve_runtime()` 이 `[knowledge]` 설정을 읽어 `build_knowledge_service()`, `build_candidate_plane()`, `build_workbench()`, `build_penny_shared_context()` helper chain 으로 production `AppState` 를 조립
  - `[knowledge]` 가 없을 때도 `build_workbench(None)` 이 degraded projection 을 wiring 하므로, workbench bootstrap surface 는 404 대신 degraded contract 를 유지한다
  - `[knowledge]` + `[knowledge.curation].enabled = true` 일 때만 `activity_capture_store`, `candidate_coordinator`, `penny_shared_surface`, `penny_assembler` 가 live wiring 된다
  - `gadgetron-gateway::handlers::capture_chat_completion()` 이 첫 production `capture_action()` owner hook 으로 landed 했고, 성공한 `/v1/chat/completions` 요청을 fire-and-forget activity event 로 append 한다
  - non-streaming chat 성공 경로는 실제 `prompt_tokens` / `completion_tokens` 를 기록하고, streaming 경로는 dispatch 시점 `0/0` placeholder usage 로 capture 한다
  - PSL-1d capture event 는 `origin = Penny`, `kind = GadgetToolCall`, `source_capability = "chat.completions"`, `actor_user_id = Uuid::nil()` (doc-10 placeholder), `hints = []` 를 사용한다. `audit_event_id = Some(audit_event_id)` 은 **drift-fix PR 5 (D-20260418-24)** 이후 handler 가 `AuditEntry.event_id` 와 동일한 `Uuid::new_v4()` 를 미리 생성해 양쪽에 주입하므로 `audit_log.id ↔ captured_activity_event.audit_event_id` JOIN 이 실제 primary key 단위로 성립한다. `request_id` 재사용이 아니다 — 같은 request 가 여러 audit row 를 만드는 streaming 경로 (Phase-2 Drop-guard) 에서도 각 행이 고유 event_id 를 가진다.
  - `gadgetron-gateway::penny::shared_context` 의 wire-stable `pending_penny_decision` renderer
  - `crates/gadgetron-gateway/tests/kc1_fixture_diff.rs`, `crates/gadgetron-knowledge/tests/kc1b_canonical_write_e2e.rs`, `crates/gadgetron-cli/src/main.rs` 의 PSL-1c smoke tests, `crates/gadgetron-gateway/tests/psl_1d_chat_capture.rs`
- [P2B-next] follow-up still open
  - workbench action / approval decision owner 가 `capture_action()` call site 를 실제 request path 에 연결하는 것
  - chat completion owner hook 에 real `actor_user_id`, `audit_event_id`, hint generation, streaming final usage accounting 을 연결하는 것
  - workbench/read-model 에 `canonical_path`, `materialization_status`, `audit_event_id` 를 영속적으로 반영하는 projection
  - config default `path_rules` 와 KC-1b runtime key-space(`direct_action`, `runtime_observation` 등) 의 정렬
  - startup-wired candidate plane 의 same-turn guarantee 를 실제 live request flow 에서 검증하는 HTTP-level smoke/e2e
- [P2C+] intentionally deferred
  - `PgActivityCaptureStore` / retention jobs / projection persistence
  - `require_user_confirmation_for` routing semantics
  - auto-prompt Penny behavior
  - audit correlation (`audit_event_id`) 의 full plumbing
  - richer candidate heuristics / clustering / scoring

따라서 이 문서는 "이상적인 최종 구조" 만 설명하지 않는다. **현재 trunk authority 와 KC-1b / PSL-1c landed behavior, 그리고 KC-1c follow-up seam 을 함께 고정하는 문서**로 사용한다.

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
    // drift-fix PR 2 (D-20260418-26) narrowed this from `Option<String>`.
    pub proposed_path: Option<gadgetron_core::knowledge::KnowledgePath>,
    pub provenance: BTreeMap<String, String>,
    pub disposition: KnowledgeCandidateDisposition,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateHint {
    pub summary: String,
    // drift-fix PR 2 (D-20260418-26) narrowed this from `Option<String>`.
    pub proposed_path: Option<gadgetron_core::knowledge::KnowledgePath>,
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
- ~~`proposed_path` 와 `KnowledgeDocumentWrite` 는 현재 trunk 에서 계속 `String` 기반 wire shape 로 유지한다.~~ **(업데이트 2026-04-18)** drift-fix PR 2 (D-20260418-26) 에서 `CandidateHint.proposed_path` / `KnowledgeCandidate.proposed_path` / `KnowledgeDocumentWrite.path` / `PennyCandidateDigest.proposed_path` / `PennyCandidateDecisionReceipt.canonical_path` 및 `materialize_accepted_candidate` 리턴을 모두 `gadgetron_core::knowledge::KnowledgePath` 로 narrow. wire shape (JSON bare string) 는 동일하게 유지됨 — `#[serde(try_from = "String", into = "String")]` 로 routing. `KnowledgeStore`-API 쪽 `KnowledgeDocument.path` / `KnowledgePutRequest.path` / `KnowledgeWriteReceipt.path` 는 여전히 `String` (별도 PR 로 narrow 고려).
- [P2B] 현재 trunk 의 첫 live owner hook 은 `gadgetron-gateway::handlers` 가 `capture_action()` 에 넘기는 `CapturedActivityEvent` 이다. 이 hook 은 `source_capability = "chat.completions"`, `origin = Penny`, `kind = GadgetToolCall`, `hints = []` 를 고정한다.
- [P2B] chat completion capture payload 는 **non-PII** 다. 제목/요약/`facts` 에 사용자 message text, reasoning text, secret, raw prompt 를 넣지 않고 모델명, stream 여부, token count 만 허용한다.

### 2.2 내부 구조

구조는 현재 trunk authority 기준으로 **capture/store/projection core + startup assembly + Penny bridge + optional materialization seam** 으로 분리한다.

1. **Capture ingress** `[P2B landed]`
   gateway, Penny, workbench action host, bundle runtime 이 `CapturedActivityEvent` 입력을 만든다.
2. **Append-only event store** `[P2B landed, in-memory only]`
   현재 trunk 는 activity event / candidate / decision 을 `tokio::sync::Mutex` 로 감싼 in-memory store 에 둔다. 영속 스토어는 아직 아니다.
3. **Candidate projection** `[P2B landed]`
   현재 disposition, latest rationale, proposed path, acceptance 가능 여부를 계산한다.
4. **Bootstrap assembly (`gadgetron-cli`)** `[P2B landed]`
   `init_serve_runtime()` 는 `load_knowledge_config_from_path()` 로 `[knowledge]` 섹션을 읽고, optional PostgreSQL semantic backend 연결을 시도한 뒤 `KnowledgeService`, workbench projection, candidate plane, Penny shared-surface assembler 를 순서대로 조립한다.
5. **Penny curation loop bridge** `[P2B landed, boot-gated on knowledge + curation]`
   Penny shared-surface service 가 pending candidate 를 읽고 accept/reject/escalate 결정을 기록한다. 단 live boot 에서는 `knowledge_service.is_some() && curation.enabled` 일 때만 이 bridge 가 `AppState` 에 연결된다.
6. **Canonical knowledge materializer** `[P2B landed]`
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

#### 2.2.0 Wire-stable snake_case 라벨 계약 `[P2B landed]`

`ActivityOrigin`, `ActivityKind`, `KnowledgeCandidateDisposition`, `CandidateDecisionKind` 모두 `#[serde(rename_all = "snake_case")]` 로 선언되어 있다. 이 라벨 문자열은 다음 네 경로에서 동일하게 나타난다:

1. **PostgreSQL column values** — `activity_events.origin`, `activity_events.kind`, `knowledge_candidates.disposition`, `candidate_decisions.decision` 컬럼은 snake_case 문자열을 그대로 저장한다 (`crates/gadgetron-xaas/migrations/20260418000001_activity_capture.sql`).
2. **Audit JSON facts** — `capture_chat_completion()` 의 `facts` payload 가 origin/kind 를 JSON 으로 내보낼 때.
3. **`<gadgetron_shared_context>` block** — `render_penny_shared_context()` 가 `[pending_penny_decision]` 형태로 emit 하는 disposition 라벨.
4. **`path_rules` keys** — `[knowledge.curation].path_rules` TOML map 의 key 가 snake_case `ActivityKind` variant 를 사용한다 (§2.3).

이 네 경로가 모두 동일한 렌더러를 쓰지 않으면 조용히 갈라져서 wire-break 이 된다. 따라서 canonical 렌더러는 하나뿐이다:

```rust
// crates/gadgetron-core/src/knowledge/candidate.rs
pub fn snake_case_label<E: Serialize>(e: &E) -> String {
    serde_json::to_value(e)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}
```

규칙:

- **Single source of truth.** 모든 crate (`gadgetron-core`, `gadgetron-knowledge`, `gadgetron-gateway`) 는 위 함수를 import 해서 쓴다. sibling copy 를 만드는 것은 drift 의 씨앗 — 삭제 대상.
- **Debug-lowercase 금지.** `format!("{:?}", disposition).to_lowercase()` 은 variant 이름이 바뀌지 않는 한 "얼추 같은" 결과를 내지만, serde 의 `rename_all` 이 개별 variant 에 `#[serde(rename = ...)]` 을 추가하면 즉시 divergent 하게 간다. PSL-1 초기 구현은 이 방식을 썼고 KC-1b 에서 교체됐다.
- **Fallback `"unknown"`.** 바레 JSON string 이 아닌 값 (즉 unit-variant 가 아닌 enum 또는 struct) 은 에러 대신 `"unknown"` 로 렌더한다. 이는 미래에 variant 가 튜플/struct-like 로 바뀌는 것을 방지하는 게 아니라, renderer 호출부의 `Result` 전파를 피하기 위한 pragmatic 선택이다. unit-variant 만 받도록 `#[non_exhaustive]` + Clippy lint 로 enforcement 는 그대로 유지된다.
- **마이그레이션 contract.** 문자열을 바꾸려면 (1) `#[serde(rename = ...)]` 추가, (2) Postgres 마이그레이션으로 기존 row 업데이트, (3) 문서화된 audit JSON 소비자에게 공지 — 3단 플레이. variant 추가는 safe (enum 이 `#[non_exhaustive]` 이므로), 이름 바꾸기는 wire-break.

Ref: D-20260418-23 (drift-fix PR 1 U-A), `crates/gadgetron-core/src/knowledge/candidate.rs::snake_case_label`.

#### 2.2.1 PSL-1d 첫 번째 live capture owner (`/v1/chat/completions`) [P2B landed]

현재 trunk 는 첫 production `capture_action()` owner hook 을 `gadgetron-gateway::handlers` 에 둔다. 이는 "후보 승격"이 아니라 "사실 이벤트 append" 를 먼저 live traffic 에 연결하는 최소 단위다.

```rust
async fn capture_chat_completion(
    coordinator: Arc<dyn KnowledgeCandidateCoordinator>,
    tenant_id: Uuid,
    request_id: Uuid,
    model: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    stream: bool,
) {
    let event = CapturedActivityEvent {
        tenant_id,
        actor_user_id: Uuid::nil(),
        request_id: Some(request_id),
        origin: ActivityOrigin::Penny,
        kind: ActivityKind::GadgetToolCall,
        title: format!("chat completion: {model}"),
        summary: format!(
            "{prompt_tokens} input / {completion_tokens} output tokens, stream={stream}"
        ),
        source_bundle: None,
        source_capability: Some("chat.completions".into()),
        // drift-fix PR 5 (D-20260418-24): handler generates a fresh Uuid::new_v4()
        // per outcome and uses it for BOTH AuditEntry.event_id and this field so
        // audit_log.id ↔ captured_activity_event.audit_event_id JOIN is unambiguous.
        audit_event_id: Some(audit_event_id),
        facts: json!({
            "model": model,
            "stream": stream,
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
        }),
        ..
    };

    coordinator.capture_action(&AuthenticatedContext, event, vec![]).await
}
```

운영 계약:

- `handle_non_streaming` 은 `router.chat(req).await` 의 `Ok(response)` arm 에서만 capture 를 spawn 한다. 실패 arm 은 [P2B] 범위 밖이다.
- `handle_streaming` 은 dispatch 시점에 capture 를 spawn 하고, usage count 는 `0/0` placeholder 로 둔다. 정확한 streaming usage 는 [P2C] Drop-guard follow-up 이 소유한다.
- capture 는 `tokio::spawn` fire-and-forget 이며, 실패해도 chat response 는 실패시키지 않는다.
- warn log 키는 `penny_shared_context.capture_chat_failed` 다. bootstrap degrade 와 같은 철학으로 "관측성은 남기되 사용자 요청은 계속 처리" 가 current contract 다.
- `hints = []` 이므로 이 hook 만으로는 `KnowledgeCandidate` 가 생성되지 않는다. 현재 landed 효과는 `recent_activity` 채우기이며, `pending_candidates` 증가는 여전히 다른 owner hook 또는 hint-generation follow-up 이 필요하다.

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

- `enabled=false` 는 이제 production startup guard 에 실제로 연결된다. 현재 trunk 에서는 workbench projection 은 계속 wiring 되지만 `activity_capture_store`, `candidate_coordinator`, `penny_shared_surface`, `penny_assembler` 는 `None` 으로 남는다.
- `capture_retention_days` 는 7 이상이어야 한다. 그보다 짧으면 incident review 가 불가능해진다.
- `candidate_retention_days` 는 `capture_retention_days` 보다 클 수 없다.
- `max_candidates_per_request` 는 1 이상 32 이하로 제한한다.
- `require_user_confirmation_for` 는 [P2B] 현재 비어 있지 않은 string label 만 요구한다. enum narrowing 은 [P2C] user-confirmation semantics 와 함께 굳힌다.
- `path_rules.*` 는 [P2B] 현재 validation 시 parent traversal (`../`, `..` segment) 만 거부한다. placeholder vocabulary, key-space(`operations` vs `direct_action`) 는 아직 validate 하지 않는다.
- `InProcessCandidateCoordinator::with_path_rules()` 로 주입된 map 은 [P2B] 현재 `ActivityKind` snake_case key (`direct_action`, `gadget_tool_call`, `runtime_observation`, ...) 를 기준으로 `{date}` / `{topic}` / `{author}` 만 확장한다.
- 따라서 current trunk 는 **config default surface 와 coordinator runtime semantics 가 아직 완전히 합쳐지지 않았다**. PSL-1c 로 startup wiring 이 landed 하면서 이 drift 는 더 이상 test-only 가 아니고, hint 에 `proposed_path = None` 인 live capture call site 가 붙는 순간 operator-facing 동작으로 드러난다.
- `auto_prompt_penny`, `require_user_confirmation_for`, `path_rules` application 은 config surface 와 coordinator/service layer 까지 landed 했다. 다만 실제 request path 에서 `capture_action()` 을 호출하는 owner hook 은 후속이다.

운영자 touchpoint:

- `GET /activity` 와 `GET /request_evidence` workbench surface 가 capture/evidence read model 을 노출한다.
- Penny gadget 의 `workbench.candidates_pending` / `workbench.candidate_decide` 가 같은 shared-surface contract 를 소비한다.
- workbench bootstrap surface 는 현재 production boot 에서도 항상 wiring 되며, knowledge service 가 없으면 degraded projection 으로 응답한다.
- `[knowledge]` + `[knowledge.curation].enabled = true` 인 서버에서는 candidate plane 과 Penny assembler 가 live 가 된다.
- current trunk 에서는 성공한 chat completion 이 실제 `CapturedActivityEvent` 로 append 되므로, coordinator 가 wiring 된 서버의 `recent_activity` 는 더 이상 항상 빈 목록이 아니다.
- 반면 `pending_candidates` 는 여전히 빈 목록으로 시작하는 것이 정상이다. PSL-1d owner hook 이 `hints = []` 를 사용하고, workbench/approval owner hook 도 아직 landed 하지 않았기 때문이다.
- 현재 landed verification gate 는 `kc1_fixture_diff`, `kc1b_canonical_write_e2e`, `psl_1c_startup_wires_penny_assembler_and_observability_fields`, `psl_1c_no_knowledge_section_leaves_observability_fields_none`, `psl_1d_successful_non_streaming_chat_completions_captures_one_event` 다섯 축이다.

### 2.4 이벤트와 후보 생성 규약

candidate 생성은 bundle 반환을 그대로 신뢰하지 않는다.

정책:

1. bundle/runtime 이 `CandidateHint` 를 0..N 개 반환할 수 있다.
2. core 는 actor, tenant, source capability, audit correlation 을 다시 주입한다.
3. [P2B] 현재 landed slice 는 `proposed_path` 를 advisory `Option<String>` 으로 보존한다. authoritative path sanitize / ACL re-check 는 materialization boundary 의 책임이며, `path_rules` template expansion 은 capture 시점에 hint path 가 비어 있을 때만 적용된다.
4. [P2B] 현재 coordinator 는 전달된 힌트를 clamp + append 한다. hint 의 `proposed_path` 가 비어 있으면 `path_rules` 를 조회하고, 매칭이 없으면 `ops/journal/<YYYY-MM-DD>/<activity_kind>` fallback string 을 넣는다. 힌트가 전혀 없는 activity 에 대한 server-generated candidate 는 [P2B-next / P2C] 로 미룬다.
5. [P2B] 현재 landed `chat completion` owner hook 은 의도적으로 `vec![]` 힌트를 전달한다. 즉, 성공한 chat traffic 은 activity feed 를 채우지만 candidate inbox 를 직접 늘리지 않는다.
6. external runtime 은 candidate 를 `Accepted` 상태로 만들 수 없다.

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
- production startup path 자체는 이제 candidate plane 을 wiring 할 수 있다. 또한 current trunk 는 성공한 chat completion 을 fire-and-forget capture 로 append 하므로 next-turn `recent_activity` freshness 는 live 로 성립한다.
- 다만 same-turn guarantee 가 live 로 성립하려면 해당 요청 owner 가 같은 요청 흐름에서 `capture_action()` 과 적절한 hint generation 을 모두 제공해야 한다. 현재 trunk 에서는 workbench/approval owner hook, chat hint generation, streaming final usage accounting 이 아직 없으므로 pending candidate same-turn 보장은 service/test harness authority 로 더 넓게 남는다.

### 2.6 Canonical write 승격 규칙

이 문서가 authoritative 로 고정하는 핵심은 **`Accepted` 와 `materialized` 는 같은 상태가 아니다** 라는 점이다. 2026-04-18 `origin/main` 기준으로는 두 단계가 모두 일부 landed 했지만, 후자의 operator-visible projection 은 아직 미완성이다.

[P2B] current trunk / KC-1b:

1. `materialize_accepted_candidate()` 는 `get_candidate()` fast-path 로 latest row 를 조회한다.
2. latest disposition 이 `Accepted` 가 아니면 `GadgetronError::Knowledge { kind = InvalidQuery }` 로 실패한다.
3. coordinator 에 `knowledge_service` 가 주입되어 있으면 `KnowledgeDocumentWrite` 를 `KnowledgePutRequest { path, markdown, create_only: false, overwrite: true }` 로 변환해 `KnowledgeService::write` 를 호출한다.
4. canonical write 성공 시 `KnowledgeWriteReceipt.path` 를 반환한다.
5. coordinator 에 `knowledge_service` 가 없으면 `candidate.proposed_path` 또는 synthetic fallback string 을 반환한다. 이 경로는 dev/test, 직접 service 인스턴스를 조립한 경로, 또는 future partial-wiring 실험 경로 호환을 위한 것이다.
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
- [P2B landed] `gadgetron-cli` startup smoke 는 `workbench`, `penny_shared_surface`, `penny_assembler`, `activity_capture_store`, `candidate_coordinator` field-presence gate 를 가진다.
- [P2B landed] `gadgetron-gateway::handlers` 는 chat capture 실패 시 `penny_shared_context.capture_chat_failed` warn event 를 남기고, request 성공/실패 semantics 는 바꾸지 않는다.
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
  - [P2B-next] persistence / projection
- `gadgetron-cli`
  - [P2B] startup orchestration (`load_knowledge_config_from_path`, optional semantic backend connect, AppState wiring)
  - [P2B-next] request-owner capture hook wiring and operator-facing troubleshooting polish
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
  [P2B-next] persistence / projection
- `gadgetron-cli`
  [P2B] startup assembly of `KnowledgeService`, workbench, candidate plane, Penny shared-surface bridge
  [P2B-next] request-owner capture hook wiring, config/runtime drift hardening
- `gadgetron-gateway`
  actor resolution, request correlation, Penny shared-surface adapter, first chat-completion capture owner hook, fixture-diff integration gate
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
  - `psl_1c_startup_wires_penny_assembler_and_observability_fields`
  - `psl_1c_no_knowledge_section_leaves_observability_fields_none`
  - `psl_1d_successful_non_streaming_chat_completions_captures_one_event`
- [P2B-next / P2C]
  - `candidate_hint_path_escape_rejected_at_materialization_boundary`
  - `accepted_candidate_requires_second_acl_check`
  - `materialization_failure_does_not_revert_acceptance`
  - `same_request_duplicate_hints_are_deduped`
  - `pending_candidates_filtered_by_actor_scope`
  - `external_runtime_hint_cannot_set_authoritative_fields`
  - `streaming_chat_capture_reconciles_final_usage_after_drop_guard`

### 4.2 테스트 하네스

- [P2B landed] in-memory append-only event store + fake workbench projection + deterministic fixture-diff normalizer + tempdir wiki-backed `KnowledgeService`
- [P2B landed] tempdir `gadgetron.toml` fixture + `load_knowledge_config_from_path()` + `build_app_state(AppStateParts { ... })` smoke harness
- [P2B landed] fake provider + real axum router + in-memory candidate coordinator + bounded settle wait (`tokio::time::sleep`) for fire-and-forget capture assertion
- [P2B-next] multi-user auth fixture + projection receipt assertions + canonical path validator
- [P2C] property-based test:
  direct action -> Penny decision -> materialization sequence permutation 에서 불변식 유지

### 4.3 커버리지 목표

- [P2B] `gadgetron-core::knowledge::candidate` / `gadgetron-knowledge::candidate`: branch 90% 이상
- [P2B] `gadgetron-cli` startup helper / guard path: branch 85% 이상
- [P2B-next] capture hook / ACL re-check / projection path: branch 95% 이상

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

1. [P2B landed] `kc1_fixture_diff`: capture event -> 2 hints -> bootstrap render -> accept 1 candidate -> bootstrap render diff
2. [P2B landed] `kc1b_canonical_write_e2e`: capture -> path_rules expansion -> accept -> materialize -> `knowledge.search()` 가 canonical page 를 찾는다
3. [P2B landed] `psl_1d_chat_capture`: successful non-streaming chat -> fire-and-forget capture -> in-memory store contains one `GadgetToolCall` activity event
4. [P2B-next] successful chat -> next-turn bootstrap/render path 에서 live `recent_activity` entry 가 shared-context block 에 나타나는 HTTP-level smoke
5. [P2B-next] direct workbench action -> capture -> pending candidate -> Penny accept -> startup-wired canonical write
6. [P2B-next] direct workbench action -> capture -> Penny escalate -> user confirm -> canonical write
7. [P2B-next] direct workbench action -> capture -> Penny reject -> no canonical write
8. [P2C] external runtime action -> candidate hint -> normalized candidate -> acceptance
9. [P2B-next] accepted candidate + write failure -> read model surfaces failed materialization

### 5.2 테스트 환경

- [P2B landed] in-memory store + fake projection + shared-context renderer
- [P2B landed] tempdir wiki + `LlmWikiStore` + keyword index + `KnowledgeService`
- [P2B landed] fake `LlmProvider`, real gateway router, `InMemoryActivityCaptureStore`, real `InProcessCandidateCoordinator`
- [P2B-next] real router + fake provider + startup-built `AppState` 에서 `<gadgetron_shared_context>` injected chat POST 검증
- [P2B-next] gateway auth fixture with multiple users/teams + Postgres testcontainers for append-only event tables + projection tables
- [P2B-next] fake Penny decision provider + boot-wired `KnowledgeService`
- [P2B-next / P2C] web read-model contract tests for pending/accepted/rejected/materialization-warning rendering

### 5.3 회귀 방지

- direct action 이 capture 없이 실행되면 실패해야 한다
- [P2B landed] successful non-streaming chat 이 coordinator wired 상태에서 activity event 를 남기지 못하면 실패해야 한다
- candidate 가 승인 전 canonical search/citation 에 노출되면 실패해야 한다
- [P2B landed] wired materialization 이 canonical wiki 에 기록하지 못하면 실패해야 한다
- [P2B landed] `[knowledge]` + `curation.enabled=true` 인데 startup 후 `penny_assembler` 가 `None` 이면 실패해야 한다
- [P2B landed] `[knowledge]` 가 없는데 degraded workbench projection 자체가 사라지면 실패해야 한다
- [P2B-next] Penny accept 가 caller write 권한을 우회하면 실패해야 한다
- [P2B-next] write 실패가 candidate history 를 덮어쓰거나 삭제하면 실패해야 한다
- [P2B-next] streaming chat capture 가 final usage accounting 을 영구적으로 `0/0` 에 고정하면 실패해야 한다
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
- production startup wiring for `knowledge_service` / workbench / candidate plane / Penny shared surface
- first live `capture_action()` owner hook for successful `/v1/chat/completions`
- Penny shared-surface pending/decide adapter
- wire-stable `pending_penny_decision` renderer
- deterministic fixture-diff + canonical-write E2E + startup smoke + chat-capture integration exit gates
- local `String`-based `KnowledgeDocumentWrite` / `proposed_path`

### 6.2 [P2B-next] live capture hooks + projection hardening

- request-owner `capture_action()` wiring (workbench action / approval decision / other non-chat surfaces)
- chat completion hint generation + real actor/audit correlation + next-turn bootstrap smoke
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
| Q-5 | `path_rules` 기본 key(`operations`, `incident`, `research`) 와 runtime `ActivityKind` key(`direct_action`, `runtime_observation` 등) 를 어떻게 정렬할 것인가 | A. default key-space 를 runtime snake_case 로 맞춤 / B. startup alias translation 추가 | **A** | 🟡 |

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

### Round 1 (PSL-1c Startup Wiring Conformance) — 2026-04-18 — @gateway-router-lead @devops-sre-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [x] 인터페이스 계약 — `AppState` 에 live wiring 되는 field 집합과 본문 phase 설명이 일치한다
- [x] 크레이트 경계 — startup orchestration 은 `gadgetron-cli`, contract/store 는 `gadgetron-core` / `gadgetron-knowledge` 에 남아 있다
- [x] 타입 중복 — boot helper 설명이 기존 shared-context / candidate types 를 재정의하지 않는다
- [x] 에러 반환 — current trunk 가 추가 error variant 없이 기존 `GadgetronError` surface 를 사용함을 유지했다
- [x] 동시성 — startup wiring 과 request-time candidate mutation 경계를 혼동하지 않도록 분리했다
- [x] 의존성 방향 — CLI 가 knowledge/gateway surface 를 조립할 뿐 역방향 의존을 만들지 않는다
- [x] Phase 태그 — landed startup wiring 과 follow-up capture hook / projection work 가 분리되었다
- [x] 레거시 결정 준수 — D-12 crate boundary 와 충돌하지 않는다

**Action Items**:
- A1: `gadgetron-cli` startup assembly 를 [P2B landed] 로 승격하고 `gadgetron-knowledge` follow-up 에서 제거했다.
- A2: `build_workbench(None)` 의 degraded projection 계약과 `knowledge + curation` guard 를 operator-facing touchpoint 로 명시했다.
- A3: per-event `capture_action()` hook 과 projection hardening 만 [P2B-next] 로 남겼다.

**Open Questions**:
- Q-1 ~ Q-5 유지

**다음 라운드 조건**: 없음. Round 1.5 진행 가능.

### Round 1.5 (PSL-1c Startup Wiring Conformance) — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1.5` 기준)
- [x] 위협 모델 (필수) — live boot wiring 이후에도 caller inheritance / advisory path / writeback privilege 경계가 유지된다
- [x] 신뢰 경계 입력 검증 — config/runtime drift 와 `path_rules` key-space mismatch 가 명시적으로 surfaced 되었다
- [x] 인증·인가 — `curation.enabled=false` 시 shared-surface bridge 가 비활성화된다는 guard 가 문서화되었다
- [x] 시크릿 관리 — startup wiring 추가로도 secret-bearing fields 를 새로 노출하지 않는다
- [x] 공급망 — 신규 dependency 요구가 없다
- [x] 암호화 — semantic backend optional PG fallback 이 암호 primitive 변경 없이 유지된다
- [x] 감사 로그 — capture hook 미landing 사실과 future audit events 가 분리되어 있다
- [x] 에러 정보 누출 — operator-facing wording 이 internal structure dump 없이 remediation 을 제공한다
- [x] LLM 특이 위협 — Penny 가 private memory 대신 shared-surface projection 만 읽는다는 경계가 유지된다
- [x] 컴플라이언스 매핑 — CC6.6 / GDPR Art.32 / HIPAA 164.312(b) 서술이 live wiring 기준으로도 유효하다
- [x] 사용자 touchpoint 워크스루 — degraded workbench, live Penny assembler, empty pending candidate 정상 상태가 모두 설명된다
- [x] 에러 메시지 3요소 — startup / materialization 실패 설명이 what/why/how-to-fix 형식을 유지한다
- [x] CLI flag — 새로운 flag surface 없이 config gate 로만 동작한다
- [x] API 응답 shape — shared-context injection follow-up 과 candidate lifecycle contract 가 OpenAI-compatible chat shape 를 깨지 않는다
- [x] config 필드 — `[knowledge.curation]` 의 live guard semantics 가 문서와 일치한다
- [x] defaults 안전성 — default `path_rules` drift 가 숨겨지지 않고 오픈 이슈로 승격되었다
- [x] 문서 5분 경로 — startup smoke gate 와 degraded behavior 설명이 운영자 확인 경로를 제공한다
- [x] runbook playbook — "assembler None / degraded projection / empty pending list" 의 정상 vs 비정상 구분이 가능하다
- [x] 하위 호환 — `[knowledge]` 없는 서버가 hard-fail 하지 않고 degraded workbench 를 유지한다
- [x] i18n 준비 — 사용자-facing 문구가 분리 가능 서술 수준을 유지한다

**Action Items**:
- A1: live wiring 이후 operator 가 실제로 보게 되는 degraded/default states 를 touchpoint 항목으로 재서술했다.
- A2: `curation.enabled=false` 가 이제 실제 boot guard 라는 점을 보안/사용성 양쪽에서 고정했다.
- A3: `path_rules` key-space drift 를 더 이상 hidden follow-up 으로 두지 않고 open issue 로 승격했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 2 진행 가능.

### Round 2 (PSL-1c Startup Wiring Conformance) — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §2` 기준)
- [x] 단위 테스트 범위 — KC-1/KC-1b tests 와 PSL-1c startup smoke tests 가 함께 명시되었다
- [x] mock 가능성 — tempdir config + fake projection + wiki-backed `KnowledgeService` harness 가 구체적이다
- [x] 결정론 — helper-chain smoke 는 네트워크/시계 의존 없이 deterministic 하다
- [x] 통합 시나리오 — future live HTTP-level capture-hook 시나리오가 landed smoke 와 분리되어 있다
- [x] CI 재현성 — `gadgetron-cli` helper smoke 와 future Postgres/container path 를 혼합하지 않는다
- [x] 성능 검증 — startup orchestration coverage 와 request-path heavy tests 를 분리했다
- [x] 회귀 테스트 — `penny_assembler` wiring regression 과 degraded workbench regression 둘 다 잡을 수 있다
- [x] 테스트 데이터 — tempdir `gadgetron.toml` fixture 와 wiki fixture 사용이 명확하다

**Action Items**:
- A1: PSL-1c smoke tests 두 개를 landed unit gate 로 추가했다.
- A2: future router-level `<gadgetron_shared_context>` chat POST 검증은 [P2B-next] HTTP integration 으로 분리했다.
- A3: coverage 목표를 startup helper path 와 future capture hook path 로 나눴다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 3 진행 가능.

### Round 3 (PSL-1c Startup Wiring Conformance) — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §3` 기준)
- [x] Rust 관용구 — public contract 와 trait-object seam 설명이 현재 API 와 일치한다
- [x] 제로 비용 추상화 — startup wiring 비용과 request hot path 비용을 분리해 과장하지 않았다
- [x] 제네릭 vs 트레이트 객체 — boot assembly 가 trait-object surface 를 조립하는 위치라는 점이 명확하다
- [x] 에러 전파 — optional knowledge service / degraded projection / future materialization failure 가 구분된다
- [x] 수명주기 — startup-built `Arc` graph 와 future request-owner hook 경계가 명시된다
- [x] 의존성 추가 — 신규 crate/dep 요구가 없다
- [x] 트레이트 설계 — `ActivityCaptureStore` / `KnowledgeCandidateCoordinator` / Penny bridge 책임 경계가 유지된다
- [x] 관측성 — startup smoke gate 와 future tracing events 가 단계적으로 정리되었다
- [x] hot path — same-turn 보장이 아직 hook 미landing 이라는 점을 분명히 적었다
- [x] 문서화 — `gadgetron-cli` orchestration 책임이 최신 trunk 에 맞게 추가되었다

**Action Items**:
- A1: phase header 와 section 6 을 "startup wiring landed / capture hooks follow-up" 구조로 다시 정렬했다.
- A2: `gadgetron-cli` 를 module-connection / dependency owner 로 명시해 trunk reality 와 맞췄다.
- A3: synthetic fallback path 를 "boot wiring absent" 가 아니라 "service unwired direct path" 로 좁혀 설명했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. 최종 승인 가능.

### 최종 승인 (PSL-1c Startup Wiring Conformance) — 2026-04-18 — PM
**결론**: Approved. 이 문서는 이제 KC-1/KC-1b candidate lifecycle 과 PSL-1c production startup wiring 현실을 함께 설명하는 trunk-authoritative 설계 문서다.

기존 승인 로그는 append-only 로 유지하고, 이번 revision 은 `W3-KC-1b` landed state 와 recent `origin/main` authority 를 맞추기 위한 conformance amendment 로 별도 재검토했다. 본 문서는 이제 candidate lifecycle 의 current trunk behavior, optional canonical write path, 그리고 KC-1c follow-up boundary 를 서로 충돌 없이 설명한다.

### Round 1 (PSL-1d Chat Capture Conformance) — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [x] 인터페이스 계약 — `CapturedActivityEvent` field mapping 과 empty-hint semantics 가 `handlers.rs` landed code 와 일치한다
- [x] 크레이트 경계 — live owner hook 은 `gadgetron-gateway`, contract/traits 는 `gadgetron-core`, store/coordinator 는 `gadgetron-knowledge` 에 남는다
- [x] 타입 중복 — chat capture payload shape 가 새 타입을 만들지 않고 기존 candidate contract 를 재사용한다
- [x] 에러 반환 — capture failure 가 warn log 로 degrade 되고 HTTP chat contract 를 바꾸지 않음이 명시되었다
- [x] 동시성 — fire-and-forget `tokio::spawn` 과 append-only mutex store 경계가 분리 서술되었다
- [x] 의존성 방향 — gateway 는 coordinator trait object 만 호출하고 역방향 결합을 만들지 않는다
- [x] Phase 태그 — landed chat hook 과 follow-up owner hooks / hint generation / projection hardening 이 분리되었다
- [x] 레거시 결정 준수 — D-12 crate boundary 와 충돌하지 않는다

**Action Items**:
- A1: PSL-1d landed owner hook, remaining non-chat owner gaps, non-PII payload rule 을 본문 phase/flow/test 섹션에 반영했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 1.5 진행 가능.

### Round 1.5 (PSL-1d Chat Capture Conformance) — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1.5` 기준)
- [x] 위협 모델 (필수) — chat capture payload 가 non-PII 요약만 남기고 raw prompt/reasoning 을 저장하지 않음을 고정했다
- [x] 신뢰 경계 입력 검증 — HTTP request 에서 넘어온 chat content 를 event title/summary/facts 에 재사용하지 않는 경계를 명시했다
- [x] 인증·인가 — `actor_user_id = nil` placeholder 와 caller inheritance 후속 복원을 open follow-up 으로 유지했다
- [x] 시크릿 관리 — model name / token count 외 시크릿 surface 가 추가되지 않는다
- [x] 공급망 — 신규 dependency 요구 없음
- [x] 암호화 — capture hook 추가가 기존 in-transit / at-rest 정책을 바꾸지 않는다
- [x] 감사 로그 — `audit_event_id = None` 현상과 KC-1c follow-up 을 문서에 드러냈다
- [x] 에러 정보 누출 — `penny_shared_context.capture_chat_failed` 는 warn 신호만 남기고 내부 구조를 사용자 응답에 노출하지 않는다
- [x] LLM 특이 위협 — chat traffic 로부터 prompt text 를 candidate plane 으로 복제하지 않는다는 방어선을 명시했다
- [x] 컴플라이언스 매핑 — CC6.6 / GDPR Art.32 / HIPAA 164.312(b) 서술이 chat capture path 기준으로도 유지된다
- [x] 사용자 touchpoint 워크스루 — recent_activity 는 채워질 수 있지만 pending_candidates 는 여전히 비어 있을 수 있다는 운영 의미가 설명되었다
- [x] 에러 메시지 3요소 — capture 실패가 request 실패가 아니라 observability issue 임을 operator 관점으로 이해 가능하다
- [x] CLI flag — 새 CLI flag surface 없음
- [x] API 응답 shape — chat capture hook 이 OpenAI-compatible 응답 body/SSE shape 를 바꾸지 않는다
- [x] config 필드 — 기존 `[knowledge.curation]` gate 의미만 확장 없이 재확인했다
- [x] defaults 안전성 — chat capture 는 hints 없이 시작해 과도한 자동 기록을 막는 기본값을 유지한다
- [x] 문서 5분 경로 — coordinator wired 상태에서 recent activity 가 생기는 operational expectation 을 문서에 추가했다
- [x] runbook playbook — "activity는 보이지만 candidate는 비어 있음" 이 정상일 수 있다는 구분이 가능하다
- [x] 하위 호환 — 기존 chat endpoint 성공/실패 semantics 가 유지된다
- [x] i18n 준비 — 사용자-facing 문자열이 payload contract 와 분리 가능하다

**Action Items**:
- A1: non-PII payload, placeholder actor/audit correlation, operator-visible recent_activity vs pending_candidates 의미를 본문에 반영했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 2 진행 가능.

### Round 2 (PSL-1d Chat Capture Conformance) — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §2` 기준)
- [x] 단위 테스트 범위 — PSL-1d non-streaming capture integration gate 가 landed test 목록에 추가되었다
- [x] mock 가능성 — fake provider + real router + in-memory coordinator harness 가 구체적으로 적혔다
- [x] 결정론 — fire-and-forget settle wait 를 bounded integration harness 로 설명했다
- [x] 통합 시나리오 — "successful chat -> one activity event" 와 "next-turn bootstrap visibility" 를 landed/follow-up 로 분리했다
- [x] CI 재현성 — current test 는 외부 DB 없이 재현 가능하고 heavier HTTP/bootstrap chain 은 후속으로 남긴다
- [x] 성능 검증 — capture hook verification 이 hot path SLO 와 별개로 작동함을 명시했다
- [x] 회귀 테스트 — successful non-streaming chat 이 activity event 를 남기지 않는 회귀를 명확히 막는다
- [x] 테스트 데이터 — fake token counts 와 store snapshot assertion 전략이 드러난다

**Action Items**:
- A1: PSL-1d integration test, fake-provider harness, streaming final-usage follow-up test gap 을 단위/통합 섹션에 반영했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 3 진행 가능.

### Round 3 (PSL-1d Chat Capture Conformance) — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §3` 기준)
- [x] Rust 관용구 — helper 가 기존 `CapturedActivityEvent` / coordinator trait surface 를 그대로 사용한다
- [x] 제로 비용 추상화 — 새로운 public abstraction 없이 existing trait-object seam 에 owner hook 만 추가된다고 문서화했다
- [x] 제네릭 vs 트레이트 객체 — gateway 가 `Arc<dyn KnowledgeCandidateCoordinator>` 만 의존함을 명시했다
- [x] 에러 전파 — capture failure graceful degrade 와 materialization failure semantics 를 혼동하지 않도록 분리했다
- [x] 수명주기 — request-local spawn 과 append-only store lifetime 경계가 분명하다
- [x] 의존성 추가 — 신규 crate/dep 요구 없음
- [x] 트레이트 설계 — `hints = []` 로도 현재 contract 가 유효하다는 점을 공용 API 기준으로 고정했다
- [x] 관측성 — warn event 이름과 landed verification gate 가 current trunk 와 정렬된다
- [x] hot path — streaming `0/0` placeholder 를 현재 truth 로 적고 future fix 로 분리했다
- [x] 문서화 — PSL-1d landed reality 가 candidate lifecycle 문서에 합쳐져 authority drift 를 제거했다

**Action Items**:
- A1: first live owner hook 의 exact field semantics, streaming placeholder rule, future actor/audit restoration 을 architecture/phase/test sections 에 반영했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. 최종 승인 가능.

### 최종 승인 (PSL-1d Chat Capture Conformance) — 2026-04-18 — PM
**결론**: Approved. 이 문서는 이제 KC-1/KC-1b candidate lifecycle, PSL-1c startup wiring, PSL-1d first live chat capture hook 을 함께 설명하는 current-trunk authority 문서다.
