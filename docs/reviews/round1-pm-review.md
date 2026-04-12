# Gadgetron 설계 문서 교차 리뷰 — Round 1 결과

> PM 주관: 2026-04-11
> 참여: Architect, Infra Lead, Serving Lead, API Lead, Platform Lead, UX Lead, Ops Lead

---

## 1. 크리티컬 이슈 (Critical Issues)

### C-1: `EvictionPolicy` 타입 중복 정의 및 불일치

**문서**: gpu-resource-manager.md vs model-serving.md

```rust
// gpu-resource-manager.md
pub enum EvictionPolicy {
    Lru,
    Priority,
    CostBased,
    WeightedLru { priority_weight: f32 },
}

// model-serving.md
pub enum EvictionPolicy {
    Lru,
    Priority { default_priority: i32 },
    // CostBased, WeightedLru 없음
}
```

**해결**: `EvictionPolicy`는 `gadgetron-core`에 단일 정의. gpu-resource-manager.md의 4-variant 버전을 기준으로 통일. `priority_weight: f32` 유지, `default_priority: i32`는 제거 (가중치가 더 유연함).

---

### C-2: `ParallelismConfig` 필드명 불일치

**문서**: gpu-resource-manager.md vs model-serving.md

```rust
// gpu-resource-manager.md
pub struct ParallelismConfig {
    pub tp_size: u32,    // ← _size 접미사
    pub pp_size: u32,
    pub ep_size: u32,
    pub dp_size: u32,
    pub numa_bind: Option<u32>,  // ← Option<u32>
}

// model-serving.md
pub struct ParallelismConfig {
    pub tp: u32,         // ← 접미사 없음
    pub pp: u32,
    pub ep: u32,
    pub dp: u32,
    pub numa_aware: bool,  // ← bool (다른 의미!)
}
```

**해결**: `_size` 접미사 버전으로 통일 (vLLM/SGLang CLI 인자와 일관성: `--tensor-parallel-size`). `numa_bind: Option<u32>` 유지 (바인딩할 NUMA 노드 번호가 필요함).

---

### C-3: `NumaTopology` 중복 정의 및 필드 불일치

**문서**: gpu-resource-manager.md vs model-serving.md

```rust
// gpu-resource-manager.md — 필드명: gpus, 구조체: GpuNumaMapping
pub struct NumaTopology {
    pub nodes: Vec<NumaNode>,
    pub gpus: Vec<GpuNumaMapping>,
}

// model-serving.md — 필드명: gpu_mappings, 구조체: GpuMapping (다른 이름)
pub struct NumaTopology {
    pub nodes: Vec<NumaNode>,
    pub gpu_mappings: Vec<GpuMapping>,
}
```

**해결**: `gadgetron-core/src/node.rs`에 `NumaTopology`를 정의. gpu-resource-manager.md의 `GpuNumaMapping` 이름으로 통일.

---

### C-4: `ModelState` 확장 — 기존 코드와 불일치

**기존 코드** (`gadgetron-core/src/model.rs`):
```rust
pub enum ModelState {
    NotDownloaded, Downloading{progress: f32}, Registered, Loading,
    Running{port: u16}, Unloading, Failed(String),
}
```

**model-serving.md** 도입:
```rust
pub enum ModelState {
    NotDownloaded, Downloading{progress: f64}, Registered, Loading,
    Running{port: u16, pid: u32}, Unloading, Failed(String),
    // 추가: Draining, HotSwapping
}
```

**해결**: `progress: f32` 유지 (기존 코드와 일관성). `pid: u32` 추가는 승인 (프로세스 관리에 필수). `Draining`, `HotSwapping` variant는 Phase 2에서 추가 (Phase 1에서는 주석으로 표시).

---

### C-5: `DownloadState` / `DownloadStatus` 중복 정의

**문서**: model-serving.md (`DownloadStatus`) vs xaas-platform.md (`DownloadState`)

두 문서가 다운로드 상태를 서로 다른 이름과 필드로 정의.

**해결**: `gadgetron-core`에 `DownloadState`로 통일 (xaas-platform의 네이밍 채택). `DownloadStatus`는 제거.

