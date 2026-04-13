# Sprint 9: NodeAgent Process Lifecycle, Scheduler-Node Connection, VRAM Eviction Execution, Provider 0 Warning

> **담당**: @gpu-scheduler-lead
> **상태**: ✅ Implemented (commit `02218b9`, 2026-04-12) — NodeAgent process lifecycle + Scheduler deploy + VRAM eviction 실행 경로 연결 완료
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-node`, `gadgetron-scheduler`, `gadgetron-cli`, `gadgetron-core`
> **Phase**: [P1]

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 스프린트가 해결하는 문제

Sprint 9는 Phase 1 MVP에서 실행 가능한 프로세스 관리 파이프라인이 전혀 작동하지 않는 4개의 핵심 공백을 닫는다.

**GAP-1 (NodeAgent 프로세스 생명주기)**: `gadgetron-node/src/agent.rs`의 모든 spawn 함수(`start_vllm_model`, `start_sglang_model`, `start_llamacpp_model`, `start_tgi_model`)에서 `cmd.spawn()`의 반환값인 `Child` 핸들을 즉시 버린다. 따라서 Gadgetron이 재시작하면 고아 프로세스가 무한히 쌓인다. 또한 포트가 `8000`, `8080`, `30000` 등 하드코딩되어 있어 같은 노드에 두 번째 모델을 올리면 포트 충돌로 spawn이 실패한다. `stop_model()`은 내부 목록에서만 항목을 제거하고 실제 프로세스를 종료하지 않는다.

**GAP-2 (Scheduler → NodeAgent 연결 부재)**: `gadgetron-scheduler/src/scheduler.rs`의 `deploy()`는 스케줄링 결정을 내린 후 `deployments` HashMap에만 삽입한다. 실제로 `NodeAgent::start_model()`을 호출하지 않으므로 추론 엔진 프로세스가 전혀 기동되지 않는다.

**GAP-3 (VRAM 퇴출 미실행)**: `find_eviction_candidate()`는 후보를 반환하지만 `deploy()`는 이를 사용하지 않는다. VRAM이 부족하면 즉시 `GadgetronError::Routing` 에러를 반환하고 끝난다. 퇴출 → 재할당 루프가 실행되지 않는다.

**GAP-4 (Provider 0 무음 경고 부재)**: `build_providers()`가 빈 `HashMap`을 반환해도 서버는 정상 기동된다. 운영자는 라우터에서 503이 나오기 전까지 provider가 없다는 사실을 알 수 없다.

### 1.2 제품 비전 연결

`docs/00-overview.md §1`은 "Rust-native GPU/LLM orchestration"을 핵심 목표로 명시한다. 현재 상태는 오케스트레이션이 없고 단순 레코드 삽입만 존재한다. 이 스프린트는 실제 프로세스 생성·종료·포트 관리·퇴출 루프를 구현하여 MVP의 골격을 완성한다. D-20260411-04 Phase 1 포함 범위 중 "LRU eviction + First-Fit Decreasing (GPU ≤ 90%)"과 직결된다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 항목 | 대안 | 채택하지 않은 이유 |
|------|------|------|
| Scheduler → NodeAgent 연결 방식 | HTTP REST API로 분리 | Phase 1에서는 단일 프로세스 내 in-process 호출이 더 단순하고 네트워크 레이턴시가 없음. HTTP 분리는 Phase 2 멀티노드 확장 시 적용 |
| 포트 할당 전략 | OS가 임의 포트 배정 (`port 0`) | OS 배정 포트를 외부에 노출하는 흐름이 복잡. Gadgetron이 포트 범위를 직접 관리해야 TUI/API에서 포트 정보를 확정적으로 제공 가능 |
| `Child` 핸들 저장소 | `std::sync::Mutex<HashMap>` | `DashMap`이 세분화된 샤드 잠금으로 동시 모델 기동 시 병목 최소화. `gadgetron-scheduler`가 이미 `dashmap`을 의존성에 포함 |
| SIGTERM 대기 시간 | 10초 | 모델 서버는 추론 요청 드레이닝에 보통 3-5초 소요. 5초로 충분하며 더 짧게 하면 요청 손실 위험 |
| Provider 0 처리 방식 | 에러로 기동 중단 | 설정 없이도 서버가 떠서 `/health` 와 `/api/v1/nodes` 는 정상 응답해야 한다. 경고만 출력하고 기동 계속이 맞음 |

### 1.4 핵심 설계 원칙

- **결정론적 구현**: 모든 타입 시그니처, 포트 범위, 타임아웃 값, 에러 variant가 이 문서에 완전히 명시된다. 이 문서만 읽으면 같은 코드가 나와야 한다.
- **D-12 경계 준수**: `PortPool`은 `gadgetron-node`에 배치한다. `ManagedProcess`도 `gadgetron-node`에 배치한다. `gadgetron-core`의 `NodeErrorKind::PortAllocationFailed`를 재사용한다.
- **D-10 ModelState**: `deploy()` 성공 시 `ModelState::Running`으로 즉시 업데이트하고, pid와 port를 `ModelDeployment`에 반영한다.
- **고아 프로세스 방지**: `ManagedProcess`가 Drop될 때 또는 `NodeAgent`가 종료될 때 모든 Child가 반드시 정리된다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

#### 2.1.1 GAP-1: `ManagedProcess` 구조체 (`gadgetron-node/src/process.rs` 신규)

```rust
// gadgetron-node/src/process.rs

use tokio::process::Child;

