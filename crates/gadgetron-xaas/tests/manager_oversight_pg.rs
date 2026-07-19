use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::manager_oversight::{
    self as oversight, CreateDirectiveInput, ManagerOversightError, RecordOutcomeInput,
    StageEventInput, TransitionDirectiveInput, WebhookSettingsInput,
};
use uuid::Uuid;

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

async fn tenant_and_admin(pool: &sqlx::PgPool, label: &str) -> (Uuid, Uuid) {
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant_id)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1,$2,$3,$4,'admin','test')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(format!("{label}@example.test"))
    .bind(label)
    .execute(pool)
    .await
    .unwrap();
    (tenant_id, user_id)
}

fn terminal_input(tenant_id: Uuid, user_id: Uuid) -> RecordOutcomeInput {
    RecordOutcomeInput {
        tenant_id,
        source_kind: "workbench_action".into(),
        source_id: Uuid::new_v4().to_string(),
        actor_user_id: Some(user_id),
        agent_label: "Penny".into(),
        agent_role: "operator".into(),
        goal: "Confirm the registered server state".into(),
        target_kind: "action".into(),
        target_id: "server.metrics.collect".into(),
        target_revision: None,
        policy_decision: "auto".into(),
        policy_revision: Some("policy:7".into()),
        evidence_refs: vec!["outcome:server-one".into()],
        current_stage: "verify".into(),
        outcome: "succeeded".into(),
        verification_state: "verified".into(),
        action_summary: "Server state was collected and verified".into(),
        before_summary: Some("Telemetry was stale".into()),
        after_summary: Some("Telemetry is current".into()),
        rollback_summary: None,
        duration_ms: 42,
        cost_minor_units: 0,
        events: [
            ("target", "recorded", "Registered server selected"),
            ("plan", "completed", "Signed collection plan accepted"),
            ("execute", "completed", "Telemetry collection completed"),
            ("verify", "completed", "Current telemetry outcome verified"),
        ]
        .into_iter()
        .map(|(stage, state, summary)| StageEventInput {
            stage: stage.into(),
            state: state.into(),
            summary: summary.into(),
            evidence_refs: Vec::new(),
        })
        .collect(),
        exception_severity: None,
        exception_summary: None,
    }
}

fn transition(revision: i64, state: &str, summary: &str) -> TransitionDirectiveInput {
    TransitionDirectiveInput {
        expected_revision: revision,
        state: state.into(),
        summary: summary.into(),
        plan_summary: (state == "planned").then(|| "Use the signed recovery action".into()),
        execution_summary: (state == "verifying")
            .then(|| "Recovery action completed without widening scope".into()),
        verification_summary: (state == "resolved")
            .then(|| "The desired server state is current".into()),
        before_summary: None,
        after_summary: None,
        evidence_refs: if state == "resolved" {
            vec!["outcome:verified-server".into()]
        } else {
            Vec::new()
        },
    }
}

#[tokio::test]
async fn r3_5_manager_ledger_directive_exception_and_delivery_are_tenant_safe() {
    if !pg_available().await {
        eprintln!("skipping R3.5 PostgreSQL fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_a, admin_a) = tenant_and_admin(pool, "manager-a").await;
    let (tenant_b, _) = tenant_and_admin(pool, "manager-b").await;

    let recorded = oversight::record_outcome(pool, terminal_input(tenant_a, admin_a))
        .await
        .unwrap();
    let detail = oversight::oversight_detail(pool, tenant_a, recorded.id)
        .await
        .unwrap();
    assert_eq!(detail.events.len(), 4);
    assert_eq!(
        detail
            .events
            .iter()
            .map(|event| event.stage.as_str())
            .collect::<Vec<_>>(),
        ["target", "plan", "execute", "verify"]
    );
    assert!(oversight::list_oversight(pool, tenant_b, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        oversight::oversight_detail(pool, tenant_b, recorded.id).await,
        Err(ManagerOversightError::NotFound)
    ));

    let mut directive = oversight::create_directive(
        pool,
        tenant_a,
        admin_a,
        CreateDirectiveInput {
            target_kind: "job".into(),
            target_id: "server-duty-cycle:edge-one".into(),
            target_revision: Some("attempt:1".into()),
            instruction: "Restore monitoring without changing the approved target".into(),
            desired_outcome: "Monitoring is current and verified".into(),
            constraints: vec!["Do not rotate the host key".into()],
            priority: "urgent".into(),
            due_at: None,
        },
    )
    .await
    .unwrap();
    for (state, summary) in [
        ("acknowledged", "Penny acknowledged the correction"),
        ("planned", "A bounded recovery plan is ready"),
        ("executing", "Recovery execution started"),
        (
            "verifying",
            "Recovery execution finished; verification started",
        ),
        ("resolved", "The corrected state is verified"),
    ] {
        directive = oversight::transition_directive(
            pool,
            tenant_a,
            directive.directive.id,
            admin_a,
            transition(directive.directive.revision, state, summary),
        )
        .await
        .unwrap();
    }
    assert_eq!(directive.directive.state, "resolved");
    assert_eq!(directive.oversight.record.outcome, "succeeded");
    assert_eq!(directive.oversight.record.verification_state, "verified");
    assert_eq!(directive.events.len(), 6);

    let setting = oversight::update_webhook_settings(
        pool,
        tenant_a,
        admin_a,
        WebhookSettingsInput {
            enabled: true,
            endpoint_url: Some("http://127.0.0.1:39001/manager".into()),
            destination_host: Some("127.0.0.1".into()),
            review_base_url: Some("http://127.0.0.1:18085".into()),
            expected_revision: 0,
        },
    )
    .await
    .unwrap();
    assert!(setting.enabled && setting.configured);
    assert_eq!(setting.destination_host.as_deref(), Some("127.0.0.1"));

    let failed = oversight::create_directive(
        pool,
        tenant_a,
        admin_a,
        CreateDirectiveInput {
            target_kind: "configuration".into(),
            target_id: "monitoring-profile:edge-one".into(),
            target_revision: None,
            instruction: "Correct the monitoring profile".into(),
            desired_outcome: "The profile applies cleanly".into(),
            constraints: Vec::new(),
            priority: "normal".into(),
            due_at: None,
        },
    )
    .await
    .unwrap();
    let failed = oversight::transition_directive(
        pool,
        tenant_a,
        failed.directive.id,
        admin_a,
        transition(
            failed.directive.revision,
            "escalated",
            "The correction stopped safely and needs a manager",
        ),
    )
    .await
    .unwrap();
    assert_eq!(failed.oversight.record.outcome, "safe_stopped");
    assert!(failed.oversight.exception.is_some());
    let deliveries = oversight::list_deliveries(pool, tenant_a, 10)
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].state, "pending");
    assert!(oversight::list_deliveries(pool, tenant_b, 10)
        .await
        .unwrap()
        .is_empty());

    harness.cleanup().await;
}
