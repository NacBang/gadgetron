# Gadgetron Platform Architecture

> **담당**: @chief-architect
> **상태**: v0 Draft — Phase B 초안. Phase C에서 8 subagent parallel review 예정.
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 결정**: D-1, D-2, D-3, D-4, D-5, D-6, D-7, D-8, D-9, D-10, D-11, D-12, D-13,
>               D-20260411-01, D-20260411-02, D-20260411-03, D-20260411-04, D-20260411-05,
>               D-20260411-06, D-20260411-07, D-20260411-08, D-20260411-09, D-20260411-10,
>               D-20260411-11, D-20260411-12, D-20260411-13
> **Phase**: [P1] 기반 — [P2]/[P3] 진화 경로 포함
> **Glossary**: see §2.G.2

---

## 목적

이 문서는 Gadgetron의 **canonical cross-component 통합 아키텍처 뷰**다.
개별 모듈 문서(`docs/modules/`)와 설계 문서(`docs/design/`)는 각 도메인 세부사항을
다루며, 이 문서는 그 문서들을 묶는 통합 참조 문서로 기능한다.

**이 문서가 답해야 할 질문:**
1. 요청은 어떤 경로로 처리되는가?
2. 상태는 어디 저장되고 프로세스 재시작 후 어떻게 복구되는가?
3. 장애 발생 시 어떤 컴포넌트가 어떻게 영향을 받는가?
4. Phase 1→2→3 진화 시 어떤 인터페이스가 안정적으로 유지되는가?
5. 서브-밀리초 P99 목표를 달성하기 위한 성능 budget 분배는?

**Phase C에서 이 문서를 기반으로 8개 subagent가 각 domain 관점에서 review한다.**
이 v0 문서는 의도적으로 gap을 포함한다 — §4에 chief-architect의 초기 관찰이 있으며,
Phase C에서 더 깊은 gap이 발견될 것이다.

---

## §0. 8 Axis 개요

이 문서는 8개 축(A-H)으로 플랫폼을 분석한다:

| Axis | 이름 | 핵심 질문 |
|------|------|-----------|
| A | 시스템 수준 | 컴포넌트 구조와 데이터 흐름 |
| B | 크로스컷 | 관측성, 보안, 설정, 에러, 동시성 |
| C | 배포 아키텍처 | 단일노드→멀티노드→K8s 진화 |
| D | 상태 관리 | 영속성, 캐시, 채널, 복구 |
| E | Phase 진화 | API 안정성, 데이터 마이그레이션, 트레이트 안정성 |
| F | 장애 모드 | 장애 시나리오, 복구 경로, 그레이스풀 셧다운 |
| G | 도메인 모델 | Bounded context, ubiquitous language, aggregate |
| H | 성능 모델 | Latency budget, throughput, SLO 검증 |

---

## 1. 철학 & 컨셉 (Why)

### 1.1 Gadgetron 비전

**"GPU 클러스터 위에서 동작하는, 서브밀리초 P99 레이턴시를 보장하는 AI 오케스트레이션 플랫폼"**

세 가지 차별화:
- **Rust-native**: GC pause 없음, 제로카피 스트리밍, lock-free DashMap
- **GPU-first**: VRAM bin-packing, NUMA-aware 배치, MIG 정적 분할 [P1] → 동적 [P2]
- **XaaS 계층**: 단일 플랫폼에서 GPUaaS → ModelaaS → AgentaaS [P2] 점진적 추상화

### 1.2 핵심 설계 원칙 (불변)

1. **단일 바이너리**: 마이크로서비스 분산 없이 `gadgetron` 바이너리 하나로 전체 스택 구동
2. **Config-driven**: TOML + `${ENV}` 치환으로 배포 환경을 선언적으로 기술
3. **P99 < 1ms gateway overhead**: 요청 도착부터 upstream call 시작까지 측정 (Provider latency 제외)
4. **Leaf crate 원칙**: `gadgetron-core`는 `sqlx`, `axum`, `nvml-wrapper` 등 구현 의존성 없음 (D-12, D-13)
5. **Forward-only migrations**: DB 스키마는 sqlx forward-only migration만 허용

### 1.3 본 문서의 위치

```
docs/00-overview.md           ← 제품 비전, Phase 로드맵, crate별 API 요약
docs/modules/*.md             ← 5개 모듈 (gateway-routing, model-serving,
                                 gpu-resource-manager, xaas-platform, deployment-operations)
docs/design/core/             ← types-consolidation (Track 1 R3 Approved)
docs/design/testing/          ← harness (Track 2 R3)
docs/design/xaas/             ← phase1 (Track 3 R3)
docs/architecture/platform-architecture.md  ← [본 문서] 통합 뷰
```

설계 세부사항은 `docs/design/` 문서를 reference. 본 문서는 통합 뷰와 cross-cutting 관계를 다룬다.

---

## 2. 8 Axis Architecture

---

### Axis A: 시스템 수준 (System Level)

#### 2.A.1 컴포넌트 구조 — 10 Crates

| Crate | 역할 | 상태 |
|-------|------|------|
| `gadgetron-core` | 공유 타입, trait, error, config | [P1] 존재 (D-1~D-3, D-10, D-13 미반영) |
| `gadgetron-provider` | 6종 LLM provider adapter | [P1] OpenAI/Anthropic/Ollama/vLLM/SGLang 구현, Gemini Week 6-7 예정 |
| `gadgetron-router` | 6종 routing strategy, lock-free MetricsStore | [P1] 구현됨 |
| `gadgetron-gateway` | axum HTTP server, SSE pipeline, middleware chain | [P1] 핸들러 stub — 연결 필요 |
| `gadgetron-scheduler` | VRAM bin-packing, LRU eviction, node registry | [P1] 구현됨 (NUMA/MIG Week 3-4) |
| `gadgetron-node` | NodeAgent (process mgmt), ResourceMonitor (CPU/GPU) | [P1] 구현됨 (VRAM sync 미정) |
| `gadgetron-tui` | ratatui read-only dashboard | [P1] 스켈레톤 |
| `gadgetron-cli` | 단일 바이너리 진입점, 부팅 시퀀스 | [P1] 부분 구현 |
| `gadgetron-xaas` | auth, tenant, quota, audit (+ billing/agent [P2]) | [P1] 신설 필요 (D-20260411-03) |
| `gadgetron-testing` | mock, fixture, harness, property test | [P1] 신설 필요 (D-20260411-05) |

#### 2.A.2 단일 바이너리 레이어드 아키텍처

```
+===========================================================================+
|                     gadgetron (단일 바이너리)                              |
|                                                                           |
|  ┌─────────────────────────────────────────────────────────────────────┐  |
|  │                      gadgetron-gateway                              │  |
|  │                                                                     │  |
|  │  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────────┐ │  |
|  │  │ /v1/chat/    │  │ /v1/models   │  │ /api/v1/{nodes,models,    │ │  |
|  │  │ completions  │  │              │  │  usage,costs}             │ │  |
|  │  └──────────────┘  └──────────────┘  └───────────────────────────┘ │  |
|  │  ┌──────────────┐  ┌──────────────────────────────────────────────┐│  |
|  │  │ /health      │  │ /api/v1/xaas/* [P1] /api/v1/ws/* [P2]       ││  |
|  │  └──────────────┘  └──────────────────────────────────────────────┘│  |
|  │                                                                     │  |
|  │  Middleware: Auth → RateLimit [P2] → Guardrails [P2] →            │  |
|  │             Routing → ProtocolTranslate → Provider →              │  |
|  │             Metrics → Audit                                        │  |
|  └───────────────────────────────┬─────────────────────────────────────┘  |
|                                  │                                         |
|  ┌───────────────────────────────▼─────────────────────────────────────┐  |
|  │                      gadgetron-xaas                                 │  |
|  │  PgKeyValidator (moka LRU cache, D-20260411-12)                     │  |
|  │  QuotaEnforcer (pre-check → post-record)                            │  |
|  │  AuditWriter (mpsc 4096, D-20260411-09)                             │  |
|  └─────────────────────┬──────────────────────────────────────────────┘   |
|                        │                                                   |
|  ┌─────────────────────▼──────────────────────────────────────────────┐   |
|  │                      gadgetron-router                              │   |
|  │  Router::resolve(&ChatRequest) → RoutingDecision                   │   |
|  │  Router::chat() / chat_stream() → fallback chain                  │   |
|  │  MetricsStore (DashMap, lock-free)                                 │   |
|  └─────────────────────┬──────────────────────────────────────────────┘   |
|                        │                                                   |
|  ┌─────────────────────▼──────────────────────────────────────────────┐   |
|  │                      gadgetron-provider                            │   |
|  │  OpenAI / Anthropic / Gemini [P1 week6] / Ollama / vLLM / SGLang  │   |
|  │  LlmProvider trait: chat / chat_stream / models / health          │   |
|  │  SseToChunkNormalizer [P1 week5] (D-20260411-02)                  │   |
|  └─────────────────────┬──────────────────────────────────────────────┘   |
|                        │                                                   |
|  ┌─────────────────────▼──────────────────────────────────────────────┐   |
|  │         gadgetron-scheduler              gadgetron-node            │   |
|  │  deploy(model, engine, vram_mb)    NodeAgent::start_model()       │   |
|  │  undeploy(model_id)                NodeAgent::stop_model()        │   |
|  │  find_eviction_candidate() LRU     ResourceMonitor::collect()     │   |
|  │  NUMA-aware [P1], MIG static [P1]  NVML feature-gate             │   |
|  └──────────────────────────────────────────────────────────────────────┘  |
|                                                                           |
|  ┌─────────────────────────────┐  ┌─────────────────────────────────┐    |
|  │     gadgetron-tui [P1 RO]  │  │     gadgetron-cli (진입점)      │    |
|  │  ratatui: Nodes/Models/Req  │  │  AppConfig::load() + wire-up   │    |
|  └─────────────────────────────┘  └─────────────────────────────────┘    |
|                                                                           |
|  ┌─────────────────────────────────────────────────────────────────────┐  |
|  │                      gadgetron-core                                 │  |
|  │  config | error | message | model | node | provider | routing | ui  │  |
|  └─────────────────────────────────────────────────────────────────────┘  |
+===========================================================================+

     ^                           ^                        ^
     | HTTP :8080                | Management             | Metrics :9090
  Client (OpenAI SDK)        K8s / Slurm [P2]         Prometheus/Grafana [P1]
```

#### 2.A.3 의존성 그래프 (D-12 + D-20260411-03/05 반영)

```
gadgetron-core        ← 외부 의존성만 (no internal crate deps)
    ↑
gadgetron-provider    ← core
gadgetron-router      ← core, provider
gadgetron-xaas        ← core             (D-20260411-03, NEW)
    ↑
gadgetron-gateway     ← core, provider, router, scheduler, node, xaas
gadgetron-scheduler   ← core, node
gadgetron-node        ← core
gadgetron-tui         ← core, router, scheduler, node
gadgetron-cli         ← core, provider, router, gateway, scheduler, node, tui, xaas
gadgetron-testing     ← all above (dev-dep only, D-20260411-05, NEW)
```

**규칙**: 
- 의존성은 단방향 하향 (역방향 금지, 순환 금지)
- `gadgetron-xaas`는 `gadgetron-gateway`에 의존하지 않음 — gateway가 xaas에 의존
- `gadgetron-testing`은 dev-dependency만 사용, 프로덕션 코드 미포함

#### 2.A.4 4가지 핵심 데이터 흐름

##### 2.A.4.1 Request Flow (비스트리밍)

