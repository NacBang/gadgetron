# Gadgetron — 전체 설계 개요

> **버전**: 0.1.0
> **에디션**: 2021
> **라이선스**: MIT
> **최소 Rust**: 1.80
> **바이너리 이름**: `gadgetron`

---

## 1. 제품 비전과 차별점

### 1.1 비전

**"GPU 클러스터 위에서 동작하는, 서브밀리초 P99 레이턴시를 보장하는 AI 오케스트레이션 플랫폼"**

Gadgetron은 대규모 LLM 서비스 인프라를 단일 바이너리로 통합하는 Rust 네이티브 플랫폼입니다. 클라우드 API와 로컬 GPU 추론을 하나의 추상화 계층으로 묶고, GPU 리소스 스케줄링부터 에이전트 오케스트레이션까지 계층형 서비스(XaaS) 모델로 제공합니다.

### 1.2 미션

1. **레이턴시 제거** — GC pause, 직렬화 오버헤드, 네트워크 홉을 원천 차단하여 P99 < 1ms 오버헤드 달성
2. **GPU 자원 최적화** — NUMA 토폴로지, NVLink/NVSwitch 인터커넥트, 열/전력 예산을 고려한 스케줄링
3. **계층형 추상화** — GPUaaS -> ModelaaS -> AgentaaS로 점진적 추상화 제공
4. **운영 단순성** — 단일 바이너리 + TOML 설정 파일로 전체 플랫폼 구동, 마이크로서비스 난비 지양
5. **멀티 백엔드** — vLLM/SGLang을 1급 시민으로, Ollama/TGI/llama.cpp를 2급 시민으로 지원
6. **멀티 플랫폼** — Linux를 1순위, macOS를 개발용, Windows를 Phase 3로 단계적 지원

### 1.3 경쟁 차별화

#### LiteLLM vs Gadgetron

| 영역 | LiteLLM | Gadgetron |
|------|---------|-----------|
| 언어 | Python | Rust (GC 없음, P99 < 1ms) |
| GPU 관리 | 없음 | NVML + NUMA + MIG + 열/전력 |
| 로컬 추론 | 간접 지원 | vLLM/SGLang 1급 시민 |
| 배포 단위 | pip 패키지 | 단일 정적 바이너리 |
| 스트리밍 | Python 제너레이터 | 제로카피 `Pin<Box<Stream>>` |
| 스케줄링 | 없음 | VRAM 인식 모델 배포 + LRU eviction |
| 클러스터 | 없음 | 다중 노드, K8s/Slurm 통합 |

#### Portkey vs Gadgetron

| 영역 | Portkey | Gadgetron |
|------|---------|-----------|
| 배포 모델 | SaaS 전용 | 자체 호스팅 (온프레미스 가능) |
| GPU 관리 | 없음 | 전체 GPU 수명주기 관리 |
| 로컬 추론 | 없음 | vLLM/SGLang/Ollama 네이티브 |
| 레이턴시 | 네트워크 홉 존재 | 인프라 내부 오버헤드 최소화 |
| 데이터 주권 | 클라우드 종속 | 완전한 데이터 통제 |

#### OpenRouter vs Gadgetron

| 영역 | OpenRouter | Gadgetron |
|------|------------|-----------|
| 모델 소싱 | 클라우드 API 전용 | 클라우드 + 로컬 GPU |
| 가격 모델 | 마크업 기반 | 셀프 호스팅(비용 무료) + API 비용 |
| GPU 제어 | 없음 | 직접 GPU 관리 |
| 커스터마이징 | 제한적 | TOML 설정 + 플러그인 |
| 에이전트 | 없음 | AgentaaS 계층 |

#### 핵심 차별화 요약

```
Gadgetron의 독보적 위치:

  +-------------------------------------------+
  |         Gadgetron만의 영역                 |
  |                                           |
  |   GPU 스케줄링 x LLM 오케스트레이션      |
  |   x 로컬 추론 x 단일 바이너리             |
  |   x Rust 성능                             |
  +-------------------------------------------+

  LiteLLM   --- LLM 라우팅만 (Python)
  Portkey   --- LLM 게이트웨이만 (SaaS)
  OpenRouter --- 클라우드 API 프록시만
  vLLM/SGLang --- 단일 모델 서빙만
```

---

## 2. 아키텍처 다이어그램

### 2.1 크레이트 의존성 및 데이터 흐름

```
                              +------------------+
                              |  gadgetron-cli   |
                              |  (bin: gadgetron)|
                              +--------+---------+
                                       |
          +------------+-------+-------+-------+---------+-----------+
          |            |       |               |         |           |
          v            v       v               v         v           v
  +------------+ +----------+ +----------+ +----------+ +--------+ +-----+
  |    core    | | provider | |  router  | | gateway  | |scheduler| | tui |
  +------------+ +----+-----+ +-----+----+ +-----+----+ +---+----+ +--+--+
       ^              |             |            |          |          |
       |              |             |            |          |          |
       |              v             v            |          v          v
       +----------<- core <----- provider        |     +-------+  +---+
                   ^   ^         ^    ^          |     | node  |  |core|
                   |   |         |    |          |     +---+---+  |router|
                   |   |         |    |          |         ^      |sched|
                   |   |         |    |          +---------+      |node |
                   |   +---------+    +----------+-+------+       +-----+
                   |                               |
                   +-------------------------------+

  실제 의존성 (Cargo.toml 기준):
    core        <- (외부 crate만)
    provider    <- core
    router      <- core, provider
    gateway     <- core, provider, router, scheduler, node
    scheduler   <- core, node
    node        <- core
    tui         <- core, router, scheduler, node
    cli         <- core, provider, router, gateway, scheduler, node, tui
```

### 2.2 런타임 컴포넌트 다이어그램

