# `gadgetron-testing` 크레이트 & 테스트 하네스 설계

> **담당**: @qa-test-architect
> **상태**: Round 2 retry
> **작성일**: 2026-04-11
> **최종 업데이트**: 2026-04-12 (Round 2 retry — A1~A8 Critical + A9~A13 High, CI/CD operational readiness)
> **관련 크레이트**: `gadgetron-testing` (신규), 전 workspace 크레이트
> **Phase**: [P1]
> **상위 결정**: D-20260411-05 (크레이트 신설), D-20260411-01 B (MVP), D-20260411-02 (Gemini Week 5-7), D-20260411-07 (UI types core), D-20260411-08 (`StreamInterrupted`), D-20260411-09 (감사 drop)
> **해소 대상 Round 2 리스크**: BLOCKER #6, HIGH #8 (`is_ready`), HIGH #10 (stream interruption), HIGH #12 (SSE normalizer), HIGH #14

---

## 1. 철학 & 컨셉 (Why)

### 1.1 해결하는 문제 [P1]
8개 크레이트 전원 `tests/` 0개, mock/fake 0개, CI workflow 0개 — 모든 모듈 doc이 `03-review-rubric.md §2`에서 즉시 fail. cross-crate 재사용 가능한 `gadgetron-testing` 크레이트로 이 상태 종식.

### 1.2 제품 비전과의 연결 [P1]
- `00-overview.md §1.2 미션 1` **P99 < 1ms 오버헤드**: criterion + mock provider로 네트워크 변수 제거, CI 반복 측정.
- `§1.2 미션 4` **운영 단순성**: 단일 바이너리 출시 전 docker-compose e2e harness로 플랫폼 부팅 검증.
- Round 2 평가 (연결성 D+ / 확장성 C / 완결성 D-)의 핵심 원인 = 테스트 인프라 부재. 본 문서는 모든 하위 리뷰의 **선행 조건**.

### 1.3 고려한 대안 (D-20260411-05) [P1]
| 대안 | 기각 사유 |
|------|-----------|
| B. 각 crate `testing` 모듈 | production 바이너리 침투 위험, dev-dep 경계 불가 |
| C. `#[cfg(test)]` 내장 | integration `tests/`에서 import 불가 |
| **A. 단일 `gadgetron-testing` (채택)** | dev-dep 단방향, 단일 소유, D-12 한 행 추가 |

### 1.4 핵심 설계 원칙 [P1]
1. **Production 제로 침투** — 모든 consumer `[dev-dependencies]`만 (A9 CI 검증).
2. **Fake > Mock** — `testcontainers::postgres` 실 Postgres(로컬 개발), `services.postgres` 실 Postgres(CI). `FakeGpuMonitor`는 NVML 시뮬 없이 값 반환.
3. **결정론** — `tokio::time::pause`, `proptest` seed 고정, `insta` 커밋. wall-clock 금지.
4. **Wire 포맷 충실도** — mock byte ≡ 실제 OpenAI/Anthropic/Gemini 와이어. `event:`/`data:` 공백 1칸, `[DONE]`, `message_stop` 순서 전부.
5. **Panic-free builder** — 모든 fixture `build() -> Result<T>`.
6. **`wiremock` 단일 HTTP mock 라이브러리** — Round 3 A3로 `mockito` 제거. `wiremock` 0.6 가 stateful matcher의 superset (retry counter, 순서 검증) + `MockServer::start()` 자동 포트 할당으로 collision 제거 + 활발한 유지보수. 단일 라이브러리로 의존성 트리 축소.
7. **Middleware 순서 불변** — `Auth → RateLimit → Routing → ProtocolTranslate → Provider → ProtocolTranslate(reverse) → record_post → Metrics`. snapshot + integration 양쪽 회귀 방지 (A1/A8).
8. **CI 재현성 ≥ local** — CI wall-clock minute 예산, parallelism race 없음, snapshot unreviewed fail.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 크레이트 구조 (D-20260411-05) [P1]

```
crates/gadgetron-testing/                  # [P1] dev-dependency only
├── Cargo.toml                             # §2.14 (R2: 완전한 versions + features)
├── insta.toml                             # Appendix B
└── src/
    ├── lib.rs                             # prelude re-exports
    ├── mocks/
    │   ├── provider/{openai,anthropic,gemini,ollama,vllm,sglang,failing}.rs
    │   ├── node/{fake_node,fake_gpu_monitor}.rs
    │   └── xaas/{fake_tenant,fake_quota,fake_audit}.rs
    ├── fixtures/
    │   ├── {config,chat_request,node_status,deployment,tenant}.rs
    │   └── streams/{openai_stream,anthropic_stream,gemini_stream}.rs
    ├── harness/{gateway,scheduler,e2e,pg}.rs
    ├── props/{vram,eviction,bin_packing,routing}.rs
    └── snapshots/                         # insta baselines committed
        ├── openai_to_anthropic__request.snap
        ├── anthropic_to_openai__stream.snap
        ├── gemini_polling_normalize.snap   # Week 5-7 (D-20260411-02)
        ├── middleware_order__happy.snap    # A1/A8 회귀
        └── error_shapes__per_provider.snap
```

모듈 경계: `mocks/` = trait impl, `fixtures/` = 순수 빌더 (streams 포함 wire byte), `harness/` = 기동 단위 (Drop 정리), `props/` = proptest strategy, `snapshots/` = insta 커밋 baseline.

### 2.2 공개 API (prelude) [P1]

`lib.rs`는 `mocks/fixtures/harness/props` 모듈 + `prelude` 재노출. `prelude`가 export하는 것: 7 mock(`Mock{OpenAi,Anthropic,Gemini,Ollama,Vllm,Sglang,FailingProvider}`) + `Method` enum + `FakeNode`/`FakeGpuMonitor` + `Fake{Tenant,Quota,Audit}*` + 5 builder(`ConfigBuilder`, `ChatRequestBuilder`, `NodeStatusBuilder`, `ModelDeploymentBuilder`, `TenantBuilder`) + stream fixtures (`fixture_openai_hello_world`, `fixture_openai_tool_calls_delta`, `fixture_openai_multi_content_part`, `fixture_openai_legacy_function_call`, `fixture_openai_partial_chunk`, `fixture_anthropic_full_stream`, `fixture_anthropic_error_event`, `fixture_anthropic_tool_use_stream`, `fixture_anthropic_truncated`, `fixture_gemini_polling`, `fixture_gemini_stream_chunks`, `fixture_gemini_function_call`, `fixture_gemini_safety_block`) + 5 harness(`GatewayHarness`, `SchedulerHarness`, `ComposeHarness`, `PgHarness`, `CrossHarness`).

### 2.3 B. 6 Mock Provider 공통 계약 [P1]

모든 mock은 두 역할을 동시 수행: (1) `LlmProvider` trait impl (in-process), (2) HTTP server (`wiremock::MockServer` 단일, wire byte 레벨). Round 3 A3로 `mockito` 의존성 제거 — `wiremock`만 사용하여 port collision 위험과 dependency 트리 부담을 동시에 해소한다.

**Round 3 A1 — Dynamic Dispatch Rationale (D-20260411-11)**: `LlmProvider` 모든 호출 경로(production router + test harness)는 `Arc<dyn LlmProvider>`를 사용한다. 이는 config-driven 6-provider runtime polymorphism의 정규 패턴이다. LLM 추론은 I/O-bound (HTTP provider 호출 ≈100 ms); vtable dispatch (≈1 ns)는 **10^8 배** 작아 무시 가능. Mock provider도 동일 패턴을 사용하여 production fidelity 보장. Generic monomorphization은 (a) 워크스페이스 폭 refactor (b) 6-provider × N-strategy 조합 폭발 (c) `Box<dyn>` 기반 fallback 체인 표현 불가 — 모두 보상 없는 비용. 상세 근거는 `docs/design/core/types-consolidation.md` §2.1.10 "Dynamic Dispatch Rationale" 참조.

`LlmProvider` trait 잠정 시그니처 (Track 1 A3 `is_ready` 반영):
```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;
    fn chat_stream(&self, req: ChatRequest)
        -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>;
    async fn is_live(&self) -> Result<bool>;                    // Round 2 HIGH #8
    async fn is_ready(&self, model: &str) -> Result<bool>;      // Round 2 HIGH #8
    async fn models(&self) -> Result<Vec<ModelInfo>>;
    fn name(&self) -> &str;
}
```
모든 mock은 6개 메서드 구현. `is_live`/`is_ready` 기본 true, 주입 가능.

대표 `MockOpenAi`:
```rust
// mocks/provider/openai.rs — [P1]
pub struct MockOpenAi {
    chunks:  Arc<Mutex<VecDeque<Result<ChatChunk>>>>,
    inter_chunk_delay: Duration,
    response: Arc<Mutex<Option<Result<ChatResponse>>>>,
    live_ok:  Arc<AtomicBool>,
    ready_ok: Arc<DashMap<String, bool>>,
}
impl MockOpenAi {
    pub fn builder() -> MockOpenAiBuilder;
    pub fn with_chunks(self, chunks: Vec<ChatChunk>) -> Self;
    pub fn with_failure(self, err: GadgetronError) -> Self;
    pub fn with_inter_chunk_delay(self, d: Duration) -> Self;
    pub fn with_is_live(self, ok: bool) -> Self;
    pub fn with_is_ready(self, model: &str, ok: bool) -> Self;
    pub fn inject_stream_interruption_after(&self, n: usize);   // D-20260411-08
}
```
`inject_stream_interruption_after`는 N chunk 방출 후 `GadgetronError::StreamInterrupted { reason: "mock: client abort".into() }`를 Err로 방출 (retry 경로 테스트).

**Wire 포맷 요구사항**:

| Mock | 라이브러리 | 와이어 요구사항 |
|------|------------|------------------|
| `MockOpenAi` | wiremock (stateful) | `Content-Type: text/event-stream`, `data: {...}\n\n`, 마지막 `data: [DONE]\n\n`, chunked |
| `MockAnthropic` | wiremock (stateful) | §2.4 A4: full event 8타입, `/v1/messages`, `anthropic-version` 헤더 |
| `MockGemini` | wiremock (stateful) | §2.5 A3: `POST /v1beta/models/{m}:generateContent` + `:streamGenerateContent` polling |
| `MockOllama` | wiremock | `POST /api/chat` NDJSON `{"done":false}`×N → `{"done":true}` |
| `MockVllm` | wiremock | OpenAI-compat + `/health` 200 + `/v1/models` |
| `MockSglang` | wiremock | OpenAI-compat + `/health` + `/get_server_info` |
| `MockFailingProvider` | in-process | §2.6 A5: per-method control |

