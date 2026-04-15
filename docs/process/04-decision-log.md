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

## D-20260414-01: Phase-aligned 워크스페이스 버저닝 정책

**날짜**: 2026-04-14
**유형**: Process / Release (사용자 직접 지시)
**상태**: 🟢 승인

### 배경

프로젝트는 `0.1.0` 워크스페이스 버전으로 시작했으나 명시적 버저닝 정책이 없었다. Phase 1 은 완료되어 `v0.1.0-phase1` 태그가 존재하고 Phase 2 가 진행 중인데, 어느 시점에 어떤 버전으로 bump 할지 규칙이 없어 혼선이 발생한다. `CHANGELOG.md`, `VERSION` 파일, 릴리스 태그 규칙도 모두 공백 상태였다.

### 결정

**Phase N 동안 워크스페이스 버전은 `0.N.X` 라인을 유지한다.** 정식 출시(public launch) 직전에만 `1.0.0` 으로 bump 한다.

- Phase 1 = `0.1.X` (완료, tag `v0.1.0-phase1`)
- Phase 2 = `0.2.X` (현재)
- Phase 3 = `0.3.X` (예정)
- … = `0.N.X`
- 정식 출시 = `1.0.0`

모든 크레이트는 `version.workspace = true` 로 lockstep 버저닝을 유지한다. 0.x 구간에서는 SemVer 호환성 약속을 하지 않으며, phase 전환 시 breaking change 가 허용된다.

### 근거

- 사용자 지시: "phase1 = 0.1.X, phase2 = 0.2.X, ... 으로 관리하고 진짜 출시전에 1.0.0으로 명시합시다."
- 크레이트 경계(D-12) 가 고정되어 있지만 내부 타입 흐름이 밀접 → 독립 버저닝의 이득 없음
- Phase 자체가 이미 사실상의 major milestone 역할을 하고 있어 phase 번호와 minor version 을 일치시키면 릴리스 계획·태그·커밋·문서의 버전 참조가 모두 단일 축으로 정렬됨

### 즉시 적용 변경

1. `Cargo.toml:17` — `version = "0.1.0"` → `version = "0.2.0"` (Phase 2 진행 중)
2. `docs/00-overview.md:3` — 버전 헤더 갱신 + 정책 문서 링크
3. `docs/process/06-versioning-policy.md` — 정책 문서 신설

### 영향 받는 문서/크레이트

- `Cargo.toml` (workspace version)
- `docs/00-overview.md`
- `docs/process/06-versioning-policy.md` (신설)
- 10개 워크스페이스 크레이트 (version.workspace 상속으로 자동 반영)

---

## D-20260414-02: OpenWebUI 제거 → `gadgetron-web` 크레이트 자체 빌드 (assistant-ui 기반)

**날짜**: 2026-04-14
**유형**: Architecture / Phase 2A scope change (사용자 직접 지시)
**상태**: 🟢 승인
**Supersedes (부분)**: `docs/design/phase2/00-overview.md §7` "Chat UI comparison (OpenWebUI chosen)", 00-overview.md Q1 (2026-04-13)
**블록 해제 조건**: `docs/design/phase2/03-gadgetron-web.md` 설계 문서 작성 + Round 1.5/2 리뷰 통과

### 배경

2026-04-13 시점 결정 당시에는 OpenWebUI 를 "가장 성숙한 OSS 채팅 UI, BSD-3" 로 간주해 P2A sibling 프로세스로 채택했다. 이후 확인된 사실:

1. **라이선스 변경 (2025-04, v0.6.6 이후)**: OpenWebUI 는 BSD-3 에서 "Open WebUI License" (CLA 기반 custom license) 로 전환. 브랜딩(이름·로고·UI 식별자) 변경 금지 조항 도입. 예외는 ①30-day 롤링 ≤50 유저, ②머지된 substantive 기여자, ③유료 Enterprise 라이선스.
2. **Gadgetron 제품 철학과 충돌**: 단일 바이너리 + 자체 브랜드 제품 전략(`docs/00-overview.md §1.2`) 과 정면 충돌. on-prem/cloud 배포에서 "Open WebUI" 브랜딩을 보존해야 한다는 조항은 Kairos 제품 동선을 깬다.
3. **DB 중복**: OpenWebUI 는 자체 사용자/세션 DB (SQLite 또는 Postgres) 를 보유 → Gadgetron 의 `tenants` / `api_keys` 모델과 완전 중복. "2 SQL DB" 문제를 가중.
4. **스택 무게**: Python FastAPI + Svelte 별도 프로세스 → 단일 바이너리 원칙과 배치.

