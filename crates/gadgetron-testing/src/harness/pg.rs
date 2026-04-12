use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

/// Local-PostgreSQL-backed test harness.
///
/// `new()` creates a fresh database named `gadgetron_test_<uuid>` on
/// `localhost:5432`, runs all `gadgetron-xaas` migrations, and exposes
/// the resulting `PgPool`.
///
/// `cleanup()` drops the temporary database. Call it at the end of every
/// test — it is not automatic because async `Drop` is not supported in Rust.
///
/// Each test that calls `PgHarness::new().await` gets a fully isolated
/// database. Parallel test runs are safe: the UUID suffix prevents collisions.
///
/// # Panics
/// Panics if PostgreSQL is not reachable at `localhost:5432` as the current OS
/// user. On the development machine (`junghopark`) this is satisfied without a
/// password. In CI set `PGHOST`/`PGUSER` as appropriate.
pub struct PgHarness {
    pub pool: PgPool,
    pub db_name: String,
}

impl PgHarness {
    fn admin_url() -> String {
        std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string())
    }

    fn base_url() -> String {
        let url = Self::admin_url();
        url.rsplit_once('/')
            .map(|(base, _)| base.to_string())
            .unwrap_or(url)
    }

    pub async fn new() -> Self {
        let db_name = format!("gadgetron_test_{}", Uuid::new_v4().simple());

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&Self::admin_url())
            .await
            .expect("admin connect to postgres failed — is PostgreSQL running? Set DATABASE_URL if needed.");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin)
            .await
            .expect("CREATE DATABASE failed");
        admin.close().await;

        // Connect to the new database and run migrations.
        let url = format!("{}/{db_name}", Self::base_url());
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("connect to test database failed");

        // Migration path is relative to the workspace root at compile time.
        sqlx::migrate!("../../crates/gadgetron-xaas/migrations")
            .run(&pool)
            .await
            .expect("migrations failed");

        Self { pool, db_name }
    }

    /// Return a reference to the connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Insert a test tenant with one API key that has the `OpenAiCompat` scope.
    ///
    /// Returns `(tenant_id, raw_api_key)`.
    ///
    /// The raw key has format `gad_live_<32 hex chars>`, which passes
    /// `ApiKey::parse` (requires `gad_` prefix and ≥16-char suffix).
    ///
    /// The key is stored in `api_keys.key_hash` as `sha256(raw_key)` — exactly
    /// the same computation that `ApiKey::parse` and `FakePgKeyValidator` use.
    pub async fn insert_test_tenant(&self) -> (Uuid, String) {
        let tenant_id = Uuid::new_v4();

        // Format: gad_live_<32 hex chars> — satisfies ApiKey::parse validation.
        let suffix = format!("{:032x}", Uuid::new_v4().as_u128());
        let raw_key = format!("gad_live_{suffix}");
        let key_hash = sha2_hex(&raw_key);

        // Insert tenant.
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
            .bind(tenant_id)
            .bind(format!("test-tenant-{tenant_id}"))
            .execute(&self.pool)
            .await
            .expect("insert tenant failed");

        // Insert API key with OpenAiCompat scope.
        // The scopes TEXT[] stores the serde variant name used by PgKeyValidator.
        sqlx::query(
            "INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes) \
             VALUES ($1, 'gad_live', $2, 'live', ARRAY['OpenAiCompat']::TEXT[])",
        )
        .bind(tenant_id)
        .bind(&key_hash)
        .execute(&self.pool)
        .await
        .expect("insert api_key failed");

        (tenant_id, raw_key)
    }

    /// Insert a test tenant with a key scoped only to `management` — used to
    /// test wrong-scope scenarios where the key exists but is denied on a route
    /// that requires a different scope.
    pub async fn insert_mgmt_only_tenant(&self) -> (Uuid, String) {
        let tenant_id = Uuid::new_v4();
        let suffix = format!("{:032x}", Uuid::new_v4().as_u128());
        let raw_key = format!("gad_live_{suffix}");
        let key_hash = sha2_hex(&raw_key);

        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
            .bind(tenant_id)
            .bind(format!("mgmt-tenant-{tenant_id}"))
            .execute(&self.pool)
            .await
            .expect("insert mgmt tenant failed");

        sqlx::query(
            "INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes) \
             VALUES ($1, 'gad_live', $2, 'live', ARRAY['Management']::TEXT[])",
        )
        .bind(tenant_id)
        .bind(&key_hash)
        .execute(&self.pool)
        .await
        .expect("insert mgmt api_key failed");

        (tenant_id, raw_key)
    }

    /// Drop the temporary database and release the connection pool.
    ///
    /// Must be called explicitly at the end of each test.
    pub async fn cleanup(self) {
        self.pool.close().await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&Self::admin_url())
            .await
            .expect("admin connect for cleanup failed");

        // WITH (FORCE) terminates any remaining connections to the test database
        // before dropping it. Required when the gateway's connection pool has not
        // fully drained by the time cleanup() runs. PostgreSQL ≥13 only — local
        // dev uses PG 16 and CI pinned to PG 16.
        sqlx::query(&format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
            self.db_name
        ))
        .execute(&admin)
        .await
        .expect("DROP DATABASE failed");

        admin.close().await;
    }
}

/// SHA-256 hex digest — identical algorithm to `ApiKey::parse` and
/// `FakePgKeyValidator`. Must stay in sync with both.
pub fn sha2_hex(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(raw.as_bytes()))
}