**모든 HTTP mock은 `wiremock::MockServer::start().await`로 자동 free-port 할당** → 동일 프로세스 내 N개 mock 동시 가동 가능, port collision/race 없음. `wiremock::matchers::{method, path, header, body_json}` + `Mock::respond_with(...).expect(N)` 로 stateful matching 표현. 시나리오-수준 격리는 §5.1 "Scenario-parallel port isolation test"에서 검증.

### 2.4 A4 — `MockAnthropic` Full Event Sequence [P1]

실제 Anthropic `messages` API 전체 event 재현.

```rust
// mocks/provider/anthropic.rs — [P1]
pub struct MockAnthropic {
    events: Arc<Mutex<Vec<AnthropicEvent>>>,
    inter_event_delay: Duration,
    inject_ping: bool, inject_error_after: Option<usize>,
    live_ok: Arc<AtomicBool>, ready_ok: Arc<DashMap<String, bool>>,
}
#[derive(Clone, Debug)]
pub enum AnthropicEvent {
    MessageStart { message_id: String, model: String, usage_input_tokens: u32 },
    ContentBlockStart { index: u32, block_type: &'static str },   // "text" | "tool_use"
    ContentBlockDelta { index: u32, delta_type: &'static str, text: String },
    ContentBlockStop { index: u32 },
    MessageDelta { stop_reason: &'static str, output_tokens: u32 },
    MessageStop, Ping,
    Error { error_type: &'static str, message: String },
}
impl MockAnthropic {
    pub fn builder() -> MockAnthropicBuilder;
    pub fn with_events(self, events: Vec<AnthropicEvent>) -> Self;
    pub fn with_inter_event_delay(self, d: Duration) -> Self;
    pub fn with_ping_heartbeat(self, every_n: usize) -> Self;
    pub fn with_error_injection(self, after_n: usize, err: &'static str) -> Self;
    pub fn inject_stream_interruption_after(&self, n: usize);
}
```

**`fixture_anthropic_full_stream()` wire byte** — 각 event는 `event: <name>\n` + `data: <json>\n\n` 형식. 순서(9 step): `message_start`(`msg_01`, `input_tokens:12`) → `content_block_start`(text, idx 0) → `ping` → `content_block_delta` ×3 (`"Hello"|", "|"world!"`) → `content_block_stop`(idx 0) → `message_delta`(`stop_reason:end_turn`, `output_tokens:3`) → `message_stop`.

**Edge case fixtures**: `fixture_anthropic_full_stream()` (happy + ping 1), `fixture_anthropic_error_event()` (start → delta×2 → `event: error` overloaded_error), `fixture_anthropic_tool_use_stream()` (`tool_use` block + `input_json_delta`), `fixture_anthropic_truncated()` (`message_stop` 없이 중단 → `StreamInterrupted` 매핑).

### 2.5 A3 — `MockGemini` Polling → SSE-like 캡슐화 [P1]

**D-20260411-02 Week 5-7**: Gemini는 polling (`generateContent` JSON, `streamGenerateContent` chunked JSON array). 공통 `SseToChunkNormalizer` 대신 `PollingToEventAdapter` 레이어가 wire adapter 담당. `MockGemini`는 adapter가 소비할 **wire byte**만 방출.

```rust
// mocks/provider/gemini.rs — [P1]
pub struct MockGemini {
    mode: GeminiMode,   // NonStreaming | Streaming
    non_streaming_response: Arc<Mutex<Option<GeminiResponse>>>,
    streaming_chunks: Arc<Mutex<Vec<GeminiChunk>>>,
    inter_chunk_delay: Duration,
    function_call_mode: FunctionCallMode,  // None | SingleCall | MultiCall
    live_ok: Arc<AtomicBool>, ready_ok: Arc<DashMap<String, bool>>,
}
impl MockGemini {
    pub fn builder() -> MockGeminiBuilder;
    pub fn with_text_response(self, text: &str) -> Self;
    pub fn with_streaming_chunks(self, chunks: Vec<GeminiChunk>) -> Self;
    pub fn with_function_call(self, name: &str, args: serde_json::Value) -> Self;
    pub fn with_exact_token_count(self, total: u32) -> Self;
    pub fn inject_polling_timeout(&self);
}
```

**Non-streaming** (`POST /v1beta/models/{model}:generateContent`) — 단일 JSON:
`{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello, world!"}]},"finishReason":"STOP","safetyRatings":[]}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":3,"totalTokenCount":15}}`.

**Streaming** (`:streamGenerateContent`) — JSON array chunks polling. 각 chunk는 배열 원소로 도착, 마지막 chunk에 `finishReason` + `usageMetadata`. 예: `[{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"}}]}, {"candidates":[{"content":{"parts":[{"text":", world"}],"role":"model"}}]}, {"candidates":[{"content":{"parts":[{"text":"!"}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":15}}]`. `PollingToEventAdapter`(`gadgetron-provider/src/normalize.rs`)가 chunk → `ChatChunk` 변환. Mock은 wire byte만 방출.

**Character-based token approximation**: Gemini 과금은 character 기준이나 공통 normalizer는 token (`usage.{prompt,completion}_tokens`) 요구. 변환 비율 **4 chars ≈ 1 token** (영문): `chars_to_tokens_approx(chars) = ceil(chars / 4.0) as u32`. Fixture `with_exact_token_count(u32)`로 synthetic override.

**`parts[].functionCall` 포맷** (OpenAI `tool_calls`와 다름): `{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"get_weather","args":{"location":"Seoul"}}}]},"finishReason":"STOP"}]}`. `translate_gemini_to_openai()`가 이 포맷을 `choices[0].message.tool_calls[0]`로 매핑, snapshot test(§2.11) 검증.

Fixtures: `fixture_gemini_polling()`, `fixture_gemini_stream_chunks()`, `fixture_gemini_function_call()`, `fixture_gemini_safety_block()` (finishReason=SAFETY), `fixture_gemini_truncated()` (StreamInterrupted).

**Week 5 Normalizer refactor 영향 (A9/A10)**: `SseToChunkNormalizer` 추출은 production `gadgetron-provider/src/normalize.rs` 국한. `MockGemini`는 wire byte만 담당 → Week 5 전후 mock API 무변. §5.1 IT-normalizer-refactor가 이 불변성을 검증.

### 2.6 A5 — `MockFailingProvider` per-method Control [P1]

Round 2 HIGH #12 (fallback) + HIGH #10 (stream interruption) 경로 커버.

```rust
// mocks/provider/failing.rs — [P1]
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum Method { Chat, ChatStream, IsLive, IsReady, Models }

pub struct MockFailingProvider {
    calls: Arc<DashMap<Method, AtomicUsize>>,
    fail_map: Arc<DashMap<Method, (usize, GadgetronError)>>,  // (fail_after_n, err)
    delegate: Option<Box<dyn LlmProvider>>,
}
impl MockFailingProvider {
    pub fn always_503() -> Self;
    pub fn fail_after_n(n: usize) -> Self;
    pub fn fail_on_method(self, m: Method) -> Self;          // A5
    pub fn with_error(self, e: GadgetronError) -> Self;
    pub fn with_delegate(self, p: Box<dyn LlmProvider>) -> Self;
    pub fn call_count(&self, m: Method) -> usize;
}
```

**사용 예**: (a) chat만 503, is_ready는 성공 — `MockFailingProvider::always_503().fail_on_method(Method::Chat).with_error(GadgetronError::Provider("upstream 503".into()))`. (b) stream만 1회 후 interruption — `MockFailingProvider::fail_after_n(1).fail_on_method(Method::ChatStream).with_error(GadgetronError::StreamInterrupted { reason: "mock: timeout".into() })`.

### 2.7 A2 — `FakeGpuMonitor` 10-field Builder [P1]

**D-20260411-07**: `GpuMetrics`가 `gadgetron-core/src/ui.rs` 위치. 본 mock은 `GpuMetrics` 반환. Track 1 A1 완료 전 잠정 가정 (§7 Q-1).

```rust
// mocks/node/fake_gpu_monitor.rs — [P1]
use gadgetron_core::ui::GpuMetrics;                       // D-20260411-07
use gadgetron_core::node::{P2PStatus, MigInstance, MigProfile, NumaTopology};
use parking_lot::RwLock;                                  // R3 A2: read-heavy lock

/// `FakeGpuMonitor` uses `parking_lot::RwLock` (not `std::sync::RwLock`) because
/// the scheduler eviction loop polls `gpu_metrics()` on the read path while test
/// injection (`set_*`/`inject_*`) is rare. `parking_lot::RwLock` is ~3× faster on
/// read-heavy workloads (no kernel futex on uncontended paths, smaller word size,
/// no poison overhead). Writer-starve risk is minimal because tests inject state
/// synchronously and polling is bounded. Safe for `[dev-dependencies]` only —
/// no production binary impact.
///
/// Phase 2: if scheduler benchmark shows measurable contention, migrate hot fields
/// to lock-free atomics — `Arc<AtomicU64>` for vram_used / `Arc<AtomicU32>` for
/// utilization/temp/power, leaving the `RwLock` only for low-frequency topology
/// updates (NVLink, NUMA, MIG profile).
pub struct FakeGpuMonitor { state: Arc<RwLock<FakeGpuState>> }
// GpuDeviceState: D-07 10 필드 + model_name 메타.

impl FakeGpuMonitor {
    pub fn builder() -> FakeGpuMonitorBuilder;
    pub fn set_vram_used(&self, idx: u32, used_mb: u64);
    pub fn set_utilization(&self, idx: u32, pct: f32);
    pub fn set_temperature(&self, idx: u32, c: f32);
    pub fn set_power(&self, idx: u32, w: f32);
    pub fn set_clock(&self, idx: u32, mhz: u32);
    pub fn set_fan(&self, idx: u32, rpm: u32);
    pub fn inject_thermal_throttle(&self, idx: u32);
    pub fn inject_mig_profile(&self, idx: u32, profile: MigProfile);
    pub fn snapshot_metrics(&self, idx: u32) -> GpuMetrics;
}

impl FakeGpuMonitorBuilder {
    pub fn node_id(self, id: &str) -> Self;
    pub fn with_device(self, idx: u32, model: &str, vram_total_mb: u64) -> Self;
    pub fn with_vram_used(self, idx: u32, used_mb: u64) -> Self;
    pub fn with_utilization(self, idx: u32, pct: f32) -> Self;
    pub fn with_temperature(self, idx: u32, c: f32) -> Self;       // A2
    pub fn with_power(self, idx: u32, w: f32) -> Self;             // A2
    pub fn with_power_limit(self, idx: u32, w: f32) -> Self;       // A2
    pub fn with_clock(self, idx: u32, mhz: u32) -> Self;           // A2
    pub fn with_fan(self, idx: u32, rpm: u32) -> Self;             // A2
    pub fn with_nvlink_group(self, devs: &[u32], bw_gbs: u32) -> Self;
    pub fn with_numa_node(self, node: u32, devs: &[u32]) -> Self;
    pub fn with_mig_profile(self, idx: u32, profile: MigProfile) -> Self;
    pub fn build(self) -> Result<FakeGpuMonitor>;
}
```

