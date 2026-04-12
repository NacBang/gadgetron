# Gadgetron 전체 플랫폼 리뷰 — Round 2

> **일자**: 2026-04-11
> **초점**: 연결성(Connectivity) · 확장성(Extensibility) · 완결성(Completeness)
> **참여자**: chief-architect · gateway-router-lead · inference-engine-lead · gpu-scheduler-lead · xaas-platform-lead · devops-sre-lead · ux-interface-lead · qa-test-architect (PM 주재)
> **방식**: 8명 전문가 병렬 리뷰 → PM consolidation
> **상태**: Review Completed, PM Escalation Pending

---

## 0. 한 줄 요약

**현재 Gadgetron은 설계 문서는 포괄적이지만 실제 코드는 "Phase 0 스텁"에 머물러 있고, 확정된 D-1~D-13 결정 13개 중 12개가 미반영 상태다. 8명 전원이 Phase 1 MVP 출시 전 즉시 해결해야 할 BLOCKER 9건과 HIGH 리스크 5건을 합의했으며, PM은 5건의 사용자 승인 결정을 대기한다.**

---

## 1. 확정 결정 적용 현황 (D-1 ~ D-13)

Round 1에서 사용자가 승인한 결정 13개의 코드·문서 반영 매트릭스:

| ID | 내용 | 문서 | 코드 | 비고 |
|----|------|:----:|:----:|------|
| D-1 | `ParallelismConfig { tp_size/pp_size/ep_size/dp_size + numa_bind }` | ⚠️ | ❌ | `gadgetron-core`에 struct 없음 |
| D-2 | `EvictionPolicy` 4-variant (Lru/Priority/CostBased/WeightedLru) | ⚠️ | ❌ | `scheduler.rs`에 enum 없음 |
| D-3 | `NumaTopology`·`GpuNumaMapping` in `gadgetron-core` | ⚠️ | ❌ | 타입 자체 미정의 |
| D-4 | PostgreSQL Phase 1부터 | ❌ | ❌ | `xaas-platform.md` 여전히 SQLite, sqlx 의존성 미추가 |
| D-5 | gRPC Phase 2 | ✅ | ✅ | 자동 준수 (미구현) |
| D-6 | Phase 1 MVP 범위 (graceful shutdown + health + Bearer) | ⚠️ | ❌ | 3종 모두 0% 구현 |
| D-7 | API 경로 네임스페이스 (`/v1`, `/api/v1`, `/api/v1/xaas`) | ⚠️ | ❌ | `routes.rs`에 경로 없음 |
| D-8 | i64 센트 과금 (f64 금지) | ❌ | ❌ | `routing.rs` f64, `xaas-platform.md` REAL |
| D-9 | sqlx 0.8 + sqlx-cli | ❌ | ❌ | 의존성 미추가, 마이그레이션 디렉토리 없음 |
| D-10 | `ModelState::Running { port, pid }` | ⚠️ | ❌ | `port`만 있고 `pid` 누락, NodeAgent가 Child handle 미저장 |
| D-11 | API 키 `gad_` 접두사 | ❌ | ❌ | `xaas-platform.md` 여전히 `gdt_` |
| D-12 | 크레이트 경계표 | ⚠️ | ❌ | 크레이트는 있으나 타입 12개 미정의 |
| D-13 | `GadgetronError` 5개 variant (Billing/Tenant/Quota/Download/HotSwap) | ⚠️ | ❌ | `error.rs`에 미추가, XaaS 타입 전파 불가 |

**결과**: D-5만 "자동 준수", 나머지 12개는 미반영 또는 부분 반영.

---

## 2. 종합 평가

### 2.1 연결성 (Connectivity): **D+**

크레이트 물리 구조는 D-12에 맞춰 생성되었으나, 실제 모듈 간 데이터·이벤트 흐름은 거의 전무.

