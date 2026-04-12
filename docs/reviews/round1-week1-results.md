# Round 1 Week 1 Cross-Review Results

> **날짜**: 2026-04-11
> **대상**: Week 1 설계 트랙 3개
> **리뷰어**: 6명 (gpu-scheduler-lead, inference-engine-lead ×2, gateway-router-lead ×2, devops-sre-lead)
> **방식**: 6 parallel Explore subagent calls, `docs/process/03-review-rubric.md §1` 적용
> **결과**: 3 트랙 전원 ⚠️ **Conditional Pass** (hard fail 없음)

---

## 📊 종합 결과

| Track | 문서 | Reviewer 1 | Reviewer 2 | 결론 | Action Items |
|:---:|---|:---:|:---:|:---:|:---:|
| 1 | `docs/design/core/types-consolidation.md` | @gpu-scheduler-lead ⚠️ | @inference-engine-lead ⚠️ | ⚠️ CP | 7 (4 blocking) |
| 2 | `docs/design/testing/harness.md` | @gateway-router-lead ⚠️ | @inference-engine-lead ⚠️ | ⚠️ CP | 15 (4 blocking) |
| 3 | `docs/design/xaas/phase1.md` + `docs/modules/xaas-platform.md` | @gateway-router-lead ⚠️ | @devops-sre-lead ⚠️ | ⚠️ CP | 21 (6 blocking) |

6명 전원 §1 체크리스트 8개 항목 (인터페이스 계약, 크레이트 경계, 타입 중복, 에러 반환, 동시성, 의존성 방향, Phase 태그, 레거시 결정) 통과. 모든 발견은 **문서화 gap** 또는 **신규 결정 sync 지연**이며, 구조적 결함은 없음.

---

## 🔴 신규 결정 D-20260411-08, 09, 10

Round 1 리뷰에서 기존 범위 밖 3건의 신규 결정이 필요해졌고, PM 재량으로 2026-04-11에 확정:

- **D-20260411-08 (옵션 A)**: `GadgetronError::StreamInterrupted { reason: String }` variant 추가 — SSE 중단 핸들링 명시 경로, Round 2 HIGH #10 해소
- **D-20260411-09 (옵션 C)**: 감사 로그 — Phase 1 채널 확대 (1024→4096) + WARN 메트릭 + Phase 2 WAL/S3 fallback 계획 + 배포 전 compliance review 주석
- **D-20260411-10 (옵션 A)**: `Scope` enum Phase 1 최소 3개 — `#[non_exhaustive] enum Scope { OpenAiCompat, Management, XaasAdmin }`

상세: [`../process/04-decision-log.md`](../process/04-decision-log.md) D-20260411-08 ~ 10

---

## 🔗 Cross-Track 공통 테마

1. **D-20260411-07 sync 지연** — Track 1, 2가 D-07 승인 직전 작성 → 공유 UI 타입 scope 미반영. Track 1이 먼저 `gadgetron-core/src/ui.rs` 추가 → Track 2가 `FakeGpuMonitor` 반환 타입 sync. 순서: **Track 1 → Track 2**.

2. **`GpuMonitor` trait 정의 위치** — Track 1이 `gadgetron-core`에 명시적 정의 필요. 현재 Track 1은 `docs/modules/gpu-resource-manager.md` 참조만. Track 2 `FakeGpuMonitor`가 이 trait import 필수.

3. **미들웨어 체인 상세 사양 부족** — Track 2 (`GatewayHarness`)와 Track 3 (`QuotaEnforcer::record_post` 위치) 둘 다 gateway 미들웨어 체인 타입 스펙 참조. 현재 `docs/modules/gateway-routing.md`는 diagram만. **Week 3 gateway-router-lead 설계 문서에서 해결 예정**. Track 2/3는 잠정 합의로 진행.

4. **Week 5 Normalizer Refactor 대응** — D-20260411-02의 Week 5 `SseToChunkNormalizer` 추출이 Track 2 mock 구현 (Week 2-4) 이후 시점. Track 2가 "Week 5 refactor 후에도 mock 통과" 시나리오 명시 필요.

---

