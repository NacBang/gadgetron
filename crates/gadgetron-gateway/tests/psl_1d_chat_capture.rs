//! W3-PSL-1d integration test — first `capture_action` call site.
//!
//! Authority: D-20260418-20,
//! `docs/design/core/knowledge-candidate-curation.md` §2.1.
//!
//! Verifies that a successful non-streaming `POST /v1/chat/completions`
//! fires the fire-and-forget `capture_chat_completion` helper and lands
//! exactly one `CapturedActivityEvent` with:
//!   - `kind = GadgetToolCall`
//!   - `origin = Penny`
//!   - `title` containing "chat completion"
//!   - `summary` containing the token counts from the fake provider

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use futures::stream;
use gadgetron_core::{
    agent::config::AgentConfig,
    context::Scope,
    knowledge::candidate::{ActivityKind, ActivityOrigin, KnowledgeCandidateCoordinator},
    message::{Content, Message, Role},
    provider::{
        ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, ChunkDelta, LlmProvider,
        ModelInfo, Usage,
    },
};
use gadgetron_gateway::server::{build_router, AppState};
use gadgetron_knowledge::candidate::{InMemoryActivityCaptureStore, InProcessCandidateCoordinator};
use gadgetron_router::{router::Router as LlmRouter, MetricsStore};
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::{KeyValidator, ValidatedKey},
    quota::enforcer::InMemoryQuotaEnforcer,
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const VALID_TOKEN: &str = "gad_live_abcdefghijklmnop1234567890";
const AUDIT_CAPACITY: usize = 16;

// ---------------------------------------------------------------------------
// FakeLlmProvider — returns deterministic token counts for assertion
// ---------------------------------------------------------------------------

/// Fake provider that returns prompt_tokens=5, completion_tokens=7 so the
/// integration test can assert on those values in the captured summary.
struct FakeLlmProvider;

#[async_trait]
impl LlmProvider for FakeLlmProvider {
    async fn chat(&self, _req: ChatRequest) -> gadgetron_core::error::Result<ChatResponse> {
        Ok(ChatResponse {
            id: "chatcmpl-psl1d-test".to_string(),
            object: "chat.completion".to_string(),
            created: 1_700_000_000,
            model: "fake-model".to_string(),
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
                prompt_tokens: 5,
                completion_tokens: 7,
                total_tokens: 12,
            },
        })
    }

    fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = gadgetron_core::error::Result<ChatChunk>> + Send>,
    > {
        Box::pin(stream::iter(vec![Ok(ChatChunk {
            id: "chatcmpl-psl1d-stream".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1_700_000_000,
            model: "fake-model".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: Some("assistant".to_string()),
                    content: Some("ok".to_string()),
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
        })]))
    }

    async fn models(&self) -> gadgetron_core::error::Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: "fake-model".to_string(),
            object: "model".to_string(),
            owned_by: "fake-org".to_string(),
        }])
    }

    fn name(&self) -> &str {
        "fake"
    }

    async fn health(&self) -> gadgetron_core::error::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Auth double
// ---------------------------------------------------------------------------

struct AlwaysAcceptValidator {
    key: Arc<ValidatedKey>,
}

impl AlwaysAcceptValidator {
    fn new() -> Self {
        Self {
            key: Arc::new(ValidatedKey {
                api_key_id: Uuid::new_v4(),
                tenant_id: Uuid::new_v4(),
                scopes: vec![Scope::OpenAiCompat],
            }),
        }
    }
}

#[async_trait]
impl KeyValidator for AlwaysAcceptValidator {
    async fn validate(&self, _key_hash: &str) -> gadgetron_core::error::Result<Arc<ValidatedKey>> {
        Ok(self.key.clone())
    }
    async fn invalidate(&self, _key_hash: &str) {}
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn lazy_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgresql://localhost/test")
        .expect("lazy pool")
}