```
Client
  │ POST /v1/chat/completions
  │ Authorization: Bearer gad_live_xxxxx
  │ {"model":"gpt-4o","messages":[...],"stream":false}
  ▼
gadgetron-gateway
  │ 1. axum::Router → chat_completions handler
  │ 2. Extract Bearer token from Authorization header
  │ 3. PgKeyValidator::validate(token)
  │    - SHA-256 hash → moka LRU cache lookup (D-20260411-12)
  │    - cache hit (99%): <50μs → ValidatedKey { tenant_id, scopes }
  │    - cache miss (1%): PostgreSQL query → 3-8ms (cold)
  │ 4. TenantContext { tenant_id, quota_config } 생성
  │ 5. QuotaEnforcer::check_pre(tenant_id, estimated_tokens)
  │    - 초과 시 → GadgetronError::QuotaExceeded → 429
  │ 6. Deserialize ChatRequest from request body
  │ 7. request_id = Uuid::new_v4() (OpenTelemetry propagation)
  ▼
gadgetron-router
  │ 8. Router::resolve(&ChatRequest)
  │    - RoutingConfig.default_strategy 확인
  │    - 전략별 provider 선택 (RoundRobin/CostOptimal/...)
  │    - RoutingDecision { provider, model, estimated_cost_usd, fallback_chain }
  │ 9. Router::chat(request) → provider.chat(request)
  ▼
gadgetron-provider
  │ 10. 선택된 LlmProvider::chat(ChatRequest) → upstream HTTP call
  │     - OpenAI: POST api.openai.com/v1/chat/completions
  │     - Anthropic: POST api.anthropic.com/v1/messages
  │     - Ollama/vLLM/SGLang: POST localhost:{port}/v1/chat/completions
  │ 11. Response 역직렬화 → ChatResponse
  ▼
gadgetron-router (post-processing)
  │ 12. MetricsStore::record_success(provider, model, latency_ms, tokens, cost)
  │     또는 record_error() → try_fallbacks() → fallback_chain 순차 시도
  ▼
gadgetron-xaas
  │ 13. QuotaEnforcer::record_post(tenant_id, actual_usage)
  │     - Usage { prompt_tokens, completion_tokens } → cost_cents = f64 → i64
  │     - quota 업데이트 (Redis [P2], PostgreSQL 직접 [P1])
  │ 14. AuditWriter::send(AuditEntry { request_id, tenant_id, model, provider,
  │                                    status, latency_ms, tokens, cost_cents })
  │     - mpsc::try_send(4096 capacity) → 실패 시 warn + counter 증분 (D-20260411-09)
  ▼
gadgetron-gateway
  │ 15. ChatResponse → JSON 직렬화 → axum Response 200
  │ 16. TraceLayer span end (request_id, latency, status)
  ▼
Client
```

##### 2.A.4.2 Stream Flow (SSE)

```
Client
  │ POST /v1/chat/completions
  │ {"stream": true, ...}
  ▼
gadgetron-gateway
  │ 1~7: 동일 (Auth + Quota + request_id)
  ▼
gadgetron-router → gadgetron-provider
  │ 8. Router::chat_stream(request)
  │    → LlmProvider::chat_stream(request)
  │    → Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>
  │
  │    OpenAI/Ollama/vLLM/SGLang:
  │      upstream SSE → eventsource-stream → ChatChunk
  │    Anthropic:
  │      "content_block_delta" SSE → normalize → ChatChunk
  │    Gemini [P1 week6]:
  │      HTTP polling → PollingToEventAdapter → ChatChunk (D-20260411-02)
  ▼
gadgetron-gateway
  │ 9. chat_chunk_to_sse(stream):
  │    ChatChunk → serde_json → Event::default().data(json) → Sse<KeepAlive(15s)>
  │    제로카피: 중간 버퍼 없이 axum response body로 파이프라인
  │
  │ 10. Stream 정상 종료: "data: [DONE]\n\n" 전송
  │     Stream 중단 감지:
  │       - client abort → GadgetronError::StreamInterrupted { reason: "client_abort" }
  │       - network timeout → StreamInterrupted { reason: "timeout" }
  │       - upstream error → StreamInterrupted { reason: "upstream_error" }
  │       → tower::retry 1회 재시도 (D-20260411-08)
  │
  │ 11. Stream 완료 후 Audit 기록 (실제 사용량은 스트림 완료 후 집계)
  ▼
Client  ← SSE: "data: {delta}\n\n" ... "data: [DONE]\n\n"
```

##### 2.A.4.3 Deploy Flow (로컬 모델 배포)

```
Client (Management API)
  │ POST /api/v1/models/deploy
  │ Authorization: Bearer gad_live_xxxxx  (Scope::Management 필요)
  │ {"model_id":"llama3-70b","engine":"vllm","vram_requirement_mb":42000}
  ▼
gadgetron-gateway
  │ 1. Scope::Management 검증 (D-20260411-10)
  │ 2. require_scope(ctx, Scope::Management) → 403 if missing
  ▼
gadgetron-scheduler
  │ 3. Scheduler::deploy(model_id, engine, vram_mb)
  │    a. 이미 ModelState::Running → 멱등 return Ok(())
  │    b. nodes.read() → healthy nodes 조회
  │    c. 가용 VRAM 검색: n.resources.available_vram_mb() >= vram_mb
  │    d. 미충족 → find_eviction_candidate(node_id, required_mb)
  │       LRU: last_used 기준 정렬 → 최소사용 모델 누적 해제
  │    e. 선택된 노드에 ModelDeployment { status: Loading, ... } 생성
  │    f. NUMA affinity 검사 (D-20260411-04): gpu.numa_node 기반 선호 노드
  │    g. ModelState 전이: Pending → Loading
  ▼
gadgetron-node
  │ 4. NodeAgent::start_model(&deployment)
  │    InferenceEngine별 시작:
  │      Vllm:   tokio::process::Command → "vllm serve --model ..."
  │      SGLang: tokio::process::Command → "python3 -m sglang.launch_server"
  │      Ollama: POST /api/generate {"keep_alive": -1}
  │      LlamaCpp: tokio::process::Command → "llama-server"
  │      TGI:    tokio::process::Command → "text-generation-launcher"
  │    Child 프로세스 PID 저장 (D-10: ModelState::Running { port, pid })
  ▼
gadgetron-scheduler ← NodeAgent heartbeat (push)
  │ 5. ModelState 전이: Loading → Running { port: u16, pid: u32 }
  │ 6. 노드 VRAM 사용량 업데이트
  ▼
Client ← 200 OK { "status": "deploying", "model_id": "llama3-70b" }

  [비동기 상태 확인]
  GET /api/v1/models/status
  → ModelStatus { status: "running", node_id: "node-01", vram_used_mb: 42000 }
```

##### 2.A.4.4 Audit Flow (감사 로그 기록)

```
모든 요청 완료 후:
  │
  ▼
gadgetron-xaas (AuditWriter)
  │ 1. QuotaEnforcer::record_post() → AuditEntry {
  │      request_id, tenant_id, model, provider, status,
  │      latency_ms, prompt_tokens, completion_tokens,
  │      cost_cents: i64  ← f64 USD * 1_000_000 변환 (D-20260411-06)
  │    }
  │
  │ 2. mpsc::Sender::try_send(entry)
  │    capacity: 4096 (D-20260411-09)
  │    실패: warn!() + counter!("gadgetron_xaas_audit_dropped_total")
  │
  ▼
AuditWriter 백그라운드 태스크 (tokio::spawn)
  │ 3. recv_many(batch, max=100) 또는 timeout(500ms) → batch flush
  │    어느 조건 먼저 충족되든 flush 트리거
  │
  │ 4. sqlx::query! INSERT INTO audit_log (...) VALUES ... (batch)
  │    PostgreSQL: audit_log.cost_cents BIGINT NOT NULL
  │    실패 시: GadgetronError::Database { kind: QueryFailed, ... }
  │             + ERROR log + Prometheus counter
  │
  │ 5. SIGTERM 수신 시: drain remaining + flush + exit (5s timeout)
  ▼
PostgreSQL: audit_log table
```

#### 2.A.5 Public vs Internal API 경계

| 경로 접두사 | 대상 | 안정성 | 인증 |
|------------|------|--------|------|
| `/v1/*` | 외부 클라이언트 (OpenAI 호환) | 영구 안정 (semver major 전까지) | Bearer `gad_live_*` or `gad_test_*` |
| `/api/v1/*` | 운영자, 관리 도구 | semver 기반 | Bearer `Scope::Management` |
| `/api/v1/xaas/*` | XaaS 관리자 | Phase 2 안정화 후 stable | Bearer `Scope::XaasAdmin` |
| `/api/v1/ws/*` | TUI/Web UI realtime | [P2] | Bearer |
| `/health` | 헬스체크, 인프라 | 안정 | 없음 (public) |
| `/metrics` | Prometheus scrape | [P1] | 없음 (internal network) |

---

### Axis B: 크로스컷 (Cross-Cutting Concerns)

#### 2.B.1 관측성 Stack

**3계층 관측성**: Logging → Metrics → Tracing

**Logging (tracing JSON)**

모든 크레이트는 `tracing` crate 사용. 포맷: JSON (프로덕션), pretty (개발).

구조화 필드 표준:
```
{
  "timestamp": "2026-04-12T10:00:00.000Z",
  "level": "INFO",
  "target": "gadgetron_gateway::handlers",
  "request_id": "<uuid-v4>",     ← 모든 span에 전파
  "tenant_id": "<tenant>",        ← 인증 후 전파
  "span_id": "<otel-span-id>",
  "trace_id": "<otel-trace-id>",
  <crate별 추가 필드>
}
```

request_id 전파 경로:
```
gateway: request_id = Uuid::new_v4()
  → router: Span::current().record("request_id", &request_id)
    → provider: http header "X-Request-Id" 전달
      → audit: AuditEntry.request_id
```

**Metrics (Prometheus)**

엔드포인트: `GET /metrics` (port 9090 또는 8080 공유)
네이밍 규칙: `gadgetron_{crate}_{metric}_{unit}`

핵심 메트릭:
```
# Gateway
gadgetron_gateway_requests_total{method, path, status}     Counter
gadgetron_gateway_request_duration_seconds{path, quantile} Summary
gadgetron_gateway_active_connections                        Gauge

# Router
gadgetron_router_routing_decisions_total{strategy, provider} Counter
gadgetron_router_fallback_total{from_provider, to_provider}  Counter

# Provider
gadgetron_provider_requests_total{provider, model, status}   Counter
gadgetron_provider_latency_seconds{provider, model, quantile} Summary
gadgetron_provider_errors_total{provider, model, error_kind} Counter

# Scheduler
gadgetron_scheduler_deployments_total{engine, status}        Counter
gadgetron_scheduler_vram_used_mb{node_id}                   Gauge
gadgetron_scheduler_evictions_total{policy, node_id}        Counter

# Node
gadgetron_node_gpu_vram_used_mb{node_id, gpu_index}         Gauge
gadgetron_node_gpu_utilization_pct{node_id, gpu_index}      Gauge
gadgetron_node_gpu_temperature_c{node_id, gpu_index}        Gauge

# XaaS
gadgetron_xaas_audit_dropped_total{tenant_id}               Counter
gadgetron_xaas_quota_exceeded_total{tenant_id}              Counter
gadgetron_xaas_auth_cache_hits_total                        Counter
gadgetron_xaas_auth_cache_misses_total                      Counter
```

**OpenTelemetry Span Propagation [P1]**

Span 계층:
```
Root span: gadgetron.request (request_id, tenant_id, model)
  └─ gadgetron.auth (cache_hit: bool)
  └─ gadgetron.quota.check (tenant_id, estimated_tokens)
  └─ gadgetron.routing (strategy, selected_provider)
  └─ gadgetron.provider.call (provider, model, latency_ms)
       └─ (outbound HTTP span: upstream provider)
  └─ gadgetron.quota.record (actual_tokens, cost_cents)
  └─ gadgetron.audit.enqueue (tenant_id)
```