- **Gateway → Router → Provider**: 크레이트 의존성 OK, 핸들러 0% 구현 (`routes.rs` 9개 전부 `{"error":"not implemented"}`)
- **Scheduler ↔ NodeAgent**: `RwLock<HashMap>`만 있고 VRAM 상태 동기화 프로토콜 미정의 (push/pull 미정)
- **XaaS 크로스컷**: `gadgetron-xaas` 크레이트 자체 미존재. billing·quota·audit 훅 없음
- **UI ↔ Gateway**: 공유 타입(`GpuMetrics`/`ModelStatus`/`WsMessage`) 미정의, WebSocket 엔드포인트 없음
- **관측성 크로스컷**: tracing span propagation, `request_id` 미들웨어, Prometheus exporter 전부 미구현

### 2.2 확장성 (Extensibility): **C**

설계는 플러그인 지향이나 구현 기반이 취약하여 실제 확장 어려움.

- `#[non_exhaustive]` 부재 → 새 variant 추가가 breaking change
- GPU 추상화 NVIDIA 전제 (AMD ROCm, Intel Gaudi 미지원)
- Routing strategy `match` 기반 closed design (trait object 미사용)
- `ProviderConfig` serde tag 하드코딩 → 핫 리로드 시 신규 provider 타입 추가 불가
- Multi-node Pipeline Parallelism, Multi-region federation 미설계
- `MetricsStore::get()`이 O(n) 전체 scan → 100 provider × 1000 model 규모에서 병목

### 2.3 완결성 (Completeness): **D-**

D-1~D-13 결정 대부분 미반영. 설계 문서 자체에 undefined behavior 다수.

- 프로세스 종료 SIGTERM → SIGKILL 타임아웃 FSM 미정
- 스트림 연결 끊김/keepalive 정책 미정
- 열 정책 action (throttle / drain / rebalance) 미정
- 비용 기반 eviction의 `$/token` 데이터 소스 없음
- Quota enforcement point (pre-request vs post-inference) 미정
- Audit log write path (sync vs async channel) 미정
- MIG 프로파일 동적 재구성 순서 미정
- NVML 부재 시 graceful degradation 미정

---

## 3. 연결성 분석 — 깨진 인터페이스

### 3.1 Gateway → Router → Provider (9단계 미들웨어)

**설계** (`docs/modules/gateway-routing.md §5.2`):
```
Auth → RateLimit → Guardrails → Routing → ProtocolTranslate
    → Provider → ProtocolTranslate(reverse) → Guardrails(out) → Metrics
```

**현실** (`gadgetron-gateway/src/server.rs`):
```rust
Router::new()
    .layer(CorsLayer::permissive())  // D-6 위반 (O-7)
    .layer(TraceLayer::new_for_http())
// Auth 없음, RateLimit 없음, Guardrails 없음, Routing 호출 없음, Metrics 없음
```

**영향**: 무인증 요청으로 모든 provider API 호출 가능. 보안 제로.

### 3.2 Scheduler ↔ NodeAgent VRAM 동기화

**설계**: `Scheduler::find_eviction_candidate()`가 node의 `available_vram_mb()` 조회.

**현실**:
- `gadgetron-scheduler/src/scheduler.rs`는 `nodes: Arc<RwLock<HashMap<String, NodeStatus>>>` 보유
- 업데이트 메커니즘 없음. NodeAgent는 local `ResourceMonitor` 만 collect, scheduler로 push 안 함
- Polling 주기, heartbeat 타임아웃 미정의
- 멀티노드에서 stale VRAM 데이터로 OOM 배치 가능

### 3.3 XaaS (존재 자체가 없음)

**설계** (`docs/modules/xaas-platform.md`): GPUaaS + ModelaaS + AgentaaS 3계층, gateway/scheduler/provider를 cross-cut.

**현실**:
- `gadgetron-xaas` 크레이트 없음
- 기존 크레이트 어디에도 tenant / billing / quota 타입 없음
- `GadgetronError`에 `Billing`/`TenantNotFound`/`QuotaExceeded`/`DownloadFailed`/`HotSwapFailed` variant 없음 (D-13 미반영)
- Gateway 미들웨어에 auth/quota hook 지점 없음

