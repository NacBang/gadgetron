//! Tenant-scoped LLM endpoint registry.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct LlmEndpointRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub kind: String,
    pub protocol: String,
    pub base_url: String,
    pub target_kind: String,
    pub target_host_id: Option<Uuid>,
    pub upstream_endpoint_id: Option<Uuid>,
    pub listen_port: Option<i32>,
    pub auth_token_env: Option<String>,
    pub model_id: Option<String>,
    pub discovered_models: serde_json::Value,
    pub runtime_compatibility: String,
    pub tool_status: String,
    pub tool_model_id: Option<String>,
    pub last_tool_probe_at: Option<DateTime<Utc>>,
    pub last_tool_error: Option<String>,
    pub capability_details: serde_json::Value,
    pub health_status: String,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_latency_ms: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct LlmEndpointCreate<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub protocol: &'a str,
    pub base_url: &'a str,
    pub target_kind: &'a str,
    pub target_host_id: Option<Uuid>,
    pub upstream_endpoint_id: Option<Uuid>,
    pub listen_port: Option<i32>,
    pub auth_token_env: Option<&'a str>,
    pub model_id: Option<&'a str>,
}

pub struct LlmEndpointUpsert<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub protocol: &'a str,
    pub base_url: &'a str,
    pub auth_token_env: Option<&'a str>,
    pub model_id: Option<&'a str>,
}

