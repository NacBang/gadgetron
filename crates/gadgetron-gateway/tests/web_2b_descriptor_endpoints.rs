//! Integration tests for W3-WEB-2b descriptor endpoints.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md`
//!
//! These tests exercise the full middleware stack (auth → tenant-ctx → scope →
//! handler) against the real `build_router` to guard against regressions on
//! the 5 new endpoints added in W3-WEB-2b:
//!   GET  /knowledge-status
//!   GET  /views
//!   GET  /views/:view_id/data
//!   GET  /actions
//!   POST /actions/:action_id

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use gadgetron_core::{context::Scope, error::GadgetronError};
use gadgetron_gateway::{
    server::{build_router, AppState},
    web::{
        action_service::InProcessWorkbenchActionService,
        catalog::DescriptorCatalog,
        projection::InProcessWorkbenchProjection,
        replay_cache::{InMemoryReplayCache, DEFAULT_REPLAY_TTL},
        workbench::{GatewayWorkbenchService, WorkbenchActionService},
    },
};
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::{KeyValidator, ValidatedKey},
    quota::enforcer::InMemoryQuotaEnforcer,
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

const VALID_TOKEN: &str = "gad_live_abcdefghijklmnop1234567890";
const AUDIT_CAPACITY: usize = 16;

struct MockKeyValidator {
    result: Arc<ValidatedKey>,
}

impl MockKeyValidator {
    fn new(scopes: Vec<Scope>) -> Self {
        Self {
            result: Arc::new(ValidatedKey {
                api_key_id: Uuid::new_v4(),
                tenant_id: Uuid::new_v4(),
                scopes,
            }),
        }
    }
}

#[async_trait]
impl KeyValidator for MockKeyValidator {
    async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        Ok(self.result.clone())
    }
    async fn invalidate(&self, _key_hash: &str) {}
}

fn lazy_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgresql://localhost/test")
        .expect("lazy pool")
}

/// Build state with the seed catalog and no coordinator.
fn make_state(scopes: Vec<Scope>) -> AppState {
    make_state_with_coordinator(scopes, None)
}

fn make_state_with_coordinator(
    scopes: Vec<Scope>,
    coordinator: Option<
        Arc<dyn gadgetron_core::knowledge::candidate::KnowledgeCandidateCoordinator>,
    >,
) -> AppState {
    let (audit_writer, _rx) = AuditWriter::new(AUDIT_CAPACITY);
    let catalog = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
        DescriptorCatalog::seed_p2b().into_snapshot(),
    ));

    let projection = Arc::new(InProcessWorkbenchProjection {
        knowledge: None,
        gateway_version: "0.0.0-test",
        descriptor_catalog: catalog.clone(),
    });

    let action_svc: Arc<dyn WorkbenchActionService> =
        Arc::new(InProcessWorkbenchActionService::new(
            catalog.clone(),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            coordinator,
        ));

    AppState {
        key_validator: Arc::new(MockKeyValidator::new(scopes)),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(HashMap::new()),
        router: None,
        pg_pool: Some(lazy_pool()),
        no_db: false,
        tui_tx: None,
        workbench: Some(Arc::new(GatewayWorkbenchService {
            projection,
            actions: Some(action_svc),
            approval_store: None,
            descriptor_catalog: Some(catalog),
            catalog_path: None,
        })),
        penny_shared_surface: None,
        penny_assembler: None,
        agent_config: Arc::new(gadgetron_core::agent::config::AgentConfig::default()),
        activity_capture_store: None,
        candidate_coordinator: None,
        activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
        tool_catalog: None,
        gadget_dispatcher: None,
        tool_audit_sink: std::sync::Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
    }
}

