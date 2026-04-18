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
2. **Gadgetron 제품 철학과 충돌**: 단일 바이너리 + 자체 브랜드 제품 전략(`docs/00-overview.md §1.2`) 과 정면 충돌. on-prem/cloud 배포에서 "Open WebUI" 브랜딩을 보존해야 한다는 조항은 Penny 제품 동선을 깬다.
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

**(d) Acceptance criteria 변경**: 기존 8개 중 OpenWebUI 관련 항목들을 `gadgetron-web` 으로 대체. "사용자가 OpenWebUI 드롭다운에서 `penny` 선택" → "사용자가 `http://localhost:8080/web` 에 접속해 API key 입력 후 `penny` 모델 선택".

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
- `docs/design/phase2/01-knowledge-layer.md` — §1.1 `penny init` 출력 contract 의 "Next steps" (OpenWebUI → gadgetron-web)
- `docs/design/phase2/02-penny-agent.md` — §고위험 자산 표
- `docs/manual/penny.md` — §2 Docker, §5 첫 대화, §Troubleshooting
- `docs/00-overview.md` — §1.2 "오픈소스 최대 활용" 리스트, §Roadmap Phase 2A row
- `docs/reviews/phase2/round2-dx-product-lead.md` — 과거 리뷰이므로 수정하지 않음 (히스토리컬)
- **NEW**: `docs/adr/ADR-P2A-04-chat-ui-selection.md` stub (assistant-ui 선택 근거)

### 후속 작업 (다음 세션)

1. `docs/design/phase2/03-gadgetron-web.md` — `gadgetron-web` 크레이트 상세 설계 (ux-interface-lead 주도)
2. Round 1.5/2 리뷰 — dx-product-lead + security-compliance-lead + qa-test-architect + chief-architect 가 신규 크레이트 + 변경된 threat model 을 재검토
3. `docs/design/phase2/00-overview.md §8` STRIDE 업데이트 — OpenWebUI 행 제거 및 `gadgetron-web` 행 추가 (same-origin XSS, API key storage 등 재평가)
4. `build.rs` / `cargo xtask` 빌드 통합 구현 방식 확정 (ADR-P2A-04 에서)

### 영향 받는 문서/크레이트

- `docs/design/phase2/*.md`, `docs/manual/penny.md`, `docs/00-overview.md`, `docs/adr/ADR-P2A-04-*.md` (신설)
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
| Penny 지식 (P2A) | filesystem + git2 | P2A | 설계 완료, DB 미사용 |

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
| `local` | **SQLite** (단일 파일 `~/.gadgetron/gadgetron.db`) | SQLite + sqlite-vec (같은 파일 또는 sidecar) | 단일유저 데스크톱, P2A Penny MVP |
| `server` | **PostgreSQL** | SQLite + sqlite-vec (per-instance) 또는 pgvector (P2C 결정) | on-prem, cloud, multi-tenant |
| `inmemory` | `InMemoryBackend` | 미지원 (벡터는 DB 필수) | CI/개발, 현재 `--no-db` 계승 |

로컬 유저는 SQLite 파일 1개만 보고, 서버 유저는 Postgres 1개만 본다. P2B 벡터 저장이 추가돼도 **같은 프로파일 내에서는 DB 엔진 1종류**.

**(c) Phase 1 코드 영향 범위**: 
- 현재 sqlx 쿼리가 Postgres 전용 문법(JSONB, `RETURNING`, `ON CONFLICT DO UPDATE`)을 쓰는 부분 조사 필요
- 조사 결과가 SQLite 포팅 난이도 결정 요소 (쉽게 호환 / 쿼리 재작성 필요 / SQLx Any 사용 / 전부 직접 구현)
- 마이그레이션은 엔진별 파일 분리 (`migrations/postgres/*.sql`, `migrations/sqlite/*.sql`)

**(d) 실행 순서**:
1. **즉시 (이 세션)**: 본 결정 로그 엔트리 기록만. P2A Penny MVP 는 DB 를 건드리지 않으므로 코드 변경 없음
2. **단기 (P2A 구현 중)**: 영향 없음 — Penny 진행
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

기존 Phase 2 설계는 Penny 를 "wiki + 웹 검색 도구를 가진 personal assistant" 로 정의했다. `gadgetron-router` 의 provider map 에 `"penny"` 이름으로 **다른 provider 와 병렬 등록**되며, 인프라(router / scheduler / node / GPU / cluster) 는 운영자가 TUI·HTTP API 로 **직접** 제어하는 별개 레이어였다.

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

- `gadgetron-web` 의 기본 모델 = **`penny`**. 모델 드롭다운에는 여전히 다른 모델(vllm/llama3, sglang/glm, openai/gpt-4o 등) 도 노출되어 API 우회 경로 편의성 유지
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
| 헤더 | "Penny wants to run a tool" | "Penny wants to run a DESTRUCTIVE tool" |
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
3. Penny provider 가 메인 SSE 스트림에 `event: gadgetron.approval_required` emit (heartbeat 로 스트림 유지)
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
  1. Config validation: `local_model` 이 `penny` 또는 Anthropic 계열이면 시작 실패
  2. 요청 태그: shim 이 `router.chat_stream` 에 `internal_call = true` 로 호출; 라우터는 이 플래그 true 일 때 `PennyProvider` 를 dispatch 대상에서 완전 제외
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
- `penny init` 은 대화형으로 운영자에게 브레인 모드를 물어봄 — 기본값 `claude_max`, `--brain-mode gadgetron_local --local-model vllm/llama3` 같은 CLI 플래그로 non-interactive 지원
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
- `#13`-`#15` (Penny session/stream/provider) 가 `ApprovalRegistry` 통합
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
- `gadgetron-penny` — `ApprovalRegistry`
- `gadgetron-knowledge` / (P2C) `gadgetron-infra` — `McpToolProvider` impl
- `gadgetron-web` — `<ApprovalCard>` + SSE parser

---

## D-20260418-01: Plugin 평면 분류 + 첫 3 primitive + EntityTree 포레스트

**날짜**: 2026-04-18
**유형**: Architecture / Plugin taxonomy (사용자 직접 지시, 2026-04-18 세션)
**상태**: 🟢 승인
**Supersedes (부분)**: `docs/design/phase2/06-backend-plugin-architecture.md` §2 의 "한 덩어리 `plugin-ai-infra`" 가정 — 3 primitive(`plugin-server` / `plugin-gpu` / `plugin-ai-infra`) 분할로 세분화. §1 의 "한 공유 wiki + 여러 pluggable 백엔드" 원칙은 유지.
**관련 문서**: `docs/design/phase2/06-backend-plugin-architecture.md` (v2 개정 예정), `docs/design/phase2/07-plugin-server.md` (신설)

### 배경

사용자 2026-04-18 세션 지시:

> 1. plugin은 gadgetron이 할 수 있는 일을 설명하고 tool을 쥐어주는 역할
> 2. 플러그인이 종속되지 않고 독자적으로도 쓰일 수 있어야 한다 (GPU 하나만, 서버 하나만, 여러 서버 동시 관리도 가능)
> 3. Penny가 대상을 파악할 때 구조적으로 — GPU–서버–클러스터–인프라의 구조를 — 파악해야 한다
> 4. `plugin-newspaper` 같은 전혀 무관한 플러그인도 있을 것 (ai-infra·서버와 섞이지 않음)

세션 흐름: plugin-ai-infra 단일 덩어리 가정 → 일반 서버 관리(`plugin-server`)로 재정의 → tier 구조 검토 → flat peer + entity tree 분리 → 3 primitive 분할 → scheduler/router 는 plugin-ai-infra 내부로 결정 → plugin-newspaper 반례로 "포레스트" 필요성 확정.

### 결정

#### (a) Plugin 은 평면(flat) peer — 구조 강제 금지

- 모든 플러그인은 서로 **동등한 sibling**. 코드 레벨에서 parent/child 관계 없음.
- 플러그인 간 관계가 필요하면 **DAG 의존** (`PluginContext::get_service::<T>("<plugin-name>")` 로 선택적 조회). 순환 금지.
- 운영자 UX 에서는 의존 DAG 를 트리처럼 보여줌 (enable 시 자동 dependency resolution + 순서 해석). 하지만 이는 표시용일 뿐 런타임 parent 강제가 아님.
- tier(Tier 1 primitive / Tier 2 workflow / Tier 3 persona bundle) 개념 **불채택**. 이전 세션 논의에서 일시 제안되었으나 flat peer 원칙과 충돌 → 폐기.

#### (b) 첫 3 primitive 플러그인

| 플러그인 | 대화 상대 | 대표 tool (개요) | 포함 범위 |
|---|---|---|---|
| `plugin-server` | SSH 로 닿는 Linux/Unix 호스트 | `server.exec`, `server.metrics`, `log.tail`, `service.*`, `file.*` | OS/shell primitive. 구 `gadgetron-node` 의 OS·SSH 파트 |
| `plugin-gpu` | NVML (NVIDIA) / ROCm (AMD) 드라이버 | `gpu.list`, `gpu.metrics`, `mig.partition`, `nvlink.topology`, `numa.info` | GPU 하드웨어 primitive. 구 `gadgetron-node/monitor.rs` NVML 부분 + MIG/NUMA |
| `plugin-ai-infra` | vLLM/SGLang/Ollama/Anthropic/OpenAI/Gemini 등 추론 엔진·프로바이더 | `model.start`, `model.stop`, `provider.*`, `route.decide`, `scheduler.*`, `catalog.*` | 추론 엔진 수명주기 + LLM provider 어댑터 6종 + **라우터 (6 전략)** + **스케줄러 (VRAM bin-pack / LRU)** + 모델 카탈로그 |

3 plugin 모두 **독립 활성화 가능**. plugin-server 는 일반 서버 관리로, plugin-gpu 는 ML 학습·과학계산 GPU 모니터링으로, plugin-ai-infra 는 두 primitive 를 사용하지만 **선언적 의존**(DAG) 이지 parent 강제 아님.

`plugin-newspaper` 같은 **완전 무관한** 플러그인도 동일 prank 에 공존 가능 (서로의 엔티티 트리에 영향 없음).

#### (c) Core vs plugin 판정 룰 — "도메인 엔티티에 의존하는가?"

| 위치 | 기준 | 예 |
|---|---|---|
| **core** | 프레임워크 primitive (trait/registry/config/error), 또는 모든 plugin 공통의 cross-cutting 인프라 | `BackendPlugin`, `PluginRegistry`, `EntityRef`, `EntityTree`, audit, approval gate, `SecretCell`, 지식층(wiki), Penny, gateway |
| **plugin** | 도메인 엔티티(`ModelDeployment`, `HostId`, `GpuDevice`, `NewsArticle` …)에 의존하는 로직 — 알고리즘이 generic 해도 대상이 domain-specific 이면 plugin | VRAM bin-packing (inference 전용), routing 전략 6종 (LLM 전용), SSH connector (host 전용) |

이 룰의 직접 귀결:
- **Scheduler 는 plugin-ai-infra 내부** (구 `gadgetron-scheduler` → `plugins/plugin-ai-infra/src/scheduler/`). 기존 `06-backend-plugin-architecture.md` §6 원안 회복. 2026-04-18 세션 중 한때 "core service" 로 분리 제안했으나 철회.
- **Router 도 plugin-ai-infra 내부** (구 `gadgetron-router` → `plugins/plugin-ai-infra/src/router/`). 6 전략 모두 LLM 추론 특화.

#### (d) EntityRef / EntityTree — forest(포레스트) 로 일급 지원

