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
    /// DashMap interior mutability means agent.start_model(&self) works without
    /// holding a write lock on the whole map.
    agents: Arc<DashMap<String, NodeAgent>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            agents: Arc::new(DashMap::new()),
        }
    }

    /// Register a NodeAgent for in-process calls.
    ///
    /// Keyed by the agent's own id. Replaces any previously registered agent
    /// for the same node_id.
    pub fn register_agent(&self, agent: NodeAgent) {
        self.agents.insert(agent.id().to_string(), agent);
    }

    /// Deploy a model to an available node with sufficient VRAM.
    ///
    /// Flow:
    ///   1. Skip if already Running (idempotency).
    ///   2. Find node with available_vram_mb >= vram_mb.
    ///   3. If none found, attempt LRU eviction via evict_and_free().
    ///   4. Call NodeAgent::start_model() in-process.
    ///   5. Update ModelDeployment.status = Running, .port = returned port.
    ///
    /// Lock discipline: no lock is held across any await point.
    pub async fn deploy(
        &self,
        model_id: &str,
        engine: InferenceEngine,
        vram_mb: u64,
    ) -> Result<()> {
        // --- 1. Idempotency check (read lock) ---
        {
            let deployments = self.deployments.read().await;
            if let Some(existing) = deployments.get(model_id) {
                if existing.is_available() {
                    return Ok(());
                }
            }
        }

        // --- 2. Find a node with enough free VRAM ---
        let target_node_id = {
            let nodes = self.nodes.read().await;
            self.find_node_with_vram(&nodes, vram_mb)
        };

        // --- 3. Determine target node, evicting if necessary ---
        let target_node_id = match target_node_id {
            Some(id) => id,
            None => {
                // evict_and_free acquires its own locks internally.
                // Must NOT hold any lock here.
                self.evict_and_free(vram_mb).await?
            }
        };

        // --- 4. Insert / ensure a Loading record (write lock, then drop) ---
        {
            let mut deployments = self.deployments.write().await;
            deployments
                .entry(model_id.to_string())
                .or_insert_with(|| ModelDeployment {
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
            // Update node assignment in case the record already existed from a
            // previous failed attempt.
            if let Some(rec) = deployments.get_mut(model_id) {
                rec.assigned_node = target_node_id.clone();
                rec.status = ModelState::Loading;
            }
        } // write lock dropped here

        // --- 5. Build the deployment snapshot to pass to start_model ---
        // Re-read under a read lock so we have the freshest args/priority.
        let deployment_snapshot = {
            let deployments = self.deployments.read().await;
            deployments.get(model_id).cloned().ok_or_else(|| {
                GadgetronError::Routing(format!(
                    "deployment record missing for model {} after insert",
                    model_id
                ))
            })?
        }; // read lock dropped here

        // --- 6. Call NodeAgent::start_model() — no lock held ---
        let port = {
            let agent = self.agents.get(&target_node_id).ok_or_else(|| {
                GadgetronError::Routing(format!("no agent registered for node {target_node_id}"))
            })?;
            agent.start_model(&deployment_snapshot).await?
        };

        // --- 7. Update status to Running (write lock) ---
        {
            let mut deployments = self.deployments.write().await;
            if let Some(rec) = deployments.get_mut(model_id) {
                rec.status = ModelState::Running;
                rec.port = port;
            }
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

    /// Undeploy (unload) a model.
    pub async fn undeploy(&self, model_id: &str) -> Result<()> {
        let mut deployments = self.deployments.write().await;
        deployments
            .remove(model_id)
            .ok_or_else(|| GadgetronError::Routing(format!("model not found: {}", model_id)))?;
        Ok(())
    }

    /// Get the current status of a model deployment.
    pub async fn get_status(&self, model_id: &str) -> Option<ModelState> {
        let deployments = self.deployments.read().await;
        deployments.get(model_id).map(|d| d.status.clone())
    }

    /// List all deployments.
    pub async fn list_deployments(&self) -> Vec<ModelDeployment> {
        let deployments = self.deployments.read().await;
        deployments.values().cloned().collect()
    }

    /// Find the best candidate for eviction based on LRU policy.
    ///
    /// Returns the model_id of the single oldest-used running model on `node_id`
    /// whose cumulative VRAM meets or exceeds `required_mb`.
    pub async fn find_eviction_candidate(&self, node_id: &str, required_mb: u64) -> Option<String> {
        let deployments = self.deployments.read().await;
        let mut candidates: Vec<&ModelDeployment> = deployments
            .values()
            .filter(|d| d.assigned_node == node_id && d.is_available())
            .collect();

        // LRU order: oldest last_used first.
        candidates.sort_by_key(|d| d.last_used);

        let mut freed_mb: u64 = 0;
        for candidate in &candidates {
            freed_mb += candidate.vram_requirement_mb;
            if freed_mb >= required_mb {
                return Some(candidate.id.clone());
            }
        }
        None
    }

    /// Register a node in the cluster.
    pub async fn register_node(&self, status: NodeStatus) {
        let mut nodes = self.nodes.write().await;
        nodes.insert(status.id.clone(), status);
    }

    /// Update node status (called by health checker).
    pub async fn update_node(&self, node_id: &str, status: NodeStatus) {
        let mut nodes = self.nodes.write().await;
        nodes.insert(node_id.to_string(), status);
    }

    /// Get all node statuses.
    pub async fn list_nodes(&self) -> Vec<NodeStatus> {
        let nodes = self.nodes.read().await;
        nodes.values().cloned().collect()
    }

    // -------------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------------

    /// Find a healthy node with sufficient free VRAM. Returns `None` when none exists.
    ///
    /// Caller must supply the `nodes` guard to avoid a redundant lock acquisition.
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

    /// Find a node where VRAM can be freed by evicting LRU models, then do so.
    ///
    /// Returns the `node_id` where space was freed, or a `Routing` error if
    /// no single node can satisfy the request even after full LRU eviction.
    ///
    /// Lock discipline (fully self-contained; caller must hold NO locks):
    ///   1. Short-lived read(nodes) + read(deployments) to collect candidates.
    ///   2. Both locks dropped before any await (agent.stop_model).
    ///   3. Write(deployments) re-acquired to mark models Stopped.
    async fn evict_and_free(&self, required_mb: u64) -> Result<String> {
        // Step 1: Collect (node_id, model_id) pairs to evict under short locks.
        let mut to_evict: Vec<(String, String)> = Vec::new();
        let mut chosen_node: Option<String> = None;

        {
            let nodes = self.nodes.read().await;
            let deployments = self.deployments.read().await;

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
        } // nodes read lock + deployments read lock both dropped here

        let target_node_id = chosen_node.ok_or_else(|| {
            GadgetronError::Routing(format!(
                "no node can free {required_mb} MB VRAM even after LRU eviction"
            ))
        })?;

        // Step 2: Both locks are dropped. Call stop_model — no lock held across await.
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

        // Step 3: Re-acquire write lock to mark evicted models as Stopped.
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
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::node::{GpuInfo, NodeConfig, NodeResources};

    // -------------------------------------------------------------------------
    // Test helpers
    // -------------------------------------------------------------------------

    /// Build a NodeStatus with `available_vram_mb` == `free_vram_mb` (no GPU used).
    fn make_node(id: &str, free_vram_mb: u64) -> NodeStatus {
        NodeStatus {
            id: id.to_string(),
            endpoint: "http://127.0.0.1:9090".to_string(),
            healthy: true,
            resources: NodeResources {
                gpus: vec![GpuInfo {
                    index: 0,
                    name: "test-gpu".to_string(),
                    vram_total_mb: free_vram_mb,
                    vram_used_mb: 0,
                    utilization_pct: 0.0,
                    temperature_c: 40,
                    power_draw_w: 0.0,
                    power_limit_w: 300.0,
                }],
                cpu_usage_pct: 0.0,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                memory_used_bytes: 0,
            },
            running_models: vec![],
            last_heartbeat: chrono::Utc::now(),
        }
    }

    /// Build a NodeAgent whose spawn helpers will fail immediately (no real binary).
    /// Used to test scheduler logic without spawning real processes.
    fn make_node_agent(id: &str) -> NodeAgent {
        NodeAgent::new(NodeConfig {
            id: id.to_string(),
            endpoint: "http://127.0.0.1:9090".to_string(),
            gpu_count: None,
            labels: None,
        })
    }

    // -------------------------------------------------------------------------
    // register_agent tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn register_agent_stores_agent() {
        let scheduler = Scheduler::new();
        let agent = make_node_agent("node-1");

        scheduler.register_agent(agent);

        // Confirm the agent is retrievable via the agents DashMap.
        assert!(
            scheduler.agents.contains_key("node-1"),
            "agent must be stored under its own id"
        );
    }

    #[tokio::test]
    async fn register_agent_replaces_existing_entry() {
        let scheduler = Scheduler::new();
        scheduler.register_agent(make_node_agent("node-1"));
        scheduler.register_agent(make_node_agent("node-1"));

        assert_eq!(
            scheduler.agents.len(),
            1,
            "second register must replace, not append"
        );
    }

    // -------------------------------------------------------------------------
    // find_node_with_vram tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn find_node_with_vram_returns_node_when_sufficient() {
        let scheduler = Scheduler::new();
        let mut map = HashMap::new();
        map.insert("n1".to_string(), make_node("n1", 16_384));

        let result = scheduler.find_node_with_vram(&map, 8_192);
        assert_eq!(result, Some("n1".to_string()));
    }

    #[tokio::test]
    async fn find_node_with_vram_returns_none_when_insufficient() {
        let scheduler = Scheduler::new();
        let mut map = HashMap::new();
        map.insert("n1".to_string(), make_node("n1", 4_096));

        let result = scheduler.find_node_with_vram(&map, 8_192);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn find_node_with_vram_skips_unhealthy_nodes() {
        let scheduler = Scheduler::new();
        let mut map = HashMap::new();
        let mut sick_node = make_node("n1", 32_768);
        sick_node.healthy = false;
        map.insert("n1".to_string(), sick_node);

        let result = scheduler.find_node_with_vram(&map, 8_192);
        assert!(result.is_none(), "unhealthy node must not be selected");
    }

    // -------------------------------------------------------------------------
    // evict_and_free tests (scheduler-only, no real agent needed because
    // we call stop_model on models that have no agent registered, which is
    // silently skipped per the best-effort contract in evict_and_free).
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn deploy_evicts_when_vram_insufficient() {
        let scheduler = Scheduler::new();

        // Node with 16 GB total (vram_used = 0, so available = 16 384).
        scheduler.register_node(make_node("node-1", 16_384)).await;

        // Pre-populate a Running deployment consuming 12 GB — leaves only 4 GB.
        {
            let mut deployments = scheduler.deployments.write().await;
            deployments.insert(
                "model-a".to_string(),
                ModelDeployment {
                    id: "model-a".to_string(),
                    engine: InferenceEngine::Vllm,
                    status: ModelState::Running,
                    assigned_node: "node-1".to_string(),
                    port: 30_000,
                    vram_requirement_mb: 12_288,
                    priority: 0,
                    args: None,
                    last_used: chrono::Utc::now() - chrono::Duration::seconds(120),
                    request_count: 0,
                },
            );
        }

        // Attempt to evict — 8 GB request cannot fit in remaining 4 GB but
        // can fit after evicting model-a (12 GB freed).
        // No agent is registered so stop_model is a no-op (best-effort).
        let result = scheduler.evict_and_free(8_192).await;
        assert!(result.is_ok(), "evict_and_free must succeed: {:?}", result);
        assert_eq!(result.unwrap(), "node-1");

        // model-a must be marked Stopped after eviction.
        let status = scheduler.get_status("model-a").await;
        assert_eq!(
            status,
            Some(ModelState::Stopped),
            "evicted model must be Stopped"
        );
    }

    #[tokio::test]
    async fn evict_and_free_returns_error_when_eviction_impossible() {
        let scheduler = Scheduler::new();

        // Node with only 4 GB and no running models to evict.
        scheduler.register_node(make_node("node-1", 4_096)).await;

        let result = scheduler.evict_and_free(8_192).await;
        assert!(
            result.is_err(),
            "must return Routing error when eviction cannot free enough VRAM"
        );
        match result.unwrap_err() {
            GadgetronError::Routing(_) => {}
            other => panic!("expected Routing error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn evict_and_free_lru_order_evicts_oldest_first() {
        let scheduler = Scheduler::new();
        scheduler.register_node(make_node("node-1", 16_384)).await;

        let now = chrono::Utc::now();
        {
            let mut deployments = scheduler.deployments.write().await;
            // model-old: used 2 minutes ago (LRU candidate)
            deployments.insert(
                "model-old".to_string(),
                ModelDeployment {
                    id: "model-old".to_string(),
                    engine: InferenceEngine::Vllm,
                    status: ModelState::Running,
                    assigned_node: "node-1".to_string(),
                    port: 30_000,
                    vram_requirement_mb: 8_192,
                    priority: 0,
                    args: None,
                    last_used: now - chrono::Duration::seconds(120),
                    request_count: 0,
                },
            );
            // model-new: used 10 seconds ago (more recently used)
            deployments.insert(
                "model-new".to_string(),
                ModelDeployment {
                    id: "model-new".to_string(),
                    engine: InferenceEngine::Vllm,
                    status: ModelState::Running,
                    assigned_node: "node-1".to_string(),
                    port: 30_001,
                    vram_requirement_mb: 8_192,
                    priority: 0,
                    args: None,
                    last_used: now - chrono::Duration::seconds(10),
                    request_count: 0,
                },
            );
        }

        // 8 GB request: only model-old needs eviction.
        let result = scheduler.evict_and_free(8_192).await;
        assert!(result.is_ok());

        assert_eq!(
            scheduler.get_status("model-old").await,
            Some(ModelState::Stopped),
            "oldest model must be evicted"
        );
        assert_eq!(
            scheduler.get_status("model-new").await,
            Some(ModelState::Running),
            "newer model must be preserved when single eviction is sufficient"
        );
    }

    // -------------------------------------------------------------------------
    // deploy() → NodeAgent::start_model() integration
    //
    // We use InferenceEngine::Ollama which calls spawn_ollama() → reqwest HTTP
    // to an endpoint that does not exist, causing a spawn error. This lets us
    // verify the full deploy() code path without requiring any real inference
    // engine binary.
    //
    // For the "calls start_model" test we verify that deploy() propagates the
    // NodeAgent error rather than silently succeeding with port==0, which would
    // be the old (stub) behaviour.
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn deploy_calls_start_model_and_propagates_agent_error() {
        let scheduler = Scheduler::new();

        // Node with plenty of VRAM — no eviction path needed.
        scheduler.register_node(make_node("node-1", 32_768)).await;

        // Register an agent whose Ollama endpoint does not exist.
        scheduler.register_agent(make_node_agent("node-1"));

        // Vllm engine: spawn_vllm will fail because "vllm" binary is not on PATH.
        // This exercises the start_model() call path.
        let result = scheduler
            .deploy("test-model", InferenceEngine::Vllm, 8_192)
            .await;

        // The old stub would return Ok(()); real deploy() propagates the spawn error.
        assert!(
            result.is_err(),
            "deploy() must propagate NodeAgent spawn error, not silently succeed"
        );
    }

    #[tokio::test]
    async fn deploy_is_idempotent_when_already_running() {
        let scheduler = Scheduler::new();

        // Pre-insert a Running record — deploy() must return Ok without touching agents.
        {
            let mut deployments = scheduler.deployments.write().await;
            deployments.insert(
                "model-x".to_string(),
                ModelDeployment {
                    id: "model-x".to_string(),
                    engine: InferenceEngine::Vllm,
                    status: ModelState::Running,
                    assigned_node: "node-1".to_string(),
                    port: 30_000,
                    vram_requirement_mb: 4_096,
                    priority: 0,
                    args: None,
                    last_used: chrono::Utc::now(),
                    request_count: 0,
                },
            );
        }

        // No node registered — if idempotency check fails it would error on node lookup.
        let result = scheduler
            .deploy("model-x", InferenceEngine::Vllm, 4_096)
            .await;
        assert!(
            result.is_ok(),
            "already-running model must return Ok immediately"
        );
    }

    #[tokio::test]
    async fn deploy_returns_routing_error_when_no_node_registered() {
        let scheduler = Scheduler::new();
        // No nodes at all — must fail at evict_and_free with Routing error.
        let result = scheduler
            .deploy("model-y", InferenceEngine::Vllm, 8_192)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            GadgetronError::Routing(_) => {}
            other => panic!("expected Routing error, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // Existing behaviour regression tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn undeploy_removes_deployment() {
        let scheduler = Scheduler::new();
        {
            let mut deployments = scheduler.deployments.write().await;
            deployments.insert(
                "m".to_string(),
                ModelDeployment {
                    id: "m".to_string(),
                    engine: InferenceEngine::Vllm,
                    status: ModelState::Running,
                    assigned_node: "n".to_string(),
                    port: 30_000,
                    vram_requirement_mb: 1_024,
                    priority: 0,
                    args: None,
                    last_used: chrono::Utc::now(),
                    request_count: 0,
                },
            );
        }
        scheduler.undeploy("m").await.unwrap();
        assert!(scheduler.get_status("m").await.is_none());
    }

    #[tokio::test]
    async fn list_deployments_returns_all() {
        let scheduler = Scheduler::new();
        assert!(scheduler.list_deployments().await.is_empty());
    }

    #[tokio::test]
    async fn list_nodes_returns_all() {
        let scheduler = Scheduler::new();
        scheduler.register_node(make_node("n1", 8_192)).await;
        scheduler.register_node(make_node("n2", 16_384)).await;
        assert_eq!(scheduler.list_nodes().await.len(), 2);
    }
}