### 3.4 UI ↔ Backend

**설계** (`docs/ui-ux/dashboard.md §8.3`): TUI + Web이 공유 Rust 타입 import, WebSocket `/api/v1/ws/metrics` 실시간 구독.

**현실**:
- 공유 타입 `GpuMetrics`·`ModelStatus`·`RequestEntry`·`ClusterHealth`·`WsMessage`가 `gadgetron-core` 어디에도 없음 (문서 스펙만 존재)
- `gadgetron-gateway/src/server.rs`에 WebSocket 라우트 없음 (Round 1 M-2 미해결)
- `gadgetron-tui/Cargo.toml`에 `tungstenite`·`reqwest` 없음 → HTTP/WS 클라이언트 불가
- TUI `ui.rs`는 레이아웃 스켈레톤만, 데이터 렌더링 0%

### 3.5 관측성 크로스컷

**설계** (`docs/modules/deployment-operations.md`): tracing (JSON) + Prometheus exporter + OpenTelemetry span propagation (Jaeger/Tempo).

**현실**:
- `gadgetron-cli/src/main.rs`에서 `tracing_subscriber::fmt()` 기본 초기화만 (JSON 포맷 비활성)
- `request_id` / `trace_id` 미들웨어 없음
- `/metrics` 엔드포인트 없음 → `MetricsStore` DashMap이 외부로 노출 안 됨
- SSE 스트림이 span context 전파 안 함 → 게이트웨이→프로바이더 추적 불가

---

## 4. Top 14 리스크

| # | 심각도 | 리스크 | 담당 | Phase 1 블로커 |
|---|:------:|--------|------|:---:|
| 1 | 🔴 BLOCKER | `gadgetron-core` D-1/D-2/D-3/D-10/D-13 타입 12개 미정의 → 다른 크레이트 컴파일 대기 | chief-architect | ✅ |
| 2 | 🔴 BLOCKER | Gateway 9개 핸들러 전부 stub, `/v1/chat/completions` 미작동 | gateway-router-lead | ✅ |
| 3 | 🔴 BLOCKER | D-6 MVP 3종(graceful shutdown + 실제 health check + Bearer auth) 0% | devops-sre-lead + gateway-router-lead | ✅ |
| 4 | 🔴 BLOCKER | `docs/modules/xaas-platform.md` 전체가 SQLite syntax + `REAL` 과금 (D-4·D-8 위반) → 문서 대규모 재작성 필요 | xaas-platform-lead | ✅ |
| 5 | 🔴 BLOCKER | Scheduler ↔ NodeAgent VRAM 동기화 프로토콜 미설계 → OOM 배치 가능 | gpu-scheduler-lead | ✅ |
| 6 | 🔴 BLOCKER | Mock LLM / Fake GPU / Integration test harness 0 → CI 작동 불가, Round 2 전원 fail | qa-test-architect | ✅ |
| 7 | 🔴 BLOCKER | 공유 UI 타입(`GpuMetrics` 등) 미정의 + WebSocket `/api/v1/ws/metrics` 엔드포인트 없음 | ux-interface-lead + gateway-router-lead | ✅ |
| 8 | 🔴 BLOCKER | `gadgetron-xaas` 크레이트 자체 미존재, billing·tenant·agent 통합 훅 없음 | xaas-platform-lead | 🟡 |
| 9 | 🔴 BLOCKER | `MigManager`·`ThermalController` `todo!()` → MIG 파티션 제약 무시 | gpu-scheduler-lead | 🟡 |
| 10 | 🟠 HIGH | Gemini adapter 완전 누락 (M-1 미해결), 코드 0 LOC | inference-engine-lead | 🟡 |
| 11 | 🟠 HIGH | `NodeAgent`가 `Child` handle 미저장, `stop_model()` stub → 프로세스 추적/kill 불가 | inference-engine-lead | ✅ |
| 12 | 🟠 HIGH | SSE protocol normalizer 각 adapter가 hand-roll (Anthropic ↔ OpenAI) → 엣지 케이스 발산 | inference-engine-lead | ✅ |
| 13 | 🟠 HIGH | Prometheus exporter / OpenTelemetry span propagation 0% → 장애 추적 불가 | devops-sre-lead | ✅ |
| 14 | 🟠 HIGH | 5개 모듈 doc 전부 "테스트 전략" 섹션 없음 → Round 2 testability review 전원 fail | qa-test-architect + 각 lead | ✅ |
| 15 | 🟡 MED | `#[non_exhaustive]` 미사용 → 장기 확장성 위험 | chief-architect |  |
| 16 | 🟡 MED | Routing strategy `match` 기반 폐쇄 설계 | gateway-router-lead |  |
| 17 | 🟡 MED | NVIDIA-only GPU 추상화 (AMD/Intel 부재) | gpu-scheduler-lead |  |