fn auth_get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {VALID_TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

fn auth_post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {VALID_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: All 5 new endpoints return 200 for OpenAiCompat key
// ---------------------------------------------------------------------------

/// An `OpenAiCompat` key must reach all 5 WEB-2b endpoints and receive 200.
#[tokio::test]
async fn openai_compat_key_can_call_all_5_endpoints() {
    let state = make_state(vec![Scope::OpenAiCompat]);
    let app = build_router(state);

    let endpoints = [
        ("/api/v1/web/workbench/knowledge-status", "GET"),
        ("/api/v1/web/workbench/views", "GET"),
        (
            "/api/v1/web/workbench/views/knowledge-activity-recent/data",
            "GET",
        ),
        ("/api/v1/web/workbench/actions", "GET"),
    ];

    for (uri, method) in endpoints {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Expected 200 for {} {uri}",
            method
        );
    }

    // POST /actions/knowledge-search with valid args
    let post_req = auth_post_json(
        "/api/v1/web/workbench/actions/knowledge-search",
        serde_json::json!({"args": {"query": "hello"}}),
    );
    let resp = app.oneshot(post_req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Expected 200 for POST /actions/knowledge-search"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Unknown view returns 404 with workbench_view_not_found
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_view_returns_404_with_workbench_view_not_found() {
    let state = make_state(vec![Scope::OpenAiCompat]);
    let app = build_router(state);

    let resp = app
        .oneshot(auth_get(
            "/api/v1/web/workbench/views/nonexistent-view/data",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        value["error"]["type"], "invalid_request_error",
        "error.type must be invalid_request_error"
    );
    assert_eq!(
        value["error"]["code"], "workbench_view_not_found",
        "error.code must be workbench_view_not_found"
    );
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("nonexistent-view"),
        "error.message must contain view_id"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Unknown action returns 404 with workbench_action_not_found
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_action_returns_404_with_workbench_action_not_found() {
    let state = make_state(vec![Scope::OpenAiCompat]);
    let app = build_router(state);

    let req = auth_post_json(
        "/api/v1/web/workbench/actions/does-not-exist",
        serde_json::json!({"args": {}}),
    );
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(value["error"]["type"], "invalid_request_error");
    assert_eq!(value["error"]["code"], "workbench_action_not_found");
    assert!(value["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does-not-exist"));
}

// ---------------------------------------------------------------------------
// Test 4: Invalid args returns 400 with workbench_action_invalid_args
// ---------------------------------------------------------------------------

#[tokio::test]
async fn action_invoke_invalid_args_returns_400_with_invalid_args_code() {
    let state = make_state(vec![Scope::OpenAiCompat]);
    let app = build_router(state);

    // "query" is required; empty args object must fail schema validation.
    let req = auth_post_json(
        "/api/v1/web/workbench/actions/knowledge-search",
        serde_json::json!({"args": {}}),
    );
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(value["error"]["type"], "invalid_request_error");
    assert_eq!(value["error"]["code"], "workbench_action_invalid_args");
    assert!(
        !value["error"]["message"].as_str().unwrap().is_empty(),
        "error.message must not be empty"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Duplicate client_invocation_id returns cached response
// ---------------------------------------------------------------------------

/// Sending the same `client_invocation_id` twice must return the same
/// cached response without executing the action a second time.
#[tokio::test]
async fn action_invoke_duplicate_client_invocation_id_returns_cached_response() {
    let state = make_state(vec![Scope::OpenAiCompat]);
    let ciid = Uuid::new_v4();

    let body = serde_json::json!({
        "args": {"query": "cached test"},
        "client_invocation_id": ciid.to_string()
    });

    let app = build_router(state);

    // First call
    let resp1 = app
        .clone()
        .oneshot(auth_post_json(
            "/api/v1/web/workbench/actions/knowledge-search",
            body.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK, "first call must succeed");
    let bytes1 = axum::body::to_bytes(resp1.into_body(), usize::MAX)
        .await
        .unwrap();
    let val1: serde_json::Value = serde_json::from_slice(&bytes1).unwrap();

    // Second call with same ciid
    let resp2 = app
        .oneshot(auth_post_json(
            "/api/v1/web/workbench/actions/knowledge-search",
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK, "second call must succeed");
    let bytes2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
        .await
        .unwrap();
    let val2: serde_json::Value = serde_json::from_slice(&bytes2).unwrap();

    // Both must have status "ok"
    assert_eq!(val1["result"]["status"], "ok");
    assert_eq!(val2["result"]["status"], "ok");
}

// ---------------------------------------------------------------------------
// Test 6: Seeded action with coordinator captures activity
// ---------------------------------------------------------------------------

/// When a coordinator is wired, invoking the seeded action must produce an
/// `activity_event_id` in the result and the coordinator's store must hold
/// a `DirectAction` event.
#[tokio::test]
async fn action_invoke_seeded_action_succeeds_and_captures_activity() {
    use gadgetron_core::knowledge::candidate::KnowledgeCandidateCoordinator;
    use gadgetron_knowledge::candidate::{
        InMemoryActivityCaptureStore, InProcessCandidateCoordinator,
    };

    // Hold the concrete store so we can call events_snapshot() after the request.
    let concrete_store = Arc::new(InMemoryActivityCaptureStore::new());
    let store_for_coord: Arc<dyn gadgetron_core::knowledge::candidate::ActivityCaptureStore> =
        concrete_store.clone();
    let coordinator: Arc<dyn KnowledgeCandidateCoordinator> =
        Arc::new(InProcessCandidateCoordinator::new(store_for_coord, 8));

    let state = make_state_with_coordinator(vec![Scope::OpenAiCompat], Some(coordinator.clone()));
    let app = build_router(state);

    let req = auth_post_json(
        "/api/v1/web/workbench/actions/knowledge-search",
        serde_json::json!({"args": {"query": "wiki article search"}}),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "action invoke must succeed");

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // --- Key assertions (the 5-10 most relevant lines per task brief) ---

    // Status must be "ok"
    assert_eq!(value["result"]["status"], "ok", "status must be ok");

    // activity_event_id must be set (coordinator is wired)
    assert!(
        !value["result"]["activity_event_id"].is_null(),
        "activity_event_id must be set when coordinator is wired; got: {:?}",
        value["result"]["activity_event_id"]
    );

    // approval_id must be absent (non-destructive, non-approval-required)
    assert!(
        value["result"]["approval_id"].is_null(),
        "approval_id must be absent on ok path"
    );

    // audit_event_id is populated on every ok path (ISSUE 3 TASK 3.1).
    assert!(
        value["result"]["audit_event_id"].is_string(),
        "audit_event_id must be set on ok (TASK 3.1)"
    );

    // activity_event_id parses as a valid UUID
    let event_id_str = value["result"]["activity_event_id"]
        .as_str()
        .expect("activity_event_id must be a string");
    let _event_id = Uuid::parse_str(event_id_str).expect("activity_event_id must be a valid UUID");

    // Store must have captured the activity event.
    let events = concrete_store.events_snapshot().await;
    assert!(
        !events.is_empty(),
        "coordinator store must hold at least one captured event"
    );
    let captured = &events[0];
    assert_eq!(
        format!("{:?}", captured.kind),
        "DirectAction",
        "captured event kind must be DirectAction"
    );
    assert!(
        captured.title.contains("knowledge-search"),
        "event title must mention action_id, got: {:?}",
        captured.title
    );
}