엔티티는 **단일 트리가 아니라 여러 독립 트리의 포레스트**. `plugin-ai-infra` 의 인프라 트리 (Cluster → Server → GPU → MIG → Model) 와 `plugin-newspaper` 의 뉴스 트리 (Source → Article) 는 서로 무관하게 공존.

```rust
// gadgetron-core 에 추가
pub struct EntityRef {
    pub plugin: String,    // "server" | "gpu" | "ai-infra" | "newspaper" | ...
    pub kind: String,      // "host" | "gpu" | "mig" | "article" | ...
    pub id: String,
}

pub struct EntityKindSpec {
    pub kind: String,
    pub parent_kinds: Vec<String>,  // 비어있으면 forest 의 root
    pub display_name: String,
    // describer, child-lister 등은 trait 으로 plugin 이 구현
}
```

각 플러그인은 `PluginContext::register_entity_kind(...)` 로 자기 kind 의 parent 관계를 **선언만** 하고, core 가 모두를 모아 **forest 를 자동 조립**. 플러그인끼리 서로 모름.

#### (e) Core 제공 generic MCP tool 3종

```
entity.schema()                    — 등록된 kind + parent 관계 전체 (zero-state 조회)
entity.tree(root?, depth?)         — 실제 엔티티 forest 스냅샷 (root 생략 시 전체 포레스트)
entity.get(ref, include_children?) — 한 노드 + 직계 자식
```

Penny 가 대화 초기 `entity.schema()` 로 세상의 형태를 파악하고, 필요한 kind 만 `tree` / `get` 으로 깊이 조회 → 컨텍스트 오염 최소화.

#### (f) Wiki 경로 convention — forest root 기준

```
wiki/
├── infra/              ← 인프라 트리 루트
│   ├── clusters/<name>.md
│   ├── servers/<host>/index.md
│   ├── servers/<host>/gpus/<idx>.md
│   └── models/<id>.md
├── news/               ← newspaper 트리 루트
│   ├── sources/<name>.md
│   └── articles/<date>-<slug>.md
└── github/             ← 미래
```

**관습 (convention) 이지 strict rule 아님**. 권위는 frontmatter 의 `plugin = "<name>"` + `type = "<kind>"` 필드. 디렉토리는 UX 편의.

#### (g) Cluster 는 wiki 페이지, Postgres 테이블 아님

"클러스터" 는 `plugin-server` 인벤토리의 별도 테이블이 아니라 **wiki 페이지** (`infra/clusters/<name>.md` + `type = "cluster"` frontmatter). 정의 자체 (선언적 label selector, 멤버 예상 수, 쿼럼 정책, 소유자) 가 markdown 에 담김. 이유: 운영자 수정 이력 / 주석 / 연결성 / 인시던트 기록이 자연스럽게 같은 레이어에 쌓임.

`plugin-server` 는 **label 기반 selector** 로 클러스터 멤버를 해석 (`cluster=prod-web` 라벨을 가진 호스트). 배치 실행(`strategy = parallel | rolling | canary | halt-on-error`) 은 `plugin-server` 의 native 기능.

### 근거

- **사용자 2026-04-18 세션** (여러 차례 interaction):
  - "plugin은 gadgetron이 할 수 있는 일을 설명하고 tool을 쥐어주는 역할" → 플러그인 본질 정의
  - "플러그인이 종속되지 않고 독자적으로도 쓰일 수 있어야" → flat peer 확정
  - "GPU–서버–클러스터–인프라의 구조" + "plugin-newspaper" → forest 필요성
- **06-backend-plugin-architecture.md §1 원칙** ("지식은 core, capability 는 pluggable") 의 직접 귀결. 이번 결정은 원칙을 바꾸지 않고 구체화.
- **ADR-P2A-07 의 "지식 레이어는 도메인 비종속"** 과 정합. 엔티티 forest 는 지식층이 plugin-agnostic 을 유지하는 메커니즘.

### 구현 영향

**즉시 landing (이 세션 후속)**:
- `docs/design/phase2/07-plugin-server.md` draft v0 신설 (이 D-entry 다음 작업)
- `docs/design/phase2/06-backend-plugin-architecture.md` 에 이 D-entry 로의 포워드 레퍼런스 note (v2 전면 개정 전 최소 변경)

**P2A+ 구현 시 landing**:
- `gadgetron-core::plugin::EntityRef`, `EntityKindSpec` 타입 신설
- `gadgetron-core::plugin::EntityTree` 서비스 (forest 조립 + `entity.schema`/`tree`/`get` MCP tool 제공)
- `PluginContext::register_entity_kind()` 메서드 추가
- `PluginContext::get_service::<T>(plugin_name)` — cross-plugin 서비스 조회
- `BuiltInAiInfraPlugin` wrapper — 기존 `gadgetron-provider` + `-scheduler` + `-node` 의 AI 부분을 한 `BackendPlugin` 으로 감쌈 (crate 이동은 P2B)
- CLI `gadgetron plugins list` / `status` / `enable` / `disable`

**P2B 에서 landing**:
- `plugins/plugin-server/` 크레이트 신설 — `07-plugin-server.md` 스펙 구현
- `plugins/plugin-gpu/` 크레이트 신설 — `08-plugin-gpu.md` (신설 예정) 스펙 구현
- `plugins/plugin-ai-infra/` 크레이트 신설 — 기존 `gadgetron-{provider,scheduler}` + `gadgetron-node` 의 process/engine 파트 + xaas 의 카탈로그 이동
- `gadgetron-provider`, `gadgetron-scheduler`, (부분) `gadgetron-node` 크레이트 **삭제**

**P2C 로 유보**:
- `plugin-k8s`, `plugin-slurm` 같은 **managed cluster 조율자 플러그인**
- `plugin-cloud-aws` 등 외부 API 플러그인
- 외부(dynamic loading) 플러그인 지원

### 영향 받는 문서/크레이트

- `docs/design/phase2/06-backend-plugin-architecture.md` — v2 개정 필요 (tier 개념 삭제, 3 primitive 분할 반영, EntityTree forest 추가, scheduler/router 위치 명시). v2 정식 개정 전까지는 이 D-entry 를 정답으로 참조.
- `docs/design/phase2/07-plugin-server.md` (신설) — SSH primitive plugin 본 설계
- `docs/design/phase2/08-plugin-gpu.md` (신설 예정) — NVML/ROCm primitive plugin
- `docs/design/phase2/00-overview.md` — §2 크레이트 표 업데이트 (provider/scheduler/node 상태 변경 언급)
- `docs/architecture/platform-architecture.md` — §2.A.1 "10 Crates" 표에 plugin 디렉토리 추가 언급
- `gadgetron-core` — `EntityRef`, `EntityKindSpec`, `EntityTree` 추가, `PluginContext` 확장
- `gadgetron-provider`, `gadgetron-scheduler` — P2B 에 `plugin-ai-infra/` 내부로 이동
- `gadgetron-node` — P2B 에 SSH/OS 부분은 `plugin-server/`, NVML 부분은 `plugin-gpu/`, process/engine 부분은 `plugin-ai-infra/` 로 3분할

---

## D-20260418-02: Multi-user + Knowledge ACL foundation (D1–D8)

**날짜**: 2026-04-18
**유형**: Architecture / Identity & Access Control (사용자 직접 지시, 2026-04-18 세션 후속)
**상태**: 🟢 승인 (8 개 sub-decision D1–D8 인터뷰 방식으로 확정)
**관련 문서**: `docs/adr/ADR-P2A-08-multi-user-foundation.md` (신설), `docs/design/phase2/08-identity-and-users.md` (신설), `docs/design/phase2/09-knowledge-acl.md` (신설), `docs/design/phase2/10-penny-permission-inheritance.md` (신설)
**Supersedes (부분)**: `docs/design/xaas/phase1.md` §1.1–§2.2 의 "API key ↔ Tenant 직결" 모델 — user 레이어 삽입으로 변경

### 배경

사용자 2026-04-18 세션 지시:

> 큰 틀에서는 멀티유저, 공유지식, 지식접근권한을 기획해야합니다.

인터뷰 방식(B)로 8 개 sub-decision 순차 답변 수집. 각 결정은 다음 결정의 전제 구성. D-20260418-01 (플러그인 flat-taxonomy + EntityTree forest) 가 먼저 놓여 있고, 그 위에 **사람·팀·권한** 레이어를 덮는 foundation.

### 결정 (8 sub-decision)

#### (D1-a) User / Tenant / API-key 관계

`Tenant 1:N User 1:N ApiKey` 모델 확정. API key 는 user 가 발급하고 user 권한을 상속. 모든 요청의 primary actor identifier 는 `user_id`. 기존 `gadgetron-xaas::api_keys.tenant_id` 는 유지하고 **`user_id` FK 추가** (nullable during migration, NOT NULL enforcement 는 P2B 완료 시점).

#### (D2-a) Tenancy 범위 — P2B single-tenant multi-user

P2B 는 **single-tenant, multi-user**. 기본 tenant `"default"` 자동 생성, 모든 user 소속. 모든 테이블에 `tenant_id` 필드 **NOT NULL** 박되 값은 `"default"` 고정. tenant 생성/삭제/격리 enforcement 는 P2C. C 옵션 (tenant 개념 자체 미도입) 은 마이그레이션 대형 비용 때문에 반려.

#### (D7-a) Team 정의 방법 — DB 테이블

`teams` + `team_members` Postgres 테이블. REST API / web UI 로 admin 이 관리. wiki/TOML 기반 GitOps 대안은 런타임 변경 빈도·무결성·권한 부여 UI 측면에서 반려. 단 `admins` 는 **built-in virtual team** — `users.role='admin'` 로 자동 membership, `team_members` 에는 row 없음.

#### (D8-b) 운영자(Admin) 지위 모델 — users.role 플래그

`users.role ∈ { 'member', 'admin', 'service' }` 로 일원화. OS/config 레벨 super-admin 은 **bootstrap 경로** (빈 DB 일 때만 `gadgetron.toml [auth.bootstrap]` 로 첫 admin 생성) 로만 쓰고 이후는 DB 가 권위. `service` role 은 비인간 자동화 (외부 SDK), UI 로그인 불가.

#### (D3-a) 지식(wiki) 스코프 레벨 — 3-level + 직교 lifecycle 마커

**Read visibility** 는 `scope = "private" | "team:<id>" | "org"` 3-level. **Lifecycle ownership** 은 `plugin = "<name>"` 별도 필드 (06 doc §4 와 orthogonal). 기본값 `"private"` (보수적). Plugin seed 페이지는 기본 `"org"`. Admin-only 콘텐츠는 `scope = "team:admins"` built-in team 사용. 별도 `"admin"` scope 도입 안 함.

#### (D4-a) Read vs Write 분리 — scope-member 공통 편집 + locked 예외

Read rule = D3 의 scope. Write rule = "scope 멤버면 누구나 + admin". Org = 위키식 오픈 편집. **`locked = true`** frontmatter 가 유일한 예외 — owner + admin only 로 write 제한 (팀 정책 문서, plugin seed 기본값 등). Plugin seed 페이지는 `locked = true` 기본, user 가 의도적 unlock 시 `source_modified_by = "user:<id>"` 자동 기록.

#### (D6-a) Search ACL 필터링 — SQL pre-filter

pgvector 유사도 + PostgreSQL keyword 의 **하이브리드 검색 쿼리 자체에** SQL `WHERE` 로 scope/team/admin 조건 결합. GIN index + HNSW index 병용. Post-filter(B) 는 accessible 결과 손실 위험으로 반려. Materialized per-user accessible_ids cache(C) 는 P2C 규모 확대 시로 연기. Admin bypass 는 쿼리 한 줄 (`a.is_admin OR ...`). 접근 불가 페이지의 존재 자체를 드러내지 않음 ("제한됨" 같은 메타 금지).

