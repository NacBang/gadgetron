use std::time::{Duration, Instant};

use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    autonomy::{
        self, AutonomyError, BundleEventProjectionQuery, BundleEventProjectionSubject,
        EnqueueBundleEvent, EventRunTerminal, RunDisposition, RunFinish, SyncBundleSchedule,
    },
    knowledge_spaces::{self as spaces, SpaceActor},
    teams,
};
use uuid::Uuid;

fn current_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .expect("/proc/self/status must be readable on the supported Linux host")
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
        .expect("VmRSS must be present in /proc/self/status")
}

#[tokio::test]
async fn core_t1_bundle_event_dedup_pins_profile_and_records_terminal_receipts() {
    if !pg_available().await {
        eprintln!("skipping Bundle event PostgreSQL test: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_id = Uuid::new_v4();
    let manager_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'bundle-event-fixture')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,'event-manager@example.test','Event manager','admin','test')",
    )
    .bind(manager_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    teams::create_team(
        pool,
        tenant_id,
        "event-ops",
        "Event Operations",
        None,
        Some(manager_id),
    )
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: manager_id,
    };
    let space = spaces::ensure_team_space(pool, actor, "event-ops", "Event Operations")
        .await
        .unwrap();

    let input = bundle_event(tenant_id, manager_id, space.id, "1".repeat(64), 2);
    let mut tx = pool.begin().await.unwrap();
    let first = autonomy::enqueue_bundle_event_in_transaction(&mut tx, input.clone())
        .await
        .unwrap();
    let duplicate = autonomy::enqueue_bundle_event_in_transaction(&mut tx, input)
        .await
        .unwrap();
    assert!(first.created);
    assert!(!duplicate.created);
    assert_eq!(duplicate.goal.id, first.goal.id);
    tx.commit().await.unwrap();
    let goal_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomy_goals WHERE tenant_id = $1 AND source_kind = 'bundle_event'",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(goal_count, 1);

    let lease = autonomy::lease_next(pool, "event-worker-1", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.run.agent_profile_snapshot,
        lease.goal.agent_profile_snapshot
    );
    assert_eq!(lease.run.agent_profile_snapshot["model"], "fast-model");
    let retry = autonomy::finish_event_run(
        pool,
        &lease,
        "event-worker-1",
        EventRunTerminal::ProviderFailure,
        "provider unavailable without changing the subject",
        None,
    )
    .await
    .unwrap();
    assert_eq!(retry.status, "retry_wait");
    sqlx::query("UPDATE autonomy_goals SET next_run_at = NOW() WHERE id = $1")
        .bind(retry.id)
        .execute(pool)
        .await
        .unwrap();
    let retry_lease = autonomy::lease_next(pool, "event-worker-2", 30)
        .await
        .unwrap()
        .unwrap();
    let failed = autonomy::finish_event_run(
        pool,
        &retry_lease,
        "event-worker-2",
        EventRunTerminal::ProviderFailure,
        "provider retry budget exhausted without changing the subject",
        None,
    )
    .await
    .unwrap();
    assert_eq!(failed.status, "failed_provider");
    let failed_receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM autonomy_event_receipts WHERE job_id = $1")
            .bind(failed.id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(failed_receipts, 0);

    let success_input = bundle_event(tenant_id, manager_id, space.id, "2".repeat(64), 1);
    let mut tx = pool.begin().await.unwrap();
    let success = autonomy::enqueue_bundle_event_in_transaction(&mut tx, success_input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let success_lease = autonomy::lease_next(pool, "event-worker-3", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(success_lease.goal.id, success.goal.id);
    let result_hash = "a".repeat(64);
    let completed = autonomy::finish_event_run(
        pool,
        &success_lease,
        "event-worker-3",
        EventRunTerminal::Succeeded,
        "canonical result attached",
        Some(&result_hash),
    )
    .await
    .unwrap();
    assert_eq!(completed.status, "succeeded");
    let receipt: (String, String) = sqlx::query_as(
        "SELECT subject_revision, result_hash FROM autonomy_event_receipts WHERE job_id = $1",
    )
    .bind(completed.id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(receipt, ("2".repeat(64), result_hash));

    harness.cleanup().await;
}

#[tokio::test]
async fn core_t2_projection_states_pin_tenant_package_and_subject_revision() {
    if !pg_available().await {
        eprintln!("skipping row-enrichment PostgreSQL test: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let actor_a = Uuid::new_v4();
    let actor_b = Uuid::new_v4();
    for (tenant_id, actor_id, suffix) in [(tenant_a, actor_a, "a"), (tenant_b, actor_b, "b")] {
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
            .bind(tenant_id)
            .bind(format!("row-enrichment-{suffix}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,$3,$3,'admin','test')",
        )
        .bind(actor_id)
        .bind(tenant_id)
        .bind(format!("row-enrichment-{suffix}@example.test"))
        .execute(pool)
        .await
        .unwrap();
        teams::create_team(
            pool,
            tenant_id,
            &format!("row-enrichment-{suffix}"),
            "Row Enrichment",
            None,
            Some(actor_id),
        )
        .await
        .unwrap();
    }
    let space_a = spaces::ensure_team_space(
        pool,
        SpaceActor {
            tenant_id: tenant_a,
            user_id: actor_a,
        },
        "row-enrichment-a",
        "Row Enrichment",
    )
    .await
    .unwrap();
    let space_b = spaces::ensure_team_space(
        pool,
        SpaceActor {
            tenant_id: tenant_b,
            user_id: actor_b,
        },
        "row-enrichment-b",
        "Row Enrichment",
    )
    .await
    .unwrap();
    let subject_id = "incident-one";
    let revision_a = "1".repeat(64);
    let stale_revision = "2".repeat(64);
    let mut event_a = bundle_event(tenant_a, actor_a, space_a.id, revision_a.clone(), 1);
    event_a.subject_id = subject_id.into();
    let mut stale_event = event_a.clone();
    stale_event.subject_revision = stale_revision.clone();
    let mut event_b = bundle_event(tenant_b, actor_b, space_b.id, revision_a.clone(), 1);
    event_b.subject_id = subject_id.into();

    let mut tx = pool.begin().await.unwrap();
    let goal_a = autonomy::enqueue_bundle_event_in_transaction(&mut tx, event_a)
        .await
        .unwrap()
        .goal;
    let stale_goal = autonomy::enqueue_bundle_event_in_transaction(&mut tx, stale_event)
        .await
        .unwrap()
        .goal;
    let goal_b = autonomy::enqueue_bundle_event_in_transaction(&mut tx, event_b)
        .await
        .unwrap()
        .goal;
    tx.commit().await.unwrap();
    sqlx::query("UPDATE autonomy_goals SET status = 'succeeded' WHERE id = $1")
        .bind(goal_a.id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE autonomy_goals SET status = 'failed_provider' WHERE id = $1")
        .bind(stale_goal.id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(goal_b.status, "ready");

    let requested = [BundleEventProjectionSubject {
        id: subject_id.into(),
        revision: revision_a.clone(),
    }];
    let package_b = "b".repeat(64);
    let tenant_a_states = autonomy::bundle_event_projection_states(
        pool,
        BundleEventProjectionQuery {
            tenant_id: tenant_a,
            owner_bundle_id: "server-operations-intelligence",
            package_manifest_sha256: &package_b,
            subject_bundle_id: "server-administrator",
            subject_kind: "log-finding",
            event_kind: "server-log-finding-created",
            agent_role_id: "server-log-finding-enricher",
            subjects: &requested,
        },
    )
    .await
    .unwrap();
    assert_eq!(tenant_a_states.len(), 2);
    assert!(tenant_a_states
        .iter()
        .any(|state| { state.revision == revision_a && state.status == "succeeded" }));
    assert!(tenant_a_states
        .iter()
        .any(|state| { state.revision == stale_revision && state.status == "failed_provider" }));

    let tenant_b_states = autonomy::bundle_event_projection_states(
        pool,
        BundleEventProjectionQuery {
            tenant_id: tenant_b,
            owner_bundle_id: "server-operations-intelligence",
            package_manifest_sha256: &package_b,
            subject_bundle_id: "server-administrator",
            subject_kind: "log-finding",
            event_kind: "server-log-finding-created",
            agent_role_id: "server-log-finding-enricher",
            subjects: &requested,
        },
    )
    .await
    .unwrap();
    assert_eq!(tenant_b_states.len(), 1);
    assert_eq!(tenant_b_states[0].status, "ready");

    let package_c = "c".repeat(64);
    let wrong_package = autonomy::bundle_event_projection_states(
        pool,
        BundleEventProjectionQuery {
            tenant_id: tenant_a,
            owner_bundle_id: "server-operations-intelligence",
            package_manifest_sha256: &package_c,
            subject_bundle_id: "server-administrator",
            subject_kind: "log-finding",
            event_kind: "server-log-finding-created",
            agent_role_id: "server-log-finding-enricher",
            subjects: &requested,
        },
    )
    .await
    .unwrap();
    assert!(wrong_package.is_empty());

    harness.cleanup().await;
}

fn open_fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("/proc/self/fd must be readable on the supported Linux host")
        .count()
}

async fn database_connection_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM pg_stat_activity WHERE usename = current_user")
        .fetch_one(pool)
        .await
        .expect("PostgreSQL connection count must be readable")
}

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .is_ok()
}

#[tokio::test]
async fn r3_6_durable_team_goal_recovers_without_duplicate_or_role_escalation() {
    if !pg_available().await {
        eprintln!("skipping autonomy PostgreSQL test: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    let operator_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'autonomy-fixture')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    for (id, email, role) in [
        (admin_id, "manager@example.test", "admin"),
        (operator_id, "operator@example.test", "member"),
    ] {
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,$3,$3,$4,'test')",
        )
        .bind(id)
        .bind(tenant_id)
        .bind(email)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    }
    teams::create_team(
        pool,
        tenant_id,
        "platform-ops",
        "Platform Operations",
        None,
        Some(admin_id),
    )
    .await
    .unwrap();
    teams::add_team_member(
        pool,
        tenant_id,
        "platform-ops",
        operator_id,
        "member",
        Some(admin_id),
    )
    .await
    .unwrap();
    let admin = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let team_space = spaces::ensure_team_space(pool, admin, "platform-ops", "Platform Operations")
        .await
        .unwrap();

    let unbound = autonomy::sync_bundle_schedule(pool, schedule(tenant_id, None, None, 2))
        .await
        .unwrap();
    assert_eq!(unbound.status, "context_required");
    assert!(autonomy::lease_next(pool, "worker-a", 30)
        .await
        .unwrap()
        .is_none());

    let ready = autonomy::sync_bundle_schedule(
        pool,
        schedule(tenant_id, Some(team_space.id), Some(admin_id), 2),
    )
    .await
    .unwrap();
    assert_eq!(ready.status, "ready");
    assert_eq!(ready.context_state, "ready");
    let service_actor = ready.service_actor_user_id.unwrap();

    let delegated = autonomy::sync_bundle_schedule(
        pool,
        schedule(tenant_id, Some(team_space.id), Some(operator_id), 2),
    )
    .await
    .unwrap();
    assert_eq!(delegated.service_actor_user_id, Some(service_actor));
    assert_eq!(delegated.effective_role.as_deref(), Some("contributor"));
    let unchanged = autonomy::sync_bundle_schedule(
        pool,
        schedule(tenant_id, Some(team_space.id), Some(operator_id), 2),
    )
    .await
    .unwrap();
    assert_eq!(unchanged.revision, delegated.revision);
    assert_eq!(unchanged.updated_at, delegated.updated_at);

    let lease = autonomy::lease_next(pool, "worker-a", 30)
        .await
        .unwrap()
        .unwrap();
    assert!(autonomy::lease_next(pool, "worker-b", 30)
        .await
        .unwrap()
        .is_none());
    autonomy::validate_lease_context(pool, &lease)
        .await
        .unwrap();
    let service_principal = service_actor.to_string();
    let service_grant: (Uuid, i64) = sqlx::query_as(
        "SELECT id, revision FROM knowledge_space_grants WHERE tenant_id = $1 AND space_id = $2 AND principal_kind = 'user' AND principal_id = $3 AND revoked_at IS NULL",
    )
    .bind(tenant_id)
    .bind(team_space.id)
    .bind(&service_principal)
    .fetch_one(pool)
    .await
    .unwrap();
    spaces::revoke_grant(pool, admin, team_space.id, service_grant.0, service_grant.1)
        .await
        .unwrap();
    assert!(matches!(
        autonomy::validate_lease_context(pool, &lease).await,
        Err(AutonomyError::ContextForbidden)
    ));
    let grant_required = autonomy::sync_bundle_schedule(
        pool,
        schedule(tenant_id, Some(team_space.id), Some(operator_id), 2),
    )
    .await
    .unwrap();
    assert_eq!(grant_required.context_state, "service_grant_required");
    assert_eq!(grant_required.acting_space_id, Some(team_space.id));
    assert_eq!(grant_required.service_actor_user_id, Some(service_actor));
    spaces::upsert_grant(
        pool,
        admin,
        team_space.id,
        spaces::PrincipalKind::User,
        &service_principal,
        spaces::SpaceRole::Contributor,
        None,
    )
    .await
    .unwrap();
    autonomy::sync_bundle_schedule(
        pool,
        schedule(tenant_id, Some(team_space.id), Some(operator_id), 2),
    )
    .await
    .unwrap();
    autonomy::validate_lease_context(pool, &lease)
        .await
        .unwrap();
    autonomy::pin_run_context(
        pool,
        &lease,
        "worker-a",
        Some("policy:1"),
        serde_json::json!({
            "state":"cited",
            "query_id":"query-1",
            "context_revision":"context-1",
            "citations":[{"citation_id":"citation-1","source_revision":"source-1"}],
        }),
    )
    .await
    .unwrap();
    let retry = autonomy::finish_run(
        pool,
        &lease,
        "worker-a",
        RunFinish {
            outcome: "failed".into(),
            verification_state: "failed".into(),
            verification_summary: "Transient Bundle failure".into(),
            evidence_refs: vec!["bundle-job:first".into()],
            disposition: RunDisposition::RetryableFailure,
        },
    )
    .await
    .unwrap();
    assert_eq!(retry.status, "retry_wait");
    sqlx::query("UPDATE autonomy_goals SET next_run_at = NOW() WHERE id = $1")
        .bind(retry.id)
        .execute(pool)
        .await
        .unwrap();

    let restarted = autonomy::lease_next(pool, "worker-after-restart", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restarted.run.attempt, 2);
    sqlx::query(
        "UPDATE autonomy_goals SET lease_expires_at = NOW() - interval '1 second' WHERE id = $1",
    )
    .bind(restarted.goal.id)
    .execute(pool)
    .await
    .unwrap();
    let recovered = autonomy::recover_expired_leases(pool, 10).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "safe_stopped");
    assert!(autonomy::lease_next(pool, "worker-c", 30)
        .await
        .unwrap()
        .is_none());

    let resumed = autonomy::resume_goal(pool, admin, recovered[0].id, recovered[0].revision)
        .await
        .unwrap();
    assert_eq!(resumed.status, "ready");
    let continued = autonomy::lease_next(pool, "worker-c", 30)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE autonomy_goals SET target_revision = 'changed' WHERE id = $1")
        .bind(continued.goal.id)
        .execute(pool)
        .await
        .unwrap();
    assert!(matches!(
        autonomy::validate_lease_context(pool, &continued).await,
        Err(AutonomyError::ExecutionSnapshotChanged)
    ));
    sqlx::query("UPDATE autonomy_goals SET target_revision = $2 WHERE id = $1")
        .bind(continued.goal.id)
        .bind(&continued.run.target_revision)
        .execute(pool)
        .await
        .unwrap();
    autonomy::validate_lease_context(pool, &continued)
        .await
        .unwrap();
    let soak_duration = std::env::var("GADGETRON_AUTONOMY_SOAK_SECONDS")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .expect("GADGETRON_AUTONOMY_SOAK_SECONDS must be an integer")
        })
        .map(Duration::from_secs);
    let soak_interval = std::env::var("GADGETRON_AUTONOMY_SOAK_INTERVAL_SECONDS")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .expect("GADGETRON_AUTONOMY_SOAK_INTERVAL_SECONDS must be an integer")
        })
        .unwrap_or(60);
    assert!((1..=60).contains(&soak_interval));
    let soak_started = Instant::now();
    let baseline_rss_kib = current_rss_kib();
    let baseline_fd_count = open_fd_count();
    let baseline_pool_connections = pool.size();
    let mut soak_lease = continued;
    let mut completed_cycles = 0_i64;
    loop {
        completed_cycles += 1;
        let completed = autonomy::finish_run(
            pool,
            &soak_lease,
            "worker-c",
            RunFinish {
                outcome: "succeeded".into(),
                verification_state: "verified".into(),
                verification_summary: format!("Soak cycle {completed_cycles} verified"),
                evidence_refs: vec![format!("soak:cycle:{completed_cycles}")],
                disposition: RunDisposition::Succeeded,
            },
        )
        .await
        .unwrap();
        assert_eq!(completed.status, "ready");
        sqlx::query("UPDATE autonomy_goals SET next_run_at = NOW() WHERE id = $1")
            .bind(completed.id)
            .execute(pool)
            .await
            .unwrap();
        let continue_soak = match soak_duration {
            Some(duration) => {
                let elapsed = soak_started.elapsed();
                if elapsed < duration {
                    tokio::time::sleep(Duration::from_secs(soak_interval).min(duration - elapsed))
                        .await;
                    true
                } else {
                    false
                }
            }
            None => completed_cycles < 5,
        };
        soak_lease = autonomy::lease_next(pool, "worker-c", 30)
            .await
            .unwrap()
            .unwrap();
        assert!(autonomy::lease_next(pool, "worker-d", 30)
            .await
            .unwrap()
            .is_none());
        if !continue_soak {
            break;
        }
    }
    let run_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomy_goal_runs WHERE tenant_id = $1 AND goal_id = $2",
    )
    .bind(tenant_id)
    .bind(soak_lease.goal.id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(run_count, completed_cycles + 3);
    let active_lease_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomy_goals WHERE tenant_id = $1 AND id = $2 AND lease_owner IS NOT NULL",
    )
    .bind(tenant_id)
    .bind(soak_lease.goal.id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(active_lease_count, 1);
    let final_rss_kib = current_rss_kib();
    let final_fd_count = open_fd_count();
    let final_pool_connections = pool.size();
    let observed_database_connections = database_connection_count(pool).await;
    assert!(final_rss_kib <= baseline_rss_kib + 128 * 1024);
    assert!(final_fd_count <= baseline_fd_count + 16);
    assert!(final_pool_connections <= baseline_pool_connections + 4);
    eprintln!(
        "autonomy soak completed: cycles={completed_cycles} elapsed_seconds={} rss_growth_kib={} fd_growth={} pool_connections={final_pool_connections} cluster_user_connections={observed_database_connections}",
        soak_started.elapsed().as_secs(),
        final_rss_kib.saturating_sub(baseline_rss_kib),
        final_fd_count.saturating_sub(baseline_fd_count),
    );
    teams::remove_team_member(pool, tenant_id, "platform-ops", operator_id)
        .await
        .unwrap();
    assert!(matches!(
        autonomy::validate_lease_context(pool, &soak_lease).await,
        Err(AutonomyError::ContextForbidden)
    ));

    harness.cleanup().await;
}

