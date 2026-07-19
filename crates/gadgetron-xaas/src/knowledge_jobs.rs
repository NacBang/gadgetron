//! Durable Knowledge agent queue, artifact store and reviewed change sets.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    knowledge_spaces::{self as spaces, KnowledgeSpaceError, SpaceActor, SpaceRole},
    service_principals::{self, ServicePrincipalError},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeJobRole {
    SourceScout,
    Researcher,
    InsightSynthesizer,
    Gardener,
}

impl KnowledgeJobRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceScout => "source_scout",
            Self::Researcher => "researcher",
            Self::InsightSynthesizer => "insight_synthesizer",
            Self::Gardener => "gardener",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeJobKind {
    OnDemand,
    SourceIngest,
    Scheduled,
    FollowUp,
    Event,
}

impl KnowledgeJobKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::SourceIngest => "source_ingest",
            Self::Scheduled => "scheduled",
            Self::FollowUp => "follow_up",
            Self::Event => "event",
        }
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeJobRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub space_id: Uuid,
    pub output_vault_id: Uuid,
    pub role: String,
    pub kind: String,
    pub status: String,
    pub priority: i16,
    pub service_actor_user_id: Uuid,
    pub requested_by_user_id: Uuid,
    pub on_behalf_of_user_id: Option<Uuid>,
    pub input: Value,
    pub input_hash: String,
    pub idempotency_key: String,
    pub runtime_backend: String,
    pub runtime_model: String,
    pub runtime_effort: String,
    pub runtime_endpoint_id: Option<Uuid>,
    pub runtime_model_source: String,
    pub runtime_local_base_url: String,
    pub runtime_local_api_key_env: String,
    pub prompt_contract_revision: String,
    pub tool_policy_revision: String,
    pub role_profile_source: Option<String>,
    pub role_profile_ref: Option<String>,
    pub bundle_id: Option<String>,
    pub bundle_role_id: Option<String>,
    pub package_manifest_sha256: Option<String>,
    pub recipe_asset_id: Option<String>,
    pub recipe_sha256: Option<String>,
    pub max_tokens: i32,
    pub max_sources: i32,
    pub max_wall_seconds: i32,
    pub used_tokens: i32,
    pub used_sources: i32,
    pub progress_percent: i16,
    pub checkpoint: Value,
    pub attempt: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub terminal_reason: Option<String>,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeJobSourceRow {
    pub tenant_id: Uuid,
    pub job_id: Uuid,
    pub source_id: Uuid,
    pub source_revision: i64,
    pub content_hash: String,
    pub object_id: Uuid,
    pub object_revision: i64,
    pub object_content_hash: String,
    pub position: i16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct VerifiedOutcomeSnapshot {
    pub id: Uuid,
    pub experience_revision: String,
    pub consumer_bundle_id: String,
    pub subject_owner_bundle: String,
    pub subject_kind: String,
    pub subject_stable_id: String,
    pub subject_revision: String,
    pub operation_id: String,
    pub context_query_id: Option<String>,
    pub context_revision: Option<String>,
    pub predicate_result: String,
    pub verification_summary: String,
    pub used_citations: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeJobArtifactRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub job_id: Uuid,
    pub space_id: Uuid,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub payload: Value,
    pub citations: Value,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeChangeSetRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub job_id: Option<Uuid>,
    pub origin: String,
    pub space_id: Uuid,
    pub output_vault_id: Uuid,
    pub candidate_artifact_id: Option<Uuid>,
    pub status: String,
    pub title: String,
    pub summary: String,
    pub operations: Value,
    pub citations: Value,
    pub created_by_user_id: Uuid,
    pub decided_by_user_id: Option<Uuid>,
    pub decision_rationale: Option<String>,
    pub expected_git_revision: Option<String>,
    pub applied_git_revision: Option<String>,
    pub materialized_object_id: Option<Uuid>,
    pub materialization_receipt: Option<Value>,
    pub materialization_key: String,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub backend: String,
    #[serde(default)]
    pub model: String,
    pub effort: String,
    #[serde(default)]
    pub endpoint_id: Option<Uuid>,
    #[serde(default = "default_model_source")]
    pub model_source: String,
    #[serde(default)]
    pub local_base_url: String,
    #[serde(default)]
    pub local_api_key_env: String,
    pub prompt_contract_revision: String,
    pub tool_policy_revision: String,
    #[serde(default)]
    pub role_profile_source: Option<String>,
    #[serde(default)]
    pub role_profile_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRoleSnapshot {
    pub bundle_id: String,
    pub bundle_role_id: String,
    pub package_manifest_sha256: String,
    pub recipe_asset_id: String,
    pub recipe_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobBudget {
    pub max_tokens: i32,
    pub max_sources: i32,
    pub max_wall_seconds: i32,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i32,
}

const fn default_max_attempts() -> i32 {
    3
}

fn default_model_source() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnqueueKnowledgeJob {
    pub space_id: Uuid,
    pub output_vault_id: Uuid,
    pub role: KnowledgeJobRole,
    #[serde(default = "default_job_kind")]
    pub kind: KnowledgeJobKind,
    #[serde(default)]
    pub priority: i16,
    pub input: Value,
    pub idempotency_key: String,
    #[serde(default)]
    pub source_ids: Vec<Uuid>,
    pub runtime: RuntimeSnapshot,
    #[serde(default)]
    pub bundle_role: Option<BundleRoleSnapshot>,
    pub budget: JobBudget,
    #[serde(default)]
    pub scheduled_at: Option<DateTime<Utc>>,
}

const fn default_job_kind() -> KnowledgeJobKind {
    KnowledgeJobKind::OnDemand
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactInput {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub payload: Value,
    #[serde(default = "empty_array")]
    pub citations: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangeSetInput {
    #[serde(default)]
    pub candidate_artifact_id: Option<Uuid>,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub operations: Value,
    pub citations: Value,
    #[serde(default)]
    pub expected_git_revision: Option<String>,
    pub materialization_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EditChangeSet {
    pub expected_revision: i64,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub operations: Value,
    pub citations: Value,
    #[serde(skip)]
    pub expected_git_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSetDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginatingSubject {
    pub owner_bundle: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub subject_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializedObjectInput {
    pub id: Uuid,
    pub path: String,
    pub content_hash: String,
    pub expected_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_subject: Option<OriginatingSubject>,
}

impl ChangeSetDecision {
    const fn status(self) -> &'static str {
        match self {
            Self::Accept => "accepted",
            Self::Reject => "rejected",
        }
    }
}

fn empty_array() -> Value {
    Value::Array(Vec::new())
}

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeJobError {
    #[error("knowledge job persistence failed")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Space(#[from] spaces::KnowledgeSpaceError),
    #[error(transparent)]
    ServicePrincipal(#[from] ServicePrincipalError),
    #[error("knowledge job input is invalid: {0}")]
    InvalidInput(String),
    #[error("knowledge job or artifact was not found")]
    NotFound,
    #[error("knowledge job revision or state changed")]
    Conflict,
    #[error("knowledge job lease is not owned by this worker")]
    LeaseLost,
}

const JOB_COLUMNS: &str = r#"id, tenant_id, space_id, output_vault_id, role, kind,
    status, priority, service_actor_user_id, requested_by_user_id, on_behalf_of_user_id,
    input, input_hash, idempotency_key, runtime_backend, runtime_model, runtime_effort,
    runtime_endpoint_id, runtime_model_source, runtime_local_base_url, runtime_local_api_key_env,
    prompt_contract_revision, tool_policy_revision, role_profile_source, role_profile_ref,
    bundle_id, bundle_role_id, package_manifest_sha256, recipe_asset_id, recipe_sha256, max_tokens,
    max_sources, max_wall_seconds, used_tokens, used_sources, progress_percent,
    checkpoint, attempt, max_attempts, scheduled_at, lease_owner, lease_expires_at,
    heartbeat_at, cancel_requested_at, terminal_reason, revision, created_at, started_at,
    finished_at, updated_at"#;

const CHANGE_SET_COLUMNS: &str = r#"id, tenant_id, job_id, origin, space_id, output_vault_id,
    candidate_artifact_id, status, title, summary, operations, citations, created_by_user_id, decided_by_user_id,
    decision_rationale, expected_git_revision, applied_git_revision, materialized_object_id,
    materialization_receipt, materialization_key, revision, created_at, updated_at, decided_at,
    applied_at"#;

pub async fn enqueue(
    pool: &PgPool,
    actor: SpaceActor,
    request: EnqueueKnowledgeJob,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    validate_enqueue(&request)?;
    spaces::require_role(pool, actor, request.space_id, SpaceRole::Contributor, true).await?;
    let service = service_principals::ensure_knowledge_agent(pool, actor.tenant_id).await?;
    let mut tx = pool.begin().await?;
    require_output_vault(
        &mut tx,
        actor.tenant_id,
        request.space_id,
        request.output_vault_id,
    )
    .await?;
    let sources = pin_sources(
        &mut tx,
        actor.tenant_id,
        request.space_id,
        &request.source_ids,
        request.budget.max_sources,
    )
    .await?;
    let input_hash = json_hash(&request.input)?;
    let scheduled_at = request.scheduled_at.unwrap_or_else(Utc::now);
    let query = format!(
        r#"INSERT INTO knowledge_jobs
           (tenant_id, space_id, output_vault_id, role, kind, priority,
            service_actor_user_id, requested_by_user_id, on_behalf_of_user_id,
            input, input_hash, idempotency_key, runtime_backend, runtime_model,
            runtime_effort, runtime_endpoint_id, runtime_model_source, runtime_local_base_url,
            runtime_local_api_key_env, prompt_contract_revision, tool_policy_revision,
            role_profile_source, role_profile_ref, bundle_id, bundle_role_id,
            package_manifest_sha256, recipe_asset_id, recipe_sha256,
            max_tokens, max_sources, max_wall_seconds,
            max_attempts, scheduled_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32)
           ON CONFLICT (tenant_id, idempotency_key)
             WHERE status IN ('queued', 'running') DO NOTHING
           RETURNING {JOB_COLUMNS}"#
    );
    let inserted = sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(actor.tenant_id)
        .bind(request.space_id)
        .bind(request.output_vault_id)
        .bind(request.role.as_str())
        .bind(request.kind.as_str())
        .bind(request.priority)
        .bind(service.user_id)
        .bind(actor.user_id)
        .bind(request.input)
        .bind(input_hash)
        .bind(&request.idempotency_key)
        .bind(&request.runtime.backend)
        .bind(&request.runtime.model)
        .bind(&request.runtime.effort)
        .bind(request.runtime.endpoint_id)
        .bind(&request.runtime.model_source)
        .bind(&request.runtime.local_base_url)
        .bind(&request.runtime.local_api_key_env)
        .bind(&request.runtime.prompt_contract_revision)
        .bind(&request.runtime.tool_policy_revision)
        .bind(&request.runtime.role_profile_source)
        .bind(&request.runtime.role_profile_ref)
        .bind(
            request
                .bundle_role
                .as_ref()
                .map(|snapshot| &snapshot.bundle_id),
        )
        .bind(
            request
                .bundle_role
                .as_ref()
                .map(|snapshot| &snapshot.bundle_role_id),
        )
        .bind(
            request
                .bundle_role
                .as_ref()
                .map(|snapshot| &snapshot.package_manifest_sha256),
        )
        .bind(
            request
                .bundle_role
                .as_ref()
                .map(|snapshot| &snapshot.recipe_asset_id),
        )
        .bind(
            request
                .bundle_role
                .as_ref()
                .map(|snapshot| &snapshot.recipe_sha256),
        )
        .bind(request.budget.max_tokens)
        .bind(request.budget.max_sources)
        .bind(request.budget.max_wall_seconds)
        .bind(request.budget.max_attempts)
        .bind(scheduled_at)
        .fetch_optional(&mut *tx)
        .await?;
    let job = if let Some(job) = inserted {
        for source in sources {
            sqlx::query(
                r#"INSERT INTO knowledge_job_sources
                   (tenant_id, job_id, source_id, source_revision, content_hash,
                    object_id, object_revision, object_content_hash, position)
                   VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
            )
            .bind(actor.tenant_id)
            .bind(job.id)
            .bind(source.source_id)
            .bind(source.source_revision)
            .bind(source.content_hash)
            .bind(source.object_id)
            .bind(source.object_revision)
            .bind(source.object_content_hash)
            .bind(source.position)
            .execute(&mut *tx)
            .await?;
        }
        append_event(
            &mut tx,
            &job,
            service.user_id,
            "queued",
            "Knowledge job queued",
            serde_json::json!({"requested_by": actor.user_id}),
        )
        .await?;
        job
    } else {
        let query = format!(
            "SELECT {JOB_COLUMNS} FROM knowledge_jobs WHERE tenant_id = $1 AND idempotency_key = $2 AND status IN ('queued','running')"
        );
        sqlx::query_as::<_, KnowledgeJobRow>(&query)
            .bind(actor.tenant_id)
            .bind(&request.idempotency_key)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(KnowledgeJobError::Conflict)?
    };
    tx.commit().await?;
    Ok(job)
}

pub async fn validate_execution_actor(
    pool: &PgPool,
    actor: SpaceActor,
    job: &KnowledgeJobRow,
) -> Result<(), KnowledgeJobError> {
    let authority_user_id = job
        .on_behalf_of_user_id
        .unwrap_or(job.service_actor_user_id);
    if actor.tenant_id != job.tenant_id || actor.user_id != authority_user_id {
        return Err(KnowledgeJobError::NotFound);
    }
    service_principals::validate_knowledge_agent(pool, job.tenant_id, job.service_actor_user_id)
        .await?;
    let required = if job.role == "gardener" {
        SpaceRole::Contributor
    } else {
        SpaceRole::Viewer
    };
    spaces::require_role(pool, actor, job.space_id, required, true).await?;
    Ok(())
}

pub async fn list_for_space(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    limit: i64,
) -> Result<Vec<KnowledgeJobRow>, KnowledgeJobError> {
    require_visible_space(pool, actor, space_id).await?;
    let query = format!(
        "SELECT {JOB_COLUMNS} FROM knowledge_jobs WHERE tenant_id = $1 AND space_id = $2 ORDER BY created_at DESC, id DESC LIMIT $3"
    );
    Ok(sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(actor.tenant_id)
        .bind(space_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await?)
}

pub async fn get(
    pool: &PgPool,
    actor: SpaceActor,
    job_id: Uuid,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    let job = find_job(pool, actor.tenant_id, job_id)
        .await?
        .ok_or(KnowledgeJobError::NotFound)?;
    require_visible_space(pool, actor, job.space_id).await?;
    Ok(job)
}

pub async fn sources(
    pool: &PgPool,
    actor: SpaceActor,
    job_id: Uuid,
) -> Result<Vec<KnowledgeJobSourceRow>, KnowledgeJobError> {
    let job = get(pool, actor, job_id).await?;
    Ok(sqlx::query_as::<_, KnowledgeJobSourceRow>(
        r#"SELECT tenant_id, job_id, source_id, source_revision, content_hash,
                  object_id, object_revision, object_content_hash, position
           FROM knowledge_job_sources WHERE tenant_id = $1 AND job_id = $2 ORDER BY position"#,
    )
    .bind(actor.tenant_id)
    .bind(job.id)
    .fetch_all(pool)
    .await?)
}

pub async fn artifacts(
    pool: &PgPool,
    actor: SpaceActor,
    job_id: Uuid,
) -> Result<Vec<KnowledgeJobArtifactRow>, KnowledgeJobError> {
    let job = get(pool, actor, job_id).await?;
    Ok(sqlx::query_as::<_, KnowledgeJobArtifactRow>(
        r#"SELECT id, tenant_id, job_id, space_id, kind, title, summary, payload,
                  citations, content_hash, created_at
           FROM knowledge_job_artifacts WHERE tenant_id = $1 AND job_id = $2
           ORDER BY created_at, id"#,
    )
    .bind(actor.tenant_id)
    .bind(job.id)
    .fetch_all(pool)
    .await?)
}

pub async fn get_artifact(
    pool: &PgPool,
    actor: SpaceActor,
    artifact_id: Uuid,
) -> Result<KnowledgeJobArtifactRow, KnowledgeJobError> {
    let row = sqlx::query_as::<_, KnowledgeJobArtifactRow>(
        r#"SELECT id, tenant_id, job_id, space_id, kind, title, summary, payload,
                  citations, content_hash, created_at
           FROM knowledge_job_artifacts WHERE tenant_id = $1 AND id = $2"#,
    )
    .bind(actor.tenant_id)
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeJobError::NotFound)?;
    require_visible_space(pool, actor, row.space_id).await?;
    Ok(row)
}

pub async fn request_cancel(
    pool: &PgPool,
    actor: SpaceActor,
    job_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    let job = get(pool, actor, job_id).await?;
    spaces::require_role(pool, actor, job.space_id, SpaceRole::Contributor, true).await?;
    let mut tx = pool.begin().await?;
    let query = format!(
        r#"UPDATE knowledge_jobs SET
             cancel_requested_at = COALESCE(cancel_requested_at, NOW()),
             status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END,
             finished_at = CASE WHEN status = 'queued' THEN NOW() ELSE finished_at END,
             terminal_reason = CASE WHEN status = 'queued' THEN 'cancelled-by-user' ELSE terminal_reason END,
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status IN ('queued','running')
           RETURNING {JOB_COLUMNS}"#
    );
    let updated = sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(actor.tenant_id)
        .bind(job_id)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(KnowledgeJobError::Conflict)?;
    append_event(
        &mut tx,
        &updated,
        actor.user_id,
        if updated.status == "cancelled" {
            "cancelled"
        } else {
            "cancel_requested"
        },
        "Cancellation requested",
        Value::Object(Default::default()),
    )
    .await?;
    tx.commit().await?;
    Ok(updated)
}

pub async fn retry(
    pool: &PgPool,
    actor: SpaceActor,
    job_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    let job = get(pool, actor, job_id).await?;
    spaces::require_role(pool, actor, job.space_id, SpaceRole::Contributor, true).await?;
    let mut tx = pool.begin().await?;
    let query = format!(
        r#"UPDATE knowledge_jobs SET status = 'queued', scheduled_at = NOW(),
             lease_owner = NULL, lease_expires_at = NULL, heartbeat_at = NULL,
             cancel_requested_at = NULL, terminal_reason = NULL, finished_at = NULL,
             progress_percent = 0, revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status IN ('failed','cancelled') AND attempt < max_attempts
           RETURNING {JOB_COLUMNS}"#
    );
    let updated = sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(actor.tenant_id)
        .bind(job_id)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(KnowledgeJobError::Conflict)?;
    append_event(
        &mut tx,
        &updated,
        actor.user_id,
        "retry_queued",
        "Knowledge job queued for retry",
        Value::Object(Default::default()),
    )
    .await?;
    tx.commit().await?;
    Ok(updated)
}

pub async fn lease_next(
    pool: &PgPool,
    worker_id: &str,
    lease_seconds: i32,
) -> Result<Option<KnowledgeJobRow>, KnowledgeJobError> {
    if worker_id.is_empty() || worker_id.len() > 128 || !(5..=300).contains(&lease_seconds) {
        return Err(KnowledgeJobError::InvalidInput(
            "worker id or lease duration is outside the supported bounds".to_string(),
        ));
    }
    let mut tx = pool.begin().await?;
    let query = r#"WITH candidate AS (
             SELECT j.id FROM knowledge_jobs j
             JOIN tenants t ON t.id = j.tenant_id
              WHERE j.cancel_requested_at IS NULL AND j.attempt < j.max_attempts
                AND j.scheduled_at <= NOW()
                AND (
                  (j.status = 'running' AND j.lease_expires_at <= NOW())
                  OR (
                    j.status = 'queued'
                    AND (
                      SELECT COUNT(*) FROM knowledge_jobs active
                       WHERE active.tenant_id = j.tenant_id
                         AND active.status = 'running'
                         AND active.lease_expires_at > NOW()
                    ) < t.knowledge_job_concurrency_limit
                  )
                )
              ORDER BY j.priority DESC, j.scheduled_at, j.created_at, j.id
              FOR UPDATE OF j, t SKIP LOCKED LIMIT 1
           )
           UPDATE knowledge_jobs j SET status = 'running', lease_owner = $1,
             lease_expires_at = NOW() + make_interval(secs => $2), heartbeat_at = NOW(),
             attempt = attempt + 1, started_at = COALESCE(started_at, NOW()),
             revision = revision + 1, updated_at = NOW()
           FROM candidate WHERE j.id = candidate.id
           RETURNING j.*"#;
    let job = sqlx::query_as::<_, KnowledgeJobRow>(query)
        .bind(worker_id)
        .bind(lease_seconds as f64)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(job) = &job {
        append_event(
            &mut tx,
            job,
            job.service_actor_user_id,
            "leased",
            "Knowledge job leased",
            serde_json::json!({"worker_id": worker_id, "attempt": job.attempt}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(job)
}

#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatResult {
    pub job: KnowledgeJobRow,
    pub cancel_requested: bool,
    pub budget_exceeded: bool,
}

#[derive(Debug, Clone)]
pub struct HeartbeatUpdate {
    pub lease_seconds: i32,
    pub progress_percent: i16,
    pub checkpoint: Value,
    pub used_tokens: i32,
    pub used_sources: i32,
}

pub async fn heartbeat(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    update: HeartbeatUpdate,
) -> Result<HeartbeatResult, KnowledgeJobError> {
    if !update.checkpoint.is_object()
        || !(0..=100).contains(&update.progress_percent)
        || update.used_tokens < 0
        || update.used_sources < 0
    {
        return Err(KnowledgeJobError::InvalidInput(
            "heartbeat progress, usage or checkpoint is invalid".to_string(),
        ));
    }
    let current = owned_running_job(pool, job_id, worker_id).await?;
    if update.used_tokens > current.max_tokens || update.used_sources > current.max_sources {
        let job = terminal_transition(
            pool,
            job_id,
            worker_id,
            TerminalUpdate {
                status: "failed",
                reason: Some("job-budget-exceeded"),
                used_tokens: update.used_tokens,
                used_sources: update.used_sources,
                retryable: false,
            },
        )
        .await?;
        return Ok(HeartbeatResult {
            job,
            cancel_requested: false,
            budget_exceeded: true,
        });
    }
    let query = format!(
        r#"UPDATE knowledge_jobs SET heartbeat_at = NOW(),
             lease_expires_at = NOW() + make_interval(secs => $3),
             progress_percent = $4, checkpoint = $5, used_tokens = $6,
             used_sources = $7, revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND status = 'running' AND lease_owner = $2
             AND lease_expires_at > NOW()
           RETURNING {JOB_COLUMNS}"#
    );
    let job = sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(job_id)
        .bind(worker_id)
        .bind(update.lease_seconds as f64)
        .bind(update.progress_percent)
        .bind(update.checkpoint)
        .bind(update.used_tokens)
        .bind(update.used_sources)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::LeaseLost)?;
    Ok(HeartbeatResult {
        cancel_requested: job.cancel_requested_at.is_some(),
        budget_exceeded: false,
        job,
    })
}

pub async fn append_artifact(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    input: ArtifactInput,
) -> Result<KnowledgeJobArtifactRow, KnowledgeJobError> {
    validate_artifact(&input)?;
    let job = owned_running_job(pool, job_id, worker_id).await?;
    validate_citations(pool, &job, &input.citations).await?;
    let content_hash = json_hash(&serde_json::json!({
        "kind": input.kind,
        "title": input.title,
        "summary": input.summary,
        "payload": input.payload,
        "citations": input.citations,
    }))?;
    Ok(sqlx::query_as::<_, KnowledgeJobArtifactRow>(
        r#"INSERT INTO knowledge_job_artifacts
           (tenant_id, job_id, space_id, kind, title, summary, payload, citations, content_hash)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
           ON CONFLICT (job_id, kind, content_hash) DO UPDATE SET title = EXCLUDED.title
           RETURNING id, tenant_id, job_id, space_id, kind, title, summary, payload,
                     citations, content_hash, created_at"#,
    )
    .bind(job.tenant_id)
    .bind(job.id)
    .bind(job.space_id)
    .bind(input.kind)
    .bind(input.title)
    .bind(input.summary)
    .bind(input.payload)
    .bind(input.citations)
    .bind(content_hash)
    .fetch_one(pool)
    .await?)
}

pub async fn append_change_set(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    input: ChangeSetInput,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    validate_change_set(&input)?;
    let job = owned_running_job(pool, job_id, worker_id).await?;
    if job.role != "gardener" {
        return Err(KnowledgeJobError::InvalidInput(
            "only a Gardener job can create a change set".to_string(),
        ));
    }
    validate_citations(pool, &job, &input.citations).await?;
    if let Some(candidate_artifact_id) = input.candidate_artifact_id {
        let candidate_matches: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(
                   SELECT 1 FROM knowledge_job_artifacts
                   WHERE tenant_id = $1 AND space_id = $2 AND id = $3 AND kind = 'candidate'
               )"#,
        )
        .bind(job.tenant_id)
        .bind(job.space_id)
        .bind(candidate_artifact_id)
        .fetch_one(pool)
        .await?;
        let pinned = job
            .input
            .get("candidate_artifact_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok());
        if !candidate_matches || pinned != Some(candidate_artifact_id) {
            return Err(KnowledgeJobError::InvalidInput(
                "Gardener change set does not match its pinned Candidate".to_string(),
            ));
        }
    }
    let query = format!(
        r#"INSERT INTO knowledge_change_sets
           (tenant_id, job_id, space_id, output_vault_id, candidate_artifact_id, status,
            title, summary, operations, citations, created_by_user_id, expected_git_revision,
            materialization_key)
           VALUES ($1,$2,$3,$4,$5,'pending_user_review',$6,$7,$8,$9,$10,$11,$12)
           ON CONFLICT (job_id) DO UPDATE SET title = knowledge_change_sets.title
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    Ok(sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(job.tenant_id)
        .bind(job.id)
        .bind(job.space_id)
        .bind(job.output_vault_id)
        .bind(input.candidate_artifact_id)
        .bind(input.title)
        .bind(input.summary)
        .bind(input.operations)
        .bind(input.citations)
        .bind(job.service_actor_user_id)
        .bind(input.expected_git_revision)
        .bind(input.materialization_key)
        .fetch_one(pool)
        .await?)
}

pub async fn create_user_merge_change_set(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    output_vault_id: Uuid,
    input: ChangeSetInput,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    validate_change_set(&input)?;
    spaces::require_role(pool, actor, space_id, SpaceRole::Contributor, true).await?;
    let output_matches: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1 FROM knowledge_vaults
               WHERE tenant_id = $1 AND space_id = $2 AND id = $3
                 AND owner_state = 'enabled'
           )"#,
    )
    .bind(actor.tenant_id)
    .bind(space_id)
    .bind(output_vault_id)
    .fetch_one(pool)
    .await?;
    if !output_matches {
        return Err(KnowledgeJobError::InvalidInput(
            "user merge output Vault is unavailable".to_string(),
        ));
    }
    if !input.citations.as_array().is_some_and(Vec::is_empty) {
        return Err(KnowledgeJobError::InvalidInput(
            "user merge change sets cannot attach job citations".to_string(),
        ));
    }
    let query = format!(
        r#"INSERT INTO knowledge_change_sets
           (tenant_id, job_id, origin, space_id, output_vault_id, candidate_artifact_id,
            status, title, summary, operations, citations, created_by_user_id,
            expected_git_revision, materialization_key)
           VALUES ($1,NULL,'user',$2,$3,NULL,'pending_user_review',$4,$5,$6,$7,$8,$9,$10)
           ON CONFLICT (tenant_id, materialization_key)
           DO UPDATE SET title = knowledge_change_sets.title
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    let row = sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(space_id)
        .bind(output_vault_id)
        .bind(input.title.trim())
        .bind(input.summary)
        .bind(input.operations)
        .bind(input.citations)
        .bind(actor.user_id)
        .bind(input.expected_git_revision)
        .bind(input.materialization_key)
        .fetch_one(pool)
        .await?;
    if row.origin != "user"
        || row.space_id != space_id
        || row.output_vault_id != output_vault_id
        || row.created_by_user_id != actor.user_id
    {
        return Err(KnowledgeJobError::Conflict);
    }
    Ok(row)
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeEvolutionTrace {
    pub candidate: KnowledgeJobArtifactRow,
    pub change_set: Option<KnowledgeChangeSetRow>,
}

pub async fn evolution_for_space(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    limit: i64,
) -> Result<Vec<KnowledgeEvolutionTrace>, KnowledgeJobError> {
    require_visible_space(pool, actor, space_id).await?;
    let candidates = sqlx::query_as::<_, KnowledgeJobArtifactRow>(
        r#"SELECT id, tenant_id, job_id, space_id, kind, title, summary, payload,
                  citations, content_hash, created_at
           FROM knowledge_job_artifacts
           WHERE tenant_id = $1 AND space_id = $2 AND kind = 'candidate'
           ORDER BY created_at DESC, id DESC LIMIT $3"#,
    )
    .bind(actor.tenant_id)
    .bind(space_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let candidate_ids: Vec<_> = candidates.iter().map(|candidate| candidate.id).collect();
    let query = format!(
        "SELECT {CHANGE_SET_COLUMNS} FROM knowledge_change_sets \
         WHERE tenant_id = $1 AND candidate_artifact_id = ANY($2)"
    );
    let change_sets = sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(&candidate_ids)
        .fetch_all(pool)
        .await?;
    let mut by_candidate: HashMap<Uuid, KnowledgeChangeSetRow> = change_sets
        .into_iter()
        .filter_map(|change_set| {
            change_set
                .candidate_artifact_id
                .map(|candidate_id| (candidate_id, change_set))
        })
        .collect();
    Ok(candidates
        .into_iter()
        .map(|candidate| KnowledgeEvolutionTrace {
            change_set: by_candidate.remove(&candidate.id),
            candidate,
        })
        .collect())
}

pub async fn complete(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    used_tokens: i32,
    used_sources: i32,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    terminal_transition(
        pool,
        job_id,
        worker_id,
        TerminalUpdate {
            status: "succeeded",
            reason: None,
            used_tokens,
            used_sources,
            retryable: false,
        },
    )
    .await
}

pub async fn fail(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    reason: &str,
    retryable: bool,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    if reason.is_empty() || reason.len() > 1024 {
        return Err(KnowledgeJobError::InvalidInput(
            "terminal reason is empty or too long".to_string(),
        ));
    }
    terminal_transition(
        pool,
        job_id,
        worker_id,
        TerminalUpdate {
            status: "failed",
            reason: Some(reason),
            used_tokens: 0,
            used_sources: 0,
            retryable,
        },
    )
    .await
}

pub async fn list_change_sets(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    limit: i64,
) -> Result<Vec<KnowledgeChangeSetRow>, KnowledgeJobError> {
    require_visible_space(pool, actor, space_id).await?;
    let query = format!(
        "SELECT {CHANGE_SET_COLUMNS} FROM knowledge_change_sets WHERE tenant_id = $1 AND space_id = $2 ORDER BY created_at DESC, id DESC LIMIT $3"
    );
    Ok(sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(space_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await?)
}

pub async fn get_change_set(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    let query = format!(
        "SELECT {CHANGE_SET_COLUMNS} FROM knowledge_change_sets WHERE tenant_id = $1 AND id = $2"
    );
    let row = sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::NotFound)?;
    require_visible_space(pool, actor, row.space_id).await?;
    Ok(row)
}

async fn require_visible_space(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
) -> Result<(), KnowledgeJobError> {
    match spaces::require_role(pool, actor, space_id, SpaceRole::Viewer, false).await {
        Ok(_) => Ok(()),
        Err(KnowledgeSpaceError::NotFound | KnowledgeSpaceError::Forbidden) => {
            Err(KnowledgeJobError::NotFound)
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn edit_change_set(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
    input: EditChangeSet,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    validate_change_set_fields(
        &input.title,
        &input.summary,
        &input.operations,
        &input.citations,
    )?;
    if input
        .expected_git_revision
        .as_ref()
        .is_some_and(|revision| revision.is_empty() || revision.len() > 128)
    {
        return Err(KnowledgeJobError::InvalidInput(
            "expected Git revision is invalid".to_string(),
        ));
    }
    let current = get_change_set(pool, actor, change_set_id).await?;
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    if let Some(job_id) = current.job_id {
        let job = get(pool, actor, job_id).await?;
        validate_citations(pool, &job, &input.citations).await?;
    } else if !input.citations.as_array().is_some_and(Vec::is_empty) {
        return Err(KnowledgeJobError::InvalidInput(
            "user merge change sets cannot attach job citations".to_string(),
        ));
    }
    let query = format!(
        r#"UPDATE knowledge_change_sets SET title = $4, summary = $5,
             operations = $6, citations = $7, status = 'pending_user_review',
             expected_git_revision = $8, materialization_receipt = NULL,
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status IN ('pending_user_review', 'failed_retryable')
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .bind(input.expected_revision)
        .bind(input.title.trim())
        .bind(input.summary)
        .bind(input.operations)
        .bind(input.citations)
        .bind(input.expected_git_revision)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::Conflict)
}

pub async fn decide_change_set(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
    expected_revision: i64,
    decision: ChangeSetDecision,
    rationale: Option<&str>,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    if rationale.is_some_and(|value| value.len() > 2_000) {
        return Err(KnowledgeJobError::InvalidInput(
            "decision rationale is too long".to_string(),
        ));
    }
    let current = get_change_set(pool, actor, change_set_id).await?;
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    let query = format!(
        r#"UPDATE knowledge_change_sets SET status = $4, decided_by_user_id = $5,
             decision_rationale = $6, decided_at = NOW(), revision = revision + 1,
             updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status = 'pending_user_review'
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .bind(expected_revision)
        .bind(decision.status())
        .bind(actor.user_id)
        .bind(rationale.filter(|value| !value.trim().is_empty()))
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::Conflict)
}

pub async fn begin_materialization(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    let current = get_change_set(pool, actor, change_set_id).await?;
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    let query = format!(
        r#"UPDATE knowledge_change_sets SET status = 'materializing',
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status IN ('accepted', 'failed_retryable')
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .bind(expected_revision)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::Conflict)
}

pub async fn complete_materialization(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
    expected_revision: i64,
    git_revision: &str,
    objects: &[MaterializedObjectInput],
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    if git_revision.is_empty() || git_revision.len() > 128 || objects.is_empty() {
        return Err(KnowledgeJobError::InvalidInput(
            "materialization receipt is invalid".to_string(),
        ));
    }
    let current = get_change_set(pool, actor, change_set_id).await?;
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    for object in objects {
        if !is_materialized_note_path(&object.path, object.id)
            || !is_sha256(&object.content_hash)
            || object
                .expected_revision
                .is_some_and(|revision| revision < 1)
            || object
                .originating_subject
                .as_ref()
                .is_some_and(|subject| !valid_originating_subject(subject))
        {
            return Err(KnowledgeJobError::InvalidInput(
                "materialized object receipt is invalid".to_string(),
            ));
        }
    }
    let mut tx = pool.begin().await?;
    for object in objects {
        let touched = if let Some(expected_revision) = object.expected_revision {
            sqlx::query(
                r#"UPDATE knowledge_objects SET content_hash = $5,
                     originating_owner_bundle = COALESCE(originating_owner_bundle, $6),
                     originating_subject_kind = COALESCE(originating_subject_kind, $7),
                     originating_subject_id = COALESCE(originating_subject_id, $8),
                     originating_subject_revision = COALESCE(originating_subject_revision, $9),
                     revision = revision + 1, updated_at = NOW()
                   WHERE tenant_id = $1 AND vault_id = $2 AND id = $3
                     AND revision = $4 AND status = 'active'
                     AND (
                       $6::TEXT IS NULL OR originating_owner_bundle IS NULL OR (
                         originating_owner_bundle = $6
                         AND originating_subject_kind = $7
                         AND originating_subject_id = $8
                         AND originating_subject_revision = $9
                       )
                     )"#,
            )
            .bind(actor.tenant_id)
            .bind(current.output_vault_id)
            .bind(object.id)
            .bind(expected_revision)
            .bind(&object.content_hash)
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.owner_bundle),
            )
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.subject_kind),
            )
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.subject_id),
            )
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.subject_revision),
            )
            .execute(&mut *tx)
            .await?
            .rows_affected()
        } else {
            sqlx::query(
                r#"INSERT INTO knowledge_objects
                   (id, tenant_id, vault_id, canonical_kind, path, content_hash, created_by,
                    originating_owner_bundle, originating_subject_kind,
                    originating_subject_id, originating_subject_revision)
                   VALUES ($1,$2,$3,'note',$4,$5,$6,$7,$8,$9,$10)"#,
            )
            .bind(object.id)
            .bind(actor.tenant_id)
            .bind(current.output_vault_id)
            .bind(&object.path)
            .bind(&object.content_hash)
            .bind(actor.user_id)
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.owner_bundle),
            )
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.subject_kind),
            )
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.subject_id),
            )
            .bind(
                object
                    .originating_subject
                    .as_ref()
                    .map(|subject| &subject.subject_revision),
            )
            .execute(&mut *tx)
            .await?
            .rows_affected()
        };
        if touched != 1 {
            return Err(KnowledgeJobError::Conflict);
        }
    }
    let receipt = serde_json::json!({
        "git_revision": git_revision,
        "objects": objects,
    });
    let query = format!(
        r#"UPDATE knowledge_change_sets SET status = 'applied', applied_git_revision = $4,
             materialized_object_id = $5, materialization_receipt = $6, applied_at = NOW(),
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3 AND status = 'materializing'
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    let applied = sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .bind(expected_revision)
        .bind(git_revision)
        .bind(objects[0].id)
        .bind(receipt)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(KnowledgeJobError::Conflict)?;
    tx.commit().await?;
    Ok(applied)
}