---

## 2. 일관성 이슈 (Consistency Issues)

### I-1: API 엔드포인트 중복 정의

여러 문서가 같은 API를 다르게 정의:
- **gateway-routing.md**: `POST /api/v1/models/deploy`
- **model-serving.md**: `POST /api/v1/serving/deploy`
- **xaas-platform.md**: `POST /api/v1/modelaas/serve`

**해결**: gateway-routing.md의 경로를 기준으로 통일:
- `POST /api/v1/models/deploy` — 모델 배치
- `DELETE /api/v1/models/:id` — 모델 언로드
- `GET /api/v1/models/status` — 모델 상태

XaaS 전용 엔드포인트는 `/api/v1/xaas/` prefix 사용:
- `POST /api/v1/xaas/gpu/allocate` — GPUaaS
- `POST /api/v1/xaas/model/serve` — ModelaaS
- `POST /api/v1/xaas/agent/create` — AgentaaS

---

### I-2: `Scheduler` vs `ModelScheduler` vs `EnhancedScheduler`

3개의 다른 스케줄러 이름이 등장:
- **기존 코드**: `gadgetron-scheduler::Scheduler`
- **model-serving.md**: `ModelScheduler`
- **gpu-resource-manager.md**: `EnhancedScheduler`

**해결**: 기존 `Scheduler`를 확장하는 방향으로 통일. 새 필드는 `SchedulerConfig`로 분리하여 `Scheduler::new(config)`로 주입.

---

### I-3: `HealthStatus` / `HealthCheck` 중복

- model-serving.md: `HealthStatus` enum, `HealthChecker` struct
- xaas-platform.md: `HealthStatus` enum (다른 variants), `HealthCheck` struct (다른 필드)

**해결**: `gadgetron-core`에 `HealthStatus` 단일 정의. 모델 헬스체크와 노드 헬스체크를 `ModelHealthStatus` / `NodeHealthStatus`로 구분.

---

### I-4: `ProcessManager` vs `NodeAgent` 책임 중복

model-serving.md의 `ProcessManager`가 기존 `NodeAgent`의 `start_model()`/`stop_model()`과 책임이 겹침.

**해결**: `ProcessManager`는 `NodeAgent` 내부 컴포넌트로 배치. `NodeAgent`가 공개 API를 유지하고, `ProcessManager`는 프로세스 생명주기 관리의 내부 구현.

---

## 3. 누락 상세 (Missing Details)

### M-1: Gemini 어댑터 누락

모든 문서에서 Gemini 어댑터가 언급되지 않음. 기존 `ProviderConfig::Gemini`가 코드에 존재하나, `gadgetron-provider`에 구현이 없음.

**해결**: model-serving.md에 Phase 1 Gemini 어댑터 구현 계획 추가. gateway-routing.md의 프로토콜 변환 표에 Gemini 행 추가.

---

### M-2: WebSocket 엔드포인트 정의 불충분

dashboard.md가 WebSocket을 언급하지만, gateway-routing.md에 WebSocket 엔드포인트 사양이 부족함.

**해결**: gateway-routing.md에 `/api/v1/ws/metrics` WebSocket 엔드포인트 사양 추가 (메시지 타입, 포맷, 인증).

---

### M-3: 동적 포트 할당 미구현

기존 코드에 `// TODO: dynamic port assignment` 주석이 있으나, model-serving.md도 구체적 구현이 부족함.

**해결**: model-serving.md에 `PortAllocator` 트레이트 추가:
```rust
pub trait PortAllocator: Send + Sync {
    fn allocate(&self, hint: Option<u16>) -> Result<u16>;
    fn release(&self, port: u16);
    fn is_available(&self, port: u16) -> bool;
}
```

---

### M-4: 에러 처리 표준화 부족

기존 `GadgetronError`에 XaaS 관련 variant가 없음.

**해결**: `gadgetron-core/src/error.rs`에 추가:
```rust
pub enum GadgetronError {
    // ... existing variants ...
    Billing(String),          // 요금/할당량 관련
    TenantNotFound(String),   // 테넌트 관련
    QuotaExceeded(String),    // 할당량 초과
    DownloadFailed(String),   // 모델 다운로드 실패
    HotSwapFailed(String),    // 핫스왑 실패
}
```