`tracing-opentelemetry` bridge로 tracing span → OTel span 변환.
Export: OTLP → Jaeger/Tempo [P1], 기타 backend [P2].

#### 2.B.2 보안

**Trust Boundary 다이어그램**:

```
[외부 인터넷]
      │
  TLS termination (rustls, reqwest default)
      │
[Gadgetron 프로세스 경계]
  │
  ├─ Bearer token validation (PgKeyValidator + moka LRU)
  │    gad_live_* → Scope::OpenAiCompat
  │    gad_test_* → Scope::OpenAiCompat
  │    Management/XaasAdmin scope → 별도 부여
  │
  ├─ Scope enforcement (require_scope() per route)
  │
  ├─ Quota check (QuotaEnforcer::check_pre)
  │
  ├─ PII guardrails [P2] — 입력/출력 스캔
  │
  └─ Rate limiting [P2] — per-tenant RPM/TPM
```

**Secret 관리**:
- Phase 1: 환경변수 (`${OPENAI_API_KEY}` 등), AppConfig::load()에서 치환
- Phase 2: K8s Secret, Vault integration

**API Key 구조** (D-11):
- `gad_live_{32-char-random}` — 프로덕션
- `gad_test_{32-char-random}` — 테스트 (lower quota)
- `gad_vk_{32-char-random}` — 가상 키 (tenant 범위 내 재배포)
- PostgreSQL: SHA-256(key) 저장, 원문 1회 발급 후 미저장

**upstream TLS**:
- `reqwest` 기본 rustls 사용
- `OpenAiProvider`, `AnthropicProvider`: HTTPS only
- `OllamaProvider`, `VllmProvider`, `SglangProvider`: HTTP 허용 (로컬 전용)

#### 2.B.3 설정 관리

**설정 로딩 파이프라인**:
```
config/gadgetron.toml
  → TOML 파싱 (serde + toml 0.8)
  → ${ENV} 치환 (AppConfig::load() 내부)
  → 검증 (필수 필드, URL 형식, timeout 범위)
  → AppConfig 구조체 (Arc<AppConfig> 로 공유)
```

**설정 섹션 구조**:
```toml
[server]           # ServerConfig: bind, request_timeout_ms
[router]           # RoutingConfig: default_strategy, fallback_chain
[providers.*]      # ProviderConfig: type, api_key, endpoint, models
[[nodes]]          # NodeConfig: id, endpoint, gpu_count
[[models]]         # LocalModelConfig: id, engine, vram_requirement_mb, args
[scheduler]        # SchedulerConfig: eviction_policy, vram_ceiling_pct=90
[xaas]             # XaasConfig: pg_url, key_cache_ttl_secs, audit_channel_cap
[telemetry]        # TelemetryConfig: log_format, otlp_endpoint, metrics_port
```

**핫 리로드**: [P2] — Phase 1은 재시작 필요. Phase 2: SIGHUP → AppConfig 재로딩 → 컴포넌트별 반영.

#### 2.B.4 에러 처리

**GadgetronError taxonomy** (D-13 + D-20260411-08 + D-20260411-13):

| Variant | 원인 | HTTP Status | Retry | Prometheus Label |
|---------|------|------------|-------|-----------------|
| `Provider(String)` | upstream 4xx/5xx | 502 | No | `provider` |
| `Router(String)` | routing logic 실패 | 500 | No | `router` |
| `Scheduler(String)` | 스케줄링 실패 | 500 | No | `scheduler` |
| `Node(String)` | node agent 오류 | 500 | No | `node` |
| `Config(String)` | 설정 오류 | 500 | No | `config` |
| `NoProvider(String)` | 조건 맞는 provider 없음 | 503 | No | `no_provider` |
| `AllProvidersFailed(String)` | fallback chain 전체 실패 | 503 | No | `all_failed` |
| `InsufficientResources(String)` | VRAM 부족 | 503 | No | `insufficient_resources` |
| `ModelNotFound(String)` | 모델 미등록 | 404 | No | `model_not_found` |
| `NodeNotFound(String)` | 노드 미등록 | 404 | No | `node_not_found` |
| `HealthCheckFailed { provider, reason }` | 헬스체크 실패 | 503 | 1회 | `health_failed` |
| `Timeout(u64)` | 요청 타임아웃 | 504 | No | `timeout` |
| `StreamInterrupted { reason }` | SSE 스트림 중단 (D-20260411-08) | 500/499 | 1회 | `stream_interrupted` |
| `Billing(String)` | 과금 오류 (D-13) | 402 | No | `billing` |
| `TenantNotFound(String)` | 테넌트 미존재 (D-13) | 401 | No | `tenant_not_found` |
| `QuotaExceeded(String)` | 쿼터 초과 (D-13) | 429 | No | `quota_exceeded` |
| `DownloadFailed(String)` | 모델 다운로드 실패 (D-13) | 502 | 1회 | `download_failed` |
| `HotSwapFailed(String)` | 핫스왑 실패 (D-13) [P2] | 500 | No | `hot_swap_failed` |
| `Database { kind, message }` | DB 오류 (D-20260411-13) | 종류별 | PoolTimeout만 | `db_*` |

**에러 전파 원칙**:
1. `gadgetron-core`는 sqlx 의존성 없음 (Leaf crate, D-13). `DatabaseErrorKind` enum은 DB-agnostic.
2. `sqlx_to_gadgetron()` helper는 `gadgetron-xaas/src/error.rs`에만 위치 (orphan rule 준수).
3. 외부 응답에 내부 에러 상세 노출 금지 (`message` 필드 로그에만, 응답은 generic).

#### 2.B.5 Async/Concurrency 모델

**Tokio 런타임**: `#[tokio::main]` with full features. 기본 multi-thread scheduler.

**동시성 프리미티브 선택 가이드**:

| 용도 | 선택 | 이유 |
|------|------|------|
| `MetricsStore` (hot path read+write) | `DashMap<K,V>` | lock-free sharding, O(1) 평균 |
| `Scheduler.deployments` | `RwLock<HashMap<K,V>>` | 읽기 다수/쓰기 소수 패턴 |
| `Scheduler.nodes` | `RwLock<HashMap<K,V>>` | heartbeat 쓰기 주기적, 읽기 빈번 |
| `PgKeyValidator` LRU cache | `moka::future::Cache<K,V>` | async-friendly, TTL, 10k entries |
| `AuditWriter` channel | `tokio::sync::mpsc(4096)` | 비동기 배치 flush, backpressure |
| `NodeAgent.running_models` | `HashMap<String, Child>` | 단일 태스크 소유, 외부 공유 없음 |
| `AtomicUsize` (RoundRobin) | `std::sync::atomic::AtomicUsize` | lock-free 카운터 |

**Drop Guard 패턴 (SIGTERM)**:
```rust
// gadgetron-cli/src/main.rs
let _shutdown_guard = ShutdownGuard::new(audit_writer_tx.clone());
// Drop on exit → AuditWriter channel flush
```

**금지 패턴** (§2.H.7 참조):
- `Mutex`/`RwLock` lock을 `.await` 경계 넘어 유지
- `spawn_blocking` 없이 sync I/O를 async context에서 직접 호출
- Hot path에서 `Arc::clone` + `Mutex::lock` 순서 역전 (deadlock)

---

### Axis C: 배포 아키텍처 (Deployment Architecture)

#### 2.C.1 단일 노드 — Dev Mode [P1]

```
+-----------------------------+
|  Host Machine (dev laptop)  |
|                             |
|  gadgetron serve            |
|  ├─ config/gadgetron.toml   |
|  └─ Ollama (localhost:11434)|
+-----------------------------+
```
- TOML 파일 직접 수정
- NVML 비활성 (`cargo run --no-default-features`)
- PostgreSQL: Docker에서 로컬 실행 또는 SQLite 호환 테스트 모드 [v0 gap: SQLite 테스트 모드 미정]

#### 2.C.2 단일 노드 — Production Docker [P1]

```
+--------------------------------------------+
|  Docker Compose / 단일 VM                  |
|                                            |
|  ┌──────────────────────┐                  |
|  │  gadgetron           │ :8080, :9090     |
|  │  (distroless image)  │                  |
|  └──────────────────────┘                  |
|  ┌──────────────────────┐                  |
|  │  postgres:16         │ :5432            |
|  └──────────────────────┘                  |
|  ┌──────────────────────┐                  |
|  │  ollama / vllm       │ :11434 / :8000   |
|  └──────────────────────┘                  |
+--------------------------------------------+
```

Dockerfile: 멀티스테이지 빌드 (builder: rust:1.82-bookworm → distroless/cc-debian12).
환경변수: `GADGETRON_CONFIG=/etc/gadgetron/gadgetron.toml`

#### 2.C.3 멀티 노드 Phase 1 — Helm Chart [P1]

```
+--------------------------------------------------+
|  Kubernetes Cluster                              |
|                                                  |
|  Namespace: gadgetron                            |
|  ┌────────────────────────────────────────────┐  |
|  │ Deployment: gadgetron                      │  |
|  │  replicas: 1 (Phase 1 stateful, 단일)     │  |
|  │  image: ghcr.io/gadgetron/gadgetron:x.y.z  │  |
|  └────────────────────────────────────────────┘  |
|  ┌────────────────────────────────────────────┐  |
|  │ StatefulSet: postgres                      │  |
|  │  PVC: 50Gi                                 │  |
|  └────────────────────────────────────────────┘  |
|  ┌───────────┐  ┌──────────────────────────────┐ |
|  │ Service   │  │ ConfigMap: gadgetron.toml    │ |
|  │ :8080     │  │ Secret: api-keys             │ |
|  └───────────┘  └──────────────────────────────┘ |
+--------------------------------------------------+

  외부 GPU 노드 (K8s 클러스터 외부 또는 노드풀):
  → gadgetron이 NodeConfig.endpoint 통해 HTTP로 제어
  → 수동 등록 (POST /api/v1/nodes/register)
```

Phase 1 제약: `replicas: 1` — in-memory state (`RwLock`) 때문에 수평 확장 불가.
Phase 2 해결: 상태를 Redis/PostgreSQL로 이전 → `replicas: N` 가능.

#### 2.C.4 K8s Operator [P2]

```
GadgetronModel CRD:
  spec.model_id, spec.engine, spec.vram_mb, spec.replicas
  status.state, status.node_id, status.endpoint

GadgetronNode CRD:
  spec.endpoint, spec.gpu_count, spec.labels
  status.healthy, status.vram_used_mb

GadgetronRouting CRD:
  spec.default_strategy, spec.providers, spec.fallback_chain
```

Reconcile loop: `watch(GadgetronModel)` → `Scheduler::deploy/undeploy` → `status.patch()`

#### 2.C.5 Slurm [P2]

```
gadgetron-node/src/slurm.rs (D-12):
  SlurmIntegration::submit_job(model_id, args) → job_id
  SlurmIntegration::cancel_job(job_id)
  SlurmIntegration::job_status(job_id) → SlurmJobState
```

SLURM GRES 연동: `--gres=gpu:2` → `ModelDeployment.assigned_gpus`

#### 2.C.6 멀티 리전 [P3]

```
Region A (primary)          Region B (secondary)
  gadgetron                   gadgetron
     │                            │
     └─── Federation API ─────────┘
          (gRPC [P2] or REST)
          
  routing: latency-optimal selects nearest region
  state sync: PostgreSQL replication or event sourcing
```

#### 2.C.7 Config Propagation 전략

