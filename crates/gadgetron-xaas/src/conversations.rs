//! Per-user conversation metadata store (ISSUE 31).
//!
//! Supports the left-rail chat sidebar: list, rename, soft-delete.
//! Rows here are the authoritative source for "which chats does user X
//! own?"; Claude Code's jsonl files at `~/.gadgetron/penny/work/` are
//! the actual message history and are referenced by
//! `claude_session_uuid`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConversationRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub claude_session_uuid: Option<Uuid>,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConversationError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("not found")]
    NotFound,
}

/// Insert a new conversation row. The caller supplies the id (so the
/// frontend can mint its own UUID client-side and avoid an extra
/// round-trip on the first message); subsequent `upsert_turn` calls
/// with the same id update the title and touch `updated_at`.
pub async fn create_conversation(
    pool: &PgPool,
    id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
    title: &str,
) -> Result<(), ConversationError> {
    sqlx::query(
        "INSERT INTO conversations (id, tenant_id, user_id, title) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(title)
    .execute(pool)
    .await?;
    Ok(())
}

/// Called on every chat turn. Ensures a row exists for this
/// (user, conversation_id) pair and updates `title` on the first turn
/// (when the existing title is still the default "New chat") using a
/// truncated form of the first user message.
pub async fn upsert_turn(
    pool: &PgPool,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
    claude_session_uuid: Option<Uuid>,
    first_user_message: &str,
) -> Result<(), ConversationError> {
    let title_candidate = summarize_message(first_user_message);
    // Insert if missing, otherwise touch updated_at and claude_session_uuid.
    // The title is only overwritten if it still equals the default seed
    // "New chat" — once a real title exists, subsequent turns don't
    // clobber it.
    sqlx::query(
        "INSERT INTO conversations (id, tenant_id, user_id, title, claude_session_uuid) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (id) DO UPDATE SET \
            updated_at = now(), \
            title = CASE WHEN conversations.title = 'New chat' AND LENGTH(EXCLUDED.title) > 0 \
                         THEN EXCLUDED.title ELSE conversations.title END, \
            claude_session_uuid = COALESCE(EXCLUDED.claude_session_uuid, conversations.claude_session_uuid)",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(&title_candidate)
    .bind(claude_session_uuid)
    .execute(pool)
    .await?;
    Ok(())
}

/// List the user's non-deleted conversations, newest first.
pub async fn list_conversations_for_user(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<ConversationRow>, ConversationError> {
    let rows: Vec<ConversationRow> = sqlx::query_as(
        "SELECT id, tenant_id, user_id, claude_session_uuid, title, created_at, updated_at \
         FROM conversations \
         WHERE tenant_id = $1 AND user_id = $2 AND deleted_at IS NULL \
         ORDER BY updated_at DESC \
         LIMIT $3",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Soft-delete a conversation. Only the owner can remove their own
/// rows; the caller passes `user_id` which the WHERE clause enforces.
pub async fn delete_conversation(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    id: Uuid,
) -> Result<(), ConversationError> {
    let affected = sqlx::query(
        "UPDATE conversations SET deleted_at = now() \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(ConversationError::NotFound);
    }
    Ok(())
}

/// Rename a conversation.
pub async fn rename_conversation(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    id: Uuid,
    new_title: &str,
) -> Result<(), ConversationError> {
    let clean = new_title.trim();
    let title = if clean.is_empty() {
        "New chat"
    } else {
        clean
    };
    let affected = sqlx::query(
        "UPDATE conversations SET title = $4, updated_at = now() \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(title)
    .execute(pool)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(ConversationError::NotFound);
    }
    Ok(())
}

fn summarize_message(s: &str) -> String {
    const MAX: usize = 80;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return "New chat".into();
    }
    // First line, capped at MAX characters (bytes, approximately chars for
    // most Korean/ASCII mixed text since we respect char boundaries below).
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    let mut out = String::new();
    for (i, ch) in first_line.chars().enumerate() {
        if i >= MAX {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_handles_empty_and_long() {
        assert_eq!(summarize_message(""), "New chat");
        assert_eq!(summarize_message("   "), "New chat");
        let long = "a".repeat(200);
        let s = summarize_message(&long);
        assert!(s.chars().count() <= 81); // 80 + ellipsis
        assert!(s.ends_with('…'));
    }

    #[test]
    fn summary_takes_first_line_only() {
        assert_eq!(summarize_message("hello\nworld"), "hello");
    }
}