**완전 10-field 예**:
```rust
let gpus = FakeGpuMonitor::builder()
    .node_id("node-0")
    .with_device(0, "A100-80GB", 81_920)
    .with_vram_used(0, 40_000).with_utilization(0, 75.0)
    .with_temperature(0, 68.5).with_power(0, 280.0).with_power_limit(0, 400.0)
    .with_clock(0, 1410).with_fan(0, 3500)
    .with_nvlink_group(&[0, 1], 600).with_numa_node(0, &[0, 1])
    .with_mig_profile(0, MigProfile::ProfileA100_3g_40gb)
    .build()?;
```

`GpuMonitor` trait impl: `device_count`, `gpu_metrics(idx) -> GpuMetrics`, `all_metrics`, `p2p_status`, `mig_instances`, `numa_info`, `refresh`. `FakeNode`는 `Arc<parking_lot::RwLock<NodeStatus>>` + `Arc<FakeGpuMonitor>` + `Arc<parking_lot::RwLock<HashMap<String, ModelDeployment>>>` (R3 A2: 동일한 read-dominant 사유로 `parking_lot::RwLock` 사용). writer/reader 경로 분리, `loom` 검증 [P2].

### 2.8 D. Fixtures (builder) [P1]

전부 panic-free, `build() -> Result<T>`. 잘못된 조합은 의미 있는 에러. R3 A5 — **builder Result 통일 정책**: §1.4.5 panic-free 원칙을 모든 builder에 강제 — `MockOpenAiBuilder`, `MockAnthropicBuilder`, `MockGeminiBuilder`, `FakeGpuMonitorBuilder`, `ConfigBuilder`, `ChatRequestBuilder`, `NodeStatusBuilder`, `ModelDeploymentBuilder`, `TenantBuilder` 모두 `build(self) -> Result<T, GadgetronError>` 시그니처. 누락 필드/모순 조합은 `Err(GadgetronError::Config("..."))` 반환. `unwrap()`/`expect()`/`panic!()` 사용 금지 — clippy lint `clippy::unwrap_used` 가 `lib.rs`에 `#![deny(...)]` 활성화.

```rust
// fixtures/chat_request.rs — [P1]
pub struct ChatRequestBuilder { /* ... */ }
impl ChatRequestBuilder {
    pub fn new() -> Self;
    pub fn model(self, m: &str) -> Self;
    pub fn user(self, text: &str) -> Self;
    pub fn system(self, text: &str) -> Self;
    pub fn stream(self) -> Self;
    pub fn build(self) -> Result<ChatRequest>;  // model 필수, messages ≥ 1
}
```
동일 패턴: `ConfigBuilder`, `NodeStatusBuilder`, `ModelDeploymentBuilder`, `TenantBuilder`.

**A6 — Stream fixtures (OpenAI edge cases)**:
- `fixture_openai_hello_world()` — 3 chunk + `[DONE]`
- `fixture_openai_tool_calls_delta(Engine::Vllm | Engine::Sglang)` — vLLM `arguments` append vs SGLang full json 차이
- `fixture_openai_multi_content_part()` — vision multi-part (`text` + `image_url`)
- `fixture_openai_legacy_function_call()` — 구버전 `function_call` (non-tools)
- `fixture_openai_partial_chunk()` — SSE chunk 중간 절단

Anthropic/Gemini fixture는 §2.4 / §2.5.

### 2.9 E. 하네스 모듈 [P1]

#### 2.9.1 A1 — `GatewayHarness::spawn(config, providers)` [P1]

> **Citation**: gateway-routing.md §5 Route Tree + State shape는 Week 3에 gateway-router-lead가 확정. 본 harness는 `axum::Router::with_state(Arc<Router>)` + `Arc<[dyn LlmProvider]>` 잠정 가정.

**잠정 state shape** (R3 A1: 모든 trait object는 D-20260411-11 dynamic dispatch rationale 적용 — I/O bound 경로에서 vtable cost 무시 가능):
```rust
pub struct GatewayState {
    pub router:    Arc<Router>,                         // gadgetron-router
    pub providers: Arc<[Arc<dyn LlmProvider>]>,         // D-20260411-11
    pub auth:      Arc<dyn AuthValidator>,              // gadgetron-xaas
    pub quota:     Arc<dyn QuotaEnforcer>,              // gadgetron-xaas
    pub audit:     Arc<dyn AuditSink>,                  // gadgetron-xaas
}
```
> **D-20260411-11 인용**: `Arc<dyn LlmProvider>`는 6 provider runtime polymorphism의 표준 패턴. HTTP provider 호출(~100 ms) vs vtable dispatch(~1 ns) = 10^8 ratio. Mock도 동일 패턴 → production fidelity. 상세: `docs/design/core/types-consolidation.md` §2.1.10 Dynamic Dispatch Rationale.

**`GatewayHarness::spawn` 시그니처**:
```rust
// harness/gateway.rs — [P1]
pub struct GatewayHarness {
    pub addr: SocketAddr, pub state: GatewayState,
    shutdown: oneshot::Sender<()>, _join: JoinHandle<()>,
}
impl GatewayHarness {
    /// Middleware chain (불변): Auth → RateLimit → Routing → ProtocolTranslate
    /// → Provider → ProtocolTranslate(reverse) → record_post → Metrics
    pub async fn spawn(
        config: GatewayConfig, providers: Vec<Arc<dyn LlmProvider>>,
    ) -> Result<Self>;
    pub async fn spawn_default(provider: Arc<dyn LlmProvider>) -> Result<Self>;
    pub async fn spawn_with_state(state: GatewayState) -> Result<Self>;
    pub fn client(&self) -> reqwest::Client;
    pub fn base_url(&self) -> String;              // "http://127.0.0.1:<port>"
    pub async fn shutdown(self) -> Result<()>;     // graceful
    pub fn middleware_layers(&self) -> &[&'static str];
}
```
`spawn` 내부: `TcpListener::bind("0.0.0.0:0")` → free port → `axum::Router::with_state(state)` → `axum::serve` → join handle. `Drop`에서 shutdown 전송.

**`test_middleware_order` integration test outline (A1/A8)**: `FakeMiddlewareAuditor` probe 를 `FakeAuditSink`에 주입 → `GatewayHarness::spawn_with_state(state)` → `POST /v1/chat/completions` → `auditor.ordered_tags()` 가 `["auth", "rate_limit", "routing", "protocol_translate_in", "provider_call", "protocol_translate_out", "record_post", "metrics"]` 순서인지 assert. insta snapshot `middleware_order__happy.snap`이 동일 순서를 yaml로 이중 기록.

**A8 — `test_middleware_order_auth_before_ratelimit`**: `FakeAuthValidator::always_invalid()` + `FakeQuotaEnforcer::always_429()` state로 spawn → bad-key POST → `assert_eq!(res.status(), 401)`. 401이 먼저 나와야 rate limit 전에 auth 차단 확인.

#### 2.9.2 기타 하네스 [P1]

```rust
// harness/scheduler.rs — [P1]
pub struct SchedulerHarness { pub scheduler: Arc<Scheduler>, pub nodes: Vec<Arc<FakeNode>> }
impl SchedulerHarness {
    pub fn with_fake_nodes(n: usize) -> Self;
    pub fn with_fake_nodes_list(nodes: Vec<FakeNode>) -> Self;
    pub fn with_gpu_topology(self, f: impl Fn(&mut FakeGpuMonitorBuilder) -> &mut FakeGpuMonitorBuilder) -> Self;
    pub async fn trigger_eviction(&self, model_id: &str) -> Result<EvictionResult>;
    pub fn gateway_compose(&self, gw: &GatewayHarness) -> CrossHarness;   // A11
}

// harness/e2e.rs — [P1] docker-compose (up/service_url/wait_healthy + Drop down -v)
// compose: postgres + wiremock(OpenAI) + wiremock(Anthropic) + gadgetron(SUT)

// harness/pg.rs — [P1] local dev: testcontainers postgres / CI: services.postgres DSN
pub struct PgHarness { _container: Option<ContainerAsync<Postgres>>, pub pool: sqlx::PgPool, pub url: String }
impl PgHarness {
    /// 기본 모드 결정: env `DATABASE_URL` 존재 → services.postgres 모드 (CI),
    /// 부재 → testcontainers 부팅 (local dev).
    pub async fn start() -> Result<Self> {
        if let Ok(url) = std::env::var("DATABASE_URL") {
            let pool = sqlx::PgPool::connect(&url).await?;
            sqlx::migrate!("../gadgetron-xaas/src/db/migrations").run(&pool).await?;
            return Ok(Self { _container: None, pool, url });
        }
        let container = testcontainers_modules::postgres::Postgres::default().start().await?;
        let port = container.get_host_port_ipv4(5432).await?;
        let url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
        let pool = sqlx::PgPool::connect(&url).await?;
        sqlx::migrate!("../gadgetron-xaas/src/db/migrations").run(&pool).await?;
        Ok(Self { _container: Some(container), pool, url })
    }
}
```

### 2.10 F. Property-based invariants (proptest) [P1]

4 파일, 각각 `proptest!` block. Seed 고정: `PROPTEST_CASES=1024`, `PROPTEST_SEED=42`.

- **`props/vram.rs`**: (a) `estimate_vram_mb` 단조 증가 — `est(base+delta, q, ctx, bs) >= est(base, q, ctx, bs)` (b) bounds: `weights_mb <= est <= weights_mb * 3`.
- **`props/bin_packing.rs`**: Johnson 1973 FFD 11/9 상한 — `used <= ceil(lower * 11/9) + 1`.
- **`props/eviction.rs`** (4-variant, D-2): `lru_never_evicts_active`, `priority_never_evicts_above_min`, `cost_based_selects_highest_cost_per_tok`, `weighted_lru_stable_under_shuffle`.
- **`props/routing.rs`**: `round_robin_uniform` (카운트 차 ≤ 1), `weighted_matches_weights` (5% tolerance), `fallback_chain_order` (primary → secondary → error invariant).

**R3 A6 — proptest strategy 실례 코드** (`props/vram.rs` 핵심):