/// A running inference engine process owned by NodeAgent.
///
/// Dropping this struct does NOT kill the child process — explicit stop() is required.
/// The child handle is stored as Option so it can be taken out for graceful shutdown.
pub struct ManagedProcess {
    /// OS process ID of the spawned inference engine.
    pub pid: u32,
    /// Port this process is listening on, allocated from PortPool.
    pub port: u16,
    /// Model ID this process serves.
    pub model_id: String,
    /// Owned child handle. `None` after process has been awaited/killed.
    child: Option<Child>,
}

impl ManagedProcess {
    /// Construct from a successfully spawned child.
    ///
    /// Panics if `child.id()` returns `None` (child already exited before construction).
    pub fn new(model_id: String, port: u16, child: Child) -> Self {
        let pid = child.id().expect("child must have pid at construction");
        Self {
            pid,
            port,
            model_id,
            child: Some(child),
        }
    }

    /// Send SIGTERM; wait up to `timeout` for clean exit; send SIGKILL if still alive.
    ///
    /// After this call, the internal child handle is consumed and set to None.
    /// Returns Ok(()) in all cases where the process is no longer running.
    pub async fn stop(&mut self, timeout: std::time::Duration) -> gadgetron_core::error::Result<()> {
        use gadgetron_core::error::{GadgetronError, NodeErrorKind};

        let Some(ref mut child) = self.child else {
            // Already stopped; idempotent.
            return Ok(());
        };

        // SIGTERM
        #[cfg(unix)]
        {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;
            let _ = signal::kill(Pid::from_raw(self.pid as i32), Signal::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            // On non-unix (Windows dev builds), fall through to kill() immediately.
            let _ = child.kill().await;
        }

        // Wait up to timeout for graceful exit.
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(_) => {},
                    Err(e) => tracing::warn!(
                        model_id = %self.model_id,
                        pid = self.pid,
                        "wait() error after SIGTERM: {e}"
                    ),
                }
            },
            _ = &mut deadline => {
                // Timeout — escalate to SIGKILL.
                tracing::warn!(
                    model_id = %self.model_id,
                    pid = self.pid,
                    "SIGTERM timeout, sending SIGKILL"
                );
                if let Err(e) = child.kill().await {
                    return Err(GadgetronError::Node {
                        kind: NodeErrorKind::ProcessKillFailed,
                        message: format!("SIGKILL failed for pid {}: {e}", self.pid),
                    });
                }
                let _ = child.wait().await;
            }
        }

        self.child = None;
        Ok(())
    }
}
```

#### 2.1.2 GAP-1: `PortPool` 구조체 (`gadgetron-node/src/port_pool.rs` 신규)

```rust
// gadgetron-node/src/port_pool.rs

use std::collections::BTreeSet;
use std::sync::Mutex;
use gadgetron_core::error::{GadgetronError, NodeErrorKind, Result};

/// Thread-safe port allocator over a fixed range [base, base + capacity).
///
/// Range: 30000–39999 (10 000 ports).
/// Ports are returned to the pool on release, enabling reuse without restart.
pub struct PortPool {
    available: Mutex<BTreeSet<u16>>,
}

impl PortPool {
    /// Create a pool covering `[base, base + capacity)`.
    ///
    /// # Panics
    /// Panics if `base as u32 + capacity > 65535`.
    pub fn new(base: u16, capacity: u16) -> Self {
        assert!(
            base as u32 + capacity as u32 <= 65535,
            "port range exceeds u16 max"
        );
        let available = (base..base + capacity).collect::<BTreeSet<_>>();
        Self {
            available: Mutex::new(available),
        }
    }

    /// Allocate the lowest available port.
    ///
    /// Returns `Err(NodeErrorKind::PortAllocationFailed)` when the pool is exhausted.
    pub fn allocate(&self) -> Result<u16> {
        let mut set = self.available.lock().unwrap();
        set.pop_first().ok_or_else(|| GadgetronError::Node {
            kind: NodeErrorKind::PortAllocationFailed,
            message: "port pool exhausted (all 10 000 ports in use)".to_string(),
        })
    }

    /// Return a port to the pool.
    ///
    /// Silently ignores ports outside the original range (defensive).
    pub fn release(&self, port: u16) {
        let mut set = self.available.lock().unwrap();
        set.insert(port);
    }

    /// Number of ports currently available.
    #[cfg(test)]
    pub fn available_count(&self) -> usize {
        self.available.lock().unwrap().len()
    }
}

/// Default pool: 30000–39999.
impl Default for PortPool {
    fn default() -> Self {
        Self::new(30_000, 10_000)
    }
}
```

#### 2.1.3 GAP-1: `NodeAgent` 변경 (`gadgetron-node/src/agent.rs`)

`NodeAgent`의 필드와 `start_model` / `stop_model` 시그니처를 다음으로 교체한다.

```rust
// gadgetron-node/src/agent.rs  (변경 후)

use dashmap::DashMap;
use std::sync::Arc;
use gadgetron_core::error::{GadgetronError, NodeErrorKind, Result};
use gadgetron_core::model::{InferenceEngine, ModelDeployment};
use gadgetron_core::node::{NodeConfig, NodeResources, NodeStatus};

use crate::monitor::ResourceMonitor;
use crate::port_pool::PortPool;
use crate::process::ManagedProcess;

