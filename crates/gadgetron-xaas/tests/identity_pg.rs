use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_xaas::{
    auth::{
        bootstrap::DEFAULT_TENANT_ID,
        validator::{KeyValidator, PgKeyValidator},
    },
    identity::backfill_api_keys_user_id,
    service_principals::ensure_cli_api_key_owner,
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

async fn insert_tenant_and_admin(pool: &sqlx::PgPool, tenant_id: Uuid, label: &str) -> Uuid {
    let admin_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING")
        .bind(tenant_id)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
         VALUES ($1,$2,$3,$4,'admin','test')",
    )
    .bind(admin_id)
    .bind(tenant_id)
    .bind(format!("{label}@example.test"))
    .bind(label)
    .execute(pool)
    .await
    .unwrap();
    admin_id
}

async fn insert_legacy_key(pool: &sqlx::PgPool, tenant_id: Uuid, key_hash: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes) \
         VALUES ($1, 'gad_live', $2, 'live', ARRAY['Management']::TEXT[]) RETURNING id",
    )
    .bind(tenant_id)
    .bind(key_hash)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn r4_3_cli_key_identity_is_tenant_bound_and_legacy_backfill_is_default_only() {
    if !pg_available().await {
        eprintln!("skipping R4.3 identity PostgreSQL fixture: PostgreSQL unavailable");
        return;
    }
    let harness = PgHarness::new().await;
    let pool = harness.pool();
    let default_admin = insert_tenant_and_admin(pool, DEFAULT_TENANT_ID, "default-admin").await;
    let other_tenant = Uuid::new_v4();
    insert_tenant_and_admin(pool, other_tenant, "other-admin").await;

    let default_owner = ensure_cli_api_key_owner(pool, DEFAULT_TENANT_ID)
        .await
        .unwrap();
    let other_owner = ensure_cli_api_key_owner(pool, other_tenant).await.unwrap();
    assert_ne!(default_owner.user_id, other_owner.user_id);
    assert_eq!(
        other_owner.user_id,
        ensure_cli_api_key_owner(pool, other_tenant)
            .await
            .unwrap()
            .user_id
    );

    let default_hash = "a".repeat(64);
    let other_hash = "b".repeat(64);
    let default_key_id = insert_legacy_key(pool, DEFAULT_TENANT_ID, &default_hash).await;
    let other_key_id = insert_legacy_key(pool, other_tenant, &other_hash).await;

    assert_eq!(backfill_api_keys_user_id(pool).await.unwrap(), 1);
    assert_eq!(backfill_api_keys_user_id(pool).await.unwrap(), 0);

    let default_key_owner: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM api_keys WHERE id = $1")
            .bind(default_key_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let other_key_owner: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM api_keys WHERE id = $1")
            .bind(other_key_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(default_key_owner, Some(default_admin));
    assert_eq!(other_key_owner, None);

    let validated = PgKeyValidator::new(pool.clone())
        .validate(&other_hash)
        .await
        .unwrap();
    assert_eq!(validated.api_key_id, other_key_id);
    assert_eq!(validated.tenant_id, other_tenant);
    assert_eq!(validated.user_id, None);

    harness.cleanup().await;
}
