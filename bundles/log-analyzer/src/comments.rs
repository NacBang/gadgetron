//! Comments attached to a `log_findings` row. Tenants share the thread;
//! authorship is split between real users and Penny (the agent itself).
//!
//! Delete authorization is NOT enforced here — callers pass the actor's
//! identity + admin flag and the handler decides. This keeps the store
//! dumb and the policy surface in one place.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum CommentError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("body empty or too long")]
    BadBody,
}

pub const MAX_BODY_LEN: usize = 4000;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Comment {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub finding_id: Uuid,
    pub author_kind: String,
    pub author_user_id: Option<Uuid>,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub enum Author {
    User(Uuid),
    Penny,
}

fn validate_body(body: &str) -> Result<&str, CommentError> {
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_BODY_LEN {
        return Err(CommentError::BadBody);
    }
    Ok(trimmed)
}

pub async fn add(
    pool: &PgPool,
    tenant_id: Uuid,
    finding_id: Uuid,
    author: Author,
    body: &str,
) -> Result<Comment, CommentError> {
    let body = validate_body(body)?;
    // Guard against cross-tenant insertion by joining the finding check
    // into the insert (CTE → returns NULL if the finding doesn't belong
    // to this tenant, which trips the NOT NULL on finding_id).
    let (kind, user_id) = match author {
        Author::User(uid) => ("user", Some(uid)),
        Author::Penny => ("penny", None),
    };
    let row: Option<Comment> = sqlx::query_as(
        "WITH f AS ( \
             SELECT id FROM log_findings \
             WHERE id = $2 AND tenant_id = $1 \
         ) \
         INSERT INTO log_finding_comments \
             (tenant_id, finding_id, author_kind, author_user_id, body) \
         SELECT $1, f.id, $3, $4, $5 FROM f \
         RETURNING id, tenant_id, finding_id, author_kind, author_user_id, \
                   body, created_at",
    )
    .bind(tenant_id)
    .bind(finding_id)
    .bind(kind)
    .bind(user_id)
    .bind(body)
    .fetch_optional(pool)
    .await?;
    row.ok_or(CommentError::NotFound)
}

pub async fn list(
    pool: &PgPool,
    tenant_id: Uuid,
    finding_id: Uuid,
) -> Result<Vec<Comment>, CommentError> {
    let rows: Vec<Comment> = sqlx::query_as(
        "SELECT id, tenant_id, finding_id, author_kind, author_user_id, \
                body, created_at \
         FROM log_finding_comments \
         WHERE tenant_id = $1 AND finding_id = $2 \
         ORDER BY created_at ASC",
    )
    .bind(tenant_id)
    .bind(finding_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Delete one comment. Returns `Forbidden` when the actor is neither the
/// author nor an admin. Penny-authored comments (`author_user_id IS NULL`)
/// are admin-only.
pub async fn delete(
    pool: &PgPool,
    tenant_id: Uuid,
    comment_id: Uuid,
    actor_user_id: Uuid,
    actor_is_admin: bool,
) -> Result<(), CommentError> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT author_user_id FROM log_finding_comments \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(comment_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;
    let author = row.ok_or(CommentError::NotFound)?.0;
    let can_delete = actor_is_admin || author == Some(actor_user_id);
    if !can_delete {
        return Err(CommentError::Forbidden);
    }
    sqlx::query("DELETE FROM log_finding_comments WHERE id = $1 AND tenant_id = $2")
        .bind(comment_id)
        .bind(tenant_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Aggregated counts per finding — powers the "💬 3" badge on the card
/// without fetching every thread eagerly.
pub async fn counts_by_finding(
    pool: &PgPool,
    tenant_id: Uuid,
    finding_ids: &[Uuid],
) -> Result<Vec<(Uuid, i64)>, CommentError> {
    if finding_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT finding_id, COUNT(*) \
         FROM log_finding_comments \
         WHERE tenant_id = $1 AND finding_id = ANY($2) \
         GROUP BY finding_id",
    )
    .bind(tenant_id)
    .bind(finding_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
