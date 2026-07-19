//! Idempotent default Team Space provisioning for human users.
//!
//! The database half of onboarding belongs in XaaS because every account
//! creation path terminates here. Canonical guide-note bytes stay outside
//! this crate and are reconciled by the gateway through the Domain Vault
//! write path.

use std::collections::HashSet;

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::bootstrap::DEFAULT_TENANT_ID;

pub const DEFAULT_TEAM_DISPLAY_NAME: &str = "Operations";
pub const DEFAULT_TEAM_SPACE_TITLE: &str = "Operations";
pub const DEFAULT_TEAM_DESCRIPTION: &str = "Shared operational knowledge for the team";
pub const DEFAULT_TEAM_HOME_BUNDLE_ID: &str = "core";
pub const DEFAULT_TEAM_GUIDE_TITLE: &str = "What this Space is for";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultTeamOnboarding {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub service_actor_id: Uuid,
    pub team_id: String,
    pub space_id: Uuid,
    pub vault_id: Uuid,
}

/// Ensure the default Team Space topology for one human user.
///
/// Service identities deliberately remain outside human onboarding. The
/// function is safe to call after every login/upsert as well as at startup.
pub async fn ensure_default_team_onboarding(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<DefaultTeamOnboarding>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let result = ensure_default_team_onboarding_in_transaction(&mut tx, tenant_id, user_id).await?;
    tx.commit().await?;
    Ok(result)
}

/// Reconcile pre-existing human users during service startup. One topology is
/// returned per tenant so the gateway can seed exactly one canonical guide.
pub async fn ensure_existing_default_team_onboarding(
    pool: &PgPool,
) -> Result<Vec<DefaultTeamOnboarding>, sqlx::Error> {
    let users: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"SELECT tenant_id, id
           FROM users
           WHERE is_active = TRUE AND role <> 'service'
           ORDER BY tenant_id, created_at, id"#,
    )
    .fetch_all(pool)
    .await?;
    let mut seen_tenants = HashSet::new();
    let mut topologies = Vec::new();
    for (tenant_id, user_id) in users {
        if let Some(topology) = ensure_default_team_onboarding(pool, tenant_id, user_id).await? {
            if seen_tenants.insert(tenant_id) {
                topologies.push(topology);
            }
        }
    }
    Ok(topologies)
}

pub(crate) async fn ensure_default_team_onboarding_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<DefaultTeamOnboarding>, sqlx::Error> {
    let user: Option<(String, bool)> =
        sqlx::query_as("SELECT role, is_active FROM users WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((role, is_active)) = user else {
        return Err(sqlx::Error::RowNotFound);
    };
    if !is_active {
        return Err(sqlx::Error::Protocol(
            "default Team onboarding requires an active user".to_string(),
        ));
    }
    if role == "service" {
        return Ok(None);
    }

    let service_actor_id =
        match crate::service_principals::ensure_knowledge_agent_in_transaction(tx, tenant_id).await
        {
            Ok(principal) => principal.user_id,
            Err(crate::service_principals::ServicePrincipalError::Database(error)) => {
                return Err(error)
            }
            Err(error) => return Err(sqlx::Error::Protocol(error.to_string())),
        };
    let team_id = default_team_id(tenant_id);
    sqlx::query(
        r#"INSERT INTO teams
           (id, tenant_id, display_name, description, created_by)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(&team_id)
    .bind(tenant_id)
    .bind(DEFAULT_TEAM_DISPLAY_NAME)
    .bind(DEFAULT_TEAM_DESCRIPTION)
    .bind(service_actor_id)
    .execute(&mut **tx)
    .await?;
    let team_tenant: Uuid = sqlx::query_scalar("SELECT tenant_id FROM teams WHERE id = $1")
        .bind(&team_id)
        .fetch_one(&mut **tx)
        .await?;
    if team_tenant != tenant_id {
        return Err(sqlx::Error::Protocol(format!(
            "default Team id {team_id} belongs to another tenant"
        )));
    }
    sqlx::query(
        r#"UPDATE teams
           SET display_name = $2, description = $3
           WHERE id = $1 AND tenant_id = $4
             AND (display_name IS DISTINCT FROM $2 OR description IS DISTINCT FROM $3)"#,
    )
    .bind(&team_id)
    .bind(DEFAULT_TEAM_DISPLAY_NAME)
    .bind(DEFAULT_TEAM_DESCRIPTION)
    .bind(tenant_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO team_members (team_id, user_id, role, added_by)
           VALUES ($1, $2, 'lead', $2)
           ON CONFLICT (team_id, user_id) DO UPDATE SET role = 'lead'"#,
    )
    .bind(&team_id)
    .bind(service_actor_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO team_members (team_id, user_id, role, added_by)
           VALUES ($1, $2, 'member', $3)
           ON CONFLICT (team_id, user_id) DO UPDATE SET
             role = CASE WHEN team_members.role = 'lead' THEN 'lead' ELSE 'member' END"#,
    )
    .bind(&team_id)
    .bind(user_id)
    .bind(service_actor_id)
    .execute(&mut **tx)
    .await?;

    let space_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO knowledge_spaces
           (tenant_id, kind, title, owner_team_id)
           VALUES ($1, 'team', $2, $3)
           ON CONFLICT (tenant_id, owner_team_id) WHERE kind = 'team' DO UPDATE SET
             title = EXCLUDED.title,
             revision = CASE WHEN knowledge_spaces.title IS DISTINCT FROM EXCLUDED.title
               THEN knowledge_spaces.revision + 1 ELSE knowledge_spaces.revision END,
             updated_at = CASE WHEN knowledge_spaces.title IS DISTINCT FROM EXCLUDED.title
               THEN NOW() ELSE knowledge_spaces.updated_at END
           RETURNING id"#,
    )
    .bind(tenant_id)
    .bind(DEFAULT_TEAM_SPACE_TITLE)
    .bind(&team_id)
    .fetch_one(&mut **tx)
    .await?;
    let vault_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO knowledge_vaults
           (tenant_id, space_id, home_bundle_id, knowledge_schema_id, schema_version)
           VALUES ($1, $2, $3, 'core.knowledge', 1)
           ON CONFLICT (tenant_id, space_id, home_bundle_id) DO UPDATE SET
             knowledge_schema_id = knowledge_vaults.knowledge_schema_id
           RETURNING id"#,
    )
    .bind(tenant_id)
    .bind(space_id)
    .bind(DEFAULT_TEAM_HOME_BUNDLE_ID)
    .fetch_one(&mut **tx)
    .await?;

    Ok(Some(DefaultTeamOnboarding {
        tenant_id,
        user_id,
        service_actor_id,
        team_id,
        space_id,
        vault_id,
    }))
}

fn default_team_id(tenant_id: Uuid) -> String {
    if tenant_id == DEFAULT_TENANT_ID {
        return "operations".to_string();
    }
    let tenant = tenant_id.simple().to_string();
    format!("operations-{}", &tenant[..21])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_id_is_readable_for_default_and_tenant_unique_elsewhere() {
        assert_eq!(default_team_id(DEFAULT_TENANT_ID), "operations");
        let tenant = Uuid::parse_str("12345678-1234-5678-9012-345678901234").unwrap();
        assert_eq!(default_team_id(tenant), "operations-123456781234567890123");
        assert_eq!(default_team_id(tenant).len(), 32);
    }
}
