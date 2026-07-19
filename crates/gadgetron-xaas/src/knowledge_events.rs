//! CORE-T3 transactional Knowledge-event outbox authority.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const MAX_ATTEMPTS: i32 = 3;

#[derive(Debug, Clone)]
pub struct EnqueueKnowledgeEvent {
    pub tenant_id: Uuid,
    pub descriptor_id: String,
    pub event_kind: String,
    pub publisher_bundle_id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub subject_revision: String,
    pub snapshot: Value,
    pub snapshot_hash: String,
    pub source_title: String,
    pub source_path_prefix: String,
    pub acting_space_id: Uuid,
    pub output_vault_bundle: String,
    pub knowledge_schema_id: String,
    pub researcher_bundle_id: String,
    pub researcher_role_id: String,
    pub requested_by_user_id: Uuid,
    pub service_actor_user_id: Uuid,
    pub effective_role: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeEventRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub descriptor_id: String,
    pub event_kind: String,
    pub publisher_bundle_id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub subject_revision: String,
    pub snapshot: Value,
    pub snapshot_hash: String,
    pub source_title: String,
    pub source_path_prefix: String,
    pub acting_space_id: Uuid,
    pub output_vault_bundle: String,
    pub knowledge_schema_id: String,
    pub researcher_bundle_id: String,
    pub researcher_role_id: String,
    pub requested_by_user_id: Uuid,
    pub service_actor_user_id: Uuid,
    pub effective_role: String,
    pub status: String,
    pub attempt_count: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub source_id: Option<Uuid>,
    pub knowledge_job_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub oversight_recorded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDisposition {
    Retry,
    Terminal,
}

pub async fn enqueue_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    event: EnqueueKnowledgeEvent,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        r#"INSERT INTO knowledge_event_outbox (
               tenant_id, descriptor_id, event_kind, publisher_bundle_id,
               subject_kind, subject_id, subject_revision, snapshot, snapshot_hash,
               source_title, source_path_prefix, acting_space_id, output_vault_bundle,
               knowledge_schema_id, researcher_bundle_id, researcher_role_id,
               requested_by_user_id, service_actor_user_id, effective_role
             ) VALUES (
               $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19
             )
             ON CONFLICT (
               tenant_id, publisher_bundle_id, event_kind, subject_kind,
               subject_id, subject_revision, researcher_bundle_id, researcher_role_id
             ) DO UPDATE SET updated_at = knowledge_event_outbox.updated_at
             RETURNING id"#,
    )
    .bind(event.tenant_id)
    .bind(event.descriptor_id)
    .bind(event.event_kind)
    .bind(event.publisher_bundle_id)
    .bind(event.subject_kind)
    .bind(event.subject_id)
    .bind(event.subject_revision)
    .bind(event.snapshot)
    .bind(event.snapshot_hash)
    .bind(event.source_title)
    .bind(event.source_path_prefix)
    .bind(event.acting_space_id)
    .bind(event.output_vault_bundle)
    .bind(event.knowledge_schema_id)
    .bind(event.researcher_bundle_id)
    .bind(event.researcher_role_id)
    .bind(event.requested_by_user_id)
    .bind(event.service_actor_user_id)
    .bind(event.effective_role)
    .fetch_one(&mut **transaction)
    .await
}

pub async fn lease_next(
    pool: &PgPool,
    worker_id: &str,
    lease_seconds: i32,
) -> Result<Option<KnowledgeEventRow>, sqlx::Error> {
    sqlx::query_as::<_, KnowledgeEventRow>(&format!(
        r#"WITH candidate AS (
             SELECT id FROM knowledge_event_outbox
              WHERE attempt_count < {MAX_ATTEMPTS}
                AND next_attempt_at <= NOW()
                AND (
                  status = 'pending'
                  OR (status = 'processing' AND lease_expires_at <= NOW())
                )
              ORDER BY next_attempt_at, created_at, id
              FOR UPDATE SKIP LOCKED
              LIMIT 1
           )
           UPDATE knowledge_event_outbox event SET
             status = 'processing', attempt_count = event.attempt_count + 1,
             lease_owner = $1,
             lease_expires_at = NOW() + make_interval(secs => $2),
             updated_at = NOW()
           FROM candidate WHERE event.id = candidate.id
           RETURNING {}"#,
        columns("event")
    ))
    .bind(worker_id)
    .bind(lease_seconds)
    .fetch_optional(pool)
    .await
}

