# Gadgetron 배포 및 운영 설계 문서

> 버전: 1.0.0
> 최종 수정: 2026-04-11
> 담당: Ops Lead

---

## 목차

1. [컨테이너 통합](#1-컨테이너-통합)
2. [Slurm 통합](#2-slurm-통합)
3. [Kubernetes 통합](#3-kubernetes-통합)
4. [설정 관리](#4-설정-관리)
5. [로깅 및 트레이싱](#5-로깅-및-트레이싱)
6. [CI/CD](#6-cicd)
7. [보안](#7-보안)
8. [관측성 스택](#8-관측성-스택)

---

## 1. 컨테이너 통합

### 1.1 Docker 이미지

Gadgetron은 멀티스테이지 빌드를 통해 최소 크기의 단일 바이너리 컨테이너 이미지를 생성합니다. 런타임 베이스 이미지로 distroless를 기본으로 사용하며, 디버깅이 필요한 경우 Alpine 변형을 제공합니다.

```dockerfile
# ============================================================
# Stage 1: 빌드 스테이지
# ============================================================
FROM rust:1.82-bookworm AS builder

# 의존성 캐싱을 위한 빈 프로젝트 생성
WORKDIR /usr/src/gadgetron
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release || true

# 실제 소스 코드 복사 및 빌드
COPY . .
RUN touch src/main.rs
RUN cargo build --release --locked

# ============================================================
# Stage 2a: distroless 런타임 (기본)
# ============================================================
FROM gcr.io/distroless/cc-debian12:nonroot AS distroless

COPY --from=builder /usr/src/gadgetron/target/release/gadgetron /usr/bin/gadgetron

EXPOSE 8080 9090
ENTRYPOINT ["gadgetron"]
CMD ["serve", "--config", "/etc/gadgetron/gadgetron.toml"]

# ============================================================
# Stage 2b: Alpine 런타임 (디버그 변형)
# ============================================================
FROM alpine:3.20 AS alpine

RUN apk add --no-cache ca-certificates libgcc
COPY --from=builder /usr/src/gadgetron/target/release/gadgetron /usr/bin/gadgetron

EXPOSE 8080 9090
ENTRYPOINT ["gadgetron"]
CMD ["serve", "--config", "/etc/gadgetron/gadgetron.toml"]
```

**이미지 태깅 전략:**

| 태그 | 설명 |
|------|------|
| `ghcr.io/gadgetron/gadgetron:latest` | 안정 릴리스 최신 버전 |
| `ghcr.io/gadgetron/gadgetron:1.2.3` | 특정 버전 |
| `ghcr.io/gadgetron/gadgetron:nightly` | main 브랜치 최신 빌드 |
| `ghcr.io/gadgetron/gadgetron:1.2.3-alpine` | Alpine 디버그 변형 |

### 1.2 Docker Compose

로컬 개발 및 소규모 배포를 위한 Docker Compose 구성입니다. Gadgetron 코어 서비스와 모델 추론 엔진(Ollama, vLLM, SGLang)을 통합합니다.

```yaml
# docker-compose.yml
version: "3.8"

services:
  gadgetron:
    image: ghcr.io/gadgetron/gadgetron:latest
    ports:
      - "8080:8080"   # API 포트
      - "9090:9090"   # 메트릭 포트
    volumes:
      - ./gadgetron.toml:/etc/gadgetron/gadgetron.toml:ro
    environment:
      GADGETRON_LOG_LEVEL: "info"
      GADGETRON_PROVIDER_OLLAMA_ENDPOINT: "http://ollama:11434"
      GADGETRON_PROVIDER_VLLM_ENDPOINT: "http://vllm:8000"
      GADGETRON_PROVIDER_SGLANG_ENDPOINT: "http://sglang:30000"
    depends_on:
      ollama:
        condition: service_healthy
      vllm:
        condition: service_healthy
      sglang:
        condition: service_healthy
    healthcheck:
      test: ["CMD", "gadgetron", "health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 15s

  ollama:
    image: ollama/ollama:latest
    ports:
      - "11434:11434"
    volumes:
      - ollama_data:/root/.ollama
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1
              capabilities: [gpu]
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:11434/api/tags"]
      interval: 15s
      timeout: 5s
      retries: 5

  vllm:
    image: vllm/vllm-openai:latest
    ports:
      - "8000:8000"
    volumes:
      - huggingface_cache:/root/.cache/huggingface
    environment:
      VLLM_MODEL: "meta-llama/Meta-Llama-3-8B-Instruct"
      VLLM_MAX_MODEL_LEN: "4096"
      VLLM_GPU_MEMORY_UTILIZATION: "0.9"
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1
              capabilities: [gpu]
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8000/health"]
      interval: 15s
      timeout: 5s
      retries: 10
      start_period: 60s

  sglang:
    image: lmsys/sglang:latest
    ports:
      - "30000:30000"
    volumes:
      - huggingface_cache:/root/.cache/huggingface
    environment:
      SGLANG_MODEL_PATH: "meta-llama/Meta-Llama-3-8B-Instruct"
      SGLANG_MEM_FRACTION_STATIC: "0.85"
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1
              capabilities: [gpu]
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:30000/health"]
      interval: 15s
      timeout: 5s
      retries: 10
      start_period: 60s

  prometheus:
    image: prom/prometheus:latest
    ports:
      - "9091:9090"
    volumes:
      - ./monitoring/prometheus.yml:/etc/prometheus/prometheus.yml:ro

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    volumes:
      - grafana_data:/var/lib/grafana
    depends_on:
      - prometheus

volumes:
  ollama_data:
  huggingface_cache:
  grafana_data:
```

### 1.3 Kubernetes — Helm 차트 구조

```
charts/gadgetron/
├── Chart.yaml
├── values.yaml
├── crds/
│   ├── gadgetronmodel.yaml
│   ├── gadgetronnode.yaml
│   └── gadgetronrouting.yaml
├── templates/
│   ├── deployment.yaml
│   ├── service.yaml
│   ├── configmap.yaml
│   ├── secret.yaml
│   ├── hpa.yaml
│   ├── pdb.yaml
│   ├── networkpolicy.yaml
│   ├── serviceaccount.yaml
│   ├── clusterrole.yaml
│   ├── clusterrolebinding.yaml
│   ├── _helpers.tpl
│   └── NOTES.txt
└── schemas/
    └── values.schema.json
```

### 1.4 컨테이너 런타임 — GPU 접근 감지

Gadgetron은 컨테이너 런타임에서 GPU 접근 가능 여부를 자동 감지하여 모델 프로세스에 GPU를 투명하게 전달합니다.

```toml
# gadgetron.toml — GPU 감지 설정
[container]
runtime = "auto"              # auto | nvidia | amd | none

[container.gpu]
detect_on_startup = true      # 시작 시 GPU 감지 수행
nvidia_runtime_path = "/usr/bin/nvidia-container-runtime"
nvidia_smi_path = "/usr/bin/nvidia-smi"
amd_rocm_path = "/opt/rocm/bin/rocm-smi"
mig_profiles = []             # 예: ["1g.5gb", "2g.10gb"]
time_slice_factor = 1         # 1 = 전용, 2+ = 시분할
```

**감지 로직 의사코드:**

```
fn detect_gpu_access() -> GpuInfo {
    // 1. NVIDIA Container Toolkit 확인
    if path_exists("/usr/bin/nvidia-container-runtime") {
        // nvidia-smi로 GPU 수, VRAM, 드라이버 버전 조회
        let gpus = run("nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv");
        return GpuInfo::Nvidia { devices: parse_gpus(gpus) };
    }

    // 2. AMD ROCm 확인
    if path_exists("/opt/rocm/bin/rocm-smi") {
        let gpus = run("rocm-smi --showproductname");
        return GpuInfo::Amd { devices: parse_amd_gpus(gpus) };
    }

    // 3. GPU 없음 — CPU 전용 모드
    GpuInfo::None
}
```

### 1.5 헬스 프로브

Gadgetron은 세 가지 헬스 프로브를 노출합니다.

| 프로브 | 엔드포인트 | 목적 | 실패 시 동작 |
|--------|-----------|------|-------------|
| **Startup** | `GET /health` | 컨테이너 시작 완료 확인 | 실패 시 재시작 보류 |
| **Liveness** | `GET /health` | 프로세스 생존 확인 | 실패 시 Pod 재시작 |
| **Readiness** | `GET /ready` | 트래픽 수신 준비 확인 | 실패 시 Service에서 제거 |

```yaml
# Kubernetes 프로브 설정
startupProbe:
  httpGet:
    path: /health
    port: 8080
  failureThreshold: 30
  periodSeconds: 5

livenessProbe:
  httpGet:
    path: /health
    port: 8080
  failureThreshold: 3
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /ready
    port: 8080
  failureThreshold: 3
  periodSeconds: 5
```

**프로브 응답 형식:**

```json
// GET /health
{
  "status": "alive",
  "uptime_seconds": 3600,
  "version": "1.2.3"
}

// GET /ready
{
  "status": "ready",
  "providers": {
    "ollama": "healthy",
    "vllm": "healthy",
    "sglang": "degraded"
  },
  "models_loaded": 5,
  "models_pending": 1
}
```

---

## 2. Slurm 통합

### 2.1 Slurm 클러스터 발견

Gadgetron은 Slurm 클러스터의 토폴로지를 자동으로 발견합니다. `sinfo` 명령어를 파싱하여 파티션, 노드, GPU 리소스 정보를 수집합니다.

```bash
# 클러스터 정보 수집 명령
sinfo --format="%P %N %G %C %D" --noheader

# 출력 예시:
# gpu* node[01-08] gpu:a100:4 8:0:0:0 8
# cpu  node[09-16] (null)     0:8:0:0 8
# test node17      gpu:a100:1 1:0:0:0 1
```

**발견 데이터 구조:**

```rust
struct SlurmClusterInfo {
    partitions: Vec<SlurmPartition>,
    nodes: Vec<SlurmNode>,
    gpus: GpuInventory,
}

struct SlurmPartition {
    name: String,
    is_default: bool,
    max_time: Option<Duration>,
    nodes: Vec<String>,
    state: PartitionState,
}

struct SlurmNode {
    hostname: String,
    partitions: Vec<String>,
    gpus: Vec<GpuDevice>,
    cpu_cores: u32,
    memory_gb: u64,
    state: NodeState,
}

struct GpuDevice {
    model: String,        // "a100", "h100" 등
    memory_gb: u64,
    index: u32,
    mig_profiles: Option<Vec<MigProfile>>,
}
```

### 2.2 작업 제출

Gadgetron은 모델 서빙 작업을 Slurm `sbatch` 명령어로 제출합니다.

```bash
# 모델 서빙 작업 제출 스크립트 생성
cat <<'SCRIPT' > /tmp/gadgetron-serve-model.sh
#!/bin/bash
#SBATCH --job-name=gadgetron-llama3-8b
#SBATCH --partition=gpu
#SBATCH --gres=gpu:a100:2
#SBATCH --cpus-per-task=8
#SBATCH --mem=32G
#SBATCH --time=24:00:00
#SBATCH --output=/var/log/gadgetron/jobs/%j.out
#SBATCH --error=/var/log/gadgetron/jobs/%j.err
#SBATCH --nodes=1
#SBATCH --ntasks-per-node=1

# Gadgetron 에이전트 시작
gadgetron model-serve \
  --model "meta-llama/Meta-Llama-3-8B-Instruct" \
  --engine vllm \
  --bind 0.0.0.0:8000 \
  --gpu-memory-utilization 0.9 \
  --max-model-len 4096
SCRIPT

sbatch /tmp/gadgetron-serve-model.sh
```

**리소스 명세 매핑:**

| 모델 카테고리 | GPU | VRAM | CPU | 메모리 | 파티션 |
|--------------|-----|------|-----|--------|--------|
| Small (< 7B) | 1x A100 40GB | 40GB | 4 | 16GB | gpu |
| Medium (7B-13B) | 1x A100 80GB | 80GB | 8 | 32GB | gpu |
| Large (13B-70B) | 2x A100 80GB | 160GB | 16 | 64GB | gpu |
| XL (70B+) | 4x A100 80GB | 320GB | 32 | 128GB | gpu |
| H100 Large | 2x H100 80GB | 160GB | 16 | 64GB | gpu-h100 |

### 2.3 큐 관리

Slurm 작업의 상태를 추적하고 관리합니다.

```bash
# 작업 상태 조회
squeue --user=gadgetron --format="%.18i %.9P %.30j %.8u %.2t %.10M %.6D %R"

# 출력 예시:
# JOBID  PARTITION  NAME                         USER    ST  TIME  NODES  NODELIST(REASON)
# 1001   gpu        gadgetron-llama3-8b           gadget  R   01:30  1      node03
# 1002   gpu        gadgetron-mixtral-8x7b        gadget  PD  0:00   1      (Priority)
# 1003   gpu        gadgetron-gemma-2b            gadget  CG  00:45  1      node05

# 완료된 작업 정보
sacct --job=1001 --format=JobID,JobName,State,Elapsed,MaxRSS,MaxVMSize
```

**작업 상태 추적 구조:**

```rust
enum JobState {
    Pending,
    Running,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}

struct SlurmJob {
    job_id: u64,
    model_id: String,
    engine: InferenceEngine,
    state: JobState,
    submitted_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    node: Option<String>,
    allocated_gpus: Vec<GpuDevice>,
    endpoint: Option<String>,  // 실행 중인 경우 서비스 엔드포인트
}
```

### 2.4 노드 등록

Slurm 노드를 Gadgetron에 자동 등록합니다.

```bash
# 노드 자동 발견 및 등록 명령
gadgetron slurm-register \
  --sinfo-path /usr/bin/sinfo \
  --partition gpu \
  --health-check-interval 60s \
  --auto-deregister-on-drain
```

**자동 등록 프로세스:**

1. `sinfo` 실행으로 노드 및 파티션 정보 수집
2. `scontrol show node <hostname>`으로 GPU 토폴로지 상세 조회
3. 노드 상태 확인 (idle/allocated/drained)
4. Gadgetron 노드 레지스트리에 등록
5. 주기적 헬스체크로 상태 동기화
6. 노드가 drain 상태가 되면 자동 등록 해제

### 2.5 통합 모드

Gadgetron은 두 가지 Slurm 통합 모드를 지원합니다.

#### Side-by-Side 모드

Gadgetron이 Slurm과 독립적으로 스케줄링을 수행합니다. Slurm 노드의 일부 GPU를 Gadgetron 전용으로 예약하고 나머지는 Slurm이 관리합니다.

```toml
[slurm]
mode = "side-by-side"

[slurm.side_by_side]
# Gadgetron이 점유할 노드 범위
dedicated_nodes = ["node01", "node02", "node03"]
# 또는 파티션 내 특정 GPU 슬롯
dedicated_gpus = { node01 = [0, 1], node02 = [0, 1, 2, 3] }
# Slurm 리소스 예약과 충돌 방지
slurm_reservation = "gadgetron_reserved"
```

#### Delegated 모드

Gadgetron이 Slurm에 작업을 위임합니다. 모든 스케줄링 결정을 Slurm이 내리며, Gadgetron은 요청을 Slurm 작업으로 변환합니다.

```toml
[slurm]
mode = "delegated"

[slurm.delegated]
sbatch_path = "/usr/bin/sbatch"
scancel_path = "/usr/bin/scancel"
squeue_path = "/usr/bin/squeue"
default_partition = "gpu"
default_time_limit = "24:00:00"
job_template_dir = "/etc/gadgetron/slurm-templates"
# 작업 제출 실패 시 재시도
max_retries = 3
retry_backoff = "exponential"
```

---

## 3. Kubernetes 통합

### 3.1 Custom Resource Definitions (CRDs)

#### GadgetronModel

모델 배포 사양을 정의하는 CRD입니다.

```yaml
# crds/gadgetronmodel.yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: gadgetronmodels.gadgetron.io
spec:
  group: gadgetron.io
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
              required: ["modelId", "engine"]
              properties:
                modelId:
                  type: string
                  description: "HuggingFace 모델 ID"
                  example: "meta-llama/Meta-Llama-3-8B-Instruct"
                engine:
                  type: string
                  enum: ["vllm", "sglang", "ollama", "tensorrt-llm"]
                  description: "추론 엔진"
                vram:
                  type: string
                  pattern: "^[0-9]+(Gi|Mi)$"
                  description: "필요한 VRAM (예: 80Gi)"
                replicas:
                  type: integer
                  minimum: 0
                  default: 1
                maxReplicas:
                  type: integer
                  minimum: 1
                  default: 10
                resources:
                  type: object
                  properties:
                    cpu:
                      type: string
                    memory:
                      type: string
                    gpuType:
                      type: string
                      description: "GPU 모델 (예: a100, h100)"
                    gpuCount:
                      type: integer
                      minimum: 1
                modelConfig:
                  type: object
                  properties:
                    maxModelLen:
                      type: integer
                    gpuMemoryUtilization:
                      type: number
                      minimum: 0.1
                      maximum: 0.99
                    quantization:
                      type: string
                      enum: ["none", "awq", "gptq", "squeezellm"]
                    trustRemoteCode:
                      type: boolean
                      default: false
                autoscaling:
                  type: object
                  properties:
                    enabled:
                      type: boolean
                      default: false
                    targetQueueDepth:
                      type: integer
                      description: "HPA 대상 큐 깊이"
                    scaleUpCooldown:
                      type: string
                      default: "60s"
                    scaleDownCooldown:
                      type: string
                      default: "300s"
            status:
              type: object
              properties:
                phase:
                  type: string
                  enum: ["Pending", "Loading", "Ready", "Degraded", "Failed"]
                replicas:
                  type: integer
                readyReplicas:
                  type: integer
                endpoint:
                  type: string
                conditions:
                  type: array
                  items:
                    type: object
                    properties:
                      type: type: string
                      status: type: string
                      lastTransitionTime: type: string
                      reason: type: string
                      message: type: string
      subresources:
        status: {}
        scale:
          specReplicasPath: .spec.replicas
          statusReplicasPath: .status.replicas
          statusSelectorPath: .status.labelSelector
      additionalPrinterColumns:
        - name: Engine
          type: string
          jsonPath: .spec.engine
        - name: Replicas
          type: integer
          jsonPath: .spec.replicas
        - name: Ready
          type: integer
          jsonPath: .status.readyReplicas
        - name: Phase
          type: string
          jsonPath: .status.phase
        - name: Endpoint
          type: string
          jsonPath: .status.endpoint
  scope: Namespaced
  names:
    plural: gadgetronmodels
    singular: gadgetronmodel
    kind: GadgetronModel
    shortNames: ["gmodel"]
```

#### GadgetronNode

GPU 노드 등록을 위한 CRD입니다.

```yaml
# crds/gadgetronnode.yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: gadgetronnodes.gadgetron.io
spec:
  group: gadgetron.io
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
              required: ["hostname"]
              properties:
                hostname:
                  type: string
                gpuTopology:
                  type: object
                  properties:
                    gpuCount:
                      type: integer
                    gpuModel:
                      type: string
                    totalVram:
                      type: string
                    nvlink:
                      type: boolean
                    migProfiles:
                      type: array
                      items:
                        type: object
                        properties:
                          name: type: string     # 예: "1g.5gb", "2g.10gb"
                          count: type: integer
                    numaNodes:
                      type: array
                      items:
                        type: object
                        properties:
                          id: type: integer
                          gpuIndices: type: array items: type: integer
                          cpuCores: type: string
                labels:
                  type: object
                  additionalProperties:
                    type: string
            status:
              type: object
              properties:
                registered:
                  type: boolean
                health:
                  type: string
                  enum: ["healthy", "degraded", "unhealthy"]
                gpuUtilization:
                  type: number
                allocatedModels:
                  type: array
                  items:
                    type: string
                lastHealthCheck:
                  type: string
  scope: Namespaced
  names:
    plural: gadgetronnodes
    singular: gadgetronnode
    kind: GadgetronNode
    shortNames: ["gnode"]
```

#### GadgetronRouting

모델별 라우팅 정책을 정의하는 CRD입니다.

```yaml
# crds/gadgetronrouting.yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: gadgetronroutings.gadgetron.io
spec:
  group: gadgetron.io
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
              required: ["modelRef", "policy"]
              properties:
                modelRef:
                  type: string
                  description: "GadgetronModel 리소스 이름"
                policy:
                  type: string
                  enum: ["round-robin", "least-connections", "random", "latency-based", "cost-based"]
                  default: "least-connections"
                priorities:
                  type: array
                  description: "엔진 우선순위 (fallback 순서)"
                  items:
                    type: object
                    properties:
                      engine: type: string
                      weight: type: integer
                tenantOverrides:
                  type: object
                  additionalProperties:
                    type: object
                    properties:
                      policy: type: string
                      maxTokens: type: integer
                rateLimit:
                  type: object
                  properties:
                    requestsPerSecond: type: integer
                    tokensPerMinute: type: integer
                retryPolicy:
                  type: object
                  properties:
                    maxRetries: type: integer
                    retryOn: type: array items: type: string
                    backoff: type: string
            status:
              type: object
              properties:
                activeBackends:
                  type: integer
                totalRequests:
                  type: integer
                avgLatencyMs:
                  type: number
  scope: Namespaced
  names:
    plural: gadgetronroutings
    singular: gadgetronrouting
    kind: GadgetronRouting
    shortNames: ["groute"]
```

### 3.2 오퍼레이터 패턴

Gadgetron 오퍼레이터는 GadgetronModel CR의 조정(reconcile) 루프를 통해 vLLM/SGLang Pod를 생성하고 관리합니다.

**조정 루프 흐름:**

```
GadgetronModel CR 변경
    │
    ▼
┌──────────────────────┐
│   Reconcile Loop     │
├──────────────────────┤
│ 1. 현재 상태 조회     │
│    - 기존 Deployment  │
│    - Pod 상태         │
│    - Service          │
│                      │
│ 2. desired state 계산 │
│    - replicas 조정    │
│    - 리소스 요구사항   │
│    - GPU 스케줄링     │
│                      │
│ 3. 차이 적용          │
│    - Deployment 생성/수정 │
│    - Service 생성/수정 │
│    - HPA 생성/수정    │
│                      │
│ 4. 상태 업데이트      │
│    - phase 업데이트   │
│    - conditions 설정  │
│    - endpoint 기록    │
└──────────────────────┘
    │
    ▼
  완료 / 재조정 대기
```

**오퍼레이터 의사코드:**

```rust
async fn reconcile(model: GadgetronModel) -> Result<(), ReconcileError> {
    // 1. 현재 상태 조회
    let deployment = k8s.get_deployment(&model.name).await?;
    let pods = k8s.list_pods(label_selector = &format("gadgetron-model={}", model.name)).await?;

    // 2. desired state 계산
    let desired_replicas = model.spec.replicas;
    let engine = match model.spec.engine {
        Engine::Vllm => "vllm/vllm-openai",
        Engine::Sglang => "lmsys/sglang",
        Engine::Ollama => "ollama/ollama",
    };

    // 3. GPU 리소스 설정
    let gpu_resources = compute_gpu_resources(&model.spec);
    let tolerations = gpu_tolerations(&model.spec);

    // 4. Deployment 생성 또는 업데이트
    if let Some(deploy) = deployment {
        if needs_update(&deploy, &model) {
            k8s.update_deployment(build_deployment(&model, engine, &gpu_resources)).await?;
        }
    } else {
        k8s.create_deployment(build_deployment(&model, engine, &gpu_resources)).await?;
        k8s.create_service(build_service(&model)).await?;
    }

    // 5. HPA 설정 (autoscaling이 활성화된 경우)
    if model.spec.autoscaling.enabled {
        k8s.create_or_update_hpa(build_hpa(&model)).await?;
    }

    // 6. 상태 업데이트
    let ready_pods = pods.iter().filter(|p| p.is_ready()).count();
    let phase = if ready_pods == 0 && desired_replicas > 0 {
        Phase::Loading
    } else if ready_pods >= desired_replicas {
        Phase::Ready
    } else {
        Phase::Degraded
    };

    k8s.update_status(&model, Status {
        phase,
        replicas: pods.len(),
        ready_replicas: ready_pods,
        endpoint: format!("http://{}-service:8000", model.name),
    }).await?;

    Ok(())
}
```

### 3.3 GPU 스케줄링

Kubernetes에서 GPU 리소스를 세밀하게 스케줄링합니다.

```yaml
# Pod 스펙의 GPU 리소스 설정
resources:
  limits:
    nvidia.com/gpu: 2          # 전용 GPU 2개
  requests:
    nvidia.com/gpu: 2

# MIG(Multi-Instance GPU) 프로파일 사용 시
resources:
  limits:
    nvidia.com/mig-1g.5gb: 4   # 1g.5gb MIG 인스턴스 4개
  requests:
    nvidia.com/mig-1g.5gb: 4

# GPU 시분할(Time-Slicing) 설정
# nvidia-device-plugin ConfigMap
apiVersion: v1
kind: ConfigMap
metadata:
  name: nvidia-device-plugin-config
data:
  config.yaml: |
    version: v1
    flags:
      migStrategy: none
    sharing:
      timeSlicing:
        resources:
          - name: nvidia.com/gpu
            devices: all
            replicas: 4    # 각 GPU를 4개 가상 GPU로 분할
```

**MIG 프로파일 매핑:**

| 모델 크기 | A100 80GB MIG 프로파일 | GPU 수 | VRAM |
|-----------|----------------------|--------|------|
| 7B 양자화 | 1g.5gb | 1 | 5GB |
| 7B FP16 | 2g.10gb | 1 | 10GB |
| 13B FP16 | 3g.20gb | 1 | 20GB |
| 70B 양자화 | 4g.20gb | 2 | 40GB |
| 70B FP16 | 전용 (MIG 없음) | 2 | 160GB |

### 3.4 Horizontal Pod Autoscaler

요청 큐 깊이를 기반으로 모델 복제본 수를 자동 조정합니다.

```yaml
# HPA — 요청 큐 깊이 기반 스케일링
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: gadgetron-model-llama3-8b
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: gadgetron-llama3-8b
  minReplicas: 1
  maxReplicas: 10
  metrics:
    # 큐 깊이 메트릭 (Prometheus Adapter 필요)
    - type: Pods
      pods:
        metric:
          name: gadgetron_request_queue_depth
        target:
          type: AverageValue
          averageValue: "5"    # Pod당 평균 5개 요청 초과 시 스케일 업
    # CPU 보조 메트릭
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
  behavior:
    scaleUp:
      stabilizationWindowSeconds: 60
      policies:
        - type: Pods
          value: 2
          periodSeconds: 60
    scaleDown:
      stabilizationWindowSeconds: 300
      policies:
        - type: Pods
          value: 1
          periodSeconds: 120
      selectPolicy: Min
```

**Prometheus Adapter 설정 (크기 기반 메트릭):**

```yaml
# Prometheus Adapter — 커스텀 메트릭 노출
apiVersion: v1
kind: ConfigMap
metadata:
  name: prometheus-adapter-custom-rules
data:
  custom-rules.yaml: |
    rules:
      - seriesQuery: 'gadgetron_request_queue_depth{namespace!="",pod!=""}'
        resources:
          overrides:
            namespace: { resource: "namespace" }
            pod: { resource: "pod" }
        name:
          matches: "gadgetron_request_queue_depth"
        metricsQuery: 'avg_over_time(gadgetron_request_queue_depth{namespace="{{.namespace}}",pod="{{.pod}}"}[1m])'
```

---

## 4. 설정 관리

### 4.1 메인 설정 파일 — gadgetron.toml

단일 TOML 파일로 모든 설정을 관리하며, 런타임 핫 리로드를 지원합니다.

```toml
# gadgetron.toml — Gadgetron 전체 설정

[server]
bind = "0.0.0.0"
port = 8080
metrics_port = 9090
workers = 4

[log]
level = "info"
format = "json"

[providers.ollama]
endpoint = "http://localhost:11434"
timeout = "30s"
max_retries = 3

[providers.vllm]
endpoint = "http://localhost:8000"
timeout = "60s"
max_retries = 2

[providers.sglang]
endpoint = "http://localhost:30000"
timeout = "60s"
max_retries = 2

[models]
default_engine = "vllm"
auto_load = true

[models.reservation]
small_vram = "40Gi"
medium_vram = "80Gi"
large_vram = "160Gi"

[routing]
default_policy = "least-connections"
health_check_interval = "10s"
health_check_timeout = "5s"

[security]
api_key_enabled = true
rate_limit_rps = 100
max_request_size_mb = 10

[tracing]
enabled = true
endpoint = "http://localhost:4317"
sample_rate = 0.1

[slurm]
mode = "delegated"
default_partition = "gpu"
```

### 4.2 환경 변수 오버라이드

모든 설정 항목은 `GADGETRON_` 접두사 환경 변수로 오버라이드할 수 있습니다. 환경 변수는 TOML 설정보다 우선순위가 높습니다.

**변환 규칙:**

| TOML 경로 | 환경 변수 |
|-----------|----------|
| `server.port` | `GADGETRON_SERVER_PORT` |
| `log.level` | `GADGETRON_LOG_LEVEL` |
| `providers.vllm.endpoint` | `GADGETRON_PROVIDER_VLLM_ENDPOINT` |
| `security.api_key_enabled` | `GADGETRON_SECURITY_API_KEY_ENABLED` |
| `tracing.sample_rate` | `GADGETRON_TRACING_SAMPLE_RATE` |

**우선순위 (높은 순서):**

1. CLI 플래그 (`--bind`, `--config`, `--log-level`)
2. 환경 변수 (`GADGETRON_*`)
3. 설정 파일 (`gadgetron.toml`)
4. 기본값

### 4.3 CLI 플래그

```bash
gadgetron serve \
  --bind 0.0.0.0 \
  --port 8080 \
  --config /etc/gadgetron/gadgetron.toml \
  --log-level debug \
  --log-format json \
  --metrics-port 9090 \
  --workers 8
```

| 플래그 | 대응 설정 | 설명 |
|--------|----------|------|
| `--bind` | `server.bind` | 바인딩 주소 |
| `--port` | `server.port` | API 포트 |
| `--config` | (설정 파일 경로) | 설정 파일 경로 |
| `--log-level` | `log.level` | 로그 레벨 |
| `--log-format` | `log.format` | 로그 형식 (json/text) |
| `--metrics-port` | `server.metrics_port` | 메트릭 포트 |
| `--workers` | `server.workers` | 워커 스레드 수 |

### 4.4 설정 유효성 검사

설정 로드 시 모든 값을 검증하고 명확한 오류 메시지를 제공합니다.

```rust
fn validate_config(config: &Config) -> Result<(), Vec<ConfigError>> {
    let mut errors = Vec::new();

    // 포트 범위 검증
    if config.server.port == 0 || config.server.port > 65535 {
        errors.push(ConfigError::new(
            "server.port",
            format!("포트 번호가 유효하지 않음: {}", config.server.port),
            "1-65535 범위의 포트 번호를 지정하세요.",
        ));
    }

    // 엔드포인트 URL 검증
    for (name, provider) in &config.providers {
        if let Err(e) = Url::parse(&provider.endpoint) {
            errors.push(ConfigError::new(
                &format!("providers.{}.endpoint", name),
                format!("유효하지 않은 URL: {}", provider.endpoint),
                &format!("올바른 URL을 지정하세요. 원인: {}", e),
            ));
        }
    }

    // VRAM 예약 값 검증
    for (tier, vram) in &config.models.reservation {
        if !parse_resource_quantity(vram) {
            errors.push(ConfigError::new(
                &format!("models.reservation.{}", tier),
                format!("VRAM 형식 오류: {}", vram),
                "형식: 숫자+단위 (예: 40Gi, 80Gi, 160Gi)",
            ));
        }
    }

    // 레이트 리밋 검증
    if config.security.rate_limit_rps == 0 {
        errors.push(ConfigError::new(
            "security.rate_limit_rps",
            "0은 허용되지 않음".into(),
            "1 이상의 값을 지정하거나 rate_limit_enabled = false로 설정하세요.",
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
```

**오류 메시지 예시:**

```
❌ 설정 유효성 검사 실패 — 3개 오류 감지:

  [server.port]
  값: 99999
  문제: 포트 번호가 유효하지 않음
  해결: 1-65535 범위의 포트 번호를 지정하세요.

  [providers.sglang.endpoint]
  값: "not-a-url"
  문제: 유효하지 않은 URL
  해결: 올바른 URL을 지정하세요. 원인: relative URL without a base

  [models.reservation.small_vram]
  값: "40"
  문제: VRAM 형식 오류
  해결: 형식: 숫자+단위 (예: 40Gi, 80Gi, 160Gi)
```

### 4.5 Diff 기반 핫 리로드

설정 파일 변경 시 전체 재시작이 아닌 변경된 컴포넌트만 리로드합니다.

```rust
fn hot_reload(old_config: &Config, new_config: &Config) -> ReloadPlan {
    let diff = compute_diff(old_config, new_config);
    let mut plan = ReloadPlan::new();

    for change in diff.changes {
        match change.path.as_str() {
            // 로그 레벨 변경 — 즉시 적용 (재시작 없음)
            "log.level" | "log.format" => {
                plan.add_action(ReloadAction::UpdateLogging {
                    level: new_config.log.level.clone(),
                    format: new_config.log.format.clone(),
                });
            }

            // 프로바이더 엔드포인트 변경 — 해당 프로바이더만 재연결
            p if p.starts_with("providers.") => {
                let provider_name = extract_provider_name(p);
                plan.add_action(ReloadAction::ReconnectProvider {
                    provider: provider_name.to_string(),
                });
            }

            // 라우팅 정책 변경 — 라우터 리로드
            "routing.default_policy" | "routing.health_check_interval" => {
                plan.add_action(ReloadAction::ReloadRouter);
            }

            // 서버 포트/바인딩 변경 — 전체 재시작 필요
            "server.port" | "server.bind" => {
                plan.add_action(ReloadAction::RequireFullRestart {
                    reason: "서버 포트/바인딩 변경은 전체 재시작이 필요합니다.".into(),
                });
            }

            _ => {
                plan.add_action(ReloadAction::ReloadComponent {
                    component: change.path.clone(),
                });
            }
        }
    }

    plan
}
```

**핫 리로드 가능 여부 매트릭스:**

| 설정 항목 | 핫 리로드 | 동작 |
|-----------|----------|------|
| `log.level` | 가능 | 즉시 로그 레벨 변경 |
| `log.format` | 가능 | 즉시 로그 포맷터 교체 |
| `providers.*.endpoint` | 가능 | 해당 프로바이더 재연결 |
| `providers.*.timeout` | 가능 | 타임아웃 값 즉시 업데이트 |
| `routing.*` | 가능 | 라우터 설정 리로드 |
| `security.rate_limit_rps` | 가능 | 토큰 버킷 재설정 |
| `server.port` | 불가 | 전체 재시작 필요 |
| `server.bind` | 불가 | 전체 재시작 필요 |
| `server.workers` | 불가 | 전체 재시작 필요 |
| `tracing.*` | 가능 | 트레이서 재초기화 |

---

## 5. 로깅 및 트레이싱

### 5.1 구조화된 로깅

`tracing` 크레이트와 JSON 포맷터를 사용하여 구조화된 로깅을 제공합니다.

```rust
use tracing_subscriber::{layer, fmt, EnvFilter};
use tracing_subscriber::fmt::format::FmtContext;

fn init_logging(config: &LogConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.level));

    match config.format.as_str() {
        "json" => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json())
                .init();
        }
        "text" => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_target(true))
                .init();
        }
        _ => panic!("지원하지 않는 로그 형식: {}", config.format),
    }
}
```

**JSON 로그 출력 예시:**

```json
{
  "timestamp": "2026-04-11T14:32:01.456Z",
  "level": "INFO",
  "target": "gadgetron::router",
  "span": {
    "name": "handle_request",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "request_id": "req-abc123"
  },
  "fields": {
    "message": "라우팅 완료",
    "model": "meta-llama/Meta-Llama-3-8B-Instruct",
    "provider": "vllm",
    "latency_ms": 12,
    "status": "200"
  }
}
```

### 5.2 모듈별 로그 레벨 설정

RUST_LOG 환경 변수 또는 설정 파일로 모듈별 로그 레벨을 세밀하게 제어합니다.

```toml
# gadgetron.toml — 모듈별 로그 레벨
[log]
level = "info"

[log.modules]
gadgetron_provider = "debug"
gadgetron_router = "debug"
gadgetron_scheduler = "info"
gadgetron_auth = "warn"
gadgetron_config = "info"
hyper = "warn"
tower = "warn"
```

```bash
# 환경 변수로 모듈별 설정
export RUST_LOG="gadgetron=info,gadgetron_provider=debug,gadgetron_router=debug,hyper=warn"
```

### 5.3 분산 트레이싱 — OpenTelemetry

OpenTelemetry SDK를 통합하여 요청의 전체 흐름을 추적합니다.

```rust
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::SubscriberExt;

fn init_tracing(config: &TracingConfig) {
    if !config.enabled {
        return;
    }

    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(&config.endpoint);

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(
            opentelemetry::sdk::trace::config()
                .with_sampler(opentelemetry::sdk::trace::Sampler::TraceIdRatioBased(
                    config.sample_rate,
                ))
                .with_resource(opentelemetry::sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new("service.name", "gadgetron"),
                    opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ]))
        )
        .install_batch(opentelemetry::runtime::Tokio)
        .expect("OpenTelemetry tracer 초기화 실패");

    let otel_layer = OpenTelemetryLayer::new(tracer);

    tracing_subscriber::registry()
        .with(otel_layer)
        .init();
}
```

### 5.4 요청 트레이싱

요청이 라우팅 → 프로바이더로 전달되는 전체 경로에 trace ID를 전파합니다.

```
클라이언트 요청
    │
    ├─ [HTTP 수신] request_id 생성, trace_id 전파
    │   span: handle_request { trace_id, request_id, tenant, model }
    │
    ├─ [인증] API 키 검증
    │   span: authenticate { tenant_id, key_prefix }
    │
    ├─ [라우팅] 모델 → 프로바이더 선택
    │   span: route { model, policy, selected_provider, latency_ms }
    │
    ├─ [프로바이더 호출] vLLM/SGLang/Ollama 요청
    │   span: provider_call { provider, endpoint, tokens_in, tokens_out, latency_ms }
    │
    ├─ [응답 처리] 스트리밍/배치 응답
    │   span: response { status, tokens_out, total_latency_ms }
    │
    └─ [감사 로그] 요청 기록
        span: audit { tenant, model, tokens, cost }
```

### 5.5 감사 로그

모든 API 요청을 감사 로그에 기록합니다.

```rust
struct AuditEntry {
    timestamp: DateTime<Utc>,
    request_id: String,
    trace_id: String,
    tenant_id: String,
    api_key_prefix: String,     // "sk-...abc" (마스킹)
    model: String,
    provider: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
    latency_ms: u64,
    status_code: u16,
    cost_usd: Option<f64>,
    source_ip: String,
    user_agent: Option<String>,
}

// 감사 로그는 별도 파일에 JSON Lines 형식으로 기록
// /var/log/gadgetron/audit/2026-04-11.jsonl
```

**감사 로그 예시 (JSONL):**

```json
{"timestamp":"2026-04-11T14:32:01.456Z","request_id":"req-abc123","trace_id":"4bf92f3577b34da6a3ce929d0e0e4736","tenant_id":"tenant-acme","api_key_prefix":"sk-...xyz","model":"meta-llama/Meta-Llama-3-8B-Instruct","provider":"vllm","prompt_tokens":128,"completion_tokens":256,"total_tokens":384,"latency_ms":1450,"status_code":200,"cost_usd":0.0023,"source_ip":"10.0.1.42","user_agent":"gadgetron-sdk/1.2.0"}
```

---

## 6. CI/CD

### 6.1 GitHub Actions 파이프라인

```yaml
# .github/workflows/ci.yml
name: Gadgetron CI

on:
  push:
    branches: [main, release/*]
    tags: ["v*"]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  REGISTRY: ghcr.io
  IMAGE_NAME: ${{ github.repository }}

jobs:
  # ──────────────────────────────────────────────
  # 린트 검사
  # ──────────────────────────────────────────────
  lint:
    name: 린트 (clippy + fmt)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - name: 포맷 검사
        run: cargo fmt --all -- --check
      - name: Clippy 린트
        run: cargo clippy --all-targets --all-features -- -D warnings

  # ──────────────────────────────────────────────
  # 테스트
  # ──────────────────────────────────────────────
  test:
    name: 테스트
    runs-on: ubuntu-latest
    needs: lint
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: 단위 테스트
        run: cargo test --lib --all-features
      - name: 통합 테스트
        run: cargo test --test '*' --all-features
      - name: 문서 테스트
        run: cargo test --doc

  # ──────────────────────────────────────────────
  # 크로스 컴파일 빌드
  # ──────────────────────────────────────────────
  build:
    name: 빌드 (${{ matrix.target }})
    runs-on: ${{ matrix.os }}
    needs: test
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
            cross: true
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
            cross: true
          - target: x86_64-apple-darwin
            os: macos-latest
            cross: false
          - target: aarch64-apple-darwin
            os: macos-latest
            cross: false
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}
      - name: 크로스 컴파일
        if: matrix.cross
        uses: cross-rs/cross-action@v2
        with:
          command: build
          args: --release --target ${{ matrix.target }}
      - name: 네이티브 빌드
        if: ${{ !matrix.cross }}
        run: cargo build --release --target ${{ matrix.target }}
      - name: 아티팩트 업로드
        uses: actions/upload-artifact@v4
        with:
          name: gadgetron-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/gadgetron

  # ──────────────────────────────────────────────
  # 컨테이너 이미지 빌드 및 푸시
  # ──────────────────────────────────────────────
  container:
    name: 컨테이너 이미지
    runs-on: ubuntu-latest
    needs: build
    if: github.event_name == 'push'
    permissions:
      contents: read
      packages: write
    steps:
      - uses: actions/checkout@v4
      - name: Docker 빌드x 설정
        uses: docker/setup-buildx-action@v3
      - name: QEMU 설정 (멀티 아키)
        uses: docker/setup-qemu-action@v3
      - name: GHCR 로그인
        uses: docker/login-action@v3
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - name: 메타데이터 추출
        id: meta
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}
          tags: |
            type=ref,event=branch
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=sha
      - name: 빌드 및 푸시
        uses: docker/build-push-action@v5
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max

  # ──────────────────────────────────────────────
  # 릴리스
  # ──────────────────────────────────────────────
  release:
    name: 릴리스
    runs-on: ubuntu-latest
    needs: [build, container]
    if: startsWith(github.ref, 'refs/tags/v')
    steps:
      - uses: actions/checkout@v4
      - name: 아티팩트 다운로드
        uses: actions/download-artifact@v4
        with:
          path: artifacts
      - uses: dtolnay/rust-toolchain@stable
      - name: cargo-dist 릴리스
        uses: axodotdev/cargo-dist-action@v0
        with:
          artifacts: artifacts/*
```

### 6.2 릴리스 채널

| 채널 | 소스 | 태그 | 배포 주기 | 안정성 |
|------|------|------|-----------|--------|
| **Nightly** | `main` 브랜치 | `nightly` | 매일 자동 | 실험적 |
| **Beta** | `release/*` 브랜치 | `1.3.0-beta.1` | 주간 | 기능 완성 |
| **Stable** | 태그 | `1.2.3` | 수동 | 프로덕션 준비 |

**릴리스 흐름:**

```
main ──────────────────────────────────── nightly
  │
  ├─ release/1.3 ──────────────────────── beta (1.3.0-beta.1, beta.2, ...)
  │     │
  │     └─ 태그 v1.3.0 ───────────────── stable (1.3.0)
  │
  └─ release/1.4 ──────────────────────── beta (1.4.0-beta.1)
```

### 6.3 크로스 컴파일 대상

| 대상 플랫폼 | 타겟 트리플 | 빌드 방식 |
|------------|------------|----------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | cross |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | cross |
| macOS Intel | `x86_64-apple-darwin` | 네이티브 |
| macOS Apple Silicon | `aarch64-apple-darwin` | 네이티브 |

### 6.4 컨테이너 레지스트리

```bash
# 태그 푸시 시 GHCR에 자동 게시
# 태그 형식에 따른 이미지 태깅:

# v1.2.3 태그 →
#   ghcr.io/gadgetron/gadgetron:1.2.3
#   ghcr.io/gadgetron/gadgetron:1.2
#   ghcr.io/gadgetron/gadgetron:latest

# main 브랜치 푸시 →
#   ghcr.io/gadgetron/gadgetron:main
#   ghcr.io/gadgetron/gadgetron:sha-abc1234

# 멀티 아키텍처 지원:
#   linux/amd64
#   linux/arm64
```

---

## 7. 보안

### 7.1 API 키 인증

Bearer 토큰 기반 API 키 인증을 제공합니다.

```rust
use axum::extract::Request;
use axum::middleware::Next;

async fn auth_middleware(
    headers: HeaderMap,
    state: Arc<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let api_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AuthError::MissingApiKey)?;

    // 키 형식 검증: sk-<32바이트 hex>
    if !api_key.starts_with("sk-") || api_key.len() != 35 {
        return Err(AuthError::InvalidFormat);
    }

    // 키 조회 및 검증
    let key_info = state.key_store
        .validate(api_key)
        .await
        .ok_or(AuthError::InvalidApiKey)?;

    // 만료 확인
    if key_info.is_expired() {
        return Err(AuthError::Expired);
    }

    // 비활성화 확인
    if !key_info.active {
        return Err(AuthError::Disabled);
    }

    // 테넌트 컨텍스트 주입
    request.extensions_mut().insert(TenantContext {
        tenant_id: key_info.tenant_id,
        key_id: key_info.key_id,
        scopes: key_info.scopes,
        quotas: key_info.quotas,
    });

    Ok(next.run(request).await)
}
```

### 7.2 가상 키 (Virtual Keys)

테넌트별 범위가 지정된 키로 할당량 관리를 지원합니다.

```rust
struct VirtualKey {
    key_id: String,
    tenant_id: String,
    key_hash: String,           // SHA-256 해시 저장 (평문 미저장)
    prefix: String,             // "sk-...abc" (식별용 프리픽스)
    scopes: Vec<Scope>,         // 권한 범위
    quotas: QuotaConfig,        // 사용량 제한
    rate_limits: RateLimitConfig,
    expires_at: Option<DateTime<Utc>>,
    active: bool,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

enum Scope {
    ModelsRead,          // 모델 목록 조회
    ModelsUse,           // 모델 추론 요청
    ModelsAdmin,         // 모델 배포 관리
    KeysManage,          // 키 관리
    TenantsRead,         // 테넌트 정보 조회
    BillingRead,         // 사용량/비용 조회
}

struct QuotaConfig {
    max_tokens_per_day: Option<u64>,
    max_tokens_per_month: Option<u64>,
    max_requests_per_minute: Option<u32>,
    allowed_models: Option<Vec<String>>,     // None = 모든 모델 허용
    max_cost_per_day_usd: Option<f64>,
}
```

**키 생성 예시:**

```bash
# 마스터 키로 새 가상 키 생성
curl -X POST http://localhost:8080/v1/keys \
  -H "Authorization: Bearer sk-master-..." \
  -H "Content-Type: application/json" \
  -d '{
    "name": "테넌트 Acme 프로덕션 키",
    "tenant_id": "tenant-acme",
    "scopes": ["ModelsUse", "ModelsRead"],
    "quotas": {
      "max_tokens_per_day": 1000000,
      "allowed_models": ["meta-llama/Meta-Llama-3-8B-Instruct"]
    },
    "rate_limits": {
      "requests_per_second": 50
    },
    "expires_at": "2026-12-31T23:59:59Z"
  }'

# 응답:
{
  "key_id": "key_xk29fj3",
  "key": "sk-vk-a1b2c3d4e5f6...",      # 한 번만 표시
  "prefix": "sk-vk-...f6",
  "tenant_id": "tenant-acme",
  "created_at": "2026-04-11T14:00:00Z"
}
```

### 7.3 TLS — rustls

모든 아웃바운드 연결에 rustls를 사용하여 TLS를 적용합니다.

```toml
# Cargo.toml — TLS 의존성
[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
rustls = "0.23"
rustls-pemfile = "2.0"
tokio-rustls = "0.26"
```

```rust
use reqwest::Client;

fn build_http_client() -> Result<Client, Box<dyn std::error::Error>> {
    // rustls 기반 TLS 클라이언트 (native-tls 미사용)
    let client = Client::builder()
        .use_rustls_tls()
        .min_tls_version(rustls::Version::TLS_1_2)
        .add_root_certs(load_system_roots()?)
        .build()?;
    Ok(client)
}
```

### 7.4 입력 유효성 검사

```rust
const MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024; // 10MB
const MAX_PROMPT_LENGTH: usize = 1_000_000;        // 100만 자

fn validate_request(req: &ModelRequest) -> Result<(), ValidationError> {
    // 요청 크기 제한
    if req.estimated_size() > MAX_REQUEST_SIZE {
        return Err(ValidationError::RequestTooLarge {
            size: req.estimated_size(),
            max: MAX_REQUEST_SIZE,
        });
    }

    // 프롬프트 길이 제한
    if req.prompt.len() > MAX_PROMPT_LENGTH {
        return Err(ValidationError::PromptTooLong {
            length: req.prompt.len(),
            max: MAX_PROMPT_LENGTH,
        });
    }

    // 기본 프롬프트 인젝션 탐지 (Phase 1 — 패턴 매칭)
    let injection_patterns = [
        "ignore all previous instructions",
        "disregard all prior directives",
        "you are now in developer mode",
        "jailbreak",
        "DAN mode activated",
    ];

    let prompt_lower = req.prompt.to_lowercase();
    for pattern in &injection_patterns {
        if prompt_lower.contains(pattern) {
            return Err(ValidationError::PotentialInjection {
                pattern: pattern.to_string(),
            });
        }
    }

    Ok(())
}
```

### 7.5 PII 가드레일 (Phase 2)

Phase 2에서 구현 예정인 PII(개인식별정보) 감지 및 마스킹 기능입니다.

```rust
// Phase 2: PII 감지 파이프라인 (설계만 문서화)
struct PiiGuardrail {
    patterns: Vec<PiiPattern>,
    action: PiiAction,
}

enum PiiPattern {
    Ssn,              // 주민등록번호 / SSN
    CreditCard,       // 신용카드 번호
    PhoneNumber,      // 전화번호
    Email,            // 이메일 주소
    Passport,         // 여권 번호
    Custom(String),   // 커스텀 정규식
}

enum PiiAction {
    Mask,             // 마스킹 후 처리 계속
    Reject,           // 요청 거부
    LogAndMask,       // 로그 기록 + 마스킹
    LogAndReject,     // 로그 기록 + 거부
}
```

### 7.6 레이트 리밋

토큰 버킷 알고리즘으로 API 키별 레이트 리밋을 적용합니다.

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::Instant;

struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,       // 초당 충전 토큰 수
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_rps: u32) -> Self {
        TokenBucket {
            tokens: max_rps as f64,
            max_tokens: max_rps as f64,
            refill_rate: max_rps as f64,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self, tokens: f64) -> bool {
        self.refill();
        if self.tokens >= tokens {
            self.tokens -= tokens;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }
}

struct RateLimiter {
    buckets: Arc<RwLock<HashMap<String, TokenBucket>>>,
    default_rps: u32,
}

impl RateLimiter {
    async fn check(&self, key_id: &str, rps_limit: Option<u32>) -> bool {
        let limit = rps_limit.unwrap_or(self.default_rps);
        let mut buckets = self.buckets.write().await;
        let bucket = buckets
            .entry(key_id.to_string())
            .or_insert_with(|| TokenBucket::new(limit));
        bucket.try_consume(1.0)
    }
}
```

---

## 8. 관측성 스택

### 8.1 메트릭 — Prometheus

`/metrics` 엔드포인트에서 Prometheus 형식 메트릭을 노출합니다.

```rust
use prometheus::{IntCounter, Histogram, IntGauge, Registry, Encoder, TextEncoder};

lazy_static! {
    static ref REGISTRY: Registry = Registry::new();

    // ── 요청 메트릭 ──
    static ref REQUEST_TOTAL: IntCounter = IntCounter::new(
        "gadgetron_request_total",
        "총 API 요청 수"
    ).unwrap();

    static ref REQUEST_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "gadgetron_request_duration_seconds",
            "요청 처리 시간"
        ).buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])
    ).unwrap();

    static ref REQUEST_QUEUE_DEPTH: IntGauge = IntGauge::new(
        "gadgetron_request_queue_depth",
        "현재 대기 중인 요청 수"
    ).unwrap();

    // ── 토큰 메트릭 ──
    static ref TOKENS_PROCESSED: IntCounter = IntCounter::new(
        "gadgetron_tokens_processed_total",
        "처리된 총 토큰 수"
    ).unwrap();

    // ── 프로바이더 메트릭 ──
    static ref PROVIDER_REQUEST_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("gadgetron_provider_request_total", "프로바이더별 요청 수"),
        &["provider", "model", "status"]
    ).unwrap();

    static ref PROVIDER_LATENCY: HistogramVec = HistogramVec::new(
        HistogramOpts::new(
            "gadgetron_provider_latency_seconds",
            "프로바이더 응답 지연 시간"
        ).buckets(vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0]),
        &["provider", "model"]
    ).unwrap();

    // ── GPU 메트릭 ──
    static ref GPU_UTILIZATION: IntGaugeVec = IntGaugeVec::new(
        Opts::new("gadgetron_gpu_utilization_percent", "GPU 사용률"),
        &["node", "gpu_index", "model"]
    ).unwrap();

    static ref GPU_VRAM_USED: IntGaugeVec = IntGaugeVec::new(
        Opts::new("gadgetron_gpu_vram_used_bytes", "GPU VRAM 사용량"),
        &["node", "gpu_index", "model"]
    ).unwrap();

    // ── 모델 메트릭 ──
    static ref MODEL_REPLICAS: IntGaugeVec = IntGaugeVec::new(
        Opts::new("gadgetron_model_replicas", "모델별 활성 복제본 수"),
        &["model", "engine"]
    ).unwrap();
}
```

**Prometheus 스크랩 설정:**

```yaml
# monitoring/prometheus.yml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: "gadgetron"
    metrics_path: "/metrics"
    static_configs:
      - targets:
          - "gadgetron:9090"
    relabel_configs:
      - source_labels: [__address__]
        target_label: instance

  - job_name: "vllm"
    metrics_path: "/metrics"
    static_configs:
      - targets:
          - "vllm:8000"

  - job_name: "node-exporter"
    static_configs:
      - targets:
          - "node-exporter:9100"

  - job_name: "nvidia-gpu"
    static_configs:
      - targets:
          - "nvidia-gpu-exporter:9835"
```

### 8.2 트레이스 — OpenTelemetry → Jaeger/Tempo

```yaml
# monitoring/tempo.yml (Tempo를 트레이스 백엔드로 사용)
server:
  http_listen_port: 3200

distributor:
  receivers:
    otlp:
      protocols:
        grpc:
          endpoint: "0.0.0.0:4317"
        http:
          endpoint: "0.0.0.0:4318"

storage:
  trace:
    backend: local
    local:
      path: /var/tempo/traces
    wal:
      path: /var/tempo/wal

metrics_generator:
  registry:
    external_labels:
      source: tempo
  storage:
    path: /var/tempo/generator/wal
    remote_write:
      - url: http://prometheus:9090/api/v1/write
```

### 8.3 로그 — 구조화된 JSON (ELK/Loki 호환)

```yaml
# monitoring/loki.yml
auth_enabled: false

server:
  http_listen_port: 3100
  grpc_listen_port: 9096

common:
  path_prefix: /loki
  storage:
    filesystem:
      chunks_directory: /loki/chunks
      rules_directory: /loki/rules
  replication_factor: 1
  ring:
    kvstore:
      store: inmemory

schema_config:
  configs:
    - from: 2020-10-24
      store: boltdb-shipper
      object_store: filesystem
      schema: v11
      index:
        prefix: index_
        period: 24h

# Promtail이 Gadgetron JSON 로그를 수집하여 Loki로 전송
```

```yaml
# monitoring/promtail.yml
server:
  http_listen_port: 9080
  grpc_listen_port: 0

positions:
  filename: /tmp/positions.yaml

clients:
  - url: http://loki:3100/loki/api/v1/push

scrape_configs:
  - job_name: gadgetron
    static_configs:
      - targets:
          - localhost
        labels:
          job: gadgetron
          __path__: /var/log/gadgetron/*.jsonl
    pipeline_stages:
      - json:
          expressions:
            level: level
            target: target
            message: fields.message
            trace_id: span.trace_id
            request_id: span.request_id
      - labels:
          level:
          target:
      - timestamp:
          source: timestamp
          format: RFC3339
```

### 8.4 Grafana 대시보드

Gadgetron 메트릭을 시각화하는 Grafana 대시보드 JSON을 제공합니다.

**대시보드 패널 구성:**

| 패널 | 메트릭 | 시각화 | 설명 |
|------|--------|--------|------|
| 요청율 | `rate(gadgetron_request_total[5m])` | Time series | 초당 요청 수 |
| P50/P95/P99 지연 | `histogram_quantile(0.95, rate(gadgetron_request_duration_seconds_bucket[5m]))` | Time series | 요청 처리 지연 |
| 큐 깊이 | `gadgetron_request_queue_depth` | Stat | 대기 중인 요청 |
| 토큰 처리율 | `rate(gadgetron_tokens_processed_total[5m])` | Time series | 초당 토큰 처리 |
| 프로바이더별 요청 | `gadgetron_provider_request_total` | Stacked bar | 프로바이더 분배 |
| GPU 사용률 | `gadgetron_gpu_utilization_percent` | Gauge | 노드별 GPU 사용률 |
| VRAM 사용량 | `gadgetron_gpu_vram_used_bytes` | Bar chart | GPU 메모리 분배 |
| 모델 복제본 | `gadgetron_model_replicas` | Table | 모델별 활성 인스턴스 |
| 에러율 | `rate(gadgetron_request_total{status=~"5.."}[5m])` | Alert | 5xx 에러율 |
| 인증 실패 | `rate(gadgetron_auth_failures_total[5m])` | Alert | 인증 실패율 |

**대시보드 JSON 내보내기 경로:**

```
monitoring/
├── grafana/
│   ├── dashboards/
│   │   ├── gadgetron-overview.json      # 전체 개요
│   │   ├── gadgetron-models.json        # 모델별 상세
│   │   ├── gadgetron-providers.json     # 프로바이더 성능
│   │   ├── gadgetron-gpu.json           # GPU 리소스
│   │   └── gadgetron-security.json      # 보안/인증
│   └── datasources/
│       ├── prometheus.yml
│       ├── loki.yml
│       └── tempo.yml
├── prometheus/
│   ├── prometheus.yml
│   └── alert_rules.yml
├── loki/
│   └── loki.yml
├── tempo/
│   └── tempo.yml
└── promtail/
    └── promtail.yml
```

---

## 부록 A: Slurm 통합 명령어 레퍼런스

```bash
# ──────────────────────────────────────────────
# 클러스터 발견
# ──────────────────────────────────────────────
# 전체 노드 및 파티션 정보
gadgetron slurm-discover

# 특정 파티션만 발견
gadgetron slurm-discover --partition gpu

# GPU 노드만 필터링
gadgetron slurm-discover --gpu-only

# ──────────────────────────────────────────────
# 작업 제출
# ──────────────────────────────────────────────
# 기본 모델 서빙 작업 제출
gadgetron slurm-submit \
  --model "meta-llama/Meta-Llama-3-8B-Instruct" \
  --engine vllm \
  --partition gpu \
  --gres "gpu:a100:1" \
  --time 24:00:00

# 커스텀 리소스로 제출
gadgetron slurm-submit \
  --model "meta-llama/Meta-Llama-3-70B-Instruct" \
  --engine sglang \
  --partition gpu-h100 \
  --gres "gpu:h100:4" \
  --cpus-per-task 32 \
  --mem 256G \
  --time 48:00:00

# ──────────────────────────────────────────────
# 작업 관리
# ──────────────────────────────────────────────
# 실행 중인 작업 목록
gadgetron slurm-jobs --status running

# 대기 중인 작업 목록
gadgetron slurm-jobs --status pending

# 특정 작업 상세 정보
gadgetron slurm-job-info --job-id 1001

# 작업 취소
gadgetron slurm-cancel --job-id 1001

# 모델 서빙 작업 전체 취소
gadgetron slurm-cancel --model "meta-llama/Meta-Llama-3-8B-Instruct"

# ──────────────────────────────────────────────
# 노드 관리
# ──────────────────────────────────────────────
# Slurm 노드를 Gadgetron에 등록
gadgetron slurm-register --auto

# 특정 노드 등록
gadgetron slurm-register --nodes "node01,node02,node03"

# 노드 상태 확인
gadgetron slurm-nodes

# 노드 드레인 및 등록 해제
gadgetron slurm-deregister --node node03
```

## 부록 B: Helm 차트 values.yaml

```yaml
# charts/gadgetron/values.yaml
replicaCount: 2

image:
  repository: ghcr.io/gadgetron/gadgetron
  pullPolicy: IfNotPresent
  tag: "1.0.0"

imagePullSecrets: []
nameOverride: ""
fullnameOverride: ""

serviceAccount:
  create: true
  annotations: {}
  name: ""

podAnnotations: {}

podSecurityContext:
  runAsNonRoot: true
  runAsUser: 1000
  fsGroup: 1000

securityContext:
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities:
    drop:
      - ALL

service:
  type: ClusterIP
  port: 8080
  metricsPort: 9090

ingress:
  enabled: false
  className: ""
  annotations: {}
  hosts: []
  tls: []

resources:
  limits:
    cpu: 2
    memory: 4Gi
  requests:
    cpu: 500m
    memory: 1Gi

autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70
  targetQueueDepth: 5

podDisruptionBudget:
  enabled: true
  minAvailable: 1

nodeSelector: {}

tolerations: []

affinity: {}

# Gadgetron 설정
config:
  server:
    bind: "0.0.0.0"
    port: 8080
    metricsPort: 9090
    workers: 4
  log:
    level: "info"
    format: "json"
  tracing:
    enabled: true
    endpoint: "http://tempo:4317"
    sampleRate: 0.1

# 프로바이더 설정
providers:
  ollama:
    enabled: false
    endpoint: "http://ollama:11434"
  vllm:
    enabled: true
    endpoint: "http://vllm:8000"
  sglang:
    enabled: false
    endpoint: "http://sglang:30000"

# 보안 설정
security:
  apiKeyEnabled: true
  existingSecret: ""       # 기존 Secret 사용 시 지정
  rateLimitRps: 100
  maxRequestSizeMb: 10

# GPU 설정
gpu:
  enabled: true
  runtime: nvidia
  timeSlicing:
    enabled: false
    replicas: 1
```

## 부록 C: 전체 아키텍처 다이어그램

```
                          ┌─────────────────────────────────────────┐
                          │            클라이언트 (SDK/API)          │
                          └─────────────────┬───────────────────────┘
                                            │
                          ┌─────────────────▼───────────────────────┐
                          │         로드 밸런서 / 인그레스           │
                          └─────────────────┬───────────────────────┘
                                            │
                ┌───────────────────────────▼───────────────────────────┐
                │                    Gadgetron 클러스터                   │
                │  ┌───────────────────────────────────────────────────┐ │
                │  │              API 게이트웨이 / 라우터               │ │
                │  │  ┌──────────┐ ┌──────────┐ ┌───────────────────┐ │ │
                │  │  │   인증   │ │ 레이트    │ │  라우팅 정책     │ │ │
                │  │  │  미들웨어│ │ 리밋     │ │ (least-conn 등) │ │ │
                │  │  └──────────┘ └──────────┘ └───────────────────┘ │ │
                │  └───────────────────────┬───────────────────────────┘ │
                │                          │                             │
                │  ┌───────────────────────▼───────────────────────────┐ │
                │  │              프로바이더 레이어                      │ │
                │  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ │ │
                │  │  │ Ollama  │ │  vLLM   │ │ SGLang  │ │ TRT-LLM │ │ │
                │  │  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘ │ │
                │  └───────┼───────────┼───────────┼───────────┼──────┘ │
                │          │           │           │           │        │
                │  ┌───────▼───────────▼───────────▼───────────▼──────┐ │
                │  │              GPU 리소스 관리                       │ │
                │  │  ┌──────────────────────────────────────────────┐ │ │
                │  │  │  K8s 스케줄러 / Slurm 통합 / 직접 관리      │ │ │
                │  │  └──────────────────────────────────────────────┘ │ │
                │  └──────────────────────────────────────────────────┘ │
                │                                                      │
                │  ┌──────────────────────────────────────────────────┐ │
                │  │              관측성 스택                          │ │
                │  │  ┌───────────┐ ┌──────────┐ ┌──────────────────┐ │ │
                │  │  │Prometheus │ │   Tempo   │ │   Loki / ELK    │ │ │
                │  │  │ /metrics  │ │ (traces) │ │    (logs)       │ │ │
                │  │  └─────┬─────┘ └─────┬────┘ └───────┬─────────┘ │ │
                │  │        └──────────────┼──────────────┘           │ │
                │  │                       ▼                          │ │
                │  │                ┌────────────┐                    │ │
                │  │                │  Grafana    │                    │ │
                │  │                └────────────┘                    │ │
                │  └──────────────────────────────────────────────────┘ │
                └──────────────────────────────────────────────────────┘
```

---

*이 문서는 Gadgetron 프로젝트의 배포 및 운영 설계를 정의합니다. 각 섹션은 구현 단계에서 상세 기술 사양으로 보완됩니다.*
