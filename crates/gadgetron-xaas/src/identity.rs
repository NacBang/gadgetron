//! Identity operations — user CRUD helpers (ISSUE 14 TASK 14.3).
//!
//! Typed `sqlx::query_as` wrappers around the `users` table that the
//! gateway's admin handlers (and the TASK 14.7 CLI) both call. Kept
//! in `gadgetron-xaas` so the gateway's HTTP layer can stay dumb: it
//! parses + validates, delegates all DB access here.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::bootstrap::{argon2_hash, BootstrapError, DEFAULT_TENANT_ID};

/// User role. Wire-frozen — the `users.role` CHECK constraint matches
/// these exact strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Member,
    Admin,
    Service,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
            Self::Service => "service",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "member" => Some(Self::Member),
            "admin" => Some(Self::Admin),
            "service" => Some(Self::Service),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("email already exists in this tenant")]
    DuplicateEmail,
    #[error("single-admin guard: cannot demote or delete the last active admin")]
    LastAdmin,
    #[error("user not found")]
    NotFound,
    #[error("service role users cannot have passwords")]
    ServiceWithPassword,
    #[error(transparent)]
    Hash(#[from] BootstrapError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn list_users(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<Vec<UserRow>, IdentityError> {
    let rows = sqlx::query_as::<_, UserRow>(
        r#"
        SELECT id, tenant_id, email, display_name, role, is_active,
               created_at, updated_at, last_login_at
        FROM users
        WHERE tenant_id = $1
        ORDER BY created_at ASC
        LIMIT $2
        "#,
    )
    .bind(tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn create_user(
    pool: &PgPool,
    tenant_id: Uuid,
    email: &str,
    display_name: &str,
    role: Role,
    password: Option<&str>,
) -> Result<UserRow, IdentityError> {
    if role == Role::Service && password.is_some() {
        return Err(IdentityError::ServiceWithPassword);
    }

    let password_hash = match password {
        Some(p) => Some(argon2_hash(p)?),
        None => None,
    };

    let result = sqlx::query_as::<_, UserRow>(
        r#"
        INSERT INTO users (tenant_id, email, display_name, role, password_hash)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, email, display_name, role, is_active,
                  created_at, updated_at, last_login_at
        "#,
    )
    .bind(tenant_id)
    .bind(email)
    .bind(display_name)
    .bind(role.as_str())
    .bind(password_hash.as_deref())
    .fetch_one(pool)
    .await;

    match result {
        Ok(row) => Ok(row),
        Err(sqlx::Error::Database(db_err))
            if db_err.constraint() == Some("users_tenant_id_email_key") =>
        {
            Err(IdentityError::DuplicateEmail)
        }
        Err(e) => Err(IdentityError::Db(e)),
    }
}

/// Delete a user. Applies the single-admin guard: refuses to delete
/// the last active admin in the tenant (spec §7 Q-1).
pub async fn delete_user(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), IdentityError> {
    let mut tx = pool.begin().await?;

    let target: Option<UserRow> = sqlx::query_as::<_, UserRow>(
        r#"SELECT id, tenant_id, email, display_name, role, is_active,
                  created_at, updated_at, last_login_at
           FROM users WHERE id = $1 AND tenant_id = $2"#,
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?;

    let target = target.ok_or(IdentityError::NotFound)?;

    if target.role == "admin" && target.is_active {
        let active_admins: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM users
               WHERE tenant_id = $1 AND role = 'admin' AND is_active = TRUE"#,
        )
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await?;
        if active_admins <= 1 {
            return Err(IdentityError::LastAdmin);
        }
    }

    let rows = sqlx::query("DELETE FROM users WHERE id = $1 AND tenant_id = $2")
        .bind(user_id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;

    if rows.rows_affected() == 0 {
        return Err(IdentityError::NotFound);
    }

    tx.commit().await?;
    Ok(())
}

/// Backfill `api_keys.user_id = <first admin in the default tenant>`
/// for rows that predate ISSUE 14. Idempotent — rows already carrying
/// a user_id are untouched. No-op when no admin exists.
pub async fn backfill_api_keys_user_id(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let admin_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM users
           WHERE tenant_id = $1 AND role = 'admin' AND is_active = TRUE
           ORDER BY created_at ASC LIMIT 1"#,
    )
    .bind(DEFAULT_TENANT_ID)
    .fetch_optional(pool)
    .await?;

    let Some(admin_id) = admin_id else {
        return Ok(0);
    };

    let result = sqlx::query(r#"UPDATE api_keys SET user_id = $1 WHERE user_id IS NULL"#)
        .bind(admin_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_strings_are_wire_frozen() {
        assert_eq!(Role::Member.as_str(), "member");
        assert_eq!(Role::Admin.as_str(), "admin");
        assert_eq!(Role::Service.as_str(), "service");
    }

    #[test]
    fn role_parse_roundtrips() {
        assert_eq!(Role::parse("member"), Some(Role::Member));
        assert_eq!(Role::parse("admin"), Some(Role::Admin));
        assert_eq!(Role::parse("service"), Some(Role::Service));
        assert_eq!(Role::parse("root"), None);
    }
}
