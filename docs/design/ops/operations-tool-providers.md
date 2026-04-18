# Operations Tool Providers for Infra / Scheduler / Cluster / Storage / Workload

> **담당**: @devops-sre-lead
> **상태**: Draft
> **작성일**: 2026-04-15
> **최종 업데이트**: 2026-04-15
> **관련 크레이트**: `gadgetron-core`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-xaas`, `gadgetron-scheduler`, `gadgetron-node`, future `gadgetron-infra`, future `gadgetron-scheduler-tools`, future `gadgetron-cluster`
> **Phase**: [P2C] / [P3] / [P4]
> **관련 문서**: `docs/design/ops/agentic-cluster-collaboration.md`, `docs/design/phase2/04-mcp-tool-registry.md`, `docs/architecture/platform-architecture.md`
>
> **Canonical terminology note**: this draft retains "tool provider" wording, but the current canonical seam is `GadgetProvider` / `GadgetRegistry`. Historical `McpToolProvider` / `McpToolRegistry` names below must be read through that mapping.

---

## 1. 철학 & 컨셉 (Why)

### 1.1 문제 한 문장

현재 Gadgetron은 operations/execution substrate 를 이미 갖고 있지만, assistant plane 이 실제 cluster operator 로 진화하려면 lower-plane 기능을 명시적인 `GadgetProvider` families 로 분해한 canonical 설계가 필요하다.

### 1.2 제품 비전과의 연결

`docs/design/ops/agentic-cluster-collaboration.md` 는 Assistant / Operations / Execution plane 을 정의한다. 이 문서는 그중 **Operations Plane + Execution Plane 을 agent 가 사용할 수 있는 tool families 로 분해**한다.

목표는 단순한 `kubectl`/`slurm` wrapper 가 아니다. 아래를 동시에 만족해야 한다.

1. 사람이 읽을 수 있는 설명과 evidence 를 남긴다
2. 정책과 approval tier 를 통과한다
3. direct human action 과 delegated agent action 이 같은 audit model 로 남는다
4. 나중에 runbook / automation 으로 승격 가능하다

### 1.3 채택하지 않은 대안

| 대안 | 설명 | 채택하지 않은 이유 |
|------|------|--------------------|
| **A. 하나의 mega `cluster.*` namespace** | infra/scheduler/storage/workload/virtualization 모두 한 provider 에 몰아넣음 | 권한/테스트/백엔드 책임이 섞여 drift 와 privilege creep 를 유발한다 |
| **B. raw shell passthrough** | `kubectl ...`, `helm ...`, `sbatch ...` 를 문자열로 바로 실행 | audit/validation/policy/rollback 가 불가능해지고 prompt injection 경계가 약해진다 |
| **C. provider families + backend adapters** | domain 별 provider 와 backend trait 를 분리 | 가장 명시적이고 phase-by-phase landing 이 가능하다 |

채택: **C. provider families + backend adapters**

### 1.4 설계 원칙과 trade-off

1. **Evidence-first operations**: 모든 조치는 target, before-state, after-state, rationale 를 남긴다.
2. **Read before mutate**: T2/T3 작업은 항상 T1 진단 도구와 함께 설계한다.
3. **Backend abstraction over shell strings**: 에이전트는 semantic action 을 요청하고, backend adapter 가 이를 API/CLI 로 번역한다.
4. **No generic root shell tool**: cluster administration 은 typed tools 로 제한한다.
5. **Phase-by-phase widening**: infra → scheduler/cluster/workload → virtualization 순서로 확장한다.

Trade-off:

- typed adapters 는 구현 비용이 높다
- 그러나 raw passthrough 보다 안전하고, 경험 축적/재사용이 훨씬 쉽다

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

#### 2.1.1 Provider families

```rust
pub struct InfraToolProvider {
    // in-process reads and reversible mutations over gadgetron runtime
}

pub struct SchedulerToolProvider<B: SchedulerBackend> {
    backend: B,
}

pub struct ClusterToolProvider<B: ClusterBackend> {
    backend: B,
}

pub struct StorageToolProvider<B: StorageBackend> {
    backend: B,
}

pub struct WorkloadToolProvider<B: WorkloadBackend> {
    backend: B,
}

pub struct VirtualizationToolProvider<B: VirtualizationBackend> {
    backend: B,
}
```

Each provider implements `GadgetProvider`.

#### 2.1.2 Backend traits

```rust
pub trait SchedulerBackend: Send + Sync {
    fn plan_workload(&self, req: WorkloadPlanRequest) -> Result<WorkloadPlan, OpsError>;
    fn rebalance(&self, target: RebalanceTarget) -> Result<MutationReceipt, OpsError>;
}

