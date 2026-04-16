use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::provider::{ChatChunk, ChatRequest, ChatResponse, LlmProvider, ModelInfo};
use gadgetron_core::routing::{RoutingConfig, RoutingDecision, RoutingStrategy};

use crate::metrics::MetricsStore;

pub struct Router {
    providers: Arc<HashMap<String, Arc<dyn LlmProvider>>>,
    config: RoutingConfig,
    metrics: Arc<MetricsStore>,
    round_robin_counter: AtomicUsize,
}

impl Router {
    pub fn new(
        providers: HashMap<String, Arc<dyn LlmProvider>>,
        config: RoutingConfig,
        metrics: Arc<MetricsStore>,
    ) -> Self {
        Self {
            providers: Arc::new(providers),
            config,
            metrics,
            round_robin_counter: AtomicUsize::new(0),
        }
    }

    pub fn resolve(&self, req: &ChatRequest) -> Result<RoutingDecision> {
        let strategy = &self.config.default_strategy;
        let model = &req.model;

        let fallback_chain = self
            .config
            .fallbacks
            .get(model)
            .cloned()
            .unwrap_or_default();

        // Direct-match: if the model name exactly matches a registered
        // provider name, route directly to that provider. This allows
        // providers like "kairos" to be addressed by model name without
        // the routing strategy sending the request to a random provider.
        if self.providers.contains_key(model) {
            return Ok(RoutingDecision {
                provider: model.clone(),
                model: model.clone(),
                strategy: strategy.clone(),
                estimated_cost_usd: self.estimate_cost(model),
                fallback_chain,
            });
        }

        let (provider_name, estimated_cost) = match strategy {
            RoutingStrategy::RoundRobin => {
                let available: Vec<&String> = self.providers.keys().collect();
                if available.is_empty() {
                    return Err(GadgetronError::Routing(format!(
                        "no provider available for model {}",
                        model
                    )));
                }
                let idx =
                    self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % available.len();
                let name = available[idx].clone();
                let cost = self.estimate_cost(model);
                (name, cost)
            }
            RoutingStrategy::CostOptimal => self.cheapest_provider(model)?,
            RoutingStrategy::LatencyOptimal => self.fastest_provider(model)?,
            RoutingStrategy::QualityOptimal => {
                self.prefer_provider(model, &["anthropic", "openai", "ollama"])?
            }
            RoutingStrategy::Fallback { chain } => {
                let mut chosen = None;
                for name in chain {
                    if self.providers.contains_key(name) {
                        chosen = Some(name.clone());
                        break;
                    }
                }
                let name = chosen.ok_or_else(|| {
                    GadgetronError::Routing(format!("no provider available for model {}", model))
                })?;
                let cost = self.estimate_cost(model);
                (name, cost)
            }
            RoutingStrategy::Weighted { weights } => self.weighted_provider(model, weights)?,
        };

        Ok(RoutingDecision {
            provider: provider_name,
            model: model.clone(),
            strategy: strategy.clone(),
            estimated_cost_usd: estimated_cost,
            fallback_chain,
        })
    }

    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let decision = self.resolve(&req)?;
        let provider = self.get_provider(&decision.provider)?;

