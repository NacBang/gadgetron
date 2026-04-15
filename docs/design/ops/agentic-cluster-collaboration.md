# Agentic Cluster Collaboration Platform

> **담당**: @chief-architect
> **상태**: Draft
> **작성일**: 2026-04-15
> **최종 업데이트**: 2026-04-15
> **관련 크레이트**: `gadgetron-core`, `gadgetron-gateway`, `gadgetron-kairos`, `gadgetron-knowledge`, `gadgetron-xaas`, `gadgetron-scheduler`, `gadgetron-node`
> **Phase**: [P2] / [P3]
> **결정 근거**: D-12 (crate boundary), D-20260414-04 (agent-centric control plane), ADR-P2A-05 (agent-centric control plane), ADR-P2A-06 (approval flow deferred to P2B)

---

## 1. 철학 & 컨셉 (Why)

### 1.1 문제 한 문장

Gadgetron은 더 이상 "GPU/LLM 게이트웨이 + 개인 비서"만으로는 설명되지 않으며, 관리자와 사용자와 에이전트가 함께 이종 클러스터를 운영하고 실행 결과를 주고받는 협업 플랫폼으로 재정의되어야 한다.

### 1.2 제품 정의

이 문서는 Gadgetron의 궁극 제품 정의를 다음 한 문장으로 고정한다.

> **Gadgetron은 관리자·사용자·에이전트가 함께 일하는 agentic heterogeneous cluster collaboration platform 이다.**

플랫폼은 세 개의 plane 으로 구성된다.

| Plane | 핵심 질문 | 대표 사용자 가치 | 현재 상태 |
|------|-----------|------------------|-----------|
| **Assistant Plane** | "무엇을 원하나?" | 일상 요청 처리, 대화형 질의응답, 요약, 보고, 위임 창구 | P2A 시작 |
| **Operations Plane** | "무슨 문제가 있고 무엇을 바꿔야 하나?" | 클러스터 모니터링, 장애 분석, 위험 경고, 설정 변경, 운영 리포트 | P1 기반 + P2C/P3 agentization 예정 |
| **Execution Plane** | "어디서 어떻게 실행해 결과를 돌려줄 것인가?" | 자원 최적화, 스케줄링, 워크로드 실행, 결과 회수 | P1 기반 + P3 확장 |

세 개의 actor 가 이 plane 들을 가로질러 협업한다.

| Actor | 역할 | 기본 권한 |
|------|------|-----------|
| **Administrator** | 인프라 소유자, 정책 승인자, 파괴적 변경 최종 승인 | 전체 cluster / scheduler / policy 관리 |
| **User** | 작업 요청자, 결과 소비자, 일부 self-service operator | 요청 생성, 제한된 실행 위임, 결과 검토 |
| **Agent** | 대화형 비서이자 운영 조력자, 실행 조정자, 경험 축적 주체 | 정책 범위 내 도구 사용, 승인 요청, 보고 생성 |

핵심 UX 원칙은 두 가지다.

1. **Direct + Delegate 공존**: 사람은 직접 작업할 수도 있고 에이전트에게 위임할 수도 있다.
2. **Experience Loop**: 에이전트는 사람의 요청, 승인, 트러블슈팅, 회고를 관찰하고 구조화하여 더 적합한 협업 파트너로 진화한다.

### 1.3 현재 문서와의 연결

- `docs/00-overview.md §1`의 "하방/상방 2층 구조"를 supersede 하지는 않지만, 이를 더 상위의 협업 프레임으로 재해석한다.
- 하방은 단순 "인프라"가 아니라 **Operations + Execution Plane 의 substrate** 다.
- 상방은 단순 "personal assistant"가 아니라 **Assistant Plane 의 초기 구현**이다.
- `docs/design/phase2/04-mcp-tool-registry.md` 의 `InfraToolProvider`, `SchedulerToolProvider`, `ClusterToolProvider` 는 본 문서의 plane 확장을 위한 기술적 seam 으로 해석한다.

### 1.4 채택하지 않은 대안

