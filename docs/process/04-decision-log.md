# 에스컬레이션 결정 로그

> 서브에이전트가 리뷰에서 해결 못한 문제 → PM이 취합 → 사용자에게 질의 → 승인 후 여기 기록.
> append-only. 과거 결정을 수정하려면 새 entry로 "supersedes" 표기.

---

## 엔트리 포맷

```markdown
## D-YYYYMMDD-NN: <제목>

- **발의자**: @<서브에이전트>
- **원문서**: docs/design/<path>.md
- **날짜**: YYYY-MM-DD
- **상태**: 🟡 사용자 승인 대기 | 🟢 승인 | 🔴 반려 | ♻️ supersedes D-...-XX

### 배경
문제 설명.

### 옵션
| 옵션 | 장점 | 단점 | PM 추천 |
|------|------|------|---------|
| A    | …    | …    | ✅      |
| B    | …    | …    |         |
| C    | …    | …    |         |

### PM 추천
옵션 A. 이유: …

### 사용자 결정
YYYY-MM-DD: 옵션 A 승인. 사유: …

### 영향 받는 문서/크레이트
- docs/design/…
- gadgetron-…
```

---

## 레거시 결정

`docs/reviews/pm-decisions.md`의 D-1 ~ D-13은 이미 확정된 결정이며 별도 엔트리 없이 유효.
변경하려면 새 D-entry로 supersedes 명시 + 사용자 승인 필수.

---

## 엔트리

## D-20260411-01: Phase 1 MVP 범위 재정의

- **발의자**: PM (8명 서브에이전트 합의)
- **원문서**: [`../reviews/round2-platform-review.md §5`](../reviews/round2-platform-review.md)
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 B — "일단")

### 배경
Round 2 전체 플랫폼 리뷰에서 8명 전원이 확인: 설계 문서는 포괄적이나 코드는 "Phase 0 스텁", D-1~D-13 결정 중 12개 미반영. 설계 문서 13,129줄 전체 범위를 8주 내 구현 불가능.

### 옵션
| 옵션 | 범위 | 기간 |
|------|------|:---:|
| A. 최소 | 2 provider + 2 strategy + 최소 scheduler + graceful shutdown + health + auth | 8주 |
| B. 중간 | A + 6 provider + 6 strategy + NUMA/MIG + 경량 XaaS + PostgreSQL + 기초 TUI | 12주 |
| C. 원설계 | 전체 | 24주+ |

### PM 추천
옵션 A. 이유: D-1~D-13 반영이 주 작업이고 핵심 파이프라인 확립 후 확장 비용이 낮음.

### 사용자 결정
**2026-04-11: 옵션 B 승인** (tentative "일단"). 12주 중간 집합으로 진행. 진행 중 필요 시 재조정 가능.

### Phase 1 포함 범위 (확정)
- **엔드포인트**: `/v1/chat/completions` SSE + `/v1/models` + `/health` + `/api/v1/{nodes, models, usage}`
- **Provider 6종**: OpenAI, Anthropic, Gemini, vLLM, SGLang, Ollama
- **Routing 6 strategy**: RoundRobin, CostOptimal, LatencyOptimal, QualityOptimal, Fallback, Weighted
- **Scheduler**: VRAM + LRU + NUMA + MIG 정적 enable/disable
- **XaaS**: 기본 API 키 + 기본 쿼터 (경량)
- **TUI**: 읽기 전용
- **보안**: Bearer + rustls + 레이트리밋 + 기본 가드레일
- **관측성**: tracing JSON + Prometheus + OpenTelemetry span propagation
- **DB**: PostgreSQL + sqlx (D-4, D-9)
- **배포**: Dockerfile + Helm chart

### Phase 2 연기
Web UI, K8s operator CRD, Slurm, Semantic/ML routing, full XaaS (billing engine, agent orchestration, tool-call bridge), ML PII guardrails, multi-region federation, multi-node Pipeline Parallelism, 열·전력 스로틀링, cost-based eviction 실데이터 연결

### 영향 받는 문서/크레이트
- `docs/modules/*.md` — 섹션별 `[P1]`/`[P2]` 태그 재정비 필요
- 신규 크레이트: `gadgetron-xaas`, `gadgetron-testing` (D-03, D-05)
- 의존성 추가: `sqlx 0.8`, `prometheus`, `opentelemetry`, `opentelemetry-otlp`

---

## D-20260411-02: Gemini 구현 전략

- **발의자**: @inference-engine-lead
- **원문서**: [`../reviews/round2-platform-review.md §5`](../reviews/round2-platform-review.md)
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 A)

### 배경
D-01 B로 Gemini가 Phase 1(12주) 포함 확정. 남은 질문은 "언제·어떻게". Gemini는 polling 기반, `parts[].functionCall` 포맷, character 과금 등 다른 5개 provider와 프로토콜이 이질적.

