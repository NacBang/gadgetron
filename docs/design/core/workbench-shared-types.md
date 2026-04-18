# Workbench Shared Wire Types

> **담당**: @chief-architect
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-core`, `gadgetron-gateway`, `gadgetron-web`, `gadgetron-penny`, `gadgetron-knowledge`
> **Phase**: [P2B] primary / [P2C] typed evidence enrichment / [P3] broader descriptor consolidation
> **관련 문서**: `docs/design/gateway/workbench-projection-and-actions.md`, `docs/design/web/expert-knowledge-workbench.md`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/design/phase2/14-penny-retrieval-citation-contract.md`, `docs/design/core/knowledge-plug-architecture.md`, `docs/process/04-decision-log.md` D-20260418-14, `docs/reviews/pm-decisions.md` D-12/D-13
> **보정 범위**: 이 문서는 2026-04-18 `origin/main` 에 이미 landed 된 `W3-WEB-2` read-only slice (`bootstrap`, `activity`, `request_evidence`) 의 shared DTO 계약만 다룬다. WEB-2b descriptor/action catalog 와 KC-1 candidate narrowing 은 범위 밖이다.

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백

2026-04-18 기준 `origin/main` 은 `745c6fb` (`W3-WEB-2`) 에서 새 `crates/gadgetron-core/src/workbench/mod.rs` 를 landed 했다. 하지만 현재 문서 구조에는 다음 공백이 남아 있다.

1. `gadgetron-core::workbench` 는 실제 shared wire type 인데, authoritative 설명이 gateway/web 문서 안에 분산되어 있다.
2. `docs/design/web/expert-knowledge-workbench.md` 는 더 넓은 제품 surface 를, `docs/design/gateway/workbench-projection-and-actions.md` 는 route/service orchestration 을 다룬다. 둘 다 `gadgetron-core` 자체의 소유 경계와 additive evolution 규칙을 중심으로 쓰이지 않았다.
3. code comment 도 아직 gateway 문서를 authority 로 가리켜, core DTO 와 gateway-local error/service 경계가 흐려질 여지가 있다.
4. 다음 순서로 예정된 `PSL-1`, `W3-WEB-2b`, `KC-1` 이 같은 타입 집합을 재사용할 텐데, core 쪽 SSOT 가 없으면 `WorkbenchActivity*`, `ToolTraceSummary`, `CitationSummary`, `candidates` field 가 각 문서에서 다른 의미로 drift 할 수 있다.

즉, 최근 mainline 변경이 드러낸 가장 큰 문서 갭은 "route 는 gateway 가, shell 은 web 이, awareness 는 Penny 가 소유하지만, **그 사이를 오가는 shared DTO 는 누가 어떤 규칙으로 소유하는가**" 이다. 이 문서는 그 seam 을 닫는다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1` 과 `docs/design/web/expert-knowledge-workbench.md` 가 정의한 Gadgetron 의 P2B surface 는 "chat transcript 하나만 보는 앱" 이 아니다. operator 는 같은 사실을 세 surface 에서 일관되게 봐야 한다.

- Web shell 은 `bootstrap/activity/evidence` 를 읽는다.
- gateway 는 그 read model 을 조립하고 OpenAI-compatible error envelope 로 서빙한다.
- Penny 는 같은 사실을 shared surface 로 읽고 다음 turn awareness 에 반영한다.

이 구조에서 `gadgetron-core::workbench` 의 책임은 policy 를 소유하는 것이 아니라, **cross-crate wire shape 를 안정화하는 것** 이다. 즉:

> core 는 DTO 를 소유하고, gateway 는 policy 와 projection 을 소유하며, web/Penny 는 같은 DTO 를 소비한다.

이는 D-12 의 크레이트 경계 원칙과도 맞는다. 여러 크레이트가 공유하는 serializable public type 은 leaf crate 의 route-local 구현이 아니라 `gadgetron-core` 에 둬야 한다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 설명 | 채택하지 않은 이유 |
|---|---|---|
| A. gateway 문서를 계속 shared DTO authority 로 둔다 | 문서 수가 늘지 않는다 | gateway-local `WorkbenchHttpError`, service trait, scope 예외까지 함께 읽어야 해서 core 소유 경계가 흐려진다 |
| B. web 문서를 authority 로 둔다 | shell consumer 관점에서 읽기 쉽다 | UI 요구와 core wire contract 가 섞여 field drift 를 막기 어렵다 |
| C. DTO 도 gateway-local 로 두고 web/Penny 가 JSON schema 로만 소비한다 | crate 수는 적어 보인다 | Rust type SSOT 가 사라지고 D-12 shared type 배치 원칙과 충돌한다 |
| D. `gadgetron-core::workbench` 의 landed read-only slice 를 독립 core 문서로 고정하고, orchestration 은 기존 gateway 문서에 남긴다 | core 소유 범위와 future additive evolution 규칙을 명확히 분리할 수 있다 | 문서가 하나 늘지만 drift 방지 비용이 더 낮다 |

채택: **D. core-owned shared wire types + gateway-owned projection/policy**

### 1.4 핵심 설계 원칙과 trade-off

1. **Core owns wire shapes, not route policy**
   `WorkbenchBootstrapResponse`, `WorkbenchActivity*`, `WorkbenchRequestEvidenceResponse` 같은 DTO 는 core 에 둔다. auth, scope, 404 mapping, rate-limit, actor filtering 은 gateway 가 소유한다.
2. **Shared DTO must stay additive**
   `origin/main` 에 landed 한 field name 과 enum casing 은 고정한다. 새 variant 는 `#[non_exhaustive]` enum 확장으로, 새 struct field 는 optional/defaultable tail 추가로만 진화시킨다.
