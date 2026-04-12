# Phase C Architecture Review — Consolidation

> **일자**: 2026-04-12
> **대상**: `docs/architecture/platform-architecture.md` v0 (1798 lines, by @chief-architect)
> **방식**: 7 native subagents 병렬 deep-dive review (8 axis 전체)
> **PM**: 메인 에이전트
> **결과**: 모든 reviewer ⚠️ Conditional — v1 작성 전 8개 BLOCKER + 17개 HIGH + 12개 MEDIUM 해소 필요

---

## 🎯 Executive Summary

1. **v0의 골격은 견고하다**. 8 axis 모두 cover됨. 18개 결정 (D-1~13 + D-20260411-01~13) 모두 반영.
2. **하지만 '미구현' 셀이 너무 많다**. 특히 Axis H 성능 모델은 모든 latency budget 구간이 "미구현" 표기 → P99 < 1ms 주장 검증 불가.
3. **v0 자체에 G-1 ~ G-9 (9개) gap이 self-identified**됨. Phase C reviewer들이 추가로 28개 gap 발견.
4. **PM 자율 결정 필요한 사안 ~25건**. 사용자 escalation 필요한 strategic 결정 1건 (K8s operator reconcile 구조 모순).
5. **v1 작성 권고**: chief-architect가 본 consolidation을 input으로 v1 작성, ~2000-2500 lines 예상.

---

## 📋 7 Reviewer 핵심 발견

### 1. @gateway-router-lead (axum/tower 10년)
**결론**: ⚠️ Gap 발견. 3 신규 gap + middleware chain 불일치 발견.

핵심 발견:
- **G-9 (신규)**: `/v1/embeddings` Phase 2 추가 시 `Scope::OpenAiCompat` 자동 허용 여부 미정 → security policy gap
- **G-10 (신규)**: `StreamInterrupted` retry semantic 오류 — partial stream 이후 retry 시 클라이언트 데이터 중복/손상. 1회 retry는 첫 청크 전송 이전에만 의미 있음.
- **G-11 (신규)**: `TenantContext.quota_config` fetch 비용이 latency budget에 누락 → P99 < 1ms 위반 가능
- **Middleware chain**: §2.A.2 평면도와 §5.2 Tower `ServiceBuilder` 코드 불일치. Audit이 Tower Layer가 아닌 handler 코드인데 다이어그램에서는 Layer로 표현.
- **G-6 (TenantContext propagation)** 해결책 제시: **Option B 변형** — `Router::chat_with_context(req, tenant_id: Uuid)` + `MetricsStore::record_*(.., tenant_id)` signature 변경. `LlmProvider` trait 변경 없음.

### 2. @inference-engine-lead (LLM serving 10년)
**결론**: ⚠️ 조건부. Provider lifecycle FSM이 Phase 1 BLOCKER 수준.

핵심 발견:
- **IE-01 [BLOCKER]**: `ProcessManager` FSM 전이 다이어그램 부재. `Loading → Failed` 전이 시 VRAM/포트 반납 미명시. 운영 중 VRAM accounting 버그 필연.
- **IE-02 [HIGH]**: `Loading → Running` readiness probe loop (주기/timeout) 미정. `is_live=true & is_ready=false` 중간 상태 처리 없음.
- **IE-03 [HIGH]**: `StreamInterrupted` retry idempotency가 6 provider 중 cloud (Anthropic, Gemini, OpenAI) 에서 보장 안 됨. 클라이언트 데이터 중복.
- **IE-04 [HIGH]**: Cloud provider `is_ready(model)` semantics — Anthropic/Gemini는 `/v1/models` endpoint 없음. 1-token probe = 비용 발생.
- **IE-05 [MEDIUM]**: Graceful shutdown drain 중 retry race condition.
- **IE-06 [MEDIUM]**: Gemini `PollingToEventAdapter` 입출력 타입 미명세. Week 6 착수 전 결정 필요.
- **`LlmProvider` trait에 `start()/stop()` 없음**: cloud vs local engine 비대칭이 trait 계약에 미반영.

### 3. @gpu-scheduler-lead (NVIDIA GPU/HPC 10년)
**결론**: 2 BLOCKER + 3 HIGH 결함.