```
+=========================================================================+
|                        Gadgetron -- 단일 바이너리                        |
|                                                                         |
|  +-------------------------------------------------------------------+  |
|  |                    gadgetron-gateway (HTTP)                        |  |
|  |  +-------------+  +------------+  +-----------+  +-------------+  |  |
|  |  |/v1/chat     |  |/v1/models  |  |/api/v1/   |  |  SSE Stream  |  |  |
|  |  | completions |  |             |  |  nodes    |  |  (zero-copy) |  |  |
|  |  +-------------+  +------------+  |  deploy    |  +-------------+  |  |
|  |                                   |  status    |                   |  |
|  |  +-------------+  +------------+  +-----------+                   |  |
|  |  |  /health    |  |/api/v1/    |  +-----------+                   |  |
|  |  |             |  |  usage     |  |/api/v1/   |                   |  |
|  |  +-------------+  +------------+  |  costs     |                   |  |
|  |                                   +-----------+                   |  |
|  |  GatewayServer::new(addr).run()                                   |  |
|  |  Routes: axum::Router + CorsLayer + TraceLayer                    |  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                       gadgetron-router                             |  |
|  |  +--------------+ +--------------+ +-------------+ +------------+  |  |
|  |  | RoundRobin   | | CostOptimal  | | LatencyOpt  | | QualityOpt |  |  |
|  |  +--------------+ +--------------+ +-------------+ +------------+  |  |
|  |  +--------------+ +----------------------------------------------+ |  |
|  |  | Fallback     | | Weighted { weights: HashMap<String, f32> }  | |  |
|  |  +--------------+ +----------------------------------------------+ |  |
|  |                                                                   |  |
|  |  Router::resolve(&ChatRequest) -> RoutingDecision                 |  |
|  |  Router::chat(ChatRequest) -> ChatResponse (with fallback)        |  |
|  |  Router::chat_stream(ChatRequest) -> Pin<Box<Stream<ChatChunk>>>  |  |
|  |                                                                   |  |
|  |  +---------------------------------------------------------------+|  |
|  |  |           MetricsStore (DashMap<(String,String), ProviderMetrics>)|  |
|  |  |  record_success() / record_error() / get() / all_metrics()     ||  |
|  |  +---------------------------------------------------------------+|  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                     gadgetron-provider                             |  |
|  |  +----------+ +-----------+ +--------+ +--------+ +-------------+  |  |
|  |  | OpenAI   | | Anthropic | | Gemini | | Ollama | | vLLM/SGLang |  |  |
|  |  | Provider | | Provider  | | (TBD)  | |Provider| |  Providers  |  |  |
|  |  +----------+ +-----------+ +--------+ +--------+ +-------------+  |  |
|  |                                                                   |  |
|  |  LlmProvider trait:                                               |  |
|  |    async fn chat(&self, ChatRequest) -> Result<ChatResponse>      |  |
|  |    fn chat_stream(&self, ChatRequest) -> Pin<Box<dyn Stream>>     |  |
|  |    async fn models(&self) -> Result<Vec<ModelInfo>>               |  |
|  |    fn name(&self) -> &str                                         |  |
|  |    async fn health(&self) -> Result<()>                           |  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                     gadgetron-scheduler                            |  |
|  |  +------------------+  +--------------------+  +----------------+  |  |
|  |  | VRAM 기반        |  | 노드 등록/갱신     |  | 모델 배포/해제  |  |  |
|  |  | 스케줄링         |  | register_node()    |  | deploy()       |  |  |
|  |  |                  |  | update_node()      |  | undeploy()     |  |  |
|  |  +------------------+  +--------------------+  +----------------+  |  |
|  |  +------------------+                                              |  |
|  |  | LRU Eviction     |  Scheduler:                                      |  |
|  |  | find_eviction_   |    deployments: RwLock<HashMap<String, ModelDeployment>>
|  |  |   candidate()    |    nodes: RwLock<HashMap<String, NodeStatus>>     |  |
|  |  +------------------+                                              |  |
|  +----------------------------+--------------------------------------+  |
|                               |                                         |
|  +----------------------------v--------------------------------------+  |
|  |                       gadgetron-node                               |  |
|  |  +----------------------+  +------------------------------------+  |  |
|  |  | NodeAgent            |  | ResourceMonitor                    |  |  |
|  |  |  start_model()       |  |   collect() -> NodeResources      |  |  |
|  |  |  stop_model()        |  |   +------+ +-----+ +------+ +---+ |  |  |
|  |  |  status()            |  |   | CPU  | | RAM | | GPU  | |온도||  |  |
|  |  |  collect_metrics()   |  |   +------+ +-----+ +------+ +---+ |  |  |
|  |  +----------------------+  |   sysinfo    NVML (feature gate)  |  |  |
|  +------------------------------------------------------------------+  |
|                                                                         |
|  +------------------------------------------------------------------+  |
|  |                        gadgetron-core                             |  |
|  |  config | error | message | model | node | provider | routing    |  |
|  +------------------------------------------------------------------+  |
|                                                                         |
|  +---------------------------+  +----------------------------------+  |
|  |     gadgetron-tui         |  |       gadgetron-cli              |  |
|  |  App::run()               |  |  main.rs -- 진입점               |  |
|  |  ratatui 대시보드         |  |  AppConfig::load()               |  |
|  |  Nodes | Models | Requests|  |  전체 크레이트 통합 부팅         |  |
|  +---------------------------+  +----------------------------------+  |
|                                                                         |
+=========================================================================+

           ^ 외부 API               ^ K8s/Slurm             ^ 모니터링
    +------+-------+        +-------+--------+     +-------+---------+
    |    Client    |        | 컨테이너 오케스트레이터|   |  Prometheus/   |
    |    (SDK)     |        | K8s/Slurm/Docker |   |  Grafana       |
    +-------------+        +------------------+   +----------------+
```

---

## 3. 크레이트별 책임과 공개 API 요약

### 3.1 gadgetron-core

**역할**: 전체 워크스페이스의 공통 타입, 트레이트, 에러, 설정 정의. 다른 모든 크레이트의 기반.

**모듈 구조** (`crates/gadgetron-core/src/`):

