//! Per-user conversation metadata store.
//!
//! Supports the left-rail chat sidebar: list, rename, soft-delete.
//! Rows here are the authoritative source for "which chats does user X
//! own?". `conversation_messages` is the runtime-neutral transcript;
//! `conversation_agent_sessions` maps the same Gadgetron conversation to
//! backend-native resume ids for Claude Code, Codex, and future compatible
//! agent backends.

use chrono::{DateTime, Utc};
use gadgetron_core::agent::{
    AgentBackend, AgentEffort, ConversationAgentProfile, ModelSource, AUTO_MODEL_ID,
};
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
    pub agent_backend: Option<String>,
    pub agent_endpoint_id: Option<Uuid>,
    pub agent_model: String,
    pub agent_effort: Option<String>,
    pub agent_model_source: Option<String>,
    pub agent_local_base_url: String,
    pub agent_local_api_key_env: String,
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
    #[error("conversation runtime is pinned to {pinned}; requested {requested}")]
    AgentBackendPinned { pinned: String, requested: String },
    #[error("invalid conversation agent profile: {0}")]
    InvalidAgentProfile(String),
}

#[derive(Debug, sqlx::FromRow)]
struct ConversationAgentProfileDbRow {
    agent_backend: String,
    agent_endpoint_id: Option<Uuid>,
    agent_model: String,
    agent_effort: String,
    agent_model_source: String,
    agent_local_base_url: String,
    agent_local_api_key_env: String,
}

fn model_source_as_str(source: ModelSource) -> &'static str {
    match source {
        ModelSource::Default => "default",
        ModelSource::Local => "local",
    }
}

fn profile_from_db(
    row: ConversationAgentProfileDbRow,
) -> Result<ConversationAgentProfile, ConversationError> {
    let backend = AgentBackend::parse(&row.agent_backend).ok_or_else(|| {
        ConversationError::InvalidAgentProfile(format!("unknown backend {:?}", row.agent_backend))
    })?;
    let effort = AgentEffort::parse(&row.agent_effort).ok_or_else(|| {
        ConversationError::InvalidAgentProfile(format!("unknown effort {:?}", row.agent_effort))
    })?;
    let model_source = match row.agent_model_source.as_str() {
        "default" => ModelSource::Default,
        "local" => ModelSource::Local,
        other => {
            return Err(ConversationError::InvalidAgentProfile(format!(
                "unknown model source {other:?}"
            )))
        }
    };
    let effort = effort.for_backend_model(backend, &row.agent_model);
    Ok(ConversationAgentProfile {
        backend,
        llm_endpoint_id: row.agent_endpoint_id,
        model: row.agent_model,
        effort,
        model_source,
        local_base_url: row.agent_local_base_url,
        local_api_key_env: row.agent_local_api_key_env,
    })
}

fn validate_agent_profile(profile: &ConversationAgentProfile) -> Result<(), ConversationError> {
    let model = profile.model.trim();
    let has_control_line = |value: &str| value.chars().any(|c| matches!(c, '\0' | '\r' | '\n'));
    if model.len() > 256 || model.starts_with('-') || has_control_line(model) {
        return Err(ConversationError::InvalidAgentProfile(
            "model must be at most 256 bytes and contain no leading dash or control lines".into(),
        ));
    }
    if profile.local_base_url.len() > 2048 || has_control_line(&profile.local_base_url) {
        return Err(ConversationError::InvalidAgentProfile(
            "local base URL must be at most 2048 bytes and contain no control lines".into(),
        ));
    }
    if profile.local_api_key_env.len() > 128 || has_control_line(&profile.local_api_key_env) {
        return Err(ConversationError::InvalidAgentProfile(
            "local API key env must be at most 128 bytes and contain no control lines".into(),
        ));
    }
    if matches!(profile.model_source, ModelSource::Local)
        && !(profile.local_base_url.starts_with("http://")
            || profile.local_base_url.starts_with("https://"))
    {
        return Err(ConversationError::InvalidAgentProfile(
            "local model source requires an http:// or https:// base URL".into(),
        ));
    }
    if matches!(profile.model_source, ModelSource::Local)
        && model.eq_ignore_ascii_case(AUTO_MODEL_ID)
    {
        return Err(ConversationError::InvalidAgentProfile(
            "Auto model is only available for built-in Claude/Codex catalogs; select an explicit local model id"
                .into(),
        ));
    }
    if matches!(profile.model_source, ModelSource::Default) && profile.llm_endpoint_id.is_some() {
        return Err(ConversationError::InvalidAgentProfile(
            "default model source must not reference a local endpoint".into(),
        ));
    }
    Ok(())
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

/// Ensure a client-minted conversation id belongs to the requesting actor.
///
/// Attachment intake can happen before the first chat turn, so it needs the
/// same first-writer-wins ownership boundary as `upsert_turn` without
/// incrementing the turn counter or fabricating transcript content.
pub async fn ensure_conversation_owner(
    pool: &PgPool,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), ConversationError> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        r#"INSERT INTO conversations (id, tenant_id, user_id, title)
           VALUES ($1, $2, $3, 'New chat')
           ON CONFLICT (id) DO UPDATE SET updated_at = conversations.updated_at
           WHERE conversations.tenant_id = $2 AND conversations.user_id = $3
             AND conversations.deleted_at IS NULL
           RETURNING tenant_id, user_id"#,
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((row_tenant, row_user)) if row_tenant == tenant_id && row_user == user_id => Ok(()),
        _ => Err(ConversationError::OwnershipMismatch),
    }
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