pub async fn fail_materialization(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
    expected_revision: i64,
    detail: &str,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    if detail.is_empty() || detail.len() > 1_000 {
        return Err(KnowledgeJobError::InvalidInput(
            "materialization failure detail is invalid".to_string(),
        ));
    }
    let current = get_change_set(pool, actor, change_set_id).await?;
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    let query = format!(
        r#"UPDATE knowledge_change_sets SET status = 'failed_retryable',
             materialization_receipt = jsonb_build_object('error', $4::TEXT),
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3 AND status = 'materializing'
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .bind(expected_revision)
        .bind(detail)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::Conflict)
}

pub async fn refresh_materialization_retry(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
    expected_revision: i64,
    expected_git_revision: &str,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    if expected_git_revision.is_empty() || expected_git_revision.len() > 128 {
        return Err(KnowledgeJobError::InvalidInput(
            "expected Git revision is invalid".to_string(),
        ));
    }
    let current = get_change_set(pool, actor, change_set_id).await?;
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    let query = format!(
        r#"UPDATE knowledge_change_sets SET expected_git_revision = $4,
             materialization_receipt = NULL, revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status = 'failed_retryable'
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .bind(expected_revision)
        .bind(expected_git_revision)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::Conflict)
}

pub async fn return_materialization_to_review(
    pool: &PgPool,
    actor: SpaceActor,
    change_set_id: Uuid,
    expected_revision: i64,
    operations: Value,
    expected_git_revision: &str,
    detail: &str,
) -> Result<KnowledgeChangeSetRow, KnowledgeJobError> {
    if expected_git_revision.is_empty()
        || expected_git_revision.len() > 128
        || detail.is_empty()
        || detail.len() > 1_000
    {
        return Err(KnowledgeJobError::InvalidInput(
            "change-set review recovery is invalid".to_string(),
        ));
    }
    let current = get_change_set(pool, actor, change_set_id).await?;
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    validate_change_set_fields(
        &current.title,
        &current.summary,
        &operations,
        &current.citations,
    )?;
    let query = format!(
        r#"UPDATE knowledge_change_sets SET status = 'pending_user_review', operations = $4,
             expected_git_revision = $5,
             materialization_receipt = jsonb_build_object(
                 'error', $6::TEXT, 'recovery', 'review_required'),
             decided_by_user_id = NULL, decision_rationale = NULL, decided_at = NULL,
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status = 'failed_retryable'
           RETURNING {CHANGE_SET_COLUMNS}"#
    );
    sqlx::query_as::<_, KnowledgeChangeSetRow>(&query)
        .bind(actor.tenant_id)
        .bind(change_set_id)
        .bind(expected_revision)
        .bind(operations)
        .bind(expected_git_revision)
        .bind(detail)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::Conflict)
}

