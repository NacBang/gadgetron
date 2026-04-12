# Gadgetron — GPU/NPU 리소스 관리자 설계서

> 모듈: `gadgetron-node`, `gadgetron-scheduler`, `gadgetron-core`
> 담당: Infra Lead
> 버전: 0.1.0-draft

---

## 1. GPU/NPU 리소스 모니터링 아키텍처

### 1.1 현재 구현

`gadgetron-node/src/monitor.rs`의 `ResourceMonitor`는 `sysinfo` + `nvml-wrapper`(feature-gated) 기반:

```rust
pub struct ResourceMonitor {
    sys: System,
}

impl ResourceMonitor {
    pub fn collect(&mut self) -> NodeResources {
        // CPU, RAM → sysinfo
        // GPU → NVML (feature = "nvml")
    }
}
```

### 1.2 확장 설계: `GpuMonitor` 트레이트

```rust
/// GPU 벤더 독립 모니터링 인터페이스
#[async_trait]
pub trait GpuMonitor: Send + Sync {
    /// GPU 수 반환
    fn device_count(&self) -> Result<u32>;
    /// 개별 GPU 상세 정보
    fn gpu_info(&self, index: u32) -> Result<GpuDetail>;
    /// 전체 GPU 목록
    fn all_gpus(&self) -> Result<Vec<GpuDetail>>;
    /// NVLink/P2P 연결 상태
    fn p2p_status(&self, gpu_a: u32, gpu_b: u32) -> Result<P2PStatus>;
    /// MIG 인스턴스 목록 (지원 시)
    fn mig_instances(&self, gpu_index: u32) -> Result<Vec<MigInstance>>;
    /// NUMA 노드 정보
    fn numa_info(&self) -> Result<NumaTopology>;
}

/// 확장 GPU 정보
pub struct GpuDetail {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub vram_total_mb: u64,
    pub vram_used_mb: u64,
    pub vram_free_mb: u64,
    pub utilization_pct: f32,
    pub temperature_c: u32,
    pub temperature_slowdown_c: u32,    // 스로틀링 임계값
    pub temperature_shutdown_c: u32,    // 셧다운 임계값
    pub power_draw_w: f32,
    pub power_limit_w: f32,
    pub clock_sm_mhz: u32,             // SM 클럭
    pub clock_mem_mhz: u32,            // 메모리 클럭
    pub pcie_link_gen: u32,            // PCIe 세대
    pub pcie_link_width: u32,          // PCIe 레인 수
    pub pcie_rx_throughput_kb: u64,    // PCIe 수신 처리량
    pub pcie_tx_throughput_kb: u64,    // PCIe 송신 처리량
    pub numa_node: Option<u32>,        // NUMA 노드
    pub mig_mode: Option<MigMode>,     // MIG 모드
    pub fan_speed_pct: Option<f32>,    // 팬 속도
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum P2PStatus {
    NvLink { bandwidth_gbps: u32 },    // NVLink 연결
    Pcie,                              // PCIe를 통한 P2P
    None,                              // P2P 불가
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigMode {
    Disabled,
    Enabled,                           // MIG 활성화
}
```

### 1.3 NVML 구현 (`NvidiaGpuMonitor`)

```rust
pub struct NvidiaGpuMonitor {
    nvml: Nvml,
}

impl NvidiaGpuMonitor {
    pub fn new() -> Result<Self> {
        let nvml = Nvml::init()
            .map_err(|e| GadgetronError::Node(format!("NVML init failed: {}", e)))?;
        Ok(Self { nvml })
    }
}

#[async_trait]
impl GpuMonitor for NvidiaGpuMonitor {
    fn device_count(&self) -> Result<u32> {
        self.nvml.device_count()
            .map_err(|e| GadgetronError::Node(format!("NVML device_count: {}", e)))
    }

    fn gpu_info(&self, index: u32) -> Result<GpuDetail> {
        let device = self.nvml.device_by_index(index)?;
        // ... NVML 필드 매핑
    }

    fn p2p_status(&self, gpu_a: u32, gpu_b: u32) -> Result<P2PStatus> {
        let dev_a = self.nvml.device_by_index(gpu_a)?;
        let dev_b = self.nvml.device_by_index(gpu_b)?;
        // NVML getP2PStatus → NvLink | Pcie | None
    }

    fn mig_instances(&self, gpu_index: u32) -> Result<Vec<MigInstance>> {
        let device = self.nvml.device_by_index(gpu_index)?;
        // NVML MIG 인스턴스 쿼리
    }

    fn numa_info(&self) -> Result<NumaTopology> {
        // NVML + /sys/bus/pci/devices/*/numa_node 파싱
    }
}
```