---

## 5. PM 에스컬레이션 — 사용자 승인 필요 결정 5건

팀 내 합의로 해결 못한 사안을 사용자 결정에 맡깁니다. 승인 시 `docs/process/04-decision-log.md`로 이관합니다.

---

### D-20260411-01: Phase 1 MVP 범위 재정의 [BLOCKER]

**발의**: PM (8명 합의)
**배경**: D-6에서 "Phase 1 MVP 최소 범위"가 승인되었으나, 현 코드 상태는 사실상 Phase 0 (모든 핸들러 stub, CLI wiring 없음). 5개 모듈 설계 문서(총 13,000줄)의 전체 범위를 8주 내 구현하는 것은 불가능.

**옵션**:

| 옵션 | 범위 | 예상 | 설명 |
|------|------|:---:|------|
| **A. 최소 집합** | `/v1/chat/completions` SSE + Bearer auth + graceful shutdown + 실제 health check + RoundRobin·Fallback 2개 전략 + OpenAI·Ollama 2개 provider. **XaaS·Gemini·MIG·TUI·관측성 고급 기능 전부 Phase 2 연기** | 8주 | 작동하는 최소 UX 확보 |
| **B. 중간 집합** | A + 6 provider (Gemini 포함) + 6 routing strategy + 기본 scheduler + 기초 TUI + Prometheus exporter | 12주 | 기능 완결성 강화 |
| **C. 원설계 유지** | 모든 모듈 설계 문서 그대로 구현 | 24주+ | 일정 초과 확실 |

**PM 추천**: **옵션 A**.
이유: LlmProvider 트레이트·MetricsStore·SSE 파이프라인이 견고해지면 나머지 확장 비용은 낮음. 작동하는 것을 빨리 만들고 Phase 2에서 넓히는 전략이 리스크 최소.

**영향 범위**: 전 모듈 도큐멘트, 전 크레이트, Phase 로드맵.

---

### D-20260411-02: Gemini Adapter 시점 [HIGH]

**발의**: inference-engine-lead
**배경**: Round 1 M-1은 "Phase 1 포함"으로 기록했으나 실제 구현 0 LOC. 현재 5 provider만 존재 (OpenAI/Anthropic/Ollama/vLLM/SGLang). Gemini는 polling 기반, function_calling 포맷이 OpenAI와 이질적.

**옵션**:

| 옵션 | 시점 | 설명 |
|------|------|------|
| **A. Phase 1 포함** | MVP 내 (~3일) | 일정 압박, 프로토콜 이질성으로 복잡도 증가 |
| **B. Phase 1b 즉시 추가** | MVP 직후 | Phase 1 안정화 후 마이너 릴리즈 |
| **C. Phase 2 연기** | 3-6개월 후 | Agent/Tool 기능과 함께 |

**PM 추천**: **옵션 B**.
이유: Gemini를 MVP에 끼워넣기보다 Phase 1b에서 제대로 구현하는 것이 안전. `ProviderConfig::Gemini` 설정은 남겨두고 어댑터만 나중에 추가.

---

### D-20260411-03: `gadgetron-xaas` 크레이트 신설 [BLOCKER]

