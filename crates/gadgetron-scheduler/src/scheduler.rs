use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::model::{InferenceEngine, ModelDeployment, ModelState};
use gadgetron_core::node::NodeStatus;

pub struct Scheduler {
    deployments: Arc<RwLock<HashMap<String, ModelDeployment>>>,
    nodes: Arc<RwLock<HashMap<String, NodeStatus>>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Deploy a model to an available node with sufficient VRAM.
    pub async fn deploy(
        &self,
        model_id: &str,
        engine: InferenceEngine,
        vram_mb: u64,
    ) -> Result<()> {
        let nodes = self.nodes.read().await;
        let mut deployments = self.deployments.write().await;

        // Check if already deployed
        if let Some(existing) = deployments.get(model_id) {
            if existing.is_available() {
                return Ok(()); // Already running
            }
        }

        // Find a node with enough VRAM
        let target_node = nodes
            .values()
            .filter(|n| n.healthy)
            .find(|n| n.resources.available_vram_mb() >= vram_mb)
            .ok_or_else(|| {
                // Try to find an eviction candidate
                GadgetronError::Routing(format!(
                    "no node has {} MB VRAM available for model {}",
                    vram_mb, model_id
                ))
            })?;

        let deployment = ModelDeployment {
            id: model_id.to_string(),
            engine,
            status: ModelState::Loading,
            assigned_node: target_node.id.clone(),
            port: 0, // assigned by node agent
            vram_requirement_mb: vram_mb,
            priority: 0,
            args: None,
            last_used: chrono::Utc::now(),
            request_count: 0,
        };

        deployments.insert(model_id.to_string(), deployment);
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
    pub async fn find_eviction_candidate(&self, node_id: &str, required_mb: u64) -> Option<String> {
        let deployments = self.deployments.read().await;
        let running: Vec<&ModelDeployment> = deployments
            .values()
            .filter(|d| d.assigned_node == node_id && d.is_available())
            .collect();

        // Sort by last_used (LRU first) and find enough cumulative VRAM
        let mut candidates: Vec<&ModelDeployment> = running;
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
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
