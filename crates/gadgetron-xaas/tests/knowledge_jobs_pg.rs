use chrono::{Duration, Utc};
use gadgetron_core::agent::config::{
    AgentBackend, AgentEffort, ConversationAgentProfile, ModelSource,
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    knowledge_agent_profiles::{
        self as profiles, KnowledgeRoleProfileScope, KnowledgeRoleProfileSource,
        KnowledgeRoleSelection, UpsertKnowledgeRoleProfile,
    },
    knowledge_jobs::{
        self as jobs, ArtifactInput, BundleRoleSnapshot, ChangeSetInput, EnqueueKnowledgeJob,
        JobBudget, KnowledgeJobError, KnowledgeJobKind, KnowledgeJobRole, RuntimeSnapshot,
    },
    knowledge_sources::{self as sources, AttachSourceBlob, CreateSource},
    knowledge_spaces::{
        self as spaces, CreateProject, EnsureVault, KnowledgeSpaceError, PrincipalKind, SpaceActor,
        SpaceRole,
    },
    service_principals,
};
use uuid::Uuid;

async fn pg_available() -> bool {
    let admin_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
    let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
    else {
        return false;
    };
    let vector: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
    )
    .fetch_optional(&pool)
    .await;
    pool.close().await;
    matches!(vector, Ok(Some(_)))
}

