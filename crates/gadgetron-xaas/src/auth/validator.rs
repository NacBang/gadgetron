use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gadgetron_core::context::Scope;
use gadgetron_core::error::GadgetronError;
use gadgetron_core::provider::{
    CallbackCredentialIssuer, CallbackCredentialLease, ChatAuditContext,
};
use gadgetron_core::secret::Secret;
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::key_gen::generate_api_key;

#[derive(Debug, Clone)]
pub struct ValidatedKey {
    pub api_key_id: Uuid,
    pub tenant_id: Uuid,
    pub scopes: Vec<Scope>,
    /// Owning user — populated for both Bearer and cookie auth paths
    /// when the DB row has `api_keys.user_id` set, OR the cookie-session
    /// middleware synthesizes it from the session row.
    /// `None` for legacy keys predating the user backfill.
    pub user_id: Option<Uuid>,
}

#[async_trait]
pub trait KeyValidator: Send + Sync {
    async fn validate(&self, key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError>;
    async fn invalidate(&self, key_hash: &str);
}

#[derive(Clone)]
struct DelegatedKeyEntry {
    validated: Arc<ValidatedKey>,
    expires_at: Instant,
}

/// Adds process-local, turn-scoped credentials in front of an existing
/// validator. Delegated hashes never enter PostgreSQL and disappear when the
/// owning lease is dropped or its bounded TTL expires.
pub struct DelegatedKeyValidator {
    inner: Arc<dyn KeyValidator + Send + Sync>,
    entries: Arc<dashmap::DashMap<String, DelegatedKeyEntry>>,
    ttl: Duration,
}

impl DelegatedKeyValidator {
    pub const DEFAULT_TTL: Duration = Duration::from_secs(15 * 60);

    pub fn new(inner: Arc<dyn KeyValidator + Send + Sync>) -> Self {
        Self::with_ttl(inner, Self::DEFAULT_TTL)
    }

    pub fn with_ttl(inner: Arc<dyn KeyValidator + Send + Sync>, ttl: Duration) -> Self {
        Self {
            inner,
            entries: Arc::new(dashmap::DashMap::new()),
            ttl,
        }
    }

    pub fn active_count(&self) -> usize {
        let now = Instant::now();
        self.entries.retain(|_, entry| entry.expires_at > now);
        self.entries.len()
    }
}

impl CallbackCredentialIssuer for DelegatedKeyValidator {
    fn issue(
        &self,
        actor: &ChatAuditContext,
    ) -> gadgetron_core::error::Result<CallbackCredentialLease> {
        let tenant_id = Uuid::parse_str(&actor.tenant_id).map_err(|_| {
            GadgetronError::Config("callback credential tenant id is invalid".into())
        })?;
        let user_id = actor
            .owner_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| {
                GadgetronError::Config("callback credential owner id is invalid".into())
            })?;
        let (raw, hash) = generate_api_key("delegate");
        self.entries.insert(
            hash.clone(),
            DelegatedKeyEntry {
                validated: Arc::new(ValidatedKey {
                    api_key_id: Uuid::new_v4(),
                    tenant_id,
                    scopes: vec![Scope::OpenAiCompat],
                    user_id,
                }),
                expires_at: Instant::now() + self.ttl,
            },
        );

        let entries = Arc::clone(&self.entries);
        Ok(CallbackCredentialLease::new(Secret::new(raw), move || {
            entries.remove(&hash);
        }))
    }
}

#[async_trait]
impl KeyValidator for DelegatedKeyValidator {
    async fn validate(&self, key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        if let Some(entry) = self.entries.get(key_hash) {
            if entry.expires_at > Instant::now() {
                return Ok(Arc::clone(&entry.validated));
            }
            drop(entry);
            self.entries.remove(key_hash);
        }
        self.inner.validate(key_hash).await
    }

    async fn invalidate(&self, key_hash: &str) {
        self.entries.remove(key_hash);
        self.inner.invalidate(key_hash).await;
    }
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
            user_id: None,
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
            "SELECT id, tenant_id, scopes, user_id FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL",
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
            user_id: row.user_id,
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
    user_id: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::key::ApiKey;

    struct RejectingValidator;

    #[async_trait]
    impl KeyValidator for RejectingValidator {
        async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
            Err(GadgetronError::TenantNotFound)
        }

        async fn invalidate(&self, _key_hash: &str) {}
    }

    fn actor() -> (ChatAuditContext, Uuid, Uuid) {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        (
            ChatAuditContext {
                tenant_id: tenant_id.to_string(),
                owner_id: Some(user_id.to_string()),
            },
            tenant_id,
            user_id,
        )
    }

    #[tokio::test]
    async fn delegated_lease_preserves_actor_narrows_scope_and_revokes() {
        let validator = DelegatedKeyValidator::new(Arc::new(RejectingValidator));
        let (actor, tenant_id, user_id) = actor();
        let lease = validator.issue(&actor).expect("issue delegated credential");
        let parsed = ApiKey::parse(lease.expose()).expect("parse delegated key");

        let validated = validator
            .validate(&parsed.hash)
            .await
            .expect("validate active lease");
        assert_eq!(validated.tenant_id, tenant_id);
        assert_eq!(validated.user_id, Some(user_id));
        assert_eq!(validated.scopes, vec![Scope::OpenAiCompat]);
        assert_eq!(validator.active_count(), 1);

        drop(lease);
        assert_eq!(validator.active_count(), 0);
        assert!(validator.validate(&parsed.hash).await.is_err());
    }

    #[tokio::test]
    async fn delegated_lease_expires_and_unknown_hash_falls_through() {
        let validator =
            DelegatedKeyValidator::with_ttl(Arc::new(RejectingValidator), Duration::from_millis(1));
        let (actor, _, _) = actor();
        let lease = validator.issue(&actor).expect("issue delegated credential");
        let parsed = ApiKey::parse(lease.expose()).expect("parse delegated key");
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(validator.validate(&parsed.hash).await.is_err());
        assert_eq!(validator.active_count(), 0);
        assert!(validator.validate("unknown-hash").await.is_err());
    }

    #[test]
    fn delegated_lease_rejects_invalid_actor_ids() {
        let validator = DelegatedKeyValidator::new(Arc::new(RejectingValidator));
        let invalid_tenant = ChatAuditContext {
            tenant_id: "not-a-uuid".into(),
            owner_id: None,
        };
        assert!(validator.issue(&invalid_tenant).is_err());

        let invalid_owner = ChatAuditContext {
            tenant_id: Uuid::new_v4().to_string(),
            owner_id: Some("not-a-uuid".into()),
        };
        assert!(validator.issue(&invalid_owner).is_err());
        assert_eq!(validator.active_count(), 0);
    }
}
