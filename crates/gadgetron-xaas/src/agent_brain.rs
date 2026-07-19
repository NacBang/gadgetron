//! Postgres-backed Penny agent backend settings.
//!
//! The table stores only routing/model metadata and env var names. It never
//! stores API keys or gateway tokens.

use chrono::{DateTime, Utc};
use gadgetron_core::agent::{
    AgentBackend, AgentBrainSettings, AgentBrainSettingsSource, AgentEffort, BrainMode,
    ModelSource, UpdateAgentBrainSettingsRequest,
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
    agent: String,
    llm_endpoint_id: Option<Uuid>,
    model_source: String,
    local_base_url: String,
    local_api_key_env: String,
    effort: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentBrainSettingsError {
    #[error("invalid persisted agent brain mode: {0}")]
    InvalidMode(String),
    #[error("invalid persisted agent backend: {0}")]
    InvalidBackend(String),
    #[error("invalid persisted model source: {0}")]
    InvalidModelSource(String),
    #[error("invalid persisted effort level: {0}")]
    InvalidEffort(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

fn parse_backend(raw: &str) -> Result<AgentBackend, AgentBrainSettingsError> {
    match raw {
        "claude_code" => Ok(AgentBackend::ClaudeCode),
        "codex_exec" => Ok(AgentBackend::CodexExec),
        other => Err(AgentBrainSettingsError::InvalidBackend(other.to_string())),
    }
}

fn backend_to_str(v: AgentBackend) -> &'static str {
    v.as_str()
}

fn parse_model_source(raw: &str) -> Result<ModelSource, AgentBrainSettingsError> {
    match raw {
        "default" => Ok(ModelSource::Default),
        "local" => Ok(ModelSource::Local),
        other => Err(AgentBrainSettingsError::InvalidModelSource(
            other.to_string(),
        )),
    }
}

fn model_source_to_str(v: ModelSource) -> &'static str {
    match v {
        ModelSource::Default => "default",
        ModelSource::Local => "local",
    }
}

fn parse_effort(raw: &str) -> Result<AgentEffort, AgentBrainSettingsError> {
    match raw {
        "auto" => Ok(AgentEffort::Auto),
        "low" => Ok(AgentEffort::Low),
        "medium" => Ok(AgentEffort::Medium),
        "high" => Ok(AgentEffort::High),
        "xhigh" => Ok(AgentEffort::Xhigh),
        "max" => Ok(AgentEffort::Max),
        "ultra" => Ok(AgentEffort::Ultra),
        other => Err(AgentBrainSettingsError::InvalidEffort(other.to_string())),
    }
}

fn effort_to_str(v: AgentEffort) -> &'static str {
    match v {
        AgentEffort::Auto => "auto",
        AgentEffort::Low => "low",
        AgentEffort::Medium => "medium",
        AgentEffort::High => "high",
        AgentEffort::Xhigh => "xhigh",
        AgentEffort::Max => "max",
        AgentEffort::Ultra => "ultra",
    }
}

fn row_to_settings(
    row: AgentBrainSettingsRow,
) -> Result<AgentBrainSettings, AgentBrainSettingsError> {
    let mode = BrainMode::parse(&row.mode)
        .ok_or_else(|| AgentBrainSettingsError::InvalidMode(row.mode.clone()))?;
    let backend = parse_backend(&row.agent)?;
    let model_source = parse_model_source(&row.model_source)?;
    let effort = parse_effort(&row.effort)?;
    Ok(AgentBrainSettings {
        mode,
        external_base_url: row.external_base_url,
        model: row.model,
        external_auth_token_env: row.external_auth_token_env,
        custom_model_option: row.custom_model_option,
        updated_at: Some(row.updated_at),
        updated_by: row.updated_by,
        source: AgentBrainSettingsSource::Database,
        backend,
        llm_endpoint_id: row.llm_endpoint_id,
        model_source,
        local_base_url: row.local_base_url,
        local_api_key_env: row.local_api_key_env,
        effort,
    })
}

pub async fn get_agent_brain_settings(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Option<AgentBrainSettings>, AgentBrainSettingsError> {
    let row = sqlx::query_as::<_, AgentBrainSettingsRow>(
        r#"
        SELECT mode, external_base_url, model, external_auth_token_env,
               custom_model_option, updated_by, updated_at,
               agent, llm_endpoint_id, model_source,
               local_base_url, local_api_key_env, effort
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
            external_auth_token_env, custom_model_option, updated_by, updated_at,
            agent, llm_endpoint_id, model_source,
            local_base_url, local_api_key_env, effort
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(),
                $8, $9, $10, $11, $12, $13)
        ON CONFLICT (tenant_id) DO UPDATE SET
            mode = EXCLUDED.mode,
            external_base_url = EXCLUDED.external_base_url,
            model = EXCLUDED.model,
            external_auth_token_env = EXCLUDED.external_auth_token_env,
            custom_model_option = EXCLUDED.custom_model_option,
            updated_by = EXCLUDED.updated_by,
            updated_at = NOW(),
            agent = EXCLUDED.agent,
            llm_endpoint_id = EXCLUDED.llm_endpoint_id,
            model_source = EXCLUDED.model_source,
            local_base_url = EXCLUDED.local_base_url,
            local_api_key_env = EXCLUDED.local_api_key_env,
            effort = EXCLUDED.effort
        RETURNING mode, external_base_url, model, external_auth_token_env,
                  custom_model_option, updated_by, updated_at,
                  agent, llm_endpoint_id, model_source,
                  local_base_url, local_api_key_env, effort
        "#,
    )
    .bind(tenant_id)
    .bind(request.mode.as_str())
    .bind(&request.external_base_url)
    .bind(&request.model)
    .bind(&request.external_auth_token_env)
    .bind(request.custom_model_option)
    .bind(actor_user_id)
    .bind(backend_to_str(request.backend))
    .bind(request.llm_endpoint_id)
    .bind(model_source_to_str(request.model_source))
    .bind(&request.local_base_url)
    .bind(&request.local_api_key_env)
    .bind(effort_to_str(request.effort))
    .fetch_one(pool)
    .await?;

    row_to_settings(row)
}