pub trait ClusterBackend: Send + Sync {
    fn list_nodes(&self, scope: ClusterScope) -> Result<Vec<ClusterNode>, OpsError>;
    fn get_resource(&self, target: ResourceRef) -> Result<ClusterResource, OpsError>;
    fn cordon_node(&self, node: &str) -> Result<MutationReceipt, OpsError>;
    fn apply_manifest(&self, manifest: AppliedManifest) -> Result<MutationReceipt, OpsError>;
}

pub trait StorageBackend: Send + Sync {
    fn list_pvcs(&self, scope: ClusterScope) -> Result<Vec<PersistentVolumeClaim>, OpsError>;
    fn expand_pvc(&self, req: ExpandPvcRequest) -> Result<MutationReceipt, OpsError>;
    fn get_volume_events(&self, target: ResourceRef) -> Result<Vec<StorageEvent>, OpsError>;
}

pub trait WorkloadBackend: Send + Sync {
    fn submit(&self, req: WorkloadRequest) -> Result<WorkloadHandle, OpsError>;
    fn status(&self, handle: &WorkloadHandle) -> Result<WorkloadStatus, OpsError>;
    fn cancel(&self, handle: &WorkloadHandle) -> Result<MutationReceipt, OpsError>;
    fn collect(&self, handle: &WorkloadHandle) -> Result<WorkloadArtifactSet, OpsError>;
}

pub trait VirtualizationBackend: Send + Sync {
    fn list_instances(&self, scope: ClusterScope) -> Result<Vec<VirtualInstance>, OpsError>;
    fn start_instance(&self, id: &str) -> Result<MutationReceipt, OpsError>;
    fn stop_instance(&self, id: &str) -> Result<MutationReceipt, OpsError>;
}
```

#### 2.1.3 Shared value objects

```rust
pub struct ResourceRef {
    pub backend: OpsBackendKind,
    pub namespace: Option<String>,
    pub kind: String,
    pub name: String,
}

pub enum OpsBackendKind {
    InProcess,
    Kubernetes,
    Slurm,
    KubeVirt,
}

pub struct MutationReceipt {
    pub change_id: uuid::Uuid,
    pub target: ResourceRef,
    pub summary: String,
}

pub struct WorkloadRequest {
    pub workload_kind: WorkloadKind,
    pub objective: String,
    pub resource_hints: ResourceHints,
    pub artifacts_path: Option<String>,
}

pub enum WorkloadKind {
    Inference,
    Batch,
    Evaluation,
}
```

#### 2.1.4 Namespace and tier map

| Namespace | Example tool | Tier | Phase |
|-----------|--------------|------|-------|
| `infra.*` | `infra.list_nodes` | T1 | P2C |
| `infra.*` | `infra.set_routing_strategy` | T2 | P2C |
| `infra.*` | `infra.undeploy_model` | T3 | P2C |
| `scheduler.*` | `scheduler.plan_workload` | T1 | P3 |
| `scheduler.*` | `scheduler.rebalance` | T2 | P3 |
| `cluster.*` | `cluster.list_nodes` | T1 | P3 |
| `cluster.*` | `cluster.cordon_node` | T2 | P3 |
| `cluster.*` | `cluster.apply_manifest` | T3 | P3 |
| `storage.*` | `storage.list_pvcs` | T1 | P3 |
| `storage.*` | `storage.expand_pvc` | T2 | P3 |
| `storage.*` | `storage.delete_pvc` | T3 | P3 |
| `workload.*` | `workload.submit` | T2 | P3 |
| `workload.*` | `workload.cancel` | T3 | P3 |
| `virtualization.*` | `virtualization.list_instances` | T1 | P4 |
| `virtualization.*` | `virtualization.stop_instance` | T2 | P4 |
| `virtualization.*` | `virtualization.delete_instance` | T3 | P4 |

### 2.2 내부 구조

#### 2.2.1 Family split

Provider families are intentionally split by **authority boundary** and **backend coupling**.

| Family | Backing system | Why separate |
|--------|----------------|-------------|
| `infra.*` | in-process Gadgetron runtime | no external control plane; fastest path; low latency |
| `scheduler.*` | internal scheduler or scheduler adapters | planning / placement semantics differ from raw cluster control |
| `cluster.*` | Kubernetes / Slurm substrate | substrate-wide actions need explicit backend and stronger audit |
| `storage.*` | PVC / StorageClass / CSI views | volume lifecycle and failure modes differ from pods/nodes |
| `workload.*` | jobs / inference / batch execution | result artifacts and cancellation semantics need dedicated modeling |
| `virtualization.*` | KubeVirt / VM substrate | highest privilege and slowest maturity path |

#### 2.2.2 Backend choices

Chosen defaults:

- **Kubernetes**: `kube` + `k8s-openapi`, not raw `kubectl` passthrough
- **Slurm**: thin wrapper over `sinfo`, `scontrol`, `sbatch`, `squeue`, `scancel`
- **Storage**: K8s PVC/StorageClass/Events first; CSI diagnostics read-only first
- **Virtualization**: deferred until after storage/workload land

This gives stronger validation and structured evidence than generic shell tools.

#### 2.2.3 Dry-run and shadow modes

Every T2/T3 tool family must support at least one of:

- `dry_run = true`
- `plan_only = true`
- `shadow_only = true`

The agent’s default operations loop is:

1. diagnose with T1 tools
2. propose a mutation with plan/dry-run
3. request approval
4. execute with evidence capture
5. emit structured summary

#### 2.2.4 Experience capture

Each provider family returns enough structured data to be reusable.

```rust
pub struct ToolEvidence {
    pub tool_name: String,
    pub backend: OpsBackendKind,
    pub target: Option<ResourceRef>,
    pub before_summary: Option<String>,
    pub after_summary: Option<String>,
    pub artifact_refs: Vec<String>,
}
```

This is what allows incident history to become runbook candidates later.

### 2.3 설정 스키마

```toml
[agent.tools.infra]
enabled = true
default_mode = "ask"