---

## 4. 크로스 도큐먼트 정렬 이슈

### X-1: 크레이트 경계 불명확

여러 문서가 `gadgetron-scheduler`에 새 타입을 추가하지만, 어느 크레이트에 배치해야 하는지 불명확:
- `VramEstimator` → model-serving.md에서 정의, 위치 불명확
- `HotSwapManager` → model-serving.md에서 정의, scheduler vs provider?
- `MigManager` → gpu-resource-manager.md에서 정의, node vs core?

**해결**:
| 타입 | 크레이트 | 이유 |
|------|----------|------|
| `VramEstimator` | gadgetron-core | 스케줄러와 프로바이더 모두 사용 |
| `HotSwapManager` | gadgetron-scheduler | 배치/언로드 로직과 밀접 |
| `MigManager` | gadgetron-node | GPU 하드웨어 관리는 노드 에이전트 책임 |
| `ThermalController` | gadgetron-node | GPU 하드웨어 관리 |
| `ParallelismConfig` | gadgetron-core | 스케줄러와 노드 에이전트 모두 사용 |
| `NumaTopology` | gadgetron-core | 다중 크레이트 참조 |
| `ProcessManager` | gadgetron-node | NodeAgent 내부 컴포넌트 |
| `TrafficRouter` | gadgetron-router | 핫스왑 트래픽 라우팅 |
| `BillingConfig` | gadgetron-core | Phase 2, 다중 크레이트 참조 |

---

### X-2: Phase 구분 불명확

여러 문서가 Phase 1과 Phase 2 기능을 명확히 구분하지 않음.

**해결**: 각 문서에 Phase 태그 추가:
- `[P1]` = Phase 1 MVP (8주 내 구현)
- `[P2]` = Phase 2 상용화 (8주 내 구현)
- `[P3]` = Phase 3 플랫폼화 (지속)

---

## 5. Top 5 크리티컬 이슈 (심각도 순)

| 순위 | 이슈 | 심각도 | 영향 |
|------|------|--------|------|
| 1 | C-2: `ParallelismConfig` 필드명 불일치 | **BLOCKER** | 구현 시 컴파일 에러, TP/PP 설정 작동 불가 |
| 2 | C-1: `EvictionPolicy` 타입 불일치 | **BLOCKER** | 스케줄러와 GPU 매니저가 다른 enum 사용 |
| 3 | I-1: API 엔드포인트 중복 정의 | **HIGH** | 클라이언트가 다른 경로로 요청, 혼란 발생 |
| 4 | X-1: 크레이트 경계 불명확 | **HIGH** | 구현 시 순환 의존성 또는 중복 코드 위험 |
| 5 | C-4: `ModelState` 확장 불일치 | **MEDIUM** | 핫스왑/드레이닝 상태 구현 시 충돌 |

---

## Round 2 액션 아이템

각 팀원이 Round 1 피드백을 반영하여 자신의 문서를 수정:

1. **모든 문서**: `ParallelismConfig` → `tp_size/pp_size/ep_size/dp_size` + `numa_bind: Option<u32>` 통일
2. **모든 문서**: `EvictionPolicy` → gpu-resource-manager.md의 4-variant 버전으로 통일
3. **모든 문서**: `NumaTopology`/`GpuNumaMapping` 이름 통일, `gadgetron-core`에 배치
4. **gateway-routing.md**: API 엔드포인트 네임스페이스 정리 (`/api/v1/xaas/` prefix)
5. **model-serving.md**: `ModelState` 확장을 Phase 1/2로 구분, `pid: u32` 추가는 승인
6. **모든 문서**: 중복 타입을 크레이트 경계표에 따라 올바른 위치에 배치
7. **gateway-routing.md**: Gemini 프로토콜 변환 섹션, WebSocket 엔드포인트 추가
8. **gadgetron-core/src/error.rs**: XaaS 관련 에러 variant 추가
9. **model-serving.md**: `PortAllocator` 트레이트 추가
10. **모든 문서**: Phase 태그 `[P1]`/`[P2]`/`[P3]` 추가