        let start = std::time::Instant::now();
        match provider.chat(req.clone()).await {
            Ok(resp) => {
                let latency = start.elapsed().as_millis() as u64;
                let cost = self.estimate_cost_value(&resp.usage);
                self.metrics.record_success(
                    &decision.provider,
                    &decision.model,
                    latency,
                    resp.usage.prompt_tokens,
                    resp.usage.completion_tokens,
                    cost,
                );
                Ok(resp)
            }
            Err(e) => {
                let latency = start.elapsed().as_millis() as u64;
                self.metrics.record_error(
                    &decision.provider,
                    &decision.model,
                    latency,
                    e.to_string(),
                );
                self.try_fallbacks(req, &decision.fallback_chain).await
            }
        }
    }

    pub fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatChunk>> + Send>> {
        let decision = self.resolve(&req);
        match decision {
            Ok(d) => {
                let provider_name = d.provider.clone();
                if let Some(provider) = self.providers.get(&provider_name) {
                    let provider = provider.clone();
                    provider.chat_stream(req)
                } else {
                    Box::pin(futures::stream::once(async move {
                        Err(GadgetronError::Routing(format!(
                            "no provider available for model {}",
                            d.model
                        )))
                    }))
                }
            }
            Err(e) => Box::pin(futures::stream::once(async { Err(e) })),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let mut all_models = Vec::new();
        for provider in self.providers.values() {
            if let Ok(models) = provider.models().await {
                all_models.extend(models);
            }
        }
        Ok(all_models)
    }

    pub async fn health_check_all(&self) -> HashMap<String, bool> {
        let mut results = HashMap::new();
        for (name, provider) in self.providers.iter() {
            let healthy = provider.health().await.is_ok();
            results.insert(name.clone(), healthy);
        }
        results
    }

    fn get_provider(&self, name: &str) -> Result<Arc<dyn LlmProvider>> {
        self.providers
            .get(name)
            .cloned()
            .ok_or_else(|| GadgetronError::Routing(format!("provider not found: {}", name)))
    }

    async fn try_fallbacks(
        &self,
        original_req: ChatRequest,
        fallback_chain: &[String],
    ) -> Result<ChatResponse> {
        for fallback_name in fallback_chain {
            if let Some(provider) = self.providers.get(fallback_name) {
                let start = std::time::Instant::now();
                match provider.chat(original_req.clone()).await {
                    Ok(resp) => {
                        let latency = start.elapsed().as_millis() as u64;
                        let cost = self.estimate_cost_value(&resp.usage);
                        self.metrics.record_success(
                            fallback_name,
                            &original_req.model,
                            latency,
                            resp.usage.prompt_tokens,
                            resp.usage.completion_tokens,
                            cost,
                        );
                        return Ok(resp);
                    }
                    Err(e) => {
                        let latency = start.elapsed().as_millis() as u64;
                        self.metrics.record_error(
                            fallback_name,
                            &original_req.model,
                            latency,
                            e.to_string(),
                        );
                        continue;
                    }
                }
            }
        }
        Err(GadgetronError::Routing(format!(
            "all providers failed for model {}",
            original_req.model
        )))
    }

    fn cheapest_provider(&self, model: &str) -> Result<(String, Option<f64>)> {
        let mut best: Option<(String, f64)> = None;
        for name in self.providers.keys() {
            if let Some(cost) = self.config.costs.get(model) {
                let total = cost.input + cost.output;
                match &best {
                    Some((_, best_cost)) if total >= *best_cost => {}
                    _ => best = Some((name.clone(), total)),
                }
            } else if best.is_none() {
                best = Some((name.clone(), 0.0));
            }
        }
        best.map(|(n, c)| (n, Some(c / 1_000_000.0)))
            .ok_or_else(|| {
                GadgetronError::Routing(format!("no provider available for model {}", model))
            })
    }

    fn fastest_provider(&self, model: &str) -> Result<(String, Option<f64>)> {
        let mut best: Option<(String, u64)> = None;
        for name in self.providers.keys() {
            let metrics = self.metrics.get(&(name.clone(), model.to_string()));
            let avg_latency = metrics.avg_latency_ms() as u64;
            match &best {
                Some((_, best_lat)) if avg_latency >= *best_lat => {}
                None if metrics.total_requests == 0 => {
                    best = Some((name.clone(), 0));
                }
                _ => best = Some((name.clone(), avg_latency)),
            }
        }
        best.map(|(n, _)| (n, None)).ok_or_else(|| {
            GadgetronError::Routing(format!("no provider available for model {}", model))
        })
    }

    fn prefer_provider(&self, model: &str, order: &[&str]) -> Result<(String, Option<f64>)> {
        for name in order {
            if self.providers.contains_key(*name) {
                return Ok((name.to_string(), self.estimate_cost(model)));
            }
        }
        self.providers
            .keys()
            .next()
            .map(|n| (n.clone(), self.estimate_cost(model)))
            .ok_or_else(|| {
                GadgetronError::Routing(format!("no provider available for model {}", model))
            })
    }

    fn weighted_provider(
        &self,
        model: &str,
        weights: &HashMap<String, f32>,
    ) -> Result<(String, Option<f64>)> {
        use rand::RngExt;
        let total: f32 = weights.values().sum();
        if total <= 0.0 {
            return Err(GadgetronError::Routing(format!(
                "no provider available for model {}",
                model
            )));
        }
        let mut rng = rand::rng();
        let mut roll = rng.random_range(0.0..total);
        for (name, weight) in weights {
            roll -= weight;
            if roll <= 0.0 && self.providers.contains_key(name) {
                return Ok((name.clone(), self.estimate_cost(model)));
            }
        }
        self.providers
            .keys()
            .next()
            .map(|n| (n.clone(), self.estimate_cost(model)))
            .ok_or_else(|| {
                GadgetronError::Routing(format!("no provider available for model {}", model))
            })
    }

    fn estimate_cost(&self, model: &str) -> Option<f64> {
        self.config
            .costs
            .get(model)
            .map(|c| (c.input + c.output) / 1_000_000.0)
    }

    fn estimate_cost_value(&self, _usage: &gadgetron_core::provider::Usage) -> f64 {
        // Simplified cost estimation based on configured costs
        0.0
    }
}