pub struct NodeAgent {
    config: NodeConfig,
    monitor: ResourceMonitor,
    /// Keyed by model_id (String). DashMap allows concurrent reads during health checks.
    /// Each value is wrapped in tokio::sync::Mutex so that stop() can be called
    /// concurrently on different models without blocking the entire map.
    processes: Arc<DashMap<String, tokio::sync::Mutex<ManagedProcess>>>,
    port_pool: Arc<PortPool>,
}

impl NodeAgent {
    pub fn new(config: NodeConfig) -> Self {
        Self {
            config,
            monitor: ResourceMonitor::new(),
            processes: Arc::new(DashMap::<String, tokio::sync::Mutex<ManagedProcess>>::new()),
            port_pool: Arc::new(PortPool::default()),
        }
    }

    pub fn id(&self) -> &str { &self.config.id }
    pub fn endpoint(&self) -> &str { &self.config.endpoint }

    pub fn collect_metrics(&mut self) -> NodeResources {
        self.monitor.collect()
    }

    pub fn status(&mut self) -> NodeStatus {
        let resources = self.collect_metrics();
        let running_models: Vec<String> = self.processes
            .iter()
            .map(|e| e.key().clone())
            .collect();
        NodeStatus {
            id: self.config.id.clone(),
            endpoint: self.config.endpoint.clone(),
            healthy: true,
            resources,
            running_models,
            last_heartbeat: chrono::Utc::now(),
        }
    }

    /// Start a model process on this node.
    ///
    /// Allocates a port from the pool, spawns the inference engine, stores the
    /// `ManagedProcess` in `self.processes`. Returns the assigned port on success.
    /// On spawn failure the port is released back to the pool.
    pub async fn start_model(&self, deployment: &ModelDeployment) -> Result<u16> {
        if self.processes.contains_key(&deployment.id) {
            // Already running — return existing port.
            let port = self.processes
                .get(&deployment.id)
                .unwrap()
                .lock()
                .await
                .port;
            return Ok(port);
        }

        let port = self.port_pool.allocate()?;

        let spawn_result = match deployment.engine {
            InferenceEngine::Ollama   => self.spawn_ollama(deployment).await,
            InferenceEngine::Vllm     => self.spawn_vllm(deployment, port).await,
            InferenceEngine::Sglang   => self.spawn_sglang(deployment, port).await,
            InferenceEngine::LlamaCpp => self.spawn_llamacpp(deployment, port).await,
            InferenceEngine::Tgi      => self.spawn_tgi(deployment, port).await,
            _ => Err(GadgetronError::Node {
                kind: NodeErrorKind::ProcessSpawnFailed,
                message: format!("unsupported engine: {:?}", deployment.engine),
            }),
        };

        match spawn_result {
            Ok(child) => {
                let process = ManagedProcess::new(deployment.id.clone(), port, child);
                tracing::info!(
                    model_id = %deployment.id,
                    pid      = process.pid,
                    port     = port,
                    engine   = ?deployment.engine,
                    "model process started"
                );
                self.processes.insert(
                    deployment.id.clone(),
                    tokio::sync::Mutex::new(process),
                );
                Ok(port)
            }
            Err(e) => {
                self.port_pool.release(port);
                Err(e)
            }
        }
    }

    /// Stop a model process: SIGTERM → 5 s wait → SIGKILL → release port.
    ///
    /// Idempotent: calling stop on an already-stopped model returns Ok(()).
    pub async fn stop_model(&self, model_id: &str) -> Result<()> {
        let Some(entry) = self.processes.get(model_id) else {
            tracing::debug!(model_id = %model_id, "stop_model called but model not found");
            return Ok(());
        };

        let mut process = entry.lock().await;
        let port = process.port;
        process.stop(std::time::Duration::from_secs(5)).await?;
        // Release DashMap entry and return port to pool.
        drop(process);
        drop(entry);
        self.processes.remove(model_id);
        self.port_pool.release(port);

        tracing::info!(model_id = %model_id, port = port, "model process stopped, port released");
        Ok(())
    }
}
```

Internal spawn helper signatures (full bodies shown in §2.2):

```rust
impl NodeAgent {
    /// Returns the raw Child handle. Port is passed in by caller (allocated from pool).
    async fn spawn_ollama(&self, deployment: &ModelDeployment) -> Result<tokio::process::Child>;
    async fn spawn_vllm(&self, deployment: &ModelDeployment, port: u16) -> Result<tokio::process::Child>;
    async fn spawn_sglang(&self, deployment: &ModelDeployment, port: u16) -> Result<tokio::process::Child>;
    async fn spawn_llamacpp(&self, deployment: &ModelDeployment, port: u16) -> Result<tokio::process::Child>;
    async fn spawn_tgi(&self, deployment: &ModelDeployment, port: u16) -> Result<tokio::process::Child>;
}
```

Ollama는 외부 데몬을 직접 관리하지 않으므로 `spawn_ollama`는 keep-alive HTTP 요청만 보내고 더미 `Child`를 돌려준다. 구체적으로는 `Child`를 반환해야 하므로 `tokio::process::Command::new("true").spawn()`을 사용해 즉시 종료되는 더미 프로세스를 반환하고, Ollama keep-alive는 별도 HTTP 호출로 처리한다.

#### 2.1.4 GAP-2: `Scheduler::deploy()` 변경 (`gadgetron-scheduler/src/scheduler.rs`)

`Scheduler`가 `NodeAgent`를 소유하도록 필드를 추가한다.

```rust
// gadgetron-scheduler/src/scheduler.rs  (변경 후)

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use dashmap::DashMap;

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::model::{InferenceEngine, ModelDeployment, ModelState};
use gadgetron_core::node::NodeStatus;
use gadgetron_node::agent::NodeAgent;