#### (D5-a) Penny 권한 상속 — Strict inheritance

Penny 는 caller user 의 신원으로만 tool 실행. 별도 elevated 권한 **없음**. Claude Code subprocess 에 `GADGETRON_CALLER_USER_ID`, `_TENANT_ID`, `_ROLE`, `_TEAMS`, `_REQUEST_ID`, `ANTHROPIC_API_KEY` 6 필드 env 주입. 모든 MCP tool 은 `AuthenticatedContext` 를 받아야 compile — context 없는 호출 경로 타입 레벨 차단.

`ADMIN_ONLY_TOOLS` const 에 admin 전용 tool (`user.create`, `team.create`, `plugin.enable`, `config.set`, `key.issue`, `audit.export`, `user.impersonate` 등) 선언. plugin 이 이들을 `register_tool` 하려 하면 **panic**. Penny 가 자기 권한을 올리는 경로 영구 차단.

Audit schema 확장: `actor_user_id`, `actor_api_key_id`, `impersonated_by ('penny' | NULL)`, `parent_request_id` 4 컬럼.

### 근거

- 사용자 2026-04-18 세션: "멀티유저, 공유지식, 지식접근권한 기획"
- 8 개 결정 모두 **interview 형식(B 옵션)** 으로 하나씩 확정 — 전 결정의 전제가 다음 결정을 규정
- Phase 2B 범위 적합성 우선 — multi-tenant 격리·SSO·LDAP 같은 복잡도는 P2C 로 연기하되 스키마 미래 호환 확보 (nullable/default 패턴)
- D1–D8 의 귀결로 Penny 권한 모델(D5) 이 자연스럽게 strict inheritance 로 수렴 — 다른 선택이 있으면 D1–D4 의 무결성이 깨짐

### 구현 영향

**즉시 landing (이 세션 후속)**:

- `docs/adr/ADR-P2A-08-multi-user-foundation.md` (신설) — umbrella ADR
- `docs/design/phase2/08-identity-and-users.md` (신설) — D1/D2/D7/D8 구현 스펙
- `docs/design/phase2/09-knowledge-acl.md` (신설) — D3/D4/D6 구현 스펙
- `docs/design/phase2/10-penny-permission-inheritance.md` (신설) — D5 STRIDE + 타입 설계
- `docs/adr/README.md` — §목록 표에 ADR-P2A-08 추가

**P2B 구현 시 landing**:

- `gadgetron-xaas` schema:
  - `users` 테이블 신설 (`id UUID PK`, `email TEXT UNIQUE`, `display_name`, `role TEXT CHECK`, `tenant_id UUID REFERENCES tenants`)
  - `teams` + `team_members` 테이블 신설
  - `api_keys.user_id UUID REFERENCES users(id)` 추가
  - `audit_log.actor_user_id`, `actor_api_key_id`, `impersonated_by`, `parent_request_id` 4 컬럼 추가
  - wiki_pages 확장: `scope TEXT NOT NULL DEFAULT 'private'`, `owner_user_id UUID REFERENCES users(id)`, `locked BOOLEAN DEFAULT FALSE`, `source_modified_by TEXT`
  - 기존 `tenants` 에 row `"default"` 자동 마이그레이션
- `gadgetron-core`:
  - `AuthenticatedContext` 타입 신설
  - `MCP tool trait` 의 `invoke` 시그니처 변경 → compile-time 강제
  - `ADMIN_ONLY_TOOLS` const 및 register_tool 검증 훅
  - `TeamCache` (moka LRU, 1분 TTL)
- `gadgetron-penny`:
  - subprocess spawn 시 env 6 필드 주입
  - `wiki.write` 내부 `actor_user_id`/`impersonated_by='penny'` 기록
- `gadgetron-knowledge`:
  - 검색 함수 시그니처에 `user_id`, `tenant_id` 파라미터 필수화
  - pre-filter SQL (D6 스케치)
  - frontmatter 파싱에 `scope`, `locked`, `owner_user_id`, `source_modified_by` 지원
- `gadgetron-gateway`:
  - web UI 세션 middleware (로그인 → user_id 바인딩)
  - API key 인증 → user_id resolve
- `gadgetron-cli`:
  - `gadgetron user create/list/promote/delete`
  - `gadgetron team create/add/remove`
  - `gadgetron wiki share <page> --team <id>` / `--org` / `--private`
- `gadgetron-web`:
  - 로그인 UI
  - 페이지 scope 드롭다운 + `locked` 토글
  - Admin 콘솔 (user/team mgmt)

**P2C 로 유보**:

- Multi-tenant enforcement (tenant_id 격리 활성화, tenant 생성 UI)
- SSO / OIDC / SAML 연동
- LDAP user 디렉토리
- Materialized per-user accessible_ids cache (D6 C 옵션)
- Row-level encryption
- Fine-grained RBAC (team-admin, billing-viewer 등 세분 role)

### 영향 받는 문서/크레이트

- `docs/adr/ADR-P2A-08-multi-user-foundation.md` (신설, umbrella)
- `docs/design/phase2/08-identity-and-users.md` (신설)
- `docs/design/phase2/09-knowledge-acl.md` (신설)
- `docs/design/phase2/10-penny-permission-inheritance.md` (신설)
- `docs/design/xaas/phase1.md` — P2B 섹션에 user 레이어 확장 note 추가 필요 (본 D-entry 가 authoritative 까지 deferred)
- `docs/design/phase2/01-knowledge-layer.md` — §4 wiki 메타에 scope/owner/locked 필드 추가 언급 필요
- `docs/design/phase2/02-penny-agent.md` — env 주입 6 필드 + ADMIN_ONLY_TOOLS 참조 추가 필요
- `docs/design/phase2/04-mcp-tool-registry.md` — `AuthenticatedContext` + `ADMIN_ONLY_TOOLS` 통합 지점 명시 필요
- `docs/design/phase2/05-knowledge-semantic.md` — §4 검색 함수 시그니처에 user_id 추가 언급 필요
- `docs/design/phase2/07-plugin-server.md` — §13 Q-8 (single vs multi-tenant scope) 에 "D2-a 에 의해 single-tenant P2B 확정" 주석 추가 필요
- `gadgetron-core`, `gadgetron-xaas`, `gadgetron-knowledge`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-cli`, `gadgetron-web` — 위 구현 영향 목록

### 리뷰 권고

- `@security-compliance-lead`: 10-penny-permission-inheritance.md §STRIDE 의 T1–T5 위협 체인, `ADMIN_ONLY_TOOLS` const 우회 벡터, env 주입 위조 방어, admin bootstrap 의 race condition
- `@xaas-platform-lead`: 08-identity-and-users.md 의 스키마 확장이 기존 xaas (tenants/api_keys/audit_log) 와 충돌 없는지, P2C 의 multi-tenant 활성화 시 마이그레이션 경로
- `@dx-product-lead`: UI/CLI 동선 (scope 승격, team 추가, admin bootstrap 실수 시 복구)
- `@chief-architect`: `AuthenticatedContext` 의 type-state 강제가 기존 MCP tool trait 과 호환되는지, D-12 leaf crate 원칙 준수

---

## D-20260418-03: RAW 데이터 ingestion + RAG foundation (I1–I7)

**날짜**: 2026-04-18
**유형**: Architecture / Ingestion pipeline (사용자 직접 지시, 2026-04-18 세션 interview mode B)
**상태**: 🟢 승인 (7 개 sub-decision I1–I7 인터뷰 방식으로 확정)
**관련 문서**: `docs/adr/ADR-P2A-09-raw-ingestion-pipeline.md` (신설), `docs/design/phase2/11-raw-ingestion-and-rag.md` (신설)
**Supersedes (부분)**: `docs/adr/ADR-P2A-07-semantic-wiki-pgvector.md` §Context 의 "청킹 알고리즘 TODO" — I3 로 확정. 그 외 ADR-P2A-07 는 **유효**하고 이 결정은 그 위에 ingestion pipeline 을 얹음.

### 배경

사용자 2026-04-18 세션 지시 (D-20260418-02 직후):

> 다음으로 좀 불투명한 부분이 RAW 데이터를 제공했을때 어떻게 Wiki에 올리거나 인덱싱을 하거나 RAG를 할것인지에 대한 것을 잘 모르겠다.

ADR-P2A-07 은 "wiki.write → 청킹 → 임베딩" 의 downstream 경로만 설계했고, PDF/docx/HTML/URL 같은 RAW 소스가 wiki 페이지가 되는 **upstream** 경로는 공백. D-20260418-02 (multi-user + ACL) 에서 확정된 scope / owner / locked / search pre-filter 규칙을 soup 하지 않으면 실제로 RAG 가 동작하지 않음.

Interview 모드(B)로 7 개 sub-decision 순차 확정.

### 결정 (7 sub-decision)

#### (I1-b+c) 입력 타입 — text 계열 + HTML/URL

**v1 지원**: `text/markdown`, `text/plain`, `application/pdf`, `application/vnd.openxmlformats-officedocument.wordprocessingml.document` (.docx), `application/vnd.openxmlformats-officedocument.presentationml.presentation` (.pptx), `text/html`.

**반려**: A (markdown-only) 실사용 미흡, D (OCR·ASR) 범위 폭증. D 는 P2C+ 에 `plugin-ai-infra` 기반 ASR extractor 로 연장 가능.

**의존**: URL fetch 는 core 가 하지 않음. `plugin-web-scrape` 가 fetch → bytes → `wiki.import` 호출.

#### (I2-a+) 저장 모델 — wiki markdown + 원본 blob 보존

RAW → `ingested_blobs` (보존) → Extract → `wiki_pages` (frontmatter 에 `source_blob_id`) → `wiki_chunks` (ADR-P2A-07 파이프라인 재사용).

**반려**: A (원본 폐기) 감사·재추출 불가, B (blob primary) 검색·ACL 이중화, C (LLM 요약 card) 품질 종속 + 이중화.

**스키마 추가**: `ingested_blobs` 테이블 하나. `wiki_pages` 는 frontmatter 만 확장 (컬럼 추가 없음, ADR-P2A-07 의 `frontmatter JSONB` 활용).

#### (I6-d) Plugin 경계 — Core trait + Plugin 구현

- **Core (`gadgetron-core`)**: `BlobStore` trait, `Extractor` trait, `ExtractedDocument`·`BlobMetadata` 타입
- **Core (`gadgetron-knowledge`)**: `IngestPipeline` 오케스트레이션 (extract → blob → wiki → chunk → embed → audit), `wiki.import` MCP tool 제공
- **Plugin**: Extractor 구현체. v1 에 두 개:
  - `plugins/plugin-document-formats/` — PDF / docx / pptx / markdown (feature-gated)
  - `plugins/plugin-web-scrape/` — HTML extractor + `web.fetch` MCP tool (URL fetch)

**반려**: A (core 에 bulky 의존), B (별도 plugin-ingest 로 pipeline 이동) 는 wiki_pages 무결성 책임 분할, C (format 별 plugin 분산) 공통 로직 중복.

**플러그인 간 호출**: `plugin-web-scrape.web.fetch(url)` → bytes → `wiki.import(bytes, "text/html")` → core 가 등록된 HTML extractor 선택. flat-taxonomy + D-20260418-01 의 의존 DAG 일관.

#### (I3-d) 청킹 전략 — Hybrid (heading 1차 + fixed-size 2차 + 원자 블록)

**규칙**:
1. Frontmatter 제거 (embedding 대상 아님)
2. Markdown heading 으로 1차 split (depth 3 = H1/H2/H3)
3. 섹션이 `max_tokens=1500` 초과 시 paragraph 경계 → fixed-size + overlap 으로 2차 split
4. 코드 블록 / 표 / 리스트는 atomic (가능한 한 분리 금지). 코드 블록이 `max_tokens` 초과 시 강제 split + warning
5. `min_tokens=100` 미만 chunk 는 sibling 과 merge
6. Target 500 / max 1500 / min 100 / overlap 50 tokens (tiktoken cl100k_base 기준)

**Chunk 메타데이터**: `heading_path: Vec<String>`, `position: u32`, `byte_start/end`, `token_count`, `source_page_hint: Option<u32>` (PDF page hint extractor 가 제공 시), `has_code_block`, `has_table`.

**Token count**: `EmbeddingProvider::token_count(text)` trait method 로 provider-specific. OpenAI embed → tiktoken, 로컬 → sentence-transformers tokenizer.

**Re-chunking**: `gadgetron reindex --rechunk` 서브커맨드 (config 변경 시). 자동 감지 없음.

**Config 노출**:
```toml
[knowledge.chunking]
strategy = "hybrid"          # fixed | heading | hybrid
target_tokens = 500
max_tokens = 1500
min_tokens = 100
overlap_tokens = 50
heading_depth = 3
preserve_code_blocks = true
preserve_tables = true