#[tokio::test]
async fn r3_4a_role_profile_inheritance_and_job_snapshot() {
    if !pg_available().await {
        eprintln!("skipping Knowledge AI role PostgreSQL test: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_id, admin_id) = tenant_and_admin(pool, "role-profile").await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let global = ConversationAgentProfile {
        backend: AgentBackend::ClaudeCode,
        llm_endpoint_id: None,
        model: "claude-sonnet-5".to_string(),
        effort: AgentEffort::Medium,
        model_source: ModelSource::Default,
        local_base_url: String::new(),
        local_api_key_env: String::new(),
    };
    let inherited = profiles::resolve_role_profile(pool, tenant_id, &global, "researcher", None)
        .await
        .unwrap();
    assert_eq!(inherited.source, KnowledgeRoleProfileSource::Global);

    profiles::upsert_role_profile_override(
        pool,
        tenant_id,
        admin_id,
        UpsertKnowledgeRoleProfile {
            scope: KnowledgeRoleProfileScope::Core,
            bundle_id: None,
            role_id: "researcher",
            expected_revision: None,
            selection: &KnowledgeRoleSelection {
                backend: AgentBackend::CodexExec,
                model: "gpt-5.5".to_string(),
                effort: AgentEffort::High,
                model_source: ModelSource::Default,
                llm_endpoint_id: None,
            },
        },
    )
    .await
    .unwrap();
    let core = profiles::resolve_role_profile(pool, tenant_id, &global, "researcher", None)
        .await
        .unwrap();
    assert_eq!(core.source, KnowledgeRoleProfileSource::Core);
    assert_eq!(core.selection.model, "gpt-5.5");
    let bundle_override = profiles::upsert_role_profile_override(
        pool,
        tenant_id,
        admin_id,
        UpsertKnowledgeRoleProfile {
            scope: KnowledgeRoleProfileScope::Bundle,
            bundle_id: Some("restaurant-research"),
            role_id: "restaurant-researcher",
            expected_revision: None,
            selection: &KnowledgeRoleSelection {
                backend: AgentBackend::ClaudeCode,
                model: "claude-fable-5".to_string(),
                effort: AgentEffort::Auto,
                model_source: ModelSource::Default,
                llm_endpoint_id: None,
            },
        },
    )
    .await
    .unwrap();
    let effective = profiles::resolve_role_profile(
        pool,
        tenant_id,
        &global,
        "researcher",
        Some(("restaurant-research", "restaurant-researcher")),
    )
    .await
    .unwrap();
    assert_eq!(effective.source, KnowledgeRoleProfileSource::Bundle);
    assert_eq!(effective.bundle_revision, Some(bundle_override.revision));
    assert_eq!(effective.selection.model, "claude-fable-5");

    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "restaurant-learning".to_string(),
            title: "Restaurant learning".to_string(),
            goal: "Preserve exact AI role execution identity".to_string(),
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
            home_bundle_id: "restaurant-research".to_string(),
            knowledge_schema_id: "restaurant.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let source_id = source_fixture(pool, actor, vault.id, "restaurant-source").await;
    let mut enqueue = request(project.space.id, vault.id, source_id);
    enqueue.idempotency_key = format!("bundle-role:{source_id}");
    enqueue.runtime.backend = effective.selection.backend.as_str().to_string();
    enqueue.runtime.model = effective.selection.model.clone();
    enqueue.runtime.effort = "high".to_string();
    enqueue.runtime.prompt_contract_revision = "restaurant-research-v1".to_string();
    enqueue.runtime.role_profile_source = Some(effective.source.as_str().to_string());
    enqueue.runtime.role_profile_ref = Some(effective.profile_ref.clone());
    enqueue.bundle_role = Some(BundleRoleSnapshot {
        bundle_id: "restaurant-research".to_string(),
        bundle_role_id: "restaurant-researcher".to_string(),
        package_manifest_sha256: "c".repeat(64),
        recipe_asset_id: "restaurant-core-source-recipe".to_string(),
        recipe_sha256: "d".repeat(64),
    });
    let job = jobs::enqueue(pool, actor, enqueue).await.unwrap();
    assert_eq!(job.role_profile_source.as_deref(), Some("bundle"));
    assert_eq!(
        job.role_profile_ref.as_deref(),
        Some(effective.profile_ref.as_str())
    );
    assert_eq!(job.bundle_id.as_deref(), Some("restaurant-research"));
    assert_eq!(job.bundle_role_id.as_deref(), Some("restaurant-researcher"));
    assert_eq!(
        job.package_manifest_sha256.as_deref(),
        Some("c".repeat(64).as_str())
    );
    assert_eq!(job.recipe_sha256.as_deref(), Some("d".repeat(64).as_str()));

    harness.cleanup().await;
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
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,$3,$4,'admin','test')"#,
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

async fn source_fixture(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
    suffix: &str,
) -> Uuid {
    let digest = "a".repeat(64);
    let blob_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO knowledge_blobs
           (id, tenant_id, content_hash, storage_key, byte_size, content_type, original_name, created_by)
           VALUES ($1,$2,$3,$4,12,'text/markdown',$5,$6)"#,
    )
    .bind(blob_id)
    .bind(actor.tenant_id)
    .bind(format!("sha256:{digest}"))
    .bind(format!("sha256/aa/{digest}"))
    .bind(format!("{suffix}.md"))
    .bind(actor.user_id)
    .execute(pool)
    .await
    .unwrap();
    let source = sources::create_pending_source(
        pool,
        actor,
        CreateSource {
            vault_id,
            conversation_id: None,
            source_kind: "upload".to_string(),
            title: format!("Source {suffix}"),
            original_name: format!("{suffix}.md"),
            requested_uri: None,
        },
    )
    .await
    .unwrap();
    let source = sources::attach_source_blob(
        pool,
        actor,
        source.id,
        source.revision,
        AttachSourceBlob {
            blob_id,
            content_type: "text/markdown".to_string(),
            byte_size: 12,
            content_hash: format!("sha256:{digest}"),
            final_uri: None,
        },
    )
    .await
    .unwrap();
    let object_id = Uuid::new_v4();
    sources::register_source_object(
        pool,
        actor,
        source.id,
        object_id,
        &format!("notes/{suffix}.md"),
        &"b".repeat(64),
    )
    .await
    .unwrap();
    sources::complete_source(pool, actor, source.id, source.revision, object_id)
        .await
        .unwrap()
        .id
}

fn request(space_id: Uuid, vault_id: Uuid, source_id: Uuid) -> EnqueueKnowledgeJob {
    EnqueueKnowledgeJob {
        space_id,
        output_vault_id: vault_id,
        role: KnowledgeJobRole::Researcher,
        kind: KnowledgeJobKind::OnDemand,
        priority: 5,
        input: serde_json::json!({"question": "What changed?"}),
        idempotency_key: format!("research:{source_id}"),
        source_ids: vec![source_id],
        runtime: RuntimeSnapshot {
            backend: "codex_exec".to_string(),
            model: "gpt-5.6-sol".to_string(),
            effort: "high".to_string(),
            endpoint_id: None,
            model_source: "default".to_string(),
            local_base_url: String::new(),
            local_api_key_env: String::new(),
            prompt_contract_revision: "research-v1".to_string(),
            tool_policy_revision: "knowledge-read-v1".to_string(),
            role_profile_source: None,
            role_profile_ref: None,
        },
        bundle_role: None,
        budget: JobBudget {
            max_tokens: 4_096,
            max_sources: 4,
            max_wall_seconds: 120,
            max_attempts: 3,
        },
        scheduled_at: None,
    }
}

