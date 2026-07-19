use chrono::{Duration, Utc};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    knowledge_collections::{
        self as collections, CollectionLocator, CollectionQuery, CollectionRunControl,
        CollectionRunTrigger, CompleteCollectionItem, CreateKnowledgeCollection,
        EnqueueCollectionRun, KnowledgeCollectionError, UpdateKnowledgeCollection,
    },
    knowledge_spaces::{self as spaces, CreateProject, EnsureVault, SpaceActor},
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
    let available: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
    )
    .fetch_optional(&pool)
    .await;
    pool.close().await;
    matches!(available, Ok(Some(_)))
}

async fn insert_user(pool: &sqlx::PgPool, tenant_id: Uuid, role: &str, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,$3,$4,$5,'test')",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(format!("{name}@collections.test"))
    .bind(name)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn r3_4a_collection_restart_cancel_retry_budget_and_health() {
    if !pg_available().await {
        eprintln!("skipping collection fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let tenant_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'Collection tenant')")
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
    let owner_id = insert_user(harness.pool(), tenant_id, "admin", "owner").await;
    let hidden_id = insert_user(harness.pool(), tenant_id, "member", "hidden").await;
    let actor = SpaceActor {
        tenant_id,
        user_id: owner_id,
    };
    let project = spaces::create_project(
        harness.pool(),
        actor,
        CreateProject {
            slug: "collection-project".into(),
            title: "Collection project".into(),
            goal: "deterministic source collection".into(),
            policy: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let vault = spaces::ensure_vault(
        harness.pool(),
        actor,
        project.space.id,
        EnsureVault {
            home_bundle_id: "restaurant-research".into(),
            knowledge_schema_id: "restaurant.knowledge".into(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let next_run = Utc::now() + Duration::hours(1);
    let query_collection = collections::create_collection(
        harness.pool(),
        actor,
        CreateKnowledgeCollection {
            space_id: project.space.id,
            output_vault_id: vault.id,
            bundle_id: "community-intelligence".into(),
            profile_id: "community-official-providers".into(),
            label: "Community source collection".into(),
            topic: "Rust cancellation behavior".into(),
            connector: "core-community-api".into(),
            source_classes: vec!["community".into()],
            allowed_domains: vec!["api.stackexchange.com".into()],
            freshness_seconds: 3_600,
            schedule: Some("*/30 * * * *".into()),
            schedule_enabled: false,
            next_run_at: None,
            max_sources: 1,
            max_bytes: 1_048_576,
            max_wall_seconds: 180,
            package_manifest_sha256: "d".repeat(64),
            recipe_asset_id: "community-source-collection".into(),
            recipe_sha256: "e".repeat(64),
            locators: vec![CollectionLocator {
                url: "https://api.stackexchange.com/2.3/search/advanced?site=stackoverflow&q=rust&gadgetron_window_days=30".into(),
                title: "Stack Exchange · stackoverflow".into(),
                source_class: "community".into(),
            }],
            queries: vec![CollectionQuery {
                provider: "stack-exchange".into(),
                query: "rust cancellation".into(),
                scope: "stackoverflow".into(),
                tags: vec!["rust".into()],
                language: None,
                window_days: 30,
            }],
        },
    )
    .await
    .unwrap();
    assert_eq!(query_collection.parsed_queries().unwrap().len(), 1);
    assert_eq!(
        query_collection.parsed_queries().unwrap()[0].provider,
        "stack-exchange"
    );

    let collection = collections::create_collection(
        harness.pool(),
        actor,
        CreateKnowledgeCollection {
            space_id: project.space.id,
            output_vault_id: vault.id,
            bundle_id: "restaurant-research".into(),
            profile_id: "restaurant-public-sources".into(),
            label: "Restaurant source collection".into(),
            topic: "Seoul tasting menus".into(),
            connector: "core-source-fetch".into(),
            source_classes: vec!["official".into(), "editorial".into()],
            allowed_domains: vec!["guide.michelin.com".into(), "www.timeout.com".into()],
            freshness_seconds: 86_400,
            schedule: Some("0 6 * * *".into()),
            schedule_enabled: true,
            next_run_at: Some(next_run),
            max_sources: 3,
            max_bytes: 128,
            max_wall_seconds: 180,
            package_manifest_sha256: "a".repeat(64),
            recipe_asset_id: "restaurant-core-source-collection".into(),
            recipe_sha256: "b".repeat(64),
            locators: vec![
                CollectionLocator {
                    url: "https://guide.michelin.com/a".into(),
                    title: "Official A".into(),
                    source_class: "official".into(),
                },
                CollectionLocator {
                    url: "https://www.timeout.com/b".into(),
                    title: "Editorial B".into(),
                    source_class: "editorial".into(),
                },
                CollectionLocator {
                    url: "https://guide.michelin.com/deleted".into(),
                    title: "Removed listing".into(),
                    source_class: "official".into(),
                },
            ],
            queries: vec![],
        },
    )
    .await
    .unwrap();
    assert!(collections::list_collections(
        harness.pool(),
        SpaceActor {
            tenant_id,
            user_id: hidden_id,
        },
        project.space.id,
    )
    .await
    .is_err());

    let enqueued = collections::enqueue_run(
        harness.pool(),
        actor,
        collection.id,
        EnqueueCollectionRun {
            expected_collection_revision: collection.revision,
            trigger: CollectionRunTrigger::OnDemand,
            requested_by_user_id: owner_id,
            on_behalf_of_user_id: owner_id,
            tool_policy_revision: "policy:collection-v1".into(),
            scheduled_at: None,
            next_schedule_at: None,
        },
    )
    .await
    .unwrap();
    assert!(enqueued.created);
    let duplicate = collections::enqueue_run(
        harness.pool(),
        actor,
        collection.id,
        EnqueueCollectionRun {
            expected_collection_revision: collection.revision,
            trigger: CollectionRunTrigger::OnDemand,
            requested_by_user_id: owner_id,
            on_behalf_of_user_id: owner_id,
            tool_policy_revision: "policy:collection-v1".into(),
            scheduled_at: None,
            next_schedule_at: None,
        },
    )
    .await
    .unwrap();
    assert!(!duplicate.created);
    assert_eq!(duplicate.run.id, enqueued.run.id);
    collections::validate_execution_actor(harness.pool(), actor, &enqueued.run)
        .await
        .unwrap();
    assert!(matches!(
        collections::validate_execution_actor(
            harness.pool(),
            SpaceActor {
                tenant_id,
                user_id: hidden_id,
            },
            &enqueued.run,
        )
        .await,
        Err(KnowledgeCollectionError::InvalidInput(_))
    ));
    sqlx::query("UPDATE users SET is_active = FALSE WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(enqueued.run.service_actor_user_id)
        .execute(harness.pool())
        .await
        .unwrap();
    assert!(matches!(
        collections::validate_execution_actor(harness.pool(), actor, &enqueued.run).await,
        Err(KnowledgeCollectionError::ServicePrincipal(
            service_principals::ServicePrincipalError::Inactive { .. }
        ))
    ));
    sqlx::query("UPDATE users SET is_active = TRUE WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(enqueued.run.service_actor_user_id)
        .execute(harness.pool())
        .await
        .unwrap();
    let persisted_schedule = collections::get_collection(
        harness.pool(),
        actor,
        collection.id,
        spaces::SpaceRole::Viewer,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        persisted_schedule.next_run_at.unwrap().timestamp_micros(),
        next_run.timestamp_micros()
    );

    let leased = collections::lease_next(harness.pool(), "collector-a", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.id, enqueued.run.id);
    assert_eq!(leased.attempt, 1);
    let source_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO knowledge_sources
           (tenant_id, vault_id, source_kind, title, original_name, requested_uri,
            attempt_count, created_by)
           VALUES ($1,$2,'article','Official A','a','https://guide.michelin.com/a',1,$3)
           RETURNING id"#,
    )
    .bind(tenant_id)
    .bind(vault.id)
    .bind(owner_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    let first = collections::claim_next_item(harness.pool(), leased.id, "collector-a")
        .await
        .unwrap()
        .unwrap();
    collections::complete_item(
        harness.pool(),
        leased.id,
        "collector-a",
        first.id,
        CompleteCollectionItem {
            status: "captured".into(),
            source_id: Some(source_id),
            canonical_locator: Some(first.locator.clone()),
            content_hash: Some(format!("sha256:{}", "c".repeat(64))),
            byte_size: Some(64),
            http_status: Some(200),
            fetched_at: Some(Utc::now()),
            fresh_until: Some(Utc::now() + Duration::days(1)),
            deletion_observed_at: None,
            failure_code: None,
            failure_detail: None,
        },
    )
    .await
    .unwrap();
    let second = collections::claim_next_item(harness.pool(), leased.id, "collector-a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.position, 1);
    collections::release_lease(harness.pool(), leased.id, "collector-a")
        .await
        .unwrap();
    let resumed = collections::lease_next(harness.pool(), "collector-b", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.id, leased.id);
    assert_eq!(resumed.attempt, 2);
    let resumed_second = collections::claim_next_item(harness.pool(), resumed.id, "collector-b")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed_second.id, second.id);
    collections::complete_item(
        harness.pool(),
        resumed.id,
        "collector-b",
        resumed_second.id,
        CompleteCollectionItem {
            status: "unchanged".into(),
            source_id: Some(source_id),
            canonical_locator: Some(resumed_second.locator.clone()),
            content_hash: Some(format!("sha256:{}", "c".repeat(64))),
            byte_size: Some(32),
            http_status: Some(200),
            fetched_at: Some(Utc::now()),
            fresh_until: Some(Utc::now() + Duration::days(1)),
            deletion_observed_at: None,
            failure_code: None,
            failure_detail: None,
        },
    )
    .await
    .unwrap();
    let third = collections::claim_next_item(harness.pool(), resumed.id, "collector-b")
        .await
        .unwrap()
        .unwrap();
    collections::complete_item(
        harness.pool(),
        resumed.id,
        "collector-b",
        third.id,
        CompleteCollectionItem {
            status: "deleted".into(),
            source_id: None,
            canonical_locator: Some(third.locator.clone()),
            content_hash: None,
            byte_size: None,
            http_status: Some(410),
            fetched_at: None,
            fresh_until: None,
            deletion_observed_at: Some(Utc::now()),
            failure_code: None,
            failure_detail: None,
        },
    )
    .await
    .unwrap();
    let finished = collections::finish_run(harness.pool(), resumed.id, "collector-b", None)
        .await
        .unwrap();
    assert_eq!(finished.status, "succeeded");
    assert_eq!(finished.used_items, 3);
    assert_eq!(finished.used_bytes, 96);
    let health = collections::source_health(harness.pool(), actor, collection.id)
        .await
        .unwrap();
    assert_eq!(health.len(), 3);
    assert_eq!(health[0].health, "current");
    assert!(health.iter().any(|item| item.health == "deleted"));

    let current = collections::get_collection(
        harness.pool(),
        actor,
        collection.id,
        spaces::SpaceRole::Contributor,
        true,
    )
    .await
    .unwrap();
    collections::update_collection(
        harness.pool(),
        actor,
        collection.id,
        UpdateKnowledgeCollection {
            expected_revision: current.revision,
            topic: "Changed after the original run".into(),
            status: "active".into(),
            schedule_enabled: true,
            next_run_at: current.next_run_at,
            locators: vec![CollectionLocator {
                url: "https://guide.michelin.com/new".into(),
                title: "New configuration".into(),
                source_class: "official".into(),
            }],
            queries: vec![],
        },
    )
    .await
    .unwrap();

    let retried = collections::retry_run(harness.pool(), actor, finished.id, finished.revision)
        .await
        .unwrap();
    assert_eq!(retried.run.parent_run_id, Some(finished.id));
    assert_eq!(retried.run.trigger, "retry");
    let retry_items = collections::run_items(harness.pool(), actor, retried.run.id)
        .await
        .unwrap();
    assert_eq!(retry_items.len(), 3);
    assert_eq!(retry_items[0].locator, "https://guide.michelin.com/a");
    assert_eq!(retry_items[0].previous_source_id, Some(source_id));
    let retry_lease = collections::lease_next(harness.pool(), "collector-c", 30)
        .await
        .unwrap()
        .unwrap();
    let cancel_requested =
        collections::request_cancel(harness.pool(), actor, retry_lease.id, retry_lease.revision)
            .await
            .unwrap();
    assert!(cancel_requested.cancel_requested_at.is_some());
    let (_, control) = collections::heartbeat(harness.pool(), retry_lease.id, "collector-c", 30)
        .await
        .unwrap();
    assert_eq!(control, CollectionRunControl::CancelRequested);
    let cancelled = collections::finish_run(
        harness.pool(),
        retry_lease.id,
        "collector-c",
        Some("cancelled by user"),
    )
    .await
    .unwrap();
    assert_eq!(cancelled.status, "cancelled");

    let scheduled = collections::enqueue_scheduled_run(
        harness.pool(),
        collections::get_collection(
            harness.pool(),
            actor,
            collection.id,
            spaces::SpaceRole::Viewer,
            false,
        )
        .await
        .unwrap(),
        "policy:collection-v1".into(),
        Utc::now() + Duration::days(1),
    )
    .await
    .unwrap();
    assert_eq!(scheduled.run.trigger, "schedule");
    assert_ne!(scheduled.run.requested_by_user_id, owner_id);
    assert_eq!(scheduled.run.on_behalf_of_user_id, owner_id);
    let budget_run = collections::lease_next(harness.pool(), "collector-budget", 30)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE knowledge_collection_runs SET used_bytes = max_bytes WHERE id = $1")
        .bind(budget_run.id)
        .execute(harness.pool())
        .await
        .unwrap();
    let (_, budget_control) =
        collections::heartbeat(harness.pool(), budget_run.id, "collector-budget", 30)
            .await
            .unwrap();
    assert_eq!(budget_control, CollectionRunControl::ByteBudgetExceeded);
    let budget_failed = collections::finish_run(
        harness.pool(),
        budget_run.id,
        "collector-budget",
        Some("collection byte budget exhausted"),
    )
    .await
    .unwrap();
    assert_eq!(budget_failed.status, "failed");

    let before_rebind = collections::get_collection(
        harness.pool(),
        actor,
        collection.id,
        spaces::SpaceRole::Contributor,
        true,
    )
    .await
    .unwrap();
    let rebound = collections::rebind_collection_package(
        harness.pool(),
        actor,
        collection.id,
        before_rebind.revision,
        "c".repeat(64),
    )
    .await
    .unwrap();
    assert_eq!(rebound.package_manifest_sha256, "c".repeat(64));
    assert_eq!(rebound.recipe_sha256, before_rebind.recipe_sha256);
    assert_eq!(rebound.revision, before_rebind.revision + 1);
    let stale = collections::rebind_collection_package(
        harness.pool(),
        actor,
        collection.id,
        before_rebind.revision,
        "d".repeat(64),
    )
    .await
    .unwrap_err();
    assert!(matches!(stale, KnowledgeCollectionError::Conflict));

    harness.cleanup().await;
}