핵심 발견:
- **G-2 [BLOCKER]**: VRAM 동기화 프로토콜 미정 (`SchedulerEventChannel` P2 분류). **본인 추천**: **Option C (Hybrid)** — push (NodeAgent → Scheduler, 10초 heartbeat, 30초 stale threshold) + pull (deploy 직전 1회 query). Phase 1으로 격상 필요.
- **MIG 프로파일 테이블 오류 [BLOCKER]**: `gpu-resource-manager.md`의 A100 80GB 테이블에 `1g.5gb` 항목이 있는데 이는 A100 40GB 전용. 프로덕션 MIG 생성 실패 risk.
- **G-1 (Scheduler restart) 본인 답변**: NodeAgent re-registration on startup. `AppConfig.nodes`에서 노드 목록 읽고 `/status` 폴링. 단일 바이너리이므로 `/api/v1/models/recover` 수동 trigger도 제공.
- **NUMA-aware FFD 의사코드 부재** [HIGH]: §2.H.4가 알고리즘 이름만. PCIe-fallback 시 bandwidth 페널티 미반영.
- **`MigManager`가 GPU monitor와 interaction**: MIG instance UUID 기반인데 `PortAllocator`가 `gpu_index: u32`만 받음.
- **CostBased eviction**: cost_score 항상 0 → `Lru`와 동일 동작 (버그). Phase 1에서 빼거나 데이터 소스 연결 필요.
- **NVML polling 주기 불일치**: 아키텍처는 `100ms`, gpu-resource-manager.md는 `1초`. 1초가 정답.

### 4. @xaas-platform-lead (multi-tenant SaaS 10년)
**결론**: ⚠️ Conditional. 3건 v1 전 결정 필요.

핵심 발견:
- **G-6 (TenantContext)** 해결책: **Option B 변형 (gateway-router-lead와 동일 의견)** — `Router::chat_with_context(req, tenant_id)`. `ChatRequest`/`LlmProvider` trait 절대 건드리지 말 것.
- **G-4 (streaming quota timing)** 해결책: **Option B 변형 — Soft cap + 다음 요청 차단 + 1-request grace**. Pre-check estimated → stream 도중 무개입 → post-record (over-cap 허용) → 다음 요청 즉시 429. audit는 always-accurate.
- **PostgreSQL SPOF**: Phase 1 acceptable조건부. 단일 인스턴스 + WAL backup, "PG down → reject all" 정책 명문화 필요. multi-instance 배포 금지 (moka cache 일관성).
- **Multi-tenant isolation**: SQL `WHERE tenant_id = ?` 누락 위험. RLS는 Phase 2, Phase 1은 PgHarness integration test로 cross-tenant data leak 검증 필수.
- **scope denial 401 vs 403 오류 [HIGH]**: `require_scope()` 실패 시 `TenantNotFound → 401`이지만 인증 성공 + 권한 부족은 **403이어야 함**. `GadgetronError::Forbidden` variant 또는 별도 매핑 코드 필요.
- **TokenBucket 재시작 grace**: 프로세스 재시작 시 메모리 초기화 → 분당 quota 우회 가능. Phase 1 known limitation으로 명문화.
- **신규 발견**: `gadgetron_xaas_audit_dropped_total{tenant_id}` 라벨이 cardinality 폭발 위험.

### 5. @devops-sre-lead (K8s/SRE 10년)
**결론**: 5 HIGH 결함, 운영 가능성 부족.

핵심 발견:
- **K8s operator reconcile 구조 모순 [BLOCKER]**: `platform-architecture.md §2.C.4`는 "operator → Scheduler::deploy 호출" 방식, `deployment-operations.md §3.2`는 "operator → Pod 직접 생성" 방식. **두 문서가 정반대**. Phase 2 설계 진입 전 필수 결정.
- **Finalizer + Leader election 부재 [HIGH]**: K8s operator HA 배포 시 race condition. `gadgetron.io/model-cleanup` finalizer + lease-based leader election 누락.
- **runbook 활용성 부족 [HIGH]**: §2.F.1 시나리오 테이블이 "수동 개입" 수준. on-call이 즉시 실행할 명령어 시퀀스 없음.
- **graceful shutdown timing [HIGH]**: 45초 → **70초 권고** (preStop 5초 + drain 30 + audit 5 + model_unload 10 + buffer 20). `terminationGracePeriodSeconds: 70`. SSE 스트림 long-running 고려.
- **Alert rules 전무 [HIGH]**: Prometheus 메트릭 목록은 충실하지만 alert rule 0개. error_rate, gpu_temp, audit_drop, p99_latency, pg_pool_exhaustion 5개 필수.
- **PostgreSQL backup 정책 전무 [HIGH]**: `audit_log = revenue source`인데 pg_dump cron 없음. PVC 손실 = 모든 audit 손실.
- **Secret management Phase 1**: `helm-secrets` 또는 External Secrets Operator 권고. revoked key moka TTL 10분 유예 = 보안 risk 명문화.
- **Health check 분리 불일치**: deployment-operations.md는 `/health` + `/ready` 분리, platform-architecture.md는 `/health`만. 합치 필요.
- **CI sqlx prepare --check 누락**: workflow에 단계 없음.

