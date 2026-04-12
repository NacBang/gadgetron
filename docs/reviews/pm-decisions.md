# Gadgetron 설계 리뷰 — PM 최종 결정 사항

> 일자: 2026-04-11
> 상태: **확정 (APPROVED)**

---

## 승인된 결정

### D-1: ParallelismConfig 필드명 통일
- **결정**: `tp_size/pp_size/ep_size/dp_size` + `numa_bind: Option<u32>` (gpu-resource-manager.md 버전)
- **크레이트**: `gadgetron-core`
- **이유**: vLLM/SGLang CLI 인자(`--tensor-parallel-size`)와 일관성

### D-2: EvictionPolicy 4-variant 통일
- **결정**: `Lru`, `Priority`, `CostBased`, `WeightedLru{priority_weight: f32}`
- **크레이트**: `gadgetron-core`
- **이유**: 더 유연한 스케줄링 정책 지원

### D-3: NumaTopology / GpuNumaMapping 이름 통일
- **결정**: `NumaTopology` (nodes + gpus 필드), `GpuNumaMapping` (gpu-resource-manager.md 버전)
- **크레이트**: `gadgetron-core`
- **이유**: gpu-resource-manager.md의 네이밍이 더 명확

### D-4: 데이터베이스 — Phase 1부터 PostgreSQL
- **결정**: Phase 1부터 **sqlx + PostgreSQL** 사용
- **마이그레이션**: sqlx-cli로 관리
- **기존 `sqlite = "0.36"` 의존성**: 제거하고 sqlx + postgres로 교체
- **이유**: 멀티테넌트 과금 시스템에 SQLite 동시 쓰기 한계 부적합

### D-5: gRPC — Phase 2에서 추가
- **결정**: Phase 1은 REST 전용, Phase 2에서 tonic/prost 기반 gRPC 추가
- **이유**: 코드베이스에 gRPC 인프라 없음, Phase 1 복잡도 최소화

### D-6: Phase 1 MVP 최소 범위
- **추가**: Graceful shutdown (`with_graceful_shutdown`), 헬스체크 구현 (프로바이더 연결 확인), Bearer 인증 미들웨어
- **Phase 2**: 레이트리밋, 가드레일, /v1/completions, /v1/embeddings, WebSocket, 핫 리로드, 멀티테넌트

### D-7: API 경로 네임스페이스
- **결정**:
  - `/v1/` — OpenAI 호환 API (chat/completions, models, embeddings)
  - `/api/v1/` — 관리 API (nodes, models, usage, costs, health)
  - `/api/v1/xaas/` — XaaS API (gpu, model, agent)
- **이유**: 명확한 네임스페이스 분리, OpenAI SDK 호환성 유지

### D-8: 과금 — 정수 센트
- **결정**: 모든 통화 계산에 `i64` 센트 단위 사용 (예: $1.50 = 150 센트)
- **금지**: 과금/결제 관련 `f64` 사용 금지
- **이유**: 부동소수점 반올림 오류 방지

### D-9: ORM — sqlx
- **결정**: sqlx (비동기 네이티브, 컴파일 타임 쿼리 검증)
- **크레이트 추가**: `sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "migrate"] }`
- **마이그레이션**: `sqlx-cli`로 `.sql` 파일 관리

### D-10: ModelState 확장
- **결정**: 기존 7-variant 유지 + `Running{port: u16, pid: u32}` (pid 추가)
- **Phase 2 추가**: `Draining`, `HotSwapping` (주석으로 표시)
- **`Downloading{progress: f32}`**: 기존 코드 유지 (f64 변경 안 함)

### D-11: API 키 접두사 통일
- **결정**: `gad_` 접두사로 통일
  - 직접 키: `gad_live_...`, `gad_test_...`
  - 가상 키: `gad_vk_...`
- **기존**: xaas-platform.md의 `gdt_*` 제거

### D-12: 크레이트 경계 (타입 배치)

| 타입 | 크레이트 | 파일 |
|------|----------|------|
| `ParallelismConfig` | gadgetron-core | src/routing.rs |
| `NumaTopology` | gadgetron-core | src/node.rs |
| `GpuNumaMapping` | gadgetron-core | src/node.rs |
| `NvLinkInfo` | gadgetron-core | src/node.rs |
| `ComputePartitioning` | gadgetron-core | src/node.rs |
| `EvictionPolicy` | gadgetron-core | src/routing.rs |
| `DownloadState` | gadgetron-core | src/model.rs |
| `ModelState` (확장) | gadgetron-core | src/model.rs |
| `PortAllocator` | gadgetron-core | src/model.rs (트레이트) |
| `SchedulerConfig` | gadgetron-scheduler | src/config.rs |
| `HotSwapManager` | gadgetron-scheduler | src/hotswap.rs |
| `TrafficRouter` | gadgetron-router | src/traffic.rs |
| `MigManager` | gadgetron-node | src/mig.rs |
| `ThermalController` | gadgetron-node | src/thermal.rs |
| `ProcessManager` | gadgetron-node | src/process.rs (NodeAgent 내부) |
| `ContainerRuntime` | gadgetron-node | src/container.rs |
| `SlurmIntegration` | gadgetron-node | src/slurm.rs |
| `NvidiaGpuMonitor` | gadgetron-node | src/gpu/nvidia.rs |
| `GpuMonitor` (트레이트) | gadgetron-node | src/gpu/mod.rs |

### D-13: GadgetronError 확장
- **추가 variant**:
  ```rust
  Billing(String),          // 과금/할당량 관련
  TenantNotFound(String),   // 테넌트 관련
  QuotaExceeded(String),    // 할당량 초과
  DownloadFailed(String),   // 모델 다운로드 실패
  HotSwapFailed(String),    // 핫스왑 실패 (Phase 2)
  ```
