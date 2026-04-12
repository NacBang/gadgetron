//! Criterion benchmark: `gadgetron_router::Router::resolve()` with RoundRobin.
//!
//! Measures the synchronous `resolve()` call time when 3 providers are
//! registered and the `RoundRobin` strategy selects by atomic counter.
//!
//! No async I/O: `resolve()` is a pure-computation function over an in-memory
//! `HashMap` + `AtomicUsize`.  Expected overhead is well below 1 µs.
//!
//! Setup:
//!   - 3 `FakeProvider` instances keyed "alpha", "beta", "gamma".
//!   - `RoutingStrategy::RoundRobin`.
//!   - `ChatRequest` model = "gpt-4o" (not in costs map → None cost).
//!
//! The `Router` is constructed once outside the timed loop (construction is
//! not the thing being measured).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, Criterion};
use futures::Stream;
use gadgetron_core::{
    error::Result,
    message::{Content, Message, Role},
    provider::{ChatChunk, ChatRequest, ChatResponse, Choice, LlmProvider, ModelInfo, Usage},
    routing::{RoutingConfig, RoutingStrategy},
};
use gadgetron_router::{MetricsStore, Router};

// ---------------------------------------------------------------------------
// Minimal LlmProvider stub — satisfies the trait without I/O
// ---------------------------------------------------------------------------

struct FakeProvider {
    name: String,
}

impl FakeProvider {
    fn create(name: impl Into<String>) -> Arc<dyn LlmProvider> {
        Arc::new(Self { name: name.into() })
    }
}

#[async_trait]
impl LlmProvider for FakeProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        Ok(ChatResponse {
            id: "bench".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Content::Text("ok".to_string()),
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        })
    }

    fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        Box::pin(futures::stream::empty())
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![])
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bench
// ---------------------------------------------------------------------------

fn bench_router_resolve_roundrobin(c: &mut Criterion) {
    // Build the router once outside the timed section.
    let mut providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
    providers.insert("alpha".to_string(), FakeProvider::create("alpha"));
    providers.insert("beta".to_string(), FakeProvider::create("beta"));
    providers.insert("gamma".to_string(), FakeProvider::create("gamma"));

    let config = RoutingConfig {
        default_strategy: RoutingStrategy::RoundRobin,
        fallbacks: HashMap::new(),
        costs: HashMap::new(),
    };

    let metrics = Arc::new(MetricsStore::new());
    let router = Router::new(providers, config, metrics);

    // Minimal ChatRequest — only `model` and `messages` are inspected by resolve().
    let req = ChatRequest {
        model: "gpt-4o".to_string(),
        messages: vec![Message::user("hi")],
        temperature: None,
        max_tokens: None,
        top_p: None,
        tools: None,
        stream: false,
        stop: None,
    };

    let mut group = c.benchmark_group("router");

    group.bench_function("resolve_roundrobin_3_providers", |b| {
        b.iter(|| {
            let decision = router.resolve(&req).expect("resolve must succeed");
            // Prevent dead-code elimination.
            std::hint::black_box(decision);
        })
    });

    group.finish();
}

criterion_group!(benches, bench_router_resolve_roundrobin);
criterion_main!(benches);