### 6. @ux-interface-lead (TUI/Web UI 10년)
**결론**: ⚠️ 3 HIGH gap.

핵심 발견:
- **TUI 데이터 소스 경로 미정 [HIGH]**: in-process Arc 공유 vs HTTP self-call이 v0에 명시되지 않음. **본인 추천**: **in-process Arc** (단일 바이너리 원칙, 읽기 전용, auth 우회).
- **Observability Context producer 미정 [HIGH]**: `WsMessage`를 만들고 broadcast channel에 send하는 컴포넌트가 어느 crate인지 미명시. 신규 `WsAggregator` 필요 (gateway 또는 node 결정).
- **`GpuMetrics` vs dashboard.md JSON 불일치 [HIGH]**: dashboard.md는 `cpu_pct`, `ram_used_gb`, `network_rx_mbps` 등 노드 레벨 필드 포함. core/src/ui.rs의 `GpuMetrics`에 없음. `NodeMetrics` 별도 타입 필요.
- **TUI 쓰기 권한 vs 읽기 전용 모순 [MED]**: dashboard.md는 `[d]Deploy [u]Undeploy` 키, 아키텍처는 "읽기 전용". 결정 필요.
- **WS payload bandwidth 미분석 [MED]**: 100 node × 8 GPU × 1KB × 1000 client = 320MB/sec egress. 단일 프로세스 감당 불가. subscription filter 또는 per-node endpoint 필요.
- **`gadgetron-web` crate 위치 미결**: workspace 멤버 vs 별도 npm. dashboard.md는 "Axum 정적 서빙" 언급, 아키텍처는 미명시.
- **WebSocket fan-out 모델** 본인 추천: `tokio::sync::broadcast::channel(1024)`. RecvError::Lagged → drop + reconnect.

### 7. @qa-test-architect (test architecture 10년)
**결론**: ⚠️ Conditional. 6/8 PARTIAL, Axis D + Axis H FAIL.

핵심 발견:
- **GAP-T1 [CRITICAL]**: Audit flow e2e 시나리오 없음. mpsc channel full → drop → warn → counter 경로와 SIGTERM drain 모두 미검증.
- **GAP-T2 [CRITICAL]**: Latency budget 구간별 criterion bench 부재. §2.H.2 모든 셀이 "미구현". P99 1ms regression 발견 시 어느 구간 문제인지 추적 불가.
- **GAP-T3 [HIGH]**: moka LRU cache invalidation 테스트 전략 없음. 키 취소 → 즉시 효력 발생 검증 경로 부재.
- **GAP-T4 [HIGH]**: G-6 (TenantContext propagation) 미결이 per-tenant Prometheus assertion test를 block.
- **GAP-T5 [HIGH]**: `gadgetron-cli` 부팅 시퀀스 + graceful shutdown 통합 테스트 없음.
- **GAP-T6 [MEDIUM]**: G-1 (process restart recovery) regression test 없음.
- **Tracing span assertion 도구 미지정**: `tracing-test` crate 사용 패턴 없음.
- **Prometheus counter assertion helper 부재**: in-process recorder mock 패턴 없음.
- **Chaos engineering**: Phase 1용 lightweight injection (toxiproxy, Docker network disconnect) 전략 없음. Phase 3로 미루기에는 너무 늦음.

---

## 🔍 Cross-Cutting Themes (4)

### Theme 1: G-6 (TenantContext propagation) — 3 reviewer 동시 지적

gateway-router-lead, xaas-platform-lead, qa-test-architect 모두 G-6 해결이 다른 작업을 block한다고 지적. 두 reviewer (gateway, xaas)가 **동일한 해결책 (Option B 변형)** 도출.

