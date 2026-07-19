use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    knowledge_events::{self as events, EnqueueKnowledgeEvent, FailureDisposition},
    knowledge_jobs::{
        self as jobs, BundleRoleSnapshot, EnqueueKnowledgeJob, JobBudget, KnowledgeJobKind,
        KnowledgeJobRole, RuntimeSnapshot,
    },
    knowledge_sources::{self as sources, MaterializeIncidentSnapshot},
    knowledge_spaces::{self as spaces, CreateProject, EnsureVault, SpaceActor},
    manager_oversight::{self as oversight, RecordOutcomeInput, StageEventInput},
};
use uuid::Uuid;

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
    else {
        return false;
    };
    let available: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
    )
    .fetch_optional(&pool)
    .await;
    pool.close().await;
    matches!(available, Ok(Some(_)))
}

fn event(
    tenant_id: Uuid,
    actor_id: Uuid,
    space_id: Uuid,
    subject_id: &str,
    revision: &str,
) -> EnqueueKnowledgeEvent {
    EnqueueKnowledgeEvent {
        tenant_id,
        descriptor_id: "incident-closed-knowledge".into(),
        event_kind: "server-incident-closed".into(),
        publisher_bundle_id: "server-administrator".into(),
        subject_kind: "server-incident".into(),
        subject_id: subject_id.into(),
        subject_revision: revision.into(),
        snapshot: serde_json::json!({
            "incident_id": subject_id,
            "revision": revision,
            "title": "Disk pressure incident",
            "timeline": [{"kind":"closed"}],
            "outcome_refs": []
        }),
        snapshot_hash: format!("sha256:{}", "a".repeat(64)),
        source_title: "Disk pressure incident".into(),
        source_path_prefix: "incidents".into(),
        acting_space_id: space_id,
        output_vault_bundle: "server-administrator".into(),
        knowledge_schema_id: "server.knowledge".into(),
        researcher_bundle_id: "server-operations-intelligence".into(),
        researcher_role_id: "server-incident-researcher".into(),
        requested_by_user_id: actor_id,
        service_actor_user_id: actor_id,
        effective_role: "manager".into(),
    }
}

fn research_job(
    space_id: Uuid,
    vault_id: Uuid,
    source_id: Uuid,
    event_id: Uuid,
) -> EnqueueKnowledgeJob {
    EnqueueKnowledgeJob {
        space_id,
        output_vault_id: vault_id,
        role: KnowledgeJobRole::Researcher,
        kind: KnowledgeJobKind::Event,
        priority: 10,
        input: serde_json::json!({
            "question": "Review the immutable incident snapshot",
            "originating_subject": {
                "owner_bundle": "server-administrator",
                "subject_kind": "server-administrator.server-incident",
                "subject_id": "incident-1",
                "subject_revision": "revision-1"
            }
        }),
        idempotency_key: format!("knowledge-event:{event_id}:server-incident-researcher"),
        source_ids: vec![source_id],
        runtime: RuntimeSnapshot {
            backend: "codex_exec".into(),
            model: "gpt-5.6-sol".into(),
            effort: "high".into(),
            endpoint_id: None,
            model_source: "default".into(),
            local_base_url: String::new(),
            local_api_key_env: String::new(),
            prompt_contract_revision: "server-incident-research-v1".into(),
            tool_policy_revision: "knowledge-read-v1".into(),
            role_profile_source: Some("bundle".into()),
            role_profile_ref: Some("b".repeat(64)),
        },
        bundle_role: Some(BundleRoleSnapshot {
            bundle_id: "server-operations-intelligence".into(),
            bundle_role_id: "server-incident-researcher".into(),
            package_manifest_sha256: "e".repeat(64),
            recipe_asset_id: "server-incident-research".into(),
            recipe_sha256: "f".repeat(64),
        }),
        budget: JobBudget {
            max_tokens: 4_096,
            max_sources: 4,
            max_wall_seconds: 60,
            max_attempts: 3,
        },
        scheduled_at: None,
    }
}

