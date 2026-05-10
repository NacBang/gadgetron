//! Postgres-backed Penny brain runtime settings.
//!
//! The table stores only routing/model metadata and env var names. It never
//! stores API keys or gateway tokens.

use chrono::{DateTime, Utc};
use gadgetron_core::agent::{
    AgentBrainSettings, AgentBrainSettingsSource, BrainMode, UpdateAgentBrainSettingsRequest,
};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
struct AgentBrainSettingsRow {
    mode: String,
    external_base_url: String,
    model: String,
    external_auth_token_env: String,
    custom_model_option: bool,
    updated_by: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentBrainSettingsError {
    #[error("invalid persisted agent brain mode: {0}")]
    InvalidMode(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

fn row_to_settings(
    row: AgentBrainSettingsRow,
) -> Result<AgentBrainSettings, AgentBrainSettingsError> {
    let mode = BrainMode::parse(&row.mode)
        .ok_or_else(|| AgentBrainSettingsError::InvalidMode(row.mode.clone()))?;
    Ok(AgentBrainSettings {
        mode,
        external_base_url: row.external_base_url,
        model: row.model,
        external_auth_token_env: row.external_auth_token_env,
        custom_model_option: row.custom_model_option,
        updated_at: Some(row.updated_at),
        updated_by: row.updated_by,
        source: AgentBrainSettingsSource::Database,
    })
}

pub async fn get_agent_brain_settings(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Option<AgentBrainSettings>, AgentBrainSettingsError> {
    let row = sqlx::query_as::<_, AgentBrainSettingsRow>(
        r#"
        SELECT mode, external_base_url, model, external_auth_token_env,
               custom_model_option, updated_by, updated_at
        FROM agent_brain_settings
        WHERE tenant_id = $1
        "#,
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;

    row.map(row_to_settings).transpose()
}

pub async fn upsert_agent_brain_settings(
    pool: &PgPool,
    tenant_id: Uuid,
    actor_user_id: Option<Uuid>,
    request: &UpdateAgentBrainSettingsRequest,
) -> Result<AgentBrainSettings, AgentBrainSettingsError> {
    let row = sqlx::query_as::<_, AgentBrainSettingsRow>(
        r#"
        INSERT INTO agent_brain_settings (
            tenant_id, mode, external_base_url, model,
            external_auth_token_env, custom_model_option, updated_by, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
        ON CONFLICT (tenant_id) DO UPDATE SET
            mode = EXCLUDED.mode,
            external_base_url = EXCLUDED.external_base_url,
            model = EXCLUDED.model,
            external_auth_token_env = EXCLUDED.external_auth_token_env,
            custom_model_option = EXCLUDED.custom_model_option,
            updated_by = EXCLUDED.updated_by,
            updated_at = NOW()
        RETURNING mode, external_base_url, model, external_auth_token_env,
                  custom_model_option, updated_by, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(request.mode.as_str())
    .bind(&request.external_base_url)
    .bind(&request.model)
    .bind(&request.external_auth_token_env)
    .bind(request.custom_model_option)
    .bind(actor_user_id)
    .fetch_one(pool)
    .await?;

    row_to_settings(row)
}