**합의 내용**:
- `Router::chat()` → `Router::chat_with_context(req: ChatRequest, tenant_id: Uuid)` signature 변경
- `MetricsStore::record_success(provider, model, latency_ms, tokens, cost, **tenant_id: Uuid**)` 추가
- `LlmProvider` trait 절대 건드리지 않음 (provider는 tenant 무관)
- `ChatRequest`에 tenant_id 추가 금지 (외부 OpenAI 호환 표면 오염 방지)
- `tokio::task_local!` 사용 금지 (테스트 곤란, AuditWriter spawn 경계 문제)

**부작용 (해소 필요)**:
- `MetricsStore` DashMap key cardinality: `(provider, model) → (provider, model, tenant_id)`. 1M entries 가능. **권고**: 1만 entry LRU 제한.

### Theme 2: Streaming Retry Idempotency (G-10 + IE-03)

gateway와 inference-engine 모두 동일 결함 지적.

**문제**: D-20260411-08 "1회 retry"가 partial stream 이후에는 의미 없음. 클라이언트는 이미 청크 N개 수신 → retry 시 처음부터 재생성 → 데이터 중복.

**PM 결정 (PM 자율)**: D-20260411-08 supersede entry 작성 — "retry는 첫 SSE 청크 전송 이전에만 허용. 이후는 `[DONE]` 없이 연결 종료 + audit."

### Theme 3: Performance Budget — 측정 가능성 결여 (Theme 4와 연관)

qa-test-architect가 GAP-T2로 지적, gateway-router-lead가 G-11로 보강.

**문제**: 
- §2.H.2 latency budget 분해 표가 모든 셀 "미구현"
- `gadgetron_xaas_auth_duration_us` 등 메트릭 정의만 있고 측정 코드 없음
- BM-routing, BM-auth-cache 등 criterion bench 미작성
- `quota_config` fetch 경로가 budget에 누락

**PM 결정 (PM 자율)**: v1에서 §2.H.2 모든 셀에 책임 crate + 메트릭 이름 + criterion bench 이름 명시. Week 3-4에 BM-* bench 선행 작성.

### Theme 4: K8s Operator Architecture — strategic 결정 필요 가능성

devops-sre-lead가 BLOCKER로 지적: `platform-architecture.md §2.C.4`와 `deployment-operations.md §3.2`가 K8s operator reconcile loop 구조를 정반대로 기술.

**Option A**: Operator가 Scheduler에 HTTP/gRPC 호출 → Scheduler가 모델 배포 결정. (중앙집중)
**Option B**: Operator가 직접 Pod 생성/관리. Scheduler는 in-binary node 단위만. (K8s native)

**PM 자율 결정**: **Option A** 채택. 이유:
- D-20260411-01 B "단일 바이너리" 철학과 일치
- Scheduler가 NUMA/MIG/VRAM aware → operator가 이를 재구현하면 중복
- Phase 1 = Helm chart (operator 없음), Phase 2 = operator 추가 시 Scheduler API 호출 경로 자연스러움
- deployment-operations.md §3.2가 잘못됨 (operator가 Pod 생성하는 안)

(이 결정이 strategic이라면 사용자에게 묻겠지만, 단일 바이너리 원칙이 이미 D-20260411-01에서 확정되었으므로 PM 자율로 본다.)

---

## 📊 Critical Gap Inventory

### BLOCKER (8 — v1 진입 전 필수 해소)

| ID | Title | 발의자 | PM 결정 |
|---|---|---|---|
| **B-1** | **G-6 TenantContext propagation** | gateway, xaas, qa | Router::chat_with_context + MetricsStore signature 변경 (Option B 변형) |
| **B-2** | **G-2 VRAM sync 프로토콜** | gpu-scheduler | Hybrid push+pull (10s heartbeat / 30s stale / deploy-time pull) |
| **B-3** | **IE-01 ProcessManager FSM 다이어그램** | inference-engine | v1 §2.F.1에 mermaid state diagram 추가, transition guard 명시 |
| **B-4** | **MIG 프로파일 테이블 오류** | gpu-scheduler | `gpu-resource-manager.md` A100 80GB 테이블에서 `1g.5gb` 제거 |
| **B-5** | **K8s operator reconcile 구조 모순** | devops-sre | Option A 채택 (operator → Scheduler 호출). deployment-operations.md §3.2 수정 |
| **B-6** | **GAP-T1 Audit flow e2e 시나리오 부재** | qa | v1 §2.A.4.4 + harness.md 시나리오 6 추가 |
| **B-7** | **GAP-T2 Latency budget bench 부재** | qa | v1 §2.H.2 모든 셀에 메트릭/bench 이름 명시. Week 3-4 선행 작성 |
| **B-8** | **G-9 + scope denial 401→403** | gateway, xaas | `GadgetronError::Forbidden` variant 추가 (D-13 확장). v1에 HTTP mapping 정정 |