### 옵션
| 옵션 | 시점 | 방식 |
|------|:---:|------|
| A. 공통 normalizer 우선 | Week 5-7 | 5개 SSE → 공통 `SseToChunkNormalizer` 추출 (Round 2 HIGH #12 해소) → Gemini polling adapter 별도 |
| B. 병렬 트랙 | Week 2-8 | 메인 5개 + 서브 Gemini 독립 |
| C. 동시 착수 | Week 2-5 | 6개 전부 동시 |
| D. 후반 집중 | Week 10-11 | 다른 기능 안정화 후 |

### PM 추천
옵션 A. 이유: Round 2 HIGH #12 동시 해결, Gemini 이질성 격리, 리스크 타이밍 안전.

### 사용자 결정
**2026-04-11: 옵션 A 승인**. Week 5-7에 Gemini 작업.

### 구현 접근 (확정)
1. **Week 2-4**: OpenAI · Anthropic · Ollama · vLLM · SGLang 5개 adapter를 `LlmProvider` trait 기반 구현
2. **Week 5**: 공통 `SseToChunkNormalizer` 추출. 기존 5개 adapter 리팩터 (HIGH #12 해소)
3. **Week 6**: Gemini `PollingToEventAdapter` 레이어 설계. `generateContent`/`streamGenerateContent` 응답을 SSE-like event stream으로 캡슐화
4. **Week 7**: Gemini 고유 포맷 처리 — `parts[].functionCall` ↔ OpenAI `tool_calls`, character 과금 → token 환산

### 영향 받는 크레이트
- `gadgetron-provider`: 5 adapter → 공통 normalizer → Gemini adapter
- `gadgetron-core`: `ChatChunk` shape 검증 (Gemini polling 수용 가능 여부)

---

## D-20260411-03: `gadgetron-xaas` 크레이트 신설

- **발의자**: @xaas-platform-lead
- **원문서**: [`../reviews/round2-platform-review.md §5`](../reviews/round2-platform-review.md)
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 A)

### 배경
D-01 B에 "기본 API 키 + 기본 쿼터 (경량)" XaaS 포함. `gadgetron-xaas` 크레이트 미존재, D-12 크레이트 경계표에도 없음. Phase 2 full XaaS 확장 경로 고려.

### 옵션
| 옵션 | 구조 |
|------|------|
| A. 단일 `gadgetron-xaas` | Phase 1 경량 → Phase 2에 billing/agent/catalog 모듈 추가 |
| B. 3개 분리 | `gadgetron-auth` + `-tenant` + `-quota` |
| C. 기존 크레이트 흡수 | auth → gateway, tenant → core, quota → router |
| D. 하이브리드 | auth/quota 흡수 + tenant/audit 신규 |

### PM 추천
옵션 A. 이유: D-12 1회 업데이트, Phase 2 확장 자연스러움, 테스트 격리, 단일 책임.

### 사용자 결정
**2026-04-11: 옵션 A 승인**. 크레이트 이름 `gadgetron-xaas` 확정.

### Phase 1 내부 구조 (확정)
```
gadgetron-xaas/
└── src/
    ├── auth/{key,validator,middleware}.rs
    ├── tenant/{model,registry}.rs
    ├── quota/{config,enforcer,bucket}.rs
    ├── audit/{entry,writer}.rs
    └── db/migrations/        # sqlx PostgreSQL schema
```

### Phase 2 확장
`billing/`, `agent/`, `catalog/`, `gpuaas/` 모듈 추가 (크레이트 이동 없음)

### D-12 업데이트 필요
`gadgetron-xaas` 행 추가 — 담당 타입: `ApiKey`, `Tenant`, `TenantContext`, `QuotaConfig`, `QuotaEnforcer`, `AuditEntry`, `AuditWriter`

### 영향 받는 크레이트
- `gadgetron-gateway`: `gadgetron-xaas` 의존성 추가 (미들웨어 체인)
- `gadgetron-core`: `GadgetronError`에 D-13 5개 variant (`Billing`, `TenantNotFound`, `QuotaExceeded`, `DownloadFailed`, `HotSwapFailed`)
- `Cargo.toml` workspace members

---

## D-20260411-04: Phase 1 GPU 기능 범위

- **발의자**: @gpu-scheduler-lead
- **원문서**: [`../reviews/round2-platform-review.md §5`](../reviews/round2-platform-review.md)
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 B — D-01 B로 자동 해소)

### 배경
`MigManager` · `ThermalController` · multi-node PP 모두 `todo!()`. Phase 1 범위 결정 필요.

### 옵션
| 옵션 | 범위 |
|------|------|
| A. 최소 | VRAM + LRU + NVML |
| B. 중간 | A + NUMA + MIG enable/disable |
| C. 전체 | B + 열·전력 + multi-node PP + cost-based |

### 사용자 결정
**2026-04-11: 옵션 B 수용** ("일단") — D-01 B에 의해 자동 해소.

### Phase 1 포함 (확정)
- VRAM 추정 (`weights + overhead + kv_cache`) + `estimate_vram_mb()`
- LRU eviction + First-Fit Decreasing (GPU ≤ 90%)
- NVML 메트릭 수집 (feature gate)
- NUMA 토폴로지 (`/sys/bus/pci/devices/*/numa_node`)
- NVLink 그룹 탐지 (union-find)
- `ParallelismConfig` {tp_size, pp_size, ep_size, dp_size, numa_bind} (D-1)
- MIG **정적** enable/disable (A100/H100 프로파일 1g.5gb ~ 7g.80gb)
- 4-variant `EvictionPolicy` (D-2)

### Phase 2 연기
- `ThermalController` 열/전력 기반 스로틀링
- Multi-node Pipeline Parallelism planner
- Cost-based eviction의 `$/token` 데이터 실연결 (Phase 1은 구조만)
- MIG **동적** 재구성
- AMD ROCm / Intel Gaudi 벤더 추상화

### gpu-scheduler-lead 설계 문서 필수 BLOCKER 섹션
`docs/design/scheduler/phase1.md`에 다음 4개 섹션 반드시 포함:
1. Scheduler ↔ NodeAgent VRAM 동기화 프로토콜 (push/pull, heartbeat 주기, stale threshold)
2. `MigManager::enable_profile()` / `destroy_instance()` 기본 구현
3. 부팅 시 NVML 부재 graceful degradation (CPU-only 모드)
4. `ModelState::Running{port, pid}` 패치 (D-10 즉시 적용)