| Phase | 전략 | 변경 반영 |
|-------|------|----------|
| P1 | 정적 TOML 파일 | 재시작 필요 |
| P2 | SIGHUP hot reload | 무중단 반영 (provider 추가/제거) |
| P2 | ConfigMap watch (K8s) | operator reconcile |
| P3 | 분산 설정 store (etcd/Consul) | 실시간 전파 |

---

### Axis D: 상태 관리 (State Management)

#### 2.D.1 영속 상태 — PostgreSQL [P1]

**스키마 (Phase 1 초기)**:

```sql
-- gadgetron-xaas/src/db/migrations/

-- 001_initial.sql
CREATE TABLE tenants (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    is_active   BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE api_keys (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID NOT NULL REFERENCES tenants(id),
    key_hash    TEXT NOT NULL UNIQUE,      -- SHA-256(gad_live_xxx)
    prefix      TEXT NOT NULL,             -- "gad_live_" or "gad_test_"
    scopes      TEXT[] NOT NULL,           -- {'OpenAiCompat'} etc.
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at  TIMESTAMPTZ,
    is_revoked  BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE quota_configs (
    tenant_id       UUID PRIMARY KEY REFERENCES tenants(id),
    rpm_limit       INT NOT NULL DEFAULT 1000,
    tpm_limit       BIGINT NOT NULL DEFAULT 1000000,
    daily_cost_cents BIGINT,              -- i64 cents (D-8)
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE audit_log (
    id                  BIGSERIAL PRIMARY KEY,
    request_id          UUID NOT NULL,
    tenant_id           UUID NOT NULL,
    model               TEXT NOT NULL,
    provider            TEXT NOT NULL,
    status              TEXT NOT NULL,     -- 'ok', 'error', 'stream_interrupted'
    latency_ms          INT NOT NULL,
    prompt_tokens       INT NOT NULL DEFAULT 0,
    completion_tokens   INT NOT NULL DEFAULT 0,
    cost_cents          BIGINT NOT NULL DEFAULT 0,  -- i64 (D-8)
    error_kind          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_audit_log_tenant_created ON audit_log(tenant_id, created_at);
CREATE INDEX idx_audit_log_request_id ON audit_log(request_id);
```

**Phase 2 추가 테이블**:
```sql
billing_ledger (tenant_id, period, total_cents, invoice_id)   [P2]
agents (id, tenant_id, template_id, state, memory_json)       [P2]
model_catalog (model_id, source, metadata, download_state)    [P2]
```

**연결 풀**: sqlx PgPool, max_connections=20 (D-20260411-12 수준 적용)

#### 2.D.2 메모리 캐시

| 구조 | 위치 | 타입 | 크기 | TTL | 목적 |
|------|------|------|------|-----|------|
| `PgKeyValidator.cache` | gadgetron-xaas | `moka::future::Cache<String, ValidatedKey>` | 10,000 entries | 10분 | auth hot path <50μs (D-20260411-12) |
| `MetricsStore` | gadgetron-router | `DashMap<(String,String), ProviderMetrics>` | unbounded | - | lock-free routing metrics |
| `Scheduler.deployments` | gadgetron-scheduler | `RwLock<HashMap<String, ModelDeployment>>` | ~1000 models | - | 배포 상태 |
| `Scheduler.nodes` | gadgetron-scheduler | `RwLock<HashMap<String, NodeStatus>>` | ~100 nodes | - | 노드 등록/VRAM |
| `Router.providers` | gadgetron-router | `HashMap<String, Arc<dyn LlmProvider>>` | ~10 providers | - | 불변 (startup init) |

**캐시 무효화**:
- moka TTL 만료 (자연 만료): 10분 후 자동
- API key 취소 시 수동 invalidation: `PgKeyValidator::invalidate_key(hash)`
- SIGTERM 시 캐시 drop (휘발성)

#### 2.D.3 파일 시스템 상태

| 경로 | 내용 | 생명주기 |
|------|------|---------|
| `config/gadgetron.toml` | 전체 설정 | 수동 관리 |
| `gadgetron-xaas/src/db/migrations/*.sql` | DB 스키마 | forward-only |
| `gadgetron-core/benches/baselines/` | criterion 기준선 | CI 자동 갱신 |
| `gadgetron-testing/snapshots/` | insta snapshot | PR 승인 |
| [P2] `~/.gadgetron/model-cache/` | HuggingFace 모델 가중치 | LRU 디스크 관리 |

#### 2.D.4 Tokio 채널

| 채널 | 용량 | 생산자 | 소비자 | 가득 찼을 때 |
|------|------|--------|--------|------------|
| `AuditWriter.tx` | 4096 | 모든 요청 처리 태스크 | AuditWriter 배경 태스크 1개 | warn + drop + counter (D-20260411-09) |
| [P2] `SchedulerEventChannel` | TBD | NodeAgent heartbeat | Scheduler 이벤트 루프 | backpressure |

#### 2.D.5 일관성 보장

**startup 시 복구**:
1. PostgreSQL 연결 확인 → 실패 시 fatal exit
2. `sqlx::migrate!()` 실행 → pending 마이그레이션 자동 적용
3. 인메모리 캐시는 빈 상태로 시작 → 워밍업 기간 cold start 증가
4. Scheduler.nodes는 빈 상태 → 운영자가 노드 재등록 (POST /api/v1/nodes/register)

**shutdown 시 flush**:
```
SIGTERM 수신
  │
  ├─ axum graceful shutdown (in-flight 요청 drain, 30s timeout)
  ├─ AuditWriter channel drain + PostgreSQL batch flush (5s timeout)
  ├─ NodeAgent running 모델 stop (SIGTERM to child processes)
  └─ PostgreSQL pool close
```

**idempotency**:
- `Scheduler::deploy()`: 이미 `Running` → `Ok(())` 즉시 반환
- `sqlx migrations`: 이미 적용된 migration → skip
- `AuditEntry`: `request_id` unique constraint → 중복 삽입 safe fail

#### 2.D.6 영속성 경계 다이어그램

```
                    Process Restart 시 복구 여부
                    
                    ┌────────────────────────────────────┐
                    │          SURVIVES RESTART          │
                    │                                    │
                    │  PostgreSQL:                       │
                    │  - tenants, api_keys, quota_configs│
                    │  - audit_log (flushed entries만)   │
                    │                                    │
                    │  Filesystem:                       │
                    │  - config/gadgetron.toml           │
                    │  - DB migration state              │
                    └────────────────────────────────────┘
                    
                    ┌────────────────────────────────────┐
                    │            LOST ON RESTART         │
                    │                                    │
                    │  In-memory:                        │
                    │  - moka LRU (auth key cache)       │
                    │    → cold start: DB re-query       │
                    │  - MetricsStore (DashMap)          │
                    │    → Prometheus scrape 기록 손실   │
                    │  - Scheduler.deployments/nodes     │
                    │    → 운영자 재등록 필요 [P1 gap]  │
                    │  - Router.providers (Arc)          │
                    │    → AppConfig 재초기화            │
                    │                                    │
                    │  In-flight:                        │
                    │  - AuditWriter channel 미flush분   │
                    │    → 4096 entries 최대 손실 가능   │
                    │    → Phase 2 WAL fallback (D-20260411-09) │
                    └────────────────────────────────────┘
```

#### 2.D.7 PostgreSQL SPOF 분석 [P1]

**Phase 1에서 PostgreSQL은 SPOF다** — auth, quota, audit 모두 필수.

| 장애 상황 | 영향 | Phase 1 대응 |
|----------|------|-------------|
| DB 연결 실패 (시작 시) | 프로세스 시작 불가 | fatal exit, 운영자 개입 필요 |
| DB 연결 실패 (실행 중) | 신규 auth: 캐시 hit → 계속 (10분 TTL), cache miss → 401 | 경고 로그, Prometheus alert |
| DB 연결 실패 (실행 중) | Audit: channel drop + counter 증가 | warn log |
| DB pool exhaustion (20 conn) | quota check 지연 → 503 | pool timeout → `Database { PoolTimeout }` → 503 |

**Phase 2 해결**: Read replica + connection proxy (PgBouncer).
**Phase 3 해결**: 멀티 리전 PostgreSQL replication.

---

### Axis E: Phase 진화 (Phase Evolution)

#### 2.E.1 Phase 1 — 12주 현재 (D-20260411-01 옵션 B)

**목표**: 단일 노드 클라우드+로컬 LLM 프록시 + 경량 멀티테넌시

포함 (확정):
- 엔드포인트: `/v1/chat/completions` SSE + `/v1/models` + `/health` + `/api/v1/{nodes,models,usage}` + `/api/v1/xaas/` (기본)
- Provider: OpenAI, Anthropic (Week 2-4), Ollama, vLLM, SGLang (Week 2-4), Gemini (Week 6-7, D-20260411-02)
- Routing 6 strategy: RoundRobin, CostOptimal, LatencyOptimal, QualityOptimal, Fallback, Weighted
- Scheduler: VRAM bin-packing + LRU + NUMA static + MIG static enable/disable (D-20260411-04)
- XaaS: API 키 + 테넌트 + 기본 쿼터 + 감사 로그 (D-20260411-03)
- TUI: 읽기 전용 ratatui
- 보안: Bearer + rustls + 기본 guardrails (PII [P2])
- 관측성: tracing JSON + Prometheus + OpenTelemetry
- DB: PostgreSQL + sqlx 0.8 (D-4, D-9)
- 배포: Dockerfile + Helm chart
- Testing: gadgetron-testing 크레이트 (D-20260411-05)

**Week 일정 (대략)**:
- Week 1: gadgetron-core 타입 확정 (D-1~D-3, D-10, D-13)
- Week 2-4: 5개 provider + XaaS 기반 + Gateway 미들웨어 연결
- Week 5: SseToChunkNormalizer 추출 (D-20260411-02)
- Week 6-7: Gemini provider + NUMA/MIG scheduler
- Week 8-10: 통합 테스트, 성능 검증, CI
- Week 11-12: Helm chart, 문서, 배포 검증

#### 2.E.2 Phase 2 — 추가 항목

| 항목 | 크레이트 | 기반 결정 |
|------|---------|---------|
| Web UI (gadgetron-web) | 신규 | D-20260411-07 (WsMessage) |
| K8s operator CRDs | 신규 gadgetron-k8s | §2.C.4 |
| Slurm | gadgetron-node/slurm.rs | D-12 |
| ML 기반 semantic routing | gadgetron-router | - |
| Full XaaS: billing engine | gadgetron-xaas | D-20260411-03 |
| AgentaaS | gadgetron-xaas | - |
| HuggingFace catalog | gadgetron-xaas | - |
| Multi-node Pipeline Parallelism | gadgetron-scheduler | D-20260411-04 |
| ThermalController | gadgetron-node | D-12 |
| PII guardrails (ML model) | gadgetron-gateway | - |
| Redis 분산 캐시 (auth) | gadgetron-xaas | D-20260411-12 |
| Hot reload (SIGHUP) | gadgetron-core/cli | §2.B.3 |
| Rate limiting (tower-governor) | gadgetron-gateway | D-6 |
| Audit WAL/S3 fallback | gadgetron-xaas | D-20260411-09 |
| gRPC (tonic/prost) | cross-crate | D-5 |
| MIG 동적 재구성 | gadgetron-node | D-20260411-04 |

#### 2.E.3 Phase 3 — 추가 항목

- 멀티 리전 federation
- Plugin system (외부 provider, routing strategy)
- AMD ROCm / Intel Gaudi 벤더 추상화 (GpuMonitor trait 확장)
- A/B 테스트 자동화 (ModelaaS)
- Profiling 기반 오토튜닝 (vLLM/SGLang 파라미터)
- HuggingFace Hub 모델 카탈로그 자동 동기화

#### 2.E.4 API 안정성 약속