| 대안 | 설명 | 채택하지 않은 이유 |
|------|------|--------------------|
| **A. Gateway-first 제품 정의 유지** | Gadgetron을 계속 LLM gateway 로만 설명 | 현재 로드맵의 agentization, infra toolization, operator UX 를 설명하지 못한다 |
| **B. Personal assistant 제품 정의 강화** | Kairos 중심 소비자형 비서로 설명 | 클러스터 운영, 워크로드 실행, 관리자 승인 체계를 하위 기능으로 축소해 버린다 |
| **C. Full autonomous operator 우선** | 사람 개입 없이 자가 운영하는 시스템으로 설계 | 초기 단계에서 보안, 책임성, audit 요구를 만족시키기 어렵다. `shadow -> approved automation` 경로가 더 안전하다 |

### 1.5 설계 원칙과 trade-off

1. **Memory != Authority**: 에이전트가 관찰하고 기억하는 것과 자동 실행 권한은 분리한다.
2. **Shadow before Auto**: 사람의 작업을 먼저 관찰하고 요약한 뒤, 승인된 절차만 runbook/policy 로 승격한다.
3. **Tool-first extensibility**: 새로운 운영 능력은 ad-hoc 코드가 아니라 `McpToolProvider` 기반 category/tool 로 추가한다.
4. **Open-source leverage**: K8s, Slurm, Helm, CSI, existing schedulers 를 활용하되, Gadgetron은 협업·정책·기억·실행 조정 계층을 제공한다.
5. **Single binary bias 유지**: 가능하면 `gadgetron` 바이너리 하나로 운영한다. 다만 외부 제어 대상은 다중 클러스터/다중 API 일 수 있다.
6. **Human-legible audit**: 모든 진단, 제안, 승인, 실행, 실패, 회고는 사람이 재구성 가능한 audit trail 로 남아야 한다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

본 문서는 구현 전 canonical vision 을 잠그기 위한 것이므로, 아래 타입과 API 는 **추가적 public surface 의 목표 형태**를 정의한다. 기존 `/v1/chat/completions` OpenAI 호환 계약은 유지하고, 협업 기능은 additive surface 로 확장한다.

#### 2.1.1 공유 타입 (`gadgetron-core`)

```rust
pub enum CollaborationRole {
    Administrator,
    User,
    Agent,
}

pub enum CollaborationPlane {
    Assistant,
    Operations,
    Execution,
}

pub enum InteractionMode {
    Direct,
    Delegate,
    Shadow,
}

pub enum WorkIntent {
    AnswerQuestion,
    GenerateReport,
    InvestigateIncident,
    ManageCluster,
    ManageStorage,
    ManageVirtualization,
    OptimizeWorkload,
    ExecuteWorkload,
}

pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
}

pub struct CollaborationContext {
    pub request_id: uuid::Uuid,
    pub tenant_id: Option<uuid::Uuid>,
    pub actor_id: String,
    pub role: CollaborationRole,
    pub plane: CollaborationPlane,
    pub mode: InteractionMode,
    pub intent: WorkIntent,
}

pub struct ExperienceRecord {
    pub id: uuid::Uuid,
    pub source: ExperienceSource,
    pub summary: String,
    pub evidence_refs: Vec<String>,
    pub approval_state: ApprovalState,
    pub reusable_as_runbook: bool,
}

pub enum ExperienceSource {
    HumanAction,
    AgentAction,
    IncidentResolution,
    ScheduledReport,
    WorkloadExecution,
}
```

#### 2.1.2 운영/협업 HTTP surface (`gadgetron-gateway`)

```text
POST /v1/chat/completions
  - 기존 계약 유지
  - model = "kairos" 일 때 CollaborationContext 를 내부 생성

GET /api/v1/ops/incidents
GET /api/v1/ops/incidents/{id}
POST /api/v1/ops/incidents/{id}/actions

GET /api/v1/ops/reports/daily
GET /api/v1/ops/reports/weekly

GET /api/v1/runbooks
POST /api/v1/runbooks/promotions

GET /api/v1/approvals/pending
POST /api/v1/approvals/{id}
```