### 결정

**(a) OpenWebUI 를 Phase 2 스택에서 완전히 제거한다.** sibling 프로세스 번들·docker-compose 서비스·appendix 모두 삭제.

**(b) `gadgetron-web` 크레이트를 P2A 스코프 안으로 승격하여 자체 Web UI 를 직접 빌드한다.** 기술 스택:
- **Frontend**: [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT, shadcn + Radix 기반 headless React 컴포넌트 라이브러리) + Next.js + Tailwind
- **Backend embed**: `include_dir!` 매크로로 `web/dist/` 정적 자산을 Rust 바이너리에 컴파일 타임 embed (`platform-architecture.md §2.C.10` 에 이미 예약된 패턴)
- **Mount**: `gadgetron-gateway` 의 feature flag `web-ui` 활성화 시 `router.nest_service("/web", gadgetron_web::service())` 로 axum 에 마운트 (gateway 본체 dispatch 는 여전히 unchanged)
- **빌드 시퀀스**: `cargo xtask build-web` (또는 `build.rs` 안 `npm run build`) → 산출물 `crates/gadgetron-web/web/dist/` → `include_dir!` → 단일 바이너리

**(c) 인증 모델**: OpenAI-compat 표준 그대로. 사용자가 `/web` UI 에서 Gadgetron API key 를 세팅 화면에 붙여넣고, 프론트엔드는 `fetch('/v1/chat/completions', { headers: { Authorization: 'Bearer …' } })` 로 호출. P2A 는 BYOK 방식 (멀티유저 OAuth 는 P2C 재개방 주제).

**(d) Acceptance criteria 변경**: 기존 8개 중 OpenWebUI 관련 항목들을 `gadgetron-web` 으로 대체. "사용자가 OpenWebUI 드롭다운에서 `kairos` 선택" → "사용자가 `http://localhost:8080/web` 에 접속해 API key 입력 후 `kairos` 모델 선택".

### 근거

- 사용자 지시 2026-04-14: "assistant-ui 사용합시다"
- 라이선스 리스크 회피 (상업 배포·브랜딩 보존)
- 단일 바이너리 철학 완전 복원 (별도 프로세스 0, 별도 DB 0)
- `gadgetron-web` 크레이트는 이미 `platform-architecture.md §2.C.10` 에 Phase 2 예약 슬롯으로 존재 → 구조 변경 최소
- assistant-ui 는 "bring your own backend" headless 라이브러리 → 라이선스·데이터 모델 양쪽 모두 Gadgetron 소유

### Trade-offs

| 측면 | 획득 | 비용 |
|---|---|---|
| 라이선스 | 순수 MIT 스택 (Gadgetron 자체 브랜딩 100%) | 프론트엔드 직접 유지보수 |
| 배포 | 단일 바이너리 (docker-compose 제거) | Next.js 빌드 체인 (`npm`) CI 에 추가 |
| 기능 | UI 기능 스코프를 Gadgetron 이 직접 통제 | 초기 UX 풍부도는 OpenWebUI < assistant-ui 기본값 < 장기 커스텀 |
| Phase 2A 타임라인 | 별도 외부 의존성 없음 → 통합 테스트 단순 | UI 컴포넌트 직접 빌드 공수 추가 (주 단위) |

### 즉시 적용 변경 (이 세션)

- `docs/design/phase2/00-overview.md` — §3, §4, §5, §7, §10 (OpenWebUI API key handling), §13, §15, Appendix C, Threat model 내 OpenWebUI 참조, Q1 해결 이력
- `docs/design/phase2/01-knowledge-layer.md` — §1.1 `kairos init` 출력 contract 의 "Next steps" (OpenWebUI → gadgetron-web)
- `docs/design/phase2/02-kairos-agent.md` — §고위험 자산 표
- `docs/manual/kairos.md` — §2 Docker, §5 첫 대화, §Troubleshooting
- `docs/00-overview.md` — §1.2 "오픈소스 최대 활용" 리스트, §Roadmap Phase 2A row
- `docs/reviews/phase2/round2-dx-product-lead.md` — 과거 리뷰이므로 수정하지 않음 (히스토리컬)
- **NEW**: `docs/adr/ADR-P2A-04-chat-ui-selection.md` stub (assistant-ui 선택 근거)