struct TerminalUpdate<'a> {
    status: &'a str,
    reason: Option<&'a str>,
    used_tokens: i32,
    used_sources: i32,
    retryable: bool,
}

async fn terminal_transition(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    update: TerminalUpdate<'_>,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    let mut tx = pool.begin().await?;
    let query = format!(
        r#"UPDATE knowledge_jobs SET
             status = CASE
               WHEN cancel_requested_at IS NOT NULL THEN 'cancelled'
               WHEN $4 AND attempt < max_attempts THEN 'queued'
               ELSE $3 END,
             scheduled_at = CASE WHEN $4 AND attempt < max_attempts
                                 THEN NOW() + make_interval(secs => LEAST(60, attempt * attempt))
                                 ELSE scheduled_at END,
             terminal_reason = CASE WHEN $4 AND attempt < max_attempts THEN $5 ELSE $5 END,
             used_tokens = GREATEST(used_tokens, $6),
             used_sources = GREATEST(used_sources, $7),
             progress_percent = CASE WHEN $3 = 'succeeded' THEN 100 ELSE progress_percent END,
             lease_owner = NULL, lease_expires_at = NULL, heartbeat_at = NULL,
             finished_at = CASE WHEN cancel_requested_at IS NOT NULL OR NOT ($4 AND attempt < max_attempts)
                                THEN NOW() ELSE NULL END,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND status = 'running' AND lease_owner = $2
             AND lease_expires_at > NOW()
           RETURNING {JOB_COLUMNS}"#
    );
    let job = sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(job_id)
        .bind(worker_id)
        .bind(update.status)
        .bind(update.retryable)
        .bind(update.reason)
        .bind(update.used_tokens)
        .bind(update.used_sources)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(KnowledgeJobError::LeaseLost)?;
    let event_kind = match job.status.as_str() {
        "queued" => "retry_queued",
        "succeeded" => "succeeded",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    append_event(
        &mut tx,
        &job,
        job.service_actor_user_id,
        event_kind,
        update.reason.unwrap_or("Knowledge job completed"),
        serde_json::json!({"attempt": job.attempt}),
    )
    .await?;
    tx.commit().await?;
    Ok(job)
}

async fn owned_running_job(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
) -> Result<KnowledgeJobRow, KnowledgeJobError> {
    let query = format!(
        "SELECT {JOB_COLUMNS} FROM knowledge_jobs WHERE id = $1 AND status = 'running' AND lease_owner = $2 AND lease_expires_at > NOW()"
    );
    sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(job_id)
        .bind(worker_id)
        .fetch_optional(pool)
        .await?
        .ok_or(KnowledgeJobError::LeaseLost)
}

async fn find_job(
    pool: &PgPool,
    tenant_id: Uuid,
    job_id: Uuid,
) -> Result<Option<KnowledgeJobRow>, sqlx::Error> {
    let query =
        format!("SELECT {JOB_COLUMNS} FROM knowledge_jobs WHERE tenant_id = $1 AND id = $2");
    sqlx::query_as::<_, KnowledgeJobRow>(&query)
        .bind(tenant_id)
        .bind(job_id)
        .fetch_optional(pool)
        .await
}

async fn require_output_vault(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    space_id: Uuid,
    vault_id: Uuid,
) -> Result<(), KnowledgeJobError> {
    let valid: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM knowledge_vaults
              WHERE tenant_id = $1 AND space_id = $2 AND id = $3 AND owner_state = 'enabled')"#,
    )
    .bind(tenant_id)
    .bind(space_id)
    .bind(vault_id)
    .fetch_one(&mut **tx)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(KnowledgeJobError::InvalidInput(
            "output Vault is not enabled in the selected Space".to_string(),
        ))
    }
}

