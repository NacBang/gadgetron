use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutingStrategy {
    RoundRobin,
    CostOptimal,
    LatencyOptimal,
    QualityOptimal,
    Fallback { chain: Vec<String> },
    Weighted { weights: HashMap<String, f32> },
}

impl Default for RoutingStrategy {
    fn default() -> Self {
        Self::RoundRobin
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoutingConfig {
    pub default_strategy: RoutingStrategy,
    #[serde(default)]
    pub fallbacks: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub costs: HashMap<String, CostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEntry {
    pub input: f64,
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub provider: String,
    pub model: String,
    pub strategy: RoutingStrategy,
    pub estimated_cost_usd: Option<f64>,
    pub fallback_chain: Vec<String>,
}

/// Metrics stored per provider+model for routing decisions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderMetrics {
    pub total_requests: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub last_error: Option<String>,
    pub last_latency_ms: Option<u64>,
}

impl ProviderMetrics {
    pub fn avg_latency_ms(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / self.total_requests as f64
        }
    }

    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_errors as f64 / self.total_requests as f64
        }
    }

    pub fn record_success(
        &mut self,
        latency_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) {
        self.total_requests += 1;
        self.total_latency_ms += latency_ms;
        self.total_input_tokens += input_tokens;
        self.total_output_tokens += output_tokens;
        self.total_cost_usd += cost_usd;
        self.last_latency_ms = Some(latency_ms);
    }

    pub fn record_error(&mut self, latency_ms: u64, error: String) {
        self.total_requests += 1;
        self.total_errors += 1;
        self.total_latency_ms += latency_ms;
        self.last_error = Some(error);
        self.last_latency_ms = Some(latency_ms);
    }
}