### 후속 작업 (다음 세션)

1. `docs/design/phase2/03-gadgetron-web.md` — `gadgetron-web` 크레이트 상세 설계 (ux-interface-lead 주도)
2. Round 1.5/2 리뷰 — dx-product-lead + security-compliance-lead + qa-test-architect + chief-architect 가 신규 크레이트 + 변경된 threat model 을 재검토
3. `docs/design/phase2/00-overview.md §8` STRIDE 업데이트 — OpenWebUI 행 제거 및 `gadgetron-web` 행 추가 (same-origin XSS, API key storage 등 재평가)
4. `build.rs` / `cargo xtask` 빌드 통합 구현 방식 확정 (ADR-P2A-04 에서)

### 영향 받는 문서/크레이트

- `docs/design/phase2/*.md`, `docs/manual/kairos.md`, `docs/00-overview.md`, `docs/adr/ADR-P2A-04-*.md` (신설)
- **NEW crate**: `crates/gadgetron-web/` (P2A 스코프)
- `gadgetron-gateway` — feature flag `web-ui` 추가 (Cargo feature 게이트)
- Workspace `Cargo.toml` — `gadgetron-web` 멤버 추가, `include_dir = "0.7"`, `tower-serve-static = "0.1"` dependencies 추가

---

## D-20260414-03: 단일 배포 프로파일당 단일 SQL DB — `DatabaseBackend` trait 도입

**날짜**: 2026-04-14
**유형**: Architecture / DB backend strategy (사용자 직접 지시)
**상태**: 🟢 승인
**관련**: D-20260411-04 (Phase 1 GPU), D-20260411-12 (PgKeyValidator LRU), D-20260411-13 (GadgetronError::Database)
**블록 해제 조건**: `docs/design/database/backend-trait.md` 설계 문서 작성 + chief-architect + qa-test-architect + security-compliance-lead Round 리뷰 통과

### 배경

현재 Gadgetron 의 DB 사용 상태:

| 역할 | 엔진 | Phase | 상태 |
|---|---|---|---|
| 인증/테넌시/쿼터/감사 | PostgreSQL (sqlx, `--no-db` 인메모리 fallback) | Phase 1 | **구현 완료, 하드코드** |
| 지식 벡터 저장 | SQLite + sqlite-vec | P2B | 설계만 (`00-overview.md §7`) |
| Kairos 지식 (P2A) | filesystem + git2 | P2A | 설계 완료, DB 미사용 |

**문제**: 로컬 단일유저가 Phase 1 인증 스택을 쓰려면 PostgreSQL 를 떠야 함 (또는 `--no-db` 인메모리 모드 — 영속성 없음). P2B 진입 시 `SQLite + sqlite-vec` 까지 추가되면 운영자는 **두 SQL DB 엔진**을 동시에 학습·백업·모니터링해야 함. Gadgetron 의 "가볍게" 원칙(`docs/00-overview.md §1.2`) 과 "단일 바이너리 + TOML 설정" 약속과 충돌.

### 결정

**핵심 원칙**: "한 배포 프로파일은 최대 한 종류의 SQL DB 만 본다."

**(a) `DatabaseBackend` trait 을 `gadgetron-core` 에 도입하여 Phase 1 의 auth/billing/audit 를 DB 엔진에 대해 추상화한다.** trait 은 다음 operation 을 노출 (잠정):
```rust
#[async_trait]
pub trait DatabaseBackend: Send + Sync + 'static {
    async fn validate_key(&self, hash: &[u8; 32]) -> Result<Option<KeyRecord>, GadgetronError>;
    async fn record_usage(&self, req: UsageEvent) -> Result<(), GadgetronError>;
    async fn insert_audit(&self, entry: AuditEntry) -> Result<(), GadgetronError>;
    async fn check_quota(&self, tenant: TenantId) -> Result<QuotaSnapshot, GadgetronError>;
    // ... 정확한 signature 는 설계 문서에서 확정
}
```