pub struct Scheduler {
    deployments: Arc<RwLock<HashMap<String, ModelDeployment>>>,
    nodes: Arc<RwLock<HashMap<String, NodeStatus>>>,
    /// In-process NodeAgent map, keyed by node_id.
    /// Arc<RwLock<...>> because agent.start_model() takes &self (DashMap interior mutability).
    agents: Arc<DashMap<String, NodeAgent>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            nodes:       Arc::new(RwLock::new(HashMap::new())),
            agents:      Arc::new(DashMap::new()),
        }
    }

    /// Register a NodeAgent for in-process calls.
    pub fn register_agent(&self, agent: NodeAgent) {
        self.agents.insert(agent.id().to_string(), agent);
    }

    /// Deploy a model to an available node with sufficient VRAM.
    ///
    /// Flow:
    ///   1. Skip if already Running.
    ///   2. Find node with available_vram_mb >= vram_mb.
    ///   3. If none found, run eviction loop (see §2.1.5).
    ///   4. Call NodeAgent::start_model() in-process.
    ///   5. Update ModelDeployment.status = Running, .port = returned port.
    pub async fn deploy(
        &self,
        model_id: &str,
        engine: InferenceEngine,
        vram_mb: u64,
    ) -> Result<()> {
        let nodes = self.nodes.read().await;
        let mut deployments = self.deployments.write().await;

        // 1. Idempotency check.
        if let Some(existing) = deployments.get(model_id) {
            if existing.is_available() {
                return Ok(());
            }
        }

        // 2. Find a node with enough VRAM.
        let target_node_id = self.find_node_with_vram(&nodes, vram_mb);

        // 3. If no node has enough VRAM, attempt eviction.
        //    evict_and_free() manages its own locking internally, so we must drop
        //    `deployments` and `nodes` before calling it to avoid deadlock.
        let target_node_id = match target_node_id {
            Some(id) => {
                // Still have valid locks — insert Loading record, then drop.
                let deployment = ModelDeployment {
                    id: model_id.to_string(),
                    engine: engine.clone(),
                    status: ModelState::Loading,
                    assigned_node: id.clone(),
                    port: 0,
                    vram_requirement_mb: vram_mb,
                    priority: 0,
                    args: None,
                    last_used: chrono::Utc::now(),
                    request_count: 0,
                };
                deployments.insert(model_id.to_string(), deployment.clone());
                drop(deployments);
                drop(nodes);
                id
            }
            None => {
                // Drop locks before entering evict_and_free which re-acquires them.
                drop(deployments);
                drop(nodes);
                self.evict_and_free(vram_mb).await?
            }
        };

        // 4. Insert a Loading record if we took the eviction path (evict_and_free already
        //    updated Stopped records; we now insert/update the new model as Loading).
        //    If we took the direct path the record was already inserted above.
        {
            let mut deployments = self.deployments.write().await;
            deployments.entry(model_id.to_string()).or_insert_with(|| ModelDeployment {
                id: model_id.to_string(),
                engine: engine.clone(),
                status: ModelState::Loading,
                assigned_node: target_node_id.clone(),
                port: 0,
                vram_requirement_mb: vram_mb,
                priority: 0,
                args: None,
                last_used: chrono::Utc::now(),
                request_count: 0,
            });
        }

        // 5. Call NodeAgent::start_model in-process.
        let port = {
            let agent = self.agents.get(&target_node_id).ok_or_else(|| {
                GadgetronError::Routing(format!(
                    "no agent registered for node {target_node_id}"
                ))
            })?;
            agent.start_model(&deployment).await?
        };

        // 6. Update record to Running with actual port and pid.
        let mut deployments = self.deployments.write().await;
        if let Some(rec) = deployments.get_mut(model_id) {
            rec.status = ModelState::Running;
            rec.port = port;
        }

        tracing::info!(
            model_id  = %model_id,
            node      = %target_node_id,
            port      = port,
            vram_mb   = vram_mb,
            "model deployed and running"
        );
        Ok(())
    }
}
```

#### 2.1.5 GAP-3: 퇴출 루프 (`evict_and_free`)

```rust
impl Scheduler {
    /// Find a node where VRAM can be freed by evicting LRU models, then do so.
    ///
    /// Returns the node_id where space was made, or Routing error if impossible.
    ///
    /// Lock discipline (self-contained):
    ///   1. Acquire nodes read lock + deployments write lock to collect eviction candidates.
    ///   2. Drop both locks before calling agent.stop_model() — no lock held across await.
    ///   3. Re-acquire deployments write lock to update ModelState::Stopped.
    ///
    /// Caller must NOT hold any lock on `self.nodes` or `self.deployments` when calling.
    async fn evict_and_free(
        &self,
        required_mb: u64,
    ) -> Result<String> {
        // Step 1: Collect (node_id, model_id) pairs to evict — under short-lived locks.
        let mut to_evict: Vec<(String, String)> = Vec::new(); // (node_id, model_id)
        let mut chosen_node: Option<String> = None;

        {
            let nodes = self.nodes.read().await;
            let deployments = self.deployments.write().await;

        'outer: for node in nodes.values().filter(|n| n.healthy) {
            let mut candidates: Vec<&ModelDeployment> = deployments
                .values()
                .filter(|d| d.assigned_node == node.id && d.is_available())
                .collect();

            // LRU order: oldest last_used first.
            candidates.sort_by_key(|d| d.last_used);

            let mut freed_mb: u64 = 0;
            let mut node_evict: Vec<(String, String)> = Vec::new();

            for candidate in &candidates {
                freed_mb += candidate.vram_requirement_mb;
                node_evict.push((node.id.clone(), candidate.id.clone()));
                if node.resources.available_vram_mb() + freed_mb >= required_mb {
                    break;
                }
            }

            if node.resources.available_vram_mb() + freed_mb >= required_mb {
                to_evict = node_evict;
                chosen_node = Some(node.id.clone());
                break 'outer;
            }
        }
        } // drops `nodes` read lock and `deployments` write lock