[agent.tools.scheduler]
enabled = false
default_mode = "ask"

[agent.tools.cluster]
enabled = false
default_mode = "ask"

[agent.tools.storage]
enabled = false
default_mode = "ask"

[agent.tools.workload]
enabled = false
default_mode = "ask"

[agent.tools.virtualization]
enabled = false
default_mode = "never"

[cluster.kubernetes]
enabled = true
kubeconfig = "~/.kube/config"
context = "prod"
namespace_allowlist = ["gadgetron", "ml"]

[cluster.slurm]
enabled = false
sinfo_path = "/usr/bin/sinfo"
scontrol_path = "/usr/bin/scontrol"
sbatch_path = "/usr/bin/sbatch"
squeue_path = "/usr/bin/squeue"
scancel_path = "/usr/bin/scancel"

[cluster.storage]
enabled = true
allow_resize = true
allow_delete = false

[cluster.workload]
enabled = true
default_backend = "internal"
artifact_root = "/var/lib/gadgetron/workloads"

[cluster.virtualization]
enabled = false
backend = "kubevirt"
```

Validation rules:

- `virtualization.enabled = true` is startup error before P4
- `cluster.storage.enabled = true` requires `cluster.kubernetes.enabled = true`
- `cluster.workload.default_backend = "slurm"` requires `cluster.slurm.enabled = true`
- any T3 family cannot set `default_mode = "auto"`
- `namespace_allowlist` cannot be empty when kubernetes backend is enabled

### 2.4 에러 & 로깅

Introduce a shared error family for operations tools.

```rust
pub enum OpsErrorKind {
    UnsupportedBackend,
    TargetNotFound,
    BackendUnavailable,
    ValidationFailed,
    PolicyRejected,
    MutationFailed,
}
```

Mapped outward as `McpError::Internal` or `McpError::ToolDisabled` with stable `error_code`.

Required log fields:

- `request_id`
- `tenant_id`
- `actor_id`
- `tool_name`
- `backend`
- `target_kind`
- `target_name`
- `approval_id`
- `dry_run`
- `change_id`

Required spans:

- `ops.tool.dispatch`
- `ops.tool.dry_run`
- `ops.tool.execute`
- `ops.tool.evidence`
- `ops.workload.collect`

#### 2.4.1 Security & Threat Model (STRIDE)

This section is required for Round 1.5 per `docs/process/03-review-rubric.md §1.5-A`.

**Assets**

| Asset | Sensitivity | Owner |
|------|-------------|-------|
| cluster credentials (`kubeconfig`, Slurm control access) | Critical | Administrator |
| mutation authority over nodes / workloads / storage | Critical | Administrator / Policy |
| workload artifacts and execution outputs | High | User / Operator |
| tool evidence and audit receipts | High | Operator |
| namespace allowlists and tool mode policy | High | Administrator |

**Trust boundaries**

| ID | Boundary | Crosses | Auth mechanism |
|----|----------|---------|----------------|
| B-O1 | agent runtime → `GadgetRegistry` | in-process tool dispatch | tenant/auth context from gateway |
| B-O2 | provider → in-process Gadgetron runtime | runtime mutation/read path | internal Rust API |
| B-O3 | provider → Kubernetes API | external cluster control plane | kubeconfig / service account |
| B-O4 | provider → Slurm CLI/backend | external scheduler boundary | local OS user / scheduler ACL |
| B-O5 | provider → artifact / storage backend | external persistence boundary | filesystem / backend policy |

**STRIDE table**

| Component | S | T | R | I | D | E | Highest unmitigated risk |
|-----------|---|---|---|---|---|---|--------------------------|
| `InfraToolProvider` | Low | Medium — runtime knobs can be changed | Low | Medium — internal state exposure | Medium | Medium | reversible writes executed without adequate evidence |
| `ClusterToolProvider` | Medium — backend identity matters | High — cluster-wide mutation surface | Medium | Medium | High — bad mutation can disrupt workloads | High | destructive substrate changes |
| `StorageToolProvider` | Medium | High — PVC resize/delete impacts durability | Medium | High — storage events may expose sensitive paths | High | High | irreversible data-loss path if T3 leaks into auto |
| `WorkloadToolProvider` | Low | Medium — wrong submission target or artifact path | Medium | High — result artifacts may contain tenant data | Medium | Medium | artifact leakage or wrong-target execution |
| `VirtualizationToolProvider` | Medium | High | Medium | Medium | High | High | VM lifecycle control before policy model matures |

**Mitigations**

| ID | Mitigation | Location |
|----|------------|----------|
| M-O1 | no raw shell passthrough; all mutations go through typed adapters | §1.3, §1.4 |
| M-O2 | T3 namespaces can never use `default_mode = "auto"` | §2.3 validation rules |
| M-O3 | read-before-mutate flow with `dry_run` / `plan_only` / `shadow_only` required for T2/T3 families | §2.2.3 |
| M-O4 | every mutation emits `MutationReceipt` and `ToolEvidence` with `change_id` for auditability | §2.1.3, §2.2.4 |
| M-O5 | namespace allowlists and backend enablement are startup-validated | §2.3 validation rules |
| M-O6 | virtualization remains deferred until storage/workload approval boundaries are proven | §2.6 |

### 2.5 의존성

| Family | Primary dependencies | Justification |
|--------|----------------------|---------------|
| `infra.*` | none beyond existing workspace | in-process runtime reads/writes |
| `cluster.*` Kubernetes | `kube`, `k8s-openapi` | typed API access + apply/patch support |
| `cluster.*` Slurm | no new crate required initially | thin wrapper over stable CLI tools |
| `storage.*` | `kube`, `k8s-openapi` | PVC / StorageClass / Events |
| `workload.*` | same as backend | unified request/status abstraction |
| `virtualization.*` | deferred | avoid overcommitting backend choice too early |

### 2.6 구현 순서

Implementation should follow the smallest authority expansion that still produces user-visible value.

1. **`InfraToolProvider` first**
   - read-only runtime inspection tools
   - reversible in-process mutations with evidence capture
2. **shared ops contracts**
   - `OpsBackendKind`, `ResourceRef`, `MutationReceipt`, `ToolEvidence`, `OpsErrorKind`
   - tier validators and `default_mode` startup checks
3. **`ClusterToolProvider` + `StorageToolProvider`**
   - Kubernetes typed adapters first
   - read paths before mutate paths, dry-run before execute
4. **`WorkloadToolProvider`**
   - submit/status/collect lifecycle
   - artifact and evidence model wired into audit/report pipeline
5. **`SchedulerToolProvider`**
   - planning/rebalance once workload submission and cluster evidence exist
6. **`VirtualizationToolProvider` last**
   - only after storage/workload approval boundaries are proven in production-like tests

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 연결 구조

```text
administrator / user
        │
        ▼