### 영향 받는 크레이트
- `gadgetron-scheduler`: 4개 BLOCKER 해결
- `gadgetron-node`: `MigManager` 기본 구현, VRAM 동기화 프로토콜
- `gadgetron-core`: `ModelState::Running{pid}`, `ParallelismConfig`, `NumaTopology`, `EvictionPolicy` (D-1, D-2, D-3, D-10)

---

## D-20260411-05: `gadgetron-testing` 크레이트 신설

- **발의자**: @qa-test-architect
- **원문서**: [`../reviews/round2-platform-review.md §5`](../reviews/round2-platform-review.md)
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 A)

### 배경
8개 crate 전원 `tests/` 0개, mock/fake 0개, CI workflow 0개. Round 2 testability review 전원 fail. D-01 B로 테스트 surface 확대 (6 provider + 6 strategy + PostgreSQL + 경량 멀티테넌트 + NUMA/MIG).

### 옵션
| 옵션 | 구조 |
|------|------|
| A. 단일 `gadgetron-testing` | 모든 mock/fake/fixture/harness 집중, dev-dep으로 import |
| B. 각 crate 자체 모듈 | `pub mod testing` |
| C. `#[cfg(test)]` 내장 | test-only private 모듈 |

### PM 추천
옵션 A. 이유: Cross-crate 통합 테스트 home 필수, 의존성 단방향, qa-test-architect 단일 소유, Round 2 BLOCKER #6 직접 해소.

### 사용자 결정
**2026-04-11: 옵션 A 승인**. 크레이트 이름 `gadgetron-testing` 확정.

### Phase 1 내부 구조 (확정)
```
gadgetron-testing/
├── Cargo.toml                          # dev-dependency only
└── src/
    ├── mocks/
    │   ├── provider/{openai,anthropic,gemini,ollama,vllm,sglang,failing}.rs
    │   ├── node/{fake_node,fake_gpu_monitor}.rs
    │   └── xaas/{fake_tenant,fake_quota,fake_audit}.rs
    ├── fixtures/{config,chat_request,node_status,deployment,tenant}.rs
    ├── harness/{gateway,scheduler,e2e,pg}.rs
    ├── props/{vram,eviction,bin_packing,routing}.rs
    └── snapshots/                      # insta baselines
```

### 의존성
- `mockito`, `wiremock` (HTTP mock)
- `testcontainers 0.23` (PostgreSQL)
- `proptest 1` (property-based)
- `insta 1` (snapshot)
- workspace 다른 크레이트를 `dev-dependencies`로 참조

### 통합 테스트 위치
- **Unit**: 각 crate `src/` 내 `#[cfg(test)]`, `gadgetron-testing`을 dev-dep으로 import
- **Cross-crate integration**: `gadgetron-testing/tests/` (예: `tests/fallback_chain.rs`, `tests/eviction_e2e.rs`)
- **Benchmarks**: 각 crate `benches/` (criterion) + harness 재사용

### D-12 업데이트
`gadgetron-testing` 행 추가 — Phase 1 필수 크레이트

### 영향 받는 크레이트
- 모든 8개 crate: `[dev-dependencies]`에 `gadgetron-testing` 추가
- `Cargo.toml` workspace members: `gadgetron-xaas`, `gadgetron-testing` 추가
- CI: `.github/workflows/` 실제 workflow yaml 생성

---

## D-20260411-06: f64 vs i64 cents 경계 (Q-A)

- **발의자**: @chief-architect (Track 1) + @xaas-platform-lead (Track 3) 충돌
- **원문서**: `docs/design/core/types-consolidation.md` Q-1 + `docs/design/xaas/phase1.md`
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 A)

### 배경
Track 1 (chief-architect)은 `gadgetron-router`의 비용 hint(`CostEntry`, `RoutingDecision::estimated_cost_usd`, `ProviderMetrics::total_cost_usd`)를 f64 유지 권고 — routing은 "대략 저렴한 것" 선택 hot path, billable 금액 아님. Track 3 (xaas-platform-lead)은 모든 과금 경로에 i64 cents 적용 (D-8 엄격 해석). 두 권고의 공존 가능 여부 및 경계 지점 결정 필요.

### 옵션
| 옵션 | 내용 |
|------|------|
| A. Routing f64, Billing i64 cents | Track 1 유지, Track 3 유지, 변환 지점 1개 |
| B. 전부 i64 cents | routing도 i64 cents 통일, 변환 없음 |
| C. 전부 f64 | 현 상태 유지, D-8 위반 |

### PM 추천
옵션 A. 이유: routing 비교는 O(n) hot path이므로 f64 연산이 i64보다 약간 빠르고, "대략 저렴한 것" 선택만 필요. Billing은 정확한 cents 필수. 변환 지점은 `QuotaEnforcer::record_post() → audit_log.cost_cents` 1회만.

### 사용자 결정
**2026-04-11: 옵션 A 승인**

### 적용 지점
- `gadgetron-router/src/metrics.rs`: `CostEntry`, `RoutingDecision::estimated_cost_usd`, `ProviderMetrics::total_cost_usd` → **f64 유지**
- `gadgetron-xaas/src/audit/entry.rs`: `AuditEntry.cost_cents: i64`
- `gadgetron-xaas/src/db/migrations/`: `audit_log.cost_cents BIGINT NOT NULL`
- 변환 함수: `gadgetron-xaas/src/quota/enforcer.rs::compute_cost_cents(usage: &Usage, rate: &CostEntry) -> i64` (유일한 변환 지점)

