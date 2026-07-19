use std::{collections::HashMap, sync::Arc, time::Duration};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Extension, Router,
};
use gadgetron_core::{
    agent::{AgentConfig, GadgetsConfig},
    audit::NoopGadgetAuditEventSink,
    context::{QuotaSnapshot, Scope, TenantContext},
    error::GadgetronError,
};
use gadgetron_gateway::{
    chat_jobs::JobStore,
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
    autonomy::{self, EnqueueBundleEvent, RunDisposition, RunFinish, SyncBundleSchedule},
    billing::BillingFailureCounter,
    knowledge_spaces::{self as spaces, SpaceActor},
    quota::enforcer::InMemoryQuotaEnforcer,
    teams,
};
use serde_json::{json, Value};
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

fn state(pool: sqlx::PgPool) -> AppState {
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
            gadget_modes: Some(Arc::new(ArcSwap::from_pointee(GadgetsConfig::default()))),
            gadget_mode_reconfigurer: None,
            agent_brain: None,
            agent_config_base: None,
            vault_layout: None,
        })),
        penny_shared_surface: None,
        penny_assembler: None,
        agent_config: Arc::new(AgentConfig::default()),
        google_oauth: None,
        activity_capture_store: None,
        candidate_coordinator: None,
        activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
        tool_catalog: None,
        gadget_dispatcher: None,
        tool_audit_sink: Arc::new(NoopGadgetAuditEventSink),
        billing_failures: Arc::new(BillingFailureCounter::new()),
        chat_jobs: Arc::new(JobStore::new()),
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

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn insert_identity(pool: &sqlx::PgPool, tenant_id: Uuid, user_id: Uuid, suffix: &str) {
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant_id)
        .bind(format!("manager-{suffix}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1,$2,$3,'Manager','admin','test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(format!("manager-{suffix}@http.test"))
    .execute(pool)
    .await
    .unwrap();
}

async fn transition(app: &Router, directive_id: &str, revision: i64, state: &str) -> Value {
    let mut body = json!({
        "expected_revision": revision,
        "state": state,
        "summary": format!("{state} recorded by the Manager")
    });
    match state {
        "planned" => body["plan_summary"] = json!("Use the bounded correction path"),
        "verifying" => body["execution_summary"] = json!("Correction completed"),
        "resolved" => {
            body["verification_summary"] = json!("Desired state independently verified");
            body["evidence_refs"] = json!(["audit-event:verified-correction"]);
            body["before_summary"] = json!("Source was missing");
            body["after_summary"] = json!("Source is present and verified");
        }
        _ => {}
    }
    let (status, body) = call(
        app,
        Method::POST,
        &format!("/admin/directives/{directive_id}/transition"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "transition response: {body}");
    body
}

#[tokio::test]
async fn r3_5_http_keeps_manager_lifecycle_tenant_safe_and_webhook_secret_write_only() {
    if !pg_available().await {
        eprintln!("skipping R3.5 HTTP fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    insert_identity(harness.pool(), tenant_a, user_a, "a").await;
    insert_identity(harness.pool(), tenant_b, user_b, "b").await;

    let app_a = workbench_routes()
        .with_state(state(harness.pool().clone()))
        .layer(Extension(context(tenant_a, user_a)));
    let app_b = workbench_routes()
        .with_state(state(harness.pool().clone()))
        .layer(Extension(context(tenant_b, user_b)));

    let create_body = json!({
        "target_kind": "configuration",
        "target_id": "server-monitoring/profile-1",
        "target_revision": "7",
        "instruction": "Restore metric collection without changing the host key",
        "desired_outcome": "Telemetry is current and independently verified",
        "constraints": ["Preserve the host key"],
        "priority": "urgent"
    });
    let (status, created) = call(
        &app_a,
        Method::POST,
        "/admin/directives",
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create response: {created}");
    let directive_id = created["directive"]["id"].as_str().unwrap().to_string();

    let (status, _) = call(
        &app_b,
        Method::GET,
        &format!("/admin/directives/{directive_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, list_b) = call(&app_b, Method::GET, "/admin/oversight", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list_b["returned"], 0);

    let (status, skipped) = call(
        &app_a,
        Method::POST,
        &format!("/admin/directives/{directive_id}/transition"),
        Some(
            json!({"expected_revision": 1, "state": "planned", "summary": "Skip a required stage"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "skip response: {skipped}");

    let mut revision = 1;
    let mut current = Value::Null;
    for state in [
        "acknowledged",
        "planned",
        "executing",
        "verifying",
        "resolved",
    ] {
        current = transition(&app_a, &directive_id, revision, state).await;
        revision = current["directive"]["revision"].as_i64().unwrap();
    }
    assert_eq!(current["directive"]["state"], "resolved");
    assert_eq!(
        current["oversight"]["record"]["verification_state"],
        "verified"
    );
    assert_eq!(current["oversight"]["events"].as_array().unwrap().len(), 6);

    let endpoint = "https://manager-alerts.example.test/gadgetron";
    let (status, webhook) = call(
        &app_a,
        Method::PATCH,
        "/admin/exception-webhook",
        Some(json!({
            "enabled": true,
            "destination_url": endpoint,
            "review_base_url": "https://gadgetron.example.test",
            "expected_revision": 0
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "webhook response: {webhook}");
    assert_eq!(webhook["configured"], true);
    assert_eq!(webhook["destination_host"], "manager-alerts.example.test");
    assert!(!webhook.to_string().contains(endpoint));
    assert!(webhook.get("endpoint_url").is_none());

    let (status, escalated) =
        call(&app_a, Method::POST, "/admin/directives", Some(create_body)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "second create response: {escalated}"
    );
    let escalated_id = escalated["directive"]["id"].as_str().unwrap();
    let escalated = transition(&app_a, escalated_id, 1, "escalated").await;
    assert_eq!(escalated["directive"]["state"], "escalated");
    assert_eq!(escalated["oversight"]["record"]["outcome"], "safe_stopped");

    let (_, exceptions) = call(&app_a, Method::GET, "/admin/exceptions", None).await;
    let (_, deliveries) = call(
        &app_a,
        Method::GET,
        "/admin/exception-webhook/deliveries",
        None,
    )
    .await;
    assert_eq!(exceptions["returned"], 1);
    assert_eq!(deliveries["returned"], 1);
    assert_eq!(deliveries["deliveries"][0]["state"], "pending");

    harness.cleanup().await;
}

#[tokio::test]
async fn r3_6_http_lists_tenant_goals_and_only_space_managers_resume_safe_stops() {
    if !pg_available().await {
        eprintln!("skipping R3.6 HTTP fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let manager_id = Uuid::new_v4();
    let member_id = Uuid::new_v4();
    let other_manager_id = Uuid::new_v4();
    insert_identity(harness.pool(), tenant_a, manager_id, "autonomy-a").await;
    insert_identity(harness.pool(), tenant_b, other_manager_id, "autonomy-b").await;
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1,$2,'operator-autonomy@http.test','Operator','member','test')",
    )
    .bind(member_id)
    .bind(tenant_a)
    .execute(harness.pool())
    .await
    .unwrap();
    teams::create_team(
        harness.pool(),
        tenant_a,
        "platform-operations",
        "Platform Operations",
        None,
        Some(manager_id),
    )
    .await
    .unwrap();
    teams::add_team_member(
        harness.pool(),
        tenant_a,
        "platform-operations",
        member_id,
        "member",
        Some(manager_id),
    )
    .await
    .unwrap();
    let manager = SpaceActor {
        tenant_id: tenant_a,
        user_id: manager_id,
    };
    let team_space = spaces::ensure_team_space(
        harness.pool(),
        manager,
        "platform-operations",
        "Platform Operations",
    )
    .await
    .unwrap();
    autonomy::sync_bundle_schedule(
        harness.pool(),
        SyncBundleSchedule {
            goal_key: format!("http-autonomy:{tenant_a}:edge-one"),
            goal: "Keep the edge server observable and recover monitoring safely".into(),
            tenant_id: tenant_a,
            owner_bundle_id: "server-administrator".into(),
            recipe_id: "server-duty-cycle".into(),
            package_manifest_sha256: "a".repeat(64),
            target_kind: "ssh".into(),
            target_id: "edge-one".into(),
            target_revision: Uuid::new_v4().to_string(),
            target_label: "Edge one".into(),
            acting_space_id: Some(team_space.id),
            requested_by_user_id: Some(manager_id),
            interval: Duration::from_secs(300),
            max_wall_time: Duration::from_secs(120),
            max_attempts: 2,
        },
    )
    .await
    .unwrap();
    let lease = autonomy::lease_next(harness.pool(), "http-worker", 30)
        .await
        .unwrap()
        .unwrap();
    let stopped = autonomy::finish_run(
        harness.pool(),
        &lease,
        "http-worker",
        RunFinish {
            outcome: "safe_stopped".into(),
            verification_state: "failed".into(),
            verification_summary: "Bounded verification could not prove recovery".into(),
            evidence_refs: vec!["autonomy-http:run-one".into()],
            disposition: RunDisposition::SafeStopped,
        },
    )
    .await
    .unwrap();

    let app_manager = workbench_routes()
        .with_state(state(harness.pool().clone()))
        .layer(Extension(context(tenant_a, manager_id)));
    let app_member = workbench_routes()
        .with_state(state(harness.pool().clone()))
        .layer(Extension(context(tenant_a, member_id)));
    let app_other_tenant = workbench_routes()
        .with_state(state(harness.pool().clone()))
        .layer(Extension(context(tenant_b, other_manager_id)));

    let (status, list) = call(&app_manager, Method::GET, "/admin/autonomy/goals", None).await;
    assert_eq!(status, StatusCode::OK, "autonomy list: {list}");
    assert_eq!(list["returned"], 1);
    assert_eq!(list["goals"][0]["status"], "safe_stopped");
    assert_eq!(
        list["goals"][0]["acting_space_title"],
        "Platform Operations"
    );
    assert_eq!(list["goals"][0]["target_label"], "Edge one");

    let (status, foreign_list) = call(
        &app_other_tenant,
        Method::GET,
        "/admin/autonomy/goals",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(foreign_list["returned"], 0);
    let (status, _) = call(
        &app_other_tenant,
        Method::GET,
        &format!("/admin/autonomy/goals/{}", stopped.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let resume = json!({"expected_revision": stopped.revision});
    let (status, _) = call(
        &app_member,
        Method::POST,
        &format!("/admin/autonomy/goals/{}/resume", stopped.id),
        Some(resume.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, stale) = call(
        &app_manager,
        Method::POST,
        &format!("/admin/autonomy/goals/{}/resume", stopped.id),
        Some(json!({"expected_revision": stopped.revision - 1})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale resume: {stale}");
    let (status, resumed) = call(
        &app_manager,
        Method::POST,
        &format!("/admin/autonomy/goals/{}/resume", stopped.id),
        Some(resume),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "manager resume: {resumed}");
    assert_eq!(resumed["status"], "ready");
    assert_eq!(resumed["acting_space_title"], "Platform Operations");

    let mut tx = harness.pool().begin().await.unwrap();
    let event = autonomy::enqueue_bundle_event_in_transaction(
        &mut tx,
        EnqueueBundleEvent {
            tenant_id: tenant_a,
            event_kind: "server-log-finding-created".into(),
            subject_bundle_id: "server-administrator".into(),
            subject_kind: "log-finding".into(),
            subject_id: Uuid::new_v4().to_string(),
            subject_revision: "1".repeat(64),
            event_payload: json!({"subject":{"summary":"preserved rule evidence"}}),
            owner_bundle_id: "server-operations-intelligence".into(),
            recipe_id: "server-log-finding-enrichment".into(),
            package_manifest_sha256: "b".repeat(64),
            agent_role_id: "server-log-finding-enricher".into(),
            result_gadget: "serverintelligence.finding-enrich-attach".into(),
            goal: "Attach bounded finding context".into(),
            acting_space_id: team_space.id,
            requested_by_user_id: manager_id,
            service_actor_user_id: manager_id,
            effective_role: "manager".into(),
            max_wall_seconds: 120,
            max_attempts: 2,
            agent_profile_snapshot: json!({"model":"fast","revision":"profile:1"}),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (status, event_detail) = call(
        &app_manager,
        Method::GET,
        &format!("/admin/autonomy/goals/{}", event.goal.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "event detail: {event_detail}");
    assert_eq!(event_detail["goal"]["source_kind"], "bundle_event");
    assert_eq!(
        event_detail["goal"]["event_kind"],
        "server-log-finding-created"
    );
    assert_eq!(
        event_detail["goal"]["agent_profile_snapshot"]["model"],
        "fast"
    );
    assert_eq!(event_detail["runs"], json!([]));

    harness.cleanup().await;
}