3. **Read model arrives post-filtered**
   core DTO 는 raw event row 나 secret-bearing payload 를 실어 나르지 않는다. actor filtering, approval gating, tenant visibility 판단은 gateway/knowledge/xaas 경계에서 끝난 뒤 DTO 로 직렬화된다.
4. **Evidence summaries are redacted by construction**
   tool trace 는 raw args 대신 digest 를, citation 은 page identity 와 optional anchor 만 담는다. request evidence DTO 는 detail dump 가 아니라 operator-facing projection 이다.
5. **Landed slice first, broader catalog later**
   본 문서는 현재 main 에 있는 `bootstrap/activity/evidence` 만 다룬다. descriptor/action DTO 는 WEB-2b authority 가 별도로 닫히기 전까지 이 문서에 몰아넣지 않는다.
6. **No fake stability claims**
   decision log 의 intent 와 현재 landed code 가 다를 수 있는 부분은 코드 기준으로 적고, future narrowing 은 open issue 로 남긴다. 예를 들어 `candidates: Vec<Value>` 는 아직 typed contract 가 아니다.

Trade-off:

- core 문서가 하나 더 생긴다.
- 대신 gateway/web/Penny 문서가 각자 DTO 를 복사해 설명하다가 drift 하는 위험을 줄인다.
- `candidates` 처럼 아직 거친 field 는 "임시지만 현재 authoritative" 로 고정되므로, future PR 이 breaking narrowing 을 하려면 명시적 설계 갱신이 필요해진다.

### 1.5 Operator Touchpoint Walkthrough

1. 사용자가 `/web` 을 열면 shell 은 `GET /api/v1/web/workbench/bootstrap` 으로 workbench readiness 를 읽는다.
2. gateway 는 `KnowledgeService::plug_health_snapshot()` 과 gateway version 을 조합해 `WorkbenchBootstrapResponse` 를 만든다.
3. shell 은 최근 activity feed 를 `WorkbenchActivityResponse` 로 읽는다. P2B current slice 에서는 비어 있을 수 있지만 wire shape 는 이미 고정된다.
4. 사용자가 특정 request evidence 를 열면 shell 또는 Penny-aware flow 가 `WorkbenchRequestEvidenceResponse` 를 읽는다.
5. 존재하지 않거나 보이지 않는 request 는 gateway-local 404 로 변환되지만, 성공 시 payload shape 는 언제나 core DTO 계약을 따른다.
6. PSL-1 이 landed 하면 Penny trace/tool trace/citation 이 같은 `WorkbenchRequestEvidenceResponse` 를 채운다. consumer 는 route owner 가 아니라 core wire contract 를 기준으로 렌더링한다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

`gadgetron-core::workbench` 의 현재 public surface 는 read-only DTO 3군으로 구성된다.

#### 2.1.1 Bootstrap DTO