### 영향 받는 문서/크레이트
- `docs/design/core/types-consolidation.md`: Q-1 closed (옵션 A + 사유)
- `docs/design/xaas/phase1.md`: 변환 함수 스펙 추가
- `gadgetron-router`, `gadgetron-xaas`, `gadgetron-core`

---

## D-20260411-07: 공유 UI 타입 위치 (Q-B)

- **발의자**: @ux-interface-lead (Round 2 BLOCKER #8) + @chief-architect (Track 1 Q-3 scope 밖)
- **원문서**: `docs/reviews/round2-platform-review.md §6.7` + `docs/design/core/types-consolidation.md` Q-3
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 A)

### 배경
TUI와 Web UI가 공유할 타입(`GpuMetrics`, `ModelStatus`, `RequestEntry`, `ClusterHealth`, `WsMessage`)이 코드에 없음 — Round 2 BLOCKER #8. Track 1은 "scope 밖"으로 Q-3 flag했으나 PM 결정 필요.

### 옵션
| 옵션 | 내용 |
|------|------|
| A. gadgetron-core | 기존 cross-crate 공유 타입 집합에 `ui` 모듈로 추가 |
| B. 새 gadgetron-ui-types 크레이트 | UI 전용 공유 crate 분리 |
| C. 각 UI crate에 duplicate | 중복 허용 |

### PM 추천
옵션 A. 이유: `gadgetron-core`가 이미 cross-crate 공유 타입의 집합 (node, provider, routing). UI 타입도 동일 패턴. 새 crate 분리는 Phase 3 Web UI 확장 시 고려. Round 2 BLOCKER #8 직접 해소.

### 사용자 결정
**2026-04-11: 옵션 A 승인** — gadgetron-core에 `ui` 모듈로 추가

### 적용 지점
- `gadgetron-core/src/ui.rs` (신규 모듈):
  - `GpuMetrics` — `node_id, gpu_index, vram_used_mb, vram_total_mb, utilization_pct, temperature_c, power_w, power_limit_w, clock_mhz, fan_rpm`
  - `ModelStatus` — `model_id, status, engine, version, node_id, vram_used_mb, loaded_at`
  - `RequestEntry` — `request_id, timestamp, model, provider, status, latency_ms, prompt_tokens, completion_tokens, total_tokens, routing_decision`
  - `ClusterHealth` — `status, total_nodes, online_nodes, total_gpus, active_gpus, running_models, requests_per_minute, error_rate_pct, cost_per_hour_usd, alerts`
  - `WsMessage` enum — `GpuMetrics | ModelStatus | RequestLog | ClusterHealth`
- 필드 명세: `docs/ui-ux/dashboard.md §8.3` 참조

### 영향 받는 문서/크레이트
- `docs/design/core/types-consolidation.md`: Section "E1. Shared UI Types" 추가 (Track 1 scope 확장)
- `docs/design/testing/harness.md`: `FakeGpuMonitor`가 `GpuMetrics` 반환 — GpuMonitor trait 반환 타입 정렬 확인
- `gadgetron-core`, `gadgetron-tui`, (향후) `gadgetron-web`

---

## D-20260411-08: `GadgetronError::StreamInterrupted` variant 추가

- **발의자**: @inference-engine-lead (Round 1 Track 1 리뷰)
- **원문서**: `docs/reviews/round1-week1-results.md` + `docs/design/core/types-consolidation.md`
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 A, PM 재량)

### 배경
현재 Track 1 설계는 SSE 스트림 중단(client abort, network timeout, HTTP/1.1 idle)을 generic `GadgetronError::Provider(String)`으로 처리. Round 1에서 inference-engine-lead가 지적: 10+ year LLM serving 경험상 streaming interruption은 production 장애의 주요 원인. 명시적 variant로 구분해야 디버깅·메트릭·알림·retry 정책이 모두 가능. Round 2 HIGH #10 해결 필수.

### 옵션
| 옵션 | 내용 |
|------|------|
| A. 추가 | `StreamInterrupted { reason: String }` variant 신설 |
| B. 생략 | `Provider(String)` + tracing field |
| C. Phase 2 연기 | Round 2 retry 시 재결정 |

### PM 추천
옵션 A. 이유: streaming은 Phase 1 MVP 핵심 UX. Variant 구분으로 `tower::retry`, Prometheus 라벨링, circuit breaker 분리 등 구현 지점 명확.

### 사용자 결정
**2026-04-11: 옵션 A 승인 (PM 재량)**

### 적용 지점
- `gadgetron-core/src/error.rs`: `StreamInterrupted { reason: String }` 6번째 추가 variant
- `docs/design/core/types-consolidation.md`: §2.1.2 `GadgetronError` 확장, retry policy 표 업데이트
- `gadgetron-provider/src/`: 각 adapter의 `chat_stream` 에러 경로에서 network error/timeout/client abort 구분 매핑
- `gadgetron-gateway/src/sse.rs`: SSE 스트림 중단 감지 시 `StreamInterrupted` 매핑
- Prometheus 라벨: `error_kind="stream_interrupted"` vs `error_kind="provider"`

### Retry 정책
- `StreamInterrupted` → idempotent stream 1회 재시도 (tower::retry layer)
- `Provider` → 재시도 없음 (4xx/5xx 별도)

