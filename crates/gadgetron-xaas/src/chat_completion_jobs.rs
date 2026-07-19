use chrono::{DateTime, Utc};
use gadgetron_core::agent::ConversationAgentProfile;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub const RESTART_TERMINAL_MESSAGE: &str =
    "Generation stopped because Gadgetron restarted. Your request and completed messages were preserved; send it again to continue.";

#[derive(Debug, Clone, Copy)]
pub enum TerminalStatus {
    Complete,
    Error,
    Cancelled,
}

impl TerminalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChatCompletionJobRow {
    pub job_id: Uuid,
    pub conversation_id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub model: String,
    pub agent_profile: Option<Value>,
    pub status: String,
    pub chunk_count: i32,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub struct StartChatCompletionJob<'a> {
    pub job_id: Uuid,
    pub conversation_id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub model: &'a str,
    pub agent_profile: Option<&'a ConversationAgentProfile>,
}

pub async fn start(
    pool: &PgPool,
    input: StartChatCompletionJob<'_>,
) -> Result<ChatCompletionJobRow, sqlx::Error> {
    let agent_profile = input
        .agent_profile
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    sqlx::query_as::<_, ChatCompletionJobRow>(
        r#"INSERT INTO chat_completion_jobs
               (job_id, conversation_id, tenant_id, user_id, model, agent_profile, status)
           VALUES ($1, $2, $3, $4, $5, $6, 'streaming')
           RETURNING job_id, conversation_id, tenant_id, user_id, model, agent_profile,
                     status, chunk_count, error_message, created_at, finished_at"#,
    )
    .bind(input.job_id)
    .bind(input.conversation_id)
    .bind(input.tenant_id)
    .bind(input.user_id)
    .bind(input.model)
    .bind(agent_profile)
    .fetch_one(pool)
    .await
}

pub async fn finish(
    pool: &PgPool,
    job_id: Uuid,
    status: TerminalStatus,
    chunk_count: usize,
    error_message: Option<&str>,
) -> Result<bool, sqlx::Error> {
    finish_with_assistant_message(pool, job_id, status, chunk_count, error_message, None).await
}

pub async fn finish_with_assistant_message(
    pool: &PgPool,
    job_id: Uuid,
    status: TerminalStatus,
    chunk_count: usize,
    error_message: Option<&str>,
    assistant_message: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let error_message =
        error_message.map(|message| message.chars().take(2_048).collect::<String>());
    let assistant_message = assistant_message
        .map(str::trim)
        .filter(|message| !message.is_empty());
    let mut transaction = pool.begin().await?;
    let owner = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        r#"UPDATE chat_completion_jobs
           SET status = $2, chunk_count = $3, error_message = $4, finished_at = now()
           WHERE job_id = $1 AND status = 'streaming'
           RETURNING conversation_id, tenant_id, user_id"#,
    )
    .bind(job_id)
    .bind(status.as_str())
    .bind(i32::try_from(chunk_count).unwrap_or(i32::MAX))
    .bind(error_message)
    .fetch_optional(&mut *transaction)
    .await?;
    if let (Some((conversation_id, tenant_id, user_id)), Some(content)) = (owner, assistant_message)
    {
        sqlx::query(
            r#"INSERT INTO conversation_messages
                   (conversation_id, tenant_id, user_id, role, content)
               VALUES ($1, $2, $3, 'assistant', $4)"#,
        )
        .bind(conversation_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(content)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(owner.is_some())
}

pub async fn latest_terminal_for_conversation(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    conversation_id: Uuid,
) -> Result<Option<ChatCompletionJobRow>, sqlx::Error> {
    sqlx::query_as::<_, ChatCompletionJobRow>(
        r#"SELECT job_id, conversation_id, tenant_id, user_id, model, agent_profile,
                  status, chunk_count, error_message, created_at, finished_at
           FROM chat_completion_jobs
           WHERE tenant_id = $1 AND user_id = $2 AND conversation_id = $3
             AND status <> 'streaming'
             AND finished_at >= now() - interval '10 minutes'
           ORDER BY finished_at DESC, created_at DESC
           LIMIT 1"#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(conversation_id)
    .fetch_optional(pool)
    .await
}

pub async fn recover_interrupted(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let rows = sqlx::query_as::<_, ChatCompletionJobRow>(
        r#"UPDATE chat_completion_jobs
           SET status = 'error', error_message = $1, finished_at = now()
           WHERE status = 'streaming'
           RETURNING job_id, conversation_id, tenant_id, user_id, model, agent_profile,
                     status, chunk_count, error_message, created_at, finished_at"#,
    )
    .bind(RESTART_TERMINAL_MESSAGE)
    .fetch_all(&mut *transaction)
    .await?;
    for row in &rows {
        append_restart_message(&mut transaction, row).await?;
    }
    transaction.commit().await?;
    Ok(rows.len() as u64)
}

async fn append_restart_message(
    transaction: &mut Transaction<'_, Postgres>,
    row: &ChatCompletionJobRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO conversation_messages
               (conversation_id, tenant_id, user_id, role, content)
           VALUES ($1, $2, $3, 'assistant', $4)"#,
    )
    .bind(row.conversation_id)
    .bind(row.tenant_id)
    .bind(row.user_id)
    .bind(RESTART_TERMINAL_MESSAGE)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