pub async fn complete(
    pool: &PgPool,
    event_id: Uuid,
    worker_id: &str,
    source_id: Uuid,
    knowledge_job_id: Uuid,
) -> Result<KnowledgeEventRow, sqlx::Error> {
    sqlx::query_as::<_, KnowledgeEventRow>(&format!(
        r#"UPDATE knowledge_event_outbox event SET
             status = 'completed', source_id = $3, knowledge_job_id = $4,
             completed_at = NOW(), lease_owner = NULL, lease_expires_at = NULL,
             last_error = NULL, updated_at = NOW()
           WHERE id = $1 AND status = 'processing' AND lease_owner = $2
           RETURNING {}"#,
        columns("event")
    ))
    .bind(event_id)
    .bind(worker_id)
    .bind(source_id)
    .bind(knowledge_job_id)
    .fetch_one(pool)
    .await
}

pub async fn fail(
    pool: &PgPool,
    event_id: Uuid,
    worker_id: &str,
    detail: &str,
) -> Result<FailureDisposition, sqlx::Error> {
    let clipped: String = detail.chars().take(1_000).collect();
    let terminal: bool = sqlx::query_scalar(
        r#"UPDATE knowledge_event_outbox SET
             status = CASE WHEN attempt_count >= $4 THEN 'failed' ELSE 'pending' END,
             next_attempt_at = CASE WHEN attempt_count >= $4 THEN next_attempt_at
                                    ELSE NOW() + make_interval(secs => attempt_count * 2) END,
             lease_owner = NULL, lease_expires_at = NULL,
             last_error = $3, updated_at = NOW()
           WHERE id = $1 AND status = 'processing' AND lease_owner = $2
           RETURNING status = 'failed'"#,
    )
    .bind(event_id)
    .bind(worker_id)
    .bind(clipped)
    .bind(MAX_ATTEMPTS)
    .fetch_one(pool)
    .await?;
    Ok(if terminal {
        FailureDisposition::Terminal
    } else {
        FailureDisposition::Retry
    })
}

pub async fn get(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<KnowledgeEventRow, sqlx::Error> {
    sqlx::query_as::<_, KnowledgeEventRow>(&format!(
        "SELECT {} FROM knowledge_event_outbox event WHERE tenant_id = $1 AND id = $2",
        columns("event")
    ))
    .bind(tenant_id)
    .bind(id)
    .fetch_one(pool)
    .await
}

pub async fn next_unreported_failure(
    pool: &PgPool,
) -> Result<Option<KnowledgeEventRow>, sqlx::Error> {
    sqlx::query_as::<_, KnowledgeEventRow>(&format!(
        r#"SELECT {} FROM knowledge_event_outbox event
           WHERE status = 'failed' AND oversight_recorded_at IS NULL
           ORDER BY updated_at, id LIMIT 1"#,
        columns("event")
    ))
    .fetch_optional(pool)
    .await
}

pub async fn mark_oversight_recorded(pool: &PgPool, event_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"UPDATE knowledge_event_outbox SET oversight_recorded_at = NOW(), updated_at = NOW()
           WHERE id = $1 AND status = 'failed' AND oversight_recorded_at IS NULL"#,
    )
    .bind(event_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn columns(alias: &str) -> String {
    [
        "id",
        "tenant_id",
        "descriptor_id",
        "event_kind",
        "publisher_bundle_id",
        "subject_kind",
        "subject_id",
        "subject_revision",
        "snapshot",
        "snapshot_hash",
        "source_title",
        "source_path_prefix",
        "acting_space_id",
        "output_vault_bundle",
        "knowledge_schema_id",
        "researcher_bundle_id",
        "researcher_role_id",
        "requested_by_user_id",
        "service_actor_user_id",
        "effective_role",
        "status",
        "attempt_count",
        "next_attempt_at",
        "lease_owner",
        "lease_expires_at",
        "last_error",
        "source_id",
        "knowledge_job_id",
        "created_at",
        "updated_at",
        "completed_at",
        "oversight_recorded_at",
    ]
    .into_iter()
    .map(|column| format!("{alias}.{column}"))
    .collect::<Vec<_>>()
    .join(", ")
}
