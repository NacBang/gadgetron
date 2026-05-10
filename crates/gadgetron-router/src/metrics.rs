use dashmap::DashMap;
use gadgetron_core::routing::ProviderMetrics;
use std::collections::HashMap;

/// Thread-safe metrics store keyed by (provider, model).
pub struct MetricsStore {
    data: DashMap<(String, String), ProviderMetrics>,
}

impl MetricsStore {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
        }
    }

    pub fn record_success(
        &self,
        provider: &str,
        model: &str,
        latency_ms: u64,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: f64,
    ) {
        self.data
            .entry((provider.to_string(), model.to_string()))
            .and_modify(|m| {
                m.record_success(
                    latency_ms,
                    input_tokens as u64,
                    output_tokens as u64,
                    cost_usd,
                )
            })
            .or_insert_with(|| {
                let mut m = ProviderMetrics::default();
                m.record_success(
                    latency_ms,
                    input_tokens as u64,
                    output_tokens as u64,
                    cost_usd,
                );
                m
            });
    }

    pub fn record_error(&self, provider: &str, model: &str, latency_ms: u64, error: String) {
        self.data
            .entry((provider.to_string(), model.to_string()))
            .and_modify(|m| m.record_error(latency_ms, error.clone()))
            .or_insert_with(|| {
                let mut m = ProviderMetrics::default();
                m.record_error(latency_ms, error);
                m
            });
    }

    pub fn get(&self, key: &(String, String)) -> ProviderMetrics {
        self.data
            .get(key)
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }

    pub fn all_metrics(&self) -> HashMap<(String, String), ProviderMetrics> {
        self.data
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }
}

impl Default for MetricsStore {
    fn default() -> Self {
        Self::new()
    }
}