| 경로 | 약속 | 변경 정책 |
|------|------|---------|
| `/v1/*` | **영구 안정** — OpenAI 호환 | 추가만 허용. 제거는 major version bump |
| `/api/v1/*` | semver 기반 안정 | minor: 추가 OK, major: deprecation 공지 6개월 |
| `/api/v1/xaas/*` | Phase 2 안정화 후 stable | Phase 2 전에는 breaking change 가능 |
| `LlmProvider` trait | `#[non_exhaustive]` 아닌 추가 메서드 → default impl 제공 | minor OK |
| `GadgetronError` | `#[non_exhaustive]` → 새 variant 추가 시 minor | 기존 variant 변경 → major |

#### 2.E.5 데이터 마이그레이션 경로

- Forward-only migrations: `gadgetron-xaas/src/db/migrations/00N_xxx.sql`
- 번호: `001_initial.sql`, `002_add_billing.sql`, ...
- Rollback: 금지. 이전 버전 복구는 백업 복원으로만.
- `sqlx::migrate!()` 시작 시 자동 적용
- CI: `sqlx prepare --check` 컴파일 타임 쿼리 검증

**Phase 1→2 스키마 진화 예시**:
```sql
-- 002_phase2_billing.sql
ALTER TABLE tenants ADD COLUMN plan TEXT NOT NULL DEFAULT 'free';
CREATE TABLE billing_ledger (...);
-- DROP TABLE은 별도 정리 migration에서만, 최소 2 minor version 후
```

#### 2.E.6 Trait 안정성

```rust
// gadgetron-core/src/provider.rs
#[async_trait]
pub trait LlmProvider: Send + Sync {
    // Phase 1 stable methods
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;
    fn chat_stream(&self, req: ChatRequest)
        -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>;
    async fn models(&self) -> Result<Vec<ModelInfo>>;
    fn name(&self) -> &str;
    async fn health(&self) -> Result<()>;
    async fn is_live(&self) -> Result<bool>;
    async fn is_ready(&self, model: &str) -> Result<bool>;
    
    // Phase 2 (default impl 제공으로 기존 impl 무중단)
    // async fn embeddings(&self, req: EmbeddingRequest) -> Result<EmbeddingResponse> { Err(...) }
}

// GadgetronError: #[non_exhaustive]는 소비자 코드의 exhaustive match를 깨지 않도록
// 내부 타입으로만 사용. 공개 Error enum은 #[non_exhaustive] 적용.
#[non_exhaustive]
pub enum GadgetronError { ... }

// Scope enum (D-20260411-10)
#[non_exhaustive]
pub enum Scope {
    OpenAiCompat,
    Management,
    XaasAdmin,
    // Phase 2: ChatRead, ChatWrite, ModelsList, ...
}
```

---

### Axis F: 장애 모드 (Failure Modes)

#### 2.F.1 컴포넌트별 장애 시나리오

**Gateway 장애**:

| # | 시나리오 | 영향 | 자동 복구 | 수동 개입 |
|---|---------|------|---------|---------|
| G-1 | 포트 바인딩 실패 | 프로세스 시작 불가 | 없음 | 포트 충돌 확인 |
| G-2 | TLS 인증서 만료 [P2] | 신규 TLS 연결 실패 | 없음 | 인증서 갱신 |
| G-3 | OOM (요청 폭주) | 프로세스 Kill | K8s 재시작 | 메모리 limit 조정 |
| G-4 | 미들웨어 panic | 현재 요청 500 | tower 격리 | 패닉 원인 조사 |
| G-5 | axum worker thread 포화 | 신규 연결 거부 | backpressure | worker count 증가 |

**Router 장애**:

| # | 시나리오 | 영향 | 자동 복구 |
|---|---------|------|---------|
| R-1 | 모든 provider 헬스체크 실패 | AllProvidersFailed | circuit breaker half-open → retry |
| R-2 | MetricsStore DashMap lock congestion | routing 지연 | lock-free sharding 자체 완화 |
| R-3 | RoutingConfig 전략 미매칭 | Router(String) 500 | 없음, 설정 수정 필요 |
| R-4 | Weighted strategy 합이 0 | 패닉 방지 로직 필요 | assert_ne!(total, 0.0) guard |

**Provider 장애**:

| # | 시나리오 | 영향 | 자동 복구 |
|---|---------|------|---------|
| P-1 | OpenAI API rate limit (429) | Provider(String) | fallback chain 다음 provider |
| P-2 | Anthropic API 5xx | Provider(String) | circuit breaker 후 half-open |
| P-3 | 로컬 vLLM OOM (CUDA OOM) | Provider(String) | 스케줄러 eviction + redeploy |
| P-4 | SSE 스트림 중단 (client abort) | StreamInterrupted | 1회 재시도 (D-20260411-08) |
| P-5 | Gemini polling timeout | StreamInterrupted | 1회 재시도 |
| P-6 | upstream TLS 오류 | Provider(String) | 없음, 로그 + 알림 |

**Scheduler 장애**:

| # | 시나리오 | 영향 | 자동 복구 |
|---|---------|------|---------|
| S-1 | 모든 노드 VRAM 부족 | InsufficientResources 503 | LRU eviction 후 재시도 |
| S-2 | LRU eviction candidate 없음 | InsufficientResources 503 | 없음, 새 노드 추가 필요 |
| S-3 | ModelState 전이 불일치 | stale state | 헬스체크 주기 동기화 |
| S-4 | RwLock poisoning | fatal panic | K8s 재시작 |
| S-5 | 노드 heartbeat 소실 | stale VRAM 데이터 → OOM 가능 | heartbeat timeout → mark unhealthy |

**Node 장애**:

| # | 시나리오 | 영향 | 자동 복구 |
|---|---------|------|---------|
| N-1 | vLLM 프로세스 crash | Provider(String), model 미응답 | Scheduler 감지 → redeploy |
| N-2 | NVML 미설치 / 드라이버 오류 | GPU 메트릭 없음 (CPU-only 모드) | graceful degradation |
| N-3 | 프로세스 시작 실패 (vllm not found) | Node(String) | 없음, 인스톨 필요 |
| N-4 | 동적 포트 충돌 [P1 gap] | start_model 실패 | RangePortAllocator 필요 |
| N-5 | Child process zombie (SIGTERM 미처리) | PID 누수 | Child.kill() + wait() in drop |

**Database 장애**:

| # | 시나리오 | 영향 | HTTP 응답 |
|---|---------|------|---------|
| D-1 | PostgreSQL 연결 실패 (시작) | 프로세스 시작 불가 | - |
| D-2 | Pool exhaustion (max 20) | quota check 대기 | 503 (PoolTimeout) |
| D-3 | Slow query (index 누락) | auth latency 증가 | 캐시 hit 시 영향 없음 |
| D-4 | Migration 실패 | 시작 불가 | - |
| D-5 | Row not found (API key) | auth 실패 | 401 |
| D-6 | Constraint violation (duplicate) | audit 중복 | 409 (safe ignore) |

**GPU 장애**:

| # | 시나리오 | 영향 | 자동 복구 |
|---|---------|------|---------|
| G-1 | CUDA OOM 중 | Provider → Provider(String) | model unload + redeploy |
| G-2 | GPU 온도 초과 (90°C+) | throttling 시작 (Phase 1: 감지만) | [P2] ThermalController 개입 |
| G-3 | NVLink 연결 끊김 | TP model 실패 | [P2] 재스케줄링 |
| G-4 | MIG profile 충돌 | Node(String) | 수동 MIG 재설정 |

**Network 장애**:

| # | 시나리오 | 영향 | 자동 복구 |
|---|---------|------|---------|
| Net-1 | 외부 DNS 실패 | OpenAI/Anthropic 연결 불가 | DNS 복구 시 자동 |
| Net-2 | NAT timeout (long SSE) | StreamInterrupted | 재시도 + keepalive(15s) |
| Net-3 | k8s pod 네트워크 분할 | 일부 노드 unreachable | heartbeat timeout → unhealthy |

#### 2.F.2 Recovery Paths

**자동 복구 흐름**:
```
Provider 실패 감지
  │
  ├─ GadgetronError::Provider → circuit breaker check
  │    open? → skip provider → next in fallback chain
  │    closed? → provider 호출 → 성공 시 reset, 실패 시 counter++
  │    3 failures/60s → circuit open → 60s cooldown → half-open
  │
  └─ AllProvidersFailed → 503 → 클라이언트 재시도 (Retry-After 헤더)
```

**수동 복구 절차**:
1. `GET /health` → 컴포넌트별 상태 확인
2. `GET /api/v1/nodes` → 노드 VRAM/health 확인
3. `POST /api/v1/models/deploy` → 실패 모델 재배포
4. `POST /api/v1/nodes/register` → 노드 재등록 (재시작 후)
5. 로그: `RUST_LOG=gadgetron=debug` + request_id 추적

#### 2.F.3 Circuit Breaker

```
provider별 circuit breaker (gadgetron-router):

State machine:
  Closed → (3 failures in 60s) → Open
  Open → (60s) → Half-Open
  Half-Open → (1 success) → Closed
  Half-Open → (1 failure) → Open

구현:
  ProviderMetrics { failure_count, last_failure_at, state }
  CircuitBreakerState { Closed, Open { opened_at }, HalfOpen }
  
  check_circuit(provider) -> CircuitBreakerState
  record_failure(provider) -> CircuitBreakerState  // transition if needed
  record_success(provider) -> ()

Prometheus:
  gadgetron_router_circuit_state{provider}    Gauge (0=closed, 1=open, 2=half-open)
  gadgetron_router_circuit_trips_total{provider}  Counter
```

#### 2.F.4 Fallback Chain

```rust
// Router::try_fallbacks()
//
// 1. RoutingDecision.fallback_chain: Vec<String> — 순서대로 시도
// 2. 각 fallback에 circuit breaker 체크
// 3. 모두 실패 → GadgetronError::AllProvidersFailed
// 4. MetricsStore.record_success / record_error 각각 기록

// 설정 예시:
// [router]
// default_strategy = "fallback"
// fallback_chain = ["openai", "anthropic", "ollama"]
```

#### 2.F.5 Backpressure

| 포인트 | 메커니즘 | 임계값 |
|--------|---------|--------|
| AuditWriter channel | mpsc(4096) → try_send 실패 시 drop + warn | 4096 entries |
| Gateway 요청 큐 | axum backpressure (OS TCP listen backlog) | ~128 (기본) |
| GPU utilization ceiling | 90% → 신규 배포 거부 (D-20260411-04) | 90% |
| PostgreSQL pool | max 20 connections → 초과 요청 대기 → PoolTimeout | 20 conn |
| RateLimit [P2] | tower-governor per-tenant RPM/TPM | QuotaConfig |

#### 2.F.6 Graceful Shutdown 시퀀스

```
SIGTERM 수신 (K8s preStop, Ctrl+C)
  │
  ├─ 1. 신규 연결 수락 중단 (axum graceful_shutdown)
  │
  ├─ 2. In-flight 요청 drain 대기 (최대 30초)
  │       진행 중 SSE 스트림: 현재 chunk 완료 후 StreamInterrupted 반환
  │
  ├─ 3. AuditWriter channel drain
  │       mpsc receiver.recv_many() until empty → PostgreSQL flush
  │       타임아웃: 5초 (D-20260411-09)
  │
  ├─ 4. NodeAgent 실행 중 모델 SIGTERM
  │       child.kill() → child.wait() → PID 정리
  │       타임아웃: 10초 → SIGKILL
  │
  ├─ 5. PostgreSQL pool close
  │       sqlx PgPool::close() → 진행 중 쿼리 완료 대기
  │
  └─ 6. process exit(0)

총 소요 시간: 최대 ~45초 (drain 30 + audit 5 + model 10)
K8s terminationGracePeriodSeconds: 60 권장
```