/// Build AppState wired with the fake provider and a real in-memory
/// candidate coordinator. Returns `(state, store_arc)` so the test can
/// inspect captured events after the request settles.
fn make_state_with_coordinator() -> (AppState, Arc<InMemoryActivityCaptureStore>) {
    let (audit_writer, _rx) = AuditWriter::new(AUDIT_CAPACITY);

    let store = Arc::new(InMemoryActivityCaptureStore::new());
    let coord: Arc<dyn KnowledgeCandidateCoordinator> =
        Arc::new(InProcessCandidateCoordinator::new(
            store.clone() as Arc<dyn gadgetron_core::knowledge::candidate::ActivityCaptureStore>,
            8,
        ));

    let mut providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
    providers.insert("fake".to_string(), Arc::new(FakeLlmProvider));

    let metrics = Arc::new(MetricsStore::new());
    let routing_config = gadgetron_core::routing::RoutingConfig {
        default_strategy: gadgetron_core::routing::RoutingStrategy::RoundRobin,
        fallbacks: HashMap::new(),
        costs: HashMap::new(),
    };
    let lrouter = LlmRouter::new(providers.clone(), routing_config, metrics);

    let providers_for_state: HashMap<String, Arc<dyn LlmProvider + Send + Sync>> = providers
        .into_iter()
        .map(|(k, v)| (k, v as Arc<dyn LlmProvider + Send + Sync>))
        .collect();

    let state = AppState {
        key_validator: Arc::new(AlwaysAcceptValidator::new()),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(providers_for_state),
        router: Some(Arc::new(lrouter)),
        pg_pool: Some(lazy_pool()),
        no_db: false,
        tui_tx: None,
        workbench: None,
        penny_shared_surface: None,
        agent_config: Arc::new(AgentConfig::default()),
        penny_assembler: None,
        activity_capture_store: Some(store.clone()),
        candidate_coordinator: Some(coord),
        activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
    };

    (state, store)
}

fn chat_request_body_non_streaming() -> serde_json::Value {
    serde_json::json!({
        "model": "fake-model",
        "messages": [{"role": "user", "content": "hello"}],
        "stream": false
    })
}

// ---------------------------------------------------------------------------
// PSL-1d: psl_1d_successful_non_streaming_chat_completes_captures_one_event
// ---------------------------------------------------------------------------

/// Successful non-streaming POST /v1/chat/completions with a wired coordinator
/// must land exactly one CapturedActivityEvent with:
///   - `kind  == GadgetToolCall`
///   - `origin == Penny`
///   - `title` contains "chat completion"
///   - `summary` contains "5 input / 7 output"  (token counts from FakeLlmProvider)
#[tokio::test]
async fn psl_1d_successful_non_streaming_chat_completions_captures_one_event() {
    let (state, store) = make_state_with_coordinator();
    let app = build_router(state);

    let body_json = serde_json::to_vec(&chat_request_body_non_streaming()).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {VALID_TOKEN}"))
        .body(Body::from(body_json))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "non-streaming must return 200 OK"
    );

    // Let the fire-and-forget tokio::spawn settle.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Assert exactly one event was captured.
    let events = store.events_snapshot().await;
    assert_eq!(
        events.len(),
        1,
        "exactly one CapturedActivityEvent must be appended after a successful chat completion"
    );

    let ev = &events[0];

    // kind and origin
    assert_eq!(
        ev.kind,
        ActivityKind::GadgetToolCall,
        "kind must be GadgetToolCall"
    );
    assert_eq!(ev.origin, ActivityOrigin::Penny, "origin must be Penny");

    // title contains model name
    assert!(
        ev.title.contains("chat completion"),
        "title must contain 'chat completion'; got: {:?}",
        ev.title
    );

    // summary contains token counts from FakeLlmProvider (5 prompt / 7 completion)
    assert!(
        ev.summary.contains("5 input / 7 output"),
        "summary must contain token counts '5 input / 7 output'; got: {:?}",
        ev.summary
    );

    // source_capability
    assert_eq!(
        ev.source_capability.as_deref(),
        Some("chat.completions"),
        "source_capability must be 'chat.completions'"
    );

    // Drift-fix PR 5 (D-20260418-24): audit_event_id must be a freshly
    // generated Uuid::new_v4(), NOT a reused request_id. The handler
    // generates it once per outcome and threads it through both the
    // AuditEntry and the capture event so audit_log.id ↔
    // captured_activity_event.audit_event_id JOIN is unambiguous.
    assert!(
        ev.audit_event_id.is_some(),
        "audit_event_id must be Some(generated UUID) for correlation; got None"
    );
    assert_ne!(
        ev.audit_event_id,
        Some(uuid::Uuid::nil()),
        "audit_event_id must not be the nil UUID"
    );
    assert_ne!(
        ev.audit_event_id, ev.request_id,
        "audit_event_id MUST differ from request_id per drift-fix PR 5"
    );
}