/// One atomic endpoint capability snapshot. Connectivity and protocol
/// compatibility are intentionally separate from the actual model tool-call
/// result so `/v1/models` or a validation response can never imply Penny
/// readiness.
pub struct LlmEndpointCapabilityUpdate<'a> {
    pub protocol: &'a str,
    pub model_id: Option<&'a str>,
    pub discovered_models: &'a [String],
    pub health_status: &'a str,
    pub last_error: Option<&'a str>,
    pub last_latency_ms: Option<i32>,
    pub runtime_compatibility: &'a str,
    pub tool_status: &'a str,
    pub tool_model_id: Option<&'a str>,
    pub last_tool_error: Option<&'a str>,
    pub capability_details: &'a serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmEndpointError {
    #[error("endpoint name already exists in this tenant")]
    DuplicateName,
    #[error("endpoint not found")]
    NotFound,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn list_llm_endpoints(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<LlmEndpointRow>, LlmEndpointError> {
    let rows = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        SELECT id, tenant_id, name, kind, protocol, base_url,
               target_kind, target_host_id, upstream_endpoint_id,
               listen_port, auth_token_env, model_id, discovered_models,
               runtime_compatibility, tool_status, tool_model_id,
               last_tool_probe_at, last_tool_error, capability_details,
               health_status, last_probe_at, last_ok_at, last_error,
               last_latency_ms, created_at, updated_at
        FROM llm_endpoints
        WHERE tenant_id = $1
        ORDER BY updated_at DESC, created_at DESC
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn create_llm_endpoint(
    pool: &PgPool,
    tenant_id: Uuid,
    name: &str,
    kind: &str,
    protocol: &str,
    base_url: &str,
    model_id: Option<&str>,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    create_llm_endpoint_with_target(
        pool,
        tenant_id,
        LlmEndpointCreate {
            name,
            kind,
            protocol,
            base_url,
            target_kind: "external",
            target_host_id: None,
            upstream_endpoint_id: None,
            listen_port: None,
            auth_token_env: None,
            model_id,
        },
    )
    .await
}

pub async fn create_llm_endpoint_with_target(
    pool: &PgPool,
    tenant_id: Uuid,
    input: LlmEndpointCreate<'_>,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    let result = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        INSERT INTO llm_endpoints (
            tenant_id, name, kind, protocol, base_url,
            target_kind, target_host_id, upstream_endpoint_id,
            listen_port, auth_token_env, model_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, tenant_id, name, kind, protocol, base_url,
                  target_kind, target_host_id, upstream_endpoint_id,
                  listen_port, auth_token_env, model_id, discovered_models,
                  runtime_compatibility, tool_status, tool_model_id,
                  last_tool_probe_at, last_tool_error, capability_details,
                  health_status, last_probe_at, last_ok_at, last_error,
                  last_latency_ms, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(input.name)
    .bind(input.kind)
    .bind(input.protocol)
    .bind(input.base_url)
    .bind(input.target_kind)
    .bind(input.target_host_id)
    .bind(input.upstream_endpoint_id)
    .bind(input.listen_port)
    .bind(input.auth_token_env)
    .bind(input.model_id)
    .fetch_one(pool)
    .await;

    match result {
        Ok(row) => Ok(row),
        Err(sqlx::Error::Database(db_err))
            if db_err.constraint() == Some("llm_endpoints_tenant_id_name_key") =>
        {
            Err(LlmEndpointError::DuplicateName)
        }
        Err(e) => Err(LlmEndpointError::Db(e)),
    }
}

pub async fn upsert_llm_endpoint_by_name(
    pool: &PgPool,
    tenant_id: Uuid,
    input: LlmEndpointUpsert<'_>,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    let row = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        INSERT INTO llm_endpoints (
            tenant_id, name, kind, protocol, base_url, target_kind,
            auth_token_env, model_id
        )
        VALUES ($1, $2, $3, $4, $5, 'external', $6, $7)
        ON CONFLICT (tenant_id, name) DO UPDATE SET
            kind = EXCLUDED.kind,
            protocol = EXCLUDED.protocol,
            base_url = EXCLUDED.base_url,
            target_kind = EXCLUDED.target_kind,
            target_host_id = NULL,
            upstream_endpoint_id = NULL,
            listen_port = NULL,
            auth_token_env = EXCLUDED.auth_token_env,
            model_id = EXCLUDED.model_id,
            discovered_models = '[]'::jsonb,
            runtime_compatibility = 'unverified',
            tool_status = 'untested',
            tool_model_id = NULL,
            last_tool_probe_at = NULL,
            last_tool_error = NULL,
            capability_details = '{}'::jsonb,
            updated_at = NOW()
        RETURNING id, tenant_id, name, kind, protocol, base_url,
                  target_kind, target_host_id, upstream_endpoint_id,
                  listen_port, auth_token_env, model_id, discovered_models,
                  runtime_compatibility, tool_status, tool_model_id,
                  last_tool_probe_at, last_tool_error, capability_details,
                  health_status, last_probe_at, last_ok_at, last_error,
                  last_latency_ms, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(input.name)
    .bind(input.kind)
    .bind(input.protocol)
    .bind(input.base_url)
    .bind(input.auth_token_env)
    .bind(input.model_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn get_llm_endpoint(
    pool: &PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    let row = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        SELECT id, tenant_id, name, kind, protocol, base_url,
               target_kind, target_host_id, upstream_endpoint_id,
               listen_port, auth_token_env, model_id, discovered_models,
               runtime_compatibility, tool_status, tool_model_id,
               last_tool_probe_at, last_tool_error, capability_details,
               health_status, last_probe_at, last_ok_at, last_error,
               last_latency_ms, created_at, updated_at
        FROM llm_endpoints
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(endpoint_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(LlmEndpointError::NotFound)
}

pub async fn update_llm_endpoint_model(
    pool: &PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
    model_id: Option<&str>,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    let row = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        UPDATE llm_endpoints
        SET model_id = $3,
            tool_status = 'untested',
            tool_model_id = NULL,
            last_tool_probe_at = NULL,
            last_tool_error = NULL,
            updated_at = NOW()
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, name, kind, protocol, base_url,
                  target_kind, target_host_id, upstream_endpoint_id,
                  listen_port, auth_token_env, model_id, discovered_models,
                  runtime_compatibility, tool_status, tool_model_id,
                  last_tool_probe_at, last_tool_error, capability_details,
                  health_status, last_probe_at, last_ok_at, last_error,
                  last_latency_ms, created_at, updated_at
        "#,
    )
    .bind(endpoint_id)
    .bind(tenant_id)
    .bind(model_id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(LlmEndpointError::NotFound)
}

pub async fn update_llm_endpoint_auth_env(
    pool: &PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
    auth_token_env: Option<&str>,
) -> Result<(), LlmEndpointError> {
    let result = sqlx::query(
        "UPDATE llm_endpoints SET auth_token_env = $3, updated_at = NOW() \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(endpoint_id)
    .bind(tenant_id)
    .bind(auth_token_env)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(LlmEndpointError::NotFound);
    }
    Ok(())
}

pub async fn update_llm_endpoint_capability(
    pool: &PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
    update: LlmEndpointCapabilityUpdate<'_>,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    let discovered_models =
        serde_json::to_value(update.discovered_models).expect("string model ids always serialize");
    let row = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        UPDATE llm_endpoints
        SET protocol = $3,
            model_id = $4,
            discovered_models = $5,
            health_status = $6,
            last_probe_at = NOW(),
            last_ok_at = CASE WHEN $6 = 'ok' THEN NOW() ELSE last_ok_at END,
            last_error = $7,
            last_latency_ms = $8,
            runtime_compatibility = $9,
            tool_status = $10,
            tool_model_id = $11,
            last_tool_probe_at = CASE
                WHEN $10 IN ('passed', 'failed') THEN NOW()
                ELSE last_tool_probe_at
            END,
            last_tool_error = $12,
            capability_details = $13,
            updated_at = NOW()
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, name, kind, protocol, base_url,
                  target_kind, target_host_id, upstream_endpoint_id,
                  listen_port, auth_token_env, model_id, discovered_models,
                  runtime_compatibility, tool_status, tool_model_id,
                  last_tool_probe_at, last_tool_error, capability_details,
                  health_status, last_probe_at, last_ok_at, last_error,
                  last_latency_ms, created_at, updated_at
        "#,
    )
    .bind(endpoint_id)
    .bind(tenant_id)
    .bind(update.protocol)
    .bind(update.model_id)
    .bind(discovered_models)
    .bind(update.health_status)
    .bind(update.last_error)
    .bind(update.last_latency_ms)
    .bind(update.runtime_compatibility)
    .bind(update.tool_status)
    .bind(update.tool_model_id)
    .bind(update.last_tool_error)
    .bind(update.capability_details)
    .fetch_optional(pool)
    .await?;
    row.ok_or(LlmEndpointError::NotFound)
}

pub async fn delete_llm_endpoint(
    pool: &PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
) -> Result<(), LlmEndpointError> {
    let result = sqlx::query("DELETE FROM llm_endpoints WHERE id = $1 AND tenant_id = $2")
        .bind(endpoint_id)
        .bind(tenant_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(LlmEndpointError::NotFound);
    }
    Ok(())
}