#### 2.F.7 OOM 시나리오

| 대상 | 시나리오 | 처리 |
|------|---------|------|
| Host RAM | 요청 폭주 → 역직렬화 버퍼 증가 | K8s memory limit → OOM Kill → 재시작 |
| GPU VRAM | LLM 추론 중 KV cache 폭주 | vLLM/SGLang 자체 OOM → Provider error → eviction |
| AuditWriter channel | 4096 full → new entries drop | warn + counter (Phase 2 WAL) |
| MetricsStore DashMap | unbounded → 100 provider × 1000 model 규모에서 위험 | [P1 gap: size limit 미정] |

#### 2.F.8 Failure Tree (Root Cause → Propagation)

```
Root Causes
  │
  ├─ [External] Provider API 장애
  │    → Provider(String) → AllProvidersFailed → 503
  │    → Circuit breaker → fallback chain 전환
  │
  ├─ [Resource] GPU VRAM 부족
  │    → InsufficientResources → LRU eviction → redeploy → or 503
  │
  ├─ [Data] PostgreSQL 장애
  │    → auth: cache hit 지속 (TTL 내) → 10분 후 401
  │    → audit: drop + warn (non-critical path)
  │    → quota: 503 (PoolTimeout) 가능
  │
  ├─ [Network] 네트워크 분할
  │    → Provider 연결 실패 → Provider(String) → fallback
  │    → NodeAgent heartbeat 소실 → stale state → mark unhealthy
  │
  └─ [Process] 자체 프로세스 패닉
       → K8s 재시작 → in-memory state 초기화
       → 운영자: 노드/모델 재등록 필요
```

---

### Axis G: 도메인 모델 (Domain Model)

#### 2.G.1 Bounded Contexts (DDD 관점)

**Inference Context** (gadgetron-provider, gadgetron-router):
- 핵심 개념: ChatRequest, ChatResponse, ChatChunk, LlmProvider, RoutingDecision
- 책임: 추론 요청/응답 처리, 프로토콜 변환, 라우팅 결정
- 외부 의존: 클라우드 API (OpenAI/Anthropic), 로컬 inference server

**Scheduling Context** (gadgetron-scheduler, gadgetron-node):
- 핵심 개념: ModelDeployment, NodeStatus, EvictionPolicy, ModelState, VRAM budget
- 책임: GPU 자원 할당, 모델 배포/해제, LRU eviction, 노드 생명주기
- 외부 의존: NVML, sysinfo, child process

**Tenancy Context** (gadgetron-xaas):
- 핵심 개념: Tenant, ApiKey, Scope, QuotaConfig, AuditEntry
- 책임: 인증, 인가, 쿼터 관리, 감사 로그
- 외부 의존: PostgreSQL

**Observability Context** (cross-cutting, gadgetron-router + gadgetron-core):
- 핵심 개념: ProviderMetrics, GpuMetrics, ClusterHealth, WsMessage
- 책임: 메트릭 수집, TUI/Web UI 데이터 공급, Prometheus export
- 외부 의존: Prometheus, OpenTelemetry (Jaeger/Tempo)

**Configuration Context** (gadgetron-core, gadgetron-cli):
- 핵심 개념: AppConfig, ServerConfig, ProviderConfig, RoutingConfig, NodeConfig
- 책임: TOML 파싱, ENV 치환, 시작 검증
- 외부 의존: 파일시스템, 환경변수

#### 2.G.2 Ubiquitous Language Glossary

| 용어 (영) | 용어 (한) | 정의 | 위치 |
|----------|----------|------|------|
| `ChatRequest` | 채팅 요청 | OpenAI 호환 `/v1/chat/completions` 요청 본문 | `gadgetron-core/src/provider.rs` |
| `ChatResponse` | 채팅 응답 | 단일 완성 응답 (비스트리밍) | same |
| `ChatChunk` | 스트림 청크 | SSE 스트리밍 델타 단위 | same |
| `LlmProvider` | LLM 프로바이더 | 특정 AI 서비스와의 어댑터 인터페이스 | same |
| `RoutingStrategy` | 라우팅 전략 | 프로바이더 선택 알고리즘 | `gadgetron-core/src/routing.rs` |
| `RoutingDecision` | 라우팅 결정 | 선택된 provider + model + fallback chain | same |
| `ModelDeployment` | 모델 배포 | 특정 노드에 배포된 모델 인스턴스 | `gadgetron-core/src/model.rs` |
| `ModelState` | 모델 상태 | Pending/Loading/Running{port,pid}/Stopping/Stopped/... | same |
| `NodeStatus` | 노드 상태 | 노드의 현재 리소스와 health 상태 | `gadgetron-core/src/node.rs` |
| `NodeResources` | 노드 리소스 | CPU/RAM/GPU 현재 사용량 | same |
| `GpuInfo` | GPU 정보 | 개별 GPU의 VRAM/온도/전력/사용률 | same |
| `EvictionPolicy` | Eviction 정책 | LRU/Priority/CostBased/WeightedLru (D-2) | `gadgetron-core/src/routing.rs` |
| `Tenant` | 테넌트 | API를 사용하는 독립적 조직/사용자 | `gadgetron-xaas/src/tenant/` |
| `ApiKey` | API 키 | `gad_` 접두사 인증 키 (D-11) | `gadgetron-xaas/src/auth/` |
| `Scope` | 권한 범위 | OpenAiCompat/Management/XaasAdmin (D-20260411-10) | same |
| `QuotaConfig` | 쿼터 설정 | 테넌트별 RPM/TPM/일일비용 한도 | `gadgetron-xaas/src/quota/` |
| `AuditEntry` | 감사 항목 | 요청당 1개 감사 기록 (cost_cents: i64, D-8) | `gadgetron-xaas/src/audit/` |
| `TenantContext` | 테넌트 컨텍스트 | 인증 후 요청에 첨부되는 테넌트 정보 | `gadgetron-xaas/src/auth/` |
| `GadgetronError` | 가젯트론 오류 | 19개 variant 통합 에러 열거형 | `gadgetron-core/src/error.rs` |
| `AppConfig` | 앱 설정 | TOML 최상위 설정 구조체 | `gadgetron-core/src/config.rs` |
| `InferenceEngine` | 추론 엔진 | Ollama/Vllm/Sglang/LlamaCpp/Tgi | `gadgetron-core/src/model.rs` |
| `ParallelismConfig` | 병렬화 설정 | tp_size/pp_size/ep_size/dp_size (D-1) | `gadgetron-core/src/routing.rs` |
| `NumaTopology` | NUMA 토폴로지 | NUMA 노드와 GPU 매핑 (D-3) | `gadgetron-core/src/node.rs` |
| `SseToChunkNormalizer` | SSE 정규화기 | Provider별 SSE를 ChatChunk로 통일 (D-20260411-02) | `gadgetron-provider/src/` |
| Gateway overhead | 게이트웨이 오버헤드 | 요청 도착~upstream call 시작 시간 (P99<1ms 목표) | §2.H.1 |

#### 2.G.3 Aggregate 경계

| Aggregate Root | 포함 엔티티 | 생명주기 |
|---------------|------------|---------|
| `ModelDeployment` | ModelState 전이, assigned_node, last_used | deploy() ~ undeploy() |
| `NodeStatus` | NodeResources, GpuInfo 배열 | register() ~ disconnect() |
| `Tenant` | ApiKey 목록, QuotaConfig | 테넌트 생성 ~ 비활성화 |
| `Router` | providers (Arc), MetricsStore, circuit breakers | 프로세스 시작 ~ 종료 |

#### 2.G.4 Core Types vs Domain Entities

**gadgetron-core**: 데이터 컨테이너 (순수 구조체 + trait 정의)
- 도메인 로직 없음 (단순 getter/setter 또는 없음)
- 예외: `estimate_vram_mb()` (순수 수식)

**도메인 로직 위치**:
- Routing logic → `gadgetron-router` (routing strategy, fallback, circuit breaker)
- Scheduling logic → `gadgetron-scheduler` (bin-packing, LRU)
- Auth/quota logic → `gadgetron-xaas` (validation, enforcement)
- Protocol translation → `gadgetron-provider` (SSE normalization)

#### 2.G.5 TenantContext 전파 경로

```
HTTP 요청 헤더
  │ Authorization: Bearer gad_live_xxx
  ▼
PgKeyValidator::validate(token_hash)
  │ cache hit → ValidatedKey { tenant_id, scopes, expires_at }
  ▼
TenantContext 생성 { tenant_id, scopes, quota_config }
  │
  ├─ axum Extension으로 handler에 전달
  │    → require_scope(ctx, Scope::Management) 검사
  │
  ├─ QuotaEnforcer::check_pre(ctx.tenant_id, estimated_tokens)
  │
  ├─ request span에 tenant_id 기록
  │    → 모든 하위 span에 전파 (tracing)
  │    → MetricsStore label에 포함
  │
  ├─ 요청 완료 후 QuotaEnforcer::record_post(ctx.tenant_id, actual_usage)
  │
  └─ AuditEntry.tenant_id 기록
```

---

### Axis H: 성능 모델 (Performance Model)

#### 2.H.1 P99 < 1ms Gateway Overhead 정의

**측정 범위**: HTTP 요청 수신 (TCP socket accept) → upstream HTTP call 시작

**제외 항목**: upstream provider 네트워크 왕복 (100ms~수십초), 모델 추론 자체

```
[측정 시작: socket accept]
  │
  ├─ axum handler dispatch:    ~1μs
  ├─ auth (cache hit):         < 50μs    (moka LRU, D-20260411-12)
  ├─ quota pre-check:          < 50μs    (in-memory QuotaEnforcer)
  ├─ request deserialization:  < 100μs   (serde_json)
  ├─ routing decision:         < 100μs   (DashMap read, AtomicUsize)
  ├─ protocol translation:     < 200μs   (ChatRequest → provider format)
  ├─ span record:              < 50μs    (tracing macro, lock-free)
  └─ upstream call initiation: ~1μs      (reqwest::Client::execute 시작)
[측정 끝: upstream call 시작]

Total non-provider overhead: < 550μs (best case) ~ < 1ms (P99 목표)
```

**SLO 재기술** (D-20260411-12 반영):
- `P99 < 1ms` (auth cache hit, 99% 케이스)
- `P99 < 8ms` (auth cache miss, 1% cold start 케이스)

#### 2.H.2 Latency Budget 분해

| 단계 | 예산 | 측정 방법 | 현재 상태 |
|------|------|---------|---------|
| axum dispatch | ~1μs | tracing span | 미측정 |
| auth (cache hit) | < 50μs | `gadgetron_xaas_auth_duration_us` | 미구현 |
| auth (cache miss) | < 5ms | same | 미구현 |
| quota pre-check | < 50μs | `gadgetron_xaas_quota_check_us` | 미구현 |
| request deserialization | < 100μs | criterion bench | 미구현 |
| routing decision | < 100μs | criterion bench `BM-routing` | 미구현 |
| protocol translation | < 200μs | criterion bench | 미구현 |
| span record | < 50μs | tracing overhead bench | 미구현 |
| **Total** | **< 1ms** | e2e load test | 미구현 |

#### 2.H.3 Throughput 모델

**가정**: 단일 노드, 8 vCPU, 32GB RAM, 미들레인지 GPU (RTX 3090 / A10)

| 시나리오 | 예상 처리량 | 제한 요소 |
|---------|-----------|---------|
| 클라우드 API 프록시 (OpenAI/Anthropic) | 10K req/s | 네트워크 I/O, upstream rate limit |
| 로컬 vLLM (1 GPU, 7B model) | 50~200 req/s | GPU 추론 throughput |
| 로컬 Ollama (CPU mode) | 5~20 req/s | CPU 추론 throughput |
| 게이트웨이 자체 처리 (mock provider) | > 50K req/s | Rust async overhead |