| 모듈 | 공개 타입 | 설명 |
|------|----------|------|
| `config` | `AppConfig`, `ServerConfig`, `ProviderConfig`, `LocalModelConfig` | TOML 설정 파싱, `${ENV}` 변수 치환 |
| `error` | `GadgetronError`, `Result<T>` | 11개 변형의 통합 에러 열거형 |
| `message` | `Message`, `Role`, `Content`, `ContentPart`, `ImageUrl` | 채팅 메시지 모델 (멀티모달 + 툴 지원) |
| `model` | `ModelState`, `InferenceEngine`, `ModelDeployment`, `Quantization`, `estimate_vram_mb()` | 모델 수명주기, VRAM 추정 |
| `node` | `NodeResources`, `GpuInfo`, `NodeConfig`, `NodeStatus` | 노드 하드웨어 메트릭 |
| `provider` | `ChatRequest`, `ChatResponse`, `ChatChunk`, `Choice`, `ChunkChoice`, `ChunkDelta`, `Usage`, `ModelInfo`, `Tool`, `ToolFunction`, `ToolCallChunk`, `FunctionChunk`, `LlmProvider` | LLM 프로바이더 트레이트 및 요청/응답 타입 |
| `routing` | `RoutingStrategy`, `RoutingConfig`, `CostEntry`, `RoutingDecision`, `ProviderMetrics` | 라우팅 전략 및 메트릭 데이터 |

**핵심 트레이트**:

```rust
// provider.rs
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;
    fn chat_stream(&self, req: ChatRequest)
        -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>;
    async fn models(&self) -> Result<Vec<ModelInfo>>;
    fn name(&self) -> &str;
    async fn health(&self) -> Result<()>;
}
```

**핵심 에러 타입**:

```rust
// error.rs
pub enum GadgetronError {
    Provider(String),
    Router(String),
    Scheduler(String),
    Node(String),
    Config(String),
    NoProvider(String),
    AllProvidersFailed(String),
    InsufficientResources(String),
    ModelNotFound(String),
    NodeNotFound(String),
    HealthCheckFailed { provider: String, reason: String },
    Timeout(u64),
}
```

**설정 로딩**:

```rust
// config.rs
impl AppConfig {
    pub fn load(path: &str) -> Result<Self>;  // TOML 파일 로드 + ${ENV} 치환
}
```

### 3.2 gadgetron-provider

**역할**: 6종 LLM 프로바이더 어댑터 구현. `LlmProvider` 트레이트 기반 다형성.

| 구조체 | 프로바이더 | 인증 방식 | 스트리밍 프로토콜 |
|--------|-----------|-----------|-----------------|
| `OpenAiProvider` | OpenAI | `Bearer` 토큰 | SSE (`data: [DONE]`) |
| `AnthropicProvider` | Anthropic | `x-api-key` + `anthropic-version` | SSE (`content_block_delta`, `message_stop`) |
| `OllamaProvider` | Ollama | 없음 | OpenAI 호환 SSE |
| `VllmProvider` | vLLM | `Bearer` (선택) | OpenAI 호환 SSE |
| `SglangProvider` | SGLang | `Bearer` (선택) | OpenAI 호환 SSE |

**공개 API**:

```rust
// 각 프로바이더
impl OpenAiProvider {
    pub fn new(api_key: String, base_url: Option<String>) -> Self;
    pub fn with_models(self, models: Vec<String>) -> Self;
}

impl AnthropicProvider {
    pub fn new(api_key: String, base_url: Option<String>) -> Self;
    pub fn with_models(self, models: Vec<String>) -> Self;
}

impl OllamaProvider {
    pub fn new(endpoint: String) -> Self;
    pub async fn pull_model(&self, model: &str) -> Result<()>;   // Ollama 전용
    pub async fn running_models(&self) -> Result<Vec<String>>;    // Ollama 전용
    pub async fn unload_model(&self, model: &str) -> Result<()>;  // Ollama 전용
}

impl VllmProvider {
    pub fn new(endpoint: String, api_key: Option<String>) -> Self;
}

impl SglangProvider {
    pub fn new(endpoint: String, api_key: Option<String>) -> Self;
}
```

**Anthropic 프로토콜 변환**: `to_anthropic_request()`가 `ChatRequest`를 Anthropic Messages API 포맷으로 변환하고, `from_anthropic_response()`가 응답을 `ChatResponse`로 역변환합니다. `Role::System`은 최상위 `system` 필드로 분리, `ContentPart::ToolResult`은 `role: user` 메시지로 매핑.

### 3.3 gadgetron-router

**역할**: 6종 라우팅 전략 구현 및 lock-free 메트릭 수집.

**공개 타입**:

```rust
pub struct Router { ... }
pub struct MetricsStore { ... }
```

**Router API**:

```rust
impl Router {
    pub fn new(
        providers: HashMap<String, Arc<dyn LlmProvider>>,
        config: RoutingConfig,
        metrics: Arc<MetricsStore>,
    ) -> Self;

    pub fn resolve(&self, req: &ChatRequest) -> Result<RoutingDecision>;
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;
    pub fn chat_stream(&self, req: ChatRequest)
        -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>;
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    pub async fn health_check_all(&self) -> HashMap<String, bool>;
}
```

**라우팅 전략** (`RoutingStrategy` 변형):

| 전략 | 선택 로직 | 구현 메서드 |
|------|----------|------------|
| `RoundRobin` | `AtomicUsize` 카운터 기반 순환 | `resolve()` 내 직접 구현 |
| `CostOptimal` | `CostEntry`(input/output per 1M tokens) 기반 최저가 | `cheapest_provider()` |
| `LatencyOptimal` | `ProviderMetrics::avg_latency_ms()` 기반 최저지연 | `fastest_provider()` |
| `QualityOptimal` | 하드코딩 우선순위: anthropic > openai > ollama | `prefer_provider()` |
| `Fallback { chain }` | 명시적 체인 순서 검색 | `resolve()` 내 직접 구현 |
| `Weighted { weights }` | 가중치 랜덤 선택 (`rand::RngExt`) | `weighted_provider()` |

