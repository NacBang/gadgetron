//! Shared UI types (D-20260411-07).
//!
//! Used by `gadgetron-tui` [P1] and `gadgetron-web` [P2].
//! All types implement `serde::{Serialize, Deserialize}` for WebSocket JSON serialization.
//! Any type change requires reviewing impact on both TUI and Web UI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── GPU metrics ────────────────────────────────────────────────────────────

/// Instantaneous metrics for a single GPU.
///
/// `node_id`: matches `NodeStatus.id` in `gadgetron-core/src/node.rs`.
/// `gpu_index`: 0-based device index, identical to `GpuInfo::index`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuMetrics {
    pub node_id: String,
    pub gpu_index: u32,
    pub name: String,
    pub temperature_c: f32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub utilization_pct: f32,
    pub power_w: f32,
    pub power_limit_w: f32,
    /// GPU core clock speed (MHz). From NVML `nvmlDeviceGetClockInfo(GRAPHICS)`.
    /// FakeGpuMonitor uses a fixed value (e.g. 1_410).
    pub clock_mhz: u32,
    pub timestamp: DateTime<Utc>,
}

// ── Model status ───────────────────────────────────────────────────────────

/// Model lifecycle state. `#[non_exhaustive]`: Phase 2 may add Migrating/Updating.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    Running,
    Loading,
    Stopped,
    Error,
    Draining,
}

impl std::fmt::Display for ModelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Running => "running",
            Self::Loading => "loading",
            Self::Stopped => "stopped",
            Self::Error => "error",
            Self::Draining => "draining",
            // Safety: #[non_exhaustive] requires a wildcard arm even in the defining crate.
            #[allow(unreachable_patterns)]
            _ => "unknown",
        };
        write!(f, "{s}")
    }
}

/// Current state of a single model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    /// Model identifier (e.g. "meta-llama/Llama-3-8B-Instruct")
    pub model_id: String,
    /// Human-readable name (e.g. "Llama 3 8B")
    pub name: String,
    pub state: ModelState,
    /// Provider name (e.g. "openai", "ollama")
    pub provider: String,
    /// Node ID (`None` = stopped/unassigned)
    pub node_id: Option<String>,
    /// VRAM occupied in MB (`None` = not loaded)
    pub vram_mb: Option<u64>,
    pub loaded_at: Option<DateTime<Utc>>,
}

// ── Request log ────────────────────────────────────────────────────────────

/// Log entry for a single LLM request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEntry {
    pub request_id: String,
    pub tenant_id: String,
    pub model: String,
    pub provider: String,
    /// HTTP response status code
    pub status: u16,
    pub latency_ms: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub timestamp: DateTime<Utc>,
}

// ── Cluster health ─────────────────────────────────────────────────────────

/// Cluster-wide health summary.
///
/// Used in TUI header bar and 3-column layout summary.
///
/// # Sprint 5 scope note
/// Per-node CPU/RAM usage is not included in the Sprint 5 TUI MVP.
/// Sprint 5 TUI is GPU-only (`GpuMetrics`-based).
/// Per-node CPU/RAM fields will be added in Sprint 6 with `NodeStatus` extension ([P2]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterHealth {
    pub total_nodes: u32,
    pub healthy_nodes: u32,
    pub total_gpus: u32,
    pub active_gpus: u32,
    pub models_loaded: u32,
    pub requests_per_sec: f32,
    pub error_rate_pct: f32,
    /// UTC timestamp of last update
    pub updated_at: DateTime<Utc>,
    // Per-node CPU/RAM display: Sprint 6 [P2]. Current TUI MVP is GPU-only.
    // per_node_cpu_pct: Vec<f32>,   // [P2] Sprint 6
    // per_node_ram_mb: Vec<u64>,    // [P2] Sprint 6
}

// ── WebSocket messages ─────────────────────────────────────────────────────

