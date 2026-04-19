//! Web UI session management (ISSUE 15 TASK 15.1).
//!
//! Complements `auth::bootstrap` (password hashing) and `auth::validator`
//! (API-key Bearer auth) with cookie-based session auth for the web UI.
//!
//! Session lifecycle:
//! 1. `create_session` — email/password → argon2 verify → INSERT row,
//!    return (session_id, raw_cookie_token).
//! 2. Client stores cookie (HttpOnly, Secure, SameSite=Lax).
//! 3. Per request: `validate_session` looks up by `sha256(cookie_token)`,
//!    checks `expires_at > now` and `revoked_at IS NULL`, updates
//!    `last_active_at`.
//! 4. `revoke_session` — logout or admin revoke.
//!
//! Spec: docs/design/phase2/08-identity-and-users.md §2.2.4 + §2.4.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::bootstrap::argon2_verify;

/// Default session TTL — 24 hours per spec.
pub const DEFAULT_SESSION_TTL: Duration = Duration::hours(24);

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct SessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub user_agent: Option<String>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Returned from `create_session` — the raw cookie token lives here
/// EXACTLY ONCE and is never persisted. The DB stores only
/// `sha256(cookie_token)`.
#[derive(Debug)]
pub struct NewSession {
    pub session_id: Uuid,
    pub cookie_token: String,
    pub expires_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user is inactive")]
    Inactive,
    #[error("user is service-role; sessions are UI-only")]
    ServiceRole,
    #[error("session not found or expired")]
    NotFound,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Hash a raw cookie token with SHA-256 (not argon2 — token lookups
/// must be fast and the token itself is high-entropy already).
fn hash_cookie(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Generate a 32-byte (64 hex-char) random session token.
fn generate_cookie_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Authenticate `(email, password)` and open a new session row.
/// Returns the raw cookie token ONCE.
pub async fn create_session(
    pool: &PgPool,
    tenant_id: Uuid,
    email: &str,
    password: &str,
    user_agent: Option<&str>,
) -> Result<NewSession, SessionError> {
    // Fetch user + password hash. Inactive / service-role rejected.
    let row: Option<(Uuid, Uuid, String, Option<String>, bool)> = sqlx::query_as(
        r#"
        SELECT id, tenant_id, role, password_hash, is_active
        FROM users
        WHERE tenant_id = $1 AND email = $2
        "#,
    )
    .bind(tenant_id)
    .bind(email)
    .fetch_optional(pool)
    .await?;

    let (user_id, user_tenant, role, password_hash, is_active) = match row {
        Some(r) => r,
        None => return Err(SessionError::InvalidCredentials),
    };

    if !is_active {
        return Err(SessionError::Inactive);
    }
    // Parse the role column through the typed enum so an unexpected
    // value (shouldn't happen — DB has CHECK — but defensive) short-
    // circuits instead of falling through to password verification.
    match crate::identity::Role::parse(&role) {
        Some(crate::identity::Role::Service) => return Err(SessionError::ServiceRole),
        Some(_) => {} // Member or Admin — OK to continue.
        None => return Err(SessionError::InvalidCredentials),
    }
    let Some(stored_hash) = password_hash else {
        return Err(SessionError::InvalidCredentials);
    };
    if !argon2_verify(password, &stored_hash) {
        return Err(SessionError::InvalidCredentials);
    }

    let token = generate_cookie_token();
    let token_hash = hash_cookie(&token);
    let expires_at = Utc::now() + DEFAULT_SESSION_TTL;

    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO user_sessions
            (user_id, tenant_id, cookie_hash, expires_at, user_agent)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(user_tenant)
    .bind(&token_hash)
    .bind(expires_at)
    .bind(user_agent)
    .fetch_one(pool)
    .await?;

    // Update `last_login_at`.
    let _ = sqlx::query("UPDATE users SET last_login_at = NOW() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await;

    Ok(NewSession {
        session_id: id,
        cookie_token: token,
        expires_at,
        user_id,
        tenant_id: user_tenant,
    })
}

/// Validate a session cookie and return the owning user context.
/// Updates `last_active_at` on successful match. Expired / revoked
/// rows return NotFound.
pub async fn validate_session(
    pool: &PgPool,
    cookie_token: &str,
) -> Result<SessionRow, SessionError> {
    let token_hash = hash_cookie(cookie_token);
    let row: Option<SessionRow> = sqlx::query_as::<_, SessionRow>(
        r#"
        SELECT id, user_id, tenant_id, created_at, expires_at,
               last_active_at, user_agent, revoked_at
        FROM user_sessions
        WHERE cookie_hash = $1
          AND revoked_at IS NULL
          AND expires_at > NOW()
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?;

    let row = row.ok_or(SessionError::NotFound)?;

    // Touch last_active_at for idle-rotation tracking.
    let _ = sqlx::query("UPDATE user_sessions SET last_active_at = NOW() WHERE id = $1")
        .bind(row.id)
        .execute(pool)
        .await;

    Ok(row)
}

/// Revoke a session (logout). Idempotent — re-revoke is a no-op.
pub async fn revoke_session(pool: &PgPool, cookie_token: &str) -> Result<(), SessionError> {
    let token_hash = hash_cookie(cookie_token);
    sqlx::query(
        r#"
        UPDATE user_sessions
        SET revoked_at = NOW()
        WHERE cookie_hash = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(&token_hash)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_cookie_is_deterministic() {
        let a = hash_cookie("some-token");
        let b = hash_cookie("some-token");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn generated_token_is_64_hex() {
        let t = generate_cookie_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn default_ttl_is_24h() {
        assert_eq!(DEFAULT_SESSION_TTL, Duration::hours(24));
    }
}