**발의**: xaas-platform-lead
**배경**: XaaS 3계층 + billing + multi-tenant 기능이 설계만 존재하고 크레이트 없음. D-12 크레이트 경계표에 XaaS 항목 없음.

**옵션**:

| 옵션 | 구조 | 설명 |
|------|------|------|
| **A. 단일 `gadgetron-xaas`** | 전 기능 한 크레이트 | 간단, 초기 개발 빠름 |
| **B. 3개 분리** | `gadgetron-billing` + `gadgetron-tenant` + `gadgetron-agent` | 경계 명확, 복잡도 증가 |
| **C. 기존 크레이트 흡수** | billing→router, tenant→gateway, agent→provider | 순환 의존 위험 |

**PM 추천**: **옵션 A**.
이유: 초기에는 단일 크레이트가 간단. 복잡도 증가 시 Phase 3에서 분할. D-12에 `gadgetron-xaas` 행 추가 필요.

---

### D-20260411-04: Phase 1 GPU 기능 범위 [HIGH]

**발의**: gpu-scheduler-lead
**배경**: `MigManager`·`ThermalController`·multi-node Pipeline Parallelism 모두 `todo!()`. Phase 1에 어디까지 포함할지 미정.

**옵션**:

| 옵션 | 포함 범위 | 예상 |
|------|----------|:---:|
| **A. 최소** | VRAM 추정 + LRU eviction + 단일 GPU 배치만 + NVML 메트릭 수집 | 3주 |
| **B. 중간** | A + NUMA 인식 + MIG enable/disable | 6주 |
| **C. 전체** | B + 열/전력 스로틀링 + multi-node PP + cost-based eviction | 12주+ |

**PM 추천**: **옵션 A**.
이유: MVP 사용자는 full-GPU 모델을 주로 사용. MIG·열·multi-node PP는 multi-tenant와 함께 Phase 2에서 완성해야 시너지.

---

### D-20260411-05: `gadgetron-testing` 크레이트 신설 [BLOCKER]

**발의**: qa-test-architect
**배경**: Mock LLM provider, Fake GPU node, Integration test harness가 전부 없음. 5개 모듈 doc 전원이 Round 2 testability review 통과 불가.

**옵션**:

| 옵션 | 구조 | 설명 |
|------|------|------|
| **A. 단일 `gadgetron-testing`** | `MockLlmProvider`, `FakeGpuMonitor`, `FixtureBuilder`, e2e harness 집중 | cross-crate import 간편 |
| **B. 각 crate 분산** | `gadgetron-provider/src/mock.rs`, `gadgetron-node/src/fake.rs` 등 | 경계 유지 but 재사용 어려움 |
| **C. `#[cfg(test)]` 내장** | test-only 모듈 | 크로스 크레이트 공유 불가 |

**PM 추천**: **옵션 A**.
이유: e2e 시나리오와 fixture builder를 cross-crate에서 import하려면 독립 크레이트가 필요. D-12에 `gadgetron-testing` 행 추가 필요.

---

## 6. 각 서브에이전트 핵심 발견 (Top 3 Only)

### 6.1 chief-architect
1. **[BLOCKER]** `gadgetron-core/src/error.rs`에 D-13 5개 variant 미추가 → XaaS/billing 타입 전파 불가
2. **[BLOCKER]** `gadgetron-core`에 D-12 타입 12개(ParallelismConfig, NumaTopology, GpuNumaMapping, NvLinkInfo, ComputePartitioning, EvictionPolicy, DownloadState, PortAllocator, 확장된 ModelState 등) 미정의 → 다른 크레이트 컴파일 불가
3. **[HIGH]** `routing.rs`의 `CostEntry`·`RoutingDecision::estimated_cost_usd`·`ProviderMetrics::total_cost_usd` 전부 `f64` → D-8 위반, 과금 오류 유발