**Bottleneck 예측**:
1. Phase 1에서 PostgreSQL quota check가 병목 가능 (pool 20 conn)
2. AuditWriter channel full 시 drop (비-critical)
3. MetricsStore DashMap은 bottleneck 아님 (lock-free)

#### 2.H.4 Capacity Planning

**Per-tenant Quota** (기본값):
```toml
[xaas.default_quota]
rpm_limit = 1000          # 분당 요청 수
tpm_limit = 1_000_000     # 분당 토큰 수
daily_cost_cents = 10_000 # 일 $100 상당
```

**GPU VRAM Bin-packing**:
```
노드 VRAM 90% ceiling (D-20260411-04)
  → available_vram = total_vram * 0.9 - used_vram
  → First-Fit Decreasing: 모델 VRAM 요구량 기준 내림차순 정렬

예시: A100 80GB 노드
  총 VRAM: 80GB
  ceiling: 72GB (90%)
  배포 가능:
    - 70B Q4_K_M: ~44GB → 1개 배포 후 28GB 남음
    - 7B Q4_K_M: ~5GB → 이후 추가 5개 가능
    → 총 6개 모델 가능 (가중치 공유 없이)
```

**PostgreSQL Connection Pool**:
```
max_connections: 20
  - XaaS: auth validation (rare), quota check (every request), audit batch (periodic)
  - 최악 경우: 20 concurrent slow queries → remaining request → PoolTimeout → 503
  - 권장: Phase 1 single instance → PgBouncer [P2]
```

#### 2.H.5 SLO 검증 경로

**Criterion 벤치마크** (D-20260411-11):
```
gadgetron-core/benches/
  BM-alloc:     메모리 할당 패턴 (ChatRequest 역직렬화)
  BM-eviction:  LRU eviction 알고리즘 (100~10000 모델)
  BM-vram:      estimate_vram_mb() 계산

gadgetron-router/benches/
  BM-routing:   RoutingDecision 결정 (6 strategy × 10 provider)
  BM-metrics:   MetricsStore record_success (DashMap concurrent)

gadgetron-testing/benches/ (통합)
  BM-e2e-gateway-overhead:  mock provider 기준 P99 측정
  BM-auth-cache:            moka hit/miss latency
```

**Load Test** (gadgetron-testing):
```
harness/gateway.rs → GatewayHarness
  - mock provider (0ms latency)
  - 10K req/s 목표
  - P99 < 1ms 검증
```

#### 2.H.6 Hot Path 최적화 Chart

| 최적화 | 구현 | 비용 | 효과 |
|--------|------|------|------|
| `DashMap` lock-free metrics | `gadgetron-router` | 0 (기본 사용) | 고동시성 읽기/쓰기 |
| `moka::future::Cache` LRU | `gadgetron-xaas` | ~100 LOC | auth 3-5ms → <50μs |
| `Pin<Box<dyn Stream>>` 제로카피 SSE | `gadgetron-provider/gateway` | trait object 비용 | 중간 버퍼 제거 |
| `AtomicUsize` RoundRobin | `gadgetron-router` | 0 | lock-free 카운터 |
| `Arc<dyn LlmProvider>` vtable | `gadgetron-router` | 1ns per call | generic 대비 runtime polymorphism (D-20260411-11) |
| `serde_json` 제로카피 deserialize | `gadgetron-gateway` | 옵션 | string field copy 제거 가능 |

**vtable 비용 정당화** (D-20260411-11):
- Provider HTTP call: ~100ms (최소)
- vtable dispatch: ~1ns
- 비율: 10^8배 차이 → vtable 비용 무시 가능
- Config-driven 6개 provider runtime 로딩은 static generic으로 불가

#### 2.H.7 Anti-patterns (금지)

```rust
// ❌ 금지: lock across .await
let guard = rwlock.write().await; // write guard 획득
do_async_thing().await;           // guard가 await 경계 넘음 → deadlock 가능
drop(guard);

// ✅ 허용: scope 내에서 lock 해제
{
    let mut data = rwlock.write().await;
    data.update();
} // lock 해제
do_async_thing().await;

// ❌ 금지: blocking I/O in async context
async fn bad() {
    std::fs::read_to_string("file.txt").unwrap(); // blocks tokio thread
}

// ✅ 허용: tokio async I/O
async fn good() {
    tokio::fs::read_to_string("file.txt").await.unwrap();
}

// ❌ 금지: hot path에서 Vec allocation
fn hot_path(req: &ChatRequest) -> RoutingDecision {
    let providers: Vec<String> = req.model.split('/').map(|s| s.to_owned()).collect(); // alloc
}

// ✅ 허용: slice 또는 &str
fn hot_path(req: &ChatRequest) -> RoutingDecision {
    let parts = req.model.splitn(2, '/'); // iterator, no alloc
}
```

---

## 3. Cross-Cutting Matrix

각 crate × 각 axis에 대한 책임 요약.