### 영향 받는 문서/크레이트
- `docs/design/core/types-consolidation.md` (Track 1 retry)
- `gadgetron-core`, `gadgetron-provider`, `gadgetron-gateway`

---

## D-20260411-09: 감사 로그 drop 정책 (Compliance)

- **발의자**: @devops-sre-lead (Round 1 Track 3 리뷰)
- **원문서**: `docs/reviews/round1-week1-results.md` + `docs/design/xaas/phase1.md`
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 C, PM 재량)

### 배경
현재 Track 3 설계(`docs/design/xaas/phase1.md` §2.2.4)는 `AuditWriter` 채널 full 시 `try_send` 실패 → warning + drop. devops-sre-lead 지적: SOC2/HIPAA/GDPR 적용 환경에서 감사 엔트리 영구 손실은 법적 위험.

### 옵션
| 옵션 | 내용 | Latency | 법규 안전성 |
|------|------|:---:|:---:|
| A. Drop 허용 (현재) | WARN + 배포 전 review 주석 | 0 | ❌ |
| B. Backpressure | 채널 full 시 20-50ms 대기, 무손실 | +p99 20-50ms | ✅ |
| C. Drop + 버퍼 확대 + Phase 2 fallback | 1024→4096 + 경고 + Phase 2 WAL/S3 | 0 (정상) | ⚠️ Phase 2 |
| D. Phase 1부터 WAL 파일 fallback | 채널 full 시 local disk flush | 0 (정상) | ✅ |

### PM 추천
옵션 C. 이유: Phase 1 MVP 경량 유지 + 버퍼 확대로 현실적 drop 방지 + Phase 2 fallback 명시로 법적 위험 경로 명확. 옵션 D는 local disk I/O 추가 복잡도(파일 rotation, disk 관리)가 MVP scope 넘음.

### 사용자 결정
**2026-04-11: 옵션 C 승인 (PM 재량)**

### 적용 지점
- `gadgetron-xaas/src/audit/writer.rs`: `mpsc::channel(4096)` (기존 1024의 4배)
- `gadgetron-xaas/src/audit/writer.rs`: `try_send` 실패 시 `metrics::counter!("gadgetron_xaas_audit_dropped_total").increment(1)` + `tracing::warn!(tenant_id, request_id, "audit dropped")`
- `docs/design/xaas/phase1.md` §2.2.4: 정책 명시 + "Phase 2 S3/Kafka fallback 예정" 주석
- `docs/modules/xaas-platform.md` Phase 2 섹션: WAL/S3 fallback 설계 할 일로 기재
- 배포 가이드 (향후 `deployment-operations.md`): "프로덕션 배포 전 compliance review 필수" 게이트 주석

### Phase 2 계획 (명시화)
- `gadgetron-xaas/src/audit/fallback.rs` (Phase 2): WAL file 또는 S3 batch upload
- SIGTERM 시 channel drain + fallback flush (5초 timeout)
- Compliance 체크리스트: retention (90일), anonymization (30일), 검색 indexing

### 영향 받는 문서/크레이트
- `docs/design/xaas/phase1.md` (Track 3 retry)
- `docs/modules/xaas-platform.md` Phase 2 섹션 보강
- `gadgetron-xaas/src/audit/`

---

## D-20260411-10: `Scope` enum Phase 1 정책

- **발의자**: @gateway-router-lead (Round 1 Track 3 리뷰)
- **원문서**: `docs/reviews/round1-week1-results.md` + `docs/design/xaas/phase1.md`
- **날짜**: 2026-04-11
- **상태**: 🟢 승인 (옵션 A, PM 재량)

### 배경
D-7이 경로 네임스페이스(`/v1/*` OpenAI 호환, `/api/v1/*` 관리, `/api/v1/xaas/*` XaaS)를 정의했으나, Track 3 Phase 1 설계는 `Scope` enum을 빈 enum(Phase 2 확장)으로 둠. Round 1에서 gateway-router-lead 지적: "이 상태로는 route guard 구현 불가능".

### 옵션
| 옵션 | 내용 |
|------|------|
| A. 최소 3개 | `Scope::{OpenAiCompat, Management, XaasAdmin}` |
| B. Phase 1 skip | 경로 guard 전부 허용 |
| C. master-only | `/api/v1/*`는 master 전용, 그 외 모두 통과 |

### PM 추천
옵션 A. 이유: 3개 scope만으로 D-7 강제 가능. `#[non_exhaustive]` 속성으로 Phase 2에 기존 3개 세분화가 breaking change 없이 가능.

### 사용자 결정
**2026-04-11: 옵션 A 승인 (PM 재량)**

### 적용 지점
- `gadgetron-xaas/src/auth/key.rs`:
  ```rust
  #[non_exhaustive]
  pub enum Scope {
      OpenAiCompat,  // /v1/* - chat/completions, models
      Management,    // /api/v1/{nodes, models, usage, costs}
      XaasAdmin,     // /api/v1/xaas/* - tenant/quota/agent ops
  }
  ```
- `gadgetron-xaas/src/auth/validator.rs`: `ValidatedKey.scopes: Vec<Scope>` (1+ scope)
- `gadgetron-gateway/src/middleware/auth.rs`: `fn require_scope(ctx: &TenantContext, required: Scope) -> Result<(), StatusCode>` helper
- `gadgetron-xaas/src/db/migrations/`: `api_keys.scopes TEXT[]` (PostgreSQL array)
- **키 발급 기본 scope**:
  - `gad_live_*` → `[OpenAiCompat]`
  - `gad_test_*` → `[OpenAiCompat]`
  - `gad_vk_*` → `[OpenAiCompat]` (tenant 범위 내)
  - `Management`/`XaasAdmin` scope은 별도 부여 필요

