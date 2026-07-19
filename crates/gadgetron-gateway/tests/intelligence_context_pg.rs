use std::{sync::Arc, time::Duration};

use gadgetron_bundle_sdk::{
    BundleId, CapabilityId, CitationRole, CitationUseRef, ContextCoverage, ContextUseRef,
    IntelligenceBudget, IntelligenceQueryDraft, OutcomeFeedbackDraft, OutcomePredicateResult,
    SubjectRevisionRef,
};
use gadgetron_gateway::web::intelligence_context::{
    IntelligenceActorBinding, IntelligenceContextService,
};
use gadgetron_knowledge::vault::TenantVaultLayout;
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::autonomy::{self, RunDisposition, RunFinish, SyncBundleSchedule};
use gadgetron_xaas::knowledge_spaces::{
    self as spaces, CreateShare, EnsureVault, RegisterObject, ShareMode, SpaceActor,
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

async fn tenant_admin_and_teams(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let tenant_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'J11 context test')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,'J11 Admin','admin','test')"#,
    )
    .bind(admin_id)
    .bind(tenant_id)
    .bind(format!("j11-{tenant_id}@example.test"))
    .execute(pool)
    .await
    .unwrap();
    for team_id in ["j11-team-a", "j11-team-b"] {
        sqlx::query("INSERT INTO teams (id, tenant_id, display_name) VALUES ($1,$2,$1)")
            .bind(team_id)
            .bind(tenant_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1,$2,'lead')")
            .bind(team_id)
            .bind(admin_id)
            .execute(pool)
            .await
            .unwrap();
    }
    (tenant_id, admin_id)
}

fn query(query_id: &str) -> IntelligenceQueryDraft {
    IntelligenceQueryDraft::new(
        query_id,
        SubjectRevisionRef::new(
            BundleId::new("server-administrator").unwrap(),
            CapabilityId::new("server.target").unwrap(),
            "edge-server-1",
            "1",
        )
        .unwrap(),
        "How should the jupiter-lattice recovery be performed?",
        86_400,
        IntelligenceBudget::new(8, 100, 65_536, 8_000, 10).unwrap(),
    )
    .unwrap()
}