        let target_node_id = chosen_node.ok_or_else(|| {
            GadgetronError::Routing(format!(
                "no node can free {required_mb} MB VRAM even after LRU eviction"
            ))
        })?;

        // Step 2: Both locks are now dropped. Call stop_model sequentially — no lock held.
        for (node_id, model_id) in &to_evict {
            tracing::info!(
                model_id = %model_id,
                node     = %node_id,
                reason   = "lru_eviction",
                "evicting model to make VRAM available"
            );
            if let Some(agent) = self.agents.get(node_id) {
                // Best-effort: ignore individual stop errors (process may already be gone).
                let _ = agent.stop_model(model_id).await;
            }
        }

        // Step 3: Re-acquire deployments write lock to mark models as Stopped.
        {
            let mut deployments = self.deployments.write().await;
            for (_, model_id) in &to_evict {
                if let Some(rec) = deployments.get_mut(model_id.as_str()) {
                    rec.status = ModelState::Stopped;
                }
            }
        }

        Ok(target_node_id)
    }

    /// Find node id with sufficient free VRAM. Returns None if no such node exists.
    fn find_node_with_vram(
        &self,
        nodes: &HashMap<String, NodeStatus>,
        required_mb: u64,
    ) -> Option<String> {
        nodes
            .values()
            .filter(|n| n.healthy)
            .find(|n| n.resources.available_vram_mb() >= required_mb)
            .map(|n| n.id.clone())
    }
}
```

#### 2.1.6 GAP-4: Provider 0 경고 (`gadgetron-cli/src/main.rs`)

`build_providers` 호출 직후에 다음 블록을 삽입한다. 기존 `eprintln!(" done ({} configured)", providers_ss.len())` 라인을 아래로 교체한다.

```rust
// gadgetron-cli/src/main.rs  (Step 9 블록 교체)

let providers_ss = build_providers(&config).context("failed to initialise LLM providers")?;

if providers_ss.is_empty() {
    // WARNING — visible in terminal startup, not suppressed by --quiet.
    eprintln!(
        "warning: no providers configured — all /v1/chat/completions requests will \
         return 503. Add at least one [[providers]] entry to gadgetron.toml, or run \
         `gadgetron init` to generate a starter configuration."
    );
} else {
    eprintln!("  Providers: {} configured", providers_ss.len());
}
```

### 2.2 내부 구조

**동시성 모델 선택**

- `DashMap<String, tokio::sync::Mutex<ManagedProcess>>` in `NodeAgent`: 모델 기동·종료·상태 조회가 동시에 발생한다. `RwLock<HashMap>`보다 샤드 잠금 방식이 write 경합을 줄인다. 각 값을 `tokio::sync::Mutex`로 감싸면 `ManagedProcess::stop()`의 `async` 특성과 호환되고, 서로 다른 모델을 동시에 종료할 때 전체 맵을 잠그지 않아도 된다. `gadgetron-scheduler`가 이미 `dashmap` 의존성을 가지고 있으므로 추가 의존성 없음.
- `Mutex<BTreeSet<u16>>` in `PortPool`: 포트 할당은 짧은 critical section이고 CPU-bound이므로 `tokio::sync::Mutex`보다 `std::sync::Mutex`가 적합하다. `BTreeSet`은 항상 최솟값 O(log n) 반환으로 포트 번호가 재사용 시 낮은 번호부터 채워져 예측 가능하다.
- `Arc<DashMap<String, NodeAgent>>` in `Scheduler`: 스케줄러가 에이전트 레퍼런스를 공유해야 하므로 Arc. `start_model()`이 `&self`를 받으므로 DashMap `get()`으로 충분하다.

**Ollama 특수 처리**

Ollama는 별도 데몬이 이미 실행 중이라고 가정하고 keep-alive HTTP만 전송한다. `spawn_ollama`는 더미 프로세스(`true`)를 spawn하여 `ManagedProcess` 인터페이스를 유지하되, stop 시에는 Ollama `/api/generate` 에 `keep_alive: 0` 요청을 추가로 보낸다. 포트는 `11434`를 반환하지만 PortPool에서 할당하지 않는다(Ollama 포트는 Gadgetron이 관리하지 않음). 따라서 `spawn_ollama` 경로에서는 `port_pool.allocate()`를 건너뛴다. `start_model` 내부에서 `InferenceEngine::Ollama` 분기 시 별도 처리:

```rust
InferenceEngine::Ollama => {
    // Ollama manages its own port; do not allocate from pool.
    let child = self.spawn_ollama(deployment).await?;
    let process = ManagedProcess::new(deployment.id.clone(), 11434, child);
    self.processes.insert(deployment.id.clone(), tokio::sync::Mutex::new(process));
    return Ok(11434);
}
```

**deploy() 잠금 순서**

`deploy()`는 read(nodes) → write(deployments) 순서로 잠금을 획득한다. `evict_and_free()`를 호출하기 전 `start_model()` 호출 전에 잠금을 모두 drop하여 `async fn` 경계를 넘지 않게 한다. 이로써 잠금 보유 중 await로 인한 deadlock 가능성을 차단한다.

### 2.3 설정 스키마

Sprint 9는 신규 TOML 필드를 추가하지 않는다. 다음 기존 필드가 관련된다.

```toml
# gadgetron.toml