```rust
// crates/gadgetron-core/src/workbench/mod.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchBootstrapResponse {
    pub gateway_version: String,
    pub default_model: Option<String>,
    pub active_plugs: Vec<PlugHealth>,
    pub degraded_reasons: Vec<String>,
    pub knowledge: WorkbenchKnowledgeSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlugHealth {
    pub id: String,
    pub role: String,
    pub healthy: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchKnowledgeSummary {
    pub canonical_ready: bool,
    pub search_ready: bool,
    pub relation_ready: bool,
    pub last_ingest_at: Option<DateTime<Utc>>,
}
```

계약:

- `gateway_version` 은 `gadgetron-gateway` crate version string 이다. build fingerprint 나 git SHA 를 core DTO 에 직접 넣지 않는다.
- `default_model` 은 현재 `Option<String>` 이다. model routing 이 아직 결정되지 않았거나 headless/test build 이면 `None` 이 허용된다.
- `active_plugs` 는 read model snapshot 이다. current P2B 에서는 `canonical` / `search` / `relation` role 을 우선 사용한다. `extractor` 같은 추가 role 은 [P2C] 이후 additive introduction 만 허용한다.
- `degraded_reasons` 는 operator-facing short string 이다. stack trace, raw DSN, secret, 내부 파일 경로는 금지한다.
- `knowledge.last_ingest_at` 은 canonical write 성공 시각 또는 derived ingest watermark 를 요약해서 싣는 자리다. current landed implementation 은 `None` 일 수 있다.

