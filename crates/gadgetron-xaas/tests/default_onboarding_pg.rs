use gadgetron_core::config::BootstrapConfig;
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    auth::bootstrap::{bootstrap_admin_if_needed, DEFAULT_TENANT_ID},
    default_onboarding::{
        ensure_default_team_onboarding, DEFAULT_TEAM_DISPLAY_NAME, DEFAULT_TEAM_HOME_BUNDLE_ID,
        DEFAULT_TEAM_SPACE_TITLE,
    },
    identity::{create_user, Role},
    knowledge_spaces::{self as spaces, SpaceActor},
    sessions::upsert_user_from_google,
    teams,
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

async fn insert_tenant(pool: &sqlx::PgPool, name: &str) -> Uuid {
    let tenant_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant_id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    tenant_id
}

#[tokio::test]
async fn signup_paths_join_one_default_team_and_preserve_private_team_acl() {
    if !pg_available().await {
        eprintln!("skipping default onboarding fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let tenant_id = insert_tenant(pool, "onboarding-signup").await;

    let user_a = create_user(
        pool,
        tenant_id,
        "member-a@example.test",
        "Member A",
        None,
        Role::Member,
        None,
    )
    .await
    .unwrap();
    let user_b = upsert_user_from_google(
        pool,
        tenant_id,
        "google-member-b",
        "member-b@example.test",
        "Member B",
        "member",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        user_b,
        upsert_user_from_google(
            pool,
            tenant_id,
            "google-member-b",
            "member-b@example.test",
            "Member B Renamed",
            "member",
            None,
        )
        .await
        .unwrap()
    );

    let topology = ensure_default_team_onboarding(pool, tenant_id, user_a.id)
        .await
        .unwrap()
        .unwrap();
    let team: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT display_name, created_by FROM teams WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(&topology.team_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(team.0, DEFAULT_TEAM_DISPLAY_NAME);
    assert_eq!(team.1, Some(topology.service_actor_id));

    let memberships: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT user_id, role FROM team_members WHERE team_id = $1 ORDER BY role, user_id",
    )
    .bind(&topology.team_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(memberships.contains(&(topology.service_actor_id, "lead".to_string())));
    assert!(memberships.contains(&(user_a.id, "member".to_string())));
    assert!(memberships.contains(&(user_b, "member".to_string())));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM teams WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
            .unwrap(),
        1
    );
    let space: (String, String) = sqlx::query_as(
        "SELECT title, owner_team_id FROM knowledge_spaces WHERE id = $1 AND tenant_id = $2",
    )
    .bind(topology.space_id)
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(space.0, DEFAULT_TEAM_SPACE_TITLE);
    assert_eq!(space.1, topology.team_id);
    let bundle_id: String =
        sqlx::query_scalar("SELECT home_bundle_id FROM knowledge_vaults WHERE id = $1")
            .bind(topology.vault_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(bundle_id, DEFAULT_TEAM_HOME_BUNDLE_ID);

    let private_team_id = format!("private-{}", &tenant_id.simple().to_string()[..12]);
    teams::create_team(
        pool,
        tenant_id,
        &private_team_id,
        "A팀 운영 지식",
        None,
        Some(topology.service_actor_id),
    )
    .await
    .unwrap();
    teams::add_team_member(
        pool,
        tenant_id,
        &private_team_id,
        user_a.id,
        "lead",
        Some(topology.service_actor_id),
    )
    .await
    .unwrap();
    let private_space = spaces::ensure_team_space(
        pool,
        SpaceActor {
            tenant_id,
            user_id: user_a.id,
        },
        &private_team_id,
        "A팀 운영 지식",
    )
    .await
    .unwrap();
    let user_b_spaces = spaces::effective_spaces(
        pool,
        SpaceActor {
            tenant_id,
            user_id: user_b,
        },
    )
    .await
    .unwrap();
    assert!(user_b_spaces
        .iter()
        .any(|row| row.space.id == topology.space_id));
    assert!(!user_b_spaces
        .iter()
        .any(|row| row.space.id == private_space.id));

    harness.cleanup().await;
}

#[tokio::test]
async fn bootstrap_admin_gets_default_team_atomically_and_idempotently() {
    if !pg_available().await {
        eprintln!("skipping bootstrap onboarding fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let password_env = format!("GADGETRON_TEST_BOOTSTRAP_{}", Uuid::new_v4().simple());
    std::env::set_var(&password_env, "onboarding-test-password");
    let config = BootstrapConfig {
        admin_email: "bootstrap@example.test".to_string(),
        admin_display_name: "Bootstrap Admin".to_string(),
        admin_password_env: password_env.clone(),
    };

    bootstrap_admin_if_needed(pool, Some(&config))
        .await
        .unwrap();
    bootstrap_admin_if_needed(pool, Some(&config))
        .await
        .unwrap();
    std::env::remove_var(password_env);

    let admin_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE tenant_id = $1 AND email = $2 AND role = 'admin'",
    )
    .bind(DEFAULT_TENANT_ID)
    .bind(&config.admin_email)
    .fetch_one(pool)
    .await
    .unwrap();
    let topology = ensure_default_team_onboarding(pool, DEFAULT_TENANT_ID, admin_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(topology.team_id, "operations");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM team_members WHERE team_id = $1 AND user_id = $2",
        )
        .bind(&topology.team_id)
        .bind(admin_id)
        .fetch_one(pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM knowledge_spaces WHERE tenant_id = $1 AND kind = 'team'",
        )
        .bind(DEFAULT_TENANT_ID)
        .fetch_one(pool)
        .await
        .unwrap(),
        1
    );

    harness.cleanup().await;
}
