//! Integration tests for W3-WEB-2 workbench read endpoints.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md`
//! Decision: `docs/process/04-decision-log.md` D-20260418-14
//!
//! These tests exercise the full middleware stack (auth → tenant-ctx → scope →
//! handler) against the real `build_router` to guard against regressions.

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
        catalog::DescriptorCatalog, projection::InProcessWorkbenchProjection,
        workbench::GatewayWorkbenchService,
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

fn make_state(scopes: Vec<Scope>) -> AppState {
    let (audit_writer, _rx) = AuditWriter::new(AUDIT_CAPACITY);
    let projection = Arc::new(InProcessWorkbenchProjection {
        knowledge: None,
        gateway_version: "0.0.0-test",
        descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
            DescriptorCatalog::seed_p2b().into_snapshot(),
        )),
    });
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
            actions: None,
            approval_store: None,
            descriptor_catalog: None,
            catalog_path: None,
            bundles_dir: None,
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

// ---------------------------------------------------------------------------
// Integration test 1: OpenAiCompat key can call bootstrap
// ---------------------------------------------------------------------------

/// An `OpenAiCompat` API key must be able to reach
/// `GET /api/v1/web/workbench/bootstrap` and receive HTTP 200.
///
/// This verifies that the scope exception for `/api/v1/web/workbench/` is
/// correctly wired and that the workbench service returns a valid response.
#[tokio::test]
async fn openai_compat_key_can_call_bootstrap() {
    let state = make_state(vec![Scope::OpenAiCompat]);
    let app = build_router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/web/workbench/bootstrap")
        .header("authorization", format!("Bearer {VALID_TOKEN}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "OpenAiCompat key must reach workbench bootstrap"
    );

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Degraded mode (no knowledge service) but still valid JSON shape.
    assert!(
        value.get("gateway_version").is_some(),
        "response must have gateway_version field"
    );
    assert!(
        value.get("active_plugs").is_some(),
        "response must have active_plugs field"
    );
}

// ---------------------------------------------------------------------------
// Integration test 2: scope regression guard
// ---------------------------------------------------------------------------

/// An `OpenAiCompat` key must NOT be able to reach `/api/v1/nodes`
/// (which requires `Management` scope). This is the scope regression guard
/// that ensures the workbench exception does not accidentally widen access
/// to the management namespace.
#[tokio::test]
async fn openai_compat_key_cannot_call_management_routes() {
    let state = make_state(vec![Scope::OpenAiCompat]);
    let app = build_router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/nodes")
        .header("authorization", format!("Bearer {VALID_TOKEN}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "OpenAiCompat key must be blocked from /api/v1/nodes (Management scope required)"
    );

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify OpenAI error shape is preserved for 403 responses.
    assert_eq!(
        value["error"]["type"], "permission_error",
        "403 must return permission_error type"
    );
}