async fn pin_sources(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    space_id: Uuid,
    source_ids: &[Uuid],
    max_sources: i32,
) -> Result<Vec<KnowledgeJobSourceRow>, KnowledgeJobError> {
    type SourcePinProjection = (i64, Option<String>, Uuid, i64, Option<String>);
    if source_ids.len() > max_sources as usize {
        return Err(KnowledgeJobError::InvalidInput(
            "source selection exceeds the job budget".to_string(),
        ));
    }
    let mut rows = Vec::with_capacity(source_ids.len());
    for (position, source_id) in source_ids.iter().enumerate() {
        let row: Option<SourcePinProjection> = sqlx::query_as(
            r#"SELECT s.revision, s.content_hash, o.id, o.revision, o.content_hash
                 FROM knowledge_sources s
                 JOIN knowledge_vaults v ON v.tenant_id = s.tenant_id AND v.id = s.vault_id
                 JOIN knowledge_objects o ON o.tenant_id = s.tenant_id
                    AND o.id = s.extracted_object_id AND o.status = 'active'
                WHERE s.tenant_id = $1 AND s.id = $2 AND v.space_id = $3
                  AND s.status = 'extracted' AND s.deleted_at IS NULL"#,
        )
        .bind(tenant_id)
        .bind(source_id)
        .bind(space_id)
        .fetch_optional(&mut **tx)
        .await?;
        let (source_revision, content_hash, object_id, object_revision, object_content_hash) = row
            .ok_or_else(|| {
                KnowledgeJobError::InvalidInput(
                    "a selected source is not an extracted source in this Space".to_string(),
                )
            })?;
        let content_hash = content_hash.ok_or_else(|| {
            KnowledgeJobError::InvalidInput(
                "a selected source has no canonical content hash".to_string(),
            )
        })?;
        let object_content_hash = object_content_hash.ok_or_else(|| {
            KnowledgeJobError::InvalidInput(
                "a selected source note has no canonical content hash".to_string(),
            )
        })?;
        rows.push(KnowledgeJobSourceRow {
            tenant_id,
            job_id: Uuid::nil(),
            source_id: *source_id,
            source_revision,
            content_hash,
            object_id,
            object_revision,
            object_content_hash,
            position: position as i16,
        });
    }
    Ok(rows)
}

