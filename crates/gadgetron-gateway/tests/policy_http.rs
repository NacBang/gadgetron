use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension,
};
use gadgetron_core::{
    agent::{GadgetMode, GadgetsConfig},
    context::{QuotaSnapshot, Scope, TenantContext},
    error::GadgetronError,
};
use gadgetron_gateway::{
    server::AppState,
    web::{
        catalog::DescriptorCatalog,
        projection::InProcessWorkbenchProjection,
        workbench::{workbench_routes, GatewayWorkbenchService},
    },
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    auth::validator::{KeyValidator, ValidatedKey},
    quota::enforcer::InMemoryQuotaEnforcer,
};
use tower::ServiceExt;
use uuid::Uuid;

struct UnusedValidator;

#[async_trait]
impl KeyValidator for UnusedValidator {
    async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        Err(GadgetronError::Forbidden)
    }

    async fn invalidate(&self, _key_hash: &str) {}
}

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .is_ok()
}

fn state(pool: sqlx::PgPool, modes: GadgetsConfig) -> AppState {
    let (audit_writer, _rx) = AuditWriter::new(64);
    let catalog = Arc::new(ArcSwap::from_pointee(
        DescriptorCatalog::seed_p2b().into_snapshot(),
    ));
    let projection = Arc::new(InProcessWorkbenchProjection {
        knowledge: None,
        gateway_version: "test",
        descriptor_catalog: catalog.clone(),
        dynamic_workbench: None,
    });
    AppState {
        key_validator: Arc::new(UnusedValidator),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(HashMap::new()),
        router: None,
        pg_pool: Some(pool),
        no_db: false,
        tui_tx: None,
        workbench: Some(Arc::new(GatewayWorkbenchService {
            projection,
            actions: None,
            approval_store: None,
            policy_evaluator: None,
            gadget_catalog: None,
            descriptor_catalog: Some(catalog),
            catalog_path: None,
            bundles_dir: None,
            bundle_signing: Default::default(),
            runtime_manager: None,
            gadget_modes: Some(Arc::new(ArcSwap::from_pointee(modes))),
            gadget_mode_reconfigurer: None,
            agent_brain: None,
            agent_config_base: None,
            vault_layout: None,
        })),
        penny_shared_surface: None,
        penny_assembler: None,
        agent_config: Arc::new(gadgetron_core::agent::AgentConfig::default()),
        google_oauth: None,
        activity_capture_store: None,
        candidate_coordinator: None,
        activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
        tool_catalog: None,
        gadget_dispatcher: None,
        tool_audit_sink: Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
        billing_failures: Arc::new(gadgetron_xaas::billing::BillingFailureCounter::new()),
        chat_jobs: Arc::new(gadgetron_gateway::chat_jobs::JobStore::new()),
    }
}

fn context(tenant_id: Uuid, user_id: Uuid) -> TenantContext {
    TenantContext {
        tenant_id,
        api_key_id: Uuid::nil(),
        scopes: vec![Scope::Management],
        quota_snapshot: Arc::new(QuotaSnapshot {
            daily_limit_cents: i64::MAX,
            daily_used_cents: 0,
            monthly_limit_cents: i64::MAX,
            monthly_used_cents: 0,
        }),
        request_id: Uuid::new_v4(),
        started_at: std::time::Instant::now(),
        actor_user_id: Some(user_id),
        actor_api_key_id: None,
    }
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn preview_body() -> serde_json::Value {
    serde_json::json!({
        "input": {
            "action_id": "wiki.write",
            "gadget_name": "wiki.write",
            "namespace": "wiki",
            "effect": "write",
            "risk": "low",
            "requested_scopes": ["management"],
            "actor_scopes": ["management"],
            "evidence": {"state": "sufficient", "references": ["source:1"]},
            "outcome": {"state": "verifiable", "predicate_ref": "note-written-v1"},
            "rollback": {"state": "available", "compensating_action": "wiki.delete"}
        }
    })
}

#[tokio::test]
async fn r3_2a_http_migrates_versions_previews_and_keeps_preview_out_of_ledger() {
    if !pg_available().await {
        eprintln!("skipping R3.2a HTTP fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'policy-http')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1,$2,'policy@http.test','Policy Manager','admin','test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .execute(harness.pool())
    .await
    .unwrap();

    let original = GadgetsConfig::default();
    let app = workbench_routes()
        .with_state(state(harness.pool().clone(), original.clone()))
        .layer(Extension(context(tenant_id, user_id)));

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/policy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = json(first).await;
    assert_eq!(first["policy"]["revision"], 1);
    assert_eq!(first["policy"]["source"], "legacy_migration");
    assert_eq!(first["enforcement_coverage"]["overall"], "unavailable");

    let preview = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/policy/preview")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&preview_body()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(json(preview).await["trace"]["decision"], "auto");

    let mut changed = original;
    changed.write.wiki_write = GadgetMode::Never;
    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/policy/legacy-revisions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "expected_revision": 1,
                        "gadgets": changed
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(json(update).await["policy"]["revision"], 2);

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/policy/preview")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&preview_body()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::OK);
    assert_eq!(json(denied).await["trace"]["decision"], "deny");

    let stale = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/policy/legacy-revisions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "expected_revision": 1,
                        "gadgets": GadgetsConfig::default()
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        json(stale).await["error"]["code"],
        "policy_revision_conflict"
    );

    let decisions = app
        .oneshot(
            Request::builder()
                .uri("/admin/policy/decisions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(decisions.status(), StatusCode::OK);
    assert_eq!(json(decisions).await["count"], 0);

    harness.cleanup().await;
}
