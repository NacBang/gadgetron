# Session Pivot — 2026-04-12

> **[RESOLVED 2026-04-13]** 본 파일은 사건 기록(historical)으로 보존. 여기서 제안된 Step 2–6의 "8-axis architecture review"는 `docs/architecture/platform-architecture.md` v1 (400 KB, 7300+ 줄) + `docs/architecture/phase-c-review-results.md` (24 KB, 7 reviewer consolidation) 로 **완료**됨. 중단되었던 Round 3 retry 3 track은 Sprint 4–9 구현이 먼저 수렴하면서 실질적으로 흡수되었고, 아래 "중단된 작업" 섹션의 편집은 **불필요**로 판정. Phase 2 설계 3종(00/01/02) v3 까지 진행되어 이 파일 작성 시점의 우려는 모두 해소됨. 새 세션은 `docs/process/04-decision-log.md` + `docs/design/phase2/00-overview.md` 에서 출발하면 충분.

> Week 1 Round 3 retry 중단, **전체 문서 재검토 (아키텍처 중심)**로 방향 변경.
> 다음 세션이 이 파일을 먼저 읽고 context를 복원해야 함. _(historical — see RESOLVED note above)_

---

## 📍 사용자 지시 (2026-04-12)

> "사실 처음부터 다시 모든 문서를 검토하고 싶음. 특히 아키텍처쪽도 미흡해보임."

**경로**: 1 (세션 저장 → Claude Code 재시작 → native subagents로 full review)
**초점**: 모두 체계적 검토 (8개 축 전체)
**gap 식별**: 에이전트 협업으로

---

## 🎯 다음 세션 TODO (순차)

### Step 0. 세션 시작 검증

Claude Code 재시작 후 **반드시** 먼저 확인:

```
/agents
```

다음 8개가 표시되어야 함:
- `chief-architect`
- `gateway-router-lead`
- `inference-engine-lead`
- `gpu-scheduler-lead`
- `xaas-platform-lead`
- `devops-sre-lead`
- `ux-interface-lead`
- `qa-test-architect`

**안 보이면**: `.claude/agents/*.md` 파일 존재 확인. 여전히 안 보이면 Claude Code 버전 확인 필요.

### Step 1. Context 복원

새 세션의 PM은 다음을 순서대로 읽어야 함:

1. `AGENTS.md` — 프로젝트 작업 규칙
2. 본 파일 (`docs/reviews/session-pivot-2026-04-12.md`) — 중단 시점 상태
3. `docs/process/04-decision-log.md` — 16개 확정 결정 (D-1~D-13 + D-20260411-01~13)
4. `docs/reviews/round2-platform-review.md` — Round 2 플랫폼 리뷰 (이전 findings)
5. `docs/reviews/round1-week1-results.md` — Week 1 Round 1 consolidation

### Step 2. Inventory (systematic re-read)

전체 문서를 full scan:

**Root**:
- `AGENTS.md`
- `Cargo.toml` (workspace)
- `config/nexus.toml`