| Crate | A 시스템 | B 크로스컷 | C 배포 | D 상태 | E 진화 | F 장애 | G 도메인 | H 성능 |
|-------|---------|----------|-------|-------|-------|-------|---------|-------|
| `core` | 공유 타입 정의, 의존성 그래프 root | GadgetronError 19 variant, AppConfig 파싱 | 배포 무관 (라이브러리) | 파일시스템 (config, migration) | #[non_exhaustive] 트레이트 안정성 | 에러 taxonomy | 모든 Bounded context 기반 타입 | BM-alloc (ChatRequest deserialization) |
| `provider` | LlmProvider 구현 (6종) | tracing span (provider.call), error → Provider(String) | 배포 무관 | 없음 (stateless) | SseToChunkNormalizer Week5, Gemini Week6 | P-1~P-6 provider 장애, StreamInterrupted | Inference Context 구현체 | vtable 1ns, 100ms network (D-20260411-11) |
| `router` | 라우팅 결정, fallback chain | MetricsStore Prometheus export, circuit breaker label | 배포 무관 | DashMap (in-memory, 재시작 소실) | RoutingStrategy #[non_exhaustive] | R-1~R-4, circuit breaker (§2.F.3) | Inference Context (routing layer) | BM-routing, DashMap lock-free |
| `gateway` | HTTP 진입점, SSE pipeline | TraceLayer, request_id span, auth middleware | :8080, :9090 expose | 없음 (stateless proxy) | /v1/* 영구 안정 약속 | G-1~G-5, graceful shutdown drain | 모든 context의 진입/출구 | P99<1ms gateway overhead (§2.H.1) |
| `scheduler` | VRAM bin-packing, LRU eviction | 배포 이벤트 tracing | 노드 목록 ConfigMap | RwLock<deployments>, RwLock<nodes> (재시작 소실) | NUMA/MIG 확장 (static→dynamic P2) | S-1~S-5, InsufficientResources | Scheduling Context | BM-eviction, bin-packing O(n) |
| `node` | 프로세스 관리, GPU 모니터링 | NVML 메트릭 → Prometheus | GPU 노드 물리 배포 | running_models HashMap (process PID) | AMD ROCm [P3], Slurm [P2] | N-1~N-5, CUDA OOM, GPU temp | Scheduling Context (node layer) | NVML polling 100ms 주기 |
| `xaas` | auth/quota/audit cross-cut | Prometheus: auth_cache_hits, quota_exceeded, audit_dropped | PostgreSQL 의존 | PgPool, moka LRU 10k/10min | billing/agent [P2], Redis [P2] | D-1~D-6 DB 장애, audit drop | Tenancy Context | auth <50μs cache hit, <8ms miss |
| `tui` | ratatui 대시보드 (읽기 전용) | 없음 (소비자) | 같은 바이너리 내 | 없음 (polling) | Web UI [P2] (WsMessage 공유 타입) | 없음 (독립 뷰) | Observability Context (UI) | 100ms polling |
| `cli` | 부팅 시퀀스, 전체 wire-up | tracing 초기화, JSON 포맷 | TOML 로드, env 치환 | ShutdownGuard (Drop flush) | 핫 리로드 SIGHUP [P2] | 전체 graceful shutdown 오케스트레이션 | Configuration Context | 시작 검증 (provider 연결, node health) |
| `testing` | mock/fixture/harness | FakeGpuMonitor, MockProvider | testcontainers PostgreSQL | 없음 (test-only) | 새 provider mock 추가 | 장애 시나리오 재현 (FailingProvider, stale VRAM) | 모든 context의 test double | BM-e2e-gateway-overhead, BM-auth-cache |

---

## 4. 식별된 Gap (v0 chief-architect Observations)

Phase C reviewers가 더 깊이 조사해야 할 초기 gap 목록.

### G-1: Scheduler 재시작 후 상태 복구 경로 미정 [HIGH]

`Scheduler.deployments`와 `nodes`는 순수 in-memory (`RwLock<HashMap>`). 프로세스 재시작 시 두 구조 모두 초기화. 현재 운영자가 노드를 수동 재등록해야 하고 기존 배포 상태를 알 방법이 없다.

**영향**: 프로덕션 환경에서 재시작 후 수십 개 모델의 상태를 수동으로 복구해야 할 수 있음.
**관련 Phase**: [P1]에서 문서화 필요, [P2]에서 PostgreSQL 기반 영속화 필요.
**Phase C 담당**: gpu-scheduler-lead가 "재시작 후 자동 상태 복구" 경로를 §4에서 명시해야 함.

### G-2: NodeAgent VRAM 동기화 프로토콜 상세 미결 [HIGH]

Scheduler의 `nodes: RwLock<HashMap<String, NodeStatus>>`는 NodeAgent가 주기적으로 업데이트해야 한다. 그러나:
- Push vs Pull 방식 미결정
- heartbeat 주기 미정 (1s? 10s?)
- stale threshold 미정 (heartbeat 2회 소실 → unhealthy?)
- Scheduler가 NodeAgent를 직접 호출하는가, NodeAgent가 Scheduler에 push하는가?

**영향**: stale VRAM 데이터로 OOM 배치 가능 (Round 2 BLOCKER #5).
**Phase C 담당**: gpu-scheduler-lead 도메인.

### G-3: MetricsStore 크기 무제한 [MEDIUM]

`MetricsStore: DashMap<(String,String), ProviderMetrics>` — `(provider, model)` pair별 1개 엔트리. 이론상 provider × model 조합 무제한 증가 가능. 100개 provider × 1000개 model = 100K entries → GiB 수준 메모리 사용 가능.

**영향**: 장기 운영 시 메모리 증가.
**Phase C 담당**: gateway-router-lead가 max entries 정책 + eviction 전략 제안 필요.

### G-4: 비스트리밍 요청의 Quota 계산 타이밍 [MEDIUM]

스트리밍 요청은 완료 후에야 actual_tokens를 알 수 있음. Pre-check는 `estimated_tokens` 기반. 실제 사용량이 추정치를 초과하는 경우:
- 이미 응답이 나간 후 quota 초과 → 어떻게 처리?
- `QuotaEnforcer::record_post()`가 실패해도 응답은 이미 전송됨
- 다음 요청에서 quota 적용 → 1 요청 grace

**영향**: 정확한 quota 집행 vs 응답 지연 트레이드오프.
**Phase C 담당**: xaas-platform-lead가 정책 명시 필요.

### G-5: 동적 포트 할당 미구현 [MEDIUM]

`NodeAgent::start_model()`의 포트 할당이 엔진별 하드코딩 (vLLM: 8000, SGLang: 30000). 동일 노드에 같은 엔진 2개 배포 시 충돌.

```
D-12에서 PortAllocator trait이 gadgetron-core/src/model.rs에 정의 예정이나
실제 RangePortAllocator 구현이 gadgetron-node에 미구현.
```

**영향**: 단일 노드 멀티모델 배포 블로커.
**Phase C 담당**: gpu-scheduler-lead.

### G-6: TenantContext를 Router/Provider에 전달하는 방법 미정 [MEDIUM]

현재 설계에서 gateway가 TenantContext를 생성하지만, Router와 Provider는 TenantContext를 인자로 받지 않는다. `MetricsStore.record_success()`에 `tenant_id`가 포함되어야 tenant별 메트릭이 가능한데 현재 signature에 없음.

**영향**: tenant별 Prometheus 메트릭 불가, per-tenant cost tracking 불가.
**Phase C 담당**: gateway-router-lead + xaas-platform-lead 협의 필요.

### G-7: GatewayHarness 미들웨어 체인 테스트 coverage 불명확 [LOW]

`gadgetron-testing`의 `GatewayHarness`가 실제 미들웨어 체인 (Auth → Quota → Routing → ...) 전체를 통과하는지, 아니면 일부만 mock하는지 명세 부족. Round 1 cross-track 이슈 #3에서 지적되었으나 해결 미확인.

**Phase C 담당**: qa-test-architect.

### G-8: Gemini PollingToEventAdapter 스트림 정합성 [LOW]

Gemini는 HTTP polling 기반. `generateContent` (동기) vs `streamGenerateContent` (SSE-like)가 모두 있으며 응답 포맷이 OpenAI와 다름 (`parts[].functionCall`). Week 6-7 구현 예정이나 현재 설계 없음.

**Phase C 담당**: inference-engine-lead.

---

## 5. Open Questions for Phase C Reviewers

Phase C 8개 subagent가 각 domain 관점에서 답해야 할 질문들.

### 5.1 @gateway-router-lead 에게

**Q1**: 미들웨어 체인의 정확한 axum layer 순서는?
```
현재 문서에 "Auth → RateLimit → Guardrails → Routing → ..." 순서가 있으나
axum에서 `ServiceBuilder::layer()` 순서가 실행 순서와 역순임.
정확한 Layer 적용 순서 + axum 의미론 명시 필요.
```

**Q2**: TenantContext를 Router에 어떻게 전달하는가? (Gap G-6)
- Option A: `Router::chat(req, tenant_ctx)` — signature 변경
- Option B: `axum::Extension<TenantContext>` → handler에서 추출 후 Router 호출 시 포함
- Option C: `ChatRequest`에 `tenant_id: Option<String>` 필드 추가 → core 변경

**Q3**: MetricsStore 크기 무제한 문제 해결 방안? (Gap G-3)

### 5.2 @inference-engine-lead 에게

**Q4**: Provider lifecycle 6가지 시나리오 (startup, healthy, degraded, failing, circuit-open, recovery)에서 정확한 `LlmProvider` 메서드 반환 값은?
- `is_live()`, `is_ready()`, `health()` 각각의 동작 보장 범위

**Q5**: Gemini `PollingToEventAdapter` 설계 — `streamGenerateContent` 응답을 ChatChunk로 변환 시 character 과금 → token 환산 공식은?

**Q6**: `SseToChunkNormalizer` 추출 (Week 5) 시 기존 5개 adapter mock이 자동으로 통과하려면 어떤 인터페이스 변경이 필요한가?

### 5.3 @gpu-scheduler-lead 에게

**Q7**: NodeAgent → Scheduler VRAM 동기화 프로토콜 결정 (Gap G-2)
- Push: NodeAgent가 30s마다 Scheduler에 HTTP POST
- Pull: Scheduler가 30s마다 NodeAgent에 GET /status
- Event: tokio channel (단일 바이너리 내부)
어느 방식을 선택하는가? stale threshold는?

**Q8**: NUMA-aware bin-packing의 실제 알고리즘:
- 현재: First-Fit Decreasing (VRAM 기준)
- NUMA 추가 시: 모델의 preferred GPU → preferred NUMA node → 해당 노드 우선 배치
- ParallelismConfig.numa_bind와 어떻게 연동하는가?

**Q9**: `RangePortAllocator` (Gap G-5) — 포트 범위, 충돌 감지, 해제 방식?

**Q10**: Scheduler 재시작 후 복구 (Gap G-1) — Phase 1에서 어떤 수준의 복구를 제공하는가?

### 5.4 @xaas-platform-lead 에게

**Q11**: Tenant isolation 보장 메커니즘:
- PostgreSQL RLS (Row Level Security) 적용 여부
- xaas crate가 다른 테넌트의 데이터를 읽지 않음을 코드 레벨에서 어떻게 보장하는가?

**Q12**: Quota pre-check에서 스트리밍 요청의 estimated_tokens 계산 방식:
- 요청 메시지 token count 추정 (tokenizer 없이?)
- max_tokens 파라미터 사용?
- Overshoot 시 처리 방침? (Gap G-4)

**Q13**: AuditEntry.cost_cents 계산 — 스트리밍 완료 전까지 집계 불가:
- 스트리밍 도중 연결 끊김 시 partial cost 처리?

### 5.5 @devops-sre-lead 에게

**Q14**: K8s operator reconcile loop (Phase 2) 정확한 동작:
- `GadgetronModel` CRD watch → Scheduler::deploy 호출 → status patch 타이밍
- finalizer 처리 (undeploy 보장)
- Reconcile 충돌 (복수 operator instance) 방지 방법

**Q15**: `terminationGracePeriodSeconds` 권장값 (§2.F.6 기반)과 preStop hook 필요성

**Q16**: `/metrics` 엔드포인트 보안 — internal network만 접근 가능하게 하는 K8s NetworkPolicy 예시

### 5.6 @ux-interface-lead 에게

**Q17**: TUI의 `WsMessage` fan-out 구독 모델 (Phase 2):
- 단일 WebSocket endpoint `/api/v1/ws/metrics`에 복수 클라이언트 구독
- broadcast channel 크기? TUI + Web UI + 모니터링 툴 동시 구독 시
- `WsMessage` enum variant별 전송 주기 (GpuMetrics: 1s? RequestLog: per-request?)

**Q18**: TUI Phase 1에서 데이터 소스:
- polling: `GET /api/v1/nodes`, `/api/v1/models/status`, `/api/v1/usage`
- 100ms 폴링 주기가 너무 빠른가? 서버 부하 고려?

### 5.7 @qa-test-architect 에게

**Q19**: 8 Axis 각각의 testability 평가:
- Axis A (데이터 흐름): e2e test coverage?
- Axis B (크로스컷): tracing/metrics unit test 방법?
- Axis F (장애 모드): chaos engineering 도구 필요성?
- Axis H (성능): criterion baseline을 CI에서 regression 검출하는 방법?

**Q20**: `FailingProvider` 구현 상세:
- 어떤 장애 패턴을 시뮬레이션해야 하는가?
  (immediate fail, delayed fail, partial stream, rate limit 429, ...)

**Q21**: PostgreSQL integration test에서 migration 적용 순서 보장 방법
(testcontainers + sqlx::migrate!)

### 5.8 @chief-architect (자기 검토용 메모)

**Q22**: `GadgetronError` variant 19개 — exhaustive match가 필요한 시점에 `#[non_exhaustive]`와의 충돌을 어떻게 처리하는가? 내부 vs 공개 API 구분 명확화 필요.

**Q23**: `Arc<dyn LlmProvider>` + `Arc<MetricsStore>` + `Arc<Router>` 가 모두 `gadgetron-gateway`의 handler에 주입되는 방식:
- axum State vs Extension vs thread-local 선택 근거 문서화 필요

---

## 6. Phase Tags 요약

전 문서의 `[P1]`/`[P2]`/`[P3]` 분포:

**[P1] Phase 1 (12주, 현재 진행 중)**:
- 10 crates 기반 구조
- 6 provider (OpenAI, Anthropic, Ollama, vLLM, SGLang, Gemini)
- 6 routing strategy
- VRAM + LRU + NUMA static + MIG static
- gadgetron-xaas (auth/tenant/quota/audit)
- PostgreSQL + sqlx
- Prometheus + OpenTelemetry
- Dockerfile + Helm chart (단일 노드)
- gadgetron-testing
- TUI (read-only)
- P99 < 1ms gateway overhead (auth cache hit)

**[P2] Phase 2 (Commercial, v0.5 예정)**:
- Hot reload (SIGHUP)
- K8s operator CRDs
- Slurm integration
- Web UI (gadgetron-web)
- Full XaaS (billing, agent, catalog)
- gRPC (tonic/prost)
- Redis 분산 캐시
- Rate limiting (tower-governor)
- Audit WAL/S3 fallback
- ThermalController
- MIG 동적 재구성
- Multi-node Pipeline Parallelism
- PII guardrails
- Horizontal scaling (stateless Scheduler)
- WsMessage fan-out WebSocket

**[P3] Phase 3 (Platform, v1.0 예정)**:
- 멀티 리전 federation
- Plugin system
- AMD ROCm / Intel Gaudi 추상화
- A/B 테스트 자동화
- HuggingFace Hub 자동 카탈로그
- Profiling 기반 오토튜닝

---

## 7. 문서 메타

**작성**: @chief-architect
**현재 상태**: v0 Draft — gap 포함, Phase C review 준비 완료

**Phase C → Phase D 계획**:
1. Phase C: 8 subagent 병렬 review (각 domain 관점)
   - gateway-router-lead, inference-engine-lead, gpu-scheduler-lead
   - xaas-platform-lead, devops-sre-lead, ux-interface-lead
   - qa-test-architect, chief-architect (자기 검토)
2. Phase D: PM consolidation → gap 해소 → v1 Approved
3. v1: 향후 모든 design doc의 canonical reference

**연관 문서**:
- `docs/00-overview.md` — 제품 비전, Phase 로드맵, crate API 요약
- `docs/design/core/types-consolidation.md` — Track 1 R3 Approved
- `docs/design/testing/harness.md` — Track 2 R3
- `docs/design/xaas/phase1.md` — Track 3 R3
- `docs/modules/gateway-routing.md`, `model-serving.md`, `gpu-resource-manager.md`, `xaas-platform.md`, `deployment-operations.md`
- `docs/process/04-decision-log.md` — D-1~D-13, D-20260411-01~13

---

## 리뷰 로그 (append-only)

### Phase B — 2026-04-12 — @chief-architect
**결론**: v0 Draft 완성. Phase C 준비.

**자체 체크리스트**:
- [x] 8 Axis 모두 coverage
- [x] D-1~D-13 + D-20260411-01~13 모두 참조
- [x] Phase 태그 전 항목 명시
- [x] 4가지 데이터 흐름 sequence diagram
- [x] 장애 모드 컴포넌트별 시나리오 테이블
- [x] 성능 budget 분해
- [x] 도메인 모델 DDD 관점
- [x] Cross-cutting matrix (10 crates × 8 axes)
- [x] Gap 식별 8건
- [x] Phase C reviewer 질문 23개

**v0 알려진 한계**:
- Gap G-1/G-2 (Scheduler 복구, VRAM 동기화) — Phase C에서 gpu-scheduler-lead 결정 필요
- Gap G-6 (TenantContext 전파) — gateway-router-lead + xaas-platform-lead 협의 필요
- §2.H 성능 측정값 대부분 미측정 (gadgetron-testing benches 미구현)
- §2.C.1 dev mode PostgreSQL 처리 방식 미정 (SQLite 호환 모드 vs 필수 Docker)

### Phase C — (예정) — 8 subagents
**결론**: TBD

### Phase D — (예정) — PM consolidation
**결론**: TBD

### v1 승인 — (예정) — PM
**결론**: TBD