**폴백 체인**: `chat()` 호출 시 1차 프로바이더 실패하면 `try_fallbacks()`가 `RoutingDecision::fallback_chain`을 순차 시도합니다. 모든 폴백 실패 시 `GadgetronError::AllProvidersFailed` 반환.

**MetricsStore API**:

```rust
impl MetricsStore {
    pub fn new() -> Self;
    pub fn record_success(&self, provider: &str, model: &str,
        latency_ms: u64, input_tokens: u32, output_tokens: u32, cost_usd: f64);
    pub fn record_error(&self, provider: &str, model: &str,
        latency_ms: u64, error: String);
    pub fn get(&self, key: &(String, String)) -> ProviderMetrics;
    pub fn all_metrics(&self) -> HashMap<(String, String), ProviderMetrics>;
}
```

내부 저장소: `DashMap<(String, String), ProviderMetrics>` -- lock-free 동시 읽기/쓰기.

### 3.4 gadgetron-gateway

**역할**: axum 기반 OpenAI 호환 HTTP API + 관리 API 서버.

**공개 타입**:

```rust
pub struct GatewayServer { ... }
```

**GatewayServer API**:

```rust
impl GatewayServer {
    pub fn new(addr: SocketAddr) -> Self;
    pub async fn run(self) -> anyhow::Result<()>;
}
```

**라우트 맵**:

| 메서드 | 경로 | 핸들러 | 설명 |
|--------|------|--------|------|
| POST | `/v1/chat/completions` | `routes::chat_completions` | 채팅 완성 (스트리밍 지원) |
| GET | `/v1/models` | `routes::list_models` | 사용 가능 모델 목록 |
| GET | `/health` | `routes::health` | 헬스체크 |
| GET | `/api/v1/nodes` | `routes::list_nodes` | 노드 목록 |
| GET | `/api/v1/nodes/:id/metrics` | `routes::node_metrics` | 노드 리소스 메트릭 |
| POST | `/api/v1/models/deploy` | `routes::deploy_model` | 모델 배포 |
| DELETE | `/api/v1/models/:id` | `routes::undeploy_model` | 모델 해제 |
| GET | `/api/v1/models/status` | `routes::model_status` | 모델 배포 상태 |
| GET | `/api/v1/usage` | `routes::usage` | 토큰 사용량 |
| GET | `/api/v1/costs` | `routes::costs` | 비용 분석 |

**미들웨어 스택**: `CorsLayer::permissive()` + `TraceLayer::new_for_http()`

**SSE 스트리밍** (`sse.rs`):

```rust
pub fn chat_chunk_to_sse(
    stream: impl Stream<Item = Result<ChatChunk>> + Send + 'static,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send>;
```

`ChatChunk` -> JSON -> `Event` -> `Sse<KeepAlive>` (15초 간격) 변환 파이프라인.

### 3.5 gadgetron-scheduler

**역할**: VRAM 기반 모델 배포 스케줄링, LRU eviction, 노드 관리.

**공개 타입**:

```rust
pub struct Scheduler { ... }
```

**Scheduler API**:

```rust
impl Scheduler {
    pub fn new() -> Self;

    pub async fn deploy(&self, model_id: &str, engine: InferenceEngine, vram_mb: u64)
        -> Result<()>;
    pub async fn undeploy(&self, model_id: &str) -> Result<()>;
    pub async fn get_status(&self, model_id: &str) -> Option<ModelState>;
    pub async fn list_deployments(&self) -> Vec<ModelDeployment>;
    pub async fn find_eviction_candidate(&self, node_id: &str, required_mb: u64)
        -> Option<String>;
    pub async fn register_node(&self, status: NodeStatus);
    pub async fn update_node(&self, node_id: &str, status: NodeStatus);
    pub async fn list_nodes(&self) -> Vec<NodeStatus>;
}
```

**배포 로직**:
1. `deploy()` 호출 시 이미 `ModelState::Running`이면 즉시 반환 (멱등성)
2. `nodes`에서 `healthy == true`이고 `available_vram_mb() >= vram_mb`인 첫 번째 노드 선택
3. 조건 충족 노드 없으면 `InsufficientResources` 에러
4. 선택된 노드에 `ModelDeployment { status: Loading }` 생성

**LRU Eviction**:
1. `find_eviction_candidate()`가 지정 노드의 실행 중 모델을 `last_used` 기준 정렬
2. 가장 오래 미사용 모델부터 누적 VRAM 확보량 계산
3. `required_mb` 이상 확보 가능한 시점의 모델 ID 반환

**내부 상태**: `deployments: Arc<RwLock<HashMap<String, ModelDeployment>>>`, `nodes: Arc<RwLock<HashMap<String, NodeStatus>>>`

### 3.6 gadgetron-node

**역할**: 노드 에이전트 (프로세스 관리) 및 하드웨어 모니터링 (CPU/RAM/GPU).

**공개 타입**:

```rust
pub struct NodeAgent { ... }
pub struct ResourceMonitor { ... }
```

**NodeAgent API**:

```rust
impl NodeAgent {
    pub fn new(config: NodeConfig) -> Self;
    pub fn id(&self) -> &str;
    pub fn endpoint(&self) -> &str;
    pub fn collect_metrics(&mut self) -> NodeResources;
    pub fn status(&mut self) -> NodeStatus;
    pub async fn start_model(&mut self, deployment: &ModelDeployment) -> Result<u16>;
    pub async fn stop_model(&mut self, model_id: &str) -> Result<()>;
}
```

**엔진별 모델 시작**:

| InferenceEngine | 시작 방식 | 기본 포트 |
|----------------|----------|----------|
| `Ollama` | HTTP POST `/api/generate` (keep_alive: -1) | 11434 |
| `Vllm` | `tokio::process::Command` -- `vllm serve` | 8000 |
| `Sglang` | `tokio::process::Command` -- `python3 -m sglang.launch_server` | 30000 |
| `LlamaCpp` | `tokio::process::Command` -- `llama-server` | 8080 |
| `Tgi` | `tokio::process::Command` -- `text-generation-launcher` | 3000 |