[knowledge.chunking.per_source]
"application/pdf" = { target_tokens = 700 }
"text/html" = { target_tokens = 400 }
```

**반려**: A (fixed-only) 구조 상실, B (heading-only) 크기 편차 극심, C (format-specific) 추출 후 정보 손실.

#### (I4-d) Frontmatter enrichment — Caller opt-in

**자동 채움** (논쟁 없음): `source="imported"`, `source_filename`, `source_content_type`, `source_blob_id`, `source_bytes_hash`, `source_imported_at`, `imported_by`, `owner_user_id`, `created`, `updated`, `title` (4-step fallback: caller arg → extractor metadata → filename → timestamp+hash).

**Enrichment opt-in**: `wiki.import(..., auto_enrich: bool)`. `true` 시 Penny 가 호출되어 `tags` (3–7), `type` (runbook/reference/policy/note/meeting/decision/incident/dataset 중 convention), `summary` (≤ 500 chars) 제안. 결과에 `auto_enriched_by="penny"`, `auto_enriched_confidence`, `reviewed_by=null` 마커.

**Caller 권한**: enrich LLM 호출은 caller `AuthenticatedContext` 로 수행 (10 doc D5 일관). quota·audit 도 caller 단위.

**반려**: A (수동 only) bulk 방치, B (항상 enrich) 비용 통제 불가, C (클릭 시만) bulk 대응 안 됨.

**후속 enrich**: `wiki.enrich(page_id)` 별 MCP tool 로 언제든 재실행.

#### (I5-c) Dedup / Update / Citation

##### (I5a-c) Dedup

- `ingested_blobs.content_hash UNIQUE (tenant_id, content_hash)` — blob 은 tenant 단위 dedup
- 같은 caller + 같은 target_path 재upload → 기존 page 반환 (idempotent)
- 다른 caller 또는 다른 target_path → 새 wiki_pages 생성, blob 재사용
- Blob reference counting (lazy GC) — `gadgetron gc --blobs` cron

##### (I5b-c) Update / Supersession

- Target path 충돌 시 caller 가 `overwrite: bool` 명시. 기본 `false` (충돌 에러)
- `overwrite: true` + 기존 page 에 `superseded_at = now`, `superseded_by_page_id = <new>` 기록. 새 페이지에 `supersedes_page_id = <old>`
- Wiki 검색 기본 필터: `superseded_at IS NULL` (최신만)
- URL re-scrape (`plugin-web-scrape`) 기본: timeline 모드 (매 fetch 새 페이지, supersedes chain). Opt-in `overwrite` 모드.
- Auto detection (같은 filename / title 에 자동 chain) **없음** — caller 명시

##### (I5c-c) Citation — Markdown footnote

Penny 응답 포맷:
```
본문 [^1] [^2] ...