### Phase 2 확장 경로
`Scope::OpenAiCompat` → `{ChatRead, ChatWrite, ModelsList, EmbeddingsRead, ...}` 세분화. `Scope::XaasAdmin` → `{XaasGpuAllocate, XaasModelDeploy, XaasAgentCreate, ...}`. `#[non_exhaustive]` 덕분에 기존 3개를 보존하며 추가.

### 영향 받는 문서/크레이트
- `docs/design/xaas/phase1.md` (Track 3 retry, A1)
- `docs/modules/xaas-platform.md` Phase 2 scope 확장 섹션
- `gadgetron-xaas/src/auth/`, `gadgetron-gateway/src/middleware/`

---

## D-20260411-11: `Arc<dyn LlmProvider>` 유지 + 근거 문서화

- **발의자**: @chief-architect (Track 2 Round 3 리뷰)
- **원문서**: docs/design/testing/harness.md + docs/design/core/types-consolidation.md
- **날짜**: 2026-04-12
- **상태**: 🟢 승인 (옵션 A, PM 재량)

### 배경
Round 3 리뷰에서 chief-architect가 `Arc<dyn LlmProvider>` hot path 비용 분석 부재를 지적. Mock + 실제 provider 모두 trait object 사용 중. Router loop/fallback chain에서 vtable lookup 비용 근거 필요.

### 옵션
| 옵션 | 내용 |
|------|------|
| A. 현재 유지 + 근거 문서화 | dyn dispatch 유지, rationale 추가 |
| B. Generic 전환 | `Router<P: LlmProvider>` workspace-wide refactor |
| C. Hybrid | Hot path generic, mock test dyn |

### 사용자 결정
**2026-04-12: 옵션 A 승인 (PM 재량)**

### 이유
LLM 서빙은 I/O-bound (HTTP provider 호출 100ms+). Vtable dispatch 1ns vs network 100ms = **10^8배 격차**로 무시 가능. Config-driven 6-provider runtime polymorphism은 static generic으로 표현 불가 (runtime 설정 파일에서 로드). Generic 전환은 workspace-wide refactor 리스크 크고 보상 작음.

### 적용 지점
- `docs/design/core/types-consolidation.md` §2.1.10: "Dynamic Dispatch Rationale" 서브섹션 + 성능 math
- `docs/design/testing/harness.md` §2.3 또는 §2.9.1: Mock provider도 같은 패턴 인용
- `gadgetron-core/src/provider.rs`: LlmProvider trait 위 rustdoc `///` 주석 (구현 시점)

### 영향 받는 문서
- docs/design/core/types-consolidation.md (Track 1 R3 retry)
- docs/design/testing/harness.md (Track 2 R3 retry)

---

## D-20260411-12: Phase 1 `PgKeyValidator` LRU 캐시 추가 (moka)

- **발의자**: @chief-architect (Track 3 Round 3 리뷰)
- **원문서**: docs/design/xaas/phase1.md §2.2.2
- **날짜**: 2026-04-12
- **상태**: 🟢 승인 (옵션 A, PM 재량)

### 배경
현재 설계는 매 요청마다 `PgKeyValidator::validate()` → SHA-256 해시 + PostgreSQL 쿼리 (3-5ms). Phase 1 SLO "P99 < 1ms gateway overhead"와 직접 충돌. 기존 설계는 "Phase 2 LRU cache 추가" 계획이었으나 chief-architect는 "Phase 1에 필수" 주장.

### 옵션
| 옵션 | 내용 | SLO 준수 |
|------|------|:---:|
| A. Phase 1 LRU cache (moka) | 10k entries, 10min TTL, ~100 LOC | ✅ |
| B. SLO 완화 문서화 | Phase 1 auth 5-12ms 허용 | ❌ |
| C. Hybrid | master key만 캐시 | ⚠️ |

### 사용자 결정
**2026-04-12: 옵션 A 승인 (PM 재량)**

### 이유
auth는 모든 요청의 hot path. DB hit 3-5ms는 sub-ms SLO 근본 위반. `moka::future::Cache` 의존성 1개, ~100 LOC, 복잡도 낮음. Phase 2로 미루면 초기 운영 시 성능 핫픽스 필요.

### 적용 지점
- `gadgetron-xaas/src/auth/validator.rs`: `PgKeyValidator`에 `moka::future::Cache<String, ValidatedKey>` 필드 추가
- `gadgetron-xaas/Cargo.toml`: `moka = { version = "0.12", features = ["future"] }`
- 설정: 10,000 entries, 10분 TTL, 키 = SHA-256 hash
- 무효화: revocation 시 수동 trigger (`invalidate_key`) + 자연 TTL 만료
- SLO 재기술: "Phase 1 auth p99 < 1ms (cache hit 99%), p99 < 8ms (cache miss 1%, cold start)"
- Phase 2: Redis 분산 캐시 고려 (multi-instance deployment 시)

### 영향 받는 문서
- docs/design/xaas/phase1.md §2.2.2, §2.4 (Track 3 R3 retry)

---

## D-20260411-13: `GadgetronError::Database` variant 추가 (D-13 확장)

- **발의자**: @chief-architect (Track 3 Round 3 리뷰)
- **원문서**: docs/design/xaas/phase1.md A9 + docs/design/core/types-consolidation.md
- **날짜**: 2026-04-12
- **상태**: 🟢 승인 (옵션 A, PM 재량)