---

## 6. Ops Lead 추가 이슈 (gateway-routing + xaas-platform 리뷰)

### O-1: 코드에 없는 엔드포인트들이 문서에 정의됨 [CRITICAL]

**gateway-routing.md**가 정의하는 엔드포인트들이 실제 `server.rs`에 없음:
- `/v1/completions` (legacy) — 미구현
- `/v1/embeddings` — 미구현, 그리고 GET이 아닌 POST여야 함
- `/api/v1/ws/metrics` (WebSocket) — 미구현
- 인증/레이트리밋/가드레일 미들웨어 — 미구현

**결정 필요**: Phase 1에 포함할 최소 엔드포인트 세트를 정의해야 합니다.

### O-2: 우아한 종료(Graceful Shutdown) 미구현 [CRITICAL]

`server.rs`가 `axum::serve(listener, app).await?`로 시작하지만 `with_graceful_shutdown()` 없음.
배포 시 진행 중인 요청이 강제 종료됩니다.

**결정 필요**: Phase 1에 graceful shutdown 추가 여부.

### O-3: 헬스체크가 스텁임 [HIGH]

`routes.rs`의 `health()`가 `{"status": "ok"}`만 반환. 실제 백엔드 연결 상태를 확인하지 않음.
프로덕션에서 모든 백엔드가 다운되어도 "healthy"로 보고.

**결정 필요**: Phase 1 헬스체크 범위 (프로바이더 연결 확인 포함?).

### O-4: SQLite vs PostgreSQL [CRITICAL]

xaas-platform.md가 과금/테넌트/에이전트 데이터를 SQLite에 저장하도록 설계.
멀티테넌트 과금 시스템에 SQLite은 동시 쓰기 한계로 부적합.

**결정 필요**: Phase 1은 SQLite, Phase 2에서 PostgreSQL 마이그레이션?

### O-5: gRPC 정의만 있고 구현 없음 [HIGH]

xaas-platform.md가 gRPC 서비스(GpuService, AgentService 등)를 정의하지만,
코드베이스에 tonic/prost 설정이 없음. "REST + gRPC 듀얼 프로토콜"이라고 하지만 구현 경로 불명확.

**결정 필요**: Phase 1은 REST 전용, Phase 2에서 gRPC 추가?

### O-6: 과금 계산 정확도 [MEDIUM]

`calculate_request_cost`가 `gpu_seconds = latency_ms / 1000.0`를 사용.
배칭된 추론에서는 여러 요청이 GPU 시간을 공유하므로 과청구 발생.
f64 부동소수점은 통화 계산에 부적합.

**결정 필요**: 정밀 과금을 위한 `rust_decimal` 또는 정수 센트 사용?

### O-7: CORS가 permissive [MEDIUM]

`server.rs`가 `CorsLayer::permissive()` 사용. 프로덕션에서 보안 위험.

**결정 필요**: Phase 1에서도 설정 가능한 CORS로 변경?

### O-8: 핫 리로드 미구현 [MEDIUM]

문서는 SIGHUP/파일워처 기반 핫 리로드를 설계하지만, `AppConfig::load()`는 1회 읽기.
설정 변경 시 재시작 필요.

**결정 필요**: Phase 1은 재시작 방식, Phase 2에서 핫 리로드?

### O-9: API 키 접두사 불일치 [LOW]

gateway-routing.md: `gad-k-v1-*` (직접 키), `gad-vk-*` (가상 키)
xaas-platform.md: `gdt_*` (테넌트 키)

**결정 필요**: `gad_` 접두사로 통일.

### O-10: XaaS API 경로 불일치 [HIGH]

gateway-routing.md: `/api/v1/nodes`, `/api/v1/models/deploy`
xaas-platform.md: `/v1/gpu/allocate`, `/v1/models/search`, `/v1/billing/dashboard`

**결정 필요**: 관리 API는 `/api/v1/` prefix, XaaS API는 `/api/v1/xaas/` prefix로 통일.