**ResourceMonitor API**:

```rust
impl ResourceMonitor {
    pub fn new() -> Self;
    pub fn collect(&mut self) -> NodeResources;
}
```

`collect()` 반환값:
- `cpu_usage_pct`: `sysinfo::System::global_cpu_usage()`
- `memory_total_bytes` / `memory_used_bytes`: `sysinfo::System::total_memory()` / `used_memory()`
- `gpus`: NVML feature gate 활성 시 `nvml_wrapper::Nvml`에서 수집 (`GpuInfo` 배열), 비활성 시 빈 벡터

Feature gate: `nvml` (기본 비활성) -- `nvml-wrapper = "0.10"` 선택적 의존성.

### 3.7 gadgetron-tui

**역할**: ratatui 기반 터미널 대시보드.

**공개 타입**:

```rust
pub struct App { ... }
```

**App API**:

```rust
impl App {
    pub fn new() -> Self;
    pub async fn run(&mut self) -> anyhow::Result<()>;
}
```

**UI 레이아웃** (3x2 그리드):

```
+--------------------------------------------------+
|           Gadgetron Orchestrator (Header)         |
+------------------------+-------------------------+
|                        |      Models             |
|       Nodes            |-------------------------+
|                        |    Recent Requests       |
+------------------------+-------------------------+
|  q: quit | Up/Down: navigate | r: refresh        |
+--------------------------------------------------+
```

100ms 폴링 주기로 이벤트 대기, `q`/`Esc`로 종료. `crossterm` 백엔드 사용.

### 3.8 gadgetron-cli

**역할**: 단일 바이너리 진입점. 전체 크레이트 통합 부팅.

**바이너리**: `gadgetron` (설정: `[[bin]] name = "gadgetron"`)

**부팅 시퀀스** (`main.rs`):

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // 1. tracing 초기화 (env filter + gadgetron=info)
    // 2. CLI 인자에서 설정 파일 경로 읽기 (기본값: config/nexus.toml)
    // 3. AppConfig::load() 로 TOML 파싱
    // 4. TODO: 프로바이더 초기화
    // 5. TODO: Router + MetricsStore 생성
    // 6. TODO: Scheduler + NodeAgent 초기화
    // 7. TODO: GatewayServer::new(addr).run()
    // 8. TODO: TUI 실행 (플래그 기반)
}
```

---

## 4. 데이터 흐름: ChatRequest -> Router -> Provider -> Response

### 4.1 비스트리밍 요청 흐름

```
  Client
    |
    |  POST /v1/chat/completions
    |  { model: "gpt-4", messages: [...], temperature: 0.7 }
    v
+----------------------------------------------+
|  gadgetron-gateway                           |
|  GatewayServer.run()                         |
|  1. axum Router가 요청 수신                   |
|  2. ChatRequest 역직렬화 (serde_json)        |
|  3. API Key 검증 (ServerConfig.api_key)      |
|  4. 타임아웃 설정 (request_timeout_ms)        |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-router                            |
|  Router::resolve(&ChatRequest)               |
|  4. RoutingConfig.default_strategy 확인      |
|  5. 프로바이더 목록에서 전략 적용             |
|     - RoundRobin: AtomicUsize 순환            |
|     - CostOptimal: cheapest_provider()        |
|     - LatencyOptimal: fastest_provider()      |
|     - QualityOptimal: prefer_provider()       |
|     - Fallback: chain 순차 검색              |
|     - Weighted: weighted_provider()           |
|  6. RoutingDecision 생성                      |
|     { provider, model, strategy,             |
|       estimated_cost_usd, fallback_chain }   |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-provider                          |
|  LlmProvider::chat(ChatRequest)              |
|  7. 선택된 프로바이더 호출                    |
|     - OpenAI: Bearer 토큰, /v1/chat/completions
|     - Anthropic: x-api-key, /v1/messages     |
|     - Ollama: 인증 없음, /v1/chat/completions|
|     - vLLM: Bearer (선택), /v1/chat/completions
|     - SGLang: Bearer (선택), /v1/chat/completions
|  8. HTTP 응답 -> ChatResponse 역직렬화       |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-router (사후 처리)                 |
|  9. 성공 시:                                 |
|     MetricsStore::record_success(             |
|       provider, model, latency_ms,           |
|       prompt_tokens, completion_tokens, cost) |
|  10. 실패 시:                                |
|     MetricsStore::record_error(               |
|       provider, model, latency_ms, error)     |
|     try_fallbacks() -> fallback_chain 순차 시도|
|     모두 실패 -> AllProvidersFailed 에러      |
+----------------------+-----------------------+
                       |
                       v
  Client <- ChatResponse {
    id, object: "chat.completion",
    model, choices: [Choice { message, finish_reason }],
    usage: Usage { prompt_tokens, completion_tokens, total_tokens }
  }
```

### 4.2 스트리밍 요청 흐름

```
  Client
    |
    |  POST /v1/chat/completions  (stream: true)
    v
+----------------------------------------------+
|  gadgetron-gateway                           |
|  SSE 스트리밍 파이프라인:                    |
|                                              |
|  LlmProvider::chat_stream(req)               |
|       -> Pin<Box<dyn Stream<Item=Result<ChatChunk>>>>  |
|       -> chat_chunk_to_sse()                 |
|       -> Sse<KeepAlive<15s>>                 |
|       -> axum response body                  |
|                                              |
|  제로카피 변환:                              |
|  ChatChunk -> serde_json::to_string()        |
|            -> Event::default().data(json)    |
|            -> SSE frame ("data: ...\n\n")    |
|                                              |
|  프로바이더별 SSE 처리:                      |
|  - OpenAI/vLLM/SGLang/Ollama:                |
|    "data: {json}\n\n" ... "data: [DONE]\n\n" |
|  - Anthropic:                                |
|    "data: {type:content_block_delta}\n\n"    |
|    "data: {type:message_stop}\n\n"           |
|    -> ChatChunk으로 정규화 후 재변환         |
+----------------------------------------------+
```

### 4.3 로컬 모델 배포 흐름

```
  POST /api/v1/models/deploy
    { model_id, engine, vram_requirement_mb }
    |
    v