#### 2.1.2 Activity DTO

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActivityOrigin {
    Penny,
    UserDirect,
    System,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActivityKind {
    ChatTurn,
    DirectAction,
    SystemEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActivityEntry {
    pub event_id: Uuid,
    pub at: DateTime<Utc>,
    pub origin: WorkbenchActivityOrigin,
    pub kind: WorkbenchActivityKind,
    pub title: String,
    pub request_id: Option<Uuid>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActivityResponse {
    pub entries: Vec<WorkbenchActivityEntry>,
    pub is_truncated: bool,
}
```

계약:

- `origin` 과 `kind` 는 `snake_case` string 으로 직렬화된다. consumer 는 exhaustive match 를 가정하면 안 된다.
- `event_id` 는 activity row identity 이다. `request_id` 는 optional correlation pointer 이며 모든 activity 가 request 에 속한다고 가정하면 안 된다.
- `title` 은 left-rail/feed label 이고, `summary` 는 optional teaser 다. P2B current slice 에서는 summary 없이 title 만 노출될 수 있다.
- `is_truncated` 는 gateway query clamp 와 별개로 "더 뒤에 항목이 남아 있다" 는 projection signal 이다.

#### 2.1.3 Request evidence DTO

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRequestEvidenceResponse {
    pub request_id: Uuid,
    pub tool_traces: Vec<ToolTraceSummary>,
    pub citations: Vec<CitationSummary>,
    pub candidates: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTraceSummary {
    pub gadget_name: String,
    pub args_digest: String,
    pub outcome: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationSummary {
    pub label: String,
    pub page_name: String,
    pub anchor: Option<String>,
}
```

계약:

- `request_id` 는 evidence projection 의 canonical lookup key 다.
- `ToolTraceSummary.args_digest` 는 raw args 전체가 아니라 SHA-256 hex prefix digest 이다. request evidence 는 debugging summary 이지 secret-bearing replay payload 가 아니다.
- `ToolTraceSummary.outcome` 은 current P2B 에서 `"success"`, `"denied"`, `"error"` 를 우선 사용한다. enum 화는 future additive refactor 대상이지만 current landed wire 는 string 이다.
- `CitationSummary.label` 은 parser/emitter 가 사용하는 markdown footnote label string 이다. consumer 가 숫자 normalize 나 caret strip 을 강제하면 안 된다.
- `CitationSummary.anchor` 는 optional display/deep-link hint 이다. 없다고 해서 page identity 가 invalid 인 것은 아니다.
- `candidates` 는 아직 `Vec<serde_json::Value>` 이다. current main 기준으로 typed summary 가 아니다. KC-1 이 이 field 를 다룰 때는 별도 설계 갱신이 필요하다.

### 2.2 내부 구조

#### 2.2.1 모듈 배치

```text
crates/gadgetron-core/
├── src/lib.rs
└── src/workbench/mod.rs
```

배치 원칙:

- `lib.rs` 는 `pub mod workbench;` 로 공개한다.
- DTO 는 core 에 두되, route handler / projection trait / gateway-local error type 은 core 로 올리지 않는다.
- 이 분리는 D-12 의 "shared type 은 core, route-local behavior 는 leaf crate" 원칙을 workbench surface 에 적용한 것이다.

#### 2.2.2 Projection assembly 경계

현재 landed read-only slice 에서 실제 조립 흐름은 다음과 같다.

```text
KnowledgeService::plug_health_snapshot()
             +
gateway version / local degraded rule
             |
             v
InProcessWorkbenchProjection
             |
             v
WorkbenchBootstrapResponse / WorkbenchActivityResponse / WorkbenchRequestEvidenceResponse
```

설계 규칙:

- `gadgetron-core::workbench` 는 projection source 를 모른다.
- `gadgetron-gateway::web::projection::InProcessWorkbenchProjection` 가 `KnowledgeService` 를 읽고 DTO 로 변환한다.
- activity/evidence source 가 비어 있는 current P2B 상태에서도 DTO shape 는 stable 해야 한다.
- Penny trace, candidate projection, approval state, richer citation anchor 는 DTO 를 재사용해 later wire 된다.

#### 2.2.3 Forward-compatibility 규칙

current landed code 를 기준으로 다음 규칙을 고정한다.

1. `WorkbenchActivityOrigin` / `WorkbenchActivityKind` 는 `#[non_exhaustive]` 유지
2. 기존 field rename 금지
3. 새 struct field 추가 시:
   - `Option<T>` 또는 empty-collection defaultable tail field 로 추가
   - gateway/web/Penny 소비자 문서 동시 갱신
   - review log 에 compatibility note 기록
4. stringly-typed field (`role`, `outcome`) 를 enum 으로 바꾸고 싶다면 in-place breaking rename 이 아니라 additive compatibility path 가 필요
5. `candidates` typed narrowing 은 별도 문서/리뷰 없이는 금지

#### 2.2.4 Redaction and minimality

이 DTO 집합은 operator-facing summary contract 다. 따라서 raw detail dump 를 실어 나르지 않는다.

| 영역 | 허용 | 금지 |
|---|---|---|
| tool trace | gadget 이름, args digest, 결과, latency | raw args, raw prompt, bearer token, provider secret |
| citation | page identity, optional anchor | hidden ACL bypass URL, blob secret, raw excerpt 전문 |
| activity | title, optional summary, request correlation | 내부 struct path, stack trace, tenant-foreign event |
| degraded reason | human-readable short reason | DB DSN, filesystem path, full upstream error blob |

#### 2.2.5 Current P2B emptiness is still contractual

current `origin/main` 의 `InProcessWorkbenchProjection` 는 다음 동작을 갖는다.

- `bootstrap`: knowledge service 가 없으면 degraded 응답을 돌린다
- `activity`: 빈 `entries`, `is_truncated = false`
- `request_evidence`: 존재하지 않는 request 로 404

이것이 "미완성이라 문서 불필요" 를 뜻하지는 않는다. 오히려 empty-but-typed behavior 가 지금 shell 과 PSL-1 downstream 기대치를 결정하므로, current landed semantics 를 SSOT 로 남겨야 한다.

### 2.3 설정 스키마

N/A + 이유:

- `gadgetron-core::workbench` 는 pure wire type 모듈이다.
- 새 `[workbench]` 또는 `[core.workbench]` TOML 섹션을 만들지 않는다.
- bootstrap 에 들어가는 값은 기존 gateway/knowledge 설정과 runtime state 에서 조립된다.
- 따라서 config ownership 은 계속 `gadgetron-gateway` 와 `gadgetron-knowledge` 에 남는다.

### 2.4 에러 & 로깅

#### 2.4.1 에러 경계

이 문서는 새 `GadgetronError` variant 를 추가하지 않는다.

- D-13 taxonomy 는 그대로 유지한다.
- workbench read-only slice 의 HTTP 변환은 gateway-local `WorkbenchHttpError` 가 맡는다.
- `RequestNotFound { request_id }` 는 gateway-local 404 이며 core 공용 에러로 승격하지 않는다.

즉, 경계는 다음과 같다.

```text
shared DTO / enum / serde contract        -> gadgetron-core
HTTP status mapping / OpenAI error shape  -> gadgetron-gateway
projection source lookup failure          -> GadgetronError or gateway-local 404
```

#### 2.4.2 tracing / observability

core DTO 자체는 tracing span 을 발행하지 않는다. 운영 상 필요한 span/event naming 은 gateway 쪽에서 유지한다.

- `web.workbench.bootstrap`
- `web.workbench.activity`
- `web.workbench.request_evidence`

필드 원칙:

- `request_id`, `event_id`, `tenant_id?`, `limit`, `outcome`, `latency_ms`
- secret, raw args, raw prompt, full citation excerpt 는 금지

#### 2.4.3 STRIDE threat model 요약

| 자산 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| `WorkbenchBootstrapResponse` | gateway -> browser / Penny | Spoofing, Tampering | Bearer auth, TLS, typed DTO serialization, same-origin fetch |
| activity feed entry | capture plane -> gateway projection -> consumer | Information Disclosure, Repudiation | actor-filtered projection, append-only upstream event source, explicit `event_id` |
| request evidence | Penny trace / knowledge citation source -> gateway -> browser | Info disclosure, Tampering | args digest only, actor visibility check before DTO assembly, no raw secret fields |
| degraded reason string | infra/runtime -> gateway -> operator | Info disclosure | short remediation text only, no DSN/path/stack trace |
| future `candidates` payload | KC-1 source -> gateway -> browser/Penny | EoP, schema drift | current scope keeps it untyped and explicit, future narrowing requires separate reviewed doc |

컴플라이언스 매핑:

- SOC2 CC6.x: actor-filtered projection + access control
- GDPR Art 32: 최소 데이터 노출, 전송 구간 보호, secret redaction
- HIPAA §164.312 relevant only when workbench evidence carries protected content; current P2B summary contract 는 raw payload 전문을 금지해 blast radius 를 줄인다

### 2.5 의존성

current landed module이 사용하는 core dependency 는 다음뿐이다.

| crate | 버전 계열 | 이유 |
|---|---|---|
| `serde` | workspace 기존 | JSON wire serialization |
| `chrono` | workspace 기존 | `DateTime<Utc>` timestamp field |
| `uuid` | workspace 기존 | request/event identity |
| `serde_json` | workspace 기존 | `candidates: Vec<Value>` temporary carrier |

원칙:

- 새 transport crate, web framework crate, jsonschema crate 를 core 에 추가하지 않는다.
- route-local concern (`axum`, `tower`, OpenAI error envelope) 은 gateway 에 남긴다.

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 상하위 연결

```text
gadgetron-web / gadgetron-penny
          |
          v
gadgetron-gateway
  ├─ web::workbench routes
  ├─ projection service
  └─ WorkbenchHttpError
          |
          v
gadgetron-core::workbench
  ├─ WorkbenchBootstrapResponse
  ├─ WorkbenchActivity*
  └─ WorkbenchRequestEvidenceResponse
          |
          +--------------------+
          |                    |
          v                    v
gadgetron-knowledge      future PSL-1 / KC-1 sources
plug health snapshot     tool traces / citations / candidates
```

### 3.2 데이터 흐름 다이어그램

#### 3.2.1 Bootstrap

```text
Browser GET /api/v1/web/workbench/bootstrap
   |
   v
Gateway scope/auth
   |
   v
InProcessWorkbenchProjection::bootstrap()
   |
   +--> KnowledgeService::plug_health_snapshot()
   |
   v
WorkbenchBootstrapResponse JSON
```

#### 3.2.2 Activity and evidence

```text
Browser or Penny consumer
   |
   +--> GET /api/v1/web/workbench/activity
   |        -> WorkbenchActivityResponse
   |
   +--> GET /api/v1/web/workbench/requests/:id/evidence
            -> WorkbenchRequestEvidenceResponse
```

### 3.3 타 서브에이전트 도메인과의 인터페이스 계약

| 도메인 | 계약 |
|---|---|
| `gateway-router-lead` | route tree, query clamp, OpenAI error shape, gateway-local 404 는 gateway 소유. shared DTO rename 불가 |
| `ux-interface-lead` | shell renderer 는 `snake_case` enum casing, `Option` field, empty feed semantics 를 그대로 소비. 클라이언트가 enum exhaustiveness 를 가정하면 안 됨 |
| `inference-engine-lead` / `PM (Penny)` | future tool trace/citation producer 는 `ToolTraceSummary` / `CitationSummary` redaction 규칙을 지켜야 함 |
| `qa-test-architect` | core serde round-trip + gateway integration contract 를 분리해 검증 |
| `security-compliance-lead` | raw args/secret leakage 금지, actor-filtered projection 선행 강제 |
| `dx-product-lead` | degraded reason / title / summary 는 operator가 self-diagnose 가능한 짧은 문장이어야 함 |

### 3.4 D-12 크레이트 경계 준수 여부

준수 방식:

- shared serializable DTO 는 `gadgetron-core`
- route-local trait (`WorkbenchProjectionService`) 는 `gadgetron-gateway`
- HTTP error adapter (`WorkbenchHttpError`) 는 `gadgetron-gateway`
- plug health snapshot source 는 `gadgetron-knowledge`
- request evidence 구체 source 는 current P2B 에선 비어 있고, future PSL-1/KC-1 이 same DTO 로 채움

이 문서는 D-12 를 변경하지 않는다. 다만 current main 에 이미 landed 한 P2B shared DTO 를 D-12 원칙에 따라 해석하는 authoritative 문서 역할을 한다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

테스트 대상과 invariant:

| 대상 | 검증할 invariant |
|---|---|
| `WorkbenchBootstrapResponse` | `gateway_version`, `active_plugs`, `degraded_reasons`, `knowledge` field 가 round-trip 에서 보존된다 |
| `PlugHealth.role` | current role string (`canonical/search/relation`) 이 그대로 직렬화된다 |
| `WorkbenchActivityOrigin` / `WorkbenchActivityKind` | `snake_case` serialization 이 유지된다 |
| `WorkbenchActivityResponse.is_truncated` | 빈/비지 않은 feed 모두에서 boolean 의미가 보존된다 |
| `ToolTraceSummary` | raw args 대신 digest 만 노출된 shape 를 snapshot 으로 고정한다 |
| `CitationSummary` | `anchor = None` 이 허용되며 `page_name` 이 canonical identity 로 유지된다 |
| `WorkbenchRequestEvidenceResponse.candidates` | empty list 와 arbitrary JSON object list 둘 다 직렬화/역직렬화 가능하다 |

권장 테스트 파일:

- `crates/gadgetron-core/tests/workbench_wire.rs`

### 4.2 테스트 하네스

- `serde_json` round-trip test
- `insta` snapshot 을 사용할 수 있으면 enum casing / field naming 고정
- mock/stub 불필요. core DTO 는 pure serialization surface 이므로 외부 의존성 없이 deterministic 해야 한다
- property-based test 는 현재 범위에 필수는 아니지만, future KC-1 typed narrowing 시 arbitrary JSON backward-compatibility 검증용으로 고려 가능

예시:

```rust
#[test]
fn activity_enums_serialize_snake_case() {
    let value = serde_json::to_value(WorkbenchActivityOrigin::UserDirect).unwrap();
    assert_eq!(value, serde_json::json!("user_direct"));
}

#[test]
fn request_evidence_accepts_empty_and_untyped_candidates() {
    let payload = WorkbenchRequestEvidenceResponse {
        request_id: uuid::Uuid::nil(),
        tool_traces: vec![],
        citations: vec![],
        candidates: vec![serde_json::json!({"kind": "pending"})],
    };
    let value = serde_json::to_value(&payload).unwrap();
    assert!(value["candidates"].is_array());
}
```

### 4.3 커버리지 목표

- `crates/gadgetron-core/tests/workbench_wire.rs`: line coverage 90%+, branch coverage 85%+
- enum casing / optional field / empty collection path 는 100% 검증
- backward-compatibility sensitive field (`candidates`, `anchor`, `summary`, `default_model`) 은 snapshot 또는 explicit assertion 으로 고정

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

함께 테스트할 크레이트:

- `gadgetron-gateway`
- `gadgetron-core`
- `gadgetron-knowledge`
- `gadgetron-testing` (state/harness helper)
- [P2C] 이후 `gadgetron-penny`

핵심 시나리오:

1. **OpenAiCompat key -> bootstrap 200**
   - gateway 가 `WorkbenchBootstrapResponse` 를 실제 router 경로로 서빙
2. **scope regression guard**
   - 같은 key 가 `/api/v1/nodes` 는 403 이고 `/api/v1/web/workbench/bootstrap` 은 200
3. **activity clamp + empty feed**
   - `limit=0`/`999` 가 clamp 된 뒤 empty but valid `WorkbenchActivityResponse` 를 반환
4. **request evidence 404 shape**
   - 미존재 `request_id` 는 gateway-local 404 이지만 성공 payload shape 와 core DTO 는 unaffected
5. **future PSL-1 evidence fill**
   - tool trace/citation source 가 wire 된 뒤 `ToolTraceSummary` / `CitationSummary` redaction invariant 유지

### 5.2 테스트 환경

- current P2B 는 in-process router + mock key validator + optional `KnowledgeService`
- Postgres 실 DB 는 필수 아님. existing integration tests 처럼 lazy pool 로도 재현 가능
- knowledge service 가 없는 degraded build 와 있는 healthy build 를 둘 다 다룬다
- PSL-1 이후 evidence source 는 fake trace store 또는 fixture request trace 로 deterministic 하게 주입

### 5.3 회귀 방지

다음 변경은 테스트를 실패시켜야 한다.

- enum casing drift (`user_direct` -> `userDirect`)
- field rename (`active_plugs` / `degraded_reasons` / `is_truncated`)
- raw args leakage (`args_digest` 대신 full args)
- `candidates` 를 무단 typed narrowing 또는 field 삭제
- gateway-local 404 를 shared core error 로 승격
- `OpenAiCompat` scope exception regression

---

## 6. Phase 구분

- [P2B] 현재 landed 범위
  - `WorkbenchBootstrapResponse`
  - `WorkbenchActivityOrigin`, `WorkbenchActivityKind`, `WorkbenchActivityEntry`, `WorkbenchActivityResponse`
  - `WorkbenchRequestEvidenceResponse`, `ToolTraceSummary`, `CitationSummary`
  - empty-but-typed `activity` / `request_evidence` current behavior

- [P2C] additive follow-up
  - richer `active_plugs.role`
  - populated `default_model`, `last_ingest_at`
  - PSL-1 tool trace/citation fill
  - KC-1 candidate summary narrowing or companion typed field

- [P3] broader consolidation candidate
  - descriptor/action DTO 의 core 승격 여부 재검토
  - richer typed outcome enums or evidence provenance sub-objects

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | `WorkbenchRequestEvidenceResponse.candidates` 를 KC-1 에서 어떻게 typed contract 로 좁힐 것인가 | A. `Vec<Value>` 유지 / B. 새 typed companion field 추가 / C. in-place type 교체 | B | 🟡 KC-1 설계 follow-up |
| Q-2 | WEB-2b descriptor/action DTO 를 future 에 core 로 올릴 것인가 | A. gateway-local 유지 / B. shared DTO 만 core 승격 / C. 전체 catalog/service contract 승격 | B | 🟡 WEB-2b 설계 follow-up |

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. `745c6fb` 에서 landed 한 `gadgetron-core::workbench` read-only slice 를 core SSOT 로 분리.

**체크리스트**:
- [x] 최근 `origin/main` 변화 반영
- [x] current landed code 확인
- [x] D-12/D-13 경계 확인
- [ ] cross-review annotations

### Round 1 — 2026-04-18 — @gateway-router-lead @ux-interface-lead
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
- A1: core DTO 와 gateway-local `WorkbenchHttpError` / `WorkbenchProjectionService` 경계를 본문에서 분리해 명시
- A2: current landed slice 가 WEB-2b descriptor/action catalog 범위를 포함하지 않음을 헤더와 §1/§6 에 반복 명시

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
- [ ] CLI flag — N/A: CLI surface 없음
- [x] API 응답 shape
- [x] config 필드
- [x] defaults 안전성
- [x] 문서 5분 경로
- [x] runbook playbook
- [x] 하위 호환
- [x] i18n 준비

**Action Items**:
- A1: tool trace / citation / degraded reason redaction 규칙을 표로 고정
- A2: `candidates` 가 아직 typed contract 가 아님을 open issue 로 올려 future silent breakage 를 방지

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
- A1: core serde round-trip test 와 gateway integration test 의 역할을 분리해 테스트 계획에 명시
- A2: `candidates` backward-compatibility 와 scope regression 을 회귀 대상에 명시

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
- A1: current landed code 기준으로 `stringly-typed role/outcome` 를 과장 없이 문서화하고, enum 승격은 additive future work 로 분리
- A2: D-12 를 변경한다고 주장하지 말고, "D-12 원칙의 현재 적용" 으로 표현

**다음 라운드 조건**: 없음

### 최종 승인 — 2026-04-18 — PM
Round 1 / 1.5 / 2 / 3 action item 반영 완료. 이 문서는 `gadgetron-core::workbench` read-only shared DTO 의 authoritative reference 이며, gateway/web/Penny 설계 문서가 공통 wire shape 를 인용할 때 우선 참조한다.