/// Messages sent over WebSocket/channel.
///
/// [P1]: TUI in-process Arc polling + tokio channel.
/// [P2]: Real WebSocket JSON serialization.
///
/// Adjacently tagged to support Vec variants (serde internally-tagged enums
/// cannot serialize sequences). JSON form:
/// `{"type":"node_update","data":[...]}` or `{"type":"health_update","data":{...}}`
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum WsMessage {
    NodeUpdate(Vec<GpuMetrics>),
    ModelUpdate(Vec<ModelStatus>),
    RequestLog(RequestEntry),
    HealthUpdate(ClusterHealth),
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_gpu_metrics() -> GpuMetrics {
        GpuMetrics {
            node_id: "node-1".to_string(),
            gpu_index: 0,
            name: "NVIDIA A100".to_string(),
            temperature_c: 55.0,
            vram_used_mb: 20_000,
            vram_total_mb: 40_000,
            utilization_pct: 75.0,
            power_w: 250.0,
            power_limit_w: 400.0,
            clock_mhz: 1_410,
            timestamp: Utc::now(),
        }
    }

    fn sample_model_status() -> ModelStatus {
        ModelStatus {
            model_id: "meta-llama/Llama-3-8B-Instruct".to_string(),
            name: "Llama 3 8B".to_string(),
            state: ModelState::Running,
            provider: "ollama".to_string(),
            node_id: Some("node-1".to_string()),
            vram_mb: Some(16_000),
            loaded_at: Some(Utc::now()),
        }
    }

    fn sample_request_entry() -> RequestEntry {
        RequestEntry {
            request_id: "req-abc123".to_string(),
            tenant_id: "tenant-42".to_string(),
            model: "llama3".to_string(),
            provider: "ollama".to_string(),
            status: 200,
            latency_ms: 312,
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            timestamp: Utc::now(),
        }
    }

    fn sample_cluster_health() -> ClusterHealth {
        ClusterHealth {
            total_nodes: 3,
            healthy_nodes: 3,
            total_gpus: 12,
            active_gpus: 10,
            models_loaded: 4,
            requests_per_sec: 42.5,
            error_rate_pct: 0.1,
            updated_at: Utc::now(),
        }
    }

    // ── GpuMetrics roundtrip ─────────────────────────────────────────────

    #[test]
    fn gpu_metrics_serde_roundtrip() {
        let original = sample_gpu_metrics();
        let json = serde_json::to_string(&original).expect("serialize GpuMetrics");
        let restored: GpuMetrics = serde_json::from_str(&json).expect("deserialize GpuMetrics");
        assert_eq!(original.node_id, restored.node_id);
        assert_eq!(original.gpu_index, restored.gpu_index);
        assert_eq!(original.name, restored.name);
        assert_eq!(original.temperature_c, restored.temperature_c);
        assert_eq!(original.vram_used_mb, restored.vram_used_mb);
        assert_eq!(original.vram_total_mb, restored.vram_total_mb);
        assert_eq!(original.utilization_pct, restored.utilization_pct);
        assert_eq!(original.power_w, restored.power_w);
        assert_eq!(original.power_limit_w, restored.power_limit_w);
        assert_eq!(original.clock_mhz, restored.clock_mhz);
    }

    // ── ModelStatus roundtrip ────────────────────────────────────────────

    #[test]
    fn model_status_serde_roundtrip() {
        let original = sample_model_status();
        let json = serde_json::to_string(&original).expect("serialize ModelStatus");
        let restored: ModelStatus = serde_json::from_str(&json).expect("deserialize ModelStatus");
        assert_eq!(original.model_id, restored.model_id);
        assert_eq!(original.name, restored.name);
        assert_eq!(original.state, restored.state);
        assert_eq!(original.provider, restored.provider);
        assert_eq!(original.node_id, restored.node_id);
        assert_eq!(original.vram_mb, restored.vram_mb);
    }

    #[test]
    fn model_status_none_fields_roundtrip() {
        let original = ModelStatus {
            model_id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            state: ModelState::Stopped,
            provider: "openai".to_string(),
            node_id: None,
            vram_mb: None,
            loaded_at: None,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: ModelStatus = serde_json::from_str(&json).expect("deserialize");
        assert!(restored.node_id.is_none());
        assert!(restored.vram_mb.is_none());
        assert!(restored.loaded_at.is_none());
    }

    // ── RequestEntry roundtrip ───────────────────────────────────────────

    #[test]
    fn request_entry_serde_roundtrip() {
        let original = sample_request_entry();
        let json = serde_json::to_string(&original).expect("serialize RequestEntry");
        let restored: RequestEntry = serde_json::from_str(&json).expect("deserialize RequestEntry");
        assert_eq!(original.request_id, restored.request_id);
        assert_eq!(original.tenant_id, restored.tenant_id);
        assert_eq!(original.model, restored.model);
        assert_eq!(original.provider, restored.provider);
        assert_eq!(original.status, restored.status);
        assert_eq!(original.latency_ms, restored.latency_ms);
        assert_eq!(original.prompt_tokens, restored.prompt_tokens);
        assert_eq!(original.completion_tokens, restored.completion_tokens);
        assert_eq!(original.total_tokens, restored.total_tokens);
    }

    // ── ClusterHealth roundtrip ──────────────────────────────────────────

    #[test]
    fn cluster_health_serde_roundtrip() {
        let original = sample_cluster_health();
        let json = serde_json::to_string(&original).expect("serialize ClusterHealth");
        let restored: ClusterHealth =
            serde_json::from_str(&json).expect("deserialize ClusterHealth");
        assert_eq!(original.total_nodes, restored.total_nodes);
        assert_eq!(original.healthy_nodes, restored.healthy_nodes);
        assert_eq!(original.total_gpus, restored.total_gpus);
        assert_eq!(original.active_gpus, restored.active_gpus);
        assert_eq!(original.models_loaded, restored.models_loaded);
        assert_eq!(original.requests_per_sec, restored.requests_per_sec);
        assert_eq!(original.error_rate_pct, restored.error_rate_pct);
    }

    #[test]
    fn cluster_health_default_is_zero() {
        let h = ClusterHealth::default();
        assert_eq!(h.total_nodes, 0);
        assert_eq!(h.healthy_nodes, 0);
        assert_eq!(h.total_gpus, 0);
        assert_eq!(h.active_gpus, 0);
        assert_eq!(h.models_loaded, 0);
        assert_eq!(h.requests_per_sec, 0.0);
        assert_eq!(h.error_rate_pct, 0.0);
    }

    // ── WsMessage serde roundtrip + tag check ────────────────────────────

    #[test]
    fn ws_message_node_update_roundtrip() {
        let metrics = vec![sample_gpu_metrics()];
        let msg = WsMessage::NodeUpdate(metrics.clone());
        let json = serde_json::to_string(&msg).expect("serialize WsMessage::NodeUpdate");
        // Verify tagged format
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "node_update");
        // Roundtrip
        let restored: WsMessage = serde_json::from_str(&json).expect("deserialize");
        if let WsMessage::NodeUpdate(v) = restored {
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].node_id, "node-1");
        } else {
            panic!("expected NodeUpdate variant");
        }
    }

    #[test]
    fn ws_message_model_update_roundtrip() {
        let msg = WsMessage::ModelUpdate(vec![sample_model_status()]);
        let json = serde_json::to_string(&msg).expect("serialize WsMessage::ModelUpdate");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "model_update");
        let restored: WsMessage = serde_json::from_str(&json).expect("deserialize");
        if let WsMessage::ModelUpdate(v) = restored {
            assert_eq!(v.len(), 1);
        } else {
            panic!("expected ModelUpdate variant");
        }
    }

    #[test]
    fn ws_message_request_log_roundtrip() {
        let msg = WsMessage::RequestLog(sample_request_entry());
        let json = serde_json::to_string(&msg).expect("serialize WsMessage::RequestLog");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "request_log");
        let restored: WsMessage = serde_json::from_str(&json).expect("deserialize");
        if let WsMessage::RequestLog(e) = restored {
            assert_eq!(e.request_id, "req-abc123");
        } else {
            panic!("expected RequestLog variant");
        }
    }

    #[test]
    fn ws_message_health_update_roundtrip() {
        let health = sample_cluster_health();
        let msg = WsMessage::HealthUpdate(health.clone());
        let json = serde_json::to_string(&msg).expect("serialize WsMessage::HealthUpdate");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "health_update");
        let restored: WsMessage = serde_json::from_str(&json).expect("deserialize");
        if let WsMessage::HealthUpdate(h) = restored {
            assert_eq!(h.total_nodes, 3);
            assert_eq!(h.healthy_nodes, 3);
        } else {
            panic!("expected HealthUpdate variant");
        }
    }

    // ── ModelState Display ───────────────────────────────────────────────

    #[test]
    fn model_state_display() {
        assert_eq!(ModelState::Running.to_string(), "running");
        assert_eq!(ModelState::Loading.to_string(), "loading");
        assert_eq!(ModelState::Stopped.to_string(), "stopped");
        assert_eq!(ModelState::Error.to_string(), "error");
        assert_eq!(ModelState::Draining.to_string(), "draining");
    }

    #[test]
    fn model_state_serde_snake_case() {
        let json = serde_json::to_string(&ModelState::Running).unwrap();
        assert_eq!(json, "\"running\"");
        let restored: ModelState = serde_json::from_str("\"draining\"").unwrap();
        assert_eq!(restored, ModelState::Draining);
    }
}