구현체: `PostgresBackend` (현재 Phase 1 코드 재배치), `SqliteBackend` (신규), `InMemoryBackend` (현재 `--no-db` 재배치).

**(b) 배포 프로파일 정의 (`gadgetron.toml [database].profile`)**:

| 프로파일 | Auth/Billing | Vector (P2B+) | 대상 유저 |
|---|---|---|---|
| `local` | **SQLite** (단일 파일 `~/.gadgetron/gadgetron.db`) | SQLite + sqlite-vec (같은 파일 또는 sidecar) | 단일유저 데스크톱, P2A Kairos MVP |
| `server` | **PostgreSQL** | SQLite + sqlite-vec (per-instance) 또는 pgvector (P2C 결정) | on-prem, cloud, multi-tenant |
| `inmemory` | `InMemoryBackend` | 미지원 (벡터는 DB 필수) | CI/개발, 현재 `--no-db` 계승 |

로컬 유저는 SQLite 파일 1개만 보고, 서버 유저는 Postgres 1개만 본다. P2B 벡터 저장이 추가돼도 **같은 프로파일 내에서는 DB 엔진 1종류**.

**(c) Phase 1 코드 영향 범위**: 
- 현재 sqlx 쿼리가 Postgres 전용 문법(JSONB, `RETURNING`, `ON CONFLICT DO UPDATE`)을 쓰는 부분 조사 필요
- 조사 결과가 SQLite 포팅 난이도 결정 요소 (쉽게 호환 / 쿼리 재작성 필요 / SQLx Any 사용 / 전부 직접 구현)
- 마이그레이션은 엔진별 파일 분리 (`migrations/postgres/*.sql`, `migrations/sqlite/*.sql`)

**(d) 실행 순서**:
1. **즉시 (이 세션)**: 본 결정 로그 엔트리 기록만. P2A Kairos MVP 는 DB 를 건드리지 않으므로 코드 변경 없음
2. **단기 (P2A 구현 중)**: 영향 없음 — Kairos 진행
3. **중기 (P2B 진입 전)**: `docs/design/database/backend-trait.md` 설계 PR → chief-architect + qa-test-architect + security-compliance-lead 리뷰 → Phase 1 SQLite 백포트 구현 → `local` 프로파일을 기본값으로 승격
4. **장기 (P2C 멀티유저 재개방)**: `server` 프로파일 강제. 벡터 저장 엔진(SQLite per-tenant vs pgvector 통합) 은 별도 결정

### 근거

- 사용자 지시 2026-04-14: "DB는 단일 유저 SQLite로 시작해서 Postgres도 지원합시다"
- 단일 바이너리 + 단일 DB 파일로 "5분 안에 첫 대화" acceptance criterion (`00-overview.md §1.2`) 달성
- Postgres 를 운영 경험 없는 로컬 유저에게 강제하는 현재 구조의 onboarding 마찰 제거
- `DatabaseBackend` 추상화는 테스트 용이성에도 기여 (in-memory fake 를 trait 구현체로 자연스럽게 표현)
- P2B 벡터 엔진 선택을 `local` 프로파일 내에서 SQLite 하나로 일원화 → 운영자 멘탈 모델 단순

### Trade-offs

| 측면 | 획득 | 비용 |
|---|---|---|
| Onboarding | 로컬 단일 파일 → Postgres 설치 없이 사용 | trait 추상화 설계 1 회 공수 |
| 쿼리 호환성 | 두 엔진에서 같은 로직 보장 | Postgres-specific 최적화 일부 포기 (JSONB 연산자 등) |
| 성능 | SQLite 로컬 쓰기가 네트워크 Postgres 보다 빠름 (단일 프로세스) | 동시 쓰기 throughput 은 Postgres 가 우세 — `server` 프로파일이 해결 |
| 마이그레이션 | 프로파일 교체 경로 명확 (`local` → `server`) | 엔진간 데이터 내보내기/불러오기 도구는 신규 개발 (P2B+) |

### 즉시 적용 변경 (이 세션)