async fn note(
    pool: &sqlx::PgPool,
    layout: &TenantVaultLayout,
    actor: SpaceActor,
    space_id: Uuid,
    vault_id: Uuid,
    path: &str,
    body: &str,
) -> spaces::KnowledgeObjectRow {
    let repository = layout.open_or_init(actor.tenant_id).unwrap();
    repository.ensure_domain(space_id, "core").unwrap();
    let state = repository
        .write_note(
            space_id,
            "core",
            path,
            body.as_bytes(),
            "test: add Team knowledge",
        )
        .unwrap();
    spaces::register_object(
        pool,
        actor,
        vault_id,
        RegisterObject {
            canonical_kind: "lesson".to_string(),
            path: path.to_string(),
            content_hash: Some(state.content_hash),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn j11_acting_team_retrieval_requires_a_live_exact_reference() {
    if !pg_available().await {
        eprintln!("skipping J11 context fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let temp = tempfile::tempdir().unwrap();
    let layout = Arc::new(TenantVaultLayout::new(temp.path()));
    let (tenant_id, admin_id) = tenant_admin_and_teams(pool).await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let team_a = spaces::ensure_team_space(pool, actor, "j11-team-a", "Team A")
        .await
        .unwrap();
    let team_b = spaces::ensure_team_space(pool, actor, "j11-team-b", "Team B")
        .await
        .unwrap();
    let vault_a = spaces::ensure_vault(
        pool,
        actor,
        team_a.id,
        EnsureVault {
            home_bundle_id: "core".to_string(),
            knowledge_schema_id: "core.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let shared_path = format!("notes/{}.md", Uuid::new_v4());
    let shared = note(
        pool,
        &layout,
        actor,
        team_a.id,
        vault_a.id,
        &shared_path,
        "# Jupiter lattice recovery\n\nRun the bounded recovery checklist before service restart.\n",
    )
    .await;
    let unshared_path = format!("notes/{}.md", Uuid::new_v4());
    let unshared = note(
        pool,
        &layout,
        actor,
        team_a.id,
        vault_a.id,
        &unshared_path,
        "# Jupiter lattice private appendix\n\nThis object was not shared with Team B.\n",
    )
    .await;
    let binding = IntelligenceActorBinding {
        tenant_id,
        actor_id: admin_id,
        authority_actor_id: None,
        acting_space_id: Some(team_b.id),
    };
    let service = IntelligenceContextService::new(pool.clone(), layout);
    let consumer = BundleId::new("server-administrator").unwrap();

    let before = service
        .resolve(binding, &consumer, query("j11-before"))
        .await
        .unwrap();
    assert_eq!(before.coverage, ContextCoverage::Unavailable);
    assert!(before.citations.is_empty());
    assert_eq!(
        before.authority.allowed_space_ids,
        vec![team_b.id.to_string()]
    );

    let share = spaces::share_object(
        pool,
        actor,
        shared.id,
        CreateShare {
            target_space_id: team_b.id,
            source_revision: shared.revision,
            mode: ShareMode::Reference,
            follow_latest: true,
            policy_disposition: "allowed".to_string(),
        },
    )
    .await
    .unwrap();
    let shared_pack = service
        .resolve(binding, &consumer, query("j11-shared"))
        .await
        .unwrap();
    assert_eq!(shared_pack.citations.len(), 1);
    assert_eq!(shared_pack.citations[0].space_id, team_a.id.to_string());
    assert_eq!(
        shared_pack.citations[0].citation_id,
        format!("{}:{}", shared.id, shared.revision)
    );
    assert!(!shared_pack.citations[0]
        .citation_id
        .starts_with(&unshared.id.to_string()));
    assert_eq!(
        shared_pack.authority.allowed_space_ids,
        [team_a.id.to_string(), team_b.id.to_string()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );

    let goal = autonomy::sync_bundle_schedule(
        pool,
        SyncBundleSchedule {
            goal_key: format!("j11-shared-context:{tenant_id}"),
            goal: "Use reviewed Team knowledge in the server duty cycle".into(),
            tenant_id,
            owner_bundle_id: "server-administrator".into(),
            recipe_id: "server-duty-cycle".into(),
            package_manifest_sha256: "a".repeat(64),
            target_kind: "ssh".into(),
            target_id: "edge-server-1".into(),
            target_revision: "1".into(),
            target_label: "Edge server 1".into(),
            acting_space_id: Some(team_b.id),
            requested_by_user_id: Some(admin_id),
            interval: Duration::from_secs(300),
            max_wall_time: Duration::from_secs(60),
            max_attempts: 2,
        },
    )
    .await
    .unwrap();
    let service_actor_id = goal.service_actor_user_id.unwrap();
    let service_spaces = spaces::effective_spaces(
        pool,
        SpaceActor {
            tenant_id,
            user_id: service_actor_id,
        },
    )
    .await
    .unwrap();
    assert!(service_spaces.iter().any(|item| item.space.id == team_b.id));
    assert!(!service_spaces.iter().any(|item| item.space.id == team_a.id));
    let delegated = IntelligenceActorBinding {
        tenant_id,
        actor_id: service_actor_id,
        authority_actor_id: Some(admin_id),
        acting_space_id: Some(team_b.id),
    };
    let scheduled_pack = service
        .resolve(delegated, &consumer, query("j11-scheduled-shared"))
        .await
        .unwrap();
    assert_eq!(
        scheduled_pack.authority.actor_id,
        service_actor_id.to_string()
    );
    assert_eq!(scheduled_pack.citations.len(), 1);
    assert_eq!(
        scheduled_pack.citations[0].citation_id,
        format!("{}:{}", shared.id, shared.revision)
    );
    let used_source = scheduled_pack.citations[0].clone();
    let receipt = service
        .record_feedback(
            delegated,
            &consumer,
            OutcomeFeedbackDraft::new(
                "j11-satisfied-recovery",
                scheduled_pack.subject.clone(),
                "j11-operation-succeeded",
                Some(ContextUseRef::new(
                    scheduled_pack.query_id.clone(),
                    scheduled_pack.context_revision.clone(),
                )),
                serde_json::json!({"service":"unavailable"}),
                serde_json::json!({"service":"healthy"}),
                OutcomePredicateResult::Satisfied,
                "Jupiter lattice recovery was verified",
                vec![CitationUseRef::new(
                    used_source.citation_id.clone(),
                    used_source.source_revision.clone(),
                )],
            ),
        )
        .await
        .unwrap();
    service
        .record_feedback(
            delegated,
            &consumer,
            OutcomeFeedbackDraft::new(
                "j11-failed-recovery",
                scheduled_pack.subject.clone(),
                "j11-operation-failed",
                Some(ContextUseRef::new(
                    scheduled_pack.query_id.clone(),
                    scheduled_pack.context_revision.clone(),
                )),
                serde_json::json!({"service":"unavailable"}),
                serde_json::json!({"service":"unavailable"}),
                OutcomePredicateResult::Failed,
                "Jupiter lattice recovery did not verify",
                vec![CitationUseRef::new(
                    used_source.citation_id.clone(),
                    used_source.source_revision.clone(),
                )],
            ),
        )
        .await
        .unwrap();

    let scheduled_experience = service
        .resolve(delegated, &consumer, query("j11-scheduled-experience"))
        .await
        .unwrap();
    assert_eq!(scheduled_experience.citations.len(), 2);
    assert_eq!(scheduled_experience.citations[0], used_source);
    let experience = &scheduled_experience.citations[1];
    assert!(experience.citation_id.starts_with("experience:"));
    assert_eq!(experience.space_id, team_a.id.to_string());
    assert_eq!(experience.owner_bundle, consumer);
    assert_eq!(experience.source_id, "j11-satisfied-recovery");
    assert_eq!(experience.source_revision, receipt.experience_revision);
    assert_eq!(experience.role, CitationRole::Context);
    assert!(experience
        .passage
        .contains("Jupiter lattice recovery was verified"));
    assert!(!experience.passage.contains("unavailable"));
    assert!(!experience.passage.contains("healthy"));
    assert_eq!(
        experience.content_sha256.as_deref(),
        receipt.experience_revision.strip_prefix("sha256:")
    );
    assert!(!scheduled_experience
        .citations
        .iter()
        .any(|citation| citation.source_id == "j11-failed-recovery"));

    let manager_experience = service
        .resolve(binding, &consumer, query("j11-manager-experience"))
        .await
        .unwrap();
    assert_eq!(manager_experience.citations.len(), 2);
    assert_eq!(
        manager_experience.citations[1].source_revision,
        receipt.experience_revision
    );
    let first_lease = autonomy::lease_next(pool, "j11-worker-before-revoke", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_lease.goal.id, goal.id);
    let pinned = autonomy::pin_run_context(
        pool,
        &first_lease,
        "j11-worker-before-revoke",
        Some("j11-policy:1"),
        serde_json::json!({
            "state": "cited",
            "query_id": scheduled_pack.query_id.clone(),
            "context_revision": scheduled_pack.context_revision.clone(),
            "acting_space_id": team_b.id,
            "citations": scheduled_pack.citations.iter().map(|citation| serde_json::json!({
                "citation_id": citation.citation_id.clone(),
                "space_id": citation.space_id.clone(),
                "source_revision": citation.source_revision.clone(),
            })).collect::<Vec<_>>(),
            "gaps": scheduled_pack.gaps.clone(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(pinned.context_snapshot["state"], "cited");
    assert_eq!(
        pinned.context_snapshot["citations"][0]["citation_id"],
        format!("{}:{}", shared.id, shared.revision)
    );
    let ready = autonomy::finish_run(
        pool,
        &first_lease,
        "j11-worker-before-revoke",
        RunFinish {
            outcome: "succeeded".into(),
            verification_state: "verified".into(),
            verification_summary: "Shared context was pinned to the bounded run".into(),
            evidence_refs: vec![format!("knowledge-context:{}", scheduled_pack.query_id)],
            disposition: RunDisposition::Succeeded,
        },
    )
    .await
    .unwrap();

    spaces::revoke_share(pool, actor, share.id, share.revision)
        .await
        .unwrap();
    let revoked = service
        .resolve(binding, &consumer, query("j11-revoked"))
        .await
        .unwrap();
    assert_eq!(revoked.coverage, ContextCoverage::Unavailable);
    assert!(revoked.citations.is_empty());
    assert_eq!(
        revoked.authority.allowed_space_ids,
        vec![team_b.id.to_string()]
    );

    sqlx::query("UPDATE autonomy_goals SET next_run_at = NOW() WHERE id = $1")
        .bind(ready.id)
        .execute(pool)
        .await
        .unwrap();
    let second_lease = autonomy::lease_next(pool, "j11-worker-after-revoke", 30)
        .await
        .unwrap()
        .unwrap();
    let scheduled_revoked = service
        .resolve(delegated, &consumer, query("j11-scheduled-revoked"))
        .await
        .unwrap();
    assert_eq!(scheduled_revoked.coverage, ContextCoverage::Unavailable);
    assert!(scheduled_revoked.citations.is_empty());
    let pinned_revoked = autonomy::pin_run_context(
        pool,
        &second_lease,
        "j11-worker-after-revoke",
        Some("j11-policy:1"),
        serde_json::json!({
            "state": "unavailable",
            "query_id": scheduled_revoked.query_id.clone(),
            "context_revision": scheduled_revoked.context_revision.clone(),
            "acting_space_id": team_b.id,
            "citations": [],
            "gaps": scheduled_revoked.gaps.clone(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(pinned_revoked.context_snapshot["state"], "unavailable");
    assert_eq!(
        pinned_revoked.context_snapshot["citations"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn incident_origin_exactly_round_trips_without_lexical_or_freshness_match() {
    if !pg_available().await {
        eprintln!("skipping CORE-T3 exact-origin fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let temp = tempfile::tempdir().unwrap();
    let layout = Arc::new(TenantVaultLayout::new(temp.path()));
    let (tenant_id, admin_id) = tenant_admin_and_teams(pool).await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let team = spaces::ensure_team_space(pool, actor, "j11-team-a", "Team A")
        .await
        .unwrap();
    let vault = spaces::ensure_vault(
        pool,
        actor,
        team.id,
        EnsureVault {
            home_bundle_id: "core".to_string(),
            knowledge_schema_id: "core.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let incident_id = Uuid::new_v4().to_string();
    let incident_revision = Uuid::new_v4().to_string();
    let path = format!("notes/{}.md", Uuid::new_v4());
    let lesson = note(
        pool,
        &layout,
        actor,
        team.id,
        vault.id,
        &path,
        "# Thermal recovery lesson\n\nVerify fan telemetry before declaring the incident resolved.\n",
    )
    .await;
    sqlx::query(
        r#"UPDATE knowledge_objects SET
             originating_owner_bundle = 'server-administrator',
             originating_subject_kind = 'server-administrator.server-incident',
             originating_subject_id = $3,
             originating_subject_revision = $4,
             updated_at = NOW() - INTERVAL '30 days'
           WHERE tenant_id = $1 AND id = $2"#,
    )
    .bind(tenant_id)
    .bind(lesson.id)
    .bind(&incident_id)
    .bind(&incident_revision)
    .execute(pool)
    .await
    .unwrap();
    let service = IntelligenceContextService::new(pool.clone(), layout);
    let binding = IntelligenceActorBinding {
        tenant_id,
        actor_id: admin_id,
        authority_actor_id: None,
        acting_space_id: Some(team.id),
    };
    let consumer = BundleId::new("server-administrator").unwrap();
    let exact = IntelligenceQueryDraft::new(
        "core-t3-exact-origin",
        SubjectRevisionRef::new(
            consumer.clone(),
            CapabilityId::new("server-administrator.server-incident").unwrap(),
            incident_id.clone(),
            incident_revision.clone(),
        )
        .unwrap(),
        "Unrelated quasar calibration phrase",
        60,
        IntelligenceBudget::new(8, 100, 65_536, 8_000, 10).unwrap(),
    )
    .unwrap();
    let pack = service.resolve(binding, &consumer, exact).await.unwrap();
    assert_eq!(pack.coverage, ContextCoverage::Partial);
    assert_eq!(pack.citations.len(), 1);
    assert_eq!(pack.citations[0].citation_id, format!("{}:1", lesson.id));
    assert!(pack.citations[0].passage.contains("Verify fan telemetry"));

    let wrong_revision = IntelligenceQueryDraft::new(
        "core-t3-wrong-origin",
        SubjectRevisionRef::new(
            consumer.clone(),
            CapabilityId::new("server-administrator.server-incident").unwrap(),
            incident_id,
            Uuid::new_v4().to_string(),
        )
        .unwrap(),
        "Unrelated quasar calibration phrase",
        60,
        IntelligenceBudget::new(8, 100, 65_536, 8_000, 10).unwrap(),
    )
    .unwrap();
    let absent = service
        .resolve(binding, &consumer, wrong_revision)
        .await
        .unwrap();
    assert_eq!(absent.coverage, ContextCoverage::Unavailable);
    assert!(absent.citations.is_empty());

    harness.cleanup().await;
}