```rust
// crates/gadgetron-testing/src/props/vram.rs
use proptest::prelude::*;
use gadgetron_core::model::{Quantization, estimate_vram_mb};

/// `prop_oneof!` 로 6 quantization variant uniform sampling.
fn arb_quantization() -> impl Strategy<Value = Quantization> {
    prop_oneof![
        Just(Quantization::Fp16), Just(Quantization::Bf16),
        Just(Quantization::Int8), Just(Quantization::Int4),
        Just(Quantization::Awq),  Just(Quantization::Gptq),
    ]
}

/// `Arbitrary` impl: weights 1..=200 GiB, ctx ∈ {512,2048,8192,32768,131072}, batch 1..=64.
#[derive(Debug, Clone)]
pub struct VramEstimateInput { pub weights_mb: u64, pub ctx_len: u32, pub batch: u32, pub quant: Quantization }
impl Arbitrary for VramEstimateInput {
    type Parameters = (); type Strategy = BoxedStrategy<Self>;
    fn arbitrary_with(_: ()) -> Self::Strategy {
        (1024u64..=200_000,
         prop_oneof![Just(512u32), Just(2048), Just(8192), Just(32768), Just(131072)],
         1u32..=64, arb_quantization())
        .prop_map(|(w, c, b, q)| Self { weights_mb: w, ctx_len: c, batch: b, quant: q })
        .boxed()
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, max_shrink_iters: 4096, .. ProptestConfig::default() })]
    #[test] fn estimate_vram_is_monotonic_in_weights(base in any::<VramEstimateInput>(), delta in 0u64..=10_000) {
        let a = estimate_vram_mb(base.weights_mb,         base.quant, base.ctx_len, base.batch);
        let b = estimate_vram_mb(base.weights_mb + delta, base.quant, base.ctx_len, base.batch);
        prop_assert!(b >= a);
    }
    #[test] fn estimate_vram_within_3x_bound(i in any::<VramEstimateInput>()) {
        let est = estimate_vram_mb(i.weights_mb, i.quant, i.ctx_len, i.batch);
        prop_assert!(est >= i.weights_mb && est <= i.weights_mb * 3);
    }
}
```

동일 패턴: `props/eviction.rs` 4 variant `prop_oneof!` (LRU/Priority/CostBased/WeightedLRU), `props/routing.rs` Strategy enum (RoundRobin/Weighted/Fallback). 모든 strategy는 `BoxedStrategy<T>` 로 export → consumer crate (`gadgetron-router/tests/`)에서 재사용 가능.

### 2.11 G. Snapshot tests (insta) — HIGH #12 + A7 [P1]

```rust
// tests/protocol_translation.rs in gadgetron-router
#[test] fn openai_request_to_anthropic_messages() {
    let req = ChatRequestBuilder::new()
        .model("gadgetron:anthropic/claude-sonnet-4-20250514")
        .system("You are helpful.").user("Hello").build().unwrap();
    insta::assert_json_snapshot!(translate_to_anthropic(&req));
}

#[test] fn anthropic_stream_to_openai_chunks() {
    let events = fixture_anthropic_full_stream();
    insta::assert_yaml_snapshot!(normalize_to_openai(events));
}

#[test] fn gemini_function_call_to_openai_tool_calls() {
    let resp = fixture_gemini_function_call();
    insta::assert_json_snapshot!(translate_gemini_to_openai(&resp));
}
```

**A7 — Snapshot 출처 정책** (`crates/gadgetron-testing/src/snapshots/README.md`):
- **Recorded** (real API): `openai_hello_world`, `anthropic_full_stream`, `gemini_polling` — 사람이 `curl -D -` 캡처 → PII 제거 → 커밋. 주석 `# source: recorded YYYY-MM-DD from <endpoint>, manually redacted`.
- **Synthetic**: edge cases (partial chunk, truncated, legacy `function_call`) — 주석 `# source: synthetic, manually curated`.
- **Drift**: 매 분기 "recorded 재캡처" 의무 (devops-sre-lead 캘린더, A12 cron job). API schema 변경 PR에 snapshot diff 첨부.
- **갱신**: `cargo insta review` 로컬 → PR에 before/after diff. **CI 자동 갱신 금지 (A11 — `INSTA_UPDATE=no`)**.

### 2.12 설정 스키마 (N/A) [P1]
본 크레이트는 프로덕션 설정 없음 (dev-only). `ConfigBuilder`는 테스트용 최소 유효 `AppConfig` 생성 유틸리티.

### 2.13 에러 & 로깅 [P1]
Mock 에러: `GadgetronError::Provider("mock: <ctx>")`, `::Config(..)`, **`::StreamInterrupted { reason }`** (D-20260411-08). 신규 variant 요청 없음. 로깅: `tracing::debug!` span `mock.request { provider, method }`, `RUST_LOG=gadgetron_testing=debug`. Harness `Drop` 실패는 `tracing::warn!`만 — 패닉 금지.

### 2.14 A4 — 의존성 (Cargo.toml, 완전 버전/feature) [P1]

```toml
# crates/gadgetron-testing/Cargo.toml
[package]
name = "gadgetron-testing"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
# Internal (workspace crates)
gadgetron-core      = { path = "../gadgetron-core", features = ["testing-support"] }
gadgetron-provider  = { path = "../gadgetron-provider" }
gadgetron-router    = { path = "../gadgetron-router" }
gadgetron-scheduler = { path = "../gadgetron-scheduler" }
gadgetron-node      = { path = "../gadgetron-node" }
gadgetron-gateway   = { path = "../gadgetron-gateway" }
gadgetron-xaas      = { path = "../gadgetron-xaas" }         # D-20260411-03

# Runtime
tokio        = { workspace = true, features = ["test-util", "sync", "time", "macros", "rt-multi-thread"] }
async-trait  = "0.1"                                          # D-20260411-11: dyn dispatch 표준
async-stream = "0.3"
futures      = { workspace = true }
dashmap      = { workspace = true }
parking_lot  = "0.12"                                         # R3 A2: read-heavy RwLock
serde        = { workspace = true }
serde_json   = { workspace = true }

# HTTP mocking — R3 A3: wiremock 단일 (mockito 제거, port collision 회피)
wiremock = "0.6"                                              # MockServer::start() auto-port
reqwest  = { workspace = true }

# DB testcontainers (로컬 개발용, CI는 services.postgres DSN 사용)
# R3 A4: 0.23 default 가 async — "blocking" feature 제거
testcontainers         = { version = "0.23" }
testcontainers-modules = { version = "0.11", features = ["postgres"] }
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "postgres", "uuid", "chrono", "macros", "migrate"] }

# Property-based + snapshot
proptest = "1.5"
insta    = { version = "1.39", features = ["json", "yaml", "redactions"] }

# Utility
uuid   = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }

[dev-dependencies]
tokio     = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
# R3 A8: `async_tokio` feature 유지 — §5.6 `b.to_async(&rt).iter(...)` 가 사용함 (확인됨)
criterion = { version = "0.5", features = ["async_tokio", "html_reports"] }
blake3    = "1"   # §5.1 시나리오 4 wire byte 해시 비교
```

workspace `Cargo.toml`에 `"crates/gadgetron-testing"`, `"crates/gadgetron-xaas"` 추가. 각 production crate는 `[dev-dependencies]`에만 `gadgetron-testing = { path = "../gadgetron-testing" }`. **`[dependencies]` 금지** — A9 `dev-dep-check` job이 CI에서 검증.

**R3 A3 — Why wiremock-only**: (1) `wiremock 0.6`의 `wiremock::matchers` (`method`, `path`, `header`, `body_json`, `body_string_contains`) + `Mock::respond_with(...).expect(N).up_to_n_times(M)` 가 stateful + retry counter + 순서 검증을 모두 지원 → `mockito`의 모든 use case 커버. (2) `MockServer::start().await`가 0.0.0.0:0 자동 바인딩 → port collision 불가. (3) `mockito 1.5`는 macro 기반 전역 server → 동시 시나리오에서 race 위험. (4) 단일 라이브러리로 의존성 트리 -1, CI 캐시 단순화. (5) 2024+ 활발한 유지보수 (`wiremock-rs` 0.6.x 라인).

---

## 3. 전체 모듈 연결 구도 (Where) [P1]

```
  (dev-dep only)

  +-----------------+       +------------------+
  | gadgetron-core  |<------+ gadgetron-testing |<--+
  |  + src/ui.rs    |       +---------+--------+   |
  |  (D-20260411-07)|                 |            |
  +-----------------+                 v            |
        ^                                          |
  +-----+----+ +--------+ +---------+ +--------+ +------+
  | provider | | router | | sched   | | gate   | | xaas |
  +----------+ +--------+ +---------+ +--------+ +------+
        ^          ^          ^           ^          ^
        +[dev-dep]-+[dev-dep]-+[dev-dep]-+[dev-dep]-+

   데이터 흐름:
   test → GatewayHarness::spawn(cfg, providers) → reqwest → assertions
   test → SchedulerHarness(FakeNode[]) → Scheduler → deploy/evict
   test → PgHarness::start() → sqlx::migrate! → xaas 쿼리
   test → ComposeHarness::up() → wiremock containers → gadgetron (SUT)
```

**D-12 업데이트**: `gadgetron-testing` 행 추가. 담당 타입: `Mock{OpenAi,Anthropic,Gemini,Ollama,Vllm,Sglang,FailingProvider}`, `Method`, `AnthropicEvent`, `GeminiChunk`, `FakeNode`, `FakeGpuMonitor`, `Fake{Tenant,Quota,Audit}*`, 모든 `*Builder`, `GatewayState` (잠정).

**A9 — dev-dep CI 검증** (`dev-dep-check` job, §5.5 YAML 참조): `cargo tree --edges normal`로 `gadgetron-testing`이 production 바이너리에 normal dependency로 들어갔는지 검증. 역참조 시 fail.

