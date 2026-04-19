//! Team + team_members CRUD (ISSUE 14 TASK 14.5).
//!
//! Teams are a tenant-scoped grouping of users. ID is kebab-case
//! TEXT ("platform", "ml-ops"); migration CHECK rejects 'admins'
//! (reserved as a virtual team for role='admin' users). team_members
//! is a join table with a per-row role (member | lead).
//!
//! This TASK ships admin-only CRUD under Management scope. Per-team
//! lead-level delegation (spec §2.6.2 "admin 또는 lead") is deferred —
//! Management can always add/remove, leads follow as an enrichment.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TeamRow {
    pub id: String,
    pub tenant_id: Uuid,
    pub display_name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
}

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TeamMemberRow {
    pub team_id: String,
    pub user_id: Uuid,
    pub role: String,
    pub added_at: DateTime<Utc>,
    pub added_by: Option<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error("team id violates format (kebab-case, ≤32 chars, not 'admins')")]
    InvalidId,
    #[error("team id already exists in this tenant")]
    Duplicate,
    #[error("team not found")]
    NotFound,
    #[error("user not found in this tenant (cannot add to team)")]
    UserNotFound,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn list_teams(pool: &PgPool, tenant_id: Uuid) -> Result<Vec<TeamRow>, TeamError> {
    let rows = sqlx::query_as::<_, TeamRow>(
        r#"
        SELECT id, tenant_id, display_name, description, created_at, created_by
        FROM teams
        WHERE tenant_id = $1
        ORDER BY id ASC
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn create_team(
    pool: &PgPool,
    tenant_id: Uuid,
    id: &str,
    display_name: &str,
    description: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<TeamRow, TeamError> {
    let result = sqlx::query_as::<_, TeamRow>(
        r#"
        INSERT INTO teams (id, tenant_id, display_name, description, created_by)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, display_name, description, created_at, created_by
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(display_name)
    .bind(description)
    .bind(created_by)
    .fetch_one(pool)
    .await;

    match result {
        Ok(row) => Ok(row),
        Err(sqlx::Error::Database(db_err)) => {
            let code = db_err.code();
            let constraint = db_err.constraint();
            // 23505 = unique_violation (DUPLICATE); 23514 = check_violation (InvalidId).
            if code.as_deref() == Some("23505") {
                Err(TeamError::Duplicate)
            } else if code.as_deref() == Some("23514")
                || constraint == Some("teams_id_check")
                || constraint == Some("teams_id_check1")
            {
                Err(TeamError::InvalidId)
            } else {
                Err(TeamError::Db(sqlx::Error::Database(db_err)))
            }
        }
        Err(e) => Err(TeamError::Db(e)),
    }
}

pub async fn delete_team(pool: &PgPool, tenant_id: Uuid, id: &str) -> Result<(), TeamError> {
    let rows = sqlx::query("DELETE FROM teams WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(tenant_id)
        .execute(pool)
        .await?;
    if rows.rows_affected() == 0 {
        return Err(TeamError::NotFound);
    }
    Ok(())
}

pub async fn list_team_members(
    pool: &PgPool,
    tenant_id: Uuid,
    team_id: &str,
) -> Result<Vec<TeamMemberRow>, TeamError> {
    // Confirm team exists in this tenant before leaking membership.
    let team_exists: Option<String> =
        sqlx::query_scalar(r#"SELECT id FROM teams WHERE id = $1 AND tenant_id = $2"#)
            .bind(team_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await?;
    if team_exists.is_none() {
        return Err(TeamError::NotFound);
    }
    let rows = sqlx::query_as::<_, TeamMemberRow>(
        r#"
        SELECT tm.team_id, tm.user_id, tm.role, tm.added_at, tm.added_by
        FROM team_members tm
        JOIN users u ON u.id = tm.user_id
        WHERE tm.team_id = $1 AND u.tenant_id = $2
        ORDER BY tm.added_at ASC
        "#,
    )
    .bind(team_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_team_member(
    pool: &PgPool,
    tenant_id: Uuid,
    team_id: &str,
    user_id: Uuid,
    role: &str,
    added_by: Option<Uuid>,
) -> Result<TeamMemberRow, TeamError> {
    // Verify user + team live in this tenant (tenant-boundary guard).
    let user_tenant: Option<Uuid> =
        sqlx::query_scalar(r#"SELECT tenant_id FROM users WHERE id = $1"#)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if user_tenant != Some(tenant_id) {
        return Err(TeamError::UserNotFound);
    }
    let team_exists: Option<String> =
        sqlx::query_scalar(r#"SELECT id FROM teams WHERE id = $1 AND tenant_id = $2"#)
            .bind(team_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await?;
    if team_exists.is_none() {
        return Err(TeamError::NotFound);
    }

    let result = sqlx::query_as::<_, TeamMemberRow>(
        r#"
        INSERT INTO team_members (team_id, user_id, role, added_by)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (team_id, user_id) DO UPDATE
            SET role = EXCLUDED.role, added_at = NOW(), added_by = EXCLUDED.added_by
        RETURNING team_id, user_id, role, added_at, added_by
        "#,
    )
    .bind(team_id)
    .bind(user_id)
    .bind(role)
    .bind(added_by)
    .fetch_one(pool)
    .await?;
    Ok(result)
}

pub async fn remove_team_member(
    pool: &PgPool,
    tenant_id: Uuid,
    team_id: &str,
    user_id: Uuid,
) -> Result<(), TeamError> {
    // Tenant-boundary via team's tenant_id.
    let team_exists: Option<String> =
        sqlx::query_scalar(r#"SELECT id FROM teams WHERE id = $1 AND tenant_id = $2"#)
            .bind(team_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await?;
    if team_exists.is_none() {
        return Err(TeamError::NotFound);
    }
    let rows = sqlx::query(r#"DELETE FROM team_members WHERE team_id = $1 AND user_id = $2"#)
        .bind(team_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if rows.rows_affected() == 0 {
        return Err(TeamError::UserNotFound);
    }
    Ok(())
}
