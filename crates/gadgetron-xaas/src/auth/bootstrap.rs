//! First-admin bootstrap flow (ISSUE 14 TASK 14.2).
//!
//! When the `users` table is empty at serve startup and
//! `[auth.bootstrap]` is present in `gadgetron.toml`, create the first
//! admin row from the config + env-var password. If the table is
//! non-empty, the config is ignored with a warning (operators are
//! expected to remove the block post-bootstrap). If the table is empty
//! AND no config is present, startup fails loudly — this is the only
//! designed path to a populated auth surface.
//!
//! Spec: `docs/design/phase2/08-identity-and-users.md` §2.4.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use gadgetron_core::config::BootstrapConfig;
use sqlx::PgPool;
use uuid::Uuid;

pub const DEFAULT_TENANT_ID: Uuid = Uuid::from_u128(0x00000000_0000_0000_0000_000000000001);

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("users table is empty but [auth.bootstrap] is missing. Set [auth.bootstrap] in gadgetron.toml to create the first admin.")]
    MissingConfig,
    #[error("env var {0} is not set — required for bootstrap admin password")]
    MissingPasswordEnv(String),
    #[error("database error during bootstrap: {0}")]
    Db(#[from] sqlx::Error),
    #[error("argon2 hashing failed: {0}")]
    Hash(String),
}

/// Hash a plaintext password with argon2id default parameters.
pub fn argon2_hash(password: &str) -> Result<String, BootstrapError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| BootstrapError::Hash(e.to_string()))
}

/// Verify a plaintext password against a stored argon2id hash.
/// Used by TASK 14.6 login; lives here to keep hash logic in one place.
pub fn argon2_verify(password: &str, stored_hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Run the bootstrap check at serve startup. Returns `Ok(())` whether
/// a bootstrap happened or not; only fails if users is empty AND the
/// config/env is incomplete (hard startup failure).
pub async fn bootstrap_admin_if_needed(
    pool: &PgPool,
    config: Option<&BootstrapConfig>,
) -> Result<(), BootstrapError> {
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;

    if user_count > 0 {
        if config.is_some() {
            tracing::warn!(
                target: "auth.bootstrap",
                "[auth.bootstrap] is configured but users table is not empty — \
                 ignoring bootstrap config. Remove [auth.bootstrap] from gadgetron.toml."
            );
        }
        return Ok(());
    }

    let Some(bootstrap) = config else {
        return Err(BootstrapError::MissingConfig);
    };

    let password = std::env::var(&bootstrap.admin_password_env)
        .map_err(|_| BootstrapError::MissingPasswordEnv(bootstrap.admin_password_env.clone()))?;

    let password_hash = argon2_hash(&password)?;

    // Ensure the hardcoded default tenant row exists before the admin
    // user INSERT — users.tenant_id FKs to tenants.id with default =
    // this UUID, so the row must be there. Upsert is safe: if the row
    // already exists (re-runs, parallel bootstrap), no-op.
    sqlx::query(
        "INSERT INTO tenants (id, name)
         VALUES ($1, 'default')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(DEFAULT_TENANT_ID)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO users (tenant_id, email, display_name, role, password_hash)
         VALUES ($1, $2, $3, 'admin', $4)",
    )
    .bind(DEFAULT_TENANT_ID)
    .bind(&bootstrap.admin_email)
    .bind(&bootstrap.admin_display_name)
    .bind(&password_hash)
    .execute(pool)
    .await?;

    tracing::info!(
        target: "auth.bootstrap",
        email = %bootstrap.admin_email,
        "bootstrap admin created — remove [auth.bootstrap] from gadgetron.toml on next deploy"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2_roundtrip() {
        let hash = argon2_hash("correct horse battery staple").unwrap();
        assert!(argon2_verify("correct horse battery staple", &hash));
        assert!(!argon2_verify("wrong password", &hash));
    }

    #[test]
    fn argon2_hash_is_pbkdf_format() {
        let hash = argon2_hash("anything").unwrap();
        // argon2id PHC-format strings start with the algorithm identifier.
        assert!(hash.starts_with("$argon2"));
    }

    #[test]
    fn argon2_verify_rejects_malformed_hash() {
        assert!(!argon2_verify("pw", "not-a-hash"));
    }

    #[test]
    fn default_tenant_id_is_stable() {
        assert_eq!(
            DEFAULT_TENANT_ID.to_string(),
            "00000000-0000-0000-0000-000000000001"
        );
    }
}