이 결정은 **기록만** 수행. 코드 변경 없음. 설계 문서(`docs/design/database/backend-trait.md`) 작성은 다음 세션에서 chief-architect 가 주도한다.

### 후속 작업

1. **조사**: Phase 1 코드의 Postgres-specific SQL 사용처 인벤토리 (`crates/gadgetron-xaas/` + `crates/gadgetron-core/` sqlx 호출 grep). 결과가 난이도 결정
2. **설계**: `docs/design/database/backend-trait.md` — trait signature 최종, 쿼리 매핑 표, 마이그레이션 전략, 테스트 fixture
3. **구현 (P2B 진입 전)**: SQLite 백엔드 + `local`/`server`/`inmemory` 프로파일 게이트 + `gadgetron init` 이 `local` 기본값으로 설정
4. **문서**: `docs/manual/configuration.md` + `docs/manual/installation.md` 업데이트 — 프로파일 선택 가이드

### 영향 받는 문서/크레이트

- `gadgetron-core` — `DatabaseBackend` trait 신설, `KeyRecord` / `AuditEntry` / `QuotaSnapshot` public export
- `gadgetron-xaas` — 현재 Postgres 직결 코드를 `PostgresBackend` 구현체로 캡슐화
- **NEW**: `gadgetron-xaas::sqlite` 모듈 또는 별도 `gadgetron-sqlite-backend` 서브크레이트 (결정 유보)
- `gadgetron.toml` 스키마 — `[database] profile = "local"|"server"|"inmemory"` 필드 추가
- `docs/design/database/backend-trait.md` (신설)
- `docs/design/phase2/00-overview.md §7` — 벡터 저장 섹션에 "same-profile" 원칙 명시

---

## D-20260414-04: Agent-Centric Control Plane + MCP Tool Registry + Brain Model Selection

**날짜**: 2026-04-14
**유형**: Architecture / Platform direction (사용자 직접 지시)
**상태**: 🟢 승인
**Supersedes (부분)**: `docs/design/phase2/00-overview.md §1 + §2` (상방/하방 프레이밍 → Agent-Centric 으로 강화), §3 "Explicit non-goals" 의 Anthropic `/v1/messages` 비구현 항목 (local brain shim 으로 조건부 reopen)
**관련 문서**: `docs/adr/ADR-P2A-05-agent-centric-control-plane.md` (신설), `docs/design/phase2/04-mcp-tool-registry.md` (신설)

### 배경

기존 Phase 2 설계는 Kairos 를 "wiki + 웹 검색 도구를 가진 personal assistant" 로 정의했다. `gadgetron-router` 의 provider map 에 `"kairos"` 이름으로 **다른 provider 와 병렬 등록**되며, 인프라(router / scheduler / node / GPU / cluster) 는 운영자가 TUI·HTTP API 로 **직접** 제어하는 별개 레이어였다.

사용자가 2026-04-14 세션에서 방향 전환 지시:

> 1. 에이전트(Claude Code CLI)는 이 플랫폼의 브레인이자 중추이다.
> 2. 모든 입력은 에이전트에게로 전달 된다.
> 3. 에이전트는 입력을 보고 하위에 여러 tools를 활용하여 지식을 관리하고, 정보를 얻고 제어한다.
> 4. 에이전트 하위 레이어는 지식레이어와 인프라레이어가 있을 것이다.
> 5. 인프라레이어는 라우팅, 프로바이더 제공, 자원 관리, 작업 스케줄러, 클러스터 관리(slurm/k8s) 등등을 할 수 있고
> 6. 에이전트는 이걸 활용하여 모니터링 및 제어를 한다. 이렇게 확장될 수 있도록 합시다.

추가로:
- 편의성을 위해 에이전트 우회 경로 (특정 `model=X` 로 직접 호출) 유지
- 에이전트의 브레인 모델도 운영자가 설정 가능; 자체 인프라의 로컬 모델일 수도 있음
- 환경설정은 유저가 명시적으로; auto-detect 금지
- 에이전트는 자기 브레인을 선택하지 않는다; 브레인 선택은 운영자 전용 권한

### 결정

#### (a) 플랫폼 비전 — Agent-Centric Control Plane