### 6.2 gateway-router-lead
1. **[BLOCKER]** Gateway 9개 핸들러 전부 `{"error":"not implemented"}` → `/v1/chat/completions` 미작동
2. **[BLOCKER]** 9단계 미들웨어 체인 완전 부재 → Auth/RateLimit/Guardrails 작동 불가, 무인증 provider 호출 가능
3. **[HIGH]** `Router::chat_stream()` → HTTP 응답 변환 파이프라인 끊김 (`routes.rs`에서 `chat_chunk_to_sse()` 호출 안 함) → SSE 스트리밍 작동 불가

### 6.3 inference-engine-lead
1. **[BLOCKER]** `ModelState::Running`에 `pid: u32` 누락 (D-10 미적용) → 프로세스 추적/kill 불가, `gadgetron-node/src/agent.rs`의 `stop_model()`이 stub
2. **[BLOCKER]** Gemini adapter 완전 부재 (M-1 미해결), `gadgetron-provider/src/`에 `gemini.rs` 없음
3. **[BLOCKER]** `PortAllocator` 트레이트 미정의 (M-3 미해결) → vLLM=8000, TGI=3000 등 하드코딩, 노드당 엔진당 1개 제한

### 6.4 gpu-scheduler-lead
1. **[BLOCKER]** NodeAgent ↔ Scheduler VRAM 동기화 프로토콜 미설계 → stale 데이터로 OOM 배치 가능
2. **[BLOCKER]** `MigManager::enable_profile()` / `ThermalController::should_throttle()` 전부 `todo!()` → MIG 파티션 제약 무시
3. **[HIGH]** Cost-based eviction의 `$/token` 데이터 경로 없음 → scheduler가 billing system 참조 불가, 우선순위 계산 불가

### 6.5 xaas-platform-lead
1. **[BLOCKER]** `docs/modules/xaas-platform.md` 전체가 SQLite syntax (`AUTOINCREMENT`, `datetime('now')`, `REAL`) → D-4·D-8·D-9 동시 위반, 대규모 문서 재작성 필요
2. **[BLOCKER]** API 키 prefix가 여전히 `gdt_` → D-11 위반
3. **[HIGH]** Quota enforcement point 미정 (pre-request vs post-inference) → 테넌트가 post-response까지 quota 우회 가능

### 6.6 devops-sre-lead
1. **[BLOCKER]** `routes.rs`의 `health()`가 항상 `{"status":"ok"}` 반환 → K8s readinessProbe 무의미, 모든 backend 다운 시에도 traffic 유입
2. **[BLOCKER]** `axum::serve()`에 `with_graceful_shutdown` 없음 → SIGTERM 시 SSE 강제 종료, 진행 중 요청 손실
3. **[HIGH]** Prometheus exporter / OpenTelemetry span propagation 0% → 프로덕션 장애 추적 불가

### 6.7 ux-interface-lead
1. **[BLOCKER]** Shared types (`GpuMetrics`, `ModelStatus`, `RequestEntry`, `ClusterHealth`, `WsMessage`) 코드에 없음 (문서 스펙만 존재)
2. **[BLOCKER]** WebSocket `/api/v1/ws/metrics` 엔드포인트 gateway에 없음 (Round 1 M-2 미해결)
3. **[HIGH]** TUI 완전 스텁 상태 — `ui.rs`가 레이아웃 스켈레톤만, 데이터 렌더링/키 바인딩 0%

### 6.8 qa-test-architect
1. **[BLOCKER]** Mock LLM provider 부재 → 모든 테스트가 실제 API 호출 의존, CI에서 격리 불가
2. **[BLOCKER]** Zero integration test harness → cross-module 검증 불가, `router + scheduler + gateway` 통합 시나리오 없음
3. **[HIGH]** 5개 모듈 doc 전원이 "테스트 전략" 섹션 없음 → Round 2 testability review 전원 fail (Rubric §2 통과 0)

---

## 7. 핵심 관찰 (Cross-Cutting)

8명 전원 독립 리뷰에서 공통 지적된 패턴:

1. **설계-구현 갭이 극단적** — 모든 전문가가 "docs는 포괄적이지만 code는 스텁"을 독립적으로 관찰. 한두 모듈이 아니라 전체.
2. **확정 결정 반영 누락** — D-1~D-13 중 12개 미반영. Round 1 리뷰 후 설계 결정이 merge만 되고 구현 페이즈로 넘어가지 않음.
3. **테스트 인프라 0** — 7명이 testability를 간접 지적하고 qa-test-architect가 직접 지적. 어떤 모듈도 Rubric §2를 통과할 수 없음.
4. **XaaS가 유령 모듈** — 크레이트 자체가 없음에도 다른 모듈 설계 문서에 integration 지점이 언급됨 → 실제 연결 지점이 undefined.
5. **관측성 크로스컷 실종** — `tracing` 의존성만 있고 span/exporter/correlation_id 모두 누락. Phase 1 MVP가 운영 불가능한 상태.

---

## 8. 다음 단계 권고

사용자가 **D-20260411-01 ~ 05** 5건을 결정하면 다음 순서로 진행:

### Week 0 — 지금
- 사용자 결정 대기 (5건)
- 승인된 항목은 `docs/process/04-decision-log.md`로 이관

### Week 1 — 기초 타입 + 테스트 하네스 병렬 트랙
- **Track 1**: `chief-architect` → `gadgetron-core` 타입 12개 설계 문서 (`docs/design/core/types-consolidation.md`) Round 1/2/3 크로스 리뷰
- **Track 2**: `qa-test-architect` → `gadgetron-testing` 크레이트 설계 문서 (`docs/design/testing/mock-harness.md`) Round 1/2/3
- **Track 3**: `xaas-platform-lead` → `docs/modules/xaas-platform.md` 재작성 (PostgreSQL + i64 cents + `gad_`) Round 1/2/3

### Week 2
- Track 1 구현 (core 타입 12개)
- Track 2 구현 (MockLlmProvider, FakeGpuMonitor, FixtureBuilder)
- Track 3 구현 (xaas-platform 재작성 완료 + sqlx 마이그레이션)

### Week 3 — Phase 1 MVP 스펙 확정
- `gateway-router-lead` + `devops-sre-lead` 공동: D-6 MVP 설계 문서 (`docs/design/gateway/phase1-mvp.md`)
  - `with_graceful_shutdown`
  - 실제 health check (provider 연결 검증)
  - Bearer auth 미들웨어
  - `/v1/chat/completions` SSE 전체 파이프라인
  - RoundRobin + Fallback 2개 전략
- Round 1/2/3 리뷰

### Week 4 — Phase 1 MVP 구현
- D-6 MVP 구현 + 통합 테스트 (gadgetron-testing 활용)
- `inference-engine-lead`: OpenAI + Ollama 2 provider 완성, `PortAllocator` 트레이트 구현

### Week 5 — Scheduler + Observability
- `gpu-scheduler-lead`: 최소 scheduler (VRAM aware + LRU eviction + NVML 메트릭) 완성
- `devops-sre-lead`: Prometheus exporter + `request_id` 미들웨어 + tracing JSON 활성화

### Week 6-7 — 통합 검증
- e2e 시나리오: happy path + fallback + eviction
- 부하 테스트: P99 < 1ms overhead 검증 (criterion)
- `ux-interface-lead`: TUI 기초 (Phase 1 용 최소 기능)

### Week 8 — Phase 1 MVP 릴리즈

Phase 2+ 기능(Gemini, 6 strategy, XaaS billing, MIG, multi-node PP, Web UI)은 MVP 이후 별도 트랙.

---

## 9. 문서 상태

- **Phase**: Review Completed
- **다음 액션**: 사용자가 D-20260411-01~05 결정 → `docs/process/04-decision-log.md`에 승인 기록 → Week 1 착수
- **원본 리뷰**: 8명 전문가 병렬 리뷰 결과 (본 대화 로그에 포함)
- **연관 문서**: [`../process/04-decision-log.md`](../process/04-decision-log.md), [`round1-pm-review.md`](round1-pm-review.md), [`pm-decisions.md`](pm-decisions.md)