### 배경
현재 `sqlx::Error`를 모두 `GadgetronError::Config(String)`으로 collapse. Production debug 시 원인(pool timeout vs row not found vs connection failed vs constraint) 추적 불가. Prometheus 라벨링 불가. Retry 정책 구분 불가. Round 1에서 A9 non-blocking 처리했으나 Round 3에서 chief-architect가 "지금 결정 필요" 재주장.

### 옵션
| 옵션 | 내용 |
|------|------|
| A. D-13 확장: Database variant + DatabaseErrorKind enum | Leaf crate 보존 (sqlx 의존성 없이) |
| B. From impl wrapper | `Config(String)` 유지 + 매핑만 |
| C. Phase 2 연기 | 현상 유지 |

### 사용자 결정
**2026-04-12: 옵션 A 승인 (PM 재량)**

### 중요 제약: `gadgetron-core` Leaf Crate 보존

`gadgetron-core`에 `sqlx` 의존성 추가 **금지** (D-12 leaf crate 원칙). DB-agnostic `DatabaseErrorKind` enum 사용:

```rust
// gadgetron-core/src/error.rs
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseErrorKind {
    RowNotFound,
    PoolTimeout,
    ConnectionFailed,
    QueryFailed,
    MigrationFailed,
    Constraint,
    Other,
}

// GadgetronError에 추가:
#[error("Database error ({kind:?}): {message}")]
Database { kind: DatabaseErrorKind, message: String },
```

Consumer crate (xaas)는 helper function으로 매핑 (orphan rule 준수):

```rust
// gadgetron-xaas/src/error.rs (not core!)
pub(crate) fn sqlx_to_gadgetron(e: sqlx::Error) -> GadgetronError {
    let kind = match &e {
        sqlx::Error::RowNotFound => DatabaseErrorKind::RowNotFound,
        sqlx::Error::PoolTimedOut => DatabaseErrorKind::PoolTimeout,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => DatabaseErrorKind::ConnectionFailed,
        sqlx::Error::Database(_) => DatabaseErrorKind::Constraint,
        sqlx::Error::Migrate(_) => DatabaseErrorKind::MigrationFailed,
        _ => DatabaseErrorKind::Other,
    };
    GadgetronError::Database { kind, message: e.to_string() }
}

// 사용: `.await.map_err(sqlx_to_gadgetron)?`
```

### 적용 지점
- `gadgetron-core/src/error.rs`: `DatabaseErrorKind` enum (`#[non_exhaustive]`) + `Database` variant. Sqlx 의존성 없음.
- `gadgetron-xaas/src/error.rs`: `sqlx_to_gadgetron` helper function
- `docs/design/core/types-consolidation.md` §2.1.2: GadgetronError 확장 + retry policy 표 ("Database { PoolTimeout }" → 1회 retry, others → no retry) + Prometheus 라벨 (`error_kind="db_pool_timeout"`, `db_row_not_found`, ...)
- `docs/design/xaas/phase1.md` §2.4.1: helper function 명시 + 모든 `.await?` 사이트에 `.map_err(sqlx_to_gadgetron)?`
- `docs/design/xaas/phase1.md` §2.4.2 HTTP Status Mapping 확장:
  - `Database { kind: PoolTimeout }` → 503
  - `Database { kind: RowNotFound }` → 404
  - `Database { kind: ConnectionFailed }` → 503
  - `Database { kind: Constraint }` → 409
  - `Database { kind: MigrationFailed }` → 500
  - `Database { kind: QueryFailed/Other }` → 500

### 영향 받는 문서/크레이트
- docs/design/core/types-consolidation.md (Track 1 R3 retry)
- docs/design/xaas/phase1.md (Track 3 R3 retry)
- gadgetron-core, gadgetron-xaas

---

## D-20260412-01: 서브에이전트 팀 8명 → 10명 확장 (security-compliance-lead + dx-product-lead)

**날짜**: 2026-04-12
**유형**: Strategic (사용자 escalation → 옵션 A 채택)
**관련**: AGENTS.md 핵심 규칙 2·3 (10년+ 전문가 / 동적 팀 편성)

### 배경
사용자 지적: "서브에이전트중에 보안관련도 한명 있어야 하지 않을까? 유저의 사용성 설계 관점을 최우선으로 보는 서브에이전트는 있나?"

기존 8명 매핑 결과:
- **보안 전담 부재**: TLS/auth는 devops-sre가, API key/tenant는 xaas-platform이 분산 보유. Threat modeling, OWASP, secret rotation, 공급망 (cargo-audit/SBOM), prompt injection, 컴플라이언스 매핑 (SOC2/GDPR/HIPAA) 전담 검토자 없음.
- **사용성 전담 부재**: ux-interface-lead는 **TUI/Web 위젯 구현** (Ratatui/React) 중심. Developer/Operator UX (CLI 사용성, error message 친절도, API 응답 형식, config 발견성, 문서 IA, 운영자 workflow) 관점 부재.

### 옵션
| 옵션 | 내용 |
|------|------|
| A. 2명 신규 추가 → 10명 | security-compliance-lead + dx-product-lead 모두 신설. Round 1.5 신설. |
| B. 1명 (보안)만 신설 → 9명 | UX는 ux-interface-lead가 scope 확장 |
| C. 둘 다 기존 scope 확장 → 8명 유지 | 명시화만 |