[^1]: [<page title> §<heading_path>](/#/wiki/<path>) · 원본 [`<source_filename>` p.<page_hint>](/api/v1/blobs/<blob_id>/view)
[^2]: ...
```

- System prompt 지침으로 강제 (chunk metadata 기반)
- Blob viewer (`GET /api/v1/blobs/<id>/view`) 는 blob 참조 wiki_pages 중 caller read 가능한 것이 하나라도 있으면 서빙. 불가 시 404 (info leakage 방지, 09 doc §8 일관)
- imported 가 아닌 user-written wiki 페이지는 원본 링크 생략

#### (I7-a) ACL at ingestion

##### (I7a-a) Scope 기본 `private`

- 09 doc §4.3 wiki 페이지 기본값 일관
- Caller 가 `wiki.import(..., scope: "team:platform" | "org")` 명시 override
- 권한 밖 scope 지정 → permission_denied (09 doc §5 can_write 규칙)

##### (I7b-a) Locked 기본 `false`

- User-contributed content 는 위키식 오픈 편집 자연
- Plugin seed 는 `locked=true` 기본 (09 doc §7) 과 분리 — import 는 user 능동적 업로드이지 plugin 자동 주입 아님
- Policy-grade 문서는 caller 가 `locked: true` 명시

##### (I7c) Blob ACL — 참조 페이지 read ACL 의 union

- Blob 자체 ACL 없음. `GET /api/v1/blobs/<id>/view` 는 blob 참조하는 wiki_pages 중 caller 가 read 가능한 것 하나라도 있으면 허용
- 실패 시 404 (not 403)
- Edge case: Alice 가 private 로 import → Bob 이 같은 bytes 를 org 로 import → blob 재사용, Bob 의 org page 로 인해 모든 org member 가 blob 접근. Alice 의 private 문맥은 유출되지 않음 (scope 는 wiki_pages 레벨)

##### (I7d) Penny import — caller 상속

10 doc D5 그대로 적용. `wiki.import` tool 이 `AuthenticatedContext` 받음. scope/target_path 검증은 caller 권한으로. `impersonated_by='penny', parent_request_id=<caller req>` audit.

### 근거

- 사용자 2026-04-18 세션: "RAW 데이터 제공 시 wiki / 인덱싱 / RAG 경로 불투명"
- 7 개 sub-decision 모두 interview 형식 (B 옵션) 으로 순차 확정
- ADR-P2A-07 의 "wiki.write → 청킹 → 임베딩" 파이프라인은 유효, 그 위에 **upstream ingestion** 레이어를 얹음
- D-20260418-01 (flat plugin) + D-20260418-02 (ACL foundation) 의 귀결로 "core pipeline + plugin extractor" (I6-d), "caller 상속" (I7d), "scope 기본 private" (I7a) 가 자연스럽게 수렴

### 구현 영향

**즉시 landing (이 세션 후속)**:

- `docs/adr/ADR-P2A-09-raw-ingestion-pipeline.md` (신설)
- `docs/design/phase2/11-raw-ingestion-and-rag.md` (신설, draft v0)
- `docs/adr/README.md` — §목록에 ADR-P2A-09 추가

**P2B 구현 시 landing**:

- `gadgetron-core::ingest::{BlobStore, Extractor, BlobMetadata, ExtractedDocument, ExtractHints, ImportOpts}` 타입 신설
- `gadgetron-core::ingest::BlobRef`, `ChunkingConfig` 신설
- `gadgetron-core::plugin::PluginContext` 에 `register_extractor`, `register_blob_store` 메서드 추가
- `gadgetron-knowledge::ingest::IngestPipeline` 신설 (파이프라인 오케스트레이션)
- `gadgetron-knowledge::chunking::chunk_hybrid` 함수 (I3 알고리즘)
- `gadgetron-knowledge::mcp::WikiImportToolProvider` — `wiki.import`, `wiki.enrich` MCP tool
- `gadgetron-xaas` schema 마이그레이션:
  ```sql
  -- 20260418_000003_ingested_blobs.sql
  CREATE TABLE ingested_blobs (
      id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
      tenant_id       UUID NOT NULL REFERENCES tenants(id),
      content_hash    TEXT NOT NULL,
      content_type    TEXT NOT NULL,
      filename        TEXT NOT NULL,
      byte_size       BIGINT NOT NULL,
      storage_uri     TEXT NOT NULL,
      imported_by     UUID NOT NULL REFERENCES users(id),
      imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
      UNIQUE (tenant_id, content_hash)
  );
  CREATE INDEX idx_blobs_hash ON ingested_blobs (content_hash);
  CREATE INDEX idx_blobs_user ON ingested_blobs (imported_by, imported_at DESC);

  CREATE INDEX idx_wiki_source_blob ON wiki_pages
      ((frontmatter->>'source_blob_id'))
      WHERE frontmatter->>'source_blob_id' IS NOT NULL;
  ```
- `gadgetron-core::blob::{FilesystemBlobStore}` — v1 기본 구현
- `plugins/plugin-document-formats/` 크레이트 신설:
  - `PdfExtractor` (pdf-extract crate)
  - `DocxExtractor` (docx-rs or pandoc subprocess)
  - `PptxExtractor` (pandoc subprocess)
  - `MarkdownExtractor` (near-noop, structure_hints 추출만)
- `plugins/plugin-web-scrape/` 크레이트 신설:
  - `web.fetch` MCP tool (HTTP client + robots.txt 존중)
  - `HtmlExtractor` (html2md crate)
- `gadgetron-cli` 서브커맨드:
  - `gadgetron wiki import <file> [--scope] [--target-path] [--auto-enrich] [--overwrite]`
  - `gadgetron wiki enrich <page>`
  - `gadgetron reindex --rechunk` — config 변경 후
  - `gadgetron gc --blobs` — 고아 blob 정리
- `gadgetron-web`:
  - 업로드 UI (drag-drop → `/v1/wiki/import` 엔드포인트)
  - 페이지 상세에 "View original" 버튼 (blob link)
  - "Suggest tags" 버튼 (wiki.enrich)
  - Supersession chain 표시 (past versions toggle)
- `gadgetron.toml` 에 신규 섹션:
  - `[knowledge.chunking]` (I3 config)
  - `[knowledge.blob_store]` (storage_uri, max_file_bytes)
  - `[plugins.document-formats]` (feature flags per format)
  - `[plugins.web-scrape]` (robots.txt 정책, user-agent)

**P2C 로 유보**:

- S3BlobStore / PostgresLoBlobStore 구현
- OCR (Tesseract) / ASR (Whisper) extractor — `plugin-ai-infra` 위에서
- Large file streaming (> 50 MB)
- Materialized per-user accessible_ids cache (D-20260418-02 D6 C 옵션)
- Email / Slack archive import extractor
- Binary content chunking (CSV, image, audio)

### 리뷰 권고

- `@security-compliance-lead`: 11 doc §STRIDE — URL fetch SSRF 방어, blob ACL leak 경로, PDF extractor 의 xpdf CVE 이력, LLM enrich 의 prompt injection (사용자 업로드 PDF 가 Penny 시스템 프롬프트 탈취 시도)
- `@chief-architect`: `BlobStore`/`Extractor` trait 이 `gadgetron-core` leaf 원칙 (D-12) 준수, `PluginContext::register_extractor` 가 D-20260418-01 (e) 의 registration 패턴 일관
- `@qa-test-architect`: 11 doc §Test plan — chunking fixture 5 종 (heading 있음/없음/큰 섹션/코드 블록/표), dedup 경계 케이스, supersession chain, citation 포맷 회귀
- `@dx-product-lead`: CLI `gadgetron wiki import` 동선, 업로드 UX (drag-drop · progress · enrich 체크박스), 에러 메시지 (unsupported_content_type / permission_denied / overwrite_required)

### 영향 받는 문서/크레이트

- `docs/adr/ADR-P2A-09-raw-ingestion-pipeline.md` (신설, umbrella)
- `docs/design/phase2/11-raw-ingestion-and-rag.md` (신설, draft v0)
- `docs/adr/ADR-P2A-07-semantic-wiki-pgvector.md` — 청킹 알고리즘 TODO 섹션에 "I3 로 확정, 11 doc §6 참조" note 추가 필요 (본 PR 에서는 deferred)
- `docs/design/phase2/05-knowledge-semantic.md` — 청킹 설계 세부가 11 doc 으로 이동 (cross-reference 추가 필요)
- `docs/design/phase2/09-knowledge-acl.md` — §6 `wiki.write` 시맨틱에 `wiki.import` 관련 subsection 추가 필요
- `docs/design/phase2/10-penny-permission-inheritance.md` — Penny 의 `wiki.import` 호출 흐름은 이미 §3.1 원칙으로 커버됨. 명시적 예시 추가 가능
- `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-xaas` — 스키마/타입/서비스 확장
- `plugins/plugin-document-formats/`, `plugins/plugin-web-scrape/` — 신설 크레이트 2 개

---

## D-20260418-04: Extension 어휘 통일 — Bundle / Plug / Gadget trinity

> **♻️ 2026-04-18 수정**: 당초 "Driver" 로 기록된 Axis A 명칭은 D-20260418-05 에서 **Plug** 로 교체됨 (동일일 외부 리뷰 반영). 본 엔트리의 "Driver" 용어는 전부 "Plug" 로 읽으시오. 그 외 Bundle / Gadget / Rust rename / CLI / config 결정은 유효.

**날짜**: 2026-04-18
**유형**: Architecture / Naming / Public API (사용자 직접 지시, 2026-04-18 세션)
**상태**: 🟢 승인 (3 결정 동시 확정) — Axis A 명칭만 D-20260418-05 로 대체됨
**관련 문서**: `docs/adr/ADR-P2A-10-bundle-driver-gadget-terminology.md` (신설), `docs/architecture/glossary.md` (신설)
**Supersedes**: D-20260418-01 §(1) "플러그인 평면 분류" 의 **용어** 부분만 — plugin 이 3 개 개념을 섞고 있었다는 문제를 해결. D-20260418-01 의 **구조적** 결정 (flat peers, 3 primitive, EntityTree forest, DAG) 은 **그대로 유효**하고 이 결정은 그 위에 명칭을 재배열.

### 배경

사용자 2026-04-18 세션 지시 흐름:

1. "graphify 프로젝트를 gadgetron 에 녹일 수 있을지 검토" — graphify 는 Python MCP stdio 유틸리티, gadgetron 의 "single binary" 와 정합성 없음
2. "single rust binary 는 core 에만 해당. 외부 기능의 utility 를 도킹 할 플랫폼을 만들자" — core 바깥의 확장 platform 필요성 확인
3. "core 확장 dock vs penny 기능 plugin 두 가지로 봐도 되나" — 2 축 분리 개념 제시
4. "명칭도 저게 적당한지 논의" — Dock/Plugin 의 overload 문제 제기
5. "Driver-Gadget 이게 의미적으로 맞을 것 같고, 번들 이름을 꼭 정해야하나" — Driver/Gadget 확정, Bundle 의 존재 필요성 질의
6. "번들로 갑시다. 번들 트레잇은 번들이지. 축약합시다" — Bundle 확정, `Bundle` trait 이름, `gadgetron install` alias 채택

**문제 (기존 어휘가 실패하는 지점)**:

- `BackendPlugin` trait (06 doc §3) 가 `initialize(&mut PluginContext)` 에서 LlmProvider / Extractor / BlobStore / HTTP routes / MCP tools / seed pages 전부 등록 — 6 가지 관심사가 한 trait 에 섞여 있음
- `McpToolProvider` (04 doc v2 §3) 는 MCP tool 만 공급 — `BackendPlugin` 과 독립 axis 인데 같은 "plugin" 단어 공유
- 외부 유틸리티 (graphify, whisper, PaddleOCR) 는 두 trait 어느 쪽에도 안 맞음 — 제 3 의 플러그인 kind 가 암묵적으로 필요해짐
- "이 plugin 이 Penny 한테 tool 을 주는가?" 가 매번 문서마다 다른 답 — 독자가 지칭 대상을 매번 재확인해야 함

### 결정 (3 sub-decision 동시)

#### (T1) Extension 은 **3 개 서로 다른 개념** 으로 분리

| 개념 | Consumer | Interface | 성능 특성 | 배포 가능 runtime |
|---|---|---|---|---|
| **Bundle** | 운영자 (install/enable/disable 대상) | `Bundle` trait + `bundle.toml` manifest | 라이프사이클 단위 | Rust 컴파일 / Rust dylib / pip / npm / docker / binary-url |
| **Driver** | Core (gateway, router, wiki, scheduler, embedding) | Rust trait impl (ex. `impl LlmProvider for OpenAi`) | Hot path 가능 (정적 디스패치) | **Rust in-process only** |
| **Gadget** | Penny (LLM agent) | MCP tool schema (JSON) | Agent loop 수준 | In-core Rust / In-bundle Rust / subprocess / HTTP / wasm |

하나의 Bundle 이 0..N Driver + 0..N Gadget 공급 가능. 3 형상 모두 valid: Driver-only Bundle (ex. `blob-s3`), Gadget-only Bundle (ex. `graphify`), Driver+Gadget Bundle (ex. `ai-infra`).

#### (T2) Rust 심볼 rename (요약, 전체 목록은 ADR-P2A-10 §Rename scope)

- `BackendPlugin` → **`Bundle`**
- `PluginContext` → **`BundleContext`**
- `McpToolProvider` → **`GadgetProvider`**
- `McpToolRegistry` → **`GadgetRegistry`**
- `Tier` → **`GadgetTier`**, `ToolMode` → **`GadgetMode`**
- `ctx.register_extractor(...)` → **`ctx.drivers.extractors.register(...)`**
- `ctx.mcp_registry_mut()` → **`ctx.gadgets_mut()`**
- `gadgetron-penny::mcp_server` → **`gadgetron-penny::gadget_server`**
- 변경 없음: `DisableBehavior`, `SeedPage`, `LlmProvider`, `Extractor`, `BlobStore`, `EntityKind` (Driver trait 이름들은 domain-specific 이라 유지)

#### (T3) CLI / config / 디렉토리 rename

- CLI 정식: `gadgetron bundle|driver|gadget <subcmd>`
- CLI alias (운영자 편의): `gadgetron install <name>` = `gadgetron bundle install <name>`
- `gadgetron mcp serve` → **`gadgetron gadget serve`** (1 릴리스 deprecation shim 유지)
- Config: `[plugins.<name>]` → **`[bundles.<name>]`**, `[agent.tools]` → **`[agent.gadgets]`**
- 디렉토리: `plugins/plugin-<name>/` → **`bundles/<name>/`** (5 개 target)

### 반려 옵션

- **Dock + Plugin** (사용자 원안 1) — `Dock` 의미 반전 (외부가 host 에 꽂힌다는 일반 직관의 역) + Docker 혼동. `Plugin` 은 오버로드 및 기존 `BackendPlugin` 과 충돌
- **Port + Gadget + Adapter** (hexagonal architecture 정합) — `Port` 의 네트워크 포트 혼동, 2-word 용어 ("LlmProvider Port, OpenAi Adapter") 는 문서 verbose
- **현 용어 유지 + 문서로 해석** — 용어 모호성이 영구 채무로 남음. Rename 비용은 한 번, 모호성 비용은 매 설계 리뷰마다 재발생
- **두 개 entry-point trait (`DriverProvider` + `GadgetProvider`, Bundle trait 없음)** — 대부분 Bundle 이 양쪽 다 공급 → boilerplate 2 배, lifecycle 에 anchor 없음
- **Bundle 대신 content 별 명명** ("Gadget package", "Driver package") — 혼합 Bundle (`ai-infra` 같은) 이 자연스러운 카테고리 없음, version/license/audit 는 분배 단위 granularity
- **Kit** (Gadget Kit 어감 좋음) — "AI-infra Kit" 이 어색, Rust 생태계 discoverability 는 `Bundle` 이 우위

### 사용자 결정

**2026-04-18**: 3 결정 동시 승인.

- Axis A (core 확장) = **Driver**
- Axis B (Penny 확장) = **Gadget**
- 분배 단위 = **Bundle**
- Rust entry-point trait = **`Bundle`** (간단하게)
- CLI = `bundle|driver|gadget` + `gadgetron install` 축약

### 영향 받는 문서/크레이트 (exhaustive)

**신설**:
- `docs/adr/ADR-P2A-10-bundle-driver-gadget-terminology.md`
- `docs/architecture/glossary.md`

**파일 rename (경로 변경)**:
- `docs/design/phase2/04-mcp-tool-registry.md` → `04-gadget-registry.md`
- `docs/design/phase2/06-backend-plugin-architecture.md` → `06-bundle-architecture.md`
- `docs/design/phase2/07-plugin-server.md` → `07-bundle-server.md`

**본문 용어 치환 대상**:
- `docs/00-overview.md`
- `docs/design/phase2/00-overview.md`
- `docs/design/phase2/01-knowledge-layer.md`
- `docs/design/phase2/02-penny-agent.md`
- `docs/design/phase2/05-knowledge-semantic.md`
- `docs/design/phase2/08-identity-and-users.md`
- `docs/design/phase2/09-knowledge-acl.md`
- `docs/design/phase2/10-penny-permission-inheritance.md`
- `docs/design/phase2/11-raw-ingestion-and-rag.md`
- `docs/design/ops/agentic-cluster-collaboration.md`
- `docs/design/ops/operations-tool-providers.md`
- `docs/adr/ADR-P2A-05-agent-centric-control-plane.md` (header note 추가)
- `docs/adr/ADR-P2A-09-raw-ingestion-pipeline.md` (본문 `plugin` 단어)
- `docs/process/00-agent-roster.md`
- `docs/agents/*.md` — 10 개 페르소나 파일
- `AGENTS.md` (루트)
- `README.md` (루트)

**Rust 코드 (현재 P2A 스캐폴드만 있음)**:
- `crates/gadgetron-core/src/agent/tools.rs` — `McpToolProvider`, `Tier`, `ToolMode`, `ToolSchema`, `ToolResult`, `McpError` rename
- `crates/gadgetron-core/src/agent/config.rs` — `[agent.tools]` → `[agent.gadgets]` 설정 경로
- `crates/gadgetron-penny/src/registry.rs` → `gadget_registry.rs`
- `crates/gadgetron-penny/src/mcp_server.rs` → `gadget_server.rs`
- `crates/gadgetron-knowledge/src/mcp/` → `src/gadgets/`

**크레이트 구조 재편 (P2B 착수 시)**:
- `plugins/` 디렉토리 → `bundles/` 로 변경
- `plugins/plugin-ai-infra/`, `plugins/plugin-cicd/`, `plugins/plugin-server/`, `plugins/plugin-document-formats/`, `plugins/plugin-web-scrape/` → `bundles/<name>/`

### 리뷰 권고

- `@chief-architect`: `Bundle` trait 및 `BundleContext` 에 `drivers.*` 네임스페이스가 D-12 leaf 원칙 및 `gadgetron-core` 의존 DAG 준수. `register_entity_kind` 위치 재검토 (drivers.entity_kinds 로 이전)
- `@security-compliance-lead`: Gadget runtime enum 에 `Subprocess`/`Http`/`Wasm` 추가로 인한 trust boundary 변경. 외부 runtime 은 별도 doc 12 에서 threat model
- `@qa-test-architect`: rename PR 에 `cargo test --all-features` 통과 조건. Config migration (`[plugins.*]` → `[bundles.*]`) 에 대한 backward-compat 테스트
- `@dx-product-lead`: `gadgetron install <name>` alias 가 error path 에서 "bundle 을 install 했다" 고 명시하는지 (에러 메시지가 축약 형태를 숨기지 않음)

### 비용 견적

| 항목 | 공수 |
|---|---|
| ADR + glossary 초안 | 0.5 일 (본 커밋에서 완료) |
| Rust 타입 rename (workspace-wide) | 1 일 — 스캐폴드만 있어 컴파일러가 전부 잡아줌 |
| 설계 문서 rename + 본문 치환 | 1 일 |
| 에이전트 페르소나 + 프로세스 문서 rename | 0.5 일 |
| 디렉토리 이전 `plugins/` → `bundles/` | 0.5 일 (P2B 초반 별도 PR) |
| 리뷰 + 머지 | 1 일 |
| **총합** | **~3.5 일** (한 엔지니어) |

### 시행 순서

1. **지금 (본 커밋)**: ADR-P2A-10 + glossary + 본 D-entry 랜딩 → 승인 고정
2. **다음 PR**: Rust 심볼 rename (`BackendPlugin` → `Bundle` 등 workspace 전체)
3. **다음 PR**: 설계 문서 파일 rename + 본문 치환
4. **다음 PR**: 에이전트 페르소나 + 프로세스 문서 치환
5. **P2B 초반**: 디렉토리 재편 (`plugins/` → `bundles/`)
6. **P2B 중반**: `docs/design/phase2/12-external-gadget-runtime.md` 신설 (subprocess/http/container/wasm runtime 상세) → graphify pilot

---

## D-20260418-05: Driver → Plug rename (ADR-P2A-10 amendment)

**날짜**: 2026-04-18
**유형**: Architecture / Naming amendment (사용자 직접 지시, 2026-04-18 세션 동일일 수정)
**상태**: 🟢 승인 (♻️ supersedes D-20260418-04 의 "Driver" 명칭 부분)
**관련 문서**: `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md` (rename 됨, 구명칭 `ADR-P2A-10-bundle-driver-gadget-terminology.md`), `docs/architecture/glossary.md` (rename 반영됨)
**Supersedes**: D-20260418-04 의 Axis A 명칭만 수정. Bundle / Gadget / 모든 다른 결정 (trait rename, CLI 구조, config 마이그레이션 방식) 은 **유효**.

### 배경

D-20260418-04 확정 직후, 5 에이전트 병렬 리뷰에서 codex-chief-advisor 가 MAJOR finding 으로 "Driver" 의 의미 오버로드 (kernel driver / JDBC / ODBC) 를 지적. "Driver" 는 하드웨어 호환성 계약을 암시하는 의미가 강하여, 단순 Rust trait 구현체를 가리키는 용도로는 과한 connotation.

사용자 세션에서 "Plug / Gadget" 을 새 후보로 제시. 검토 결과:

- `Plug` 는 "port 에 꽂는다" 전기/메카닉 메타포로 Rust trait 구현이 core-defined trait 에 꽂히는 동작과 정확히 일치
- `Plug` 는 Rust / 시스템 / ML 생태계에서 overload 없음
- `Plug` + `Gadget` 이 일관된 workbench 메타포 (plug 는 socket 에, gadget 은 Penny 손에)
- `gadgetron plug list` — CLI verb + noun 둘 다 한 단어
- D-20260418-04 의 3 개 서브 결정 (T1 3-concept 분리, T2 Rust symbol rename, T3 CLI/config/dir rename) 은 전부 그대로 유효. 단지 "Axis A 명칭" 만 Driver → Plug

동일일(2026-04-18) 내 수정이므로 D-20260418-04 를 in-place 수정하지 않고 별도 D-entry 로 기록 (append-only 정책).

### 결정

- Axis A (core 확장) 명칭: **Driver → Plug**
- Rust symbol: `ctx.drivers.*` → `ctx.plugs.*`, `BundleContext::drivers` field → `BundleContext::plugs`, `DriverProvider` (Alt 4 에서만 언급된 hypothetical) → `PlugProvider`
- CLI: `gadgetron driver list|info` → `gadgetron plug list|info`
- 디렉토리 영향 없음 (bundle 디렉토리명은 도메인 기반이지 driver/plug 접두어 아님)
- 문서 파일명: `ADR-P2A-10-bundle-driver-gadget-terminology.md` → `ADR-P2A-10-bundle-plug-gadget-terminology.md`

Bundle / Gadget / 모든 Rust trait rename (McpToolProvider→GadgetProvider 등) / config 마이그레이션 전략 / 외부 runtime 설계 / 기타 D-20260418-04 결정은 **변경 없음**.

### 반려 옵션

- **Driver 유지 + glossary 주석만 추가**: 주석 한 줄로 overload 완화 가능하지만, 매 design doc 가 매번 "이 Driver 는 kernel driver 아님" 재설명하는 비용이 장기로 누적
- **사용자 제안 "core-gadget / mcp-gadget" 통일**: 브랜드 통일성은 있으나 CLI 2-word verbose + Rust `trait McpGadgetProvider` cumbersome + "MCP" 를 domain 어휘로 다시 불러들임 (미래 non-MCP 에이전트 프로토콜 수용 시 misnomer)
- **Adapter 로 교체**: hexagonal 정통이지만 "adapter-blob-s3" 어색, 일반 디자인 패턴 단어
- **Backend, Card, Engine 등 기타**: 각각 front/back 프레이밍 / 하드웨어 과잉 / 너무 무거운 어감

### 사용자 결정

**2026-04-18**: Plug / Gadget 승인.

### 영향 받는 문서/파일

**신규/수정**:
- `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md` — rename (구: `-driver-`), Amendment §추가 + 본문 Driver→Plug 치환
- `docs/architecture/glossary.md` — Driver 엔트리 → Plug, 모든 교차참조 업데이트
- `docs/process/04-decision-log.md` — 본 D-entry 추가

**코드 수정 (P2B 착수 전)**:
- `crates/gadgetron-cli/src/main.rs` — `Driver` 서브커맨드 enum variant → `Plug`, `cmd_driver_list_stub` → `cmd_plug_list_stub`
- `crates/gadgetron-core/src/agent/mod.rs` — 모듈 doc comment 의 "Bundle / Driver / Gadget" → "Bundle / Plug / Gadget"
- `crates/gadgetron-core/src/agent/tools.rs` — 모듈 doc comment 에서 "Drivers and/or GadgetProviders" → "Plugs and/or GadgetProviders"
- `crates/gadgetron-knowledge/src/lib.rs` — 같은 패턴 doc comment

**미래 P2B 설계 반영**:
- `docs/design/phase2/06-bundle-architecture.md` (기존 `06-backend-plugin-architecture.md` 에서 rename 예정) — `PlugContext` 대신 `BundleContext::plugs.*` 를 Driver 대체로
- `docs/design/phase2/11-raw-ingestion-and-rag.md` — `ctx.register_extractor(...)` 가 `ctx.plugs.extractors.register(...)` 가 됨을 §4.4 PluginContext 확장 설명에서 반영

### 리뷰 권고

이 amendment 자체는 재-리뷰 불필요 (naming-only, 구조 불변). 단, `@chief-architect` 는 rename PR 병합 전 `ctx.plugs.*` 필드 네이밍이 향후 `BundleContext` 구현 시 clippy `needless_pub_self` / `large_enum_variant` 트리거하지 않는지 확인.

### 비용

추가 rename 공수 ≈ 0.5 일:
- ADR + glossary + decision log 3 개 doc = 본 커밋에서 완료
- CLI 코드 5 hit + 3 doc comment 수정 = 15 분
- `cargo check` + `cargo clippy` 재검증 = 5 분

D-20260418-04 의 3.5 일 견적 위에 누적 → 총 ~4 일 여전히 한 엔지니어 분기 내 흡수 가능.

---

## D-20260418-06: Team synthesis → ADR-P2A-10-ADDENDUM-01 rev3 (RBAC granularity 최종화)

**날짜**: 2026-04-18
**유형**: Architecture / Team convergence (PM-directed 2 라운드 팀 논의 + rev3 라운드 3 피드백 통합)
**상태**: 🟢 승인 (rev3 의 18 항목 decision matrix 만장일치 팀 수렴)
**관련 문서**: `docs/adr/ADR-P2A-10-ADDENDUM-01-rbac-granularity.md` (rev3)
**Supersedes**: D-20260418-04 / -05 의 Open question 3 건을 decision 으로 전환. Bundle/Plug/Gadget trinity 는 유효.

### 배경

D-20260418-05 (Driver→Plug amendment) 직후, security-compliance-lead 가 ADR-P2A-10-ADDENDUM-01 v1 을 authoring 하며 3 개 open question 을 PM 에게 위임. PM 은 팀 convergence meeting (6 에이전트 라운드 1 + 3 에이전트 라운드 2 검수) 을 통해 10 개 결정을 수렴.

### 수렴된 10 결정

| # | 항목 | 팀 입장 | 해결 |
|---|---|---|---|
| 1 | 3축 RBAC (Bundle / Gadget / Plug) | 만장일치 | Ship — §1 |
| 2 | P2B per-deployment, `tenant_overrides` P2C reserved | 5 찬성 + 1 minority (xaas) | Ship — §2 |
| 3 | `requires_plugs` cascade P2B-alpha ship | 4 찬성 + 2 이견 (security/dx 는 beta 선호, 최종 승복) | Ship — §3 |
| 4 | `PlugId` newtype + `#[must_use] RegistrationOutcome` | chief-architect 원안 만장일치 승인 | Ship — §4 |
| 5 | 외부 런타임 5 enforcement points | 만장일치 | Ship — §5 |
| 6 | `admin_detail: Option<String>` leak-safety | xaas 제안 만장일치 | Ship — §5 |
| 7 | JSONB `external_runtime_meta` additive migration | xaas 제안 만장일치 (category overloading 대신) | Ship — §5 |
| 8 | `GadgetronBundlesHome` resolver (4-단계 priority chain) | devops 제안 만장일치 | Ship — §7 |
| 9 | `bundle info` 3 컬럼 (NAME/PORT/STATUS) + `--json` | dx + devops 합의 | Ship — §6 |
| 10 | CLI 는 config-only, `--dry-run` TOML 프린터만 추가 | dx 권고 만장일치 | Ship — §11 |

### 라운드 2 검수 결과 (rev2 대상)

- **chief-architect**: APPROVE (4 Rust 변경 전부 §4 에 반영)
- **xaas**: REQUEST CHANGES → 3 targeted edits (last_synced_at, FD-open step 3b, P2B admin annotation) → rev3 에 반영
- **codex-chief-advisor**: BLOCK 3 CLOSED (외부 2차 검증 통과) + 3 MAJOR + 3 MINOR 잔여 concerns → rev3 에 전부 반영

### 라운드 3 (rev3) 변경사항

- §2: `tenant_overrides` `info!` → `warn!` + `[features] tenant_plug_overrides_accepted_as_reserved` 토글 + CFG-045 startup gate
- §3: `cargo xtask check-bundles` lint CI gate (codex MAJOR 1)
- §5: Enforcement floors 6 (Resource ceilings: RLIMIT + cgroup) + 7 (Egress policy: default-deny + namespaced nftables) (codex MAJOR 2)
- §5: `admin_detail` → `Denied`/`Execution` 확장 + `render_gadget_error_for_caller` 단일 choke-point + regression test (codex MAJOR 3)
- §6: `TENANT OVERRIDES` 테이블 헤더 annotation + 행별 `(reserved — not enforced until P2C)` 접미사 (xaas round-2 + codex MINOR 5)
- §7: `tracing::info!(tier = …, resolved_path = …)` 해상도 로깅 (codex MINOR 6)
- §8: `tenant_workdir_quota.last_synced_at TIMESTAMPTZ` (xaas round-2)
- §8: deletion cascade step 3b: `/proc/<daemon_pid>/fd` scan + 30s backoff (xaas round-2)
- STRIDE **I**: timing-oracle 위협 명시 + `render_gadget_error_for_caller` constant-time padding (codex MINOR 4)
- Consequences: 테스트 9 → 13, 공수 5 → 6 engineer-days

### 파급 문서

**수정**:
- `docs/adr/ADR-P2A-10-ADDENDUM-01-rbac-granularity.md` — v1 → rev2 (in-place, 11 섹션)
- `docs/architecture/glossary.md` — `PlugId`, `RegistrationOutcome`, `GadgetronBundlesHome` 용어 추가 예정

**신규 (P2B-alpha 착수 시)**:
- `crates/gadgetron-xaas/migrations/20260418000001_external_runtime_meta.sql` — JSONB additive
- `crates/gadgetron-core/src/bundle/` — 새 모듈 (id, context, registry, home)
- `crates/gadgetron-testing/src/mocks/bundle/` — FakeBundle, FakePlugRegistry, FakeTenantContext
- `docs/design/phase2/12-external-gadget-runtime.md` — contract floor 5 points 에 따라 작성

### 리뷰 권고

라운드 2 수렴 meeting 결과로 재-review 불필요. P2B-alpha 착수 시 구현 PR 별 기존 rubric 적용.

### 비용

rev2 작성 자체 = 본 커밋 (0.5 일). P2B-alpha 테스트 계획 9 tests + 3 fakes + 1 helper = ~5 engineer-days (qa 추정).

---

## D-20260418-07: P2B-alpha 실행 계획 — 4-week DAG + 5팀 sign-off

**날짜**: 2026-04-18
**유형**: Execution plan / Team convergence (라운드 4: 5 에이전트 병렬 실행계획 수립)
**상태**: 🟢 승인 (5 에이전트 전원 CONDITIONAL GO, 2 security MUST-LAND gate + 3 precondition 명시)
**관련 문서**: D-20260418-04/-05/-06, `docs/adr/ADR-P2A-10-ADDENDUM-01-rbac-granularity.md` (rev3)
**Supersedes**: N/A (신규 실행계획)

### 배경

ADR-P2A-10-ADDENDUM-01 rev3 팀 수렴 완료 후 P2B-alpha 실행 단계 진입. 5 도메인 에이전트 (chief-architect / devops / qa / xaas / security) 가 각 도메인 주간별 deliverable + 의존성 + 리스크 + sign-off 병렬 제출. 본 D-entry 는 통합 계획.

### 5 에이전트 sign-off 결과

| 도메인 | 에이전트 | 판결 | 조건 |
|---|---|---|---|
| Core Rust | chief-architect | 🟢 YES conditional | (i) qa W1 fakes delivery, (ii) devops xtask ownership, (iii) xaas migration by W2 |
| DevOps/SRE | devops-sre-lead | 🟢 CONDITIONAL GO | K8s managed cluster 에서 `egress.enforceMode: "proxy-only"` 경로 CI 검증 필요 |
| QA Testing | qa-test-architect | 🟢 CONDITIONAL GO | (i) Bundle trait W2 freeze, (ii) `render_gadget_error_for_caller` choke-point gate W3, (iii) `PROPTEST_CASES=1024` CI env 추가 |
| XaaS Platform | xaas-platform-lead | 🟢 CONDITIONAL GO | (i) CFG-045 startup gate 확정 (warn vs error), (ii) macOS lsof fallback 패턴, (iii) `WorkdirPurgeJob` design review |
| Security | security-compliance-lead | 🟡 MAJOR — conditional | **2 MUST-LAND before P2B-alpha release tag**: audit sink wiring + JSONB strict-schema validator |

### 통합 4-week DAG

**W0 (pre-sprint, 1-2 days)** — 선결 조건 5건 처리 (§Preconditions 참조)

**W1 — foundations (8 dev-days across 4 agents)**
- chief-architect: `gadgetron-core/src/bundle/{mod,id,manifest,toml_parse,home}.rs` + `AppConfig` 확장
  - Exit: `plug_id_rejects_uppercase`, `bundle_manifest_parses_requires_plugs`, `bundles_home_resolver_fail_closed_on_root_home`, `tenant_overrides_without_ack_toggle_refuses_startup_cfg_045`, `app_config_accepts_runtime_limits` green
  - Deliverable freeze: `BundleManifest` 공개 schema + `GadgetronError::{Config, Bundle}` variants
- qa: `gadgetron-testing/src/mocks/bundle/` 4 fake + `tracing_test` helper — stub-only (Bundle trait W2 bind 예정)
- xaas: `20260418000001_external_runtime_meta.sql` (JSONB additive) + `20260418000002_tenant_workdir_quota.sql` (`last_synced_at` 포함) land
- security: `target: "gadgetron_audit"` append-only sink 설계 draft (`docs/design/phase2/12-external-gadget-runtime.md` §Audit-sink)

**W2 — core mechanism (5 dev-days)**
- chief-architect: `Bundle` trait + `BundleContext` + `PlugRegistry<T>` + `#[must_use] RegistrationOutcome` + `trybuild` compile-fail regression
  - **W2 end freeze**: `Bundle` / `BundleContext` / `PlugRegistry` / `RegistrationOutcome` API. 이후 변경은 supersedes D-entry 필수
- qa: W1 fakes 에 실제 Bundle trait bind + 4 contract tests (`plug_disabled_by_config_is_not_registered`, `is_plug_enabled_returns_correct_tristate`, `bundle_disabled_takes_precedence_over_plug_override`, `bundle_plug_toml_subsection_parses_with_defaults`)
- security: audit sink 구현 + `RuntimeMetaV1` strict serde deserializer + `external_runtime_meta_rejects_unvalidated_jsonb` regression test

**W3 — dispatch + query surface (5 dev-days)**
- chief-architect: `BundleRegistry::list_plugs/list_gadgets` + `requires_plugs` cascade resolver + AST helper API
- qa: cascade 테스트 + redaction 테스트 (`gadget_not_available_hides_admin_detail_from_non_admin` 매트릭스) + external runtime 테스트 + resource ceiling 테스트 (4건)
- devops: `cargo xtask check-bundles` lint + `.github/workflows/check-bundles.yml` + CI workflow 분할 (`test-cpu` / `test-integration`) + Helm values 업데이트 + PVC template
- xaas: `WorkdirPurgeJob` 구현 + `FdScanner` trait + Linux/macOS 구현체
- security: 2 MUST-LAND gate 검증 pass

**W4 — hardening + ship gate (3 dev-days)**
- qa: PBT 2건 (`is_plug_enabled_is_pure_function_of_config`, `authenticated_context_survives_serialization_roundtrip`) + CI gate 통합
- devops: Prometheus metrics scrape + Grafana dashboard JSON + PagerDuty alert rule
- security: 최종 STRIDE re-pass + secret redaction layer 확인

**총 소요**: ~21 dev-days, 4 에이전트 병렬 가동 기준 ~5 calendar-week

### Preconditions (W1 시작 전 해결)

| # | 항목 | 주인 | PM 결정 필요? |
|---|---|---|---|
| P1 | CFG-045 startup gate 동작 확정 | xaas | ✅ **YES** — "warn 후 계속" vs "error 후 startup 실패". 기존 배포 TOML 에 `[tenant_overrides]` 가 없으므로 **error 권고** |
| P2 | macOS dev 의 FD-scan fallback 패턴 | xaas | ✅ **YES** — `GADGETRON_SKIP_FD_SCAN=1` env + noop scanner 기본 vs `/usr/sbin/lsof` first-choice. 권고: 환경변수 + noop fallback |
| P3 | `WorkdirPurgeJob` job runner 설계 | xaas + chief-architect | 공동 리뷰 (PM 결정 불필요, chief-architect 트레이트 리뷰) |
| P4 | `PROPTEST_CASES=1024` `PROPTEST_SEED=42` `INSTA_UPDATE=no` CI env | devops | PM 결정 불필요 |
| P5 | `cargo xtask check-bundles` ownership | chief-architect (AST helper) + devops (CI gate wiring) | 공동 완료, PM confirm only |

### 2 Security MUST-LAND gates (P2B-alpha release tag 전 필수)

1. **Audit sink wiring** (STRIDE R MED) — `target: "gadgetron_audit"` 이벤트가 append-only persistent storage (DB audit table 또는 signed log shipper) 로 흘러야 함. SOC2 CC6.6 evidence 가 stdout tracing 으론 불충분. `docs/design/phase2/12-external-gadget-runtime.md` §Audit-sink 또는 별도 audit-persistence spec 필수.
2. **JSONB strict-schema validator** (STRIDE T MED) — `external_runtime_meta` insert path 가 `RuntimeMetaV1` deny-unknown-fields deserializer + string-length cap 적용. Regression test `external_runtime_meta_rejects_unvalidated_jsonb` 포함.

두 gate 는 **code-complete blocker 아님, release-tag blocker**. 외부 유저가 external-runtime Gadget 건드리기 전에만 land 하면 됨.

### Cross-team dependency matrix

| From → To | 산출물 | 시점 | 블로커? |
|---|---|---|---|
| xaas → chief-architect | migrations `20260418000001`/`000002` | W1 end | YES (sqlx compile-time check) |
| qa → chief-architect | `FakeBundle` + `FakeTenantContext` stubs | W1 end | partial (chief-architect 가 `FakePlugRegistry` 자체 ship 가능) |
| chief-architect → qa | `Bundle` trait freeze | W2 end | YES |
| chief-architect → xaas | AST helper `BundleManifest::plug_callsites_required()` API | W3 start | YES (devops `cargo xtask check-bundles` 가 의존) |
| chief-architect → devops | `BundleRegistry::list_plugs/gadgets` API | W3 mid | YES (`gadgetron bundle info` CLI 의존) |
| devops → chief-architect | `xtask/src/check_bundles.rs` scaffold | W3 start | partial (lint 미탑재 시 release block) |
| security → xaas | `RuntimeMetaV1` deserializer spec | W2 end | YES (audit writer 통합) |
| security → all | audit sink 설계 doc | W1 end | YES (모든 팀의 tracing 타겟이 여기로 흘러가야 함) |

### Risk register (consolidated)

| # | 리스크 | impact | likelihood | 주인 | 완화 |
|---|---|---|---|---|---|
| R1 | `Bundle` trait shape 가 W3 중 변경됨 | med | med | chief-architect | W2 end `#[non_exhaustive]` freeze + 2 real Bundle prototype 리뷰 |
| R2 | D-12 leaf violation | high | low | chief-architect | `deny.toml` + `cargo tree` snapshot CI gate |
| R3 | `#[must_use] RegistrationOutcome` 무시됨 | med | med | chief-architect | `trybuild` 컴파일-실패 회귀 + Bundle PR 체크리스트 |
| R4 | `cargo xtask check-bundles` false-positive 로 dev 차단 | med | med | devops | `--warn-only` 모드 3 sprint 관찰 후 hard-block 전환 |
| R5 | K8s managed cluster (GKE Autopilot, EKS Fargate) nftables 미허용 | high | high | devops | `egress.enforceMode: "nftables"\|"proxy-only"` Helm switch + startup warn |
| R6 | PVC 백업 미지원으로 tenant workdir 손실 | med | med | devops | `values.yaml` 주석 ephemeral hot data 명시, P2B-beta 에 VolumeSnapshot CRD |
| R7 | macOS dev 와 Linux CI 경험 차이로 PR 품질 저하 | med | med | devops | Linux CI job 이 merge 차단, Docker on macOS runbook 제공 |
| R8 | Bundle trait 변경에 fragile 한 테스트 | med | med | qa | `FakeBundle::new(enabled)` 단일 factory 공개, behavior 단위 assert |
| R9 | `render_gadget_error_for_caller` 우회 테스트 누락 | high | low | qa | CI grep gate + `cargo llvm-cov` function-level guard |
| R10 | 576개 기존 테스트 회귀 | med | low | qa | 매 PR `cargo test --workspace` 별도 status check |
| R11 | `external_runtime_meta` JSONB attacker XSS (저장) | med | med | security | `RuntimeMetaV1` strict deserializer + length caps (MUST-LAND #2) |
| R12 | Audit stdout tracing 의 compliance 불충분 | med | med | security | append-only DB sink (MUST-LAND #1) |
| R13 | Provider API key leakage | med | med | security | Tracing subscriber redaction layer day 1 |
| R14 | lsof 미존재 시 purge 진행 허용 (macOS dev) | low | med | xaas | `warn!` emit + env var skip, Linux /proc 경로는 안전 |

### 3 PM 결정 대기

팀 수렴의 마지막 공백:

1. **P1 — CFG-045 동작**: warn 후 계속 vs error 후 startup 실패. **팀 권고: error (기존 배포 영향 없음)**.
2. **P2 — macOS FD-scan fallback**: `GADGETRON_SKIP_FD_SCAN=1` env + noop scanner 기본 vs `/usr/sbin/lsof` first-choice 자동 감지. **팀 권고: 환경변수 + noop fallback (개발자 경험 우선)**.
3. **External runtime doc 12 신설 여부 + 담당**: security MUST-LAND gate 들이 doc 12 에 안착해야 함. 누가 언제 작성? **팀 권고: security-compliance-lead 가 W1 end 까지 draft, chief-architect + xaas 가 W2 리뷰**.

### 비용

| 항목 | dev-days |
|---|---|
| chief-architect W1+W2+W3 | 8 |
| qa W1+W2+W3+W4 | 6 |
| xaas W1+W3 (migrations, job, FdScanner) | 4 |
| devops W3+W4 (CI, Helm, Observability) | 2 |
| security W1+W2+W4 (sink design + validator + redaction + STRIDE) | 3 |
| Cross-review + rubber ducking + bug fixes | 2 |
| **Total** | **~25 dev-days** |

4 agent 병렬 (PM 제외) 기준 ~**5 calendar-week** (1 달 + 1 주), buffer 포함 **6 주** 권고.

### 시행 순서

1. **지금 (W0 day 1)**: 본 D-entry land + 3 PM 결정 확정 + `docs/design/phase2/12-external-gadget-runtime.md` 스텁 생성 요청 (security)
2. **W0 day 2-3**: P1/P2/P3 preconditions 해결 + 팀별 W1 PR 브랜치 오픈
3. **W1**: 병렬 실행, end-of-week demo (chief-architect W1 exit + qa stubs + xaas migrations + security sink design)
4. **W2**: Bundle trait freeze (W2 mid), W2 end freeze ceremony
5. **W3**: dispatch + query + CI gates + WorkdirPurgeJob
6. **W4**: hardening + release-tag MUST-LAND gates verify + P2B-alpha tag

---

## D-20260418-08: P2B-alpha 선결 조건 3건 확정 (CFG-045 / FD-scan / doc 12)

**날짜**: 2026-04-18
**유형**: Execution precondition (D-20260418-07 §Preconditions 의 PM 결정 마감)
**상태**: 🟢 승인 (팀 권고 그대로 채택)
**관련 문서**: D-20260418-07

### 결정

**P1 — CFG-045 startup gate 동작**: **error + startup 실패** (팀 권고 채택)
- `[tenant_overrides]` 스탠자가 존재하는데 `[features] tenant_plug_overrides_accepted_as_reserved = true` 토글이 없으면 `GadgetronError::Config("CFG-045: ...")` 반환하고 startup 실패
- 근거: 기존 배포 TOML 에 `[tenant_overrides]` 가 존재할 가능성 0 (P2B-alpha 에 새로 도입되는 필드). 실수로 설정한 operator 가 silently 동작 안 하는 것보다 명시적 실패가 안전
- CHANGELOG 에 업그레이드 가이드 포함 (stanza 제거 or 토글 활성화)

**P2 — macOS dev FD-scan fallback**: **`GADGETRON_SKIP_FD_SCAN=1` env + noop scanner 기본**
- macOS 개발 환경에서 `lsof` 경로 편차 (`/usr/sbin/lsof` vs `/opt/homebrew/bin/lsof`) 때문에 환경변수로 skip 가능하게
- 환경변수 설정 시 `FdScanner::is_path_fd_open` 이 `Ok(false)` 반환 + `warn!("FD scan skipped via GADGETRON_SKIP_FD_SCAN")` emit
- Linux 프로덕션은 `/proc/<pid>/fd` 경로 사용 (env 무시 권고 또는 별도 prod 모드 플래그)

**P3 — doc 12 (external runtime) 신설 주인 + 타임라인**: **security-compliance-lead 가 W1 end draft, chief-architect + xaas 가 W2 리뷰**
- 파일: `docs/design/phase2/12-external-gadget-runtime.md`
- 포함 내용: 2 MUST-LAND gate (audit sink 영속화 + `RuntimeMetaV1` JSONB validator) + 7 enforcement floors 의 wire protocol 세부 + subprocess/HTTP/container/wasm runtime 별 STRIDE
- W1 end (5 working days from now): draft v0 (목차 + 2 MUST-LAND 스펙)
- W2 end: v1 (chief-architect + xaas 리뷰 반영)
- W3+ 의 구현 PR 이 이 문서를 참조

### 영향

- P1: `crates/gadgetron-core/src/config.rs` 에 CFG-045 에러 경로 추가 필요 (W1 구현 범위 포함)
- P2: `crates/gadgetron-xaas/src/jobs/fd_scanner.rs` 의 `FdScanner` trait 에 env-var 체크 포함 (W3 구현 범위)
- P3: security-compliance-lead 의 W1 deliverable 에 doc 12 draft 포함

---

## D-20260418-09: W2 kickoff 팀 수렴 + 실행 플랜

**날짜**: 2026-04-18
**유형**: Execution plan (W1 머지 직후, W2 착수 전 4-agent review)
**상태**: 🟢 승인 (4 agents: chief-architect APPROVE, qa APPROVE, security CONDITIONAL OK, codex APPROVE WITH CONCERNS)
**관련 문서**: `docs/adr/ADR-P2A-10-ADDENDUM-01-rbac-granularity.md` rev4

### 배경

W1 (PR #62 `aa080de`) 머지 완료 후 W2 (`Bundle` trait + `BundleContext` + `PlugRegistry` + `#[must_use] RegistrationOutcome`) 착수 전 4-agent 리뷰. rev3 까지는 개념 수렴, rev4 는 구현 직전 shape 확정.

### 수렴된 결정 (7건)

1. **W2 단일 PR** — codex 권고 채택 (trait + context + registry + outcome 을 같이 freeze 해야 shape 검증). chief-architect 의 2-PR 분할안 기각.
2. **`BundleRegistry` metadata-only** (codex MAJOR 1) — live `dyn Bundle` 은 `install` 후 drop, `Vec<Arc<dyn Bundle>>` 금지. metadata+inventory+status 만 저장.
3. **`SkippedByAvailability { missing: Vec<PlugId> }`** (codex MAJOR 2) — 운영 debugging 위해 누락된 plug IDs 캐리.
4. **Field-form style 통일** (codex MAJOR 3) — `ctx.plugs.*` + `ctx.gadgets.*` (method form `gadgets_mut()` 폐기). ADR §4 + glossary 동기화.
5. **Security 6 deliverables 필수** (security-compliance-lead CONDITIONAL OK):
   - (a) `catch_unwind` around `Bundle::install` in `BundleRegistry::install_all` (DoS 방어)
   - (b) `BundleRegistry` duplicate-id rejection
   - (c) `register()` log field whitelist — `bundle`, `plug`, `axis` 만. Arc<T> Debug 금지
   - (d) `let _` audit completeness (discarded outcome 도 tracing 발행 guarantee)
   - (e) `CoreAuditEvent::PlugSkippedByConfig` structured variant — Gate 1 MUST-LAND wire freeze preview
   - (f) `Bundle` trait rustdoc trust 제약 3건 명시 (P2B in-tree only / audit target operator-only / in-core full-trust)
6. **테스트 2건 rename** (qa):
   - `is_plug_enabled_returns_correct_tristate` → `is_plug_enabled_reflects_bundle_and_plug_axes`
   - `bundle_plug_toml_subsection_parses_with_defaults` → `plug_override_omitting_enabled_defaults_to_true`
7. **`trybuild` W2 defer** — `clippy -D warnings` 가 `#[must_use]` 를 이미 error 로 승격하므로 W2 에서는 문서+inline test 만. trybuild 는 W3 compile-fail 배치에 포함.

### 예상 범위

| 항목 | LOC |
|---|---|
| `gadgetron-core::bundle::trait_def` (`Bundle` trait + `BundleDescriptor` + `DisableBehavior`) | ~120 |
| `gadgetron-core::bundle::context` (`BundleContext` + `PlugPredicates` + `PlugHandles`) | ~180 |
| `gadgetron-core::bundle::registry` (`PlugRegistry<T>` + `RegistrationOutcome`) | ~150 |
| `gadgetron-core::bundle::bundle_registry` (metadata-only + `catch_unwind` + duplicate-id) | ~120 |
| `gadgetron-core::audit::event` 확장 (`CoreAuditEvent::PlugSkippedByConfig`) | ~40 |
| 테스트 8건 (4 contract + 4 security) | ~380 |
| **총** | **~990 LOC** |

### 시행 순서

1. 본 D-entry + ADR rev4 + glossary 수정 커밋 (docs-only)
2. chief-architect delegation: trait + registry 구현 + 8 tests
3. `cargo check` + `clippy` + `test -p gadgetron-core`
4. `cargo fmt`
5. Feature branch `p2b-alpha/w2-bundle-trait` → push
6. PR + CI
7. Admin override merge (W1 precedent 과 동일)

### 리뷰 권고

4-agent synthesis 결과로 rev4 와 본 D-entry 수렴됐으므로 W2 구현 PR 은 재-리뷰 불필요. `codex-chief-advisor` 재검증은 W2 PR 머지 후 W3 착수 전에 실시.

### 비용

~4-5 agent-hours (chief-architect W2 구현 + qa fake inline + security 4 regression tests).

---

_(다음 엔트리는 아래에 append)_