/// Load the durable Penny execution profile for a conversation.
///
/// `None` means the conversation exists but has not started/pinned a runtime
/// yet. The scoped variant additionally enforces the sidebar owner boundary.
pub async fn get_conversation_agent_profile(
    pool: &PgPool,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ConversationAgentProfile>, ConversationError> {
    let row = sqlx::query_as::<_, ConversationAgentProfileDbRow>(
        "SELECT agent_backend, agent_endpoint_id, agent_model, agent_effort, agent_model_source, \
                agent_local_base_url, agent_local_api_key_env \
         FROM conversations \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3 \
           AND deleted_at IS NULL AND agent_backend IS NOT NULL",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    row.map(profile_from_db).transpose()
}

/// Internal runtime lookup after the gateway has already enforced ownership.
/// This is used by Penny's durable session adapter, which receives only the
/// namespaced conversation id and intentionally does not carry HTTP identity.
pub async fn get_conversation_agent_profile_unscoped(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Option<ConversationAgentProfile>, ConversationError> {
    let row = sqlx::query_as::<_, ConversationAgentProfileDbRow>(
        "SELECT agent_backend, agent_endpoint_id, agent_model, agent_effort, agent_model_source, \
                agent_local_base_url, agent_local_api_key_env \
         FROM conversations \
         WHERE id = $1 AND deleted_at IS NULL AND agent_backend IS NOT NULL",
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await?;
    row.map(profile_from_db).transpose()
}

/// Atomically create/pin or update a conversation's Penny profile.
///
/// The backend is compare-and-set: NULL may become the requested backend,
/// the same backend may update model/effort, and a different backend is a
/// conflict. The row lock keeps simultaneous first-turn requests from both
/// believing they won the runtime pin.
pub async fn upsert_conversation_agent_profile(
    pool: &PgPool,
    conversation_id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
    profile: &ConversationAgentProfile,
) -> Result<ConversationAgentProfile, ConversationError> {
    validate_agent_profile(profile)?;
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO conversations (id, tenant_id, user_id, title) \
         VALUES ($1, $2, $3, 'New chat') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    let owner: Option<(Uuid, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT tenant_id, user_id, agent_backend \
         FROM conversations \
         WHERE id = $1 AND deleted_at IS NULL \
         FOR UPDATE",
    )
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((row_tenant, row_user, pinned_backend)) = owner else {
        return Err(ConversationError::NotFound);
    };
    if row_tenant != tenant_id || row_user != user_id {
        return Err(ConversationError::OwnershipMismatch);
    }
    if let Some(pinned) = pinned_backend.as_deref() {
        if pinned != profile.backend.as_str() {
            return Err(ConversationError::AgentBackendPinned {
                pinned: pinned.to_string(),
                requested: profile.backend.as_str().to_string(),
            });
        }
    }

    let row = sqlx::query_as::<_, ConversationAgentProfileDbRow>(
        "UPDATE conversations SET \
            agent_backend = COALESCE(agent_backend, $4), \
            agent_endpoint_id = $5, \
            agent_model = $6, \
            agent_effort = $7, \
            agent_model_source = $8, \
            agent_local_base_url = $9, \
            agent_local_api_key_env = $10, \
            updated_at = now() \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3 \
         RETURNING agent_backend, agent_endpoint_id, agent_model, agent_effort, agent_model_source, \
                   agent_local_base_url, agent_local_api_key_env",
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(profile.backend.as_str())
    .bind(profile.llm_endpoint_id)
    .bind(profile.model.trim())
    .bind(
        profile
            .effort
            .for_backend_model(profile.backend, &profile.model)
            .as_str(),
    )
    .bind(model_source_as_str(profile.model_source))
    .bind(profile.local_base_url.trim_end_matches('/'))
    .bind(profile.local_api_key_env.trim())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    profile_from_db(row)
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
        "SELECT id, tenant_id, user_id, claude_session_uuid, title, \
                agent_backend, agent_endpoint_id, agent_model, agent_effort, agent_model_source, \
                agent_local_base_url, agent_local_api_key_env, \
                created_at, updated_at \
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

    #[test]
    fn agent_profile_accepts_authless_local_responses_endpoint() {
        let profile = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: "local-model".into(),
            effort: AgentEffort::High,
            model_source: ModelSource::Local,
            local_base_url: "http://10.0.0.8:8000/v1".into(),
            local_api_key_env: String::new(),
        };
        assert!(validate_agent_profile(&profile).is_ok());
    }

    #[test]
    fn agent_profile_rejects_local_source_without_http_endpoint() {
        let profile = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: "local-model".into(),
            effort: AgentEffort::High,
            model_source: ModelSource::Local,
            local_base_url: "10.0.0.8:8000".into(),
            local_api_key_env: String::new(),
        };
        assert!(validate_agent_profile(&profile).is_err());
    }
}
