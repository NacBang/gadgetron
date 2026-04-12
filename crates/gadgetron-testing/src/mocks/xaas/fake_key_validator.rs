use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::{context::Scope, error::GadgetronError};
use gadgetron_xaas::auth::validator::{KeyValidator, ValidatedKey};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// PgPool-backed KeyValidator for E2E tests.
///
/// The `KeyValidator::validate(key_hash)` contract: the caller (`auth_middleware`)
/// passes the SHA-256 hex hash that `ApiKey::parse` computed from the raw token.
/// This validator receives the already-hashed value and queries the DB directly
/// — no second hash step.
///
/// This matches the production `PgKeyValidator` contract exactly.
/// The cache layer is absent (no moka) to keep tests deterministic.
pub struct FakePgKeyValidator {
    pool: PgPool,
}

impl FakePgKeyValidator {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn to_db_err(e: sqlx::Error) -> GadgetronError {
    GadgetronError::Database {
        kind: gadgetron_core::error::DatabaseErrorKind::QueryFailed,
        message: e.to_string(),
    }
}

#[async_trait]
impl KeyValidator for FakePgKeyValidator {
    /// Validate a key hash (already computed by the auth middleware).
    ///
    /// `key_hash` is the SHA-256 hex digest of the raw bearer token, as computed
    /// by `ApiKey::parse`. We look it up in `api_keys.key_hash` directly.
    async fn validate(&self, key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        let row = sqlx::query(
            "SELECT id, tenant_id, scopes \
             FROM api_keys \
             WHERE key_hash = $1 AND revoked_at IS NULL",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_db_err)?
        .ok_or(GadgetronError::TenantNotFound)?;

        let api_key_id: Uuid = row.try_get("id").map_err(to_db_err)?;
        let tenant_id: Uuid = row.try_get("tenant_id").map_err(to_db_err)?;
        let scope_strings: Vec<String> = row.try_get("scopes").map_err(to_db_err)?;

        // Parse TEXT[] scopes to Vec<Scope>.
        // Production PgKeyValidator uses serde_json deserialization of the variant name.
        let scopes: Vec<Scope> = scope_strings
            .iter()
            .filter_map(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
            .collect();

        Ok(Arc::new(ValidatedKey {
            api_key_id,
            tenant_id,
            scopes,
        }))
    }

    async fn invalidate(&self, _key_hash: &str) {
        // No cache to invalidate.
    }
}