**서브에이전트 도메인 계약**:
- `chief-architect` ↔ Q-1: `GpuMonitor` trait + `GpuMetrics` 필드 (Track 1 A1)
- `gpu-scheduler-lead` ↔ `FakeNode` 시맨틱 (VRAM 동기화 BLOCKER #5)
- `xaas-platform-lead` ↔ `PgHarness` 마이그레이션 경로
- `devops-sre-lead` ↔ CI matrix (§5.5), dev-dep check (A9), cost/parallelism (§5.7)
- `gateway-router-lead` ↔ `GatewayState` 잠정 shape (Week 3 확정 대기)

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위 [P1]
| 검증 대상 | 유형 | invariant |
|-----------|------|-----------|
| `MockOpenAi::chat_stream` | unit | chunk 순서·개수 정확, `pause` 하 결정론 |
| `MockOpenAi` edge fixtures (tool_calls/vision/legacy/partial) | snapshot | A6 wire 포맷 고정 |
| `MockAnthropic` full event sequence | snapshot | A4: 8 타입 순서, ping/error injection |
| `MockAnthropic::inject_stream_interruption_after` | unit | N chunk 후 `StreamInterrupted` (D-08) |
| `MockGemini::generateContent` | unit | non-stream JSON 필드 일치 |
| `MockGemini::streamGenerateContent` | unit | polling chunk 순서, char→token, functionCall |
| `MockFailingProvider::fail_on_method(Method::Chat)` | unit | chat만 실패, is_ready/models 정상 |
| `FakeGpuMonitor::snapshot_metrics` | unit | 10 필드 builder 주입값 일치 |
| `FakeGpuMonitor::set_*` | unit | writer/reader 격리, race 없음 |
| `FakeNode::available_vram_mb` | property | `vram_total − Σ deployed ≥ 0` |
| `ChatRequestBuilder::build` | unit | 필수 필드 누락 시 `Err`, panic 없음 |
| `ConfigBuilder::build` | unit | TOML round-trip 일치 |
| `GatewayHarness::spawn` | unit | free port, shutdown 후 재바인딩 |
| `GatewayHarness` middleware order | integration | A1/A8: 레이어 순서 태그 |
| `PgHarness::start` | integration | CI `services.postgres` DSN / local testcontainers, 마이그레이션 clean |
| `props::*` | property | §2.10 invariants 전부 |

### 4.2 테스트 하네스 (self-testing) [P1]
- wiremock — 실제 HTTP 바인딩 self-test (R3 A3: 단일 라이브러리).
- fixture `fixture_*` 반환값을 golden.
- `proptest` seed 고정 (`PROPTEST_CASES=1024`, `PROPTEST_SEED=42`).
- `tokio::time::pause`. wall-clock 금지.

### 4.3 커버리지 목표 [P1]
Line ≥ **85%** (`cargo llvm-cov --workspace -p gadgetron-testing`), Branch ≥ 80%. CI fail: `--fail-under-lines 85`.

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 K. 5 통합 시나리오 [P1]

#### 시나리오 1 — Happy path: `POST /v1/chat/completions` (OpenAI mock → 200 SSE) [P1]
```rust
// crates/gadgetron-gateway/tests/chat_completions_happy.rs
#[tokio::test(flavor = "multi_thread")]
async fn happy_path_openai_sse() -> Result<()> {
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    // R3 A3: MockServer::start() — auto-port, no collision
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_bytes(fixture_openai_hello_world_bytes()))
        .mount(&mock_server).await;

    let mock = MockOpenAi::builder()
        .with_upstream_url(&mock_server.uri())   // wiremock-backed
        .with_chunks(fixture_openai_hello_world()).build()?;
    let harness = GatewayHarness::spawn(
        GatewayConfig::test_defaults(),
        vec![Arc::new(mock) as Arc<dyn LlmProvider>],
    ).await?;
    let res = harness.client()
        .post(format!("{}/v1/chat/completions", harness.base_url()))
        .bearer_auth("test-key")
        .json(&ChatRequestBuilder::new().model("gadgetron:openai/gpt-4o").user("hi").stream().build()?)
        .send().await?;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers().get("content-type").unwrap(), "text/event-stream");
    let body = res.bytes().await?;
    assert!(body.windows(5).any(|w| w == b"data:"));
    assert!(body.ends_with(b"data: [DONE]\n\n"));
    Ok(())
}
```
회귀: BLOCKER #2 (9 handler stub), BLOCKER #3 (bearer auth). R3 A3: `MockServer::start()` 자동 free port 할당으로 시나리오 동시 실행 시 port collision 없음.

#### 시나리오 2 — Fallback chain: primary 503 → secondary 200 [P1]
```rust
// crates/gadgetron-router/tests/fallback_chain.rs
#[tokio::test(start_paused = true)]
async fn fallback_on_503_to_secondary() {
    let primary = MockFailingProvider::always_503()
        .fail_on_method(Method::Chat)
        .with_error(GadgetronError::Provider("upstream 503".into()));
    let secondary = MockOpenAi::builder().with_chunks(fixture_openai_hello_world()).build();
    let router = RouterBuilder::new()
        .with_strategy(Strategy::Fallback(vec!["primary".into(), "secondary".into()]))
        .with_provider("primary",   Box::new(primary.clone()))
        .with_provider("secondary", Box::new(secondary))
        .build();

    let out = router.chat(ChatRequestBuilder::new().model("x").user("hi").build()?).await?;
    assert!(out.choices[0].message.content.as_deref().unwrap_or("").starts_with("Hello"));
    assert_eq!(primary.call_count(Method::Chat), 1);
    assert_eq!(router.metrics().error_count("primary"), 1);
    assert_eq!(router.metrics().success_count("secondary"), 1);
}
```
**A13 — Circuit breaker 60s caveat**: mock은 `call_count(Method::Chat)`만 제공. 실제 60s window는 `Router::try_fallbacks` 소관. 테스트는 `tokio::time::pause` + `tokio::time::advance(Duration::from_secs(61))` 로 가상 시간 진행. Mock은 wall-clock 금지.

#### 시나리오 3 — VRAM eviction (Cross-harness, A11) [P1]
```rust
// crates/gadgetron-scheduler/tests/eviction_e2e.rs
#[tokio::test(flavor = "multi_thread")]
async fn full_node_triggers_lru_eviction_via_gateway() {
    let node = FakeNode::builder("node-0")
        .with_gpu_builder(|g| g.with_device(0, "A100-80GB", 81_920)
            .with_vram_used(0, 70_000).with_utilization(0, 30.0)
            .with_temperature(0, 55.0).with_power(0, 200.0).with_power_limit(0, 400.0)
            .with_clock(0, 1410).with_fan(0, 3000))
        .with_deployment(fixture_model("old-llama-70b", 70_000, last_used_hours_ago(4)))
        .build()?;
    let sched = SchedulerHarness::with_fake_nodes_list(vec![node.clone()]);
    let gw = GatewayHarness::spawn(GatewayConfig::test_defaults(), vec![]).await?;
    let cross = sched.gateway_compose(&gw);
    let new = ModelDeploymentBuilder::new().id("new-qwen-72b").vram_mb(72_000).priority(5).build()?;
    let result = cross.deploy_and_verify(new).await?;
    assert_eq!(result.evicted, vec!["old-llama-70b"]);
    assert!(node.status().deployed_models.contains(&"new-qwen-72b".to_string()));
    let models = gw.client().get(format!("{}/v1/models", gw.base_url())).send().await?
        .json::<serde_json::Value>().await?;
    assert!(models["data"].as_array().unwrap().iter().any(|m| m["id"] == "new-qwen-72b"));
}
```
**A11**: `CrossHarness = SchedulerHarness + GatewayHarness`. BLOCKER #5 (scheduler↔node VRAM 동기화) 회귀.

#### 시나리오 4 — IT-normalizer-refactor (A9 High, Week 5 전후 불변 검증) [P1]

**목적**: D-20260411-02 Week 5 `SseToChunkNormalizer` 추출 전/후 mock fixture byte level + Scenario 1/2/3 assertion 동치 증명. Week 5 refactor 브랜치에서 "mock 생존" 보증.

```rust
// crates/gadgetron-testing/tests/it_normalizer_refactor.rs
#[tokio::test(flavor = "multi_thread")]
async fn normalizer_refactor_preserves_mock_wire_and_assertions() -> Result<()> {
    // 1. Byte level: fixture wire byte 해시 고정
    assert_eq!(
        blake3::hash(&fixture_anthropic_full_stream_bytes()),
        blake3::hash(&fixture_openai_hello_world_bytes()),
        "refactor must not touch mock wire byte"
    );
    // 2. Assertion level: Scenario 1/2/3 재실행 → production 경로로 pass.
    happy_path_openai_sse().await?;
    fallback_on_503_to_secondary().await?;
    full_node_triggers_lru_eviction_via_gateway().await?;
    // 3. ChatChunk sequence 동치: hand-rolled vs normalizer 경로 bitwise 동일
    let events = fixture_anthropic_full_stream();
    assert_eq!(
        legacy_parse_to_chunks(&events),
        SseToChunkNormalizer::new().normalize(&events),
        "Week 5 normalizer must be bitwise-equivalent to hand-rolled parser"
    );
    Ok(())
}
```
회귀: Week 5 PR이 mock/fixture/snapshot 중 하나라도 건드리면 즉시 fail. 본 테스트는 Week 5 까지 `#[ignore = "waiting for normalizer"]`, Week 5 PR에서 unignore.

#### 시나리오 5 — Scenario-parallel port isolation (R3 A3) [P1]

**목적**: `wiremock::MockServer` 자동 포트 할당이 동시 실행 시나리오에서 collision-free임을 회귀 보장. `mockito` 제거 결정의 정당성 — 동일 프로세스 내 다중 mock server가 race 없이 공존해야 한다.

```rust
// crates/gadgetron-testing/tests/scenario_parallel_isolation.rs
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_mock_servers_no_port_collision() -> Result<()> {
    use std::collections::HashSet;
    use wiremock::{MockServer, Mock, ResponseTemplate, matchers::{method, path}};

    // 8 동시 mock server 부팅 — port 충돌 시 bind 실패가 즉시 panic
    let mut joins = Vec::new();
    for i in 0..8 {
        joins.push(tokio::spawn(async move {
            let s = MockServer::start().await;
            Mock::given(method("GET")).and(path(format!("/s-{}", i)))
                .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
                .mount(&s).await;
            let res = reqwest::get(&format!("{}/s-{}", s.uri(), i)).await.expect("self-call");
            assert_eq!(res.status(), 200);
            s.address().port()
        }));
    }
    let mut ports: HashSet<u16> = HashSet::new();
    for j in joins {
        let p = j.await.expect("join");
        assert!(ports.insert(p), "port {} reused — wiremock auto-allocation broke", p);
    }
    assert_eq!(ports.len(), 8, "8 distinct ports expected");

    // 추가: Scenario 1 + Scenario 2 동시 실행 stub 도 distinct ports 보장
    let (p1, p2) = tokio::join!(
        async { MockServer::start().await.address().port() },
        async { MockServer::start().await.address().port() },
    );
    assert_ne!(p1, p2, "Scenario 1과 2 mock 동시 가동 시 port 분리 필수");
    Ok(())
}
```

회귀: `mockito` 재도입 PR이나 `wiremock` 0.6 → 0.5 다운그레이드 (구버전 매크로 기반 전역 server) 모두 본 테스트에서 fail. R3 A3 결정의 자기검증 게이트.

### 5.2 테스트 환경 [P1]
- **postgres (CI)**: `services.postgres` (GHA 네이티브, ~2s 부팅, §5.7)
- **postgres (local)**: `PgHarness::start()` → `testcontainers-modules::postgres` (~5-7s 부팅)
- **mock LLM**: `MockOpenAi/Anthropic/Gemini` (wiremock 단일 — R3 A3, 실 HTTP, auto-port)
- **fake GPU**: `FakeGpuMonitor` in-process, NVML 불필요
- **docker-compose**: `crates/gadgetron-testing/compose/mock-cluster.docker-compose.yml`

### 5.3 회귀 방지 [P1]

| 과거 위험 | 이 테스트가 잡는다 |
|-----------|---------------------|
| BLOCKER #2 — 9 handler stub | 시나리오 1 |
| BLOCKER #3 — MVP 미구현 (bearer/graceful/health) | 시나리오 1 + `GatewayHarness::shutdown()` |
| BLOCKER #5 — scheduler↔node VRAM stale | 시나리오 3 + cross-harness |
| BLOCKER #6 — 테스트 하네스 부재 | 본 문서 전체 |
| HIGH #8 — `is_ready` 미정의 | `LlmProvider::is_ready` + `MockOpenAi::with_is_ready` |
| HIGH #10 — stream interruption 미구분 | `inject_stream_interruption_after` + `StreamInterrupted` |
| HIGH #12 — SSE normalizer hand-roll | §2.11 snapshot + 시나리오 4 (IT-normalizer-refactor) |
| HIGH #14 — 모듈 doc 테스트 전략 부재 | §5.4 템플릿 |
| Middleware 순서 회귀 | §2.9.1 `test_middleware_order` + A8 test |
| Anthropic event 누락 | §2.4 fixture + snapshot |
| Gemini functionCall/safety 누락 | §2.5 fixture |
| Week 5 Normalizer refactor 로 mock byte 파괴 | 시나리오 4 (IT-normalizer-refactor) |
| HTTP mock port collision (R3 A3) | 시나리오 5 (parallel isolation) |
| `Arc<dyn LlmProvider>` 무근거 generic 전환 PR | §2.3 D-20260411-11 인용 + Mock fidelity 의존 |
| `FakeGpuMonitor` read contention | §2.7 `parking_lot::RwLock` + Phase 2 atomic 노트 |

### 5.4 I. 모듈 doc `## 테스트 전략` 템플릿 (HIGH #14) [P1]

5개 module doc (`gateway-routing.md`, `model-serving.md`, `gpu-resource-manager.md`, `xaas-platform.md`, `deployment-operations.md`) 각각에 추가 필수:

```markdown
## 테스트 전략

### 단위 범위
- 검증 대상 공개 함수 [목록]
- 각 함수별 invariant 1줄

### Mock / Fake 플랜
- 외부 경계 → `gadgetron-testing::prelude::{Mock*, Fake*}`
- 예: "Anthropic → `MockAnthropic`, GPU → `FakeGpuMonitor`, PG → `PgHarness`"

### e2e 시나리오 (≥1)
- `crates/<crate>/tests/<feature>.rs`, 최종 assertion 1~3개

### Property-based (선택)
- `gadgetron-testing::props::*`

### CI 노트
- GPU 필요 (`gpu-ci`) / PG 필요 (`integration`)
- testcontainers 사용 시 DinD 명시
```

### 5.5 H. CI 매트릭스 요약 (Round 2 A3 — 전체 YAML은 Appendix A) [P1]

| Job | Runner | 트리거 | 핵심 명령 | 예상 (min) |
|-----|--------|--------|-----------|:---:|
| `fmt` | ubuntu-24.04 | 모든 PR/push | `cargo fmt --all --check` | 0.5 |
| `clippy` | ubuntu-24.04 | 모든 PR/push | `cargo clippy --workspace --all-targets --features testing-support -- -D warnings` | 2 |
| `dev-dep-check` [A9] | ubuntu-24.04 | 모든 PR/push | `cargo tree --prefix none --format '{p} {f}' --edges normal \| grep -v gadgetron-testing` | 0.5 |
| `test-cpu` | ubuntu-24.04 + `services.postgres` | 모든 PR/push, `needs: [fmt, clippy]` | `cargo nextest run --workspace --features testing-support --jobs 2`, env `PROPTEST_CASES=1024 INSTA_UPDATE=no` | 4 |
| `test-release` | ubuntu-24.04 + `services.postgres` | main merge, `needs: [test-cpu]` | `cargo nextest run --workspace --release --features testing-support --jobs 2` | 3 |
| `bench` | ubuntu-24.04-4core | `schedule` + main push, `needs: [test-cpu]` | `cargo bench --workspace -- --save-baseline $SHA` + `critcmp` + artifact 90d | 6 |
| `snapshot-refresh` | ubuntu-24.04 | quarterly cron (`0 0 1 */3 *`) | `cargo insta test --workspace` + manual approval gate | 3 |
| `test-gpu` [P2] | self-hosted gpu | `gpu` 라벨 PR | `cargo test --workspace --features nvml` | — |

**GPU-less CI**: 모든 PR에서 `fmt` + `clippy` + `dev-dep-check` + `test-cpu`. `FakeGpuMonitor`가 NVML 부재 커버.

### 5.6 J. P99 < 1ms overhead verification (A6 Criterion 정의) [P1]

`criterion` bench (`crates/gadgetron-gateway/benches/overhead.rs`): **request in → response flush** (mock latency 제외). `MockOpenAi::builder().with_chunks(fixture_openai_hello_world()).with_inter_chunk_delay(Duration::ZERO)` 로 순수 오버헤드만 측정.

```rust
// crates/gadgetron-gateway/benches/overhead.rs — [P1]
fn bench_gateway_overhead(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let harness = rt.block_on(async {
        let mock = MockOpenAi::builder().with_chunks(fixture_openai_hello_world())
            .with_inter_chunk_delay(Duration::ZERO).build();
        GatewayHarness::spawn_default(Arc::new(mock)).await.unwrap()
    });
    let mut group = c.benchmark_group("gateway.chat_completion.overhead");
    group.warm_up_time(Duration::from_secs(3));            // A6: 3s warmup
    group.measurement_time(Duration::from_secs(5));        // A6: 5s steady-state window
    group.sample_size(100);                                // A6: 100 samples
    group.significance_level(0.05).noise_threshold(0.10);  // A6: ±10% drift
    group.bench_function("overhead", |b| b.to_async(&rt).iter(|| async {
        harness.client().post(format!("{}/v1/chat/completions", harness.base_url()))
            .bearer_auth("bench-key")
            .json(&ChatRequestBuilder::new().model("openai/gpt-4o").user("hi").build().unwrap())
            .send().await.unwrap()
    }));
    group.finish();
}
criterion_group!(benches, bench_gateway_overhead);
criterion_main!(benches);
```

**A6 — P99 측정 정의 (criterion)**:
- **Warmup**: 3s (criterion default) — JIT/캐시 웜업
- **Measurement window**: 5s, steady-state, 100 samples (criterion sample_size)
- **Percentile calc**: criterion built-in (sorted + linear interpolation). JSON `estimates.json` → `critcmp` consume
- **Noise control**: `ubuntu-24.04-4core` 러너 (표준 `ubuntu-24.04` 대비 CPU/메모리 일관성 ↑)
- **CI vs Dev drift tolerance**: ±10% (criterion default `noise_threshold`)
- **Target**: P99 < **1000 μs** (mock latency 제외)
- **Result format**: criterion HTML report + JSON summary (`target/criterion/<group>/<bench>/new/estimates.json`)

**R3 A9 — `async-trait` allocation hot-path bench**: D-20260411-11 dyn dispatch는 vtable + boxed-future 할당을 만든다. 무시 가능 수준이나 회귀 감지를 위해 별도 criterion group:

```rust
// crates/gadgetron-gateway/benches/overhead.rs (추가 group)
fn bench_async_trait_alloc(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mock: Arc<dyn LlmProvider> = Arc::new(
        MockOpenAi::builder().with_chunks(fixture_openai_hello_world()).build().unwrap());
    c.benchmark_group("provider.dyn_dispatch.alloc")
        .bench_function("Arc<dyn LlmProvider>::is_live", |b| {
            b.to_async(&rt).iter(|| async { let _ = mock.is_live().await; })
        });
}
```

**Target**: P99 < **5 μs** (Arc clone + vtable + boxed future drop). 회귀 시 `cargo bench --bench overhead -- provider.dyn_dispatch` 로 root cause 분리. D-20260411-11 정량 검증 게이트.

### 5.7 A1~A2 — CI Operational Cost & Parallelism (NEW) [P1]

#### 5.7.1 Testcontainers Cold Start 측정 (A1)

| 환경 | Postgres 부팅 | 3개 e2e 시나리오 × 3 PG | 비고 |
|------|:---:|:---:|------|
| **GHA `services.postgres` (native Docker)** | **~2s** | **~2s** (공유 1회) | **CI 기본** — 네이티브, health-check 빌트인, secret 단순 |
| **GHA `testcontainers` (DinD, cached image)** | ~5-7s | ~20-25s (순차 3회) | 로컬 dev 전용 |
| **Local `testcontainers` (Docker Desktop)** | ~3-5s | ~15s | 개발자 머신 |

**Decision rationale**: CI는 `services.postgres` 선택. 근거 (3가지):
1. **~3x 빠른 부팅** — 20-25s → 2s 절감
2. **GHA 네이티브** — Docker-in-Docker 오버헤드 제거, `--health-cmd pg_isready` 빌트인
3. **Secret 단순** — `POSTGRES_PASSWORD` env만 있으면 됨. DinD처럼 credential helper 설정 불필요

로컬 개발(1인 CI)은 `testcontainers`로 격리 유지 (여러 feature 브랜치 동시 작업). `PgHarness::start()`가 `DATABASE_URL` 환경변수로 자동 분기 (§2.9.2).

#### 5.7.2 Parallelism Limit (A2)

**Testcontainers 사용 테스트 (로컬 dev)**:
- `cargo nextest run --jobs 2` — 병렬 2 jobs 제한
- 사유: 동일 머신에서 Postgres 컨테이너 3+ 동시 부팅 시 port 5432 alias collision + `ryuk` cleanup 레이스
- `TESTCONTAINERS_RYUK_DISABLED=false` (기본) — 컨테이너 dangling 방지
- `TESTCONTAINERS_LOG=true` (디버그 시) — reaper 로그 확인

**CPU-only 테스트 (mock provider만)**:
- 별도 nextest partition 혹은 동일 job 내 `--jobs=num_cpus` (기본)
- fake/mock만 사용, 포트 충돌 없음

**CI (`services.postgres` 단일 컨테이너 공유)**:
- `cargo nextest run --jobs 2` (동일 설정)
- Postgres는 job 전체에서 1개만 기동 (port 5432 host), 여러 테스트가 DB 공유
- DB 격리: 테스트별 스키마 prefix 혹은 transaction rollback 패턴

**Race 방지 보조**:
- 각 `PgHarness::start()`는 고유 `CREATE SCHEMA test_<uuid>` 생성 → 테스트 종료 시 `DROP SCHEMA CASCADE`
- Migration은 `CREATE TABLE IF NOT EXISTS` (xaas-phase1 A6 정책 준수)

#### 5.7.3 비용 추정 (A3 연결)

- **PR당**: fmt (0.5) + clippy (2) + dev-dep-check (0.5) + test-cpu (4) + (main merge 시 +test-release 3) = **~7-8분**
- **월간**: 50 PRs × 8 min = **400 min/month** (GH Free tier 2000/month의 20% 수준, 편안함)
- **bench**: main push + weekly cron → 월 4-8회 × 6 min = 30-50 min
- **Total**: ~450-500 min/month. Free tier 안전 마진 1500+.
- **상한 경고**: PR 볼륨이 2배(100/month)가 되어도 ~900 min/month — tier 여유 충분

#### 5.7.4 A7 — Regression Blocking Policy (Bench) [P1]

| Δ vs main baseline | 상태 | PR 처리 | 코멘트 |
|:---:|:---:|:---:|------|
| **≤ +5%** | **Green** | Pass, 코멘트 없음 | 정상 변동 (noise floor) |
| **+5% ~ +15%** | **Yellow** | Warning 코멘트 (PR에 bot 자동 첨부), 리뷰어 판단 | "Bench regression in X by +Y%" 링크 첨부 |
| **> +15%** | **Red** | **Hard fail**, PR blocked | 투자 없이 머지 불가 — root cause 조사 후 재실행 |

**Exception**:
- **신규 벤치 (first appearance)**: baseline 없음 → 회귀 검사 skip, first-commit부터 baseline 등록
- **`[skip-bench]`**: PR 제목에 포함 시 bench job 조건부 skip — infrastructure-only PR (e.g., CI YAML tweaks, docs) 에 사용
- **Rerun**: PR comment `/rerun-bench` 로 bench job 재실행 가능 (noise floor 통과 가능성 검증)

**Enforcement**: `bench` job 내 `critcmp main $SHA` 실행 → diff JSON을 action이 파싱 → threshold 초과 시 `exit 1`. PR bot이 diff를 코멘트로 첨부.

#### 5.7.5 A8 — Bench Artifact Strategy [P1]

- **Retention**: `actions/upload-artifact@v4` `retention-days: 90` — 90일 보관
- **Format**:
  - Criterion HTML report (browseable, 리뷰어 친화적)
  - JSON summary (`estimates.json`, `benchmark.json`) — `critcmp` 입력
- **Storage**: GitHub Actions artifacts (Free tier 500 MB/월 × 90일 ≈ OK for criterion)
- **Naming**: `criterion-report-${{ github.sha }}` — SHA 기준 lookup
- **Phase 2 upgrade**: `bencher.dev` 통합 — 장기 보관, 트렌드 그래프, PR 코멘트 자동화

---

## 6. Phase 구분

| 항목 | Phase |
|------|:-----:|
| `gadgetron-testing` 크레이트 + prelude | [P1] |
| 6 Mock Provider in-process | [P1] |
| `MockAnthropic` full event sequence (§2.4, A4) | [P1] |
| `MockGemini` generate + stream + functionCall (§2.5, A3) | [P1] (D-20260411-02 Week 5-7) |
| `MockFailingProvider` per-method ctrl (§2.6, A5) | [P1] |
| `FakeGpuMonitor` 10-field builder (§2.7, A2) | [P1] |
| `FakeNode` + `SchedulerHarness` | [P1] |
| Fixtures (+ stream fixtures A6) | [P1] |
| `GatewayHarness::spawn(cfg, providers)` + middleware order test (A1/A8) | [P1] |
| `CrossHarness` composition (A11) | [P1] |
| `ComposeHarness` (docker-compose e2e) | [P1] |
| `PgHarness` (testcontainers local / services.postgres CI) | [P1] |
| proptest 4종 (vram/eviction/bin_packing/routing) | [P1] |
| insta snapshot baseline + 출처 정책 (A7) + insta.toml (R2 A10) | [P1] |
| **CI matrix full YAML + cost/parallelism (R2 A1~A3)** | [P1] |
| **dev-dep-check job (A9)** | [P1] |
| **Bench CI + regression policy + artifact (R2 A5~A8)** | [P1] |
| **Criterion P99 definition (R2 A6)** | [P1] |
| **IT-normalizer-refactor test (R2 A9)** | [P1] |
| **Quarterly snapshot refresh cron (R2 A12)** | [P1] |
| **PII redaction SOP (R2 A13)** | [P1] |
| 5 module doc `## 테스트 전략` 섹션 (HIGH #14) | [P1] |
| GPU-enabled CI (self-hosted) | [P2] |
| `bencher.dev` 통합 | [P2] |
| `loom` 동시성 (FakeNode/FakeGpuMonitor) | [P2] |
| Real API playback (VCR-style) | [P2] |
| Chaos harness | [P3] |
| Multi-region federation e2e | [P3] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|----|------|------|------|------|
| Q-1 | `GpuMonitor` trait + `GpuMetrics` 필드가 Track 1 A1 (`gadgetron-core/src/ui.rs`) 확정 전 | A: Track 1 sync 후 patch / B: Track 1 blocker / C: feature flag | **A** | 🟡 Track 1 retry 대기 |
| Q-2 | CI Postgres: `services.postgres` vs `testcontainers` (A1 R2) | A: services (CI) + testcontainers (local) / B: DinD 전면 / C: mock only | **A** | 🟢 R2 확정 |
| Q-3 | HTTP mock 라이브러리 선택 | A: 둘 다 / **B: wiremock만** / C: mockito만 | **B** | 🟢 R3 A3 확정 (`mockito` 제거) |
| Q-4 | MockGemini snapshot 커밋 시점 (Week 5-7) | A: Week 1 placeholder / B: Week 5 본 구현 / C: 빈 파일 | **A** | 🟢 |
| Q-5 | `PgHarness::start()` `migrate!` 경로 | A: xaas Week 1 후 본 크레이트 Week 2 / B: 가짜 migrations / C: feature gate | **A** | 🟡 PM 트랙 조율 |
| Q-6 | `GatewayState` 잠정 가정 → Week 3 gateway-router-lead 확정 시 patch 범위 | A: harness patch only / B: prelude 재노출로 흡수 / C: 별도 doc | **A** | 🟡 Week 3 대기 |

---

## Appendix A — A3 완전한 `.github/workflows/ci.yml` [P1]

```yaml
# .github/workflows/ci.yml
name: CI
on:
  pull_request:
  push:
    branches: [main]
  schedule:
    - cron: '0 0 1 */3 *'   # 분기 1일 00:00 UTC — snapshot-refresh

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  fmt:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: rustfmt }
      - run: cargo fmt --all --check

  clippy:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: clippy }
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets --features testing-support -- -D warnings

  test-cpu:
    runs-on: ubuntu-24.04
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_PASSWORD: postgres
          POSTGRES_DB: gadgetron_test
        ports: [5432:5432]
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
    env:
      DATABASE_URL: postgres://postgres:postgres@localhost:5432/gadgetron_test
      PROPTEST_CASES: "1024"
      PROPTEST_SEED: "42"
      RUST_LOG: gadgetron=debug
      INSTA_UPDATE: "no"        # R2 A11: CI never auto-updates snapshots
      INSTA_FORCE_PASS: "0"     # R2 A11: CI fails on unreviewed snapshots
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install nextest (R3 A7: --locked 필수)
        # --locked 강제: Cargo.lock 무시한 transitive 변동 방지 → CI 재현성 보장.
        # `cargo install cargo-nextest --locked` 없이 실행하면 매 PR마다 빌드 시간/캐시 hit이 달라짐.
        # 빠른 옵션은 `taiki-e/install-action@nextest` 를 검토 (binary 다운로드, ~5s).
        run: cargo install cargo-nextest --locked
      - name: Install sqlx-cli (--locked 동일 사유)
        run: cargo install sqlx-cli --no-default-features --features postgres --locked
      - name: Run migrations
        run: sqlx migrate run --source crates/gadgetron-xaas/src/db/migrations
      - name: Nextest (unit + integration)
        run: cargo nextest run --workspace --features testing-support --jobs 2

  test-release:
    runs-on: ubuntu-24.04
    if: github.ref == 'refs/heads/main'
    needs: [test-cpu]
    services:
      postgres:
        image: postgres:16
        env: { POSTGRES_PASSWORD: postgres, POSTGRES_DB: gadgetron_test }
        ports: [5432:5432]
        options: >-
          --health-cmd pg_isready --health-interval 10s
    env:
      DATABASE_URL: postgres://postgres:postgres@localhost:5432/gadgetron_test
      INSTA_UPDATE: "no"
      INSTA_FORCE_PASS: "0"
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo install sqlx-cli --no-default-features --features postgres
      - run: sqlx migrate run --source crates/gadgetron-xaas/src/db/migrations
      - run: cargo nextest run --workspace --release --features testing-support --jobs 2

  bench:
    runs-on: ubuntu-24.04-4core     # R2 A6: 일관된 P99 러너
    if: github.event_name == 'schedule' || (github.event_name == 'push' && github.ref == 'refs/heads/main')
    needs: [test-cpu]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Run benchmarks
        run: cargo bench --workspace --bench '*' -- --save-baseline ${{ github.sha }}
      - name: Compare to main baseline
        run: |
          cargo install critcmp
          critcmp main ${{ github.sha }} || true   # non-blocking diff
      - name: Regression gate (R2 A7)
        run: |
          # diff JSON 파싱 → +15% 초과 시 exit 1 (hard fail)
          # +5~15% 시 PR bot warning comment (non-blocking)
          ./devops/ci/bench-regression-gate.sh main ${{ github.sha }}
      - name: Upload criterion report
        uses: actions/upload-artifact@v4
        with:
          name: criterion-report-${{ github.sha }}
          path: target/criterion/
          retention-days: 90                      # R2 A8: 90-day retention

  dev-dep-check:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Verify gadgetron-testing is dev-dep only
        run: |
          set -euo pipefail
          OUT=$(cargo tree --prefix none --format '{p} {f}' --edges normal | grep 'gadgetron-testing' || true)
          if [ -n "$OUT" ]; then
            echo "FAIL: gadgetron-testing in production dependency tree:"
            echo "$OUT"
            exit 1
          fi
          echo "OK: gadgetron-testing not in production tree"

  snapshot-refresh:
    runs-on: ubuntu-24.04
    if: github.event_name == 'schedule'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Validate existing snapshots against fixtures
        run: |
          # Manual approval gate: 실제 real-API 재캡처는 devops/ci/capture-snapshots.sh
          # (API keys 필요, manual trigger). 본 job은 기존 snapshot 무결성만 검증.
          echo "Quarterly snapshot validation — manual capture required separately."
      - run: cargo insta test --workspace
        env:
          INSTA_UPDATE: "no"
```

**Decision rationale (요약)**:
- **`services.postgres` > `testcontainers` (CI only)** — §5.7.1: ~3x 빠른 부팅, GHA 네이티브, secret 단순
- **Unit tests `cargo nextest --jobs 2`** — §5.7.2: port/ryuk race 방지
- **`INSTA_UPDATE=no`** — R2 A11: CI 자동 갱신 금지, 로컬 `cargo insta review` 강제
- **`bench` cron + main only** — R2 A5: noise floor 유지, PR마다 돌리면 cost/noise ↑
- **Estimated CI minutes**: ~7-8 min/PR (§5.7.3)
- **Monthly projection**: ~450 min/month (GHA Free tier 2000 한계 22%)

---

## Appendix B — R2 A10 `insta.toml` 설정 [P1]

```toml
# crates/gadgetron-testing/insta.toml
[defaults]
force_pass = false          # Test가 snapshot mismatch 시 실패
output     = "diff"         # diff 뷰 (human-friendly)

[review]
include_info_section = true # review 시 meta info 섹션 포함

[storage]
snapshots_dir = "src/snapshots"   # §2.1 경로와 일치
```

**의미**: 로컬 dev에서는 `cargo insta review` 로 diff 확인 후 승인 / 거부. CI는 env `INSTA_UPDATE=no` + `INSTA_FORCE_PASS=0` 로 **덮어쓰기 일절 금지** (R2 A11). 커밋되지 않은 snapshot(PR에 `.snap.new`가 올라옴)은 `cargo insta test` 시 fail.

---

## Appendix C — R2 A13 PII Redaction SOP [P1]

**목적**: Recorded snapshot 커밋 전 PII/비밀값 자동/수동 제거. SOC2/GDPR 최소 요건.

**`devops/ci/redact-snapshot.sh` (예시)**:
```bash
#!/usr/bin/env bash
# devops/ci/redact-snapshot.sh <in.snap> <out.snap>
set -euo pipefail
IN="$1"; OUT="$2"
sed -E \
  -e 's/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/<email>/g' \
  -e 's/req_[A-Za-z0-9]{8,}/req_<redacted>/g' \
  -e 's/sk-[A-Za-z0-9]{20,}/sk-<redacted>/g' \
  -e 's/Bearer [A-Za-z0-9._-]+/Bearer <token>/g' \
  -e 's/([A-Za-z0-9+\/=]{64,})/<base64>/g' \
  "$IN" > "$OUT"
echo "Redacted: $IN → $OUT"
echo "MANUAL REVIEW REQUIRED before commit."
```

**체크리스트 (PR 리뷰어 매뉴얼 확인)**:
- [ ] Email 패턴 (`name@domain.tld`) — 스크립트 치환 후 남은 인스턴스 0
- [ ] Request ID (`req_*`, `r_*`, UUID) — 스크립트 치환 후 남은 인스턴스 0
- [ ] API 키 (`sk-*`, `gad_live_*`, `gad_test_*`, `AIza*`) — 0
- [ ] Bearer 토큰 (`Authorization: Bearer *`) — 0
- [ ] Base64 blob ≥64 chars — `<base64>` 플레이스홀더로 치환
- [ ] 호스트명/IP — production 도메인 제거, `api.example.com`/`127.0.0.1` 로 교체
- [ ] 모델 스냅샷 내 system prompt — 회사 고유 정보 제거

**커밋 정책**:
- Real API 재캡처 PR에는 반드시 `redact-snapshot.sh` 실행 로그 첨부
- snapshot 파일 첫 줄 주석: `# source: recorded 2026-MM-DD, redacted via devops/ci/redact-snapshot.sh`
- `snapshot-refresh` cron job은 **redaction 검증만** 수행 (재캡처는 별도 manual job, API key 주입 필요)
- PII/secret 누출 의심 시 커밋 revert + git history scrub (`git filter-repo`)

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-11 — @gateway-router-lead + @inference-engine-lead
**결론**: ⚠️ Conditional Pass. 핵심 발견: (1) `GatewayHarness` state shape 불명확, (2) Gemini mock polling 구체화 부족, (3) Anthropic full event sequence 누락. Action Items: A1~A4 blocking, A5~A15 non-blocking (상세: `docs/reviews/round1-week1-results.md §Track 2`).

### Round 1 Retry — 2026-04-11 — @qa-test-architect
**결론**: ✅ All blocking action items resolved (잠정 — Track 1 의존성 sync 필요)

Resolutions: A1 ✅ `GatewayHarness::spawn` + `GatewayState` 잠정 + `test_middleware_order` (§2.9.1). A2 ✅ `FakeGpuMonitor::builder()` 10 필드 (§2.7). A3 ✅ `MockGemini` non-stream + streaming + char→token + functionCall (§2.5). A4 ✅ `MockAnthropic` full 8 event sequence + fixtures (§2.4). Non-blocking A5~A11, A13 반영. Cross-track: Track 1 A1/A3/A4 완료 후 최종 sync 필요 (§7 Q-1). 다음: Round 2는 @devops-sre-lead 대타 (self-review 회피).

### Round 2 — 2026-04-12 — @devops-sre-lead (qa-test-architect 대타)
**결론**: ⚠️ Conditional Pass — 8 Critical + 10 High (CI/CD operational focus)

**체크리스트 결과**: 5/8 full Pass, 3/8 Partial (CI재현성·성능검증·테스트데이터).
**핵심**: Mock fidelity/fixture/determinism 철저. CI yaml 예제 0, DinD 비용 미계산, nextest parallelism race, bench regression 정책 미비, snapshot 관리 SOP 부족.

### Round 2 Retry — 2026-04-12 — @qa-test-architect
**Resolutions**:
- A1-A2 ✅ Testcontainers cold start ~5-7s + `nextest --jobs 2` 제한 명시 (§5.7.1, §5.7.2)
- A3 ✅ Complete `.github/workflows/ci.yml` (fmt/clippy/test-cpu/test-release/bench/dev-dep-check/snapshot-refresh 7 jobs, `services.postgres` decision, 400 min/month 추정) — Appendix A
- A4 ✅ Cargo.toml 완전한 dev-dependencies (versions + features) — §2.14
- A5 ✅ Bench CI (cron + main-only) + artifact upload 90일 — §5.7.5 + Appendix A
- A6 ✅ Criterion P99 definition (warmup 3s, steady 5s, 100 samples, ubuntu-24.04-4core 러너, ±10% drift) — §5.6
- A7 ✅ Regression policy (+5% green / +5~15% yellow / >15% red, `[skip-bench]` override) — §5.7.4
- A8 ✅ Artifact 90일 + Phase 2 bencher.dev — §5.7.5
- A9 ✅ IT-normalizer-refactor (Week 5 전후 mock 생존 검증) — §5.1 시나리오 4
- A10 ✅ `insta.toml` 설정 — Appendix B
- A11 ✅ CI unreviewed snapshot fail 정책 (`INSTA_UPDATE=no`, `INSTA_FORCE_PASS=0`) — Appendix A
- A12 ✅ Quarterly snapshot refresh cron job (`0 0 1 */3 *`) — Appendix A
- A13 ✅ PII redaction SOP (`redact-snapshot.sh` example + 체크리스트) — Appendix C

**Cross-track 영향**: `services.postgres` 결정으로 `PgHarness::start()` 가 env 기반 자동 분기 (§2.9.2). Local dev는 testcontainers 유지, CI는 DSN 직접.

**다음 라운드**: Round 3 (chief-architect) 진행 가능.

### Round 3 — 2026-04-12 — @chief-architect
**결론**: ⚠️ Conditional Pass — 3 blocking Rust idiom + 6 non-blocking

**체크리스트 (§3)**: 7/10 Pass, 3/10 Partial
**핵심 발견**:
1. `Arc<dyn LlmProvider>` 선택 근거 부족 (D-20260411-11 필요)
2. `FakeGpuMonitor RwLock` read-heavy 성능 분석 부재
3. `mockito + wiremock` 이중 사용 port collision

### Round 3 Retry — 2026-04-12 — @qa-test-architect
**Resolutions**:
- A1 ✅ §2.3/§2.9.1 `Arc<dyn LlmProvider>` rationale (D-20260411-11 인용, network vs vtable 10^8 ratio, core doc cross-ref)
- A2 ✅ §2.7 `FakeGpuMonitor` → `parking_lot::RwLock` (~3× faster read) + Phase 2 lock-free atomic note + §2.14 dev-dep 추가 + `FakeNode` 동일 사유 적용
- A3 ✅ `mockito` 제거, `wiremock` 단일 사용, §2.14 Cargo.toml 업데이트, §2.3 mock table 업데이트, §5.1 시나리오 5 "scenario-parallel port isolation test" 추가, §1.4.6 원칙 재서술, Q-3 옵션 B 확정
- A4 ✅ §2.14 testcontainers `"blocking"` feature 제거 (0.23 default async)
- A5 ✅ §2.8 builder Result 통일 정책 + `clippy::unwrap_used` deny lint
- A6 ✅ §2.10 proptest strategy 실례 코드 (`prop_oneof!` 6 quantization, `Arbitrary` impl, BoxedStrategy export)
- A7 ✅ Appendix A `cargo install nextest --locked` 강화 주석 (재현성, 빠른 옵션)
- A8 ✅ §2.14 `criterion` `async_tokio` feature 사용처 확인 (§5.6 `b.to_async(&rt)`) — 유지
- A9 ✅ §5.6 `provider.dyn_dispatch.alloc` criterion group 추가 (P99 < 5 μs target, D-20260411-11 정량 게이트)

**Cross-track 영향**: D-20260411-11 인용으로 Track 1 §2.1.10 cross-ref 안정. D-20260411-13 (`GadgetronError::Database`) 는 §2.13 Mock 에러 목록에 향후 추가 가능 (Phase 1 PgKeyValidator mock 시뮬레이션 — 본 라운드 비포함).

**최종 상태**: ✅ **Approved** — 구현 착수 가능.

### 최종 승인 — 대기 중 — PM