PennyProvider / CollaborationCoordinator
        │
        ▼
GadgetRegistry
   ├── KnowledgeGadgetProvider
   ├── InfraToolProvider
   ├── SchedulerToolProvider
   ├── ClusterToolProvider
   ├── StorageToolProvider
   ├── WorkloadToolProvider
   └── VirtualizationToolProvider
        │
        ├── in-process runtime (router/scheduler/node)
        ├── Kubernetes API
        ├── Slurm CLI
        └── artifact / event stores
```

### 3.2 Crate boundaries

- `gadgetron-core`
  - shared traits / enums / value objects only
- `gadgetron-penny`
  - registry + orchestration only
- `gadgetron-infra`
  - in-process runtime tool family
- `gadgetron-scheduler-tools`
  - planning / rebalance tool family
- `gadgetron-cluster`
  - cluster / storage / workload / virtualization adapters

No kube/slurm dependency may enter `gadgetron-core`.

### 3.3 타 도메인과의 인터페이스 계약

- `docs/design/phase2/04-mcp-tool-registry.md`
  - owns the generic trait and tier model
- `docs/architecture/platform-architecture.md`
  - remains canonical for substrate topology and runtime state
- `docs/design/ops/agentic-cluster-collaboration.md`
  - remains canonical for product-level plane framing

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 대상 | 검증 invariant |
|------|----------------|
| namespace-to-tier map | T3 tool이 `auto`로 노출되지 않아야 함 |
| backend validators | unsupported backend 조합이 startup error 여야 함 |
| adapter serializers | ResourceRef / MutationReceipt / WorkloadRequest round-trip 보장 |
| dry-run path | no mutation side effect when `dry_run = true` |
| evidence builder | before/after summaries 누락 없이 생성되어야 함 |

### 4.2 테스트 하네스

- fake in-process runtime for `infra.*`
- fake kubernetes backend
- fake slurm backend
- fake workload backend

Named tests:

- `infra_read_tools_are_t1`
- `cluster_apply_is_t3`
- `storage_delete_never_auto`
- `virtualization_family_rejected_before_p4`
- `workload_submit_returns_artifact_handle`
- `dry_run_never_mutates_backend`

### 4.3 커버리지 목표

- line coverage 85%+
- branch coverage 75%+
- tier validation and backend validation 90%+

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

1. **infra T1/T2 flow**
   - `infra.list_nodes` + `infra.set_routing_strategy`
   - approval gate respected

2. **cluster + storage flow**
   - `cluster.list_nodes`
   - `storage.list_pvcs`
   - `storage.expand_pvc` dry-run and execute

3. **workload flow**
   - `workload.submit`
   - `workload.status`
   - `workload.collect`

4. **incident-to-runbook evidence**
   - tool call
   - evidence capture
   - runbook draft candidate generated

### 5.2 테스트 환경

- fake Kubernetes API server
- fake Slurm command directory
- fake artifact store
- in-process scheduler/node fixtures

### 5.3 회귀 방지

Tests must fail if:

- T3 tools become auto-executable
- raw shell passthrough bypasses typed adapters
- evidence is not emitted for mutations
- storage or workload tools activate without the required backend

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| `InfraToolProvider` | [P2C] |
| `SchedulerToolProvider` | [P3] |
| `ClusterToolProvider` | [P3] |
| `StorageToolProvider` | [P3] |
| `WorkloadToolProvider` | [P3] |
| `VirtualizationToolProvider` | [P4] |
| multi-cluster federation / org-wide policy propagation | [P4+] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID  | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| Q-1 | Kubernetes destructive path 를 `apply/patch`까지 typed API 로 제한할지 | A: typed only / B: `kubectl` passthrough 허용 | A — 감사와 정책 일관성 보존 | 🟡 PM 검토 요청 |
| Q-2 | Slurm jobs 를 `cluster.*` 에 둘지 `workload.*` 에 둘지 | A: cluster / B: workload | B — 사용자 가치가 job lifecycle 과 artifact 회수에 더 가깝다 | 🟡 PM 검토 요청 |
| Q-3 | virtualization family 도입 시점 | A: P3 / B: P4 | B — storage/workload 먼저 닫는 것이 현실적 | 🟡 PM 검토 요청 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-15 — 예정
**결론**: 미실시

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [ ] 인터페이스 계약
- [ ] 크레이트 경계
- [ ] 타입 중복
- [ ] 에러 반환
- [ ] 동시성
- [ ] 의존성 방향
- [ ] Phase 태그
- [ ] 레거시 결정 준수

**다음 라운드 조건**: Round 1 리뷰어(@chief-architect, @devops-sre-lead) 검토 후

### Round 1.5 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §1.5` 기준)

### Round 2 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §2` 기준)

### Round 3 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §3` 기준)
