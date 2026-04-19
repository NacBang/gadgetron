//! User-scoped API-key self-service (ISSUE 14 TASK 14.4).
//!
//! Complements `auth::key_gen` (raw-key generation + hash) with the
//! "user sees / creates / revokes their own keys" surface. Tenant
//! boundary enforced by the handler (tenant_id = caller's). User
//! boundary enforced here (user_id = caller's api_key → user_id
//! lookup). Keys predating ISSUE 14 carry user_id = NULL; they show
//! up for callers who also have user_id = NULL (legacy equivalence
//! class) so harness / legacy operators aren't locked out before
//! bootstrap completes.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::key_gen::generate_api_key;

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct KeyRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub prefix: String,
    pub kind: String,
    pub scopes: Vec<String>,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Response for `POST /keys`. Raw key exposed EXACTLY ONCE; server
/// stores only the SHA-256 hash.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NewKeyResponse {
    pub id: Uuid,
    pub raw_key: String,
    pub prefix: String,
    pub kind: String,
    pub scopes: Vec<String>,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("key not found or not owned by caller")]
    NotFound,
    #[error("requested scope {0:?} exceeds caller's own scopes")]
    ScopeEscalation(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Look up the `user_id` owner of the calling API key. `Ok(None)`
/// for legacy keys (pre-backfill).
pub async fn caller_user_id(pool: &PgPool, api_key_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(r#"SELECT user_id FROM api_keys WHERE id = $1"#)
        .bind(api_key_id)
        .fetch_one(pool)
        .await
}

/// List keys owned by `owner_user_id` in `tenant_id`. NULL==NULL via
/// `IS NOT DISTINCT FROM`. Includes revoked rows (UI filters client-side).
pub async fn list_keys(
    pool: &PgPool,
    tenant_id: Uuid,
    owner_user_id: Option<Uuid>,
) -> Result<Vec<KeyRow>, sqlx::Error> {
    sqlx::query_as::<_, KeyRow>(
        r#"
        SELECT id, tenant_id, user_id, prefix, kind, scopes, label,
               created_at, last_used_at, revoked_at
        FROM api_keys
        WHERE tenant_id = $1 AND user_id IS NOT DISTINCT FROM $2
        ORDER BY created_at DESC
        "#,
    )
    .bind(tenant_id)
    .bind(owner_user_id)
    .fetch_all(pool)
    .await
}

/// Create a new key bound to the caller's user in the caller's tenant.
/// Handler enforces scope narrowing BEFORE calling here.
pub async fn create_key(
    pool: &PgPool,
    tenant_id: Uuid,
    owner_user_id: Option<Uuid>,
    label: Option<&str>,
    scopes: &[String],
    kind: &str,
) -> Result<NewKeyResponse, KeyError> {
    let (raw, hash) = generate_api_key(kind);
    let prefix = raw.chars().take(12).collect::<String>();

    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        r#"
        INSERT INTO api_keys (tenant_id, user_id, prefix, key_hash, kind, scopes, label)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, created_at
        "#,
    )
    .bind(tenant_id)
    .bind(owner_user_id)
    .bind(&prefix)
    .bind(&hash)
    .bind(kind)
    .bind(scopes)
    .bind(label)
    .fetch_one(pool)
    .await?;

    Ok(NewKeyResponse {
        id: row.0,
        raw_key: raw,
        prefix,
        kind: kind.to_string(),
        scopes: scopes.to_vec(),
        label: label.map(str::to_string),
        created_at: row.1,
    })
}

/// Revoke a key. Caller must own it (user_id match) within the same
/// tenant. Idempotent — re-revoking is a no-op success.
pub async fn revoke_key(
    pool: &PgPool,
    tenant_id: Uuid,
    caller_user_id: Option<Uuid>,
    key_id: Uuid,
) -> Result<(), KeyError> {
    let rows = sqlx::query(
        r#"
        UPDATE api_keys
        SET revoked_at = NOW()
        WHERE id = $1
          AND tenant_id = $2
          AND user_id IS NOT DISTINCT FROM $3
          AND revoked_at IS NULL
        "#,
    )
    .bind(key_id)
    .bind(tenant_id)
    .bind(caller_user_id)
    .execute(pool)
    .await?;

    if rows.rows_affected() == 0 {
        let already_revoked: Option<Uuid> = sqlx::query_scalar(
            r#"SELECT id FROM api_keys
               WHERE id = $1 AND tenant_id = $2 AND revoked_at IS NOT NULL
               AND user_id IS NOT DISTINCT FROM $3"#,
        )
        .bind(key_id)
        .bind(tenant_id)
        .bind(caller_user_id)
        .fetch_optional(pool)
        .await?;
        if already_revoked.is_some() {
            return Ok(()); // idempotent
        }
        return Err(KeyError::NotFound);
    }
    Ok(())
}