Gadgetron 의 주요 UX 는 "에이전트(Claude Code)가 브레인이고, 지식 레이어와 인프라 레이어가 모두 에이전트의 도구 세트" 라는 단일 문장으로 요약된다. 인프라는 "하방 레이어" 가 아니라 "에이전트의 tool category" 로 재프레이밍.

- `gadgetron-web` 의 기본 모델 = **`kairos`**. 모델 드롭다운에는 여전히 다른 모델(vllm/llama3, sglang/glm, openai/gpt-4o 등) 도 노출되어 API 우회 경로 편의성 유지
- API SDK 소비자의 `POST /v1/chat/completions` 직접 호출은 기존처럼 `model=X` 라우팅 — 기존 OpenAI 호환 계약 불변
- 그러나 "Gadgetron 의 상방 제품" 문서는 항상 **에이전트 경로** 를 기본으로 설명

#### (b) MCP Tool Registry — 3-tier + 3-mode 권한 모델

모든 MCP 도구는 stable `McpToolProvider` trait 을 통해 플러그인으로 등록된다. 확장 가능성을 P2A 에서부터 인터페이스 수준에서 보장:

**Tier 분류** (도구 개발자가 `ToolSchema.tier` 로 선언):
- **T1 Read**: 서버 상태를 관찰만 하는 순수 함수. 예) `wiki_list`, `wiki_get`, `wiki_search`, `web_search`, `list_nodes`, `get_gpu_util`, `list_models`. **항상 활성** (`mode = "auto"`, 변경 불가).
- **T2 Write**: 서버 상태를 수정하지만 되돌릴 수 있음. 예) `wiki_write`, `deploy_model`, `hot_reload_config`, `set_routing_strategy`. **기본 `ask`**, 운영자가 서브카테고리별로 `auto` 또는 `never` 로 override.
- **T3 Destructive**: 되돌릴 수 없음. 예) `kill_process`, `undeploy_model`, `slurm_cancel`, `kubectl_delete`, `wipe_wiki_page_history`. **기본 `enabled = false`**; 활성화해도 **mode 는 항상 `ask`** (config validation 이 `auto` 설정 시 시작 실패).

**3-mode**:
- `auto` — MCP server 가 즉시 실행, 감사 로그 기록, 사용자 개입 없음
- `ask` — 실행 전 정지, SSE 스트림에 `gadgetron.approval_required` 이벤트 emit, 프론트엔드 승인 카드 표시, 사용자 Allow/Deny 결정 받아 진행
- `never` — 항상 denial. 에이전트는 `tool_result { isError: true, reason: "disabled by policy" }` 수신

**T3 Cardinal Rule**: T3 도구는 `mode = "auto"` 로 설정 불가. `gadgetron.toml` 의 `[agent.tools.destructive]` 가 `default_mode = "auto"` 를 지정하면 **시작 실패** with 명시적 에러 메시지.

#### (c) 승인 카드 UX

채팅 입력이 아닌 UI 카드. `gadgetron-web` 이 SSE `gadgetron.approval_required` 이벤트 수신 시 모달/인라인 `<ApprovalCard>` 렌더:

| 요소 | T2 | T3 |
|---|---|---|
| 보더/색상 | 주황 | 빨강 |
| 헤더 | "Kairos wants to run a tool" | "Kairos wants to run a DESTRUCTIVE tool" |
| Tier 배지 | T2·Write | T3·Destruct |
| 도구명 + 카테고리 | ✅ | ✅ |
| 에이전트 rationale | ✅ | ✅ (자체 설명 강제) |
| 인자 (sanitize) | ✅ | ✅ |
| 되돌림 여부 문구 | "reversible" | "CANNOT be undone" |
| Rate limit 잔여 | — | "3 remaining this hour" |
| Allow once | ✅ | ✅ |
| Allow always | ✅ (localStorage 기억) | **❌ 영구 금지** |
| Deny | ✅ | ✅ |
| Timeout | 60s auto-deny (config) | 60s auto-deny (config) |

#### (d) 서버 측 approval 흐름