### PM 추천: 옵션 A
근거:
1. 보안 + DX 둘 다 cross-cutting → 한 에이전트에 묶으면 review pass 될 위험
2. Round 1.5에 두 관점 정식 포함되어야 빠짐 없음
3. 10명도 PM 관리 가능 범위
4. v0 platform-architecture 재검토 시 두 관점에서 추가 gap 발견 기대

### 사용자 결정
**2026-04-12: 옵션 A 승인 + 추가 지시 — "너무 많으면 중복/불필요한 롤 통합/제거"**

### PM 후속 점검: 통합 대상 없음
10명 책임 매핑 결과:
- 통합 가능 후보 0건. 각 에이전트 책임 명확.
- 책임 경계 3쌍 명확화 필요 (RACI):
  - 보안 영역: devops-sre (구현) / xaas-platform (구현) / security-compliance (검토 + 정책)
  - UX 영역: ux-interface (위젯 구현) / dx-product (텍스트 + 워크플로우)
- 4주 운영 후 PM이 실제 발견 사례 기반 재평가 (통합/scope 조정 여지)

### 적용 지점
- `docs/agents/security-compliance-lead.md` (canonical 신규)
- `docs/agents/dx-product-lead.md` (canonical 신규)
- `.claude/agents/security-compliance-lead.md` (Claude Code adapter 신규)
- `.claude/agents/dx-product-lead.md` (Claude Code adapter 신규)
- `AGENTS.md`: "8명 → 10명" 표기 갱신
- `docs/process/00-agent-roster.md`: 9·10번 섹션 추가, 조직도 확장, 책임 경계 RACI, 변경 이력 append
- `docs/process/03-review-rubric.md`: §1.5 신규 (Round 1.5 — 보안 + 사용성, 20개 체크리스트)

### 신규 Round 구조
| Round | 담당 | 목적 |
|---|---|---|
| 1 | PM 선정 도메인 lead 2명 | 도메인 정합성 (인터페이스·크레이트 경계·타입) |
| **1.5 (신규)** | security-compliance-lead + dx-product-lead 병렬 | 보안 위협 + 사용자 사용성 |
| 2 | qa-test-architect | 테스트 가능성 |
| 3 | chief-architect | Rust 관용구 + 아키텍처 일관성 |

### 운영 제약 (Claude Code 한정)
`.claude/agents/*.md`는 세션 시작 시만 로드 (memory: feedback_claude_code_subagent_hot_reload). 본 결정 적용 후 새 에이전트 사용을 위해 **세션 재시작 필요**.

---

## D-20260412-02: 모든 설계 문서는 구현 결정론(implementation determinism) 보장

**날짜**: 2026-04-12
**유형**: Process / Quality bar (사용자 직접 지시)

### 배경
사용자 지시: "그 어느 누가 와서 코딩을 해도 같은 결과가 나오도록 계획이 구체적이고 빈틈이 없어야합니다. 명심해주세요."

본 프로젝트는 PM + 10 subagent + 도구 독립 (Claude Code / Codex / Cursor / OpenCode) 체제. 즉 **다른 도구·다른 세션·다른 사람**이 같은 design doc을 보고도 동일한 코드를 산출해야 한다.

### 결정
모든 design doc은 **구현 결정론**을 보장한다:

**필수 명시 항목 (10가지)**:
1. 모든 type signature 완성 (제네릭 bound, lifetime, async/sync, Send/Sync)
2. 에러 처리 결정 (`GadgetronError` variant, `From` impl, retry 가능 정보 전달)
3. 동시성 모델 결정 (`Arc<Mutex>` vs `RwLock` vs 채널 vs lock-free, 근거 1줄)
4. 라이브러리 선택 결정 (선택 + 근거)
5. 모든 enum variant 열거 (`#[non_exhaustive]` 여부, `Display`/`source` 처리)
6. 모든 async 결정 (`spawn` vs `spawn_blocking` vs `JoinSet` vs `select!`, cancellation 안전성)
7. 모든 magic number 명시 (timeout, cache size, eviction policy, 근거 + 변경 가능성)
8. 외부 의존성 contract (HTTP endpoint, 응답 형식, 에러 시 동작, retry 정책)
9. 상태 전이 표 (state × event → next state)
10. 테스트 케이스 구체 입력/기대 출력

**금지 표현 (red flags)**:
- TBD / to be decided / later / TODO (명시적 phase 분리 제외)
- "적절한", "효율적인", "최적화된" (구체 수치 없음)
- "비슷하게 처리한다", "기존 패턴을 따른다" (참조 라인 없음)
- "여러 옵션이 있다" (선택 결과 없음)
- "필요에 따라", "유연하게"
- "..." 또는 "기타 등등"으로 끝나는 enum/list

### 적용 지점
- 모든 design doc 작성·리뷰 시 위 10가지 체크리스트 적용
- Round 1.5 dx-product-lead가 "모호함 발견" 시 reject (사용성 = 다음 코더의 사용성)
- Round 3 chief-architect가 "구현 결정론" 최종 게이트
- PM 자율 결정도 모두 본 decision log에 기록 (왜 이렇게? 추적 가능)
- 모호한 채로 통과시키느니 한 라운드 더 도는 것이 항상 저렴
- `docs/process/03-review-rubric.md`에 "구현 결정론" 항목 §1·§3에 강화 포함 (후속 작업)

### 영향 받는 문서
- `docs/process/03-review-rubric.md` (§1, §3 강화 예정)
- `docs/architecture/platform-architecture.md` v1 (chief-architect 작성 시 본 결정 준수 강제)
- 모든 `docs/design/**/*.md` (재검토 대상)

---

_(다음 엔트리는 아래에 append)_