#### 2.1.3 도구 category 확장 (`McpToolProvider`)

기존 `McpToolProvider` trait 은 유지한다. 새로운 plane 은 category 확장으로 수용한다.

```rust
// Category examples
"knowledge"      // wiki, search, summary
"infra"          // nodes, gpu, routing, model deploy
"scheduler"      // job/query/cancel
"cluster"        // kubectl/helm/slurm
"storage"        // pvc/storageclass/volume diagnostics
"virtualization" // kubevirt/openstack/proxmox class (future)
"workload"       // submit/monitor/collect artifacts
```

Reserved namespace:

```rust
"agent" // permanently reserved; no self-brain or self-policy mutation
```

### 2.2 내부 구조

#### 2.2.1 네 개의 루프

플랫폼 내부 상태는 아래 네 개의 loop 로 설명한다.

1. **Assistant Loop**
   - 대화 요청 수신
   - intent 분류
   - 필요한 지식/운영/실행 도구 호출
   - 응답 생성

2. **Operations Loop**
   - metrics/logs/events/alerts 수집
   - 이상 징후 탐지
   - incident summary 생성
   - operator approval 후 조치 또는 인간에게 escalation

3. **Execution Loop**
   - workload 요구사항 해석
   - resource fit / cost / locality / policy 평가
   - 최적 target 에 배치
   - 상태 추적 후 결과 반환

4. **Experience Loop**
   - human action / agent action / incident resolution 관찰
   - evidence 를 구조화
   - runbook draft 생성
   - 사람 승인 후 재사용 가능한 절차로 승격

#### 2.2.2 메모리 계층 분리

에이전트의 "똑똑해짐"은 단일 memory 가 아니라 네 계층의 합으로 모델링한다.

| 계층 | 용도 | 권한 |
|------|------|------|
| **Conversational Memory** | 최근 대화 문맥, 사용자 선호, 말투, 약속 | 응답 품질 향상 |
| **Knowledge Memory** | wiki, 문서, 회의록, 운영 지식 | 질의응답/설명 |
| **Operational Memory** | incident, alert, remediation history, reports | 운영 추론/추천 |
| **Runbook Memory** | 사람이 승인한 표준 절차 | 제한적 자동화 재사용 |

**중요**: conversational/knowledge/operational memory 는 자동 실행 권한이 아니며, runbook memory 만 정책과 결합될 때 재사용 가능한 automation candidate 가 된다.

#### 2.2.3 권한 경계

- **T1 Read**: 자동 허용 가능
- **T2 Reversible Write**: 기본 `ask`
- **T3 Destructive**: 항상 `ask`, 자동 실행 금지
- **T4 Delegated Human-only**: 에이전트가 제안과 초안만 만들 수 있고 실제 실행은 인간만 가능

예시:

| 작업 | Tier |
|------|------|
| 로그 조회, 노드 상태 점검 | T1 |
| model deploy, routing 변경, drain/cordon | T2 |
| delete, wipe, force terminate, volume detach | T3 |
| root credential rotation, cluster bootstrap, irreversible infra migration | T4 |

#### 2.2.4 경험 축적 모델

경험 축적은 아래 단계를 강제한다.

1. **Observe** — 사람이 직접 수행한 조치와 결과를 수집
2. **Summarize** — 에이전트가 어떤 맥락에서 무엇이 효과 있었는지 요약
3. **Draft** — runbook candidate / playbook candidate 생성
4. **Approve** — 관리자 승인
5. **Reuse** — 이후 유사 사건에서 추천 또는 반자동 실행
6. **Automate** — 충분한 검증 후 정책적으로 허용된 범위 내 자동화

이 모델은 "서당개처럼 옆에서 보며 배운다"는 요구를 제품 안전성 안에 넣기 위한 구조다.

#### 2.2.5 확장 대상

본 비전이 최종적으로 수용해야 하는 대상은 다음과 같다.

- **Cluster management**: K8s, Slurm, bare-metal node registry
- **Storage management**: PVC, StorageClass, CSI-backed volume diagnostics
- **Virtualization management**: KubeVirt/OpenStack/기타 VM substrate
- **Workload management**: inference jobs, batch jobs, eval jobs, data jobs
- **Reporting**: 일일/주간 운영 리포트, incident digest, capacity analysis

