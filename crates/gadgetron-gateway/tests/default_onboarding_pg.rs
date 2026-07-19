use gadgetron_gateway::default_onboarding::{
    apply_default_team_guide_titles, ensure_default_team_guides,
};
use gadgetron_knowledge::{
    source::parse_obsidian_note,
    vault::{note_relative_path, TenantVaultLayout},
};
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    default_onboarding::ensure_existing_default_team_onboarding,
    identity::{create_user, Role},
    knowledge_spaces::{self as spaces, SpaceActor},
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

#[tokio::test]
async fn default_team_guide_uses_canonical_vault_path_and_is_visible_once() {
    if !pg_available().await {
        eprintln!("skipping default Team guide fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'guide-tenant')")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    let member = create_user(
        pool,
        tenant_id,
        "guide-member@example.test",
        "Guide Member",
        None,
        Role::Member,
        None,
    )
    .await
    .unwrap();
    let topologies = ensure_existing_default_team_onboarding(pool).await.unwrap();
    assert_eq!(topologies.len(), 1);
    let topology = &topologies[0];
    let vault_root = tempfile::tempdir().unwrap();
    let layout = TenantVaultLayout::new(vault_root.path());

    let first = ensure_default_team_guides(pool, &layout, &topologies)
        .await
        .unwrap();
    let second = ensure_default_team_guides(pool, &layout, &topologies)
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first[0].object_id, topology.space_id);
    assert_eq!(
        first[0].path,
        note_relative_path("What this Space is for", first[0].object_id)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM knowledge_objects WHERE tenant_id = $1 AND vault_id = $2 AND status = 'active'",
        )
        .bind(tenant_id)
        .bind(topology.vault_id)
        .fetch_one(pool)
        .await
        .unwrap(),
        1
    );

    let actor = SpaceActor {
        tenant_id,
        user_id: member.id,
    };
    let mut objects =
        spaces::list_objects(pool, actor, topology.space_id, Some("core"), Some("note"))
            .await
            .unwrap();
    apply_default_team_guide_titles(&mut objects);
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].id, first[0].object_id);
    assert_eq!(objects[0].created_by, topology.service_actor_id);
    assert_eq!(objects[0].title.as_deref(), Some("What this Space is for"));

    let repository = layout.open_existing(tenant_id).unwrap();
    let note = repository
        .read_note_exact(
            topology.space_id,
            "core",
            &first[0].path,
            objects[0].content_hash.as_deref(),
        )
        .unwrap();
    assert!(!note.externally_changed);
    let raw = String::from_utf8(note.bytes).unwrap();
    let parsed = parse_obsidian_note(&raw).unwrap();
    assert_eq!(
        parsed
            .properties
            .get("title")
            .and_then(|value| value.as_str()),
        Some("What this Space is for")
    );
    assert!(parsed
        .body
        .contains("Record the knowledge, decisions, and outcomes"));
    assert_eq!(
        parsed
            .properties
            .get("locale")
            .and_then(serde_json::Value::as_str),
        Some("en")
    );

    harness.cleanup().await;
}
