//! Group + user_groups CRUD.
//!
//! Groups are a tenant-scoped, kebab-case-keyed access-permission
//! bucket. Unlike `teams` (collaboration unit with per-membership
//! role member|lead), `user_groups` is a flat membership join — a
//! user is either in a group or not. Permission policies (scopes,
//! resource ACLs) attached to a group are a Phase 2 concern; this
//! module is just CRUD over membership.
//!
//! Tenant boundary is enforced by every query — group_id alone is
//! never trusted; callers must pass tenant_id.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct GroupRow {
    pub id: String,
    pub tenant_id: Uuid,
    pub display_name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
}

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct UserGroupRow {
    pub group_id: String,
    pub user_id: Uuid,
    pub added_at: DateTime<Utc>,
    pub added_by: Option<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum GroupError {
    #[error("group id violates format (kebab-case, ≤32 chars)")]
    InvalidId,
    #[error("group id already exists in this tenant")]
    Duplicate,
    #[error("group not found")]
    NotFound,
    #[error("user not found in this tenant (cannot add to group)")]
    UserNotFound,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn list_groups(pool: &PgPool, tenant_id: Uuid) -> Result<Vec<GroupRow>, GroupError> {
    let rows = sqlx::query_as::<_, GroupRow>(
        r#"
        SELECT id, tenant_id, display_name, description, created_at, created_by
        FROM groups
        WHERE tenant_id = $1
        ORDER BY id ASC
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn create_group(
    pool: &PgPool,
    tenant_id: Uuid,
    id: &str,
    display_name: &str,
    description: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<GroupRow, GroupError> {
    let result = sqlx::query_as::<_, GroupRow>(
        r#"
        INSERT INTO groups (id, tenant_id, display_name, description, created_by)
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
            // 23505 = unique_violation, 23514 = check_violation.
            if code.as_deref() == Some("23505") {
                Err(GroupError::Duplicate)
            } else if code.as_deref() == Some("23514") {
                Err(GroupError::InvalidId)
            } else {
                Err(GroupError::Db(sqlx::Error::Database(db_err)))
            }
        }
        Err(e) => Err(GroupError::Db(e)),
    }
}

pub async fn delete_group(pool: &PgPool, tenant_id: Uuid, id: &str) -> Result<(), GroupError> {
    let rows = sqlx::query("DELETE FROM groups WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(tenant_id)
        .execute(pool)
        .await?;
    if rows.rows_affected() == 0 {
        return Err(GroupError::NotFound);
    }
    Ok(())
}

pub async fn list_group_members(
    pool: &PgPool,
    tenant_id: Uuid,
    group_id: &str,
) -> Result<Vec<UserGroupRow>, GroupError> {
    let group_exists: Option<String> =
        sqlx::query_scalar(r#"SELECT id FROM groups WHERE id = $1 AND tenant_id = $2"#)
            .bind(group_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await?;
    if group_exists.is_none() {
        return Err(GroupError::NotFound);
    }
    let rows = sqlx::query_as::<_, UserGroupRow>(
        r#"
        SELECT ug.group_id, ug.user_id, ug.added_at, ug.added_by
        FROM user_groups ug
        JOIN users u ON u.id = ug.user_id
        WHERE ug.group_id = $1 AND u.tenant_id = $2
        ORDER BY ug.added_at ASC
        "#,
    )
    .bind(group_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_groups_for_user(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<GroupRow>, GroupError> {
    let user_tenant: Option<Uuid> =
        sqlx::query_scalar(r#"SELECT tenant_id FROM users WHERE id = $1"#)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if user_tenant != Some(tenant_id) {
        return Err(GroupError::UserNotFound);
    }
    let rows = sqlx::query_as::<_, GroupRow>(
        r#"
        SELECT g.id, g.tenant_id, g.display_name, g.description, g.created_at, g.created_by
        FROM groups g
        JOIN user_groups ug ON ug.group_id = g.id
        WHERE ug.user_id = $1 AND g.tenant_id = $2
        ORDER BY g.id ASC
        "#,
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_user_to_group(
    pool: &PgPool,
    tenant_id: Uuid,
    group_id: &str,
    user_id: Uuid,
    added_by: Option<Uuid>,
) -> Result<UserGroupRow, GroupError> {
    let user_tenant: Option<Uuid> =
        sqlx::query_scalar(r#"SELECT tenant_id FROM users WHERE id = $1"#)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if user_tenant != Some(tenant_id) {
        return Err(GroupError::UserNotFound);
    }
    let group_exists: Option<String> =
        sqlx::query_scalar(r#"SELECT id FROM groups WHERE id = $1 AND tenant_id = $2"#)
            .bind(group_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await?;
    if group_exists.is_none() {
        return Err(GroupError::NotFound);
    }

    let result = sqlx::query_as::<_, UserGroupRow>(
        r#"
        INSERT INTO user_groups (group_id, user_id, added_by)
        VALUES ($1, $2, $3)
        ON CONFLICT (group_id, user_id) DO UPDATE
            SET added_at = NOW(), added_by = EXCLUDED.added_by
        RETURNING group_id, user_id, added_at, added_by
        "#,
    )
    .bind(group_id)
    .bind(user_id)
    .bind(added_by)
    .fetch_one(pool)
    .await?;
    Ok(result)
}

pub async fn remove_user_from_group(
    pool: &PgPool,
    tenant_id: Uuid,
    group_id: &str,
    user_id: Uuid,
) -> Result<(), GroupError> {
    let group_exists: Option<String> =
        sqlx::query_scalar(r#"SELECT id FROM groups WHERE id = $1 AND tenant_id = $2"#)
            .bind(group_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await?;
    if group_exists.is_none() {
        return Err(GroupError::NotFound);
    }
    let rows = sqlx::query(r#"DELETE FROM user_groups WHERE group_id = $1 AND user_id = $2"#)
        .bind(group_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if rows.rows_affected() == 0 {
        return Err(GroupError::UserNotFound);
    }
    Ok(())
}

/// Replace the full set of groups a user belongs to. Computes the
/// add/remove delta inside a single transaction so the user never
/// observes a partial state. `group_ids` is validated up-front: every
/// id must exist in the caller's tenant, otherwise the whole sync
/// fails with `NotFound` and nothing is mutated.
pub async fn sync_user_groups(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    group_ids: &[String],
    added_by: Option<Uuid>,
) -> Result<Vec<UserGroupRow>, GroupError> {
    let user_tenant: Option<Uuid> =
        sqlx::query_scalar(r#"SELECT tenant_id FROM users WHERE id = $1"#)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if user_tenant != Some(tenant_id) {
        return Err(GroupError::UserNotFound);
    }

    let mut tx = pool.begin().await?;

    // Validate every requested group lives in this tenant before any mutation.
    for gid in group_ids {
        let exists: Option<String> =
            sqlx::query_scalar(r#"SELECT id FROM groups WHERE id = $1 AND tenant_id = $2"#)
                .bind(gid)
                .bind(tenant_id)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(GroupError::NotFound);
        }
    }

    // Remove memberships not in the target set.
    if group_ids.is_empty() {
        sqlx::query(r#"DELETE FROM user_groups WHERE user_id = $1"#)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query(
            r#"DELETE FROM user_groups
               WHERE user_id = $1 AND group_id <> ALL($2)"#,
        )
        .bind(user_id)
        .bind(group_ids)
        .execute(&mut *tx)
        .await?;
    }

    // Upsert the requested set.
    for gid in group_ids {
        sqlx::query(
            r#"INSERT INTO user_groups (group_id, user_id, added_by)
               VALUES ($1, $2, $3)
               ON CONFLICT (group_id, user_id) DO NOTHING"#,
        )
        .bind(gid)
        .bind(user_id)
        .bind(added_by)
        .execute(&mut *tx)
        .await?;
    }

    let rows = sqlx::query_as::<_, UserGroupRow>(
        r#"
        SELECT group_id, user_id, added_at, added_by
        FROM user_groups
        WHERE user_id = $1
        ORDER BY group_id ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(rows)
}