### HIGH (17)

| ID | Title | 발의자 | PM 결정 |
|---|---|---|---|
| H-1 | G-10/IE-03 Streaming retry semantic | gateway, inference | "첫 청크 전송 이전 retry만 허용" 정책. D-20260411-08 supersede |
| H-2 | IE-02 readiness probe loop | inference | `is_ready` polling 5초 주기, 5분 timeout, 실패 임계값 3회 |
| H-3 | IE-04 Cloud provider is_ready cost | inference | Cloud provider는 `is_ready=is_live` (1-token probe 금지). doc 명시 |
| H-4 | IE-06 Gemini PollingToEventAdapter 타입 | inference | Week 6 시작 전 명세 — `SseToChunkNormalizer` 입력 = SSE event로 통일. PollingToEventAdapter가 JSON → SSE 변환 |
| H-5 | G-4 streaming quota timing | xaas | Soft cap + 1-request grace + audit always-accurate |
| H-6 | G-11 quota_config fetch latency | gateway | `quota_config`도 moka cache에 포함 (10분 TTL). v1 §2.H.2 budget 추가 |
| H-7 | NUMA-aware FFD 의사코드 부재 | gpu-scheduler | v1 §2.H.4에 의사코드 + NVLink 그룹 우선 + PCIe fallback warning |
| H-8 | CostBased eviction 데이터 소스 | gpu-scheduler | Phase 1에서 `ModelDeployment.priority: i32` 기반 stub 사용. Phase 2에서 cost 데이터 연결 |
| H-9 | NVML polling 주기 불일치 | gpu-scheduler | 1초로 통일. v1 cross-cutting matrix 수정 |
| H-10 | PostgreSQL SPOF 정책 미명문화 | xaas, devops | "PG down → reject all", multi-instance 금지 (moka 일관성) v1에 명시 |
| H-11 | TUI 데이터 소스 (in-process Arc) | ux | in-process Arc 공유. Phase 1 TUI는 읽기 전용 (deploy 키 disabled) |
| H-12 | Observability WsAggregator producer | ux | `gadgetron-gateway/src/observability/aggregator.rs` 신설. ResourceMonitor 폴링 → broadcast send |
| H-13 | GpuMetrics vs dashboard.md 필드 불일치 | ux | `NodeMetrics` 별도 타입 신설 (cpu_pct, ram_used_gb, network_*) |
| H-14 | runbook 활용성 부족 | devops | v1 §2.F에 4개 핵심 시나리오 (G-1, S-1, D-1, P-3) copy-paste 명령어 시퀀스 추가 |
| H-15 | graceful shutdown 70초 상향 | devops | preStop 5s + drain 30s + audit 5s + model_unload 10s + buffer 20s = 70s. `terminationGracePeriodSeconds: 70` |
| H-16 | Alert rules 전무 | devops | v1에 5개 alert rule YAML (error_rate, gpu_temp, audit_drop, p99_latency, pg_pool_exhaustion) |
| H-17 | Finalizer + Leader election | devops | v1 §2.C.4에 finalizer 패턴 + lease-based leader election 추가 (Phase 2) |

### MEDIUM (12)

