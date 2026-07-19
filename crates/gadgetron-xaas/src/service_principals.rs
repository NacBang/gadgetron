use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const BUNDLE_MONITOR_EMAIL: &str = "bundle-monitor@system.gadgetron";
const CLI_API_KEY_EMAIL: &str = "cli-api-key@system.gadgetron";
const KNOWLEDGE_AGENT_EMAIL: &str = "knowledge-agent@system.gadgetron";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServicePrincipal {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub kind: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum ServicePrincipalError {
    #[error("service-principal persistence failed")]
    Database(#[from] sqlx::Error),
    #[error("the reserved {kind} identity conflicts with a non-service user")]
    IdentityConflict { kind: &'static str },
    #[error("the {kind} service principal is inactive")]
    Inactive { kind: &'static str },
}

pub async fn ensure_bundle_monitor(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    ensure_service_user(
        pool,
        tenant_id,
        BUNDLE_MONITOR_EMAIL,
        "Bundle Monitor",
        "bundle-monitor",
    )
    .await
}

pub async fn ensure_cli_api_key_owner(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    ensure_service_user(
        pool,
        tenant_id,
        CLI_API_KEY_EMAIL,
        "CLI API Key",
        "cli-api-key",
    )
    .await
}

pub async fn ensure_knowledge_agent(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    let mut tx = pool.begin().await?;
    let principal = ensure_knowledge_agent_in_transaction(&mut tx, tenant_id).await?;
    tx.commit().await?;
    Ok(principal)
}

pub(crate) async fn ensure_knowledge_agent_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    ensure_service_user_in_transaction(
        tx,
        tenant_id,
        KNOWLEDGE_AGENT_EMAIL,
        "Knowledge Agent",
        "knowledge-agent",
    )
    .await
}

async fn ensure_service_user_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    email: &str,
    display_name: &str,
    kind: &'static str,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    sqlx::query(
        r#"
        INSERT INTO users (tenant_id, email, display_name, role, password_hash)
        VALUES ($1, $2, $3, 'service', NULL)
        ON CONFLICT (tenant_id, email) DO NOTHING
        "#,
    )
    .bind(tenant_id)
    .bind(email)
    .bind(display_name)
    .execute(&mut **tx)
    .await?;

    let row: (Uuid, String, bool, Option<String>) = sqlx::query_as(
        r#"
        SELECT id, role, is_active, password_hash
          FROM users
         WHERE tenant_id = $1 AND email = $2
        "#,
    )
    .bind(tenant_id)
    .bind(email)
    .fetch_one(&mut **tx)
    .await?;
    if row.1 != "service" || row.3.is_some() {
        return Err(ServicePrincipalError::IdentityConflict { kind });
    }
    if !row.2 {
        return Err(ServicePrincipalError::Inactive { kind });
    }
    Ok(ServicePrincipal {
        user_id: row.0,
        tenant_id,
        kind,
    })
}

pub async fn validate_knowledge_agent(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    let row: Option<(String, String, bool, Option<String>)> = sqlx::query_as(
        r#"
        SELECT email, role, is_active, password_hash
          FROM users
         WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some((email, role, is_active, password_hash)) = row else {
        return Err(ServicePrincipalError::IdentityConflict {
            kind: "knowledge-agent",
        });
    };
    if email != KNOWLEDGE_AGENT_EMAIL || role != "service" || password_hash.is_some() {
        return Err(ServicePrincipalError::IdentityConflict {
            kind: "knowledge-agent",
        });
    }
    if !is_active {
        return Err(ServicePrincipalError::Inactive {
            kind: "knowledge-agent",
        });
    }
    Ok(ServicePrincipal {
        user_id,
        tenant_id,
        kind: "knowledge-agent",
    })
}

pub async fn ensure_autonomy_agent(
    pool: &PgPool,
    tenant_id: Uuid,
    goal_id: Uuid,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    let email = format!("autonomy-{goal_id}@system.gadgetron");
    let display_name = format!("Autonomy Worker {}", &goal_id.simple().to_string()[..8]);
    ensure_service_user(pool, tenant_id, &email, &display_name, "autonomy-agent").await
}

async fn ensure_service_user(
    pool: &PgPool,
    tenant_id: Uuid,
    email: &str,
    display_name: &str,
    kind: &'static str,
) -> Result<ServicePrincipal, ServicePrincipalError> {
    sqlx::query(
        r#"
        INSERT INTO users (tenant_id, email, display_name, role, password_hash)
        VALUES ($1, $2, $3, 'service', NULL)
        ON CONFLICT (tenant_id, email) DO NOTHING
        "#,
    )
    .bind(tenant_id)
    .bind(email)
    .bind(display_name)
    .execute(pool)
    .await?;

    let row: (Uuid, String, bool, Option<String>) = sqlx::query_as(
        r#"
        SELECT id, role, is_active, password_hash
          FROM users
         WHERE tenant_id = $1 AND email = $2
        "#,
    )
    .bind(tenant_id)
    .bind(email)
    .fetch_one(pool)
    .await?;
    if row.1 != "service" || row.3.is_some() {
        return Err(ServicePrincipalError::IdentityConflict { kind });
    }
    if !row.2 {
        return Err(ServicePrincipalError::Inactive { kind });
    }
    Ok(ServicePrincipal {
        user_id: row.0,
        tenant_id,
        kind,
    })
}