### 1.4 메트릭 수집 주기와 버퍼링

```rust
pub struct MetricsCollector {
    monitor: Box<dyn GpuMonitor>,
    interval: Duration,               // 기본 1초
    buffer: Arc<RwLock<MetricBuffer>>,
    history_retention: Duration,       // 기본 1시간
}

pub struct MetricBuffer {
    gpu_snapshots: VecDeque<GpuSnapshot>,  // 순환 버퍼
    system_snapshots: VecDeque<SystemSnapshot>,
}

pub struct GpuSnapshot {
    timestamp: DateTime<Utc>,
    gpus: Vec<GpuDetail>,
}

pub struct SystemSnapshot {
    timestamp: DateTime<Utc>,
    cpu_usage_pct: f32,
    memory_total_bytes: u64,
    memory_used_bytes: u64,
    load_avg_1m: f64,
    load_avg_5m: f64,
    load_avg_15m: f64,
}
```

**수집 주기**:
| 메트릭 | 주기 | 비고 |
|--------|------|------|
| GPU VRAM/온도/전력 | 1초 | NVML 폴링 |
| GPU 활용률 | 1초 | NVML utilizationRates |
| PCIe 처리량 | 5초 | 낮은 변화율 |
| CPU/RAM | 2초 | sysinfo |
| NUMA 정보 | 60초 | 정적 정보 |
| NVLink 상태 | 60초 | 변화 감지 |

---

## 2. MIG (Multi-Instance GPU) 지원

### 2.1 MIG 프로파일 관리

NVIDIA A100/H100의 MIG는 GPU를 여러 인스턴스로 분할:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigProfile {
    pub name: String,                  // "1g.5gb", "2g.10gb", "3g.20gb", "4g.20gb", "7g.40gb"
    pub gpu_slice: u8,                 // GPU 연산 스미스 비율 (1/7 단위)
    pub memory_mb: u64,                // 할당 VRAM
    pub max_instances: u8,             // GPU당 최대 인스턴스 수
}