1. MCP server 가 tool 요청을 받으면 mode 확인
2. `ask` mode → `ApprovalRegistry::enqueue(pending)` → `(ApprovalId, oneshot::Receiver<Decision>)` 반환
3. Kairos provider 가 메인 SSE 스트림에 `event: gadgetron.approval_required` emit (heartbeat 로 스트림 유지)
4. `rx.await` 로 대기
5. 프론트엔드 → `POST /v1/approvals/{id}` with `{decision: "allow"|"deny"}` (신규 엔드포인트)
6. Gateway → `ApprovalRegistry::decide(id, decision)` → `tx.send(decision)` → `rx` unblock
7. Allow → tool 실행; Deny → `tool_result { isError: true, content: "User denied" }`
8. Timeout(60s, config) → auto-deny, rate limit counter 증가 안 함
9. 각 단계별 감사 로그 엔트리 (`ToolApprovalRequested` / `Granted` / `Denied` / `Timeout` / `ToolCallCompleted`)

#### (e) 브레인 모델 선택 — `[agent.brain]`

에이전트(Claude Code CLI)의 추론에 사용되는 LLM 은 **운영자 전용 설정**이다. 4 가지 모드:

- **`claude_max`** (기본값): 사용자의 `~/.claude/` OAuth (Claude Max 구독). Claude Code 가 Anthropic 클라우드 직결. Gadgetron 무관
- **`external_anthropic`**: 명시적 Anthropic API key + 선택적 `base_url`. 엔터프라이즈 계정 등. Claude Code 가 외부 엔드포인트 직결
- **`external_proxy`**: 사용자 운영 LiteLLM/프록시. `base_url` 로 지정. Gadgetron 무관
- **`gadgetron_local`**: Gadgetron 이 자체 `/internal/agent-brain/v1/messages` Anthropic 호환 shim 을 제공. `local_model = "vllm/llama3"` 등 라우터 provider map 의 기존 엔트리 선택. shim 이 Anthropic ↔ OpenAI 프로토콜 번역 후 기존 `gadgetron-router` 로 디스패치

**구현 방식 (옵션 D — 최소 내부 shim)**:
- `/internal/agent-brain/v1/messages` 엔드포인트 (Gateway 내부 경로)
- Rust 네이티브 번역기: `messages` / `system` / `tools` / 스트림 이벤트만 커버. 이미지 / PDF / cache_control 등 확장은 Phase 3
- Loopback 바인딩 (`127.0.0.1`)
- 시작 시 32바이트 랜덤 토큰 생성 → Claude Code subprocess 의 `ANTHROPIC_API_KEY` env 로 전달 → shim 이 헤더 비교. 토큰은 메모리에만, 로그·감사에 기록 금지, 재시작 시 rotation
- **재귀 방지 3중 방어**:
  1. Config validation: `local_model` 이 `kairos` 또는 Anthropic 계열이면 시작 실패
  2. 요청 태그: shim 이 `router.chat_stream` 에 `internal_call = true` 로 호출; 라우터는 이 플래그 true 일 때 `KairosProvider` 를 dispatch 대상에서 완전 제외
  3. Recursion depth 헤더: `X-Gadgetron-Recursion-Depth: 1` ≥ 2 이면 shim 이 거부
- **쿼터·감사·빌링**:
  - 사용자 쿼터는 **최상위** `/v1/chat/completions` 요청 단위로만 차감 (기존 Phase 1 동작)
  - 브레인 호출은 **쿼터 차감 제외** — 한 요청이 수십 번 브레인을 불러도 사용자 쿼터는 한 번만
  - 브레인 호출은 별도 audit 카테고리 `agent_brain` 로 기록, `parent_request_id` 로 상위 요청과 연결
  - 토큰 사용량(input/output)은 상위 요청의 audit entry 에 집계

#### (f) 명시적 scope boundary — 에이전트는 자기 브레인을 선택하지 않는다

- MCP 도구 registry 에 `agent.set_brain`, `agent.list_brains`, `agent.switch_model` 같은 **브레인 제어 도구 영구 제외**
- `[agent.brain]` 섹션은 **운영자 전용 config**. 에이전트가 `gadgetron.toml` 을 읽거나 수정하는 도구도 미제공
- 이유:
  - 프롬프트 인젝션 공격 벡터 차단 — "더 제약 없는 모델로 바꿔" 에 에이전트가 응답할 수 없게
  - 감사 추적 단순화 — 브레인은 Gadgetron 시작 시점에 고정, 교체는 명시적 config 변경 + 재시작
  - 권한 상승 차단 — 권한 설정 자체를 에이전트 스스로 바꿀 수 없음
