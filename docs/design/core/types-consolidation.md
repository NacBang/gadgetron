# gadgetron-core 타입 통합 설계

> **담당**: @chief-architect
> **상태**: Round 3 (retry) — ✅ Approved
> **작성일**: 2026-04-11
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-core` (primary), `gadgetron-scheduler`, `gadgetron-node`, `gadgetron-gateway`, `gadgetron-router`, `gadgetron-provider`, `gadgetron-xaas`, `gadgetron-tui`, `gadgetron-web`, `gadgetron-testing`
> **Phase**: [P1] (D-20260411-01 B)
> **관련 결정**: D-1, D-2, D-3, D-10, D-12, D-13, D-20260411-01, D-20260411-03, D-20260411-04, D-20260411-05, D-20260411-06, D-20260411-07, D-20260411-08, **D-20260411-11**, **D-20260411-13** (FYI: D-20260411-12 xaas scope)
> **관련 이슈**: Round 1 C-1 ~ C-5, M-3, M-4; Round 2 BLOCKER #1, BLOCKER #8, HIGH #8, HIGH #10; Round 3 D-11 dyn dispatch + D-13 Database variant

---

## 1. 철학 & 컨셉 (Why)

### 1.1 문제

Round 2 플랫폼 리뷰 §1 D-status 표에서 확인된 BLOCKER #1:

> `gadgetron-core` D-1/D-2/D-3/D-10/D-13 타입 12개 미정의 → 다른 크레이트 컴파일 대기

D-1~D-13 레거시 결정과 Round 1 C-1~C-5 충돌 해결안이 모두 "core 확장"을 전제로 하지만, 실제 `crates/gadgetron-core/src/`에는 필수 타입 **12개**가 아직 존재하지 않는다. Round 1 리뷰 (2026-04-11) 이후 PM 재량으로 **D-20260411-06/07/08** 세 개의 신규 결정이 추가되어, streaming interruption variant·공유 UI 타입·f64/i64 과금 경계까지 본 문서 범위에 포함되었다. 본 문서는 Week 1 Track 1 결과물로서, `gadgetron-core`를 Phase 1 MVP의 타입 기반으로 확장·고정하는 단일 진실의 공급원(SSOT)을 정의한다.

### 1.2 제품 비전 연결 (`docs/00-overview.md §1`)

- **미션 1 (레이턴시 제거)** — hot path에서 allocation / lock 없는 타입 설계. `Copy` 가능한 것은 `Copy`, ID는 `Arc<str>`/`&'static str`, 상태 전이는 `#[non_exhaustive]`로 장기 확장 허용.
- **미션 4 (운영 단순성)** — TOML 설정에서 바로 직렬화/역직렬화 가능한 단일 enum 체계. 중복 정의 제거.
- **경쟁 차별화** — `GadgetronError`를 단일 taxonomy로 유지하여, 9단계 미들웨어 체인(D-6, Round 2 §3.1) 어디서 실패해도 동일 에러 채널로 귀결되는 `?` 전파 경로를 확보. D-20260411-08의 `StreamInterrupted` variant 분리로 SSE 재시도·회로 차단·Prometheus 라벨링이 generic `Provider(String)`에 묻히지 않는다.

### 1.3 고려 대안

| 대안 | 장점 | 단점 | 결론 |
|------|------|------|------|
| **A. 각 크레이트에 도메인 타입 분산** | 소유 명확 | Round 1 C-1~C-5 재발 확실, 순환 의존 위험 | 거부 (D-12와 모순) |
| **B. 단일 core + 재-export 허용** | SSOT, 크레이트 경계 선명 | core 크기 증가 | **채택** (D-12 준수) |
| **C. 별도 `gadgetron-types` 신설** | core 슬림 | 크레이트 추가 비용, D-12 재작성 | 거부 |
| **D. 매크로 기반 DSL** | 변형 추가 간편 | rust-analyzer 미완 지원, 읽기 어려움 | 거부 |
| **E. 공유 UI 타입을 `gadgetron-ui-types` 신설** | core 슬림 | Round 2 BLOCKER #8 지연 | 거부 (D-20260411-07 A 채택) |

### 1.4 핵심 설계 원칙