#[tokio::test]
async fn r2_5_identity_dedup_lease_cancel_retry_budget_and_citation_boundary() {
    if !pg_available().await {
        eprintln!("skipping R2.5 PostgreSQL fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_id, admin_id) = tenant_and_admin(pool, "r2-5-a").await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "research-loop".to_string(),
            title: "Research Loop".to_string(),
            goal: "R2.5".to_string(),
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
            home_bundle_id: "computer-science-research".to_string(),
            knowledge_schema_id: "cs.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let source_id = source_fixture(pool, actor, vault.id, "paper-one").await;

    let principal = service_principals::ensure_knowledge_agent(pool, tenant_id)
        .await
        .unwrap();
    assert_eq!(principal.kind, "knowledge-agent");
    assert_eq!(
        principal.user_id,
        service_principals::ensure_knowledge_agent(pool, tenant_id)
            .await
            .unwrap()
            .user_id
    );
    let identity: (String, bool, Option<String>) = sqlx::query_as(
        "SELECT role, is_active, password_hash FROM users WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(principal.user_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(identity, ("service".to_string(), true, None));

    let job = jobs::enqueue(pool, actor, request(project.space.id, vault.id, source_id))
        .await
        .unwrap();
    let duplicate = jobs::enqueue(pool, actor, request(project.space.id, vault.id, source_id))
        .await
        .unwrap();
    assert_eq!(duplicate.id, job.id);
    assert_eq!(job.service_actor_user_id, principal.user_id);
    assert_eq!(job.on_behalf_of_user_id, Some(admin_id));
    assert_eq!(jobs::sources(pool, actor, job.id).await.unwrap().len(), 1);
    jobs::validate_execution_actor(pool, actor, &job)
        .await
        .unwrap();
    assert!(matches!(
        jobs::validate_execution_actor(
            pool,
            SpaceActor {
                tenant_id,
                user_id: Uuid::new_v4(),
            },
            &job,
        )
        .await,
        Err(KnowledgeJobError::NotFound)
    ));

    sqlx::query("UPDATE users SET is_active = FALSE WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(principal.user_id)
        .execute(pool)
        .await
        .unwrap();
    assert!(matches!(
        jobs::validate_execution_actor(pool, actor, &job).await,
        Err(KnowledgeJobError::ServicePrincipal(
            service_principals::ServicePrincipalError::Inactive { .. }
        ))
    ));
    sqlx::query(
        "UPDATE users SET is_active = TRUE, role = 'member', password_hash = 'test' \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(principal.user_id)
    .execute(pool)
    .await
    .unwrap();
    assert!(matches!(
        jobs::validate_execution_actor(pool, actor, &job).await,
        Err(KnowledgeJobError::ServicePrincipal(
            service_principals::ServicePrincipalError::IdentityConflict { .. }
        ))
    ));
    sqlx::query(
        "UPDATE users SET role = 'service', password_hash = NULL \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(principal.user_id)
    .execute(pool)
    .await
    .unwrap();

    let (first, second) = tokio::join!(
        jobs::lease_next(pool, "worker-a", 30),
        jobs::lease_next(pool, "worker-b", 30)
    );
    let leased = first.unwrap().or(second.unwrap()).unwrap();
    let worker = leased.lease_owner.as_deref().unwrap();
    assert_eq!(leased.attempt, 1);
    assert!(matches!(
        jobs::heartbeat(
            pool,
            job.id,
            "not-the-owner",
            jobs::HeartbeatUpdate {
                lease_seconds: 30,
                progress_percent: 10,
                checkpoint: serde_json::json!({}),
                used_tokens: 10,
                used_sources: 1,
            }
        )
        .await,
        Err(KnowledgeJobError::LeaseLost)
    ));

    let artifact = jobs::append_artifact(
        pool,
        job.id,
        worker,
        ArtifactInput {
            kind: "dossier".to_string(),
            title: "Pinned dossier".to_string(),
            summary: "One source".to_string(),
            payload: serde_json::json!({"claims": ["bounded"]}),
            citations: serde_json::json!([{"source_id": source_id, "locator": "line 1"}]),
        },
    )
    .await
    .unwrap();
    assert_eq!(artifact.job_id, job.id);
    assert!(jobs::append_artifact(
        pool,
        job.id,
        worker,
        ArtifactInput {
            kind: "candidate".to_string(),
            title: "Invalid citation".to_string(),
            summary: String::new(),
            payload: serde_json::json!({}),
            citations: serde_json::json!([{"source_id": Uuid::new_v4()}]),
        },
    )
    .await
    .is_err());

    let current = jobs::get(pool, actor, job.id).await.unwrap();
    let cancelling = jobs::request_cancel(pool, actor, job.id, current.revision)
        .await
        .unwrap();
    assert!(cancelling.cancel_requested_at.is_some());
    let cancelled = jobs::complete(pool, job.id, worker, 100, 1).await.unwrap();
    assert_eq!(cancelled.status, "cancelled");
    let retried = jobs::retry(pool, actor, job.id, cancelled.revision)
        .await
        .unwrap();
    assert_eq!(retried.status, "queued");

    let leased = jobs::lease_next(pool, "worker-retry", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.attempt, 2);
    sqlx::query("UPDATE knowledge_jobs SET lease_expires_at = $2 WHERE id = $1")
        .bind(job.id)
        .bind(Utc::now() - Duration::seconds(1))
        .execute(pool)
        .await
        .unwrap();
    let recovered = jobs::lease_next(pool, "worker-recovered", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.id, job.id);
    assert_eq!(recovered.attempt, 3);
    let budget = jobs::heartbeat(
        pool,
        job.id,
        "worker-recovered",
        jobs::HeartbeatUpdate {
            lease_seconds: 30,
            progress_percent: 90,
            checkpoint: serde_json::json!({"phase": "agent"}),
            used_tokens: 4_097,
            used_sources: 1,
        },
    )
    .await
    .unwrap();
    assert!(budget.budget_exceeded);
    assert_eq!(budget.job.status, "failed");

    let contributor_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO users (id, tenant_id, email, display_name, role, password_hash)
           VALUES ($1,$2,'contributor@r2-5.test','Contributor','member','test')"#,
    )
    .bind(contributor_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    let grant = spaces::upsert_grant(
        pool,
        actor,
        project.space.id,
        PrincipalKind::User,
        &contributor_id.to_string(),
        SpaceRole::Contributor,
        None,
    )
    .await
    .unwrap();
    let contributor = SpaceActor {
        tenant_id,
        user_id: contributor_id,
    };
    let delegated = jobs::enqueue(
        pool,
        contributor,
        request(project.space.id, vault.id, source_id),
    )
    .await
    .unwrap();
    jobs::validate_execution_actor(pool, contributor, &delegated)
        .await
        .unwrap();
    spaces::revoke_grant(pool, actor, project.space.id, grant.id, grant.revision)
        .await
        .unwrap();
    assert!(matches!(
        jobs::validate_execution_actor(pool, contributor, &delegated).await,
        Err(KnowledgeJobError::Space(KnowledgeSpaceError::Forbidden))
    ));

    harness.cleanup().await;
}

#[tokio::test]
async fn k11_tenant_concurrency_and_retry_idempotency_are_durable() {
    if !pg_available().await {
        eprintln!("skipping K1.1 concurrency/idempotency fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_id, admin_id) = tenant_and_admin(pool, "k11-budget").await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "k11-budget".to_string(),
            title: "K1.1 budget".to_string(),
            goal: "Prove tenant concurrency and retry idempotency".to_string(),
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
            home_bundle_id: "computer-science-research".to_string(),
            knowledge_schema_id: "cs.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let source_id = source_fixture(pool, actor, vault.id, "k11-source").await;

    let default_limit: i32 =
        sqlx::query_scalar("SELECT knowledge_job_concurrency_limit FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(default_limit, 4);
    sqlx::query("UPDATE tenants SET knowledge_job_concurrency_limit = 1 WHERE id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();

    let mut first_request = request(project.space.id, vault.id, source_id);
    first_request.idempotency_key = format!("k11-concurrency-a:{source_id}");
    let first_job = jobs::enqueue(pool, actor, first_request).await.unwrap();
    let mut second_request = request(project.space.id, vault.id, source_id);
    second_request.idempotency_key = format!("k11-concurrency-b:{source_id}");
    let second_job = jobs::enqueue(pool, actor, second_request).await.unwrap();

    let (left, right) = tokio::join!(
        jobs::lease_next(pool, "k11-worker-a", 30),
        jobs::lease_next(pool, "k11-worker-b", 30),
    );
    let leased = [left.unwrap(), right.unwrap()];
    assert_eq!(leased.iter().filter(|job| job.is_some()).count(), 1);
    let leased = leased.into_iter().flatten().next().unwrap();
    let deferred_id = if leased.id == first_job.id {
        second_job.id
    } else {
        first_job.id
    };
    assert_eq!(
        jobs::get(pool, actor, deferred_id).await.unwrap().status,
        "queued"
    );
    jobs::complete(
        pool,
        leased.id,
        leased.lease_owner.as_deref().unwrap(),
        10,
        1,
    )
    .await
    .unwrap();
    let deferred = jobs::lease_next(pool, "k11-worker-next", 30)
        .await
        .unwrap()
        .expect("deferred tenant job must lease after a slot is released");
    assert_eq!(deferred.id, deferred_id);
    jobs::complete(pool, deferred.id, "k11-worker-next", 10, 1)
        .await
        .unwrap();

    let mut gardener_request = request(project.space.id, vault.id, source_id);
    gardener_request.role = KnowledgeJobRole::Gardener;
    gardener_request.idempotency_key = format!("k11-idempotency:{source_id}");
    gardener_request.input = serde_json::json!({"question": "Create one reviewable change set"});
    let gardener = jobs::enqueue(pool, actor, gardener_request).await.unwrap();
    let leased = jobs::lease_next(pool, "k11-gardener-crash", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.id, gardener.id);
    let change = ChangeSetInput {
        candidate_artifact_id: None,
        title: "One durable proposal".to_string(),
        summary: "A retry must return this same change set".to_string(),
        operations: serde_json::json!([{
            "op": "create_note",
            "title": "Durable proposal",
            "body": "Apply exactly once after review."
        }]),
        citations: serde_json::json!([]),
        expected_git_revision: Some("a".repeat(40)),
        materialization_key: format!("gardener:{}", gardener.id),
    };
    let before_crash =
        jobs::append_change_set(pool, gardener.id, "k11-gardener-crash", change.clone())
            .await
            .unwrap();
    sqlx::query(
        "UPDATE knowledge_jobs SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE id = $1",
    )
    .bind(gardener.id)
    .execute(pool)
    .await
    .unwrap();
    let recovered = jobs::lease_next(pool, "k11-gardener-retry", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.id, gardener.id);
    assert_eq!(recovered.attempt, 2);
    let after_retry = jobs::append_change_set(pool, gardener.id, "k11-gardener-retry", change)
        .await
        .unwrap();
    assert_eq!(after_retry.id, before_crash.id);
    let change_set_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_change_sets WHERE job_id = $1")
            .bind(gardener.id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(change_set_count, 1);
    jobs::complete(pool, gardener.id, "k11-gardener-retry", 20, 1)
        .await
        .unwrap();

    harness.cleanup().await;
}

#[tokio::test]
async fn k14_user_merge_change_set_is_honest_and_idempotent() {
    if !pg_available().await {
        eprintln!("skipping K1.4 user merge fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let (tenant_id, admin_id) = tenant_and_admin(pool, "k14-user-merge").await;
    let actor = SpaceActor {
        tenant_id,
        user_id: admin_id,
    };
    let project = spaces::create_project(
        pool,
        actor,
        CreateProject {
            slug: "k14-user-merge".to_string(),
            title: "K1.4 cleanup".to_string(),
            goal: "Prepare an honest user-origin duplicate merge".to_string(),
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
            home_bundle_id: "computer-science-research".to_string(),
            knowledge_schema_id: "cs.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let input = ChangeSetInput {
        candidate_artifact_id: None,
        title: "Merge 2 duplicate notes".to_string(),
        summary: "Keep the primary note and combine exact duplicates.".to_string(),
        operations: serde_json::json!([{
            "op": "merge_notes",
            "sources": [
                {"object_id": first_id, "expected_revision": 1},
                {"object_id": second_id, "expected_revision": 1}
            ],
            "title": "Canonical note",
            "body": "Merged body"
        }]),
        citations: serde_json::json!([]),
        expected_git_revision: Some("b".repeat(40)),
        materialization_key: format!("user-merge:{admin_id}:{}", Uuid::new_v4()),
    };
    let created =
        jobs::create_user_merge_change_set(pool, actor, project.space.id, vault.id, input.clone())
            .await
            .unwrap();
    assert_eq!(created.origin, "user");
    assert_eq!(created.job_id, None);
    assert_eq!(created.created_by_user_id, admin_id);
    assert_eq!(created.status, "pending_user_review");
    let repeated =
        jobs::create_user_merge_change_set(pool, actor, project.space.id, vault.id, input)
            .await
            .unwrap();
    assert_eq!(repeated.id, created.id);

    harness.cleanup().await;
}