fn schedule(
    tenant_id: Uuid,
    acting_space_id: Option<Uuid>,
    requested_by_user_id: Option<Uuid>,
    max_attempts: i32,
) -> SyncBundleSchedule {
    SyncBundleSchedule {
        goal_key: format!(
            "bundle-schedule:server-administrator:server-duty-cycle:{tenant_id}:edge-one"
        ),
        goal: "Keep edge one observable and recover monitoring safely".into(),
        tenant_id,
        owner_bundle_id: "server-administrator".into(),
        recipe_id: "server-duty-cycle".into(),
        package_manifest_sha256: "a".repeat(64),
        target_kind: "ssh".into(),
        target_id: "edge-one".into(),
        target_revision: "11111111-2222-4333-8444-555555555555".into(),
        target_label: "Edge one".into(),
        acting_space_id,
        requested_by_user_id,
        interval: Duration::from_secs(300),
        max_wall_time: Duration::from_secs(100),
        max_attempts,
    }
}

fn bundle_event(
    tenant_id: Uuid,
    actor_id: Uuid,
    acting_space_id: Uuid,
    subject_revision: String,
    max_attempts: i32,
) -> EnqueueBundleEvent {
    EnqueueBundleEvent {
        tenant_id,
        event_kind: "server-log-finding-created".into(),
        subject_bundle_id: "server-administrator".into(),
        subject_kind: "log-finding".into(),
        subject_id: Uuid::new_v4().to_string(),
        subject_revision,
        event_payload: serde_json::json!({"subject":{"summary":"rule evidence"}}),
        owner_bundle_id: "server-operations-intelligence".into(),
        recipe_id: "server-log-finding-enrichment".into(),
        package_manifest_sha256: "b".repeat(64),
        agent_role_id: "server-log-finding-enricher".into(),
        result_gadget: "serverintelligence.finding-enrich-attach".into(),
        goal: "Add bounded context without changing the finding".into(),
        acting_space_id,
        requested_by_user_id: actor_id,
        service_actor_user_id: actor_id,
        effective_role: "manager".into(),
        max_wall_seconds: 120,
        max_attempts,
        agent_profile_snapshot: serde_json::json!({
            "backend":"codex_exec",
            "model":"fast-model",
            "effort":"low",
            "endpoint_id":null,
            "model_source":"default",
            "local_base_url":"",
            "local_api_key_env":"",
            "prompt_contract_revision":"server-log-finding-enrichment-v1",
            "tool_policy_revision":"policy:1",
            "role_profile_source":"global",
            "role_profile_ref":null
        }),
    }
}