1. **SSOT** — 각 타입은 `gadgetron-core` 내 **정확히 하나의 모듈**에만 정의. 타 크레이트는 import만 한다.
2. **`#[non_exhaustive]`로 forward compatibility** — 장기 확장이 예측되는 모든 public enum/struct에 적용 (§2.8). `WsMessage`도 포함 (D-20260411-07).
3. **Zero-cost where possible** — hot path(scheduler loop, eviction score, port alloc)에서 `String`/`HashMap` 할당 최소화.
4. **Serde는 TOML과 JSON 양쪽 환경에서 동작해야 함** — `#[serde(tag = "type", rename_all = "snake_case")]`를 enum 표준으로. `WsMessage`는 `#[serde(tag = "type", content = "data")]`로 프론트엔드 호환.
5. **에러는 tracing span과 대응** — 각 `GadgetronError` variant는 span field와 retry policy를 명시 (§2.4의 표).
6. **Phase 1 ≠ Phase 2** — Phase 2 필드/variant는 주석 또는 `// [P2]` 플레이스홀더로 남기되, 타입 본체는 `[P1]`에 고정한다.
7. **과금 경계 명시 (D-20260411-06)** — routing hot path는 f64, 청구·정산은 i64 cents. 변환은 `gadgetron-xaas::QuotaEnforcer::record_post()` 한 곳에서만 발생. core는 구조만 정의한다.
8. **liveness ≠ readiness (D-20260411-08 + Round 2 HIGH #8)** — provider trait은 "엔진 프로세스 살아있음" 과 "특정 모델 로드 완료" 를 별도 메서드로 구분한다.

### 1.5 Trade-off

- `#[non_exhaustive]` 채택 ↔ 외부 crate의 `match` exhaustiveness 불편 → matched by default arm 및 rustdoc 예시로 완화.
- core 크기 증가 ↔ 중복 제거 이득 → D-12 크레이트 경계표가 허용. core는 **trait + 데이터 타입**만 보유하고 로직은 모두 하위 크레이트.
- f64 cost 필드 유지(`CostEntry`, `RoutingDecision::estimated_cost_usd`) ↔ D-8 (i64 cents 과금) → D-20260411-06 옵션 A 확정. "advisory" 필드는 f64, 청구는 i64 cents, 변환 지점 1곳. Q-1 **🟢 closed**.
- 공유 UI 타입을 core에 둠 ↔ UI-only 의존성 발생 위험 → core는 `chrono`/`serde`만 이미 사용. 추가 의존성 0 (D-20260411-07).

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

#### 2.1.1 `lib.rs` 모듈 선언 [P1]

```rust
//! gadgetron-core — 워크스페이스 공통 타입/트레이트/에러/설정
//! D-12 크레이트 경계표 + D-20260411-06/07/08 만족, Round 1 C-1~C-5 타입 충돌 방지.

pub mod config; pub mod error; pub mod message; pub mod model;
pub mod node; pub mod provider; pub mod routing;
pub mod ui;        // [P1] D-20260411-07: 공유 UI 타입 (TUI + Web + Gateway WS)

pub mod prelude {
    pub use crate::error::{GadgetronError, Result};
    pub use crate::message::{Message, Role, Content, ContentPart};
    pub use crate::model::{
        ModelState, ModelDeployment, InferenceEngine, Quantization,
        DownloadState, PortAllocator, RangePortAllocator, estimate_vram_mb,
    };
    pub use crate::node::{
        NodeResources, NodeStatus, NodeConfig, GpuInfo,
        NumaTopology, NumaNode, GpuNumaMapping, NvLinkInfo, ComputePartitioning,
    };
    pub use crate::provider::{LlmProvider, ChatRequest, ChatResponse, ChatChunk, ModelInfo, Usage};
    pub use crate::routing::{
        RoutingStrategy, RoutingConfig, RoutingDecision, ProviderMetrics,
        ParallelismConfig, EvictionPolicy,
    };
    // D-20260411-07 공유 UI 타입
    pub use crate::ui::{
        GpuMetrics, ModelStatus, ModelStatusKind, RequestEntry, ClusterHealth,
        HealthStatus, Alert, AlertSeverity, WsMessage,
    };
}
```

#### 2.1.2 `GadgetronError` 확장 (D-13 + D-20260411-08 + D-20260411-13) [P1]

```rust
// src/error.rs — 기존 12 + D-13 5 + D-20260411-08 1 + D-20260411-13 1 = 19 variant, #[non_exhaustive]

/// D-20260411-13: DB-agnostic 에러 분류 (D-12 leaf 원칙: core 에 sqlx/diesel/tokio-postgres
/// 의존성 0). consumer (`gadgetron-xaas`) 가 helper fn `sqlx_to_gadgetron` 으로 매핑 (§3.3).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DatabaseErrorKind {
    RowNotFound, PoolTimeout, ConnectionFailed,
    QueryFailed, MigrationFailed, Constraint, Other,
}

#[derive(Debug, thiserror::Error)] #[non_exhaustive]
pub enum GadgetronError {
    // --- 기존 12: Provider, Router, Scheduler, Node, Config,
    //     NoProvider, AllProvidersFailed, InsufficientResources,
    //     ModelNotFound, NodeNotFound, HealthCheckFailed{provider,reason}, Timeout(u64) ---

    #[error("Billing error: {0}")]        Billing(String),          // D-13 xaas::billing
    #[error("Tenant not found: {0}")]     TenantNotFound(String),   // D-13 xaas::tenant
    #[error("Quota exceeded: {0}")]       QuotaExceeded(String),    // D-13 limit/used/reset 포함
    #[error("Download failed: {0}")]      DownloadFailed(String),   // D-13 HTTP/checksum/disk
    #[error("Hot swap failed: {0}")]      HotSwapFailed(String),    // D-13 [P2]

    /// D-20260411-08: SSE/chunked 스트림 중단. reason 표준 태그 권장:
    /// `client_abort`/`network_timeout`/`idle_timeout`/`upstream_eof`.
    #[error("Stream interrupted: {reason}")]
    StreamInterrupted { reason: String },

    /// D-20260411-13: DB 에러 분류 + 메시지. core 는 sqlx 의존성 0, consumer helper 로 매핑.
    #[error("Database error ({kind:?}): {message}")]
    Database { kind: DatabaseErrorKind, message: String },
}
pub type Result<T> = std::result::Result<T, GadgetronError>;
```

##### tracing span 필드 및 retry 정책

| Variant | level | span 필드 | Retry | Prometheus `error_kind` |
|---------|:---:|-----------|:---:|:---:|
| `Provider(_)` | WARN | `provider`, `model` | ❌ (fallback chain) | `provider` |
| `Router(_)` | ERROR | `strategy` | ❌ | `router` |
| `Scheduler(_)` | ERROR | `model_id`, `node_id` | ❌ | `scheduler` |
| `Node(_)` | WARN | `node_id` | ✅ (heartbeat) | `node` |
| `Config(_)` | ERROR | `path` | ❌ (부팅 실패) | `config` |
| `NoProvider(_)` | WARN | `model` | ❌ → 404 | `no_provider` |
| `AllProvidersFailed(_)` | ERROR | `model`, `chain_len` | ❌ → 502 | `all_providers_failed` |
| `InsufficientResources(_)` | WARN | `node_id`, `required_mb` | 부분 (eviction 후 1회) | `insufficient_resources` |
| `ModelNotFound(_)` | INFO | `model_id` | ❌ → 404 | `model_not_found` |
| `NodeNotFound(_)` | WARN | `node_id` | ❌ → 404 | `node_not_found` |
| `HealthCheckFailed{..}` | WARN | `provider`, `reason` | ✅ (지수 백오프) | `health_check_failed` |
| `Timeout(ms)` | WARN | `ms` | ✅ (멱등 1회) | `timeout` |
| **`Billing(_)`** | ERROR | `tenant_id`, `stage` | ❌ → 500 + rollback | `billing` |
| **`TenantNotFound(_)`** | INFO | `api_key_hash` | ❌ → 401 | `tenant_not_found` |
| **`QuotaExceeded(_)`** | WARN | `tenant_id`, `limit`, `used` | ❌ → 429 `Retry-After` | `quota_exceeded` |
| **`DownloadFailed(_)`** | WARN | `model_id`, `source_url` | ✅ (지수 백오프 3회) | `download_failed` |
| **`HotSwapFailed(_)`** | ERROR | `model_id`, `from_pid`, `to_pid` | ❌ (traffic rollback) | `hot_swap_failed` |
| **`StreamInterrupted{..}`** | WARN | `reason`, `cause?`, `request_id`, `provider`, `model` | ✅ (`tower::retry` 1회, 멱등) → 499 | `stream_interrupted` |
| **`Database{ PoolTimeout }`** | WARN | `tenant_id?`, `query_id?`, `pool_size` | ✅ 1회 retry (멱등) | `db_pool_timeout` |
| **`Database{ RowNotFound }`** | INFO | `query_id?`, `entity` | ❌ → 404 | `db_row_not_found` |
| **`Database{ ConnectionFailed }`** | ERROR | `dsn_host`, `attempt` | ✅ 3회 retry w/ exponential backoff | `db_connection` |
| **`Database{ Constraint }`** | INFO | `constraint?`, `entity` | ❌ → 409 (client error) | `db_constraint` |
| **`Database{ MigrationFailed }`** | ERROR | `version`, `path` | ❌ → 부팅 실패 | `db_migration` |
| **`Database{ QueryFailed/Other }`** | ERROR | `query_id?`, `kind` | ❌ → 500 | `db_query` |

**`StreamInterrupted` 상세 (D-20260411-08)**: `reason` 은 사람이 읽는 문자열, `cause` 는 표준 태그 (`client_abort` / `network_timeout` / `idle_timeout` / `upstream_eof`) — tracing span 필드로만 export (Error Display 에는 포함 안 함, API body PII 안전성). `gadgetron-gateway/src/sse.rs` 가 `Body::poll_frame` Err 를 감지하면 이 variant 로 매핑. `gadgetron-provider` 각 adapter(`chat_stream`) 는 network error / tokio timeout / client abort 를 개별 태그로 래핑. `tower::retry` 레이어가 `StreamInterrupted` 만 1회 재시도, `Provider(_)` 는 fallback chain 관할이므로 제외.

**`Database{ kind, message }` 상세 (D-20260411-13)**: leaf crate 보존 — `gadgetron-core` 는 `sqlx::Error` wrap 금지, `From<sqlx::Error>` 미정의. consumer (`gadgetron-xaas`) 가 helper fn `sqlx_to_gadgetron(e) -> GadgetronError { Database { kind: classify(&e), message: e.to_string() } }` + `.map_err(sqlx_to_gadgetron)?` 로 매핑 (orphan rule 회피). `kind: Copy+Eq+Serialize` → Prometheus 라벨/HTTP status/retry 분기 무할당. `message` 는 디버그 전용, gateway prod 변환 레이어에서 redact (gateway-router-lead 소관).

##### 신규 `From` 구현 [P1]

```rust
// 각 하위 크레이트에서 발생하는 에러 wrap
impl From<std::io::Error> for GadgetronError {
    fn from(e: std::io::Error) -> Self { Self::Config(e.to_string()) }
}
impl From<serde_json::Error> for GadgetronError {
    fn from(e: serde_json::Error) -> Self { Self::Config(format!("JSON: {e}")) }
}
impl From<toml::de::Error> for GadgetronError {
    fn from(e: toml::de::Error) -> Self { Self::Config(format!("TOML: {e}")) }
}
// reqwest::Error는 `gadgetron-provider`에서 wrap — core에는 reqwest 의존성 추가 금지.
// sqlx::Error는 `gadgetron-xaas::sqlx_to_gadgetron` helper로 wrap — D-20260411-13.
//   core에 sqlx 의존성 추가 금지 (D-12 leaf 원칙). orphan rule 우회 위해 helper fn 채택.
```

> **원칙**: `From` 구현은 **core 자체 의존성만** 커버. `reqwest`/`sqlx`/`nvml-wrapper` 등 하위 크레이트 에러는 해당 크레이트에서 `.map_err(|e| GadgetronError::…(e.to_string()))` 또는 `.map_err(sqlx_to_gadgetron)?` (D-20260411-13) 로 wrap한다. 이는 D-12 크레이트 경계를 보호한다.

#### 2.1.3 `ParallelismConfig` (D-1) [P1]

위치: `gadgetron-core/src/routing.rs` (D-12).

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelismConfig {
    pub tp_size: u32, pub pp_size: u32, pub ep_size: u32, pub dp_size: u32,  // TP/PP/EP(MoE)/DP
    pub numa_bind: Option<u32>,  // numactl --cpunodebind
}
impl Default for ParallelismConfig {
    fn default() -> Self { Self { tp_size: 1, pp_size: 1, ep_size: 1, dp_size: 1, numa_bind: None } }
}
impl ParallelismConfig {
    /// 총 GPU 소비 = tp * pp * dp (ep 는 MoE 내부 분할이므로 제외)
    pub fn gpu_count(&self) -> u32 {
        self.tp_size.saturating_mul(self.pp_size).saturating_mul(self.dp_size)
    }
    pub fn validate(&self, available_gpus: u32) -> Result<()>; // 각 차원 ≥1, gpu_count ≤ available
}
```

**참고**: gpu-resource-manager.md §5.2의 `gpu_ids: Vec<u32>` 필드는 **배치 결정 결과**이므로 `gadgetron-scheduler::Placement` 타입으로 이동 (D-12 경계). Core의 `ParallelismConfig`는 사용자 의도만 표현.

#### 2.1.4 `EvictionPolicy` (D-2) [P1]

위치: `gadgetron-core/src/routing.rs`.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")] #[non_exhaustive]
pub enum EvictionPolicy {
    Lru,                                           // 기본 정책
    Priority,                                      // ModelDeployment.priority 사용
    CostBased,                                     // [P2] fieldless stub, f64→f64 비교만 (D-06)
    WeightedLru { priority_weight: f32 },          // score = lru*(1-w) + priority*w
}
impl Default for EvictionPolicy { fn default() -> Self { Self::Lru } }
```

**`WeightedLru` 검증**: `priority_weight ∈ [0.0, 1.0]`. 범위 외는 `GadgetronError::Config`.

**`CostBased` — D-20260411-06 f64/i64 경계 (A8 stub 구조)**: router f64 `RoutingDecision::estimated_cost_usd` → scheduler 내부 f64→f64 비교만. 청구용 i64 cents 변환은 `gadgetron-xaas::QuotaEnforcer::record_post()` 단 1곳 (D-06). core 구조만 정의.
- Phase 1: fieldless variant. Scheduler 는 match 후 `warn!("cost-based eviction not wired, falling back to LRU")` + Lru fallback (`todo!()` 금지).
- Phase 2: `CostBased { threshold_usd_per_hour: f64 }` 확장, `#[non_exhaustive]` 덕분 breaking-change 없음.
- 변환: `gadgetron-xaas/src/quota/enforcer.rs::compute_cost_cents(usage, rate) -> i64` 유일. routing/scheduler/core 전부 f64.
- f64 유지 필드: `CostEntry::{input,output}`, `RoutingDecision::estimated_cost_usd`, `ProviderMetrics::total_cost_usd`.

#### 2.1.5 `NumaTopology`, `NumaNode`, `GpuNumaMapping`, `NvLinkInfo`, `ComputePartitioning` (D-3) [P1]

위치: `gadgetron-core/src/node.rs` (D-12). 필드 스펙은 `gpu-resource-manager.md §5.1` 그대로 채택.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaTopology { pub nodes: Vec<NumaNode>, pub gpus: Vec<GpuNumaMapping> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaNode {
    pub id: u32, pub cpu_cores: Vec<u32>,
    pub memory_total_mb: u64, pub memory_available_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuNumaMapping {
    pub gpu_index: u32, pub numa_node: u32,
    pub pcie_bus_id: String,       // "/sys/bus/pci/devices/*"
    pub nvlinks: Vec<NvLinkInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NvLinkInfo {
    pub remote_gpu_index: u32,
    pub link_count: u32,           // A100=12, H100=18 max
    pub bandwidth_gbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")] #[non_exhaustive]
pub enum ComputePartitioning {
    Dedicated,
    Mps { active_thread_percentage: u32, default_pinned_mem_pct: u32 },
    TimeSlicing { time_slice_ms: u32 },       // 기본 20ms
    Mig { profile: String },                   // Phase 1 정적 enable/disable만 (D-20260411-04)
}

impl NumaTopology {
    pub fn nvlink_groups(&self) -> Vec<Vec<u32>>;  // union-find, hot path 바깥 1회 계산
    pub fn gpus_on_numa(&self, numa_id: u32) -> Vec<u32>;
}
```

**동시성**: `Arc<NumaTopology>`로 공유, Phase 1 immutable. Phase 2 dynamic reconfig은 Q-2.

**MIG 타입 배치 (A7)**: `MigProfile`, `MigInstance`, `MigMode` 타입은 **`gadgetron-node/src/mig.rs`** 에 배치한다 (D-12 크레이트 경계표에 행 추가 예정). Core의 `ComputePartitioning::Mig { profile: String }`은 serde-호환 문자열만 참조 — MIG 조작 로직은 node 크레이트 소관. `GpuMonitor::mig_instances()` 반환 타입(`Vec<MigInstance>`)은 node 크레이트에서 정의되며, core의 `GpuMonitor` trait alias에서는 generic associated type으로 노출하지 않는다 (D-12 leaf 원칙 유지).

#### 2.1.6 `ModelState` 확장 (D-10) [P1]

위치: `gadgetron-core/src/model.rs`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")] #[non_exhaustive]
pub enum ModelState {
    NotDownloaded,                                  // 로컬 weight 없음
    Downloading { progress: f32 },                  // D-10: f32 유지
    Registered,                                     // 레지스트리 등록 완료
    Loading,                                        // 엔진 로딩 중 (CUDA init, mmap)
    Running { port: u16, pid: u32 },                // D-10: pid 추가 (kill/lineage)
    Unloading,                                      // 프로세스 종료 대기
    Failed(String),
    // [P2] Draining, HotSwapping { next_pid: u32 }
}
```

**전이**: §2.2.2 다이어그램 참조. 불변식: `Running`에서만 `port`와 `pid`가 유효. `pid == 0`은 Ollama HTTP 프로토콜 전용(외부 프로세스 미소유) 센티넬.

#### 2.1.7 `PortAllocator` 트레이트 (M-3) [P1]

위치: `gadgetron-core/src/model.rs` (D-12 경계표 명시).

```rust
pub trait PortAllocator: Send + Sync + 'static {
    fn allocate(&self, hint: Option<u16>) -> Result<u16>;  // hint 가능 → hint, 실패 시 InsufficientResources
    fn release(&self, port: u16);                          // idempotent
    fn is_available(&self, port: u16) -> bool;
}

/// Phase 1 기본 구현. BTreeSet O(log n), 노드당 ~256 포트.
/// # SAFETY (A5)
/// 내부 `Mutex` poisoning = **fatal node state**. `.expect("poisoned")` → 프로세스 종료,
/// scheduler 는 heartbeat timeout (D-20260411-04 stale threshold 30초) 으로 노드 isolate.
pub struct RangePortAllocator { start: u16, end: u16, used: Arc<Mutex<BTreeSet<u16>>> }

impl RangePortAllocator { pub fn new(start: u16, end: u16) -> Self; /* assert start<=end */ }
// impl PortAllocator: allocate=hint|first-free|Err, release=remove(no-op if absent), is_available=range∧!used
```

**동시성**: `Arc<Mutex<BTreeSet<u16>>>` — 배포 시 1회 호출, critical section 짧음, `std::sync::Mutex` (parking_lot 금지, core dep 축소). `Send+Sync+'static` → `Arc<dyn PortAllocator>` DI. poisoning `.expect()` + heartbeat 감지 — **A5 rustdoc 반영, Q-4 🟢 closed**.

**Phase 1 기본값**: vLLM=8000-8099, SGLang=30000-30099, LlamaCpp=8080-8179, TGI=3000-3099.

#### 2.1.8 `DownloadState` enum [P1]

위치: `gadgetron-core/src/model.rs`. Round 1 C-5 해소 (xaas-platform의 `DownloadState` 네이밍 채택, `DownloadStatus`는 제거).

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")] #[non_exhaustive]
pub enum DownloadState {
    Queued,
    Downloading { progress: f32, bytes_done: u64, bytes_total: u64 }, // 0.0~1.0, total 0=unknown
    Verifying,    // SHA256 검증
    Registered,   // 레지스트리 등록 완료
    Failed(String),
}
impl DownloadState {
    pub fn is_terminal(&self) -> bool { matches!(self, Self::Registered | Self::Failed(_)) }
}
```

**`ModelState::Downloading { progress: f32 }`와의 관계**: `ModelState`는 *서빙 수명주기*, `DownloadState`는 *다운로드 파이프라인*의 디테일. 스케줄러는 `ModelState`만 참조. XaaS/catalog 다운로드 관리 레이어만 `DownloadState`를 관찰.

#### 2.1.9 Shared UI Types (D-20260411-07) [P1]

위치: **신규 모듈 `gadgetron-core/src/ui.rs`**. Round 2 BLOCKER #8 + Round 1 A1 blocking 해소. 필드는 `docs/ui-ux/dashboard.md §8.3` 1:1 매칭. TUI/Web UI 동일 Axum 백엔드 + WebSocket 공유, 타입 중복 방지를 위해 core 집중 (D-20260411-07 옵션 A). **신규 crate 의존성 0** — `chrono`/`serde` 기존 workspace dep.

```rust
// src/ui.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::routing::RoutingDecision;

/// GPU 메트릭 스냅샷 (1초 주기, WebSocket broadcast 단위).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuMetrics {
    pub node_id: String, pub gpu_index: u32,
    pub vram_used_mb: u64, pub vram_total_mb: u64,
    pub utilization_pct: f32, pub temperature_c: f32,
    pub power_w: f32, pub power_limit_w: f32,
    pub clock_mhz: u32, pub fan_rpm: u32,
}

/// 모델 서빙 상태 UI 뷰 — `ModelState` 의 UI-friendly 투영.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelStatus {
    pub model_id: String, pub status: ModelStatusKind,
    pub engine: String, pub version: String,
    pub node_id: Option<String>, pub vram_used_mb: Option<u64>,
    pub loaded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelStatusKind { Running, Loading, Stopped, Error, Draining }
// Draining → Phase 2 `ModelState::Draining` 매핑, Phase 1 wire만 확보.

/// 요청 1건 감사·관측 엔트리. `RequestLog` WebSocket 페이로드.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEntry {
    pub request_id: String, pub timestamp: DateTime<Utc>,
    pub model: String, pub provider: String,
    pub status: u16, pub latency_ms: u32,
    pub prompt_tokens: u32, pub completion_tokens: u32, pub total_tokens: u32,
    pub routing_decision: Option<RoutingDecision>,  // 타입 재사용 → C-3 재발 방지
}

/// 클러스터 레벨 헬스 스냅샷.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterHealth {
    pub status: HealthStatus, pub total_nodes: u32, pub online_nodes: u32,
    pub total_gpus: u32, pub active_gpus: u32, pub running_models: u32,
    pub requests_per_minute: f32, pub error_rate_pct: f32,
    /// advisory f64 — D-20260411-06 routing hint. Billing 은 xaas i64 cents.
    pub cost_per_hour_usd: f64,
    pub alerts: Vec<Alert>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")] #[non_exhaustive]
pub enum HealthStatus { Healthy, Degraded, Critical }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String, pub severity: AlertSeverity,
    pub message: String, pub since: DateTime<Utc>,
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")] #[non_exhaustive]
pub enum AlertSeverity { Info, Warning, Error, Critical }

/// 게이트웨이 → TUI/Web WebSocket 프레임. tag-content 직렬화 → TypeScript discriminated union 호환.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]      // D-20260411-07: forward-compat 필수
pub enum WsMessage {
    GpuMetrics(Vec<GpuMetrics>),  // 100ms window aggregate, 단일도 Vec<1> (Q-8)
    ModelStatus(ModelStatus),
    RequestLog(RequestEntry),
    ClusterHealth(ClusterHealth),
}
```

`#[non_exhaustive]` 는 4개 enum 전부(`WsMessage`/`ModelStatusKind`/`HealthStatus`/`AlertSeverity`) 에 적용. Phase 2 variant 추가 breaking change 없음. dashboard.md §8.3 의 `GpuMetricsBatch` 는 `Vec<GpuMetrics>` 로 단순화 (type alias 는 `non_exhaustive` 부적합).

**Serde 사용 범위 (A7)**: Phase 1 에서 `ui::*` 직렬화는 **JSON only** — gateway WebSocket (`/ws/{metrics,models,requests,cluster}`) 4 endpoint + HTTP API (`/api/v1/{nodes,models,usage}`) 응답. TUI (`gadgetron-tui`) 는 **in-process value** 로 동일 타입을 공유하되 serde 경로를 사용하지 않는다 (ratatui 가 값을 직접 렌더). Web UI 프론트엔드는 Phase 2 에 msgpack/binary WebSocket 프로토콜 여부를 재결정 — Phase 1 은 `serde_json` 단일 포맷으로 고정해 TypeScript discriminated union 매핑 안정성을 확보한다.

#### 2.1.10 `LlmProvider` 트레이트 (Round 2 HIGH #8) [P1]

위치: `gadgetron-core/src/provider.rs`. Round 1 A3 blocking — liveness (`is_live`) 와 readiness (`is_ready(model)`) 를 분리한다.

```rust
use async_trait::async_trait;
use futures::stream::BoxStream;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;  // metric/span label: "openai"|"anthropic"|"gemini"|"vllm"|"sglang"|"ollama"

    /// **liveness** (엔진 프로세스). 30초 health loop.
    /// 구현: vLLM/SGLang/TGI/llama.cpp=`GET /health`; OpenAI=`HEAD /v1/models`;
    /// Anthropic=TCP+meta (1-token 금지, quota 소모); Gemini=`models.list` (D-20260411-02 polling 래핑);
    /// Ollama=`GET /api/version`.
    async fn is_live(&self) -> Result<bool>;

    /// **readiness** (모델 로드 완료). Scheduler `Loading → Running` 전이 조건.
    /// vLLM/SGLang/TGI/llama.cpp=`GET /v1/models` 파싱; Ollama=`GET /api/tags`;
    /// OpenAI/Anthropic/Gemini=managed → `Ok(true)` 상수 (SLA 신뢰).
    async fn is_ready(&self, model: &str) -> Result<bool>;

    async fn models(&self) -> Result<Vec<ModelInfo>>;
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;
    /// 스트림 중단 시 `StreamInterrupted { reason }` (D-20260411-08).
    async fn chat_stream(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<ChatChunk>>>;
}
```

**분리 근거 (Round 2 HIGH #8)**: Anthropic 1-token health check 는 quota 소모 → `is_live()` 분리로 health loop 무과금. vLLM/SGLang `/health` (cost 0) 는 liveness 판단, 모델 로드는 `/v1/models` 파싱 필요 → 별도 메서드 필수. 이전 `health_check` 혼용은 "프로세스 up, 모델 미로드" 상태 구분 불가. `docs/modules/model-serving.md §3.4` 2단 분리 와 일치.

**Dynamic Dispatch Rationale (D-20260411-11)**: provider registry 는 `Vec<Arc<dyn LlmProvider>>`. router/scheduler/test fixture 모두 trait object. Generic `Router<P: LlmProvider>` 검토 → **거부**, 4단 근거:

1. **성능 math (10^8 격차)** — provider 호출 1회 ≈ HTTP+inference **100ms** (OpenAI median, vLLM 12B p50). vtable lookup ≈ **1ns** (L1 hit, indirect call). 비율 **10^8**. router hot path 5ns 합쳐도 P99 < 1ms SLO 의 0.0005% — noise floor 이하, monomorphization 절약 측정 불가.
2. **Config-driven 6-provider runtime polymorphism** — `ProviderConfig` 가 TOML 에서 OpenAI/Anthropic/Gemini/vLLM/SGLang/Ollama 6 종을 런타임 선택·조합 (D-20260411-02 fallback chain). static generic 은 enum tag 컴파일타임 단일화 필수 → 이종 provider 합성 불가.
3. **Workspace-wide refactor 비용** — `router`/`gateway`/`scheduler`/`testing` 4 크레이트 type parameter 전파 + fixture rewrite. 1 dev × 1 week + 재리뷰 cycle. 위 (1) 로 보상 무.
4. **Trait object 표준 패턴** — `tower::Service`/`axum::Handler`/`tracing::Subscriber` 모두 동일. Mock provider 도 `Arc<dyn LlmProvider>` 로 inject (Track 2 `docs/design/testing/harness.md`).

→ **결론**: `Arc<dyn LlmProvider>` 유지. `Send + Sync` 강제로 async `'static` 요건 충족. SLO 위협 시 Phase 2 에 hot-path generic split 재검토 (옵션 C Hybrid). D-20260411-11 PM 옵션 A 승인 (2026-04-12).

**rustdoc doctest (A3, UT-prov-1)** — `cargo test --doc` 로 실행되는 실제 compile 예시:

```rust
/// ```
/// use async_trait::async_trait;
/// use futures::stream::BoxStream;
/// use gadgetron_core::prelude::*;
/// struct FakeProvider;
/// #[async_trait]
/// impl LlmProvider for FakeProvider {
///     fn name(&self) -> &'static str { "fake" }
///     async fn is_live(&self) -> Result<bool> { Ok(true) }
///     async fn is_ready(&self, m: &str) -> Result<bool> { Ok(m == "llama-3-8b") }
///     async fn models(&self) -> Result<Vec<ModelInfo>> { Ok(vec![]) }
///     async fn chat(&self, _: ChatRequest) -> Result<ChatResponse> { unimplemented!() }
///     async fn chat_stream(&self, _: ChatRequest)
///         -> Result<BoxStream<'static, Result<ChatChunk>>> { unimplemented!() }
/// }
/// # tokio_test::block_on(async {
/// let p = FakeProvider;
/// assert!(p.is_live().await.unwrap());
/// assert!(p.is_ready("llama-3-8b").await.unwrap());
/// assert!(!p.is_ready("llama-3-70b").await.unwrap());
/// # });
/// ```
```

### 2.2 내부 구조

#### 2.2.1 동시성 모델

| 타입 | 공유 방식 | 선택 근거 |
|------|----------|---------|
| `NumaTopology` | `Arc<NumaTopology>` (immutable) | Phase 1에서 부팅 1회 수집, mutate 없음 |
| `ParallelismConfig` | `Clone` (small, value) | GPU당 배치 시 1회 복사 |
| `RangePortAllocator` | `Arc<dyn PortAllocator>` | trait object, 노드 전역 1개 |
| `ModelState` | `Arc<RwLock<HashMap<String, ModelState>>>` (scheduler) | 다중 읽기, 드문 쓰기 |
| `GadgetronError` | value, `?`로 전파 | 소유권 이동만 |
| `WsMessage` / `ClusterHealth` | value, `tokio::sync::broadcast` 채널 | gateway가 N subscriber에 fan-out |
| `LlmProvider` | `Arc<dyn LlmProvider>` | trait object, provider registry 소유 |

core 자체는 `RwLock`/`DashMap`/`Mutex` **소유자가 아님**. 소비 크레이트(scheduler, node, gateway)가 소유한다. 이유: core는 데이터와 trait만 정의, 상태머신은 도메인 크레이트.

#### 2.2.2 `ModelState` 상태머신

```text
NotDownloaded → Downloading → Registered → Loading → Running{port,pid}
                     ↓                         ↓          ↓
                  Failed                    Failed     Unloading → (removed)
                                                          ↓
                                                        Failed
```

전이 검증 함수는 `gadgetron-scheduler`가 소유. Core는 enum 정의만.

### 2.3 설정 스키마

Phase 1 core config 변경 없음 (`AppConfig` 기존 유지). 새 타입 등장 시 예시:

```toml
# config/nexus.toml
[router]
default_strategy = { type = "round_robin" }
eviction = { type = "weighted_lru", priority_weight = 0.3 }  # D-2

[[models]]
id = "llama-3-70b"; engine = "vllm"; vram_requirement_mb = 44032
[models.parallelism]                                         # D-1
tp_size = 4; pp_size = 1; ep_size = 1; dp_size = 1; numa_bind = 0
```

검증: `parallelism.tp*pp*dp <= nodes[n].gpu_count`, `priority_weight ∈ [0.0,1.0]`, TOML 역직렬화 실패 → `GadgetronError::Config(format!("TOML: {e}"))`.

### 2.4 에러 & 로깅

§2.1.2 표 참조. tracing 규칙: 경계 함수 `#[tracing::instrument(skip(self))]`, span target `gadgetron_core::<module>`, Error Display 에 PII/내부 경로 금지, `StreamInterrupted.cause` 는 span field only (Display 에는 `reason` 만 — API body 안전성).

### 2.5 의존성

Phase 1 `gadgetron-core/Cargo.toml` — **신규 의존성 0** (dev-dep `criterion` 은 §4.4). 기존 workspace deps: `serde`, `serde_json`, `toml`, `thiserror`, `async-trait`, `futures`, `tokio`, `tracing`, `uuid`, `chrono`. **추가 금지 (D-12 강화)**: `reqwest`/`sqlx`/`nvml-wrapper`/`axum`/`tonic`/`parking_lot`.

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 의존성 그래프 (D-12 준수)

```
gadgetron-core (leaf, 외부 crate만 참조)
  ← provider ← router ← gateway
  ← scheduler  ← gateway
  ← node       ← gateway
  ← xaas       ← gateway
  ← tui, web   (WebSocket client, 타입 공유)
  ← testing    (dev-dep only, 모든 crate)
```

gateway 는 `/ws/{metrics,models,requests,cluster}` 4개 WebSocket endpoint 에서 `core::ui::WsMessage` broadcast. Phase 1 web 은 타입만 공유, 본 구현은 Phase 2.

- Core 는 leaf — 순환 의존 불가. `GadgetronError::Billing` 등도 문자열 래핑만.
- **A6 VRAM sync 메모**: 런타임 VRAM 동기화 프로토콜(push/pull/heartbeat/stale threshold)은 `docs/design/scheduler/phase1.md §4.1.1` 소관 (Week 3+, gpu-scheduler-lead). Core 는 `ModelState::Running{pid}` + `Arc<NumaTopology>` immutable + `GpuMetrics` broadcast 타입만 제공; 동기화 state machine 은 scheduler 가 소유.

> **D-12 크레이트 경계표 업데이트 필요 (A3, `docs/reviews/pm-decisions.md`)**: Round 3 xaas-platform-lead 가 D-12 표 `gadgetron-xaas` 행 outdated 지적. 추가 필요 타입: `Scope`, `ApiKey`, `ValidatedKey`, `KeyValidator`, `KeyKind`, `TenantContext`, `Tenant`, `TenantSpec`, `TenantRegistry`, `QuotaConfig`, `QuotaEnforcer`, `QuotaToken`, `AuditEntry`, `AuditWriter`, `PgKeyValidator` (with `moka::future::Cache` per D-20260411-12, FYI only), `sqlx_to_gadgetron` helper (per D-20260411-13, orphan rule 회피). PM 결정 변경 아님 → chief-architect 직접 반영. Track 3 (`docs/design/xaas/phase1.md`) 가 owner.

### 3.2 타입별 consumer 매트릭스

모든 타입은 `gadgetron-core` 가 **def (정의)**. 아래는 각 타입을 **consume** 하는 크레이트.

| 타입 | consumer |
|------|----------|
| `GadgetronError` (D-13, 전 variant) | provider, router, gateway, scheduler, node, xaas, tui, web, testing |
| `GadgetronError::StreamInterrupted` (D-08) | provider (매핑), gateway (SSE 변환), testing |
| **`GadgetronError::Database` + `DatabaseErrorKind` (D-13 확장 / D-20260411-13)** | xaas (helper fn `sqlx_to_gadgetron` 매핑 + 사용), gateway (HTTP status mapping), testing |
| `ParallelismConfig` (D-1) | scheduler, node, testing |
| `EvictionPolicy` (D-2) | scheduler, testing |
| `NumaTopology`/`GpuNumaMapping`/`NvLinkInfo`/`ComputePartitioning` (D-3) | scheduler, node, testing |
| `ModelState` (D-10) | router, gateway, scheduler, node, xaas, tui, web, testing |
| `PortAllocator` / `RangePortAllocator` (M-3) | node, testing |
| `DownloadState` | gateway, scheduler, xaas, tui, web, testing |
| `LlmProvider` (trait, HIGH #8) | provider (impl), router, gateway, testing |
| **`ui::GpuMetrics` (D-07)** | gateway (WS broadcast), node (src), tui, web, testing |
| **`ui::ModelStatus` (D-07)** | gateway (WS), scheduler (src), tui, web, testing |
| **`ui::RequestEntry` (D-07)** | gateway (WS + src), tui, web, testing |
| **`ui::ClusterHealth` (D-07)** | gateway (WS), scheduler (src), tui, web, testing |
| **`ui::WsMessage` (D-07)** | gateway (4 WS endpoints), tui, web, testing |

(WS) = WebSocket 전송, (src) = 소스 데이터 생산자.

### 3.3 인터페이스 계약 (타 에이전트 도메인)

**→ `gateway-router-lead`**: `GadgetronError::{QuotaExceeded,TenantNotFound,Billing,StreamInterrupted}` → HTTP 429/401/500/499 변환 (Track 3 §2.4). 9단계 미들웨어(Round 2 §3.1) 가 `Result<T, GadgetronError>` 를 `?` 로 전파. **WebSocket 4개 엔드포인트** (`/ws/metrics`, `/ws/models`, `/ws/requests`, `/ws/cluster`) 가 `core::ui::WsMessage` 4 variant 를 각각 broadcast — gateway 소유 `tokio::sync::broadcast::Sender<WsMessage>` + `serde_json` 직렬화. SSE loop 의 `Body::poll_frame` Err → `StreamInterrupted { reason: classify_cause(e) }` (D-08).

**Track 3 HTTP Status Mapping cross-reference (A4, D-20260411-13)**: Track 3 `docs/design/xaas/phase1.md` §2.4.2 의 HTTP Status Mapping 은 본 문서 `GadgetronError` variant 와 **1:1 대응** 필수. 신규 `Database { kind, message }` 매핑:

| `Database { kind: ... }` | HTTP | 비고 |
|---|:---:|---|
| `PoolTimeout` | **503** | retryable, `Retry-After: 1` |
| `RowNotFound` | **404** | non-retryable |
| `ConnectionFailed` | **503** | retryable (3회 backoff) |
| `Constraint` | **409** | client error, non-retryable |
| `MigrationFailed` | **500** | startup fail, 부팅 거부 |
| `QueryFailed` / `Other` | **500** | non-retryable |

Track 3 의 `xaas/src/error.rs::sqlx_to_gadgetron` helper 가 `sqlx::Error → Database { kind, message }` 매핑하면 gateway `IntoResponse` 가 위 표 `match`. cross-ref 가 (i) Track 3 임의 status 변경 차단, (ii) `DatabaseErrorKind` 신규 variant 시 Track 3 동일 라운드 갱신 강제 (`#[non_exhaustive]` + compile_fail doctest 의 `_ => 500` arm 요구).

**→ `inference-engine-lead`**: `ModelState::Running { pid }` — `NodeAgent::start_model()` 이 `Child::id()` 저장. `PortAllocator` trait — `NodeAgent::new()` 에 `Arc<dyn PortAllocator>` 주입. **`LlmProvider::is_live` / `is_ready(model)` (Round 2 HIGH #8)** — provider crate 6개 adapter 모두 구현. `is_ready` 는 scheduler `Loading → Running` 전이 조건. 각 adapter 의 `chat_stream` 에러 경로에서 `network_timeout`/`client_abort`/`idle_timeout`/`upstream_eof` 태그로 `StreamInterrupted` 매핑.

**→ `gpu-scheduler-lead`**: `NumaTopology` + `GpuNumaMapping` + `NvLinkInfo` — `EnhancedScheduler` (D-20260411-04) NUMA-aware 배치. `ParallelismConfig` — `Scheduler::deploy(..., parallelism: &ParallelismConfig)`. `EvictionPolicy` — `SchedulerConfig::eviction_policy` (scheduler 소유). `CostBased` 는 f64→f64 비교만 (D-06). **VRAM 동기화 프로토콜 (A6)**: Core는 타입·trait만 제공. push/pull 주기·stale threshold·exponential backoff 은 `docs/design/scheduler/phase1.md §4.1.1` 소관 (Week 3+).

**→ `xaas-platform-lead`**: `GadgetronError` D-13 5 variant + `StreamInterrupted` + **`Database { kind, message }` (D-20260411-13)** — billing/tenant/quota/stream/db 경로 귀결. `DownloadState` — xaas catalog 진행률. **f64/i64 변환 경계 (D-06)**: routing f64 → `QuotaEnforcer::record_post() → compute_cost_cents() -> i64` 단 1곳, core는 f64만 정의 (§2.1.4). **DB 에러 매핑 (D-20260411-13)**: `gadgetron-xaas/src/error.rs::sqlx_to_gadgetron` helper fn 이 `sqlx::Error` → `GadgetronError::Database { kind, message }` 매핑을 owner — `gadgetron-core` 는 `sqlx` 의존성 0 유지 (D-12 leaf). **Phase 1 LRU cache (D-20260411-12, FYI only)**: `PgKeyValidator` 는 `moka::future::Cache<String, ValidatedKey>` (10k entries / 10min TTL) 보유 — Track 3 phase1 doc 소관, core 는 영향 없음.

**→ `qa-test-architect`**: `gadgetron-testing` prop-test 로 상태머신/allocator/parallelism 검증. **`non_exhaustive` × proptest (Q-5 🟢 closed 2026-04-12)**: core `testing-support` feature flag + `all_variants_for_test()` 7 타입 (§4.2.1). UT-port-6 loom 경로 동 flag. **Performance bench (§4.4)**: `crates/gadgetron-core/benches/` BM-alloc/BM-eviction-score/BM-vram-estimate + critcmp baseline. **`FakeGpuMonitor` (Track 2 A2)**: `ui::GpuMetrics` 10 필드 builder — Track 1 가 원천.

**→ `ux-interface-lead`**: `ModelState` 직접 렌더. **공유 UI 타입 (`GpuMetrics`/`ModelStatus`/`RequestEntry`/`ClusterHealth`/`Alert`/`WsMessage`) 은 `gadgetron-core::ui`** (D-07). 별도 crate 분리 없음, Round 1 Q-3 🟢 closed. `gadgetron-tui`/`gadgetron-web` 모두 `use gadgetron_core::ui::*;`.

### 3.4 D-12 크레이트 경계표 준수 확인

| D-12 행 → 파일 | 반영 섹션 |
|---------|:---:|
| `ParallelismConfig` → core/src/routing.rs | §2.1.3 ✅ |
| `NumaTopology` / `NumaNode` / `GpuNumaMapping` / `NvLinkInfo` / `ComputePartitioning` → core/src/node.rs | §2.1.5 ✅ |
| `EvictionPolicy` → core/src/routing.rs | §2.1.4 ✅ |
| `DownloadState` / `ModelState` (확장) / `PortAllocator` → core/src/model.rs | §2.1.6~8 ✅ |
| `LlmProvider` → core/src/provider.rs | §2.1.10 ✅ (HIGH #8) |
| `GadgetronError::StreamInterrupted` → core/src/error.rs | §2.1.2 ✅ (D-08) |
| **`GadgetronError::Database` + `DatabaseErrorKind` → core/src/error.rs** | **§2.1.2 ✅ (D-20260411-13, sqlx 의존성 0 유지)** |
| **`ui::{GpuMetrics,ModelStatus,RequestEntry,ClusterHealth,Alert,WsMessage}` → core/src/ui.rs (신규)** | **§2.1.9 ✅ (D-07)** |
| `MigProfile` / `MigInstance` / `MigMode` → **node/src/mig.rs** (A7) | §2.1.5 주석 ✅ (D-12 행 추가 예정) |
| `sqlx_to_gadgetron` helper → **xaas/src/error.rs** (D-20260411-13) | §3.3 xaas 계약 ✅ (orphan rule 회피, core 외부) |

스케줄러/노드/라우터 전용 타입(`SchedulerConfig`, `HotSwapManager`, `TrafficRouter`, `MigManager`, `ThermalController` 등)은 본 문서 **범위 외**.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

모든 테스트는 **`gadgetron-testing` (D-20260411-05) dev-dep** 을 통해 공용 fixture/prop-strategy 재사용.

| ID | 대상 | invariant | 하네스 |
|----|------|----------|-------|
| UT-err-1..5 | `GadgetronError` Display (`Provider`/`TenantNotFound`/`QuotaExceeded` 등) | PII 누출 없음, 필드 포함 | insta 스냅샷 |
| UT-err-6 | `From<io::Error>` / `From<toml::de::Error>` | Config variant wrap | assert |
| UT-err-7 | `#[non_exhaustive]` 동작 | 외부 match 에서 `_` arm 필수 | `compile_fail` doctest |
| UT-err-8 | `StreamInterrupted { reason }` Display (D-08) | `reason`만 노출, `cause` 는 span 전용 | insta 스냅샷 |
| UT-err-9 | `StreamInterrupted` retry eligibility | `is_retryable() == true` | assert |
| UT-err-10 | `Database { kind, message }` Display + serde (D-20260411-13) | `kind` Debug 출력 + 7 variant 모두 round-trip, `sqlx` 의존성 0 verify | insta + serde_json |
| UT-err-11 | `DatabaseErrorKind::all_variants_for_test()` 길이 7 + 모두 distinct | `#[non_exhaustive]` 누락 시 컴파일 실패 | table + compile_fail |
| UT-mod-1 | `ModelState` serde round-trip | 7 variant 모두 `to_str`/`from_str` 동치 | serde_json |
| UT-mod-2 | `ModelState` 전이 valid path | `NotDownloaded → … → Unloading` | state-machine builder |
| UT-mod-3 | `estimate_vram_mb` × **6 Quantization variant 매트릭스** (A6) | `{FP32, FP16, BF16, Q8_0, Q4_K_M, Q3_K_M}` × {0B, 7B, 13B, 70B, 175B, u64::MAX} table + `prop_oneof!` strategy; u64 saturating; 70B×Q4_K_M→44032 고정값; 각 양자화 별 bytes/param 상수 정확도 | table + prop-test |
| UT-mod-4 | `DownloadState::is_terminal` | Registered/Failed only | table |
| UT-rout-1 | `RoutingStrategy` / `EvictionPolicy` serde (4 variant, `CostBased` fieldless 포함) | TOML + serde_json round-trip | serde_json + TOML |
| UT-rout-2 | `EvictionPolicy::WeightedLru` 검증 | `priority_weight ∈ [0,1]` | prop-test |
| UT-rout-3 | `ParallelismConfig::gpu_count` / `validate` / `Default` | `tp*pp*dp`, ep 제외, 모두 1 | table + prop-test |
| UT-node-1 | `NumaTopology::nvlink_groups` | union-find 정확성 | prop-test |
| UT-node-2 | `NumaTopology::gpus_on_numa` + `ComputePartitioning` serde | NUMA 필터링, 4 variant 직렬화 | table + serde_json |
| UT-port-1..5 | `RangePortAllocator` alloc(None)/alloc(hint)/release idempotent/고갈/is_available | 시나리오별 불변식 | sequence test |
| UT-port-6 | `RangePortAllocator` 동시성 (N thread × 100 op) | 중복 없음 | prop-test + `loom` (feature `testing-support`, A9) |
| UT-ui-1 | `WsMessage` serde round-trip (4 variant) | TypeScript discriminated union 호환 (`type`/`data`) | insta snapshot |
| UT-ui-2 | `GpuMetrics` 필드 10개 완전성 | dashboard.md §8.3 와 1:1 매칭 | struct literal + serde_json |
| UT-ui-3 | `ClusterHealth.cost_per_hour_usd` f64 보존 (D-06) | 소수점 정확도 | serde_json round-trip |
| UT-ui-4 | `ModelStatusKind` / `HealthStatus` / `AlertSeverity` serde snake_case + 순서 | Web UI TS enum 호환 | serde_json + `Ord` derive |
| UT-ui-5 | `WsMessage::GpuMetrics(Vec<_>)` 배치 | 0·1·N GPU 모두 정상 직렬화 | serde_json |
| UT-prov-1 | `LlmProvider::is_live` / `is_ready` 시그니처 + **rustdoc doctest** (A3) | trait bounds + `async fn` + 실제 mock 구현 호출 | `cargo test --doc` |

### 4.2 테스트 하네스

- **Location**: 각 `src/*.rs` 내 `#[cfg(test)] mod tests`. Mock 없음 — core 는 외부 I/O 없음.
- **Property-based** (`proptest 1` via `gadgetron-testing`): UT-mod-3 (6-variant Quantization 매트릭스, A6), UT-rout-2~3, UT-node-1, UT-port-6. Shrink 전략은 `gadgetron-testing::props::{vram, eviction, ...}` 재사용.
- **Snapshot** (`insta 1` via `gadgetron-testing`): UT-err-1/8 (에러 Display 안정성), UT-ui-1/5 (WebSocket JSON 호환). 스냅샷 위치: `crates/gadgetron-core/tests/snapshots/`.
- **`compile_fail` doctest**: `#[non_exhaustive]` enum 외부 match 에서 `_` arm 누락 시 컴파일 실패 검증.
- **rustdoc doctest**: UT-prov-1 (A3) `LlmProvider` trait 예시 — `cargo test --doc` 경로.

#### 4.2.1 `testing-support` feature flag (A2 MUST — Q-5 closure)

`#[non_exhaustive]` 때문에 외부 crate proptest 가 enum 전체 variant 를 cover 하려면 내부 API 접근 필수. opt-in feature 로만 `all_variants_for_test()` 헬퍼를 노출, 프로덕션 바이너리에는 CI 가드로 차단.

```toml
# crates/gadgetron-core/Cargo.toml
[features]
default = []
testing-support = []          # dev-dep 전용

[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio", "html_reports"] }
```

```rust
// error.rs — 19 variant (기존 12 + D-13 5 + D-20260411-08 1 + D-20260411-13 1)
impl GadgetronError {
    #[cfg(feature = "testing-support")]
    pub fn all_variants_for_test() -> Vec<Self> {
        use crate::error::DatabaseErrorKind;
        vec![
            Self::Provider("t".into()), Self::Router("t".into()),
            Self::Scheduler("t".into()), Self::Node("t".into()), Self::Config("t".into()),
            Self::NoProvider("m".into()), Self::AllProvidersFailed("m".into()),
            Self::InsufficientResources("v".into()),
            Self::ModelNotFound("m".into()), Self::NodeNotFound("n".into()),
            Self::HealthCheckFailed { provider: "p".into(), reason: "r".into() },
            Self::Timeout(30_000),
            // D-13
            Self::Billing("t".into()), Self::TenantNotFound("t".into()),
            Self::QuotaExceeded("t".into()), Self::DownloadFailed("m".into()),
            Self::HotSwapFailed("m".into()),
            // D-20260411-08
            Self::StreamInterrupted { reason: "client_abort".into() },
            // D-20260411-13 — 한 variant 만 선언, DatabaseErrorKind 7 종은 별도 헬퍼로 노출
            Self::Database { kind: DatabaseErrorKind::PoolTimeout, message: "pool".into() },
        ]
    }
}

#[cfg(feature = "testing-support")]
impl DatabaseErrorKind {
    /// D-20260411-13 + Q-5 옵션 B: 7 variant 모두 cover (proptest strategy 용).
    pub fn all_variants_for_test() -> &'static [Self] {
        &[Self::RowNotFound, Self::PoolTimeout, Self::ConnectionFailed,
          Self::QueryFailed, Self::MigrationFailed, Self::Constraint, Self::Other]
    }
}
// ModelState (7 variant), DownloadState (5), EvictionPolicy (4: Lru/Priority/CostBased/WeightedLru{0.3}),
// InferenceEngine (5: VLLM/SGLang/LlamaCpp/Ollama/Tgi), Quantization (6: Fp32/Fp16/Bf16/Q8_0/Q4_K_M/Q3_K_M),
// ComputePartitioning (4: Dedicated/Mps/TimeSlicing/Mig{"1g.5gb"}) — 동일 패턴으로 각 5-10 line 선언.
```

**`gadgetron-testing` 활성화 + proptest strategy**:

```toml
# crates/gadgetron-testing/Cargo.toml
[dev-dependencies]
gadgetron-core = { path = "../gadgetron-core", features = ["testing-support"] }
```

```rust
use gadgetron_core::error::GadgetronError;
use proptest::prelude::*;
fn arb_error() -> impl Strategy<Value = GadgetronError> {
    let v = GadgetronError::all_variants_for_test();
    (0..v.len()).prop_map(move |i| v[i].clone())
}
proptest! {
    #[test]
    fn error_variant_serde_round_trip(err in arb_error()) {
        let json = serde_json::to_string(&err).unwrap();
        let back: GadgetronError = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(err.to_string(), back.to_string());
    }
}
```

**CI 가드**: `cargo test --features testing-support` (gadgetron-testing 경유, variant exposure) / `cargo test` (default, 헬퍼 부재 확인) / `devops/ci/check-dev-dep.sh` — `cargo tree -p gadgetron-cli --no-default-features` 결과에 `testing-support` feature 활성화 없음을 grep, 실패 시 CI red.

**Q-5 🟢 Closed (2026-04-12)** — Phase 1 옵션 B (feature flag) 확정, 8 타입 적용 (`GadgetronError` **19**, `DatabaseErrorKind` 7 (D-20260411-13), `ModelState` 7, `DownloadState` 5, `EvictionPolicy` 4, `InferenceEngine` 5, `Quantization` 6, `ComputePartitioning` 4).

### 4.3 커버리지 목표

- **Line coverage**: ≥ 90% (core는 로직 거의 없음, 도달 가능)
- **Branch coverage**: ≥ 85%
- **`#[non_exhaustive]` enum**: 모든 variant 직접 테스트
- 측정: `cargo llvm-cov` — CI에 통합.

### 4.4 Performance Benchmarking (A1 MUST — P99 < 1ms SLO 검증)

Round 2 체크리스트 "성능 검증" 항목 해소. Gadgetron 핵심 SLO **P99 < 1ms overhead** 가 core 타입 hot path 에서 유지되는지 `criterion 0.5` 로 측정. 본 벤치는 `crates/gadgetron-core/benches/` 에 위치하며, `crates/gadgetron-testing/benches/` (cross-crate scenario) 와 **분리** — core 는 micro-benchmark, testing 은 e2e 시나리오.

#### 4.4.1 벤치마크 매트릭스

| ID | 대상 | 입력 | P99 목표 | 이유 |
|----|------|------|:--------:|-----|
| **BM-alloc** | `RangePortAllocator::allocate(hint: Option<u16>=Some(8000))` | 범위 8000-8099 | **< 100 μs** | 모델 배포당 1회지만 Mutex+BTreeSet hot path, 전체 overhead 10% 이하 |
| **BM-eviction-score** | `EvictionPolicy::score_stub()` (Phase 1 fieldless stub, `match self { _ => 0.0 }`) | 4 variant (Lru/Priority/CostBased/WeightedLru{0.3}) | **< 10 μs** | Phase 2 `CostBased { threshold_usd_per_hour }` 실구현 대비 baseline 확보 |
| **BM-vram-estimate** | `estimate_vram_mb(70_000_000_000, Quantization::Q4_K_M)` | 0B~175B × 6 양자화 | **< 5 μs** | eviction/placement 루프마다 호출, 순수 산술 → 저예산 |

Phase 1 `score_stub()` 은 match dispatch 비용만 측정. Phase 2 `CostBased` 실데이터 연결 시 동일 파일에 신규 variant 추가.

#### 4.4.2 파일 구조 + 예시

```
crates/gadgetron-core/benches/
├── port_allocator.rs         # BM-alloc
├── eviction_score.rs         # BM-eviction-score
└── vram_estimate.rs          # BM-vram-estimate
```

`Cargo.toml` 은 3 벤치를 `[[bench]]` (harness=false) 로 등록 + dev-dep `criterion 0.5 features=["async_tokio","html_reports"]`.

```rust
// benches/port_allocator.rs — 대표 예시
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use gadgetron_core::prelude::*;
fn bench_allocate(c: &mut Criterion) {
    let alloc = RangePortAllocator::new(8000, 8099);
    c.bench_function("RangePortAllocator::allocate_hint_8000", |b| b.iter(|| {
        let p = alloc.allocate(black_box(Some(8000))).unwrap();
        alloc.release(p);
    }));
}
criterion_group!(benches, bench_allocate);
criterion_main!(benches);
```

#### 4.4.3 실행 + Regression Gating

```bash
cargo bench -p gadgetron-core --bench port_allocator
cargo bench -p gadgetron-core --bench eviction_score
cargo bench -p gadgetron-core --bench vram_estimate
```

- **Baseline**: `target/criterion/` (local) + `.github/criterion-baselines/main/` (CI 아티팩트). `critcmp baseline-main current` variance report, HTML PR comment 링크.
- **Phase 1**: non-blocking (warning only). GitHub Action `::warning::` annotate, PR 머지 허용. devops-sre-lead 주간 트렌드 리뷰.
- **Phase 2**: blocking. `current.median > baseline.median * 1.15` (15% 회귀) 시 CI red, override 는 architect 승인 필요.
- **근거**: Phase 1 CI runner noise 10-15% 수준. Phase 2 진입 시 self-hosted runner + warm-up 2회 + CPU governor pin 후 gating 활성화. 100μs/10μs/5μs 목표는 10-100배 마진으로 noise 흡수.
- **경계**: criterion 은 dev-dep only — D-12 leaf 원칙 유지, `testing-support` feature 와 독립, `cargo tree -p gadgetron-cli` 에 링크되지 않음.

### 4.5 범위 외 테스트

- `ApiKey` 파싱 — `gadgetron-xaas` 소관. 본 문서는 `TenantNotFound` error variant만 커버.
- DB round-trip — `gadgetron-xaas` 소관.
- HTTP error 변환 (GadgetronError → StatusCode) — `gadgetron-gateway` 소관.
- WebSocket fan-out backpressure — `gadgetron-gateway` 소관 (core는 타입만).

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

| ID | 크레이트 | 목적 |
|---------|---------|-----|
| IT-1 | core + 하위 9 crate | `cargo check --workspace` 컴파일 가드 |
| IT-2 | core + scheduler + node | `ModelState::Running{port,pid}` wiring, Child::id() → kill_tree |
| IT-3 | core + xaas | `GadgetronError::TenantNotFound` → auth middleware 전파 |
| IT-4 | core + provider + router | `LlmProvider::chat` → `Provider` → fallback. `chat_stream` 중단 → `StreamInterrupted` → tower::retry 1회 (D-08). **주석 (A4)**: Gemini polling mock 은 Track 2 A3 (D-20260411-02 Week 5-7 `SseToChunkNormalizer`/`PollingToEventAdapter` refactor 후) 에 추가 예정, Phase 1 초기 IT-4 는 OpenAI/Anthropic/vLLM/SGLang/Ollama 5 adapter 로 진입. |
| IT-5 | core + node | `RangePortAllocator` concurrent alloc — 동시 start_model 포트 충돌 없음 |
| IT-6 | core + cli | TOML config round-trip — `parallelism`, `eviction` 파싱/직렬화 동치 |
| IT-7 | core + scheduler | `WeightedLru` 스코어 e2e. `CostBased` fallback-to-LRU 경로 |
| IT-8 | core + node | `NumaTopology` 수집 → scheduler push → `GpuNumaMapping` 배치 결정 (mock fixture) |
| IT-9 | core + gateway + tui (D-07) | `WsMessage` 4 variant broadcast + deserialize 호환 |
| IT-10 | core + gateway + xaas (D-06) | routing f64 → `compute_cost_cents` i64 변환 — 단 1지점 |
| IT-11 | core + provider (HIGH #8) | `is_live` vs `is_ready(model)` 분리 — vLLM `/health`200 & `/v1/models` 모델 미포함 → `is_ready = Ok(false)`, scheduler `Loading` 유지 |
| IT-12 | core + gateway (D-08) | SSE client abort → `StreamInterrupted{reason="client_abort"}` → 499 + Prometheus `error_kind="stream_interrupted"` |
| IT-13 | core + xaas + gateway (D-20260411-13) | `sqlx::Error::PoolTimedOut` → `xaas::sqlx_to_gadgetron` → `Database{kind:PoolTimeout}` → gateway 503 + Prometheus `error_kind="db_pool_timeout"`. core 의 `cargo tree -p gadgetron-core` 에 sqlx 부재 verify (leaf crate guard) |

### 5.2 테스트 환경

- **Home**: `crates/gadgetron-testing/tests/core_integration.rs` (D-20260411-05).
- **외부 의존**: 없음. 본 문서 통합 테스트는 전부 in-process (GPU/NVML mock). PostgreSQL 은 xaas integration 소관.
- **Fixtures**: `FixtureBuilder::numa_topology_2_gpu_1_numa()`, `port_allocator_range(8000, 8099)`, `model_state_running(port, pid)`, `gpu_metrics_sample(node_id, gpu_index)` (D-07), `cluster_health_healthy()` (D-07), `ws_message_batch(n)` (D-07).

### 5.3 회귀 방지

이 테스트들이 실패해야 하는 변경:

1. core 타입 필드 삭제 (예: `ParallelismConfig::ep_size`) → IT-1
2. `ModelState::Running { pid }` 제거 → IT-2 (D-10 회귀)
3. `GadgetronError::{TenantNotFound, StreamInterrupted}` 제거 → IT-3/IT-4/IT-12 (D-13/D-08 회귀)
4. `RangePortAllocator` 동시성 버그 → IT-5, UT-port-6
5. TOML 스키마 변경 (`numa_bind` → `numa_aware: bool`) → IT-6 (C-2 회귀)
6. `EvictionPolicy` 3 variant 축소 → IT-7 (C-1 회귀)
7. core 에 `reqwest` 의존성 추가 → IT-1 cycle detection (D-12 회귀)
8. `ui::GpuMetrics` 필드 rename → IT-9 + UT-ui-2 스냅샷 (D-07 회귀)
9. `LlmProvider::is_ready(model)` 제거 → IT-11 (HIGH #8 회귀)
10. `CostBased` 를 i64 cents 로 되돌림 → IT-10 (D-06 회귀)
11. `GadgetronError::Database` 또는 `DatabaseErrorKind` 제거 → IT-13 + UT-err-10 (D-20260411-13 회귀)
12. `gadgetron-core/Cargo.toml` 에 `sqlx` 의존성 추가 → IT-13 leaf crate guard (`cargo tree -p gadgetron-core | grep -q sqlx && exit 1`)

---

## 6. Phase 구분

| 항목 | Phase | 근거 |
|------|:-----:|-----|
| `GadgetronError::{Billing,TenantNotFound,QuotaExceeded,DownloadFailed}` | P1 | D-13 + D-20260411-03 |
| `GadgetronError::HotSwapFailed` | P1 정의만 / P2 사용 | D-13, D-10 |
| **`GadgetronError::StreamInterrupted`** | **P1** | **D-20260411-08** |
| **`GadgetronError::Database` + `DatabaseErrorKind`** | **P1** | **D-20260411-13 (leaf crate, sqlx 의존성 0)** |
| `ParallelismConfig` (D-1) | P1 | D-20260411-04 |
| `EvictionPolicy::{Lru, Priority, WeightedLru}` | P1 | D-2 |
| `EvictionPolicy::CostBased` (fieldless stub) | P1 stub / P2 `threshold_usd_per_hour: f64` | D-2, D-06 |
| `NumaTopology`/`NumaNode`/`GpuNumaMapping`/`NvLinkInfo` | P1 | D-3 + D-20260411-04 |
| `ComputePartitioning::{Dedicated,Mps,TimeSlicing}` | P1 | D-20260411-04 |
| `ComputePartitioning::Mig` | P1 정적만 | D-20260411-04 |
| `ModelState::Running { pid }` | P1 | D-10 |
| `ModelState::Draining/HotSwapping` | P2 | D-10 주석 |
| `PortAllocator` + `RangePortAllocator` | P1 | M-3 |
| `DownloadState` | P1 | C-5 |
| **`LlmProvider::is_live` + `is_ready(model)`** | **P1** | **Round 2 HIGH #8** |
| **`ui::{GpuMetrics,ModelStatus,RequestEntry,ClusterHealth,Alert,WsMessage}`** | **P1** | **D-20260411-07** |
| `#[non_exhaustive]` 전면 도입 | P1 | Round 2 §2.2 확장성 C |
| core prelude | P1 | DX |

**Phase 2 후보** (본 문서 범위 외): `ModelState::Draining/HotSwapping`, `HotSwapManager`/`TrafficRouter`, multi-vendor GPU 추상화(AMD/Intel), `NumaTopology::refresh()`, `EvictionPolicy::CostBased { threshold_usd_per_hour: f64 }` 실연결, `WsMessage` 신규 variant (`NodeJoined`/`TenantQuotaUpdate` 등), `gadgetron-web` 본 구현.

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 결론 | 상태 |
|----|------|------|------|
| Q-1 | routing `f64` vs 과금 `i64 cents` 경계 | A. routing advisory f64 유지, billing xaas i64 cents, 변환 1지점 | 🟢 **D-20260411-06 closed** |
| Q-2 | `NumaTopology` Phase 2 mutation API (dynamic MIG reconfig) | B. `Arc<NumaTopology>` + `replace(new)` (hot path 최적, scheduler versioning) | 🟡 P2 연기 |
| Q-3 | 공유 UI 타입 위치 (core vs 별도 crate) | A. core `ui` 모듈 | 🟢 **D-20260411-07 closed** (§2.1.9) |
| Q-4 | `RangePortAllocator` `Mutex` poisoning 정책 | A. panic + heartbeat isolation (A5 rustdoc 반영) | 🟢 결론 |
| Q-5 | `#[non_exhaustive]` × proptest 상호작용 | B. core `testing-support = []` feature flag + 7 타입 `all_variants_for_test()` (§4.2.1) | 🟢 **Closed (2026-04-12) — Round 2 A2** |
| Q-6 | `Downloading::progress` f32 vs u16 basis-points | A. f32 유지 (D-10), UI는 `f32 * 10000.0` 변환 | 🟢 결론 |
| Q-7 | `HotSwapFailed` Phase 1 정의만 dead-code 경고 | C. `gadgetron-testing` prop-test 에서 variant touch | 🟢 결론 |
| Q-8 | `WsMessage::GpuMetrics(Vec<_>)` 단일 GPU 시에도 `Vec<1>` 인가 | A. gateway 100ms window aggregate, 단일도 `Vec<1>`, P2 분리 가능(`non_exhaustive`) | 🟢 결론 |
| Q-9 | `LlmProvider::is_ready(model)` `&str` vs `&ModelId` newtype | A. Phase 1 `&str` 유지, P2 `ModelId` newtype RFC | 🟡 P2 후보 |

---

## 8. `#[non_exhaustive]` 감사

Round 2 §2.2 확장성 평가 "C등급" 지적사항 해소. 외부 crate match 불편을 감수하고 breaking-change 없는 variant 추가를 확보한다.

| 타입 (위치) | `#[non_exhaustive]` | 근거 |
|------|:---:|-----|
| `GadgetronError` (error.rs) | **Yes** | D-13 5개 + D-20260411-08 1개 + D-20260411-13 1개 = 19 variant, Phase 2 추가 예상 |
| **`DatabaseErrorKind` (error.rs)** | **Yes** | **D-20260411-13: 7 variant (RowNotFound/PoolTimeout/ConnectionFailed/QueryFailed/MigrationFailed/Constraint/Other), Phase 2 신규 sqlx error 분류 forward-compat** |
| `RoutingStrategy` (routing.rs) | **Yes** | Semantic/ML routing P2 로드맵 |
| `EvictionPolicy` (routing.rs) | **Yes** | D-2 확장 전제, `CostBased` P2 필드 추가 |
| `InferenceEngine` (model.rs) | **Yes** | TensorRT-LLM, MLC-LLM, ONNX Runtime 확장 |
| `ModelState` (model.rs) | **Yes** | D-10 Draining/HotSwapping P2 |
| `Quantization` (model.rs) | **Yes** | FP4, AWQ 등 신규 포맷 |
| `DownloadState` (model.rs) | **Yes** | Mirrored/Seeded 확장 가능 |
| `ComputePartitioning` (node.rs) | **Yes** | AMD MxGPU, Intel Flex 벤더 확장 |
| `ContentPart` (message.rs) | **Yes** | Audio/Video 멀티모달 P2 |
| `ProviderConfig` (config.rs) | **Yes** | 새 provider breaking-change 방지 |
| **`ui::WsMessage` (ui.rs)** | **Yes** | **D-20260411-07: NodeJoined/TenantQuotaUpdate 등 P2** |
| **`ui::ModelStatusKind`** | **Yes** | **HotSwapping/QueuedForRestart 등 P2** |
| **`ui::HealthStatus`** | **Yes** | **Maintenance/Partial 등 P2** |
| **`ui::AlertSeverity`** | **Yes** | **커스텀 레벨 P2** |
| `Role` / `Content` (message.rs) | **No** | OpenAI 표준 고정 / `Text`/`Parts` serde-untagged 불변 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-11 — @gpu-scheduler-lead + @inference-engine-lead
**결론**: ⚠️ Conditional Pass

**체크리스트 (§1)**: 인터페이스 계약 / 크레이트 경계 (D-12) / 타입 중복 (C-1~C-5 해소) / 에러 반환 / 동시성 / 의존성 방향 / Phase 태그 ✅, 레거시 결정 준수 — D-20260411-06/07 sync 지연 ❌ (A1, A2 해결).

**핵심 발견**: (1) D-07 UI types 미반영 — Track 2 의존성 block, (2) D-06 f64/i64 경계 미반영, (3) `LlmProvider::is_ready` 미정의 (Round 2 HIGH #8).

**Action Items**: A1~A4 blocking, A5~A9 non-blocking (상세: `docs/reviews/round1-week1-results.md §Track 1`).

### Round 1 Retry — 2026-04-11 — @chief-architect
**결론**: ✅ All blocking action items resolved

- A1 ✅ §2.1.9 Shared UI Types (GpuMetrics, ModelStatus, RequestEntry, ClusterHealth, WsMessage)
- A2 ✅ §2.1.4 EvictionPolicy::CostBased D-20260411-06 경계 주석
- A3 ✅ LlmProvider `is_ready(model)` 추가 (HIGH #8 해소)
- A4 ✅ GadgetronError::StreamInterrupted (D-20260411-08)
- A5~A9 ✅ 문서 내 해당 섹션

**다음 라운드**: Round 2 (qa-test-architect testability review).

### Round 2 — 2026-04-12 — @qa-test-architect
**결론**: ✅ Pass (with 2 minor open items)

**체크리스트 결과 (§2)**:
- [x] 단위 테스트 범위 — 22 UT ID 모두 cover
- [x] mock 가능성 — gadgetron-testing 선설계 조화
- [x] 결정론 — wall-clock 제로, loom 경로, proptest seed
- [x] 통합 시나리오 — 12 IT cross-crate
- [x] CI 재현성 — insta + proptest + llvm-cov
- [ ] 성능 검증 — ⚠️ criterion bench 경로 미명시 (A1으로 해결)
- [x] 회귀 테스트 — §5.3 10개 회귀 케이스
- [x] 테스트 데이터 — fixture 위치 + builder

**핵심 발견**: 22 UT ID + 12 IT + 10 regression 완벽. D-06/07/08 반영. 구체 gap은 (1) criterion bench P99 < 1ms 명시 (2) `testing-support` feature flag 구체화 2개.

### Round 2 Retry — 2026-04-12 — @chief-architect
**Resolutions**:
- A1 ✅ §4.4 Performance Benchmarking 신설: BM-alloc (P99 <100μs) / BM-eviction-score (P99 <10μs) / BM-vram-estimate (P99 <5μs) + Cargo.toml `[[bench]]` 3개 + critcmp baseline/main + Phase 1 non-blocking / Phase 2 +15% regression gating
- A2 ✅ §4.2.1 `testing-support` feature flag + `all_variants_for_test()` 7 타입 전체 (GadgetronError 18 variant, ModelState 7, DownloadState, EvictionPolicy, InferenceEngine, Quantization, ComputePartitioning) + proptest strategy + CI guard 스크립트 — **Q-5 🟢 closed**
- A3 ✅ §2.1.10 UT-prov-1 rustdoc doctest 추가 (FakeProvider 10-line 예시, `cargo test --doc` 경로)
- A4 ✅ §5.1 IT-4 Gemini polling 주석 (D-20260411-02 Week 5-7 normalizer refactor 후 추가)
- A6 ✅ §4.1 UT-mod-3 quantization 매트릭스 proptest (6 variant × 6 모델 크기, `prop_oneof!`)
- A7 ✅ §2.1.9 TUI serde format 명확화 (Phase 1 JSON only, TUI in-process, Web UI msgpack 여부 Phase 2 결정)

**다음 라운드**: Round 3 (chief-architect self-review 회피) — @gateway-router-lead + @xaas-platform-lead 공동 진행 가능.

### Round 3 — 2026-04-12 — @gateway-router-lead + @xaas-platform-lead (joint)
**결론**:
- @gateway-router-lead: ✅ Approved (10/10 §3 Pass, optional doc 개선만 — D-1~D-6)
- @xaas-platform-lead: ⚠️ Conditional (D-12 sync + Track 3 cross-ref 2 blocking)

**체크리스트 (§3)**: 10/10 Pass + 2 Partial (관측성, 문서화 개선 권고)

**핵심 발견**:
1. `Arc<dyn LlmProvider>` 선택 근거 문서화 → D-20260411-11 해결
2. `GadgetronError` 의 `sqlx::Error` context 손실 → D-20260411-13 해결 (D-13 확장)
3. D-12 크레이트 경계표 `gadgetron-xaas` 행 업데이트 필요 (Scope/ApiKey/Tenant/Quota/Audit/PgKeyValidator/sqlx_to_gadgetron 등)
4. HTTP Status Mapping Track 3 cross-reference 필요

### Round 3 Retry — 2026-04-12 — @chief-architect
**Resolutions**:
- A1 ✅ §2.1.10 Dynamic Dispatch Rationale 서브섹션 (D-20260411-11 인용, 10^8 성능 math, 4단 근거: I/O-bound, config-driven 6-provider, workspace-wide refactor 비용, trait object 표준 패턴)
- A2 ✅ §2.1.2 `DatabaseErrorKind` enum (`#[non_exhaustive]`, 7 variant) + `Database { kind, message }` variant (D-20260411-13). retry policy 표 6행 추가 + Prometheus 라벨 (`db_pool_timeout`/`db_row_not_found`/`db_connection`/`db_constraint`/`db_migration`/`db_query`). **leaf crate 보존**: `gadgetron-core` 에 `sqlx` 의존성 0, `From<sqlx::Error>` 미정의, consumer 가 helper fn 으로 매핑 (orphan rule 회피).
- A3 ✅ §3 D-12 크레이트 경계표 업데이트 노트 — `gadgetron-xaas` 행에 Scope/ApiKey/ValidatedKey/KeyValidator/KeyKind/TenantContext/Tenant/TenantSpec/TenantRegistry/QuotaConfig/QuotaEnforcer/QuotaToken/AuditEntry/AuditWriter/PgKeyValidator(moka)/sqlx_to_gadgetron 추가 명시
- A4 ✅ §3.3 gateway-router-lead 계약에 Track 3 HTTP Status Mapping cross-reference 표 6행 (`Database` 6 kind → 503/404/503/409/500/500). `#[non_exhaustive]` compile_fail 가드로 Track 1 ↔ Track 3 동기화 강제.

**보강**:
- §2.1.2 retry policy 표 19 variant 모두 cover (`Database{*}` 6행)
- §3.2 consumer 매트릭스 + §3.4 D-12 표에 `Database` / `sqlx_to_gadgetron` 행 추가
- §4.1 UT-err-10/11 신규 (Database serde + DatabaseErrorKind variant 길이)
- §4.2.1 `all_variants_for_test()` 19 variant + `DatabaseErrorKind::all_variants_for_test()` 7 variant (Q-5 closure 8 타입 확장)
- §5.1 IT-13 (`sqlx::Error::PoolTimedOut → 503`) + leaf crate `cargo tree -p gadgetron-core` guard
- §5.3 회귀 케이스 11/12 (`Database` 제거 / sqlx 의존성 추가)
- §6 Phase 표에 `Database` P1
- §8 `#[non_exhaustive]` 감사 표에 `DatabaseErrorKind` 행

**최종 상태**: ✅ **Approved** — 구현 착수 가능. **19 variants** (12 original + 5 D-13 + 1 D-08 + 1 D-20260411-13 = 19). `gadgetron-core` 에 `sqlx`/`reqwest`/`nvml-wrapper` 의존성 0 유지 (D-12 leaf 원칙 보존).

### 최종 승인 — 2026-04-12 — PM (D-20260411-11/13 적용 확인 후)