- M-1: GAP-T3 moka cache invalidation 테스트 → harness.md에 시나리오 추가
- M-2: GAP-T4 G-6 미결로 testability block → B-1 해소 후 자동 해소
- M-3: GAP-T5 cli 부팅 시퀀스 테스트 → harness.md에 통합 테스트 추가
- M-4: GAP-T6 process restart recovery 테스트 → harness.md에 시나리오 추가
- M-5: IE-05 graceful shutdown drain retry race → 30s drain 후에는 retry 금지 정책
- M-6: TokenBucket 재시작 grace → known limitation 명문화
- M-7: TUI 쓰기 권한 vs 읽기 전용 모순 → Phase 1 disabled (UI에서 hint 표시)
- M-8: WS payload bandwidth 미분석 → v1 §2.H에 분석 추가, subscription filter Phase 2 결정
- M-9: `gadgetron-web` crate 위치 → workspace 멤버 + tower-serve-static embed
- M-10: PostgreSQL backup 정책 → pg_dump CronJob daily, RPO 24h
- M-11: Tracing span assertion 도구 → `tracing-test` crate 추가
- M-12: Prometheus counter assertion helper → `gadgetron-testing::prelude` 추가

### LOW (4)

- L-1: TypeScript codegen 전략 (typeshare/schemars) → Phase 2
- L-2: Security scanning (cargo audit, Trivy) → CI에 추가
- L-3: Helm chart versioning 자동화 → release/* 브랜치
- L-4: TUI keyboard 일관성 + i18n → Phase 2

---

## 🟢 PM 자율 결정 (총 25건)

각 BLOCKER, HIGH, MEDIUM 항목에 대해 PM이 결정. 위 표의 "PM 결정" 컬럼 참조.

**핵심 결정 요약**:

1. **G-6** = `Router::chat_with_context` + `MetricsStore` signature 변경
2. **G-2** = Hybrid push+pull (10s/30s)
3. **K8s operator** = Option A (operator → Scheduler 호출)
4. **Streaming retry** = 첫 청크 이전만 retry, D-20260411-08 supersede
5. **Scope denial** = `GadgetronError::Forbidden` variant 신설 (D-13 추가 확장)
6. **Phase 1 PostgreSQL** = 단일 인스턴스 + WAL backup, multi-instance 금지
7. **TUI** = in-process Arc, 읽기 전용 (Phase 1)
8. **WsAggregator** = `gadgetron-gateway/src/observability/aggregator.rs` 신설
9. **CostBased eviction** = `ModelDeployment.priority` 기반 stub (Phase 1)
10. **Graceful shutdown** = 70초 (preStop 5 + drain 30 + audit 5 + unload 10 + buffer 20)

---

## 🔴 사용자 Escalation 필요 (Strategic)

**없음**. 모든 결정은 PM 권한 범위 내.

만약 사용자가 특히 검토하고 싶은 결정이 있다면:
- **K8s operator 구조 (Theme 4)**: Phase 2 architectural direction 결정. 단일 바이너리 원칙이 이미 확정이라 PM 판단했으나, "operator가 Pod 직접 생성"이 K8s native 패턴이라는 측면에서 사용자가 다른 의견을 가질 수 있음. 알려주시면 재논의.

---

## 📝 v1 작성 plan (chief-architect 지시 사항)

chief-architect가 v0 → v1 update 시 반영해야 할 변경 사항을 섹션별로 정리:

### §2.A (시스템 수준)
- **§2.A.4.4 Audit Flow**: e2e 시나리오 6 추가 (B-6)
- **§2.A.5 Public API**: `/v1/embeddings [P2]` 행 추가 (G-9)
- **§2.A.5**: `/health` + `/ready` 분리 (devops 발견)

### §2.B (크로스컷)
- **§2.B.1 Observability**: `init_observability()` 단일 함수 통합. JSON layer + OTel layer + EnvFilter 합성 코드 skeleton. `/metrics` 9090 포트 확정.
- **§2.B.1**: `tracing-test` + Prometheus assertion helper 추가 노트
- **§2.B.4 Error**: `Forbidden(String)` variant 추가 (D-13 확장). HTTP mapping 표 정정 (TenantNotFound→401, Forbidden→403, QuotaExceeded→429)

### §2.C (배포)
- **§2.C.4 K8s operator**: Option A 명시. Finalizer + Leader election 패턴 추가
- **§2.C.3 Helm**: replicas: 1 고정 이유 명시 (moka cache 일관성)
- **deployment-operations.md §3.2 수정** 별도 작업

### §2.D (상태)
- **§2.D.1**: PostgreSQL backup 정책 (pg_dump daily, RPO 24h)
- **§2.D.2**: moka cache invalidation 테스트 전략 (M-1)
- **§2.D.7**: PG SPOF 정책 명문화 (PG down → reject all, multi-instance 금지)

### §2.E (Phase 진화)
- **§2.E.4**: API stability에 `/v1/embeddings` Phase 2 scope 결정 (G-9)
- **§2.E.5**: TokenBucket 재시작 grace를 known limitation으로 추가

### §2.F (장애 모드)
- **§2.F.1**: `ProcessManager FSM` mermaid state diagram (B-3)
- **§2.F.1**: 4개 시나리오 (G-1, S-1, D-1, P-3) copy-paste runbook (H-14)
- **§2.F.3 Circuit Breaker**: failure_timestamps VecDeque 슬라이딩 윈도우 명시
- **§2.F.6 Graceful Shutdown**: 70초로 상향, preStop 5s 추가 (H-15)

### §2.G (도메인 모델)
- **§2.G.5 TenantContext propagation**: `Router::chat_with_context` + `MetricsStore::record_*` signature 변경 명시 (B-1)
- **§2.G.2 Glossary**: 22 → ~30 용어로 확장 (xaas/ux 발견)

### §2.H (성능 모델)
- **§2.H.2 Latency Budget**: 모든 셀에 책임 crate + 메트릭 이름 + criterion bench 이름 명시 (B-7)
- **§2.H.2**: `quota_config` fetch (cache hit) 비용 추가 (H-6)
- **§2.H.4 NUMA-aware FFD**: 의사코드 추가 (H-7)
- **§2.H**: WS payload bandwidth 분석 추가 (M-8)

### §3 (Cross-cutting Matrix)
- NVML polling 주기 1초로 정정 (H-9)
- Audit flow row 추가
- WsAggregator row 추가

### §4 (Gap)
- G-1, G-2 → 본 consolidation에서 해결됨, "Resolved in Phase D" 표기
- G-7 (GatewayHarness) → Week 3에 gateway-router-lead 확정
- 신규 G-12 ~ G-30 (Phase C 발견 28건) 모두 추가

### §5 (Open Questions)
- Q1~Q23 → 본 consolidation에서 답변됨, 답변 인라인 추가
- 신규 Q24~Q30 (Phase C 결과)

### §6 (Phase Tags)
- `[P1]` 태그 누락 항목 정비

### 신규 섹션 §7
**Phase D Resolutions**: 본 consolidation 링크 + 25개 PM 결정 요약

---

## 🎯 다음 단계

### Phase D-1 (지금 — 본 consolidation 작성 완료)
✅ 7 review 통합
✅ 25개 PM 결정
✅ v1 작성 plan 도출

### Phase D-2 (다음)
- chief-architect에게 v1 작성 지시 (본 consolidation을 input으로)
- v1 예상: 1798 → 2400-2700 lines
- 작업 시간: ~10-15분 (subagent)

### Phase E (v1 이후)
- v1을 다시 8 subagents가 검증 (또는 PM이 review 요약)
- v1 Approved 후:
  - `docs/modules/deployment-operations.md` §3.2 수정 (K8s operator 구조)
  - `docs/modules/gpu-resource-manager.md` MIG 프로파일 테이블 정정
  - `docs/design/core/types-consolidation.md` `Forbidden` variant 추가, `Router::chat_with_context` 반영
  - `docs/design/xaas/phase1.md` Streaming quota grace 정책, scope 403
  - `docs/design/testing/harness.md` 시나리오 6 (audit), tracing/Prometheus assertion helper

### Phase F (구현 준비)
- `gadgetron-cli` main.rs wire-up (현재 stub)
- `gadgetron-xaas` 신규 크레이트 디렉토리 + 기본 파일 생성
- `gadgetron-testing` 신규 크레이트 디렉토리 + 기본 파일 생성
- Workspace `Cargo.toml` 멤버 추가
- 첫 GitHub Actions workflow 작성

---

## 📁 관련 파일

- **검토 대상**: `docs/architecture/platform-architecture.md` v0 (1798 lines)
- **본 consolidation**: `docs/architecture/phase-c-review-results.md` (이 파일)
- **다음 산출물**: `docs/architecture/platform-architecture.md` v1 (chief-architect 작성 예정)
- **관련 결정**: `docs/process/04-decision-log.md` D-1~D-13 + D-20260411-01~13

---

**Phase D-1 Status**: ✅ Complete
**다음**: chief-architect v1 작성 (Phase D-2)