#[tokio::test]
async fn incident_snapshot_outbox_source_and_researcher_are_atomic_and_deduplicated() {
    if !pg_available().await {
        eprintln!("skipping CORE-T3 Knowledge event PG fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'CORE-T3 event pipeline')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,'CORE-T3 Admin','admin','test')"#,
    )
    .bind(actor_id)
    .bind(tenant_id)
    .bind(format!("core-t3-event-{tenant_id}@example.test"))
    .execute(pool)
    .await
    .unwrap();
    let actor = SpaceActor {
        tenant_id,
        user_id: actor_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "incident-learning".into(),
            title: "Incident learning".into(),
            goal: "Review closed incidents".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        pool,
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "server-administrator".into(),
            knowledge_schema_id: "server.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();

    let mut rollback = pool.begin().await.unwrap();
    events::enqueue_in_transaction(
        &mut rollback,
        event(
            tenant_id,
            actor_id,
            project.space.id,
            "incident-rollback",
            "revision-rollback",
        ),
    )
    .await
    .unwrap();
    rollback.rollback().await.unwrap();
    let rolled_back: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_event_outbox WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(rolled_back, 0);

    let mut transaction = pool.begin().await.unwrap();
    let event_id = events::enqueue_in_transaction(
        &mut transaction,
        event(
            tenant_id,
            actor_id,
            project.space.id,
            "incident-1",
            "revision-1",
        ),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let mut duplicate = pool.begin().await.unwrap();
    let duplicate_id = events::enqueue_in_transaction(
        &mut duplicate,
        event(
            tenant_id,
            actor_id,
            project.space.id,
            "incident-1",
            "revision-1",
        ),
    )
    .await
    .unwrap();
    duplicate.commit().await.unwrap();
    assert_eq!(duplicate_id, event_id);

    let leased = events::lease_next(pool, "worker-a", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.id, event_id);
    assert_eq!(leased.attempt_count, 1);
    assert_eq!(
        events::fail(pool, event_id, "worker-a", "temporary failure")
            .await
            .unwrap(),
        FailureDisposition::Retry
    );

    let bytes = br#"{"incident_id":"incident-1","revision":"revision-1"}"#;
    let blob_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO knowledge_blobs
           (id, tenant_id, content_hash, storage_key, byte_size, content_type,
            original_name, created_by)
           VALUES ($1,$2,$3,$4,$5,'application/json','incident-1.json',$6)"#,
    )
    .bind(blob_id)
    .bind(tenant_id)
    .bind(format!("sha256:{}", "c".repeat(64)))
    .bind(format!("sha256/cc/{}", "c".repeat(64)))
    .bind(bytes.len() as i64)
    .bind(actor_id)
    .execute(pool)
    .await
    .unwrap();
    let source = sources::materialize_incident_snapshot(
        pool,
        actor,
        MaterializeIncidentSnapshot {
            source_id: event_id,
            object_id: Uuid::new_v4(),
            vault_id: vault.id,
            title: "Disk pressure incident".into(),
            original_name: "disk-pressure--incident-1.md".into(),
            final_uri: "gadgetron://server-administrator/server-incident/incident-1@revision-1"
                .into(),
            content_type: "application/json".into(),
            byte_size: bytes.len() as i64,
            content_hash: format!("sha256:{}", "c".repeat(64)),
            blob_id,
            path: "incidents/disk-pressure--incident-1.md".into(),
            note_content_hash: "d".repeat(64),
        },
    )
    .await
    .unwrap();
    assert_eq!(source.id, event_id);
    assert_eq!(source.source_kind, "incident_snapshot");
    assert_eq!(source.status, "extracted");
    let ledger_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_sources WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(ledger_rows, 1);

    let first_job = jobs::enqueue(
        pool,
        actor,
        research_job(project.space.id, vault.id, source.id, event_id),
    )
    .await
    .unwrap();
    let duplicate_job = jobs::enqueue(
        pool,
        actor,
        research_job(project.space.id, vault.id, source.id, event_id),
    )
    .await
    .unwrap();
    assert_eq!(duplicate_job.id, first_job.id);
    assert_eq!(first_job.kind, "event");
    let job_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_jobs WHERE tenant_id = $1 AND idempotency_key = $2",
    )
    .bind(tenant_id)
    .bind(&first_job.idempotency_key)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(job_count, 1);

    sqlx::query("UPDATE knowledge_event_outbox SET next_attempt_at = NOW() WHERE id = $1")
        .bind(event_id)
        .execute(pool)
        .await
        .unwrap();
    let resumed = events::lease_next(pool, "worker-b", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.id, event_id);
    let completed = events::complete(pool, event_id, "worker-b", source.id, first_job.id)
        .await
        .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.attempt_count, 2);

    let mut failing_transaction = pool.begin().await.unwrap();
    let failing_event_id = events::enqueue_in_transaction(
        &mut failing_transaction,
        event(
            tenant_id,
            actor_id,
            project.space.id,
            "incident-failed",
            "revision-failed",
        ),
    )
    .await
    .unwrap();
    failing_transaction.commit().await.unwrap();
    for attempt in 1..=3 {
        let worker = format!("worker-fail-{attempt}");
        let leased = events::lease_next(pool, &worker, 30)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leased.id, failing_event_id);
        assert_eq!(leased.attempt_count, attempt);
        let disposition = events::fail(pool, failing_event_id, &worker, "permanent failure")
            .await
            .unwrap();
        assert_eq!(
            disposition,
            if attempt == 3 {
                FailureDisposition::Terminal
            } else {
                FailureDisposition::Retry
            }
        );
        if attempt < 3 {
            sqlx::query("UPDATE knowledge_event_outbox SET next_attempt_at = NOW() WHERE id = $1")
                .bind(failing_event_id)
                .execute(pool)
                .await
                .unwrap();
        }
    }
    let failed_event = events::get(pool, tenant_id, failing_event_id)
        .await
        .unwrap();
    assert_eq!(failed_event.status, "failed");
    assert_eq!(failed_event.attempt_count, 3);
    assert_eq!(
        failed_event.last_error.as_deref(),
        Some("permanent failure")
    );
    let oversight = oversight::record_outcome(
        pool,
        RecordOutcomeInput {
            tenant_id,
            source_kind: "knowledge_event".into(),
            source_id: failing_event_id.to_string(),
            actor_user_id: Some(actor_id),
            agent_label: "Knowledge event bridge".into(),
            agent_role: "researcher".into(),
            goal: "Materialize immutable incident evidence".into(),
            target_kind: "knowledge_revision".into(),
            target_id: "incident-failed".into(),
            target_revision: Some("revision-failed".into()),
            policy_decision: "auto".into(),
            policy_revision: None,
            evidence_refs: vec![format!("knowledge-event:{failing_event_id}")],
            current_stage: "execute".into(),
            outcome: "failed".into(),
            verification_state: "failed".into(),
            action_summary: "permanent failure".into(),
            before_summary: None,
            after_summary: None,
            rollback_summary: Some("No partial Source row was committed".into()),
            duration_ms: 0,
            cost_minor_units: 0,
            events: vec![StageEventInput {
                stage: "execute".into(),
                state: "failed".into(),
                summary: "permanent failure".into(),
                evidence_refs: vec![format!("knowledge-event:{failing_event_id}")],
            }],
            exception_severity: Some("error".into()),
            exception_summary: Some("permanent failure".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(oversight.source_kind, "knowledge_event");
    assert_eq!(oversight.outcome, "failed");
    let unreported = events::next_unreported_failure(pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unreported.id, failing_event_id);
    events::mark_oversight_recorded(pool, failing_event_id)
        .await
        .unwrap();
    assert!(events::next_unreported_failure(pool)
        .await
        .unwrap()
        .is_none());

    harness.cleanup().await;
}