### 2.3 설정 스키마

```toml
[collaboration]
enabled = true
default_plane = "assistant"
allow_direct_human_actions = true
allow_agent_delegation = true

[agent.learning]
observe_human_actions = true
capture_troubleshooting = true
capture_agent_failures = true
auto_promote_runbooks = false
max_pending_runbook_drafts = 128

[agent.reporting]
daily_ops_report = true
daily_ops_report_cron = "0 9 * * *"
weekly_capacity_report = true
incident_digest = true

[agent.operations]
auto_diagnose = true
auto_remediate_t2 = false
auto_remediate_t3 = false
require_human_summary_on_failure = true

[cluster]
targets = ["k8s", "slurm"]
default_target = "k8s"

[cluster.storage]
enabled = false
providers = ["csi"]

[cluster.virtualization]
enabled = false
providers = ["kubevirt"]
```

검증 규칙:

- `auto_remediate_t3 = true` 는 시작 실패
- `auto_promote_runbooks = true` 는 P3 전까지 시작 실패
- `observe_human_actions = true` 는 audit/event capture 가 켜져 있어야 한다
- `cluster.storage.enabled = true` 는 `cluster.targets` 에 `k8s` 가 포함되지 않으면 시작 실패
- `cluster.virtualization.enabled = true` 는 Phase 지원 전에는 명시적 startup error 로 드러낸다

### 2.4 에러 & 로깅

신규 에러 표면은 기존 `GadgetronError` 를 확장하는 방향으로 설계한다.

추천 신규 variant:

```rust
GadgetronError::Policy(String)
GadgetronError::Incident(String)
GadgetronError::Runbook(String)
GadgetronError::Cluster(String)
```

핵심 span:

- `collaboration.request`
- `collaboration.intent_classified`
- `ops.incident_opened`
- `ops.remediation_proposed`
- `approval.wait`
- `runbook.draft_created`
- `runbook.promoted`
- `report.generated`
- `workload.optimized`

필수 로그 필드:

- `request_id`
- `tenant_id`
- `actor_id`
- `role`
- `plane`
- `intent`
- `tool_name`
- `approval_id`
- `incident_id`
- `runbook_id`

#### 2.4.1 Security & Threat Model (STRIDE)

This section is required for Round 1.5 per `docs/process/03-review-rubric.md §1.5-A`.

**Assets**

| Asset | Sensitivity | Owner |
|------|-------------|-------|
| approval records and policy settings | Critical | Administrator |
| incident / remediation history | High | Operator |
| runbook drafts and promoted runbooks | High | Administrator / Operator |
| experience records linking human and agent actions | High | Operator |
| workload objectives and resulting artifacts | High | User / Operator |

**Trust boundaries**

| ID | Boundary | Crosses | Auth mechanism |
|----|----------|---------|----------------|
| B-C1 | administrator / user → gateway collaboration surface | HTTP/API boundary | API key / tenant auth |
| B-C2 | gateway → agent runtime | in-process orchestration boundary | authenticated request context |
| B-C3 | agent runtime → tool providers | policy and approval boundary | tool tier + mode policy |
| B-C4 | tool providers → cluster / scheduler / storage substrates | external control plane boundary | backend-specific credentials |
| B-C5 | experience capture → runbook promotion | memory-to-authority boundary | explicit admin approval |

**STRIDE table**

| Component | S | T | R | I | D | E | Highest unmitigated risk |
|-----------|---|---|---|---|---|---|--------------------------|
| collaboration HTTP surface | Medium — actor identity matters | Medium | Medium | Medium | Medium | Medium | mis-scoped actor context |
| experience capture pipeline | Low | Medium — evidence may be incomplete or reordered | Medium | High — incident history contains sensitive ops detail | Low | Medium | untrusted observation promoted as trusted memory |
| runbook promotion path | Medium | High — draft may be edited into unsafe automation | High | Medium | Medium | High | memory gaining authority without explicit review |
| operations / execution delegation | Medium | High | Medium | Medium | High | High | delegated destructive action bypassing approval |

