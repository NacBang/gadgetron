use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::context::Scope;
use gadgetron_core::error::GadgetronError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ValidatedKey {
    pub api_key_id: Uuid,
    pub tenant_id: Uuid,
    pub scopes: Vec<Scope>,
}

#[async_trait]
pub trait KeyValidator: Send + Sync {
    async fn validate(&self, key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError>;
    async fn invalidate(&self, key_hash: &str);
}

/// Key validator for `--no-db` / dev mode.
///
/// Accepts any key hash and returns a synthetic `ValidatedKey` with all scopes.
/// Never consults PostgreSQL. Do not use in production.
pub struct InMemoryKeyValidator;

#[async_trait]
impl KeyValidator for InMemoryKeyValidator {
    async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        Ok(Arc::new(ValidatedKey {
            api_key_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            scopes: vec![Scope::OpenAiCompat, Scope::Management, Scope::XaasAdmin],
        }))
    }

    async fn invalidate(&self, _key_hash: &str) {
        // no-op: no cache to invalidate in memory-only mode
    }
}

/// Returns true if `raw_key` has a valid `gad_live_` or `gad_test_` prefix.
///
/// Used by auth middleware to reject obviously malformed keys before hashing.
#[allow(dead_code)]
pub(crate) fn validate_raw_key_format(raw_key: &str) -> bool {
    raw_key.starts_with("gad_live_") || raw_key.starts_with("gad_test_")
}

pub struct PgKeyValidator {
    pool: sqlx::PgPool,
    cache: moka::future::Cache<String, Arc<ValidatedKey>>,
}

impl PgKeyValidator {
    pub fn new(pool: sqlx::PgPool) -> Self {
        let cache = moka::future::Cache::builder()
            .max_capacity(10_000)
            .time_to_live(std::time::Duration::from_secs(600))
            .build();
        Self { pool, cache }
    }
}

#[async_trait]
impl KeyValidator for PgKeyValidator {
    async fn validate(&self, key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        if let Some(cached) = self.cache.get(key_hash).await {
            return Ok(cached);
        }

        let row: Option<KeyRow> = sqlx::query_as(
            "SELECT id, tenant_id, scopes FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(crate::error::sqlx_to_gadgetron)?;

        let row = row.ok_or(GadgetronError::TenantNotFound)?;

        let scopes: Vec<Scope> = row
            .scopes
            .iter()
            .filter_map(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
            .collect();

        let validated = Arc::new(ValidatedKey {
            api_key_id: row.id,
            tenant_id: row.tenant_id,
            scopes,
        });

        self.cache
            .insert(key_hash.to_string(), validated.clone())
            .await;

        Ok(validated)
    }

    async fn invalidate(&self, key_hash: &str) {
        self.cache.invalidate(key_hash).await;
    }
}

#[derive(Debug, FromRow)]
struct KeyRow {
    id: Uuid,
    tenant_id: Uuid,
    scopes: Vec<String>,
}