[[nodes]]
id       = "local"
endpoint = "http://127.0.0.1:9090"

[[models]]
id                  = "llama3.1-8b"
engine              = "vllm"
vram_requirement_mb = 16384
priority            = 10
args                = ["--max-model-len", "4096"]
```

PortPool 범위(30000–39999)는 하드코딩이며 이 스프린트에서 설정 가능하게 노출하지 않는다. Phase 2에서 `[scheduler].port_range_base` / `port_range_size` 필드로 노출한다.

### 2.4 에러 & 로깅

**사용하는 `GadgetronError` variant**

| 상황 | variant | kind |
|------|---------|------|
| 포트 풀 소진 | `Node { kind: NodeErrorKind::PortAllocationFailed, .. }` | `port_allocation_failed` |
| spawn 실패 (exec not found 등) | `Node { kind: NodeErrorKind::ProcessSpawnFailed, .. }` | `process_spawn_failed` |
| SIGKILL 실패 | `Node { kind: NodeErrorKind::ProcessKillFailed, .. }` | `process_kill_failed` |
| 에이전트 미등록 | `Routing(String)` | `routing_failure` |
| 전체 클러스터 VRAM 부족 | `Routing(String)` | `routing_failure` |

**PM 자율 결정** — `NodeErrorKind::ProcessKillFailed` variant를 `gadgetron-core/src/error.rs`에 추가한다. spawn 실패(프로세스 생성 불가)와 kill 실패(프로세스 종료 불가)는 의미상 다른 오류이므로 별도 variant가 필요하다. 기존 `PortAllocationFailed`와 동일한 패턴으로 추가하면 되며 외부 API 변경 없음.

**tracing span / event 목록**

| 위치 | 레벨 | 필드 |
|------|------|------|
| `NodeAgent::start_model` 성공 | `INFO` | `model_id`, `pid`, `port`, `engine` |
| `NodeAgent::stop_model` 성공 | `INFO` | `model_id`, `port` |
| SIGTERM 타임아웃 → SIGKILL | `WARN` | `model_id`, `pid` |
| `evict_and_free` 퇴출 결정 | `INFO` | `model_id`, `node`, `reason="lru_eviction"` |
| `deploy` 완료 | `INFO` | `model_id`, `node`, `port`, `vram_mb` |
| provider 0 경고 | `WARN` (eprintln) | — |

### 2.5 의존성

**루트 `Cargo.toml` (`[workspace.dependencies]`) 추가**

```toml
# root Cargo.toml — [workspace.dependencies] 섹션에 추가
nix = { version = "0.29", features = ["signal", "process"] }
```

`nix`를 workspace 수준에서 선언해야 `gadgetron-node`의 `[target.'cfg(unix)'.dependencies]`에서 `{ workspace = true }`로 참조할 수 있다.

**`gadgetron-node/Cargo.toml` 추가**

```toml
dashmap  = { workspace = true }   # DashMap for processes map

[target.'cfg(unix)'.dependencies]
nix = { workspace = true }
```

`nix`는 SIGTERM 전송에 필요. Unix-only feature gate로 Windows 빌드를 깨지 않는다.

**`gadgetron-scheduler/Cargo.toml`** — 이미 `dashmap`이 있으므로 추가 불필요.

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 의존성 그래프

```
gadgetron-core
  └── model.rs         (ModelDeployment, ModelState, InferenceEngine)
  └── node.rs          (NodeConfig, NodeStatus, NodeResources)
  └── error.rs         (GadgetronError, NodeErrorKind)

gadgetron-node          [depends on: gadgetron-core]
  └── process.rs       [NEW] ManagedProcess
  └── port_pool.rs     [NEW] PortPool
  └── agent.rs         [MODIFIED] NodeAgent (processes: DashMap, port_pool: PortPool)
  └── monitor.rs       (unchanged)

gadgetron-scheduler     [depends on: gadgetron-core, gadgetron-node]
  └── scheduler.rs     [MODIFIED] Scheduler (agents: DashMap<NodeAgent>)

gadgetron-cli           [depends on: gadgetron-scheduler, gadgetron-node, gadgetron-core]
  └── main.rs          [MODIFIED] build_providers → provider 0 warning
```

### 3.2 데이터 흐름 다이어그램

```
User / API
    │
    ▼
gadgetron-cli::main
    │ build_providers → 0개? → eprintln warning
    │
    ▼
Scheduler::deploy(model_id, engine, vram_mb)
    │
    ├─► find_node_with_vram() ──── OK ────────────────────────────────────────┐
    │                                                                          │
    └─► (no VRAM) evict_and_free()                                            │
            │                                                                  │
            ├─ LRU sort                                                        │
            ├─ for each candidate:                                             │
            │     NodeAgent::stop_model(evict_id)                             │
            │         └─► ManagedProcess::stop(5s)                            │
            │               SIGTERM → wait → SIGKILL                          │
            │               port_pool.release(port)                           │
            └─ return node_id ──────────────────────────────────────────────►─┤
                                                                               │
                                                                       NodeAgent::start_model(deployment)
                                                                           │
                                                                           ├─ port_pool.allocate()
                                                                           ├─ spawn_vllm/sglang/...
                                                                           │     → Child handle stored in DashMap
                                                                           └─ return port
                                                                               │
                                                                       deployments[model_id].status = Running
                                                                       deployments[model_id].port   = port