+----------------------------------------------+
|  gadgetron-scheduler                         |
|  Scheduler::deploy(model_id, engine, vram_mb)|
|                                              |
|  1. 기존 배포 확인 (멱등성)                  |
|     if ModelState::Running -> return Ok(())  |
|                                              |
|  2. 가용 노드 탐색                           |
|     nodes.values()                           |
|       .filter(|n| n.healthy)                 |
|       .find(|n| n.resources.available_vram_mb() >= vram_mb)
|                                              |
|  3. 미충족 시 find_eviction_candidate()      |
|     -> LRU 기반 모델 해제 후 재시도          |
|                                              |
|  4. ModelDeployment 생성                     |
|     { status: Loading, assigned_node, ... }  |
+----------------------+-----------------------+
                       |
                       v
+----------------------------------------------+
|  gadgetron-node                              |
|  NodeAgent::start_model(&deployment)         |
|                                              |
|  5. InferenceEngine별 프로세스 시작           |
|     - Ollama: POST /api/generate (keep_alive)|
|     - Vllm:  tokio::process::Command         |
|     - SGLang: tokio::process::Command        |
|     - LlamaCpp: tokio::process::Command      |
|     - TGI: tokio::process::Command           |
|                                              |
|  6. 포트 할당 및 running_models 갱신         |
|  7. ModelState::Running { port } 전환        |
+----------------------------------------------+
```

### 4.4 VRAM 추정 공식

```rust
// model.rs
pub fn estimate_vram_mb(params_billion: f64, quantization: Quantization) -> u64 {
    let gb_per_billion = match quantization {
        Quantization::Fp16    => 2.0,
        Quantization::Fp8     => 1.0,
        Quantization::Q8_0    => 1.1,
        Quantization::Q5_K_M  => 0.7,
        Quantization::Q4_K_M  => 0.6,
        Quantization::Q3_K_M  => 0.45,
        Quantization::GgufAuto => 0.6,
    };
    ((params_billion * gb_per_billion * 1024.0) + 1024.0) as u64
}
```

예시: Llama-3-70B Q4_K_M -> `70 * 0.6 * 1024 + 1024` = 44,032 MB

---

## 5. Phase 1 (MVP), Phase 2 (상용화), Phase 3 (플랫폼화) 로드맵

### Phase 1 -- MVP (v0.1, 현재 구현 상태)

**목표**: 단일 노드에서 클라우드+로컬 LLM 프록시 작동

| 항목 | 상태 | 크레이트 | 상세 |
|------|------|---------|------|
| 기본 라우팅 (6종) | 구현됨 | `gadgetron-router` | RoundRobin, CostOptimal, LatencyOptimal, QualityOptimal, Fallback, Weighted |
| 클라우드 프로바이더 | 구현됨 | `gadgetron-provider` | `OpenAiProvider`, `AnthropicProvider` |
| 로컬 프로바이더 | 구현됨 | `gadgetron-provider` | `OllamaProvider`, `VllmProvider`, `SglangProvider` |
| 스트리밍 SSE | 구현됨 | `gadgetron-gateway` | `chat_chunk_to_sse()` + `KeepAlive(15s)` |
| TOML 설정 | 구현됨 | `gadgetron-core` | `AppConfig::load()` + `${ENV}` 치환 |
| GPU 모니터링 | 구현됨 | `gadgetron-node` | `ResourceMonitor` + NVML feature gate |
| TUI 대시보드 | 구현됨 | `gadgetron-tui` | ratatui 기본 레이아웃 (Nodes, Models, Requests) |
| 모델 배포/해제 | 구현됨 | `gadgetron-scheduler` | `Scheduler::deploy/undeploy` + LRU eviction |
| lock-free 메트릭 | 구현됨 | `gadgetron-router` | `MetricsStore` (DashMap) |
| 헬스체크 | 구현됨 | `gadgetron-provider` | `LlmProvider::health()` |
| VRAM 추정 | 구현됨 | `gadgetron-core` | `estimate_vram_mb()` + `Quantization` 열거형 |
| 엔진별 프로세스 관리 | 구현됨 | `gadgetron-node` | `NodeAgent::start_model/stop_model` |
| API 라우트 정의 | 구현됨 | `gadgetron-gateway` | 10개 엔드포인트 라우트 등록 |
| 핸들러 구현 | 스텁 | `gadgetron-gateway` | TODO: Router/Scheduler 통합 필요 |
| Gemini 프로바이더 | 미구현 | `gadgetron-provider` | TODO |
| CLI 통합 부팅 | 부분 | `gadgetron-cli` | 설정 로드만 구현, 컴포넌트 와이어링 TODO |

### Phase 2 -- Commercial (v0.5)

**목표**: 프로덕션급 다중 노드 클러스터

| 항목 | 설명 | 관련 크레이트 |
|------|------|-------------|
| NUMA 인식 스케줄링 | GPU-NUMA 노드 선호도 기반 배치 결정 | `gadgetron-scheduler` |
| MIG(Time-slicing) | NVIDIA MIG 파티셔닝, GPU 타임슬라이싱 | `gadgetron-node` |
| 열/전력 스로틀링 | `GpuInfo::temperature_c`, `power_draw_w` 기반 요청 제한 | `gadgetron-scheduler` + `gadgetron-node` |
| 인터커넥트 인식 | NVLink/NVSwitch 토폴로지 고려 텐서 병렬 배치 | `gadgetron-scheduler` |
| 핫 리로드 | TOML 설정 변경 시 무중단 반영 | `gadgetron-core` |
| K8s 통합 | CRD 기반 모델 배포, HPA/VPA 오토스케일링 | 신규 `gadgetron-k8s` |
| Slurm 통합 | HPC 클러스터 잡 서브미션 | 신규 `gadgetron-slurm` |
| 인증/인가 | API Key, RBAC, 테넌트 격리 | `gadgetron-gateway` |
| 영구 메트릭 | Prometheus 익스포터, 시계열 DB 저장 | `gadgetron-router` |
| Web UI | GPU/모델/요청 실시간 대시보드 | 신규 `gadgetron-web` |
| 동적 포트 할당 | `NodeAgent::start_model`의 하드코딩 포트 해소 | `gadgetron-node` |
| 게이트웨이 핸들러 완성 | `routes::*` 핸들러에 Router/Scheduler 통합 | `gadgetron-gateway` |
| 설정 검증 | 시작 시 프로바이더 연결, 노드 헬스 사전 검증 | `gadgetron-cli` |

### Phase 3 -- Platform (v1.0)

**목표**: XaaS 풀 스택 플랫폼

| 항목 | 설명 |
|------|------|
| GPUaaS | GPU 단위 할당, MIG 파티션, 타임슬라이스 관리 |
| ModelaaS | 모델 자동 배포, 오토스케일링, A/B 테스트 |
| AgentaaS | 멀티 에이전트 오케스트레이션, 툴 사용, 메모리 |
| 벤더마이그레이션 | 클라우드 <-> 로컬 자동 페일오버 |
| 비용 최적화 | 실시간 비용 추적, 예산 한도, 자동 프로바이더 전환 |
| 멀티 테넌시 | 조직 격리, 리소스 쿼터, 사용량 청구 |
| 플러그인 시스템 | 커스텀 프로바이더, 라우팅 전략, 미들웨어 |
| 글로벌 분산 | 리전 간 라우팅, 지연 시간 최적화 |
| Profiling 기반 오토튜닝 | vLLM/SGLang 파라미터 자동 최적화 |
| HuggingFace Hub 통합 | 모델 카탈로그/다운로드 |
| 컨테이너 런타임 | Docker/Podman/containerd 모델 서빙 |
| OpenTelemetry | 분산 트레이싱 |

---

## 6. 기술 스택과 의존성 맵

### 6.1 워크스페이스 의존성

| 계층 | 크레이트 | 버전 | 사용 크레이트 |
|------|---------|------|-------------|
| 비동기 런타임 | `tokio` | 1 (full) | core, provider, router, gateway, scheduler, node, tui, cli |
| 웹 프레임워크 | `axum` | 0.8 (macros) | gateway |
| 미들웨어 | `tower` | 0.5 | gateway |
| | `tower-http` | 0.6 (cors, trace) | gateway |
| HTTP 클라이언트 | `reqwest` | 0.12 (stream, json, rustls-tls) | provider, node |
| 직렬화 | `serde` | 1 (derive) | 전 크레이트 |
| | `serde_json` | 1 | 전 크레이트 |
| | `toml` | 0.8 | core, cli |
| 스트리밍 | `futures` | 0.3 | core, provider, router, gateway |
| | `tokio-stream` | 0.1 | provider, gateway, tui |
| | `eventsource-stream` | 0.2 | provider |
| | `async-stream` | 0.3.6 | provider |
| 에러 | `thiserror` | 2 | core, provider, router, gateway, scheduler, node |
| | `anyhow` | 1 | gateway, tui, cli |
| 로깅 | `tracing` | 0.1 | 전 크레이트 |
| | `tracing-subscriber` | 0.3 (env-filter, json) | gateway, cli |
| 시스템 모니터링 | `sysinfo` | 0.33 | node |
| GPU 모니터링 | `nvml-wrapper` | 0.10 (optional) | node |
| 동시 컬렉션 | `dashmap` | 6 | router, scheduler |
| TUI | `ratatui` | 0.29 | tui |
| | `crossterm` | 0.28 (event-stream) | tui |
| 유틸리티 | `uuid` | 1 (v4) | core, provider, gateway |
| | `chrono` | 0.4 (serde) | core, provider, router, gateway, scheduler, node, tui |
| | `async-trait` | 0.1 | core, provider, router, scheduler, node |
| 난수 | `rand` | 0.10.0 | router |

### 6.2 의존성 행렬 (실제 Cargo.toml 기준)

| 크레이트 | core | provider | router | gateway | scheduler | node | tui | cli |
|----------|:----:|:--------:|:------:|:-------:|:---------:|:----:|:---:|:---:|
| **core** | -- | | | | | | | |
| **provider** | O | -- | | | | | | |
| **router** | O | O | -- | | | | | |
| **gateway** | O | O | O | -- | O | O | | |
| **scheduler** | O | | | | -- | O | | |
| **node** | O | | | | | -- | | |
| **tui** | O | | O | | O | O | -- | |
| **cli** | O | O | O | O | O | O | O | -- |

> O = 직접 의존. 간접 의존은 표기하지 않음.

### 6.3 Feature Gates

| 크레이트 | Feature | 의존성 | 설명 |
|---------|---------|--------|------|
| `gadgetron-node` | `nvml` | `nvml-wrapper = "0.10"` | NVIDIA GPU 메트릭 수집 (GPU 이름, VRAM, 사용률, 온도, 전력). 비활성 시 `gpus: Vec::new()` |

---

## 7. 핵심 설계 원칙

### 7.1 Sub-ms P99 오버헤드

Rust 네이티브 + GC 없음 + 제로카피 스트리밍으로 게이트웨이 자체 오버헤드를 서브밀리초 수준으로 유지합니다.

- **GC-free**: Rust의 소유권 기반 메모리 관리로 GC pause 원천 차단
- **제로카피 스트리밍**: `LlmProvider::chat_stream()`이 `Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>`를 반환하고, `chat_chunk_to_sse()`가 이를 SSE `Event`로 직접 변환. 중간 버퍼링 없이 axum response body로 파이프라인
- **lock-free 메트릭**: `MetricsStore`가 `DashMap` 기반으로 읽기/쓰기 시 뮤텍스 없이 동시 접근
- **비동기 I/O**: `tokio` 런타임 기반으로 파일/네트워크 I/O가 논블로킹

### 7.2 GPU-first 스케줄링

모델 배치 결정에 GPU 리소스를 1순위로 고려합니다.

- **VRAM 인식**: `NodeResources::available_vram_mb()`로 가용 VRAM 계산, `estimate_vram_mb()`로 모델 요구량 추정
- **LRU Eviction**: `Scheduler::find_eviction_candidate()`가 `ModelDeployment::last_used` 기준으로 최소 사용 모델 우선 해제
- **NVML 메트릭**: `GpuInfo` 구조체가 `temperature_c`, `power_draw_w`, `utilization_pct` 제공 (Phase 2 스로틀링 기반)
- **NUMA/인터커넥트**: Phase 2에서 NVLink 토폴로지, NUMA 선호도 기반 배치 예정

### 7.3 Config-driven 단일 바이너리

```toml
[server]
bind = "0.0.0.0:8080"
api_key = "${OPENAI_API_KEY}"
request_timeout_ms = 30000