- 인프라 읽기 도구 (`list_providers`, `list_models`) 는 제공하지만, 결과에서 **브레인으로 선택된 모델** 은 플래그 표시하거나 숨김 처리 (optional, `04-mcp-tool-registry.md` 에서 결정)

#### (g) 환경설정 — 유저 명시적

- **Auto-detect 금지**: `gadgetron.toml` 이 유일한 진실 공급원
- `kairos init` 은 대화형으로 운영자에게 브레인 모드를 물어봄 — 기본값 `claude_max`, `--brain-mode gadgetron_local --local-model vllm/llama3` 같은 CLI 플래그로 non-interactive 지원
- 자동 탐지 로직 (`~/.claude/` 존재 여부로 mode 추정, Anthropic API key env var 우선순위 등) **금지**. 모호성은 설정 에러로 드러나야 함

### 근거

- 사용자 지시 2026-04-14 (3 차례 interaction)
- 에이전트 중심 제어 평면은 "Claude Code 는 브레인, Rust 는 도구/하부 구조" 원칙의 자연스러운 확장
- 우회 경로 유지는 API 호환성 + 운영 유연성 (디버깅·벤치마크·레거시 통합)
- 브레인 모델 선택은 사용자의 모델 선호 / 비용 관리 / 에어갭 배포 수용을 위해 필수
- 에이전트가 브레인을 선택할 수 없다는 경계는 보안 원칙: **권한 상승을 유발할 수 있는 메타-조작은 에이전트 외부에서만**

### 구현 영향

**즉시 landing (이 세션)**:
- `docs/adr/ADR-P2A-05-agent-centric-control-plane.md` (신설, 아래 후속 커밋)
- `docs/design/phase2/04-mcp-tool-registry.md` (신설)
- `gadgetron-core::config::AgentConfig`, `ToolsConfig`, `BrainConfig` 타입 추가 + `AppConfig::agent` 필드
- `gadgetron-core::context::Scope` 에 `AgentApproval` variant 추가 (`D-20260411-10` 확장)

**P2A 구현 중 landing**:
- `#10` (MCP server) 가 `McpToolProvider` trait 기반으로 재작성 — P2B/C 확장 seam
- `#13`-`#15` (Kairos session/stream/provider) 가 `ApprovalRegistry` 통합
- Gateway `POST /v1/approvals/{id}` 엔드포인트 (`#5` 후속 확장)
- `gadgetron-web` `<ApprovalCard>` + `gadgetron.approval_required` SSE 파서 (`#4` 후속 확장)

**P2C 로 유보**:
- `/internal/agent-brain/v1/messages` shim 구현 (Anthropic ↔ OpenAI 번역기)
- `InfraToolProvider` (T2 infra_write) + `list_nodes`, `deploy_model` 등
- `gadgetron_local` 브레인 모드 활성화

**P3 로 유보**:
- `SchedulerToolProvider` (slurm, k8s)
- `ClusterToolProvider`
- Anthropic `/v1/messages` 완전 호환 확장 (Option C 경로)

### 영향 받는 문서/크레이트

- `docs/design/phase2/00-overview.md` §1 + §2 + §3 + §5 + §13 + §14 개정 (Agent-Centric 프레이밍)
- `docs/design/phase2/03-gadgetron-web.md` §12 + §17 (승인 카드 + UX flow 반영)
- `docs/adr/ADR-P2A-05-agent-centric-control-plane.md` (신설)
- `docs/design/phase2/04-mcp-tool-registry.md` (신설)
- `gadgetron-core::config` — 신규 타입
- `gadgetron-core::context::Scope` — `AgentApproval` variant
- `gadgetron-gateway` — `POST /v1/approvals/{id}` + SSE event emit
- `gadgetron-kairos` — `ApprovalRegistry`
- `gadgetron-knowledge` / (P2C) `gadgetron-infra` — `McpToolProvider` impl
- `gadgetron-web` — `<ApprovalCard>` + SSE parser

---

_(다음 엔트리는 아래에 append)_