```

### 3.3 타 서브에이전트 도메인 인터페이스 계약

- **@chief-architect**: `ModelDeployment.port` 필드가 이미 `u16`으로 존재한다. D-10에 따라 `ModelState::Running{port, pid}`로 확장해야 하지만 이 스프린트에서는 `ModelDeployment.port`와 기존 `ModelState`(enum variant 없이 별도 필드)를 사용한다. D-10 완전 적용은 별도 스프린트에서 `gadgetron-core` 변경을 동반한다.
- **@inference-engine-lead**: `spawn_vllm` / `spawn_sglang` 등의 인자 구성은 `ModelDeployment.args: Option<Vec<String>>`를 그대로 CLI 인자로 전달한다. 엔진 특화 구조체(`VllmArgs` 등)로의 파싱은 Phase 2로 연기한다.
- **@devops-sre-lead**: NVML feature gate는 이 스프린트에서 건드리지 않는다. `nix` 크레이트의 Unix-only feature gate는 CI에서 `--target x86_64-unknown-linux-gnu` 빌드 시 자동 활성화된다.

### 3.4 D-12 크레이트 경계 준수

| 타입 | 실제 배치 | D-12 규정 | 준수 여부 |
|------|-----------|-----------|---------|
| `ManagedProcess` | `gadgetron-node/src/process.rs` | `ProcessManager gadgetron-node src/process.rs` | 준수 |
| `PortPool` | `gadgetron-node/src/port_pool.rs` | `PortAllocator gadgetron-core src/model.rs (트레이트)` | **주의**: D-12는 `PortAllocator` 트레이트를 `gadgetron-core`에 두도록 규정한다. 이 스프린트에서는 구현체 `PortPool`을 `gadgetron-node`에 두고 트레이트는 생략한다. Phase 2에서 트레이트 추출 예정 |
| `NodeAgent` (수정) | `gadgetron-node/src/agent.rs` | 규정 없음 (암묵적으로 node 크레이트) | 준수 |
| `Scheduler` (수정) | `gadgetron-scheduler/src/scheduler.rs` | 규정 없음 | 준수 |

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 대상 함수/타입 | 검증할 invariant |
|----------------|----------------|
| `PortPool::allocate` | 최솟값부터 반환, 소진 시 `PortAllocationFailed` |
| `PortPool::release` | 반환 후 `allocate`로 재획득 가능 |
| `PortPool::new` | 범위 오버플로 시 panic |
| `ManagedProcess::new` | pid가 child.id()와 일치 |
| `ManagedProcess::stop` | 두 번 호출 시 두 번째는 Ok(()) (idempotent) |
| `NodeAgent::start_model` (mock) | spawn 실패 시 port가 pool에 반환됨 |
| `NodeAgent::stop_model` | 없는 model_id는 Ok(()) 반환 |
| `Scheduler::find_node_with_vram` | VRAM 충분한 노드 반환, 없으면 None |
| `Scheduler::evict_and_free` | LRU 순서로 퇴출, 불가능 시 Routing 에러 |
| `build_providers` (provider 0) | 빈 config에서 Ok(empty map) 반환 확인 (기존 테스트 유지) |

### 4.2 테스트 하네스

**mock 전략**

- `ManagedProcess::stop()`의 SIGTERM/SIGKILL 경로는 실제 프로세스 없이 테스트하기 어렵다. `tokio::process::Command::new("sleep").arg("100").spawn()`으로 실제 sleep 프로세스를 기동한 뒤 stop을 호출하여 실제 시그널 경로를 검증한다.
- `NodeAgent::start_model()`의 spawn 실패 테스트는 존재하지 않는 바이너리(`/nonexistent-binary-gadgetron-test`)를 지정한 `ModelDeployment`로 호출한다.
- `Scheduler` 단위 테스트에서 `NodeAgent`를 mock하려면 `NodeAgent`에 `trait ProcessLauncher` 추상화가 필요하다. 이 스프린트에서는 `NodeAgent`를 직접 사용하되, spawn 바이너리를 `echo`(즉시 종료)로 교체하는 `ModelDeployment.args` 사용으로 실제 프로세스 기동 없이 테스트한다.

```rust
// gadgetron-node/src/port_pool.rs 내 단위 테스트 예시
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_returns_lowest_port_first() {
        let pool = PortPool::new(30_000, 3);
        assert_eq!(pool.allocate().unwrap(), 30_000);
        assert_eq!(pool.allocate().unwrap(), 30_001);
        assert_eq!(pool.allocate().unwrap(), 30_002);
    }

    #[test]
    fn exhausted_pool_returns_error() {
        let pool = PortPool::new(30_000, 1);
        pool.allocate().unwrap();
        let err = pool.allocate().unwrap_err();
        assert!(matches!(
            err,
            gadgetron_core::error::GadgetronError::Node {
                kind: gadgetron_core::error::NodeErrorKind::PortAllocationFailed,
                ..
            }
        ));
    }

    #[test]
    fn released_port_is_reallocated() {
        let pool = PortPool::new(30_000, 1);
        let p = pool.allocate().unwrap();
        pool.release(p);
        let p2 = pool.allocate().unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    #[should_panic]
    fn overflow_range_panics() {
        PortPool::new(65_535, 2); // 65535 + 2 > 65535
    }
}
```

### 4.3 커버리지 목표

- `port_pool.rs`: 라인 커버리지 100%, 브랜치 커버리지 90%
- `process.rs` (stop 경로): 라인 커버리지 80% (SIGKILL 경로는 slow test로 분리)
- `agent.rs` (start/stop 경로): 라인 커버리지 70% (spawn helper는 OS 의존성)
- `scheduler.rs` (evict_and_free): 라인 커버리지 85%

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

**함께 테스트할 크레이트**: `gadgetron-scheduler` + `gadgetron-node` + `gadgetron-core`

**e2e 시나리오 1: deploy → 프로세스 기동 확인**

1. `Scheduler::new()` + `NodeAgent::new(config)` 생성 후 `register_agent()`
2. 노드 VRAM 16384 MB로 `register_node()` 호출
3. `scheduler.deploy("llama3-8b", InferenceEngine::Vllm, 8192)` 호출
4. `scheduler.get_status("llama3-8b")` → `ModelState::Running` 확인
5. `scheduler.list_deployments()[0].port` != 0 확인

인프라: `ModelDeployment.args = Some(vec!["--help".to_string()])` 전달 시 vLLM이 즉시 종료되더라도 포트 번호가 반환된 상태이면 테스트 통과. 실제 vLLM은 CI에서 없으므로 `spawn`이 성공하는 `echo` 같은 바이너리를 engine 별 실행 파일명으로 PATH에 mock-script를 심는 방식을 채택한다.

**e2e 시나리오 2: VRAM 부족 → LRU 퇴출 → 재배포**

1. 노드 VRAM 16384 MB
2. 모델 A (12288 MB) 배포 → Running
3. 모델 B (8192 MB) 배포 시도 → 남은 4096 MB 부족
4. 퇴출 후 B 배포 성공 확인
5. A의 `ModelState` == `Stopped` 확인

**e2e 시나리오 3: stop_model → port 반환 확인**

1. 모델 기동 → port P 확인
2. `stop_model()` 호출
3. 동일 범위 내 새 모델 기동 시 port P 재사용 확인 (BTreeSet 최솟값 반환 특성)

### 5.2 테스트 환경

- GPU 없이 동작 가능: `NodeResources::gpus = vec![GpuInfo { vram_total_mb: 16384, vram_used_mb: 0, ... }]`를 고정 값으로 구성한 `MockNodeAgent` 또는 직접 구성
- 외부 의존성: 없음 (PostgreSQL, NVML 불필요)
- mock script 배치: `gadgetron-testing/fixtures/mock-scripts/` 에 `vllm`, `python3`, `llama-server`, `text-generation-launcher` 심볼릭 링크 또는 shell script 배치, PATH에 해당 디렉토리 prepend

### 5.3 회귀 방지

다음 변경이 테스트를 실패시켜야 한다:

- `cmd.spawn()?` 결과를 다시 버리는 변경 → 시나리오 1 실패 (port == 0)
- `stop_model()`에서 `port_pool.release()`를 제거 → 시나리오 3 실패 (port P 재사용 안 됨)
- `evict_and_free()`가 `stop_model()`을 호출하지 않는 변경 → 시나리오 2 실패 (Routing error)
- provider 0 warning 제거 → §4.1 `build_providers` 빈 config 테스트는 warning 출력 여부를 assert하지 않으므로 별도 출력 캡처 테스트 추가 필요

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| `PortPool` (30000–39999 고정 범위) | [P1] |
| `ManagedProcess` (SIGTERM + SIGKILL) | [P1] |
| `NodeAgent::start_model` / `stop_model` | [P1] |
| `Scheduler::deploy` → `NodeAgent::start_model` in-process | [P1] |
| LRU eviction loop in `deploy()` | [P1] |
| Provider 0 warning | [P1] |
| `PortPool` 설정 가능 범위 (`[scheduler].port_range_base`) | [P2] |
| `NodeAgent::start_model` → HTTP RPC 분리 (멀티노드) | [P2] |
| `trait ProcessLauncher` 추상화 (mock 용이성) | [P2] |
| `ModelState::Running { port, pid }` (D-10 완전 적용) | [P2] |
| Ollama keep-alive = 0 정상 종료 | [P1] (단, 이 스프린트에서 더미 구현) |
| `ThermalController` / `MigManager` 연동 | [P2] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID  | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| Q-1 | Ollama `spawn_ollama`가 더미 `Child`(true 프로세스)를 반환하는 방식이 맞는가? `ManagedProcess` 인터페이스를 유지하기 위한 타협 | A: 더미 Child 사용 (현재 설계) / B: Ollama를 `ManagedProcess`에서 제외하고 별도 `OllamaHandle` 타입 도입 | A — Phase 1 단순화 우선 | 🟡 PM 검토 요청 |
| Q-2 | `evict_and_free()`가 여러 모델을 순차 퇴출할 때 `stop_model` 실패(예: 프로세스 이미 종료)를 무시해도 되는가? 현재는 `let _ = agent.stop_model(evict_id).await` | A: 무시 (현재 설계) / B: 에러 누적 후 partial eviction 보고 | A — 퇴출 실패는 VRAM 반환 실패를 의미하지 않음 | 🟡 PM 검토 요청 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-12 — 예정
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

**다음 라운드 조건**: Round 1 리뷰어(@chief-architect, @inference-engine-lead) 검토 후