// A100 80GB MIG 프로파일
pub const A100_80GB_PROFILES: &[MigProfile] = &[
    MigProfile { name: "1g.5gb".into(),   gpu_slice: 1, memory_mb: 5120,   max_instances: 7 },
    MigProfile { name: "1g.10gb".into(),  gpu_slice: 1, memory_mb: 10240,  max_instances: 4 },
    MigProfile { name: "2g.20gb".into(),  gpu_slice: 2, memory_mb: 20480,  max_instances: 3 },
    MigProfile { name: "3g.40gb".into(),  gpu_slice: 3, memory_mb: 40960,  max_instances: 2 },
    MigProfile { name: "4g.40gb".into(),  gpu_slice: 4, memory_mb: 40960,  max_instances: 1 },
    MigProfile { name: "7g.80gb".into(),  gpu_slice: 7, memory_mb: 81920,  max_instances: 1 },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigInstance {
    pub gpu_index: u32,
    pub profile: MigProfile,
    pub instance_id: u8,
    pub status: MigInstanceStatus,
    pub assigned_model: Option<String>,   // 할당된 모델 ID
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigInstanceStatus {
    Ready,
    Running { model_id: String, port: u16 },
    Failed(String),
}
```

### 2.2 MIG 인스턴스 생성/해제 흐름

```rust
pub struct MigManager {
    nvml: Nvml,
    instances: Arc<RwLock<HashMap<String, MigInstance>>>,
}

impl MigManager {
    /// MIG 인스턴스 생성
    pub async fn create_instance(
        &self,
        gpu_index: u32,
        profile: &MigProfile,
    ) -> Result<MigInstance> {
        // 1. nvidia-smi mig -cgi {gpu_index} -C {profile.name}
        // 2. 인스턴스 상태 확인
        // 3. instances 맵에 등록
        todo!()
    }

    /// MIG 인스턴스 해제
    pub async fn destroy_instance(
        &self,
        gpu_index: u32,
        instance_id: u8,
    ) -> Result<()> {
        // 1. 실행 중 모델 정지
        // 2. nvidia-smi mig -dgi {gpu_index}:{instance_id}
        // 3. instances 맵에서 제거
        todo!()
    }

    /// 모델 VRAM 요구량에 맞는 최소 MIG 프로파일 선택
    pub fn select_profile(
        &self,
        vram_requirement_mb: u64,
        available_profiles: &[MigProfile],
    ) -> Option<MigProfile> {
        available_profiles
            .iter()
            .filter(|p| p.memory_mb >= vram_requirement_mb)
            .min_by_key(|p| p.memory_mb)  // 최소 프로파일
            .cloned()
    }
}
```

### 2.3 nvidia-smi MIG 래핑

```rust
async fn nvidia_smi_mig_create(gpu_index: u32, profile: &str) -> Result<()> {
    let output = tokio::process::Command::new("nvidia-smi")
        .args(["mig", "-cgi", &gpu_index.to_string(), "-C", profile])
        .output()
        .await
        .map_err(|e| GadgetronError::Node(format!("nvidia-smi mig create failed: {}", e)))?;

    if !output.status.success() {
        return Err(GadgetronError::Node(format!(
            "MIG create error: {}", String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

async fn nvidia_smi_mig_destroy(gpu_index: u32, instance_id: u8) -> Result<()> {
    let gi_id = format!("{}:{}", gpu_index, instance_id);
    let output = tokio::process::Command::new("nvidia-smi")
        .args(["mig", "-dgi", &gi_id])
        .output()
        .await
        .map_err(|e| GadgetronError::Node(format!("nvidia-smi mig destroy failed: {}", e)))?;

    if !output.status.success() {
        return Err(GadgetronError::Node(format!(
            "MIG destroy error: {}", String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}
```

---

## 3. GPU Time-Slicing 지원

### 3.1 MPS vs Time-Slicing 선택

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComputePartitioning {
    /// 전용 GPU — 모델이 GPU를 독점
    Dedicated,
    /// NVIDIA MPS (Multi-Process Service) — 메모리 공유, 커널 동시 실행
    Mps {
        active_thread_percentage: u32,  // 1-100
        default_pinned_mem_pct: u32,    // 1-100
    },
    /// Time-Slicing — 시간 분할, 오버헤드 크지만 격리性强
    TimeSlicing {
        time_slice_ms: u32,             // 기본 20ms
    },
    /// MIG — 하드웨어 분할, 최고 격리
    Mig {
        profile: String,                // "3g.40gb" 등
    },
}

impl ComputePartitioning {
    /// 워크로드 특성에 따른 추천
    pub fn recommend(vram_mb: u64, gpu_total_mb: u64, concurrent_models: u32) -> Self {
        let utilization = vram_mb as f64 / gpu_total_mb as f64;
        match (utilization, concurrent_models) {
            (u, _) if u > 0.8 => ComputePartitioning::Dedicated,     // 대형 모델
            (u, 1) if u > 0.5 => ComputePartitioning::Dedicated,     // 중형 단일
            (_, n) if n <= 3 => ComputePartitioning::Mps {            // 소수 동시
                active_thread_percentage: 100 / n as u32,
                default_pinned_mem_pct: 90,
            },
            _ => ComputePartitioning::TimeSlicing { time_slice_ms: 20 }, // 다수 동시
        }
    }
}
```

### 3.2 MPS 제어

```rust
pub struct MpsController;

impl MpsController {
    /// MPS 데몬 시작
    pub async fn start_daemon(&self) -> Result<()> {
        tokio::process::Command::new("nvidia-cuda-mps-control")
            .arg("start")
            .output()
            .await
            .map_err(|e| GadgetronError::Node(format!("MPS daemon start failed: {}", e)))?;
        Ok(())
    }

    /// MPS 데몬 정지
    pub async fn stop_daemon(&self) -> Result<()> {
        tokio::process::Command::new("nvidia-cuda-mps-control")
            .arg("quit")
            .output()
            .await
            .map_err(|e| GadgetronError::Node(format!("MPS daemon stop failed: {}", e)))?;
        Ok(())
    }

    /// 프로세스별 활성 스레드 비율 설정
    pub fn set_active_thread_percentage(&self, pct: u32) -> Result<()> {
        std::env::set_var("CUDA_MPS_ACTIVE_THREAD_PERCENTAGE", pct.to_string());
        Ok(())
    }
}
```

---

## 4. 열-전력 인지 오케스트레이션

### 4.1 온도 임계값과 스로틀링

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalPolicy {
    /// 경고 온도 (°C) — 로그 알림
    pub warning_temp_c: u32,            // 기본 80
    /// 스로틀링 온도 (°C) — 새 요청 거부
    pub throttle_temp_c: u32,           // 기본 85
    /// 셧다운 온도 (°C) — 모델 evict
    pub shutdown_temp_c: u32,           // 기본 90
    /// 전력 상한 (W) — nvidia-smi로 설정
    pub power_cap_w: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThermalAction {
    None,
    Warning { gpu_index: u32, temp_c: u32 },
    Throttle { gpu_index: u32, temp_c: u32 },     // 새 요청 거부
    Evict { gpu_index: u32, model_id: String },    // 모델 evict
    Shutdown { gpu_index: u32 },                    // GPU 셧다운
}

pub struct ThermalController {
    policy: ThermalPolicy,
    scheduler: Arc<Scheduler>,
}

impl ThermalController {
    /// 메트릭 기반 열 관리 결정
    pub async fn evaluate(&self, gpu: &GpuDetail) -> ThermalAction {
        if gpu.temperature_c >= self.policy.shutdown_temp_c {
            // 온도 셧다운 → 해당 GPU 모델 모두 evict
            ThermalAction::Shutdown { gpu_index: gpu.index }
        } else if gpu.temperature_c >= self.policy.throttle_temp_c {
            ThermalAction::Throttle { gpu_index: gpu.index, temp_c: gpu.temperature_c }
        } else if gpu.temperature_c >= self.policy.warning_temp_c {
            ThermalAction::Warning { gpu_index: gpu.index, temp_c: gpu.temperature_c }
        } else {
            ThermalAction::None
        }
    }

    /// 전력 상한 설정
    pub async fn set_power_cap(&self, gpu_index: u32, cap_w: f32) -> Result<()> {
        tokio::process::Command::new("nvidia-smi")
            .args(["-i", &gpu_index.to_string(), "-pl", &cap_w.to_string()])
            .output()
            .await
            .map_err(|e| GadgetronError::Node(format!("Power cap set failed: {}", e)))?;
        Ok(())
    }
}
```

### 4.2 전력 소비 모니터링

`GpuDetail`의 `power_draw_w` / `power_limit_w` 필드를 활용:

```rust
/// 전력 효율성 메트릭
pub struct PowerEfficiency {
    pub tokens_per_watt: f64,          // 토큰/와트
    pub vram_utilization_pct: f32,     // VRAM 활용률
    pub gpu_utilization_pct: f32,      // GPU 연산 활용률
    pub power_efficiency_score: f64,   // 종합 효율성 점수
}

impl PowerEfficiency {
    pub fn calculate(gpu: &GpuDetail, tokens_per_sec: f64) -> Self {
        let tpw = if gpu.power_draw_w > 0.0 {
            tokens_per_sec / gpu.power_draw_w as f64
        } else {
            0.0
        };
        let score = tpw * gpu.vram_used_mb as f64 / gpu.vram_total_mb as f64;
        Self {
            tokens_per_watt: tpw,
            vram_utilization_pct: gpu.vram_used_mb as f32 / gpu.vram_total_mb as f32 * 100.0,
            gpu_utilization_pct: gpu.utilization_pct,
            power_efficiency_score: score,
        }
    }
}
```

---

## 5. 인터커넥션 인지 스케줄링

### 5.1 NUMA 토폴로지 감지

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaTopology {
    pub nodes: Vec<NumaNode>,
    pub gpus: Vec<GpuNumaMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaNode {
    pub id: u32,
    pub cpu_cores: Vec<u32>,
    pub memory_total_mb: u64,
    pub memory_available_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuNumaMapping {
    pub gpu_index: u32,
    pub numa_node: u32,
    pub pcie_bus_id: String,           // "0000:3b:00.0"
    pub nvlinks: Vec<NvLinkInfo>,      // NVLink 연결 정보
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NvLinkInfo {
    pub remote_gpu_index: u32,
    pub link_count: u32,               // NVLink 링크 수 (1-6)
    pub bandwidth_gbps: u32,           // 총 대역폭
}

impl NumaTopology {
    /// NVLink로 직접 연결된 GPU 그룹 감지
    pub fn nvlink_groups(&self) -> Vec<Vec<u32>> {
        // Union-Find로 NVLink 연결 그룹 탐지
        let mut parent: HashMap<u32, u32> = HashMap::new();
        for mapping in &self.gpus {
            parent.insert(mapping.gpu_index, mapping.gpu_index);
        }
        for mapping in &self.gpus {
            for link in &mapping.nvlinks {
                union(&mut parent, mapping.gpu_index, link.remote_gpu_index);
            }
        }
        // 그룹화
        let mut groups: HashMap<u32, Vec<u32>> = HashMap::new();
        for mapping in &self.gpus {
            let root = find(&mut parent, mapping.gpu_index);
            groups.entry(root).or_default().push(mapping.gpu_index);
        }
        groups.into_values().collect()
    }

    /// 동일 NUMA 노드의 GPU 목록
    pub fn gpus_on_numa(&self, numa_id: u32) -> Vec<u32> {
        self.gpus.iter()
            .filter(|g| g.numa_node == numa_id)
            .map(|g| g.gpu_index)
            .collect()
    }
}
```

### 5.2 Tensor Parallelism 배치

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelismConfig {
    pub tp_size: u32,                   // Tensor Parallelism 크기
    pub pp_size: u32,                   // Pipeline Parallelism 크기
    pub ep_size: u32,                   // Expert Parallelism 크기 (MoE)
    pub dp_size: u32,                   // Data Parallelism 크기
    pub gpu_ids: Vec<u32>,             // 사용할 GPU ID 목록
    pub numa_bind: Option<u32>,        // NUMA 노드 바인딩
}

impl ParallelismConfig {
    /// NVLink 그룹 기반 TP 자동 설정
    pub fn auto_tp(topology: &NumaTopology, model_vram_mb: u64, gpu_vram_mb: u64) -> Self {
        let groups = topology.nvlink_groups();
        // 모델 VRAM을 단일 GPU에 올릴 수 있으면 TP=1
        if model_vram_mb <= gpu_vram_mb {
            return Self { tp_size: 1, pp_size: 1, ep_size: 1, dp_size: 1, gpu_ids: vec![0], numa_bind: None };
        }
        // 필요한 GPU 수 계산
        let needed_gpus = (model_vram_mb as f64 / gpu_vram_mb as f64).ceil() as u32;
        // NVLink 그룹에서 충분한 GPU가 있는 그룹 찾기
        for group in &groups {
            if group.len() >= needed_gpus as usize {
                let gpus: Vec<u32> = group.iter().take(needed_gpus as usize).cloned().collect();
                let numa = topology.gpus.iter()
                    .find(|g| g.gpu_index == gpus[0])
                    .map(|g| g.numa_node);
                return Self {
                    tp_size: needed_gpus,
                    pp_size: 1,
                    ep_size: 1,
                    dp_size: 1,
                    gpu_ids: gpus,
                    numa_bind: numa,
                };
            }
        }
        // NVLink 그룹이 부족하면 PP 고려
        let tp_per_node = groups.first().map(|g| g.len()).unwrap_or(1) as u32;
        let pp_needed = (needed_gpus + tp_per_node - 1) / tp_per_node;
        let mut all_gpus = Vec::new();
        for group in &groups {
            all_gpus.extend(group.iter().take(tp_per_node as usize));
            if all_gpus.len() >= (tp_per_node * pp_needed) as usize {
                break;
            }
        }
        Self {
            tp_size: tp_per_node,
            pp_size: pp_needed,
            ep_size: 1,
            dp_size: 1,
            gpu_ids: all_gpus,
            numa_bind: None,
        }
    }

    /// vLLM 시작 인자 생성
    pub fn vllm_args(&self) -> Vec<String> {
        let mut args = vec![
            format!("--tensor-parallel-size={}", self.tp_size),
        ];
        if self.pp_size > 1 {
            args.push(format!("--pipeline-parallel-size={}", self.pp_size));
        }
        args
    }

    /// SGLang 시작 인자 생성
    pub fn sglang_args(&self) -> Vec<String> {
        let mut args = vec![
            format!("--tp={}", self.tp_size),
        ];
        if self.pp_size > 1 {
            args.push(format!("--pp={}", self.pp_size));
        }
        args
    }

    /// numactl 바인딩 인자 생성
    pub fn numactl_args(&self) -> Vec<String> {
        if let Some(numa) = self.numa_bind {
            vec!["numactl".to_string(), "--cpunodebind=".to_string() + &numa.to_string(),
                 "--membind=".to_string() + &numa.to_string()]
        } else {
            vec![]
        }
    }
}
```

### 5.3 Pipeline Parallelism (노드 간)

```rust
/// 멀티노드 PP 배치
pub struct PipelineParallelPlanner {
    topology: NumaTopology,
}

impl PipelineParallelPlanner {
    /// 인터커넥트 대역폭 기반 PP 스테이지 배치
    pub fn plan(
        &self,
        nodes: &[NodeStatus],
        model_vram_mb: u64,
        tp_size: u32,
    ) -> Result<Vec<PipelineStage>> {
        // 1. 각 노드의 사용 가능 VRAM 확인
        // 2. NVLink 그룹 기반 TP 배치
        // 3. 노드 간 PCIe/NVLink 대역폭 기반 PP 스테이지 할당
        // 4. 대역폭이 높은 노드 쌍을 인접 스테이지로 배치
        todo!()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStage {
    pub stage_id: u32,
    pub node_id: String,
    pub gpu_ids: Vec<u32>,
    pub tp_size: u32,
    pub vram_allocated_mb: u64,
}
```

---

## 6. 동적 자원 할당

### 6.1 VRAM 기반 스케줄링 (확장)

기존 `Scheduler::deploy()`를 확장:

```rust
pub struct EnhancedScheduler {
    deployments: Arc<RwLock<HashMap<String, ModelDeployment>>>,
    nodes: Arc<RwLock<HashMap<String, NodeStatus>>>,
    topology: Arc<RwLock<Option<NumaTopology>>>,
    thermal_controller: Arc<ThermalController>,
    // 스케줄링 정책
    policy: SchedulingPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulingPolicy {
    /// Eviction 전략
    pub eviction_policy: EvictionPolicy,
    /// 우선순위 가중치
    pub priority_weight: f32,
    /// VRAM 안전 마진 (MB)
    pub vram_safety_margin_mb: u64,
    /// 프리페치 활성화
    pub enable_prefetch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionPolicy {
    /// LRU (기본)
    Lru,
    /// 우선순위 기반 (낮은 우선순위 먼저 evict)
    Priority,
    /// 비용 기반 (가장 저렴한 모델 먼저 evict)
    CostBased,
    /// 혼합: LRU + 우선순위 가중치
    WeightedLru { priority_weight: f32 },
}

impl EnhancedScheduler {
    /// 개선된 배치: NUMA 인식 + 열 관리
    pub async fn deploy_enhanced(
        &self,
        model_id: &str,
        engine: InferenceEngine,
        vram_mb: u64,
        parallelism: &ParallelismConfig,
    ) -> Result<()> {
        // 1. 열 상태 확인 — 스로틀링 중인 노드 제외
        // 2. NUMA 노드별 GPU 가용성 확인
        // 3. NVLink 그룹 기반 TP 배치
        // 4. VRAM 확보 (필요 시 evict)
        // 5. numactl 바인딩으로 프로세스 시작
        // 6. 헬스체크 대기
        todo!()
    }

    /// 개선된 eviction: 우선순위 + 비용 가중치
    pub async fn find_eviction_candidate_enhanced(
        &self,
        node_id: &str,
        required_mb: u64,
    ) -> Option<EvictionCandidate> {
        let deployments = self.deployments.read().await;
        let running: Vec<&ModelDeployment> = deployments.values()
            .filter(|d| d.assigned_node == node_id && d.is_available())
            .collect();

        let mut candidates: Vec<EvictionCandidate> = running.iter().map(|d| {
            let priority_score = 1.0 / (d.request_count as f32 + 1.0);
            let lru_score = (chrono::Utc::now() - d.last_used).num_seconds() as f32 / 3600.0;
            let cost_score = 0.0; // TODO: 비용 데이터 연동
            let score = match &self.policy.eviction_policy {
                EvictionPolicy::Lru => lru_score,
                EvictionPolicy::Priority => priority_score,
                EvictionPolicy::CostBased => cost_score,
                EvictionPolicy::WeightedLru { priority_weight } => {
                    lru_score * (1.0 - priority_weight) + priority_score * priority_weight
                }
            };
            EvictionCandidate {
                model_id: d.id.clone(),
                vram_mb: d.vram_requirement_mb,
                score,
            }
        }).collect();

        // 점수 기준 정렬 (높을수록 eviction 후보)
        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // 필요 VRAM을 충족할 때까지 누적
        let mut freed_mb: u64 = 0;
        for candidate in &candidates {
            freed_mb += candidate.vram_mb;
            if freed_mb >= required_mb {
                return Some(candidate.clone());
            }
        }
        None
    }

    /// 사용 패턴 기반 프리페치
    pub async fn prefetch_check(&self) {
        if !self.policy.enable_prefetch {
            return;
        }
        // 시간대별 사용 패턴 분석
        // 다음에 사용될 모델 예측
        // VRAM 여유 시 미리 로드
        // TODO: Phase 2 구현
    }
}

#[derive(Debug, Clone)]
pub struct EvictionCandidate {
    pub model_id: String,
    pub vram_mb: u64,
    pub score: f32,
}
```

---

## 7. K8s/Slurm 통합

### 7.1 Kubernetes CRD

```yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: gadgetronmodels.gadgetron.dev
spec:
  group: gadgetron.dev
  versions:
    - name: v1alpha1
      served: true
      storage: true
      schema:
        openAPIV3Schema:
          type: object
          properties:
            spec:
              type: object
              properties:
                modelId:
                  type: string
                engine:
                  type: string
                  enum: [ollama, vllm, sglang, llamacpp, tgi]
                vramRequirementMB:
                  type: integer
                parallelism:
                  type: object
                  properties:
                    tpSize:
                      type: integer
                    ppSize:
                      type: integer
                    epSize:
                      type: integer
                args:
                  type: array
                  items:
                    type: string
                priority:
                  type: integer
                nodeSelector:
                  type: object
                  additionalProperties:
                    type: string
            status:
              type: object
              properties:
                phase:
                  type: string
                  enum: [Pending, Loading, Running, Failed, Unloading]
                endpoint:
                  type: string
                assignedNode:
                  type: string
                assignedGpus:
                  type: array
                  items:
                    type: integer
                port:
                  type: integer
  scope: Namespaced
  names:
    plural: gadgetronmodels
    singular: gadgetronmodel
    kind: GadgetronModel
    shortNames: [gmodel]
```

### 7.2 K8s 오퍼레이터 (의사코드)

```rust
/// Gadgetron K8s 오퍼레이터 리컨실 루프
pub async fn reconcile(model: GadgetronModel) -> Result<()> {
    match model.status.phase {
        None | "Pending" => {
            // 1. GPU 노드 탐색 (nodeSelector 기반)
            // 2. VRAM 가용성 확인
            // 3. Scheduler::deploy() 호출
            // 4. status.phase = "Loading"
        }
        "Loading" => {
            // 1. 모델 로드 상태 폴링
            // 2. 준비되면 status.phase = "Running"
        }
        "Running" => {
            // 1. 헬스체크
            // 2. 실패 시 재시도 또는 "Failed"
        }
        "Failed" => {
            // 1. 에러 로그 수집
            // 2. 재시도 정책에 따라 "Pending" 복귀
        }
        _ => {}
    }
    Ok(())
}
```

### 7.3 Slurm 통합

```rust
pub struct SlurmIntegration {
    sinfo_path: String,    // 기본 "sinfo"
    squeue_path: String,   // 기본 "squeue"
    sbatch_path: String,   // 기본 "sbatch"
    sacct_path: String,    // 기본 "sacct"
}

impl SlurmIntegration {
    /// Slurm 클러스터 GPU 파티션 탐색
    pub async fn discover_gpu_partitions(&self) -> Result<Vec<SlurmPartition>> {
        let output = tokio::process::Command::new(&self.sinfo_path)
            .args(["--format=%P %G %N %C", "--noheader"])
            .output()
            .await?;

        // sinfo 출력 파싱 → SlurmPartition 목록
        todo!()
    }

    /// 모델 서빙을 위한 sbatch 스크립트 생성
    pub fn generate_sbatch_script(
        &self,
        model_id: &str,
        engine: &InferenceEngine,
        parallelism: &ParallelismConfig,
        partition: &str,
    ) -> String {
        let gpus = format!("gpu:{}", parallelism.tp_size);
        let cpus = parallelism.tp_size * 4; // GPU당 4 CPU

        let command = match engine {
            InferenceEngine::Vllm => format!(
                "vllm serve {} --tensor-parallel-size={} --port $PORT",
                model_id, parallelism.tp_size
            ),
            InferenceEngine::Sglang => format!(
                "python3 -m sglang.launch_server --model-path {} --tp {} --port $PORT",
                model_id, parallelism.tp_size
            ),
            _ => format!("echo 'Engine {} not supported on Slurm'", engine),
        };

        format!(r#"#!/bin/bash
#SBATCH --job-name=gadgetron-{model_id}
#SBATCH --partition={partition}
#SBATCH --gres={gpus}
#SBATCH --cpus-per-task={cpus}
#SBATCH --mem=64G
#SBATCH --time=24:00:00
#SBATCH --output=gadgetron-%j.log

# NUMA 바인딩
numactl --cpunodebind=$SLURM_LOCALID --membind=$SLURM_LOCALID \
  {command}
"#, model_id = model_id, partition = partition, gpus = gpus, cpus = cpus, command = command)
    }

    /// 작업 제출
    pub async fn submit_job(&self, script: &str) -> Result<u64> {
        let output = tokio::process::Command::new(&self.sbatch_path)
            .stdin(std::process::Stdio::piped())
            .output()
            .await?;

        // "Submitted batch job 12345" 파싱
        todo!()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlurmPartition {
    pub name: String,
    pub gpus: Vec<String>,            // GPU 타입 (A100, H100 등)
    pub nodes: Vec<String>,
    pub total_cpus: u32,
    pub total_gpus: u32,
}
```

### 7.4 컨테이너 런타임 통합

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContainerRuntime {
    Docker,
    Podman,
    Containerd { namespace: String },
}

impl ContainerRuntime {
    /// GPU 할당을 위한 컨테이너 시작 인자 생성
    pub fn gpu_args(&self, gpu_ids: &[u32], runtime: &ContainerRuntime) -> Vec<String> {
        let device_args = gpu_ids.iter()
            .flat_map(|id| vec!["--device".to_string(), format!("/dev/nvidia{}", id)])
            .collect();

        let runtime_args = match runtime {
            ContainerRuntime::Docker => vec!["--runtime=nvidia".to_string()],
            ContainerRuntime::Podman => vec!["--security-opt=label=disable".to_string()],
            ContainerRuntime::Containerd { .. } => vec![],
        };

        let env_args = vec![
            "-e".to_string(),
            format!("NVIDIA_VISIBLE_DEVICES={}", gpu_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")),
        ];

        [runtime_args, device_args, env_args].concat()
    }

    /// 컨테이너 시작
    pub async fn start_container(
        &self,
        image: &str,
        gpu_ids: &[u32],
        port: u16,
        model_args: &[String],
    ) -> Result<String> {
        let mut cmd = match self {
            ContainerRuntime::Docker => {
                let mut c = tokio::process::Command::new("docker");
                c.arg("run").arg("-d");
                c
            }
            ContainerRuntime::Podman => {
                let mut c = tokio::process::Command::new("podman");
                c.arg("run").arg("-d");
                c
            }
            ContainerRuntime::Containerd { namespace } => {
                let mut c = tokio::process::Command::new("ctr");
                c.arg("-n").arg(namespace).arg("run").arg("-d");
                c
            }
        };

        // GPU 인자
        for arg in self.gpu_args(gpu_ids, self) {
            cmd.arg(arg);
        }

        // 포트 매핑
        cmd.arg("-p").arg(format!("{}:{}", port, port));

        // 이미지와 모델 인자
        cmd.arg(image);
        for arg in model_args {
            cmd.arg(arg);
        }

        let output = cmd.output().await
            .map_err(|e| GadgetronError::Node(format!("Container start failed: {}", e)))?;

        // 컨테이너 ID 반환
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}
```

---

## 8. 설정 스키마

### gadgetron.toml GPU/스케줄링 섹션

```toml
[gpu]
# GPU 모니터링
poll_interval_ms = 1000              # GPU 메트릭 수집 주기
enable_nvml = true                   # NVML 활성화

[gpu.thermal]
warning_temp_c = 80                  # 경고 온도
throttle_temp_c = 85                 # 스로틀링 온도
shutdown_temp_c = 90                 # 셧다운 온도
power_cap_w = 300                    # 전력 상한 (W)

[gpu.mig]
enabled = false                      # MIG 활성화
default_profile = "3g.40gb"          # 기본 MIG 프로파일

[gpu.partitioning]
default = "dedicated"                # dedicated | mps | time_slicing | mig
mps_active_thread_pct = 50           # MPS 스레드 비율
time_slice_ms = 20                   # Time-slicing 간격

[scheduling]
eviction_policy = "weighted_lru"     # lru | priority | cost_based | weighted_lru
priority_weight = 0.3               # 우선순위 가중치 (0.0~1.0)
vram_safety_margin_mb = 1024        # VRAM 안전 마진
enable_prefetch = false              # 프리페치 활성화 (Phase 2)

[scheduling.parallelism]
auto_tp = true                       # NVLink 기반 TP 자동 설정
prefer_nvlink = true                 # NVLink GPU 우선 배치
numa_aware = true                    # NUMA 바인딩 활성화

[integration.kubernetes]
enabled = false
namespace = "gadgetron"
kubeconfig = "/etc/gadgetron/kubeconfig"

[integration.slurm]
enabled = false
partition = "gpu"
sinfo_path = "/usr/bin/sinfo"
sbatch_path = "/usr/bin/sbatch"
max_job_duration_hours = 24

[integration.container]
runtime = "docker"                   # docker | podman | containerd
containerd_namespace = "default"
nvidia_runtime = "nvidia"            # NVIDIA Container Toolkit 런타임
```

---

## 9. API 엔드포인트

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/gpus` | 전체 GPU 목록 및 상태 |
| GET | `/api/v1/gpus/:index` | 개별 GPU 상세 정보 |
| GET | `/api/v1/gpus/:index/mig` | MIG 인스턴스 목록 |
| POST | `/api/v1/gpus/:index/mig` | MIG 인스턴스 생성 |
| DELETE | `/api/v1/gpus/:index/mig/:instance_id` | MIG 인스턴스 해제 |
| GET | `/api/v1/topology` | NUMA/NVLink 토폴로지 |
| GET | `/api/v1/topology/nvlink-groups` | NVLink 그룹 목록 |
| POST | `/api/v1/scheduling/deploy` | 모델 배치 (병렬 설정 포함) |
| POST | `/api/v1/scheduling/evict` | 수동 evict 요청 |
| GET | `/api/v1/scheduling/candidates` | Eviction 후보 목록 |
| POST | `/api/v1/thermal/set-cap` | 전력 상한 설정 |
| GET | `/api/v1/thermal/policy` | 열 관리 정책 조회 |
| PUT | `/api/v1/thermal/policy` | 열 관리 정책 수정 |