async fn validate_citations(
    pool: &PgPool,
    job: &KnowledgeJobRow,
    citations: &Value,
) -> Result<(), KnowledgeJobError> {
    let entries = citations
        .as_array()
        .ok_or_else(|| KnowledgeJobError::InvalidInput("citations must be an array".to_string()))?;
    let allowed: Vec<Uuid> = sqlx::query_scalar(
        "SELECT source_id FROM knowledge_job_sources WHERE tenant_id = $1 AND job_id = $2",
    )
    .bind(job.tenant_id)
    .bind(job.id)
    .fetch_all(pool)
    .await?;
    for entry in entries {
        let source_id = entry
            .get("source_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| {
                KnowledgeJobError::InvalidInput(
                    "every citation must contain a source_id UUID".to_string(),
                )
            })?;
        if !allowed.contains(&source_id) {
            return Err(KnowledgeJobError::InvalidInput(
                "citation references a source outside the pinned job set".to_string(),
            ));
        }
    }
    Ok(())
}

async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    job: &KnowledgeJobRow,
    actor_user_id: Uuid,
    event_kind: &str,
    summary: &str,
    details: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO knowledge_job_events
           (tenant_id, job_id, event_kind, actor_user_id, summary, details)
           VALUES ($1,$2,$3,$4,$5,$6)"#,
    )
    .bind(job.tenant_id)
    .bind(job.id)
    .bind(event_kind)
    .bind(actor_user_id)
    .bind(summary)
    .bind(details)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_enqueue(request: &EnqueueKnowledgeJob) -> Result<(), KnowledgeJobError> {
    if !request.input.is_object() {
        return Err(KnowledgeJobError::InvalidInput(
            "job input must be an object".to_string(),
        ));
    }
    if request.idempotency_key.is_empty() || request.idempotency_key.len() > 200 {
        return Err(KnowledgeJobError::InvalidInput(
            "idempotency key is empty or too long".to_string(),
        ));
    }
    if request.source_ids.is_empty() && request.role != KnowledgeJobRole::SourceScout {
        return Err(KnowledgeJobError::InvalidInput(
            "Knowledge evolution jobs require at least one pinned source".to_string(),
        ));
    }
    if !matches!(
        request.runtime.backend.as_str(),
        "claude_code" | "codex_exec"
    ) || !matches!(
        request.runtime.effort.as_str(),
        "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    ) {
        return Err(KnowledgeJobError::InvalidInput(
            "runtime backend or effort is unsupported".to_string(),
        ));
    }
    if !matches!(request.runtime.model_source.as_str(), "default" | "local")
        || (request.runtime.model_source == "local") != request.runtime.endpoint_id.is_some()
        || request.runtime.model.len() > 200
        || request.runtime.local_base_url.len() > 2_048
        || request.runtime.local_api_key_env.len() > 128
    {
        return Err(KnowledgeJobError::InvalidInput(
            "runtime model source or endpoint snapshot is invalid".to_string(),
        ));
    }
    match (
        request.runtime.role_profile_source.as_deref(),
        request.runtime.role_profile_ref.as_deref(),
    ) {
        (None, None) => {}
        (Some(source), Some(profile_ref))
            if matches!(source, "global" | "core" | "bundle") && is_lower_sha256(profile_ref) => {}
        _ => {
            return Err(KnowledgeJobError::InvalidInput(
                "role profile snapshot is incomplete or invalid".to_string(),
            ))
        }
    }
    if let Some(snapshot) = &request.bundle_role {
        if request.runtime.role_profile_ref.is_none()
            || snapshot.bundle_id.is_empty()
            || snapshot.bundle_id.len() > 128
            || snapshot.bundle_role_id.is_empty()
            || snapshot.bundle_role_id.len() > 128
            || snapshot.recipe_asset_id.is_empty()
            || snapshot.recipe_asset_id.len() > 128
            || !is_lower_sha256(&snapshot.package_manifest_sha256)
            || !is_lower_sha256(&snapshot.recipe_sha256)
        {
            return Err(KnowledgeJobError::InvalidInput(
                "Bundle AI role snapshot is invalid".to_string(),
            ));
        }
    }
    if request.runtime.role_profile_source.as_deref() == Some("bundle")
        && request.bundle_role.is_none()
    {
        return Err(KnowledgeJobError::InvalidInput(
            "a Bundle role profile requires a signed Bundle role snapshot".to_string(),
        ));
    }
    if request.runtime.prompt_contract_revision.is_empty()
        || request.runtime.prompt_contract_revision.len() > 64
        || request.runtime.tool_policy_revision.is_empty()
        || request.runtime.tool_policy_revision.len() > 256
        || !(256..=200_000).contains(&request.budget.max_tokens)
        || !(1..=100).contains(&request.budget.max_sources)
        || !(5..=3_600).contains(&request.budget.max_wall_seconds)
        || !(1..=10).contains(&request.budget.max_attempts)
        || !(-100..=100).contains(&request.priority)
    {
        return Err(KnowledgeJobError::InvalidInput(
            "runtime revision, priority or budget is outside supported bounds".to_string(),
        ));
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_artifact(input: &ArtifactInput) -> Result<(), KnowledgeJobError> {
    if !matches!(
        input.kind.as_str(),
        "source_proposal" | "dossier" | "partial_dossier" | "candidate" | "agent_output"
    ) || input.title.trim().is_empty()
        || input.title.len() > 300
        || input.summary.len() > 4_000
        || !input.payload.is_object()
        || !input.citations.is_array()
    {
        return Err(KnowledgeJobError::InvalidInput(
            "artifact shape is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_change_set(input: &ChangeSetInput) -> Result<(), KnowledgeJobError> {
    validate_change_set_fields(
        &input.title,
        &input.summary,
        &input.operations,
        &input.citations,
    )?;
    if input.materialization_key.is_empty() || input.materialization_key.len() > 200 {
        return Err(KnowledgeJobError::InvalidInput(
            "change-set shape is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_change_set_fields(
    title: &str,
    summary: &str,
    operations: &Value,
    citations: &Value,
) -> Result<(), KnowledgeJobError> {
    if title.trim().is_empty()
        || title.len() > 300
        || summary.len() > 4_000
        || !operations.is_array()
        || operations.as_array().map_or(true, Vec::is_empty)
        || !citations.is_array()
    {
        return Err(KnowledgeJobError::InvalidInput(
            "change-set shape is invalid".to_string(),
        ));
    }
    for operation in operations.as_array().into_iter().flatten() {
        if !matches!(
            operation.get("op").and_then(Value::as_str),
            Some("create_note" | "update_note" | "link" | "merge_notes" | "split_note")
        ) {
            return Err(KnowledgeJobError::InvalidInput(
                "change-set operation is not allowed".to_string(),
            ));
        }
    }
    Ok(())
}

fn json_hash(value: &Value) -> Result<String, KnowledgeJobError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        KnowledgeJobError::InvalidInput(format!("JSON cannot be hashed: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_originating_subject(subject: &OriginatingSubject) -> bool {
    let bundle = subject.owner_bundle.as_bytes();
    let kind = subject.subject_kind.as_bytes();
    (2..=64).contains(&bundle.len())
        && bundle[0].is_ascii_lowercase()
        && bundle[bundle.len() - 1].is_ascii_alphanumeric()
        && bundle
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && (2..=128).contains(&kind.len())
        && kind[0].is_ascii_lowercase()
        && kind[kind.len() - 1].is_ascii_alphanumeric()
        && kind.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        })
        && (1..=256).contains(&subject.subject_id.len())
        && (1..=256).contains(&subject.subject_revision.len())
        && !subject.subject_id.chars().any(char::is_control)
        && !subject.subject_revision.chars().any(char::is_control)
}

fn is_materialized_note_path(path: &str, id: Uuid) -> bool {
    if path == format!("notes/{id}.md") {
        return true;
    }
    let Some(file_name) = path
        .strip_prefix("notes/")
        .and_then(|value| value.strip_suffix(".md"))
    else {
        return false;
    };
    if file_name.is_empty() || file_name.contains('/') || file_name.chars().count() > 96 {
        return false;
    }
    let id = id.simple().to_string();
    let Some(slug) = file_name.strip_suffix(&format!("--{}", &id[..8])) else {
        return false;
    };
    !slug.is_empty()
        && slug
            .chars()
            .all(|character| character.is_alphanumeric() || character == '-')
}