**Mitigations**

| ID | Mitigation | Location |
|----|------------|----------|
| M-C1 | `Memory != Authority` — observation and execution rights are modeled separately | §1.5, §2.2.2 |
| M-C2 | `Shadow before Auto` — observation must pass summarize/draft/approve before reuse | §1.5, §2.2.4 |
| M-C3 | T1/T2/T3/T4 tier split with human-only boundary for irreversible actions | §2.2.3 |
| M-C4 | human-legible audit with request, incident, approval, and runbook IDs on every action | §1.5, §2.4 |
| M-C5 | lower-plane authority is delivered via typed tool providers, not ad-hoc shell execution | §1.5, `operations-tool-providers.md` |

### 2.5 의존성

즉시 landing 이 필요한 새 의존성은 없다. 구현 phase 별 후보는 아래와 같다.

| Phase | 후보 의존성 | 정당화 |
|------|-------------|--------|
| [P2B] | `cron` 또는 동급 scheduler crate | 주기 리포트 스케줄링 |
| [P2C] | `kube`, `k8s-openapi` | K8s operator / cluster / storage surface |
| [P3] | Slurm CLI adapter 유지 또는 thin client | 기존 HPC 연계 유지 |
| [P3] | 별도 virtualization client 는 optional | KubeVirt/OpenStack 는 지원 범위와 통합 범위를 분리해 검토 |

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 상위-하위 연결

```text
Administrator / User
        │
        ▼
gadgetron-web / CLI / SDK
        │
        ▼
gadgetron-gateway
        │
        ▼
KairosProvider / CollaborationCoordinator
   ├── Knowledge memory (wiki/search)
   ├── Operational memory (incidents/reports)
   ├── Runbook memory (approved playbooks)
   ├── Policy / Approval engine
   └── Tool providers
        ├── infra.*
        ├── scheduler.*
        ├── cluster.*
        ├── storage.*
        ├── virtualization.*
        └── workload.*
                │
                ▼
      router / scheduler / node / xaas / external control planes
```

### 3.2 데이터 흐름

```text
Human request
  -> Assistant Plane
  -> intent classification
  -> knowledge lookup OR ops/workload delegation
  -> approval/policy check
  -> tool execution
  -> result + structured evidence
  -> experience record
  -> optional runbook draft
```

### 3.3 크레이트 경계

- `gadgetron-core`: `CollaborationContext`, shared enums, policy-safe value objects만 보유
- `gadgetron-kairos`: intent classification, tool orchestration, experience draft creation
- `gadgetron-knowledge`: wiki/search 기반 knowledge memory
- `gadgetron-gateway`: approval/report/incident HTTP surface
- `gadgetron-xaas`: actor/tenant/auth/audit trail
- `gadgetron-scheduler` + `gadgetron-node`: execution substrate
- future `gadgetron-infra`: infra.* tools
- future `gadgetron-cluster`: cluster/storage/virtualization/workload tool providers

`gadgetron-core` 에 kube/slurm specific dependency 를 넣지 않는다는 점에서 D-12 crate boundary 를 유지한다.

### 3.4 타 도메인과의 인터페이스 계약

- **dx-product-lead**: direct/delegate/shadow UX, 보고서 copy, approval copy
- **ux-interface-lead**: approval card, incident timeline, runbook draft review UI
- **qa-test-architect**: 경험 축적이 실제 자동화 권한을 우회하지 못함을 검증
- **security-compliance-lead**: 관찰/기억/자동화 분리, audit completeness, prompt injection boundary 검토
- **devops-sre-lead**: cluster/storage/virtualization 대상 범위 정의

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 대상 | 검증 invariant |
|------|----------------|
| `WorkIntent` classifier | 같은 입력이 deterministic 하게 같은 plane/intent 로 분류되어야 함 |
| policy evaluator | T3/T4 가 자동 실행되지 않아야 함 |
| `ExperienceRecord` builder | evidence ref 누락 없이 구조화되어야 함 |
| runbook promotion rule | 승인 없는 promotion 이 불가능해야 함 |
| config validator | unsupported storage/virtualization auto-enable 이 시작 실패해야 함 |

