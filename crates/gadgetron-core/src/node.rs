use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResources {
    pub gpus: Vec<GpuInfo>,
    pub cpu_usage_pct: f32,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
}

impl NodeResources {
    pub fn available_vram_mb(&self) -> u64 {
        self.gpus
            .iter()
            .map(|g| g.vram_total_mb.saturating_sub(g.vram_used_mb))
            .sum()
    }

    pub fn memory_available_bytes(&self) -> u64 {
        self.memory_total_bytes
            .saturating_sub(self.memory_used_bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    pub vram_total_mb: u64,
    pub vram_used_mb: u64,
    pub utilization_pct: f32,
    pub temperature_c: u32,
    pub power_draw_w: f32,
    pub power_limit_w: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub id: String,
    pub endpoint: String,
    #[serde(default)]
    pub gpu_count: Option<u32>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub id: String,
    pub endpoint: String,
    pub healthy: bool,
    pub resources: NodeResources,
    pub running_models: Vec<String>,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
}
