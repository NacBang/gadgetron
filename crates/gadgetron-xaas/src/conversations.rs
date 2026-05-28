//! Per-user conversation metadata store.
//!
//! Supports the left-rail chat sidebar: list, rename, soft-delete.
//! Rows here are the authoritative source for "which chats does user X
//! own?". `conversation_messages` is the runtime-neutral transcript;
//! `conversation_agent_sessions` maps the same Gadgetron conversation to
//! backend-native resume ids for Claude Code, Codex, and future compatible
//! agent backends.

use chrono::{DateTime, Utc};
use gadgetron_core::agent::AgentBackend;
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

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConversationMessageRow {
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConversationError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("not found")]
    NotFound,
    /// The supplied `(tenant_id, user_id)` does not match the existing
    /// conversation row's owner. The caller is attempting to write
    /// turns into another principal's conversation — refuse loudly.
    #[error("conversation owned by a different principal")]
    OwnershipMismatch,
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
/// (user, conversation_id) pair, bumps `turn_count`, and seeds `title`
/// on the very first turn (when the existing title is still the
/// default "New chat") using a truncated form of the first user
/// message. Later turns don't touch the title — the Penny-powered
/// summarizer in `summarize_title_if_due` rolls it forward.
///
/// Returns `(turn_count, summary_turn_at)` so the caller can decide
/// whether to kick off an async summary refresh.
pub async fn upsert_turn(
    pool: &PgPool,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
    claude_session_uuid: Option<Uuid>,
    first_user_message: &str,
) -> Result<(i32, i32), ConversationError> {
    let title_candidate = summarize_message(first_user_message);
    // The `WHERE conversations.tenant_id = $2 AND conversations.user_id = $3`
    // predicate on the ON CONFLICT branch is the security gate: a
    // caller who guesses (or leaks) another principal's conversation
    // UUID cannot append turns to it. When the predicate fails the
    // UPDATE matches no row, RETURNING produces no row, and `fetch_optional`
    // hands back `None`, which we map to `OwnershipMismatch`. The
    // INSERT path (no row exists yet) still succeeds — first-write
    // wins, and from there the predicate locks the owner.
    let row: Option<(i32, i32)> = sqlx::query_as(
        "INSERT INTO conversations \
             (id, tenant_id, user_id, title, claude_session_uuid, turn_count, summary_turn_at) \
         VALUES ($1, $2, $3, $4, $5, 1, 0) \
         ON CONFLICT (id) DO UPDATE SET \
            updated_at = now(), \
            title = CASE WHEN conversations.title = 'New chat' AND LENGTH(EXCLUDED.title) > 0 \
                         THEN EXCLUDED.title ELSE conversations.title END, \
            claude_session_uuid = COALESCE(EXCLUDED.claude_session_uuid, conversations.claude_session_uuid), \
            turn_count = conversations.turn_count + 1 \
         WHERE conversations.tenant_id = $2 AND conversations.user_id = $3 \
         RETURNING turn_count, summary_turn_at",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(&title_candidate)
    .bind(claude_session_uuid)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ConversationError::OwnershipMismatch)
}

/// Append one user-visible chat message to the runtime-neutral transcript.
///
/// The caller must already have passed the `conversations` ownership gate.
/// This insert still carries `(tenant_id, user_id)` so reads can enforce the
/// same boundary without trusting only `conversation_id`.
pub async fn append_message(
    pool: &PgPool,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
    role: &str,
    content: &str,
) -> Result<(), ConversationError> {
    let clean = content.trim();
    if clean.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO conversation_messages \
             (conversation_id, tenant_id, user_id, role, content) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(role)
    .bind(clean)
    .execute(pool)
    .await?;
    Ok(())
}

/// Load the DB-backed transcript for a conversation owned by the caller.
pub async fn list_messages(
    pool: &PgPool,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<ConversationMessageRow>, ConversationError> {
    let rows = sqlx::query_as::<_, ConversationMessageRow>(
        "SELECT role, content, created_at \
         FROM conversation_messages \
         WHERE conversation_id = $1 AND tenant_id = $2 AND user_id = $3 \
         ORDER BY id ASC",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Load the durable backend-native session id for a Gadgetron conversation.
pub async fn get_agent_backend_session_id(
    pool: &PgPool,
    conversation_id: Uuid,
    backend: AgentBackend,
) -> Result<Option<String>, ConversationError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT backend_session_id \
         FROM conversation_agent_sessions \
         WHERE conversation_id = $1 AND backend = $2",
    )
    .bind(conversation_id)
    .bind(backend.as_str())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(session_id,)| session_id))
}

/// Persist the backend-native resume id after a successful Penny turn.
///
/// Ownership columns are copied from `conversations`; this keeps the mapping
/// consistent with the sidebar owner and avoids trusting the agent subprocess
/// path with tenant/user identity.
pub async fn upsert_agent_backend_session_id(
    pool: &PgPool,
    conversation_id: Uuid,
    backend: AgentBackend,
    backend_session_id: &str,
) -> Result<(), ConversationError> {
    let clean = backend_session_id.trim();
    if clean.is_empty() {
        return Ok(());
    }
    let result = sqlx::query(
        "INSERT INTO conversation_agent_sessions \
             (conversation_id, tenant_id, user_id, backend, backend_session_id) \
         SELECT id, tenant_id, user_id, $2, $3 \
         FROM conversations \
         WHERE id = $1 \
         ON CONFLICT (conversation_id, backend) DO UPDATE SET \
            backend_session_id = EXCLUDED.backend_session_id, \
            tenant_id = EXCLUDED.tenant_id, \
            user_id = EXCLUDED.user_id, \
            updated_at = now()",
    )
    .bind(conversation_id)
    .bind(backend.as_str())
    .bind(clean)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ConversationError::NotFound);
    }
    Ok(())
}

/// Apply a Penny-generated summary as the conversation's new title.
/// Writes both the title and a marker (`summary_turn_at = turn_count`
/// at the moment of submission) so the next regen trigger knows how
/// much the conversation has grown since.
pub async fn set_rolling_summary(
    pool: &PgPool,
    conversation_id: Uuid,
    new_title: &str,
    turn_count_at_summary: i32,
) -> Result<(), ConversationError> {
    let clean = new_title.trim();
    if clean.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE conversations \
         SET title = $2, summary_turn_at = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(conversation_id)
    .bind(clean)
    .bind(turn_count_at_summary)
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
    let title = if clean.is_empty() { "New chat" } else { clean };
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
