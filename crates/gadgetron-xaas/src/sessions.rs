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

/// Open a session for a user who was already authenticated through a
/// trusted upstream (e.g. Google OAuth `id_token` verification). Skips
/// the password check in `create_session`; the caller is responsible
/// for having proven identity.
pub async fn create_session_for_user(
    pool: &PgPool,
    user_id: Uuid,
    user_agent: Option<&str>,
) -> Result<NewSession, SessionError> {
    let row: Option<(Uuid, String, bool)> = sqlx::query_as(
        r#"
        SELECT tenant_id, role, is_active
        FROM users
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let (tenant_id, role, is_active) = row.ok_or(SessionError::InvalidCredentials)?;
    if !is_active {
        return Err(SessionError::Inactive);
    }
    if matches!(
        crate::identity::Role::parse(&role),
        Some(crate::identity::Role::Service)
    ) {
        return Err(SessionError::ServiceRole);
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
    .bind(tenant_id)
    .bind(&token_hash)
    .bind(expires_at)
    .bind(user_agent)
    .fetch_one(pool)
    .await?;

    let _ = sqlx::query("UPDATE users SET last_login_at = NOW() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await;

    Ok(NewSession {
        session_id: id,
        cookie_token: token,
        expires_at,
        user_id,
        tenant_id,
    })
}

/// Upsert a user by Google OIDC `sub`. If a user row already exists for
/// this `(tenant_id, google_sub)` — or for `(tenant_id, email)` when the
/// `sub` isn't yet linked — its name is refreshed and the `google_sub`
/// column is set. Otherwise a new row is inserted with `role =
/// default_role`.
pub async fn upsert_user_from_google(
    pool: &PgPool,
    tenant_id: Uuid,
    google_sub: &str,
    email: &str,
    display_name: &str,
    default_role: &str,
    avatar_url: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    // Match-by-sub first (stable).
    let by_sub: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE tenant_id = $1 AND google_sub = $2",
    )
    .bind(tenant_id)
    .bind(google_sub)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = by_sub {
        let _ = sqlx::query(
            "UPDATE users SET display_name = $1, avatar_url = COALESCE($2, avatar_url), \
             updated_at = NOW() WHERE id = $3",
        )
        .bind(display_name)
        .bind(avatar_url)
        .bind(id)
        .execute(pool)
        .await;
        return Ok(id);
    }

    // Match-by-email (existing account linking first-time to Google).
    let by_email: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE tenant_id = $1 AND email = $2",
    )
    .bind(tenant_id)
    .bind(email)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = by_email {
        let _ = sqlx::query(
            "UPDATE users SET google_sub = $1, display_name = $2, \
             avatar_url = COALESCE($3, avatar_url), updated_at = NOW() \
             WHERE id = $4",
        )
        .bind(google_sub)
        .bind(display_name)
        .bind(avatar_url)
        .bind(id)
        .execute(pool)
        .await;
        return Ok(id);
    }

    // Fresh insert.
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (tenant_id, email, display_name, role, google_sub, avatar_url, is_active) \
         VALUES ($1, $2, $3, $4, $5, $6, TRUE) \
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(email)
    .bind(display_name)
    .bind(default_role)
    .bind(google_sub)
    .bind(avatar_url)
    .fetch_one(pool)
    .await?;
    Ok(id)
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
