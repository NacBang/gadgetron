use std::sync::Arc;

use dashmap::DashMap;
use gadgetron_core::error::{GadgetronError, NodeErrorKind, Result};
use gadgetron_core::model::{InferenceEngine, ModelDeployment};
use gadgetron_core::node::{NodeConfig, NodeResources, NodeStatus};

use crate::monitor::ResourceMonitor;
use crate::port_pool::PortPool;
use crate::process::ManagedProcess;

pub struct NodeAgent {
    config: NodeConfig,
    monitor: ResourceMonitor,
    /// Keyed by model_id. DashMap allows concurrent reads during health checks.
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
        let running_models: Vec<String> = self.processes.iter().map(|e| e.key().clone()).collect();
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
    ///
    /// Idempotent: if the model is already running, returns its existing port.
    pub async fn start_model(&self, deployment: &ModelDeployment) -> Result<u16> {
        if self.processes.contains_key(&deployment.id) {
            // Already running — return existing port.
            let port = self
                .processes
                .get(&deployment.id)
                .unwrap()
                .lock()
                .await
                .port;
            return Ok(port);
        }

        let port = self.port_pool.allocate()?;

        let spawn_result = match deployment.engine {
            InferenceEngine::Ollama => self.spawn_ollama(deployment).await,
            InferenceEngine::Vllm => self.spawn_vllm(deployment, port).await,
            InferenceEngine::Sglang => self.spawn_sglang(deployment, port).await,
            InferenceEngine::LlamaCpp => self.spawn_llamacpp(deployment, port).await,
            InferenceEngine::Tgi => self.spawn_tgi(deployment, port).await,
            _ => Err(GadgetronError::Node {
                kind: NodeErrorKind::ProcessSpawnFailed,
                message: format!("unsupported inference engine: {:?}", deployment.engine),
            }),
        };

        match spawn_result {
            Ok(child) => {
                let process = ManagedProcess::new(deployment.id.clone(), port, child);
                tracing::info!(
                    model_id = %deployment.id,
                    pid = process.pid,
                    port = port,
                    engine = ?deployment.engine,
                    "model process started"
                );
                self.processes
                    .insert(deployment.id.clone(), tokio::sync::Mutex::new(process));
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
    /// Idempotent: calling stop on an already-stopped or unknown model returns Ok(()).
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

    // -------------------------------------------------------------------------
    // Internal spawn helpers — each returns the raw Child handle.
    // -------------------------------------------------------------------------

    async fn spawn_ollama(&self, deployment: &ModelDeployment) -> Result<tokio::process::Child> {
        // Ollama runs as an external daemon; we can't own its Child.
        // Send a keep-alive HTTP request to load the model, then return a
        // dummy immediately-exiting child so ManagedProcess can be stored.
        let client = reqwest::Client::new();
        let url = format!("{}/api/generate", self.config.endpoint);
        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "model": deployment.id,
                "keep_alive": -1i64,
                "prompt": "",
            }))
            .send()
            .await
            .map_err(|e| GadgetronError::Node {
                kind: NodeErrorKind::ProcessSpawnFailed,
                message: format!("Failed to reach Ollama: {}", e),
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GadgetronError::Node {
                kind: NodeErrorKind::ProcessSpawnFailed,
                message: format!("Ollama model start failed: {}", body),
            });
        }

        // Dummy child — exits immediately, just serves as a placeholder handle.
        tokio::process::Command::new("true")
            .spawn()
            .map_err(|e| GadgetronError::Node {
                kind: NodeErrorKind::ProcessSpawnFailed,
                message: format!("Failed to spawn dummy child for Ollama: {}", e),
            })
    }

    async fn spawn_vllm(
        &self,
        deployment: &ModelDeployment,
        port: u16,
    ) -> Result<tokio::process::Child> {
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
        })
    }

    async fn spawn_llamacpp(
        &self,
        deployment: &ModelDeployment,
        port: u16,
    ) -> Result<tokio::process::Child> {
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
        })
    }

    async fn spawn_tgi(
        &self,
        deployment: &ModelDeployment,
        port: u16,
    ) -> Result<tokio::process::Child> {
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
        })
    }

    async fn spawn_sglang(
        &self,
        deployment: &ModelDeployment,
        port: u16,
    ) -> Result<tokio::process::Child> {
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
        })
    }
}
