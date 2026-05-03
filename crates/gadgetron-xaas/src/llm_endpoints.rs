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
               listen_port, auth_token_env, model_id,
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
                  listen_port, auth_token_env, model_id,
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
    name: &str,
    kind: &str,
    protocol: &str,
    base_url: &str,
    model_id: Option<&str>,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    let row = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        INSERT INTO llm_endpoints (
            tenant_id, name, kind, protocol, base_url, target_kind, model_id
        )
        VALUES ($1, $2, $3, $4, $5, 'external', $6)
        ON CONFLICT (tenant_id, name) DO UPDATE SET
            kind = EXCLUDED.kind,
            protocol = EXCLUDED.protocol,
            base_url = EXCLUDED.base_url,
            target_kind = EXCLUDED.target_kind,
            target_host_id = NULL,
            upstream_endpoint_id = NULL,
            listen_port = NULL,
            auth_token_env = NULL,
            model_id = EXCLUDED.model_id,
            updated_at = NOW()
        RETURNING id, tenant_id, name, kind, protocol, base_url,
                  target_kind, target_host_id, upstream_endpoint_id,
                  listen_port, auth_token_env, model_id,
                  health_status, last_probe_at, last_ok_at, last_error,
                  last_latency_ms, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(name)
    .bind(kind)
    .bind(protocol)
    .bind(base_url)
    .bind(model_id)
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
               listen_port, auth_token_env, model_id,
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
            updated_at = NOW()
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, name, kind, protocol, base_url,
                  target_kind, target_host_id, upstream_endpoint_id,
                  listen_port, auth_token_env, model_id,
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

pub async fn update_llm_endpoint_probe(
    pool: &PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
    health_status: &str,
    last_error: Option<&str>,
    last_latency_ms: Option<i32>,
) -> Result<LlmEndpointRow, LlmEndpointError> {
    let row = sqlx::query_as::<_, LlmEndpointRow>(
        r#"
        UPDATE llm_endpoints
        SET health_status = $3,
            last_probe_at = NOW(),
            last_ok_at = CASE WHEN $3 = 'ok' THEN NOW() ELSE last_ok_at END,
            last_error = $4,
            last_latency_ms = $5,
            updated_at = NOW()
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, name, kind, protocol, base_url,
                  target_kind, target_host_id, upstream_endpoint_id,
                  listen_port, auth_token_env, model_id,
                  health_status, last_probe_at, last_ok_at, last_error,
                  last_latency_ms, created_at, updated_at
        "#,
    )
    .bind(endpoint_id)
    .bind(tenant_id)
    .bind(health_status)
    .bind(last_error)
    .bind(last_latency_ms)
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
