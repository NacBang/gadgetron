use gadgetron_core::error::{GadgetronError, NodeErrorKind, Result};
use gadgetron_core::model::{InferenceEngine, ModelDeployment};
use gadgetron_core::node::{NodeConfig, NodeResources, NodeStatus};

use crate::monitor::ResourceMonitor;

pub struct NodeAgent {
    config: NodeConfig,
    monitor: ResourceMonitor,
    running_models: Vec<String>,
}

impl NodeAgent {
    pub fn new(config: NodeConfig) -> Self {
        Self {
            config,
            monitor: ResourceMonitor::new(),
            running_models: Vec::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.config.id
    }

    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    /// Collect current resource metrics.
    pub fn collect_metrics(&mut self) -> NodeResources {
        self.monitor.collect()
    }

    /// Get current node status.
    pub fn status(&mut self) -> NodeStatus {
        let resources = self.collect_metrics();
        NodeStatus {
            id: self.config.id.clone(),
            endpoint: self.config.endpoint.clone(),
            healthy: true,
            resources,
            running_models: self.running_models.clone(),
            last_heartbeat: chrono::Utc::now(),
        }
    }

    /// Start a model on this node.
    pub async fn start_model(&mut self, deployment: &ModelDeployment) -> Result<u16> {
        let port = match deployment.engine {
            InferenceEngine::Ollama => self.start_ollama_model(deployment).await?,
            InferenceEngine::Vllm => self.start_vllm_model(deployment).await?,
            InferenceEngine::Sglang => self.start_sglang_model(deployment).await?,
            InferenceEngine::LlamaCpp => self.start_llamacpp_model(deployment).await?,
            InferenceEngine::Tgi => self.start_tgi_model(deployment).await?,
        };
        self.running_models.push(deployment.id.clone());
        Ok(port)
    }

    /// Stop a model on this node.
    pub async fn stop_model(&mut self, model_id: &str) -> Result<()> {
        self.running_models.retain(|m| m != model_id);
        // TODO: Send unload request to Ollama or kill vLLM/llama.cpp process
        Ok(())
    }

    async fn start_ollama_model(&self, deployment: &ModelDeployment) -> Result<u16> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/generate", self.config.endpoint);

        // Ollama loads model on first request; we send a keep_alive request
        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "model": deployment.id,
                "keep_alive": -1i64,  // keep loaded indefinitely
                "prompt": "",
            }))
            .send()
            .await
            .map_err(|e| GadgetronError::Node {
                kind: NodeErrorKind::ProcessSpawnFailed,
                message: format!("Failed to start Ollama model: {}", e),
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GadgetronError::Node {
                kind: NodeErrorKind::ProcessSpawnFailed,
                message: format!("Ollama model start failed: {}", body),
            });
        }

        Ok(11434) // Ollama default port
    }

    async fn start_vllm_model(&self, deployment: &ModelDeployment) -> Result<u16> {
        // vLLM requires starting a new process per model
        let port = 8000u16; // TODO: dynamic port assignment
        let mut cmd = tokio::process::Command::new("vllm");
        cmd.arg("serve")
            .arg(&deployment.id)
            .arg("--port")
            .arg(port.to_string());

        if let Some(ref args) = deployment.args {
            for arg in args {
                cmd.arg(arg);
            }
        }

        cmd.spawn().map_err(|e| GadgetronError::Node {
            kind: NodeErrorKind::ProcessSpawnFailed,
            message: format!("Failed to spawn vLLM: {}", e),
        })?;

        Ok(port)
    }

    async fn start_llamacpp_model(&self, deployment: &ModelDeployment) -> Result<u16> {
        let port = 8080u16; // TODO: dynamic port assignment
        let mut cmd = tokio::process::Command::new("llama-server");
        cmd.arg("-m")
            .arg(&deployment.id)
            .arg("--port")
            .arg(port.to_string());

        if let Some(ref args) = deployment.args {
            for arg in args {
                cmd.arg(arg);
            }
        }

        cmd.spawn().map_err(|e| GadgetronError::Node {
            kind: NodeErrorKind::ProcessSpawnFailed,
            message: format!("Failed to spawn llama-server: {}", e),
        })?;

        Ok(port)
    }

    async fn start_tgi_model(&self, deployment: &ModelDeployment) -> Result<u16> {
        let port = 3000u16; // TODO: dynamic port assignment
        let mut cmd = tokio::process::Command::new("text-generation-launcher");
        cmd.arg("--model-id")
            .arg(&deployment.id)
            .arg("--port")
            .arg(port.to_string());

        if let Some(ref args) = deployment.args {
            for arg in args {
                cmd.arg(arg);
            }
        }

        cmd.spawn().map_err(|e| GadgetronError::Node {
            kind: NodeErrorKind::ProcessSpawnFailed,
            message: format!("Failed to spawn TGI: {}", e),
        })?;

        Ok(port)
    }

    async fn start_sglang_model(&self, deployment: &ModelDeployment) -> Result<u16> {
        // SGLang uses `python -m sglang.launch_server` to start a serving process
        let port = 30000u16; // SGLang default port
        let mut cmd = tokio::process::Command::new("python3");
        cmd.arg("-m")
            .arg("sglang.launch_server")
            .arg("--model-path")
            .arg(&deployment.id)
            .arg("--port")
            .arg(port.to_string());

        if let Some(ref args) = deployment.args {
            for arg in args {
                cmd.arg(arg);
            }
        }

        cmd.spawn().map_err(|e| GadgetronError::Node {
            kind: NodeErrorKind::ProcessSpawnFailed,
            message: format!("Failed to spawn SGLang: {}", e),
        })?;

        Ok(port)
    }
}