**docs/**:
- `00-overview.md` (998 lines) — 전체 비전 + 아키텍처
- `modules/gateway-routing.md` (3039 lines)
- `modules/model-serving.md` (3421 lines)
- `modules/gpu-resource-manager.md` (1127 lines)
- `modules/xaas-platform.md` (1227 lines, rewritten 2026-04-11)
- `modules/deployment-operations.md` (2467 lines)
- `ui-ux/dashboard.md` (1549 lines)
- `reviews/pm-decisions.md` (D-1~D-13 legacy)
- `reviews/round1-pm-review.md`
- `reviews/round2-platform-review.md`
- `reviews/round1-week1-results.md`
- `design/core/types-consolidation.md` (941 lines, R2 retry)
- `design/testing/harness.md` (1149 lines, R2 retry)
- `design/xaas/phase1.md` (1482 lines, R2 retry)
- `process/*.md` (5 files)
- `agents/*.md` (8 files)

**crates/**:
- 8 existing crates: core, provider, router, gateway, scheduler, node, tui, cli
- 실제 구현 상태 확인 (대부분 stub)

### Step 3. Systematic Architecture Review — 8축 전체

PM이 8 subagents에게 병렬 deep-dive 지시:

| 축 | 내용 |
|---|---|
| **A** | 시스템 수준: 전체 구조, 크레이트 경계, 데이터 흐름, 의존성 그래프, public/private 모듈 분리 |
| **B** | 크로스컷: 관측성, 보안, 설정, 에러 처리, async/concurrency 일관성, 로깅 구조 |
| **C** | 배포 아키텍처: 단일노드 → 멀티노드 → K8s → Slurm → 멀티리전 진화 경로, config 관리 |
| **D** | 상태 관리: PostgreSQL + 메모리 캐시 + 파일 시스템 + Tokio 채널 합치성, 영속성 경계 |
| **E** | Phase 진화: Phase 1 → 2 → 3 major rewrite 없이 가능한가, API 안정성, 데이터 migration |
| **F** | 장애 모드: 실패 시나리오 × 각 컴포넌트, recovery path, circuit breaker, fallback, backpressure |
| **G** | 도메인 모델: Core 타입 vs 도메인 엔티티, ubiquitous language, bounded context |
| **H** | 성능 모델: latency budget 분해, throughput 계산, capacity planning, SLO 검증 경로 |

### Step 4. 8 Subagents 병렬 Deep-Dive

각 subagent가 본인 domain 관점으로 전체 문서 리뷰 (각 ~500-800 words):

| Subagent | 주 관점 |
|---|---|
| `chief-architect` | A · B · D · E · G (크로스컷 타입/트레이트/에러 일관성) |
| `gateway-router-lead` | API boundary (외부 vs 내부), middleware 조합성, F (장애 모드) |
| `inference-engine-lead` | 프로바이더 추상화, 프로토콜 경계, provider lifecycle |
| `gpu-scheduler-lead` | GPU hardware 추상화, NUMA/MIG scaling, H (성능 모델) |
| `xaas-platform-lead` | 멀티테넌시, billing event flow, D (상태 관리) |
| `devops-sre-lead` | C (배포 아키텍처), B (관측성 cross-cut), F (장애 복구) |
| `ux-interface-lead` | UI → API boundary, 실시간 데이터 모델 |
| `qa-test-architect` | 전 아키텍처의 testability + 관측 가능성 |

### Step 5. Gap Consolidation (PM)

- 8 subagent 리뷰 결과 통합
- 중복 제거 + 우선순위 매기기
- 심각도 분류 (BLOCKER / HIGH / MEDIUM / NICE)
- 도메인간 conflict 식별

### Step 6. 신규 결정 & Week 1 재정렬

- 아키텍처 gap 결과에 따른 신규 결정 (D-20260412-XX 시리즈)
- Phase 1 MVP scope 재검토 (현재 D-20260411-01 B 12주 유지 여부)
- Week 1 design docs 수정 필요 여부 결정 (현재 Round 2 retry 통과 상태)
- Round 3 retry 재개 (기존 접근) 또는 폐기 후 새 접근 결정

---

## 📦 보존된 상태 (세션 간 persistent)

### Decision log: 16 결정
- **Legacy**: D-1 ~ D-13 (pm-decisions.md)
- **Session 2026-04-11**:
  - D-20260411-01: Phase 1 MVP 범위 (**옵션 B**, 12주 중간)
  - D-20260411-02: Gemini 전략 (옵션 A, Week 5-7 `SseToChunkNormalizer` 추출 후)
  - D-20260411-03: `gadgetron-xaas` 단일 크레이트 신설
  - D-20260411-04: GPU Phase 1 범위 (옵션 B, VRAM + LRU + NUMA + MIG static)
  - D-20260411-05: `gadgetron-testing` 단일 크레이트 신설
  - D-20260411-06: f64 vs i64 cents 경계 (routing f64, billing i64, `QuotaEnforcer::record_post`에서 변환)
  - D-20260411-07: 공유 UI 타입 → `gadgetron-core/src/ui.rs`
  - D-20260411-08: `GadgetronError::StreamInterrupted { reason }` variant
  - D-20260411-09: 감사 drop 정책 (Phase 1 채널 4096 + WARN, Phase 2 S3 fallback)
  - D-20260411-10: `Scope` enum 3-variant (`OpenAiCompat`, `Management`, `XaasAdmin`)
- **Session 2026-04-12**:
  - D-20260411-11: `Arc<dyn LlmProvider>` 유지 + rationale (LLM은 I/O bound)
  - D-20260411-12: Phase 1 `PgKeyValidator` moka LRU cache (10k entries, 10min TTL)
  - D-20260411-13: `GadgetronError::Database { kind, message }` variant + `DatabaseErrorKind` enum (leaf crate 보존, `sqlx_to_gadgetron` helper in xaas)

### Design docs (Round 2 retry 상태, Round 3 retry **미완**)
- `docs/design/core/types-consolidation.md` (941 lines)
- `docs/design/testing/harness.md` (1149 lines)
- `docs/design/xaas/phase1.md` (1482 lines)
- `docs/modules/xaas-platform.md` (1227 lines, rewritten)

### 리뷰 artifacts
- `docs/reviews/round1-week1-results.md`
- `docs/reviews/round2-platform-review.md`
- 각 design doc의 `## 리뷰 로그` 섹션 (Round 1 + Retry, Round 2 + Retry 완료, Round 3 + Retry **미완**)

### 팀 구성
- `docs/agents/*.md` (8 canonical, tool-agnostic markdown)
- `.claude/agents/*.md` (8 Claude Code adapter, 2026-04-12 생성 → **next session부터 작동**)
- `docs/process/00-agent-roster.md`

---

## ⚠️ 중단된 작업

### Track 1/2/3 Round 3 retry 편집 (취소)
- Track 1: +~150 lines (D-11 rationale, `Database` variant, `DatabaseErrorKind`)
- Track 2: +~180 lines (dyn rationale, `parking_lot::RwLock`, `wiremock` 단일화)
- Track 3: +~280 lines (`QuotaToken` Drop, `assert_object_safe`, moka LRU, `sqlx_to_gadgetron`)

**상태**: D-20260411-11/12/13 결정은 이미 기록됨. 실제 문서 편집만 미완. 아키텍처 리뷰 결과에 따라 재개 또는 폐기 결정.

---

## 🧠 세션 간 기억해야 할 Lessons

### Lesson 1: Claude Code subagent hot-reload 제약 (2026-04-12 발견)

- `.claude/agents/*.md` 파일은 **Claude Code 세션 시작 시점에만** 로드됨
- Mid-session으로 새 파일을 만들면 `/agents`에 보이지 않음
- `subagent_type: <new_name>` 호출 시 `"Agent type '<name>' not found. Available agents: general-purpose, statusline-setup, Explore, Plan, claude-code-guide"` 에러
- **해결**: 새 subagent 추가/수정 후 **Claude Code 완전 재시작 필요**
- **교훈**: Mid-session으로 subagent 편집을 시도하지 말 것. 사전에 모든 것을 세팅하거나, 변경 후 재시작 안내.

### Lesson 2: "Tool-agnostic" 해석 (2026-04-12 발견)

- **올바른 해석**: "모든 도구에서 작동 가능" → hybrid (canonical + optional tool-specific adapters)
- **잘못된 해석**: "tool-specific 파일 완전 배제" (단일 tool 지원 소실)
- **권장 구조**:
  ```
  docs/agents/*.md              # canonical (tool-agnostic markdown)
  .claude/agents/*.md           # Claude Code adapter (frontmatter + "read canonical" 지시)
  AGENTS.md                     # 모든 도구의 entry point
  ```
- **교훈**: 사용자가 "디펜던시 없이"라고 말해도 "모든 도구가 잘 작동하도록"으로 해석. Tool-specific adapter는 **보조 layer**로 유지.

### Lesson 3: Round 1/2/3 단일 domain review의 한계 (2026-04-12 발견)

- Round 1 = 도메인 정합성 (§1 rubric)
- Round 2 = 테스트 가능성 (§2 rubric)
- Round 3 = Rust 관용구 (§3 rubric)
- **빠진 것**: 아키텍처 전체 systematic review
- 결과: Week 1 design doc이 individual quality는 높지만, **cross-cut + state + failure mode + domain model + performance budget**은 못 잡음
- **교훈**: 3 round 외에 **Round 0 (아키텍처 기반 검증)** 또는 **Round 4 (architecture integration)** 필요. 또는 Round 1 이전에 아키텍처 scaffolding review 수행.

### Lesson 4: 아키텍처 리뷰 타이밍 (2026-04-12 user feedback)

- 3 round review 완료 후 사용자가 "아키텍처 미흡" 지적
- 이 시점까지 왔을 때는 design doc이 이미 substantial (3000+ lines)
- 수정 비용 증가
- **교훈**: 아키텍처 review는 **design doc 작성 전** 또는 **design doc 초안 직후**에 수행. 3 round가 끝나기 전.

---

## 🚀 Next Session User Instructions

사용자가 다음 세션 시작 시:

1. **먼저 `/agents` 실행** — 8개 subagents 표시 확인
2. 확인되면 단순히: **"아키텍처 리뷰 시작"** 또는 **"계속"**
3. 안 보이면: **"agents 안 보인다"** → PM이 진단

PM이 다음 세션에서 자동으로:
1. 본 파일 (`session-pivot-2026-04-12.md`) 로드
2. Context 복원 (Step 1)
3. 사용자 확인 후 Step 2-6 순차 진행
4. 각 Phase 완료 시 사용자에게 중간 보고

---

## 📎 부록: 취소된 작업 원본 reference

Round 3 리뷰 원본은 이 세션의 대화 로그에 존재. Key findings:

**Track 1 Round 3 (joint)**:
- @gateway-router-lead: ✅ Approved (minor documentation)
- @xaas-platform-lead: ⚠️ Conditional (D-12 sync + Track 3 cross-ref)

**Track 2 Round 3**:
- @chief-architect: ⚠️ Conditional Pass — 3 blocking Rust idiom 이슈
  - A1: `Arc<dyn LlmProvider>` hot path 근거
  - A2: `FakeGpuMonitor RwLock` 성능 (→ `parking_lot::RwLock`)
  - A3: `mockito + wiremock` 이중 사용 port collision

**Track 3 Round 3**:
- @chief-architect: ⚠️ Conditional Pass — 4 blocking Rust idiom + 성능 이슈
  - A1: `QuotaToken` Drop guard (async cancellation safety)
  - A2: `assert_object_safe()` compile-time tests (4 traits)
  - A4: `PgKeyValidator` per-request DB hit (3-5ms, SLO 위반) → moka cache
  - A6: `sqlx::Error → GadgetronError::Config(String)` context 손실

이 findings는 아키텍처 리뷰 결과에 따라 Round 3 retry 재개 시 다시 반영됨.

---

**Session 종료 시점**: 2026-04-12
**다음 세션 entry point**: `docs/reviews/session-pivot-2026-04-12.md` (본 파일)