[router]
default_strategy = "cost_optimal"

[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4", "gpt-4o"]

[providers.anthropic]
type = "anthropic"
api_key = "${ANTHROPIC_API_KEY}"
models = ["claude-sonnet-4-20250514"]

[providers.ollama]
type = "ollama"
endpoint = "http://localhost:11434"

[providers.vllm-local]
type = "vllm"
endpoint = "http://localhost:8000"

[[nodes]]
id = "node-01"
endpoint = "http://localhost:11434"
gpu_count = 2

[[models]]
id = "meta-llama/Meta-Llama-3-70B"
engine = "vllm"
vram_requirement_mb = 42000
priority = 10
args = ["--tensor-parallel-size", "2"]
```

- `AppConfig::load()`가 TOML 파싱 + `${ENV}` 환경변수 치환
- 단일 `gadgetron` 바이너리로 전체 스택 구동
- 마이크로서비스 분할 없이 `gadgetron-cli`가 모든 크레이트를 정적 링크

### 7.4 LlmProvider 트레이트 기반 다형성

모든 프로바이더가 동일한 트레이트를 구현하여 `Router`가 프로바이더 타입을 알 필요 없이 라우팅합니다.

```rust
// 프로바이더 등록 패턴
let providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::from([
    ("openai".into(), Arc::new(OpenAiProvider::new(key, None))),
    ("anthropic".into(), Arc::new(AnthropicProvider::new(key, None).with_models(models))),
    ("ollama".into(), Arc::new(OllamaProvider::new(endpoint))),
]);

let router = Router::new(providers, config, metrics);
```

- 클라우드 프로바이더 (OpenAI, Anthropic): HTTP API + 각자의 인증 헤더
- 로컬 프로바이더 (Ollama, vLLM, SGLang): OpenAI 호환 API 또는 전용 엔드포인트
- Anthropic: `to_anthropic_request()`/`from_anthropic_response()`로 프로토콜 변환 계층 적용

### 7.5 단일 에러 타입

`GadgetronError` 열거형이 모든 크레이트의 에러를 통합합니다. `thiserror` 기반 `Display` 구현으로 에러 체인 추적이 용이합니다.

```rust
pub type Result<T> = std::result::Result<T, GadgetronError>;
```

각 변형은 발생 도메인을 명확히 표시합니다: `Provider`, `Router`, `Scheduler`, `Node`, `Config`, `NoProvider`, `AllProvidersFailed`, `InsufficientResources`, `ModelNotFound`, `NodeNotFound`, `HealthCheckFailed`, `Timeout`.

### 7.6 스트리밍 일관성

모든 프로바이더가 `chat_stream()`을 동일한 시그니처로 구현합니다:

- **OpenAI/vLLM/SGLang/Ollama**: SSE `data: {json}\n\n` ... `data: [DONE]\n\n` 파싱
- **Anthropic**: `content_block_delta` / `message_stop` 이벤트를 `ChatChunk`로 정규화 후 통일된 SSE 출력
- **버퍼링**: 모든 프로바이더가 `\n\n` 기준 버퍼 분할로 불완전 청크 처리
- **에러 전파**: 스트림 내 에러도 `Result<ChatChunk>`로 전달, 강제 종료 없이 graceful degradation

### 7.7 구현-ready 타입 시그니처 요약

| 크레이트 | 타입 | 주요 메서드 |
|---------|------|-----------|
| core | `LlmProvider` | `chat()`, `chat_stream()`, `models()`, `name()`, `health()` |
| core | `AppConfig` | `load(path) -> Result<Self>` |
| core | `NodeResources` | `available_vram_mb()`, `memory_available_bytes()` |
| core | `ModelDeployment` | `is_available()` |
| core | `estimate_vram_mb(params_billion, quantization) -> u64` |
| router | `Router` | `new()`, `resolve()`, `chat()`, `chat_stream()`, `list_models()`, `health_check_all()` |
| router | `MetricsStore` | `new()`, `record_success()`, `record_error()`, `get()`, `all_metrics()` |
| gateway | `GatewayServer` | `new(addr)`, `run()` |
| gateway | `chat_chunk_to_sse()` | `impl Stream<Item=Result<ChatChunk>> -> Sse<...>` |
| scheduler | `Scheduler` | `new()`, `deploy()`, `undeploy()`, `get_status()`, `list_deployments()`, `find_eviction_candidate()`, `register_node()`, `update_node()`, `list_nodes()` |
| node | `NodeAgent` | `new(config)`, `id()`, `endpoint()`, `collect_metrics()`, `status()`, `start_model()`, `stop_model()` |
| node | `ResourceMonitor` | `new()`, `collect() -> NodeResources` |
| tui | `App` | `new()`, `run()` |