### 4.2 테스트 하네스

- `gadgetron-core`: pure unit test
- `gadgetron-kairos`: fake tool providers + fake approval registry
- `gadgetron-gateway`: HTTP handler tests for incident/report/runbook endpoints
- property-based test:
  - policy matrix (`role × plane × mode × tier`)
  - intent classification fallback invariants

### 4.3 커버리지 목표

- line coverage 85%+
- branch coverage 75%+
- policy evaluator / config validator / runbook promotion path 는 90%+

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

1. **Assistant request**
   - user asks for a daily summary
   - Kairos reads wiki + prior report
   - response includes cited operational context

2. **Incident investigation**
   - fake alert arrives
   - agent opens incident draft
   - reads logs/metrics
   - proposes remediation
   - waits for approval

3. **Shadow learning**
   - administrator performs direct cluster action
   - event stream is captured
   - agent generates runbook draft
   - draft remains non-executable until approval

4. **Workload optimization**
   - user submits workload objective
   - scheduler + cluster tool providers choose target
   - result artifact + reasoning summary returned

### 5.2 테스트 환경

- PostgreSQL for audit / incident / report metadata
- fake GPU nodes
- fake K8s API server or mock `kubectl`
- fake Slurm command adapters
- local filesystem wiki repo

### 5.3 회귀 방지

이 테스트는 아래 변경을 반드시 실패시켜야 한다.

- T3 action 이 approval 없이 실행되는 변경
- shadow observation 이 곧바로 auto runbook 으로 승격되는 변경
- incident/report evidence chain 이 audit 에 남지 않는 변경
- unsupported storage/virtualization provider 가 침묵 속에 enable 되는 변경

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| Assistant Plane as `model = "kairos"` entry | [P2A] |
| Approval flow (`ask`) + incident/report HTTP surface | [P2B] |
| `InfraToolProvider`, runbook drafting, scheduled reports | [P2C] |
| `SchedulerToolProvider` + `ClusterToolProvider` | [P3] |
| storage.* / workload.* provider families | [P3] |
| virtualization.* provider family | [P4] |
| shadow learning -> approved automation pipeline | [P3] |
| multi-cluster federation + organization-wide experience sharing | [P4+] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID  | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| Q-1 | 경험 저장소의 canonical source 는 무엇인가? | A: wiki+git / B: PostgreSQL / C: hybrid | C — human-readable summary 는 wiki, structured linkage 는 PostgreSQL | 🟡 PM 검토 요청 |
| Q-2 | infra/cluster/storage/workload family 세부 계약을 어디서 canonical 로 잠글지 | A: `04-mcp-tool-registry.md` / B: 별도 ops 설계 문서 | B — `operations-tool-providers.md`로 세부 계약을 분리 | 🟢 내부 정리 완료 |
| Q-3 | user 가 직접 cluster 조치를 요청할 수 있는 최대 범위는? | A: T1 only / B: tenant-scoped T2 일부 / C: admin 승인 시 확대 | C — direct+delegate UX 와 안전성의 균형 | 🟡 사용자 승인 대기 |
| Q-4 | workload 범위를 inference 에 한정할지 general batch 까지 열지 | A: inference only / B: batch 포함 / C: plugin-defined | B — 제품 야망과 사용자 요구에 맞음, 단 Phase tag 명확화 필요 | 🟡 PM 검토 요청 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-15 — 예정
**결론**: 미실시

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [ ] 인터페이스 계약
- [ ] 크레이트 경계
- [ ] 타입 중복
- [ ] 에러 반환
- [ ] 동시성
- [ ] 의존성 방향
- [ ] Phase 태그
- [ ] 레거시 결정 준수

**다음 라운드 조건**: Round 1 리뷰어(@devops-sre-lead, @dx-product-lead) 검토 후

### Round 1.5 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §1.5` 기준)

### Round 2 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §2` 기준)

### Round 3 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §3` 기준)
