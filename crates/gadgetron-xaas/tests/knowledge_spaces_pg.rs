use chrono::{Duration, Utc};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::knowledge_graph::{
    self as graph, GraphEdgeInput, GraphNodeInput, GraphSnapshotInput, ReconcileMode,
};
use gadgetron_xaas::knowledge_spaces::{
    self as spaces, CreateProject, CreateShare, EnsureVault, KnowledgeSpaceError, PrincipalKind,
    RegisterObject, ShareMode, SpaceActor, SpaceRole, VaultOwnerState,
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

async fn insert_tenant(pool: &sqlx::PgPool, label: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_user(pool: &sqlx::PgPool, tenant_id: Uuid, label: &str, role: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $5 = 'service' THEN NULL ELSE 'test' END)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(format!("{label}@example.test"))
    .bind(label)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_team(pool: &sqlx::PgPool, tenant_id: Uuid, id: &str, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO teams (id, tenant_id, display_name) VALUES ($1, $2, $1)")
        .bind(id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn r2_1_two_tenant_space_acl_share_revision_and_owner_lifecycle() {
    if !pg_available().await {
        eprintln!("skipping R2.1 PostgreSQL fixture: pgvector/PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_a = insert_tenant(pool, "r2-1-a").await;
    let tenant_b = insert_tenant(pool, "r2-1-b").await;
    let admin_a = insert_user(pool, tenant_a, "admin-a", "admin").await;
    let member_a = insert_user(pool, tenant_a, "member-a", "member").await;
    let outsider_a = insert_user(pool, tenant_a, "outsider-a", "member").await;
    let admin_b = insert_user(pool, tenant_b, "admin-b", "admin").await;
    let actor_a = SpaceActor {
        tenant_id: tenant_a,
        user_id: admin_a,
    };
    let member_actor = SpaceActor {
        tenant_id: tenant_a,
        user_id: member_a,
    };
    let outsider_actor = SpaceActor {
        tenant_id: tenant_a,
        user_id: outsider_a,
    };
    let actor_b = SpaceActor {
        tenant_id: tenant_b,
        user_id: admin_b,
    };

    insert_team(pool, tenant_a, "platform-a", member_a, "member").await;
    insert_team(pool, tenant_b, "research-b", admin_b, "lead").await;
    sqlx::query("INSERT INTO groups (id, tenant_id, display_name) VALUES ('project-readers-a', $1, 'Readers')")
        .bind(tenant_a)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_groups (group_id, user_id) VALUES ('project-readers-a', $1)")
        .bind(member_a)
        .execute(pool)
        .await
        .unwrap();

    let personal = spaces::ensure_personal_space(pool, member_actor, "Member private")
        .await
        .unwrap();
    let team_a = spaces::ensure_team_space(pool, actor_a, "platform-a", "Platform")
        .await
        .unwrap();
    let team_b = spaces::ensure_team_space(pool, actor_b, "research-b", "Research")
        .await
        .unwrap();
    let shared = spaces::ensure_tenant_shared_space(pool, actor_a, "Tenant Shared")
        .await
        .unwrap();
    let project = spaces::create_project(
        pool,
        actor_a,
        CreateProject {
            slug: "awakening-engine".to_string(),
            title: "Awakening Engine".to_string(),
            goal: "Connected knowledge".to_string(),
            policy: serde_json::json!({"share": "reviewed"}),
        },
    )
    .await
    .unwrap();

    assert_eq!(personal.tenant_id, tenant_a);
    assert_eq!(team_b.tenant_id, tenant_b);
    assert_ne!(team_a.id, team_b.id);
    let initial = spaces::effective_spaces(pool, member_actor).await.unwrap();
    assert!(initial
        .iter()
        .any(|row| { row.space.id == personal.id && row.effective_role == SpaceRole::Manager }));
    assert!(initial
        .iter()
        .any(|row| { row.space.id == team_a.id && row.effective_role == SpaceRole::Contributor }));
    assert!(initial
        .iter()
        .any(|row| { row.space.id == shared.id && row.effective_role == SpaceRole::Viewer }));
    assert!(!initial.iter().any(|row| row.space.id == project.space.id));

    let grant = spaces::upsert_grant(
        pool,
        actor_a,
        project.space.id,
        PrincipalKind::Group,
        "project-readers-a",
        SpaceRole::Viewer,
        None,
    )
    .await
    .unwrap();
    assert!(spaces::effective_spaces(pool, member_actor)
        .await
        .unwrap()
        .iter()
        .any(|row| row.space.id == project.space.id));

    let expired = spaces::upsert_grant(
        pool,
        actor_a,
        project.space.id,
        PrincipalKind::User,
        &outsider_a.to_string(),
        SpaceRole::Viewer,
        Some(Utc::now() - Duration::minutes(1)),
    )
    .await
    .unwrap();
    assert!(!spaces::effective_spaces(pool, outsider_actor)
        .await
        .unwrap()
        .iter()
        .any(|row| row.space.id == project.space.id));

    let cross_tenant = spaces::upsert_grant(
        pool,
        actor_a,
        project.space.id,
        PrincipalKind::User,
        &admin_b.to_string(),
        SpaceRole::Viewer,
        None,
    )
    .await;
    assert!(matches!(cross_tenant, Err(KnowledgeSpaceError::NotFound)));

    let mut vaults = Vec::new();
    for bundle in [
        "core",
        "server-administrator",
        "travel-planner",
        "computer-science-research",
    ] {
        vaults.push(
            spaces::ensure_vault(
                pool,
                actor_a,
                project.space.id,
                EnsureVault {
                    home_bundle_id: bundle.to_string(),
                    knowledge_schema_id: format!("{bundle}.knowledge"),
                    schema_version: 1,
                },
            )
            .await
            .unwrap(),
        );
    }
    let server_vault = vaults
        .iter()
        .find(|vault| vault.home_bundle_id == "server-administrator")
        .unwrap();
    let object = spaces::register_object(
        pool,
        actor_a,
        server_vault.id,
        RegisterObject {
            canonical_kind: "lesson".to_string(),
            path: "lessons/server-recovery.md".to_string(),
            content_hash: Some("a".repeat(64)),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        spaces::register_object(
            pool,
            actor_a,
            server_vault.id,
            RegisterObject {
                canonical_kind: "note".to_string(),
                path: "../escape.md".to_string(),
                content_hash: None,
            },
        )
        .await,
        Err(KnowledgeSpaceError::InvalidInput(_))
    ));
    let share = spaces::share_object(
        pool,
        actor_a,
        object.id,
        CreateShare {
            target_space_id: team_a.id,
            source_revision: object.revision,
            mode: ShareMode::Reference,
            follow_latest: true,
            policy_disposition: "allowed".to_string(),
        },
    )
    .await
    .unwrap();
    assert_eq!(share.source_revision, 1);
    let team_vault = spaces::ensure_vault(
        pool,
        actor_a,
        team_a.id,
        EnsureVault {
            home_bundle_id: "server-administrator".to_string(),
            knowledge_schema_id: "server-administrator.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await
    .unwrap();
    let target_object = spaces::register_object(
        pool,
        actor_a,
        team_vault.id,
        RegisterObject {
            canonical_kind: "note".to_string(),
            path: format!("notes/{}.md", Uuid::new_v4()),
            content_hash: Some("b".repeat(64)),
        },
    )
    .await
    .unwrap();
    let snapshot = spaces::share_object_with_target(
        pool,
        actor_a,
        object.id,
        CreateShare {
            target_space_id: team_a.id,
            source_revision: object.revision,
            mode: ShareMode::Snapshot,
            follow_latest: false,
            policy_disposition: "allowed".to_string(),
        },
        Some(target_object.id),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.target_object_id, Some(target_object.id));
    let source_node_id = format!("note:{}", object.id);
    let reference_node_id = format!("share:{}", share.id);
    graph::materialize(
        pool,
        tenant_a,
        admin_a,
        ReconcileMode::Full,
        GraphSnapshotInput {
            nodes: vec![
                GraphNodeInput {
                    stable_node_id: source_node_id.clone(),
                    space_id: project.space.id,
                    vault_id: Some(server_vault.id),
                    node_kind: "note".to_string(),
                    canonical_id: Some(object.id),
                    canonical_revision: object.revision,
                    home_bundle_id: "server-administrator".to_string(),
                    title: "Server recovery".to_string(),
                    status: "active".to_string(),
                    freshness: "current".to_string(),
                    content_hash: object.content_hash.clone(),
                    metadata: serde_json::json!({}),
                },
                GraphNodeInput {
                    stable_node_id: reference_node_id.clone(),
                    space_id: team_a.id,
                    vault_id: None,
                    node_kind: "reference".to_string(),
                    canonical_id: None,
                    canonical_revision: share.revision,
                    home_bundle_id: "server-administrator".to_string(),
                    title: "Server recovery".to_string(),
                    status: "active".to_string(),
                    freshness: "current".to_string(),
                    content_hash: object.content_hash.clone(),
                    metadata: serde_json::json!({"source_space_id": project.space.id}),
                },
            ],
            edges: vec![GraphEdgeInput {
                stable_edge_id: "a".repeat(64),
                from_node_id: reference_node_id.clone(),
                to_node_id: Some(source_node_id.clone()),
                target_ref: source_node_id,
                relation_kind: "bridge_to".to_string(),
                source_space_id: team_a.id,
                target_space_id: Some(project.space.id),
                home_bundle_id: "server-administrator".to_string(),
                producer_kind: "system".to_string(),
                producer_revision: share.revision,
                status: "active".to_string(),
                evidence: serde_json::json!({"kind": "knowledge_share"}),
            }],
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        graph::get_node(pool, tenant_a, &[team_a.id], &reference_node_id).await,
        Err(KnowledgeSpaceError::NotFound)
    ));
    assert!(graph::get_node(
        pool,
        tenant_a,
        &[project.space.id, team_a.id],
        &reference_node_id,
    )
    .await
    .is_ok());
    assert!(matches!(
        spaces::share_object(
            pool,
            actor_a,
            object.id,
            CreateShare {
                target_space_id: team_a.id,
                source_revision: 999,
                mode: ShareMode::Snapshot,
                follow_latest: false,
                policy_disposition: "allowed".to_string(),
            },
        )
        .await,
        Err(KnowledgeSpaceError::RevisionConflict)
    ));
    let revoked_share = spaces::revoke_share(pool, actor_a, share.id, share.revision)
        .await
        .unwrap();
    assert!(revoked_share.revoked_at.is_some());
    let revoked_snapshot = spaces::revoke_share(pool, actor_a, snapshot.id, snapshot.revision)
        .await
        .unwrap();
    assert_eq!(revoked_snapshot.target_object_id, Some(target_object.id));
    assert!(matches!(
        spaces::revoke_share(pool, actor_a, share.id, share.revision).await,
        Err(KnowledgeSpaceError::RevisionConflict)
    ));

    let disabled = spaces::set_bundle_owner_state(
        pool,
        actor_a,
        "server-administrator",
        VaultOwnerState::OwnerUnavailable,
    )
    .await
    .unwrap();
    assert_eq!(disabled.len(), 2);
    let disabled_source = disabled
        .iter()
        .find(|vault| vault.id == server_vault.id)
        .unwrap();
    assert_eq!(disabled_source.revision, server_vault.revision + 1);
    let enabled = spaces::set_bundle_owner_state(
        pool,
        actor_a,
        "server-administrator",
        VaultOwnerState::Enabled,
    )
    .await
    .unwrap();
    let enabled_source = enabled
        .iter()
        .find(|vault| vault.id == server_vault.id)
        .unwrap();
    assert_eq!(enabled_source.revision, server_vault.revision + 2);

    assert!(matches!(
        spaces::set_bundle_owner_state(
            pool,
            member_actor,
            "server-administrator",
            VaultOwnerState::OwnerUnavailable,
        )
        .await,
        Err(KnowledgeSpaceError::Forbidden)
    ));

    spaces::revoke_grant(pool, actor_a, project.space.id, grant.id, grant.revision)
        .await
        .unwrap();
    assert!(!spaces::effective_spaces(pool, member_actor)
        .await
        .unwrap()
        .iter()
        .any(|row| row.space.id == project.space.id));
    assert!(matches!(
        spaces::revoke_grant(pool, actor_a, project.space.id, expired.id, 999).await,
        Err(KnowledgeSpaceError::RevisionConflict)
    ));

    let archived = spaces::archive_project(pool, actor_a, project.project.id, 1)
        .await
        .unwrap();
    assert_eq!(archived.project.status, "archived");
    assert_eq!(archived.space.status, "archived");
    let preserved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_objects WHERE id = $1 AND tenant_id = $2",
    )
    .bind(object.id)
    .bind(tenant_a)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(preserved, 1);
    assert!(matches!(
        spaces::register_object(
            pool,
            actor_a,
            server_vault.id,
            RegisterObject {
                canonical_kind: "note".to_string(),
                path: "after-archive.md".to_string(),
                content_hash: None,
            },
        )
        .await,
        Err(KnowledgeSpaceError::Forbidden)
    ));

    harness.cleanup().await;
}