## 📋 Track 1: Core Types Consolidation

**문서**: `docs/design/core/types-consolidation.md` (@chief-architect)
**결론**: ⚠️ Conditional Pass
**Reviewers**: @gpu-scheduler-lead, @inference-engine-lead

### Blocking Action Items (Round 1 retry 필수)

- **A1** — **D-20260411-07 반영**: §2.1.9 신규 섹션 "Shared UI Types" 추가
  - `gadgetron-core/src/ui.rs` 신규 모듈 스펙
  - `GpuMetrics`, `ModelStatus`, `RequestEntry`, `ClusterHealth`, `WsMessage` 정의 (필드는 `docs/ui-ux/dashboard.md §8.3` 준수)
  - prelude에 노출

- **A2** — **D-20260411-06 경계 명시**: §2.1.4 `EvictionPolicy` 섹션에 주석 추가
  - "`EvictionPolicy::CostBased`는 `gadgetron-router`의 f64 `RoutingDecision::estimated_cost_usd`를 입력 → scheduler 내부 `f64 → f64` 비교만 수행"
  - "청구 정산용 i64 cents 변환은 `gadgetron-xaas::QuotaEnforcer::record_post()`에서만 발생. 본 문서는 구조만 정의."

- **A3** — **`LlmProvider::is_ready` 추가** (Round 2 HIGH #8):
  ```rust
  #[async_trait]
  pub trait LlmProvider: Send + Sync {
      async fn is_live(&self) -> Result<bool>;                      // 엔진 프로세스 생존
      async fn is_ready(&self, model: &str) -> Result<bool>;         // 모델 로드 완료
      // 기존 chat/chat_stream/models/name
  }
  ```

- **A4** — **D-20260411-08 반영**: `GadgetronError::StreamInterrupted { reason: String }` variant 추가
  - `#[error("Stream interrupted: {reason}")]`
  - Retry policy 표 업데이트: idempotent stream 1회 retry, Prometheus label `error_kind="stream_interrupted"`

### Non-blocking (Round 2까지)

- **A5** — `RangePortAllocator` rustdoc: "SAFETY: Mutex poisoning = fatal node state, heartbeat isolation"
- **A6** — §3 VRAM sync 메모: "런타임 VRAM 동기화 프로토콜은 `docs/design/scheduler/phase1.md` §4.1.1 소관. Core는 `ModelState::Running{pid}` + `Arc<NumaTopology>` immutable 공유만 제공."
- **A7** — MIG types (`MigProfile`, `MigInstance`, `MigMode`) D-12 placement 확인: `gadgetron-node/src/mig.rs` 배치 + §3.4 주석
- **A8** — `EvictionPolicy::CostBased` Phase 1 stub 구조 명시 (fieldless vs `{threshold_usd_per_hour: f64}`)
- **A9** — `#[non_exhaustive]` × proptest 상호작용 (Q-5): qa-test-architect와 `testing-support` feature flag 협의

### Key Findings (Top 3)

1. D-07 (UI types) scope 미반영 → Track 2 의존성 block
2. D-06 (f64/i64 경계) 미반영 → routing/billing 변환 지점 불명확
3. `LlmProvider::is_ready` 미정의 → model loaded 감지 경로 부재

---

## 📋 Track 2: Testing Harness

**문서**: `docs/design/testing/harness.md` (@qa-test-architect)
**결론**: ⚠️ Conditional Pass
**Reviewers**: @gateway-router-lead, @inference-engine-lead

### Blocking Action Items

- **A1** — **`GatewayHarness` + `Router` state shape 명시**: `axum::Router::with_state(Arc<Router>)` 및 trait-based routing strategy 연결점. gateway-router-lead의 Week 3 설계 문서 대기하되, 잠정적 state shape은 `Arc<Router> + Arc<[dyn LlmProvider]>`로 명시.

- **A2** — **`FakeGpuMonitor` builder 확장**: `with_temperature(c)`, `with_power(w)`, `with_clock(mhz)`, `with_fan(rpm)` 메서드 추가. D-07 `GpuMetrics` 10개 필드 모두 설정 가능.

- **A3** — **`MockGemini` polling → SSE-like 구체화**: D-20260411-02 Week 5-7 `PollingToEventAdapter` 레이어 spec을 mock에 반영. `generateContent` (non-stream) + `streamGenerateContent` (chunked) 둘 다 지원. character-based token counts.

- **A4** — **Anthropic mock full event sequence**: `message_start`, `content_block_start`, `content_block_delta` (multi), `content_block_stop`, `message_delta`, `message_stop`, `event: error`, `event: ping` 전부 emit.

### Non-blocking

- **A5** — `MockFailingProvider` per-method 실패 제어: `.fail_on_method(Method::Chat).with_error(GadgetronError::Provider(...))`
- **A6** — OpenAI mock edge cases: partial chunk, `tool_calls` delta (vLLM/SGLang 구분), multi-content-part (vision), legacy `function_call`
- **A7** — Protocol translation snapshot 출처 명시 (insta baseline): real API vs synthetic, drift 방지 전략
- **A8** — Middleware chain 순서 검증 path: `test_middleware_order_auth_before_ratelimit` integration test outline
- **A9** — dev-dep CI check script: `devops/ci/check-dev-dep.sh` (production binary에 gadgetron-testing 미노출 확인)
- **A10** — Week 5 Normalizer refactor 영향 평가: `SseToChunkNormalizer` 추출 후 mock/fixture/snapshot 생존 시나리오
- **A11** — Scenario 3 (VRAM eviction) cross-harness composition 명시
- **A12** — `FakeNode` `GpuMonitor` 가정 — Track 1 A1 sync 대기
- **A13** — Circuit breaker 60s window: "mock은 call count only, 실제 60s는 Router 소관" caveat
- **A14** — Real API response playback (VCR-style) — [P2]
- **A15** — `loom` sync testing — [P2]

### Key Findings (Top 3)

1. `GatewayHarness` state shape 불명확 → Week 3 gateway-router-lead 문서 대기
2. Gemini mock polling 구체화 부족 → D-20260411-02 timing 동기화 필요
3. Anthropic full event sequence 누락 → production 장애 재현 불가

---

## 📋 Track 3: XaaS Phase 1

**문서**: `docs/design/xaas/phase1.md` + `docs/modules/xaas-platform.md` (@xaas-platform-lead)
**결론**: ⚠️ Conditional Pass
**Reviewers**: @gateway-router-lead, @devops-sre-lead

### Blocking Action Items

- **A1** — **D-20260411-10 `Scope` enum 정의**:
  ```rust
  #[non_exhaustive]
  pub enum Scope { OpenAiCompat, Management, XaasAdmin }
  ```
  - `api_keys.scopes TEXT[]` 스키마 추가
  - `ValidatedKey.scopes: Vec<Scope>`
  - `gadgetron-gateway/src/middleware/auth.rs::require_scope()` 헬퍼
  - 키 타입별 기본 scope (gad_live_* → [OpenAiCompat], 등)

- **A2** — **`QuotaEnforcer::record_post()` tower layer 위치 명확화**: pseudo-code 포함
  - `tower::ServiceExt::map_result` 사용법 구체화
  - axum response hook 메커니즘 diagram
  - 호출 순서도: `Auth → RateLimit → Routing → ProtocolTranslate → Provider → ProtocolTranslate(reverse) → record_post → Metrics`
  - `QuotaToken` lifetime 명시

- **A3** — **HTTP Status Mapping 테이블 추가** (§2.4 신설):
  | `GadgetronError` | HTTP | Response Body |
  |:---|:---:|:---|
  | `TenantNotFound` | 401 | `{"error": {"type": "invalid_api_key", "message": "..."}}` |
  | `QuotaExceeded` | 429 | `{"error": {"type": "rate_limit_exceeded", "message": "..."}}` |
  | `Billing` | 402 | Phase 2 |
  | `StreamInterrupted` | 499 client_closed | `{"error": {"type": "stream_interrupted", ...}}` |
  | `Config` | 500 | generic internal error |

- **A4** — **D-20260411-09 반영**: 감사 drop 정책
  - `mpsc::channel(4096)` (기존 1024의 4배)
  - `metrics::counter!("gadgetron_xaas_audit_dropped_total")` + WARN 로그
  - "Phase 2에 S3/Kafka fallback 추가 예정" 주석
  - "프로덕션 배포 전 compliance review 필수" 주석

- **A5** — **Graceful shutdown drain audit**:
  - `PgAuditWriter::flush(&self) -> Result<(), GadgetronError>` async 메서드 추가
  - `main.rs::with_graceful_shutdown` 콜백 내 호출
  - 5초 timeout + progress log

- **A6** — **Migration idempotency + IF NOT EXISTS**:
  - `gadgetron-xaas/src/db/migrations/README.md` 생성
  - Forward-only 정책 명시
  - 모든 DDL에 `CREATE TABLE IF NOT EXISTS` 적용
  - sqlx-cli 명령 예제

### Non-blocking

- **A7** — D-20260411-06 명시적 인용: "Phase 1 `cost_cents = 0`, Phase 2 `compute_cost_cents(usage, rate)` 구현"
- **A8** — `KeyValidator` 캐싱: "Phase 1 매 요청 DB hit, Phase 2 LRU cache (60s TTL)"
- **A9** — `SqlxError → GadgetronError` 매핑: Phase 1은 `Config(String)` 임시, Phase 2에 별도 variant (chief-architect 협의)
- **A10** — CI DATABASE_URL: `.github/workflows/ci.yml` 에 `services.postgres` + `sqlx migrate run`
- **A11** — Boot-time DB connection test (fail-fast)
- **A12** — k8s Secret DATABASE_URL spec (Phase 2 Helm chart)
- **A13** — Prometheus 메트릭 위치: `gadgetron-router::MetricsStore` 통합 vs 별도 — devops-sre-lead 후속 결정
- **A14** — Config 부팅 검증 `validate_xaas()` 함수
- **A15** — `RUST_LOG=gadgetron_xaas=info` 프로덕션 기본
- **A16** — COPY vs UNNEST bulk insert 결정
- **A17** — `tenant_cache` TTL 구현 확인 (30초)
- **A18** — Virtual key tenant mismatch 검증
- **A19** — GitHub Actions testcontainers workflow yaml
- **A20** — `PgKeyValidator` rustdoc (Send+Sync, axum Extension compat)
- **A21** — Connection pool fine-tune 가이드

### Key Findings (Top 3)

1. `Scope` enum 미정 → route guard 구현 불가 (D-10으로 해결)
2. `record_post()` 미들웨어 체인 내 위치 불명확 → 실제 구현 시 lifetime 오류 가능
3. 감사 drop compliance 위험 → D-09로 완화 (Phase 1 경량 유지 + Phase 2 fallback)

---

## ➡️ Round 1 Retry 계획

### 병렬 실행 순서

1. **Track 1 먼저** (@chief-architect): A1~A4 blocking. 완료 후 Track 2 unblock.
2. **Track 2 병렬 가능** (@qa-test-architect): A1~A4 blocking. A12 (GpuMonitor trait 가정)은 Track 1 A1 완료 후 sync.
3. **Track 3 병렬 가능** (@xaas-platform-lead): A1~A6 blocking. D-08 (`StreamInterrupted`)은 Track 1과 독립.

### 의존성

- Track 2 `FakeGpuMonitor` → Track 1 `GpuMonitor` trait 정의 대기
- Track 3 `GadgetronError` 매핑 → Track 1 D-08 variant 추가 대기 (소프트 의존성)
- 실제 구현 순서는 **Track 1 먼저** → **Track 2/3 병렬**

### Round 2 진입 조건

- 3 Track 전원 Round 1 retry 통과 (blocking Action Items 모두 해결)
- Cross-track 의존성 해소 확인
- 새 결정 D-08/09/10 반영 확인

---

## 📝 리뷰 원본

6명 리뷰어의 full 리뷰는 2026-04-11 Claude Code 세션 로그에서 확인 가능. 본 문서는 공식 consolidation.

**다음 단계**: Track 1/2/3 저자 (chief-architect, qa-test-architect, xaas-platform-lead)가 병렬로 Round 1 retry 실행.
