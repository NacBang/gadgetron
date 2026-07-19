//! Durable Core authority for acting-context-bound autonomous duty cycles.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    knowledge_spaces::{self as spaces, SpaceActor, SpaceRole},
    service_principals::{self, ServicePrincipalError},
};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutonomyGoalRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub goal_key: String,
    pub source_kind: String,
    pub status: String,
    pub context_state: String,
    pub goal: String,
    pub owner_bundle_id: String,
    pub recipe_id: String,
    pub package_manifest_sha256: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_revision: String,
    pub target_label: String,
    pub event_kind: Option<String>,
    pub subject_bundle_id: Option<String>,
    pub subject_kind: Option<String>,
    pub event_payload: Value,
    pub agent_role_id: Option<String>,
    pub result_gadget: Option<String>,
    pub agent_profile_snapshot: Value,
    pub acting_space_id: Option<Uuid>,
    pub requested_by_user_id: Option<Uuid>,
    pub service_actor_user_id: Option<Uuid>,
    pub effective_role: Option<String>,
    pub last_policy_revision: Option<String>,
    pub interval_seconds: i32,
    pub max_wall_seconds: i32,
    pub attempt: i32,
    pub max_attempts: i32,
    pub next_run_at: DateTime<Utc>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub checkpoint: Value,
    pub last_outcome: Option<String>,
    pub last_verification: Option<String>,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_finished_at: Option<DateTime<Utc>>,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutonomyRunRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub goal_id: Uuid,
    pub attempt: i32,
    pub status: String,
    pub worker_id: String,
    pub acting_space_id: Uuid,
    pub acting_space_revision: i64,
    pub requested_by_user_id: Uuid,
    pub service_actor_user_id: Uuid,
    pub effective_role: String,
    pub package_manifest_sha256: String,
    pub target_revision: String,
    pub agent_profile_snapshot: Value,
    pub policy_revision: Option<String>,
    pub context_snapshot: Value,
    pub checkpoint: Value,
    pub runtime_job_id: Option<String>,
    pub outcome: Option<String>,
    pub verification_state: Option<String>,
    pub verification_summary: Option<String>,
    pub evidence_refs: Value,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SyncBundleSchedule {
    pub goal_key: String,
    pub goal: String,
    pub tenant_id: Uuid,
    pub owner_bundle_id: String,
    pub recipe_id: String,
    pub package_manifest_sha256: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_revision: String,
    pub target_label: String,
    pub acting_space_id: Option<Uuid>,
    pub requested_by_user_id: Option<Uuid>,
    pub interval: Duration,
    pub max_wall_time: Duration,
    pub max_attempts: i32,
}

#[derive(Debug, Clone)]
pub struct EnqueueBundleEvent {
    pub tenant_id: Uuid,
    pub event_kind: String,
    pub subject_bundle_id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub subject_revision: String,
    pub event_payload: Value,
    pub owner_bundle_id: String,
    pub recipe_id: String,
    pub package_manifest_sha256: String,
    pub agent_role_id: String,
    pub result_gadget: String,
    pub goal: String,
    pub acting_space_id: Uuid,
    pub requested_by_user_id: Uuid,
    pub service_actor_user_id: Uuid,
    pub effective_role: String,
    pub max_wall_seconds: i32,
    pub max_attempts: i32,
    pub agent_profile_snapshot: Value,
}

#[derive(Debug, Clone)]
pub struct EnqueuedBundleEvent {
    pub goal: AutonomyGoalRow,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleEventProjectionSubject {
    pub id: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BundleEventProjectionState {
    pub id: String,
    pub revision: String,
    pub status: String,
}

#[derive(Debug, Clone, Copy)]
pub struct BundleEventProjectionQuery<'a> {
    pub tenant_id: Uuid,
    pub owner_bundle_id: &'a str,
    pub package_manifest_sha256: &'a str,
    pub subject_bundle_id: &'a str,
    pub subject_kind: &'a str,
    pub event_kind: &'a str,
    pub agent_role_id: &'a str,
    pub subjects: &'a [BundleEventProjectionSubject],
}

#[derive(Debug, Clone)]
pub struct AutonomyLease {
    pub goal: AutonomyGoalRow,
    pub run: AutonomyRunRow,
}

#[derive(Debug, Clone)]
pub struct RunFinish {
    pub outcome: String,
    pub verification_state: String,
    pub verification_summary: String,
    pub evidence_refs: Vec<String>,
    pub disposition: RunDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDisposition {
    Succeeded,
    RetryableFailure,
    SafeStopped,
    ContextRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventRunTerminal {
    Succeeded,
    ProviderFailure,
    PolicyFailure,
    StaleSubject,
}

#[derive(Debug, thiserror::Error)]
pub enum AutonomyError {
    #[error("invalid autonomous goal input: {0}")]
    InvalidInput(String),
    #[error("autonomous goal was not found")]
    NotFound,
    #[error("autonomous goal lease was lost")]
    LeaseLost,
    #[error("autonomous goal revision changed")]
    Conflict,
    #[error("autonomous acting context is no longer authorized")]
    ContextForbidden,
    #[error("autonomous signed recipe, target, or acting context changed")]
    ExecutionSnapshotChanged,
    #[error("autonomous goal persistence failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("autonomous service identity failed: {0}")]
    ServicePrincipal(#[from] ServicePrincipalError),
}

const GOAL_COLUMNS: &str = r#"id, tenant_id, goal_key, source_kind, status, context_state,
    goal, owner_bundle_id, recipe_id, package_manifest_sha256, target_kind, target_id,
    target_revision, target_label, event_kind, subject_bundle_id, subject_kind, event_payload,
    agent_role_id, result_gadget, agent_profile_snapshot,
    acting_space_id, requested_by_user_id, service_actor_user_id,
    effective_role, last_policy_revision, interval_seconds, max_wall_seconds, attempt,
    max_attempts, next_run_at, lease_owner, lease_expires_at, heartbeat_at, checkpoint,
    last_outcome, last_verification, last_started_at, last_finished_at, revision,
    created_at, updated_at"#;

const RUN_COLUMNS: &str = r#"id, tenant_id, goal_id, attempt, status, worker_id,
    acting_space_id, acting_space_revision, requested_by_user_id, service_actor_user_id,
    effective_role, package_manifest_sha256, target_revision, policy_revision,
    agent_profile_snapshot, context_snapshot, checkpoint, runtime_job_id, outcome, verification_state,
    verification_summary, evidence_refs, started_at, finished_at, updated_at"#;

#[derive(sqlx::FromRow)]
struct ExistingScheduleRow {
    id: Uuid,
    status: String,
    context_state: String,
    acting_space_id: Option<Uuid>,
    service_actor_user_id: Option<Uuid>,
}

pub async fn sync_bundle_schedule(
    pool: &PgPool,
    input: SyncBundleSchedule,
) -> Result<AutonomyGoalRow, AutonomyError> {
    validate_schedule(&input)?;
    let existing: Option<ExistingScheduleRow> = sqlx::query_as(
        "SELECT id, status, context_state, acting_space_id, service_actor_user_id FROM autonomy_goals WHERE tenant_id = $1 AND goal_key = $2",
    )
    .bind(input.tenant_id)
    .bind(&input.goal_key)
    .fetch_optional(pool)
    .await?;
    let goal_id = existing.as_ref().map_or_else(Uuid::new_v4, |row| row.id);
    let allow_service_grant = existing.as_ref().map_or(true, |row| {
        row.service_actor_user_id.is_none() || row.acting_space_id != input.acting_space_id
    });
    let existing_service_actor = existing.as_ref().and_then(|row| row.service_actor_user_id);
    let context = resolve_context(
        pool,
        goal_id,
        &input,
        existing_service_actor,
        allow_service_grant,
    )
    .await?;
    let desired_status = if context.state == "ready" {
        "ready"
    } else {
        "context_required"
    };
    let interval_seconds = i32::try_from(input.interval.as_secs()).map_err(|_| {
        AutonomyError::InvalidInput("schedule interval exceeds the supported range".into())
    })?;
    let max_wall_seconds = i32::try_from(input.max_wall_time.as_secs()).map_err(|_| {
        AutonomyError::InvalidInput("wall-time budget exceeds the supported range".into())
    })?;
    let query = format!(
        r#"INSERT INTO autonomy_goals
           (id, tenant_id, goal_key, source_kind, status, context_state, goal,
            owner_bundle_id, recipe_id, package_manifest_sha256, target_kind, target_id,
            target_revision, target_label, acting_space_id, requested_by_user_id, service_actor_user_id,
            effective_role, interval_seconds, max_wall_seconds, max_attempts)
           VALUES ($1,$2,$3,'bundle_schedule',$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
           ON CONFLICT (tenant_id, goal_key) DO UPDATE SET
             status = CASE
               WHEN autonomy_goals.status = 'running' THEN 'running'
               WHEN EXCLUDED.status = 'context_required' THEN 'context_required'
               WHEN autonomy_goals.status IN ('paused','safe_stopped') THEN autonomy_goals.status
               ELSE EXCLUDED.status END,
             context_state = EXCLUDED.context_state,
             goal = EXCLUDED.goal,
             owner_bundle_id = EXCLUDED.owner_bundle_id,
             recipe_id = EXCLUDED.recipe_id,
             package_manifest_sha256 = EXCLUDED.package_manifest_sha256,
             target_kind = EXCLUDED.target_kind,
             target_id = EXCLUDED.target_id,
             target_revision = EXCLUDED.target_revision,
             target_label = EXCLUDED.target_label,
             acting_space_id = EXCLUDED.acting_space_id,
             requested_by_user_id = EXCLUDED.requested_by_user_id,
             service_actor_user_id = EXCLUDED.service_actor_user_id,
             effective_role = EXCLUDED.effective_role,
             interval_seconds = EXCLUDED.interval_seconds,
             max_wall_seconds = EXCLUDED.max_wall_seconds,
             max_attempts = EXCLUDED.max_attempts,
             next_run_at = CASE
               WHEN autonomy_goals.context_state <> EXCLUDED.context_state
                 OR autonomy_goals.target_revision <> EXCLUDED.target_revision
                 OR autonomy_goals.package_manifest_sha256 <> EXCLUDED.package_manifest_sha256
               THEN NOW() ELSE autonomy_goals.next_run_at END,
             attempt = CASE
               WHEN autonomy_goals.target_revision <> EXCLUDED.target_revision
                 OR autonomy_goals.package_manifest_sha256 <> EXCLUDED.package_manifest_sha256
               THEN 0 ELSE autonomy_goals.attempt END,
             revision = autonomy_goals.revision + 1,
             updated_at = NOW()
           WHERE autonomy_goals.status IS DISTINCT FROM CASE
                   WHEN autonomy_goals.status = 'running' THEN 'running'
                   WHEN EXCLUDED.status = 'context_required' THEN 'context_required'
                   WHEN autonomy_goals.status IN ('paused','safe_stopped') THEN autonomy_goals.status
                   ELSE EXCLUDED.status END
              OR autonomy_goals.context_state IS DISTINCT FROM EXCLUDED.context_state
              OR autonomy_goals.goal IS DISTINCT FROM EXCLUDED.goal
              OR autonomy_goals.owner_bundle_id IS DISTINCT FROM EXCLUDED.owner_bundle_id
              OR autonomy_goals.recipe_id IS DISTINCT FROM EXCLUDED.recipe_id
              OR autonomy_goals.package_manifest_sha256 IS DISTINCT FROM EXCLUDED.package_manifest_sha256
              OR autonomy_goals.target_kind IS DISTINCT FROM EXCLUDED.target_kind
              OR autonomy_goals.target_id IS DISTINCT FROM EXCLUDED.target_id
              OR autonomy_goals.target_revision IS DISTINCT FROM EXCLUDED.target_revision
              OR autonomy_goals.target_label IS DISTINCT FROM EXCLUDED.target_label
              OR autonomy_goals.acting_space_id IS DISTINCT FROM EXCLUDED.acting_space_id
              OR autonomy_goals.requested_by_user_id IS DISTINCT FROM EXCLUDED.requested_by_user_id
              OR autonomy_goals.service_actor_user_id IS DISTINCT FROM EXCLUDED.service_actor_user_id
              OR autonomy_goals.effective_role IS DISTINCT FROM EXCLUDED.effective_role
              OR autonomy_goals.interval_seconds IS DISTINCT FROM EXCLUDED.interval_seconds
              OR autonomy_goals.max_wall_seconds IS DISTINCT FROM EXCLUDED.max_wall_seconds
              OR autonomy_goals.max_attempts IS DISTINCT FROM EXCLUDED.max_attempts
           RETURNING {GOAL_COLUMNS}"#,
    );
    let mut tx = pool.begin().await?;
    let updated = sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(goal_id)
        .bind(input.tenant_id)
        .bind(&input.goal_key)
        .bind(desired_status)
        .bind(context.state)
        .bind(&input.goal)
        .bind(&input.owner_bundle_id)
        .bind(&input.recipe_id)
        .bind(&input.package_manifest_sha256)
        .bind(&input.target_kind)
        .bind(&input.target_id)
        .bind(&input.target_revision)
        .bind(&input.target_label)
        .bind(context.space_id)
        .bind(context.requested_by)
        .bind(context.service_actor)
        .bind(context.effective_role)
        .bind(interval_seconds)
        .bind(max_wall_seconds)
        .bind(input.max_attempts)
        .fetch_optional(&mut *tx)
        .await?;
    let was_updated = updated.is_some();
    let goal = match updated {
        Some(goal) => goal,
        None => {
            let select = format!(
                "SELECT {GOAL_COLUMNS} FROM autonomy_goals WHERE tenant_id = $1 AND goal_key = $2"
            );
            sqlx::query_as::<_, AutonomyGoalRow>(&select)
                .bind(input.tenant_id)
                .bind(&input.goal_key)
                .fetch_one(&mut *tx)
                .await?
        }
    };
    let changed = was_updated
        && existing.as_ref().map_or(true, |row| {
            row.status != goal.status || row.context_state != goal.context_state
        });
    if changed {
        append_event(
            &mut tx,
            &goal,
            None,
            context.requested_by,
            if goal.context_state == "ready" {
                "ready"
            } else {
                "context_required"
            },
            if goal.context_state == "ready" {
                "Autonomous goal is ready"
            } else {
                "Autonomous goal needs an operating context"
            },
            serde_json::json!({"context_state": goal.context_state}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(goal)
}

pub async fn enqueue_bundle_event_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    input: EnqueueBundleEvent,
) -> Result<EnqueuedBundleEvent, AutonomyError> {
    validate_bundle_event(&input)?;
    let mut identity = Sha256::new();
    for value in [
        input.event_kind.as_str(),
        input.subject_bundle_id.as_str(),
        input.subject_kind.as_str(),
        input.subject_id.as_str(),
        input.subject_revision.as_str(),
        input.owner_bundle_id.as_str(),
        input.agent_role_id.as_str(),
    ] {
        identity.update(value.as_bytes());
        identity.update([0]);
    }
    let goal_key = format!("bundle-event:{}", hex::encode(identity.finalize()));
    let target_label = format!("{} {}", input.subject_kind, input.subject_id)
        .chars()
        .take(300)
        .collect::<String>();
    let query = format!(
        r#"INSERT INTO autonomy_goals
           (id, tenant_id, goal_key, source_kind, status, context_state, goal,
            owner_bundle_id, recipe_id, package_manifest_sha256, target_kind, target_id,
            target_revision, target_label, acting_space_id, requested_by_user_id,
            service_actor_user_id, effective_role, interval_seconds, max_wall_seconds,
            max_attempts, event_kind, subject_bundle_id, subject_kind, event_payload,
            agent_role_id, result_gadget, agent_profile_snapshot)
           VALUES ($1,$2,$3,'bundle_event','ready','ready',$4,$5,$6,$7,$8,$9,$10,$11,
                   $12,$13,$14,$15,10,$16,$17,$18,$19,$20,$21,$22,$23,$24)
           ON CONFLICT DO NOTHING
           RETURNING {GOAL_COLUMNS}"#,
    );
    let inserted = sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(Uuid::new_v4())
        .bind(input.tenant_id)
        .bind(&goal_key)
        .bind(&input.goal)
        .bind(&input.owner_bundle_id)
        .bind(&input.recipe_id)
        .bind(&input.package_manifest_sha256)
        .bind(&input.subject_kind)
        .bind(&input.subject_id)
        .bind(&input.subject_revision)
        .bind(target_label)
        .bind(input.acting_space_id)
        .bind(input.requested_by_user_id)
        .bind(input.service_actor_user_id)
        .bind(&input.effective_role)
        .bind(input.max_wall_seconds)
        .bind(input.max_attempts)
        .bind(&input.event_kind)
        .bind(&input.subject_bundle_id)
        .bind(&input.subject_kind)
        .bind(&input.event_payload)
        .bind(&input.agent_role_id)
        .bind(&input.result_gadget)
        .bind(&input.agent_profile_snapshot)
        .fetch_optional(&mut **tx)
        .await?;
    let created = inserted.is_some();
    let goal = match inserted {
        Some(goal) => goal,
        None => {
            let select = format!(
                r#"SELECT {GOAL_COLUMNS} FROM autonomy_goals
                   WHERE tenant_id = $1 AND source_kind = 'bundle_event'
                     AND subject_bundle_id = $2 AND subject_kind = $3 AND target_id = $4
                     AND target_revision = $5 AND owner_bundle_id = $6 AND agent_role_id = $7"#,
            );
            sqlx::query_as::<_, AutonomyGoalRow>(&select)
                .bind(input.tenant_id)
                .bind(&input.subject_bundle_id)
                .bind(&input.subject_kind)
                .bind(&input.subject_id)
                .bind(&input.subject_revision)
                .bind(&input.owner_bundle_id)
                .bind(&input.agent_role_id)
                .fetch_one(&mut **tx)
                .await?
        }
    };
    if created {
        append_event(
            tx,
            &goal,
            None,
            Some(input.service_actor_user_id),
            "ready",
            "Signed Bundle event enrichment is ready",
            serde_json::json!({
                "event_kind": input.event_kind,
                "subject_bundle_id": input.subject_bundle_id,
                "subject_kind": input.subject_kind,
                "subject_id": input.subject_id,
                "subject_revision": input.subject_revision,
                "agent_role_id": input.agent_role_id,
            }),
        )
        .await?;
    }
    Ok(EnqueuedBundleEvent { goal, created })
}

pub async fn bundle_event_projection_states(
    pool: &PgPool,
    input: BundleEventProjectionQuery<'_>,
) -> Result<Vec<BundleEventProjectionState>, AutonomyError> {
    if !is_sha256(input.package_manifest_sha256)
        || input.subjects.len() > 200
        || input.subjects.iter().any(|subject| {
            subject.id.is_empty()
                || subject.id.len() > 256
                || subject.id.chars().any(char::is_control)
                || !is_sha256(&subject.revision)
        })
    {
        return Err(AutonomyError::InvalidInput(
            "Bundle event projection subjects are outside the supported bounds".into(),
        ));
    }
    if input.subjects.is_empty() {
        return Ok(Vec::new());
    }
    let subject_ids = input
        .subjects
        .iter()
        .map(|subject| subject.id.as_str())
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, BundleEventProjectionState>(
        r#"SELECT target_id AS id, target_revision AS revision, status
           FROM autonomy_goals
           WHERE tenant_id = $1 AND source_kind = 'bundle_event'
             AND owner_bundle_id = $2 AND package_manifest_sha256 = $3
             AND subject_bundle_id = $4 AND subject_kind = $5
             AND event_kind = $6 AND agent_role_id = $7
             AND target_id = ANY($8)
           ORDER BY updated_at DESC, id"#,
    )
    .bind(input.tenant_id)
    .bind(input.owner_bundle_id)
    .bind(input.package_manifest_sha256)
    .bind(input.subject_bundle_id)
    .bind(input.subject_kind)
    .bind(input.event_kind)
    .bind(input.agent_role_id)
    .bind(&subject_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn retire_missing_bundle_schedules(
    pool: &PgPool,
    visible_goal_keys: &[String],
) -> Result<Vec<AutonomyGoalRow>, AutonomyError> {
    let query = format!(
        r#"UPDATE autonomy_goals SET status = 'retired', lease_owner = NULL,
             lease_expires_at = NULL, heartbeat_at = NULL, revision = revision + 1,
             updated_at = NOW()
           WHERE source_kind = 'bundle_schedule' AND status <> 'running'
             AND status <> 'retired' AND NOT (goal_key = ANY($1))
           RETURNING {GOAL_COLUMNS}"#,
    );
    let mut tx = pool.begin().await?;
    let retired = sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(visible_goal_keys)
        .fetch_all(&mut *tx)
        .await?;
    for goal in &retired {
        append_event(
            &mut tx,
            goal,
            None,
            goal.service_actor_user_id,
            "retired",
            "Signed schedule is no longer visible",
            Value::Object(Default::default()),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(retired)
}

pub async fn recover_expired_leases(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AutonomyGoalRow>, AutonomyError> {
    let mut recovered = Vec::new();
    for _ in 0..limit.clamp(1, 100) {
        let mut tx = pool.begin().await?;
        let select = format!(
            "SELECT {GOAL_COLUMNS} FROM autonomy_goals WHERE status = 'running' AND lease_expires_at <= NOW() ORDER BY lease_expires_at, id FOR UPDATE SKIP LOCKED LIMIT 1"
        );
        let Some(goal) = sqlx::query_as::<_, AutonomyGoalRow>(&select)
            .fetch_optional(&mut *tx)
            .await?
        else {
            tx.rollback().await?;
            break;
        };
        let run_id: Option<Uuid> = sqlx::query_scalar(
            "UPDATE autonomy_goal_runs SET status = 'interrupted', outcome = 'interrupted', verification_state = 'failed', verification_summary = 'Core worker lease expired before the outcome was verified', finished_at = NOW(), updated_at = NOW() WHERE tenant_id = $1 AND goal_id = $2 AND status = 'running' RETURNING id",
        )
        .bind(goal.tenant_id)
        .bind(goal.id)
        .fetch_optional(&mut *tx)
        .await?;
        let retry = goal.attempt < goal.max_attempts && goal.context_state == "ready";
        let query = format!(
            r#"UPDATE autonomy_goals SET status = $3, next_run_at = NOW(),
                 lease_owner = NULL, lease_expires_at = NULL, heartbeat_at = NULL,
                 last_outcome = 'interrupted',
                 last_verification = 'Core worker lease expired before verification',
                 last_finished_at = NOW(), revision = revision + 1, updated_at = NOW()
               WHERE tenant_id = $1 AND id = $2 RETURNING {GOAL_COLUMNS}"#,
        );
        let updated = sqlx::query_as::<_, AutonomyGoalRow>(&query)
            .bind(goal.tenant_id)
            .bind(goal.id)
            .bind(if retry { "retry_wait" } else { "safe_stopped" })
            .fetch_one(&mut *tx)
            .await?;
        append_event(
            &mut tx,
            &updated,
            run_id,
            updated.service_actor_user_id,
            if retry { "interrupted" } else { "safe_stopped" },
            if retry {
                "Expired worker lease will continue as a bounded retry"
            } else {
                "Expired worker lease exhausted the retry budget"
            },
            serde_json::json!({"attempt": updated.attempt, "max_attempts": updated.max_attempts}),
        )
        .await?;
        tx.commit().await?;
        recovered.push(updated);
    }
    Ok(recovered)
}

pub async fn lease_next(
    pool: &PgPool,
    worker_id: &str,
    lease_seconds: i32,
) -> Result<Option<AutonomyLease>, AutonomyError> {
    validate_worker(worker_id, lease_seconds)?;
    let mut tx = pool.begin().await?;
    let select = format!(
        r#"SELECT {GOAL_COLUMNS} FROM autonomy_goals
           WHERE status IN ('ready','retry_wait') AND context_state = 'ready'
             AND next_run_at <= NOW()
           ORDER BY next_run_at, created_at, id
           FOR UPDATE SKIP LOCKED LIMIT 1"#,
    );
    let Some(current) = sqlx::query_as::<_, AutonomyGoalRow>(&select)
        .fetch_optional(&mut *tx)
        .await?
    else {
        tx.rollback().await?;
        return Ok(None);
    };
    let attempt = current.attempt + 1;
    let update = format!(
        r#"UPDATE autonomy_goals SET status = 'running', attempt = $3,
             lease_owner = $4, lease_expires_at = NOW() + make_interval(secs => $5),
             heartbeat_at = NOW(), last_started_at = NOW(), revision = revision + 1,
             updated_at = NOW() WHERE tenant_id = $1 AND id = $2 RETURNING {GOAL_COLUMNS}"#,
    );
    let goal = sqlx::query_as::<_, AutonomyGoalRow>(&update)
        .bind(current.tenant_id)
        .bind(current.id)
        .bind(attempt)
        .bind(worker_id)
        .bind(f64::from(lease_seconds))
        .fetch_one(&mut *tx)
        .await?;
    let run_id = Uuid::new_v4();
    let run_query = format!(
        r#"INSERT INTO autonomy_goal_runs
           (id, tenant_id, goal_id, attempt, worker_id, acting_space_id,
            acting_space_revision, requested_by_user_id, service_actor_user_id,
            effective_role, package_manifest_sha256, target_revision, agent_profile_snapshot)
           SELECT $1, g.tenant_id, g.id, g.attempt, $2, g.acting_space_id, s.revision,
                  g.requested_by_user_id, g.service_actor_user_id, g.effective_role,
                  g.package_manifest_sha256, g.target_revision, g.agent_profile_snapshot
             FROM autonomy_goals g JOIN knowledge_spaces s
               ON s.tenant_id = g.tenant_id AND s.id = g.acting_space_id
            WHERE g.id = $3
           RETURNING {RUN_COLUMNS}"#,
    );
    let run = sqlx::query_as::<_, AutonomyRunRow>(&run_query)
        .bind(run_id)
        .bind(worker_id)
        .bind(goal.id)
        .fetch_one(&mut *tx)
        .await?;
    append_event(
        &mut tx,
        &goal,
        Some(run.id),
        goal.service_actor_user_id,
        "leased",
        "Autonomous goal leased",
        serde_json::json!({"attempt": attempt, "worker_id": worker_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(AutonomyLease { goal, run }))
}

pub async fn validate_lease_context(
    pool: &PgPool,
    lease: &AutonomyLease,
) -> Result<(), AutonomyError> {
    let current = get_goal(pool, lease.goal.tenant_id, lease.goal.id).await?;
    if current.status != "running"
        || current.lease_owner != lease.goal.lease_owner
        || current
            .lease_expires_at
            .map_or(true, |expiry| expiry <= Utc::now())
    {
        return Err(AutonomyError::LeaseLost);
    }
    if current.context_state != "ready"
        || current.acting_space_id != Some(lease.run.acting_space_id)
        || current.requested_by_user_id != Some(lease.run.requested_by_user_id)
        || current.service_actor_user_id != Some(lease.run.service_actor_user_id)
    {
        return Err(AutonomyError::ContextForbidden);
    }
    if current.package_manifest_sha256 != lease.run.package_manifest_sha256
        || current.target_revision != lease.run.target_revision
    {
        return Err(AutonomyError::ExecutionSnapshotChanged);
    }
    let actor = SpaceActor {
        tenant_id: lease.goal.tenant_id,
        user_id: lease.run.requested_by_user_id,
    };
    let space = spaces::require_role(
        pool,
        actor,
        lease.run.acting_space_id,
        SpaceRole::Contributor,
        true,
    )
    .await
    .map_err(|error| match error {
        spaces::KnowledgeSpaceError::Database(error) => AutonomyError::Database(error),
        _ => AutonomyError::ContextForbidden,
    })?;
    if !matches!(space.kind.as_str(), "project" | "team")
        || space.revision != lease.run.acting_space_revision
    {
        return Err(AutonomyError::ContextForbidden);
    }
    spaces::require_role(
        pool,
        SpaceActor {
            tenant_id: lease.goal.tenant_id,
            user_id: lease.run.service_actor_user_id,
        },
        lease.run.acting_space_id,
        SpaceRole::Contributor,
        true,
    )
    .await
    .map_err(|error| match error {
        spaces::KnowledgeSpaceError::Database(error) => AutonomyError::Database(error),
        _ => AutonomyError::ContextForbidden,
    })?;
    Ok(())
}

pub async fn pin_run_context(
    pool: &PgPool,
    lease: &AutonomyLease,
    worker_id: &str,
    policy_revision: Option<&str>,
    context_snapshot: Value,
) -> Result<AutonomyRunRow, AutonomyError> {
    if !context_snapshot.is_object() || policy_revision.is_some_and(|value| value.len() > 256) {
        return Err(AutonomyError::InvalidInput(
            "policy or cited context snapshot is invalid".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    assert_owned_lease(&mut tx, lease.goal.id, lease.run.id, worker_id).await?;
    let query = format!(
        r#"UPDATE autonomy_goal_runs SET policy_revision = $4, context_snapshot = $5,
             updated_at = NOW() WHERE tenant_id = $1 AND goal_id = $2 AND id = $3
             RETURNING {RUN_COLUMNS}"#,
    );
    let run = sqlx::query_as::<_, AutonomyRunRow>(&query)
        .bind(lease.goal.tenant_id)
        .bind(lease.goal.id)
        .bind(lease.run.id)
        .bind(policy_revision)
        .bind(context_snapshot)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE autonomy_goals SET last_policy_revision = $3, revision = revision + 1, updated_at = NOW() WHERE tenant_id = $1 AND id = $2",
    )
    .bind(lease.goal.tenant_id)
    .bind(lease.goal.id)
    .bind(policy_revision)
    .execute(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        &lease.goal,
        Some(lease.run.id),
        lease.goal.service_actor_user_id,
        "context_pinned",
        "Policy and cited context pinned for this attempt",
        Value::Object(Default::default()),
    )
    .await?;
    tx.commit().await?;
    Ok(run)
}

pub async fn attach_runtime_job(
    pool: &PgPool,
    lease: &AutonomyLease,
    worker_id: &str,
    runtime_job_id: &str,
) -> Result<(), AutonomyError> {
    if runtime_job_id.is_empty() || runtime_job_id.len() > 256 {
        return Err(AutonomyError::InvalidInput(
            "runtime job id is outside the supported bounds".into(),
        ));
    }
    let updated = sqlx::query(
        r#"UPDATE autonomy_goal_runs r SET runtime_job_id = $4, updated_at = NOW()
           FROM autonomy_goals g
           WHERE r.tenant_id = $1 AND r.goal_id = $2 AND r.id = $3
             AND g.id = r.goal_id AND g.status = 'running' AND g.lease_owner = $5
             AND g.lease_expires_at > NOW()"#,
    )
    .bind(lease.goal.tenant_id)
    .bind(lease.goal.id)
    .bind(lease.run.id)
    .bind(runtime_job_id)
    .bind(worker_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AutonomyError::LeaseLost);
    }
    Ok(())
}

pub async fn heartbeat(
    pool: &PgPool,
    lease: &AutonomyLease,
    worker_id: &str,
    lease_seconds: i32,
    checkpoint: Value,
) -> Result<(), AutonomyError> {
    validate_worker(worker_id, lease_seconds)?;
    if !checkpoint.is_object() {
        return Err(AutonomyError::InvalidInput(
            "autonomy checkpoint must be an object".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        r#"UPDATE autonomy_goals SET lease_expires_at = NOW() + make_interval(secs => $4),
             heartbeat_at = NOW(), checkpoint = $5, revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND status = 'running' AND lease_owner = $3
             AND lease_expires_at > NOW()"#,
    )
    .bind(lease.goal.tenant_id)
    .bind(lease.goal.id)
    .bind(worker_id)
    .bind(f64::from(lease_seconds))
    .bind(&checkpoint)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AutonomyError::LeaseLost);
    }
    sqlx::query(
        "UPDATE autonomy_goal_runs SET checkpoint = $4, updated_at = NOW() WHERE tenant_id = $1 AND goal_id = $2 AND id = $3 AND status = 'running'",
    )
    .bind(lease.goal.tenant_id)
    .bind(lease.goal.id)
    .bind(lease.run.id)
    .bind(checkpoint)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn finish_run(
    pool: &PgPool,
    lease: &AutonomyLease,
    worker_id: &str,
    finish: RunFinish,
) -> Result<AutonomyGoalRow, AutonomyError> {
    validate_finish(&finish)?;
    let evidence = serde_json::to_value(&finish.evidence_refs)
        .map_err(|_| AutonomyError::InvalidInput("evidence references are invalid".into()))?;
    let mut tx = pool.begin().await?;
    let current = assert_owned_lease(&mut tx, lease.goal.id, lease.run.id, worker_id).await?;
    let (run_status, mut goal_status, event_kind) = match finish.disposition {
        RunDisposition::Succeeded => ("succeeded", "ready", "succeeded"),
        RunDisposition::RetryableFailure if current.attempt < current.max_attempts => {
            ("failed", "retry_wait", "retry_scheduled")
        }
        RunDisposition::ContextRequired => ("safe_stopped", "context_required", "context_required"),
        RunDisposition::RetryableFailure | RunDisposition::SafeStopped => {
            ("safe_stopped", "safe_stopped", "safe_stopped")
        }
    };
    if current.context_state != "ready" {
        goal_status = "context_required";
    }
    sqlx::query(
        r#"UPDATE autonomy_goal_runs SET status = $4, outcome = $5,
             verification_state = $6, verification_summary = $7, evidence_refs = $8,
             finished_at = NOW(), updated_at = NOW()
           WHERE tenant_id = $1 AND goal_id = $2 AND id = $3 AND status = 'running'"#,
    )
    .bind(current.tenant_id)
    .bind(current.id)
    .bind(lease.run.id)
    .bind(run_status)
    .bind(&finish.outcome)
    .bind(&finish.verification_state)
    .bind(&finish.verification_summary)
    .bind(evidence)
    .execute(&mut *tx)
    .await?;
    let query = format!(
        r#"UPDATE autonomy_goals SET status = $3,
             next_run_at = CASE
               WHEN $3 = 'ready' THEN NOW() + make_interval(secs => interval_seconds)
               WHEN $3 = 'retry_wait' THEN NOW() + make_interval(secs => LEAST(60, attempt * attempt))
               ELSE next_run_at END,
             attempt = CASE WHEN $3 = 'ready' THEN 0 ELSE attempt END,
             lease_owner = NULL, lease_expires_at = NULL, heartbeat_at = NULL,
             last_outcome = $4, last_verification = $5, last_finished_at = NOW(),
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 RETURNING {GOAL_COLUMNS}"#,
    );
    let goal = sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(current.tenant_id)
        .bind(current.id)
        .bind(goal_status)
        .bind(&finish.outcome)
        .bind(&finish.verification_summary)
        .fetch_one(&mut *tx)
        .await?;
    append_event(
        &mut tx,
        &goal,
        Some(lease.run.id),
        goal.service_actor_user_id,
        event_kind,
        &finish.verification_summary,
        serde_json::json!({"outcome": finish.outcome, "verification_state": finish.verification_state}),
    )
    .await?;
    tx.commit().await?;
    Ok(goal)
}

pub async fn finish_event_run(
    pool: &PgPool,
    lease: &AutonomyLease,
    worker_id: &str,
    terminal: EventRunTerminal,
    summary: &str,
    result_hash: Option<&str>,
) -> Result<AutonomyGoalRow, AutonomyError> {
    if summary.is_empty()
        || summary.len() > 2_000
        || result_hash.is_some_and(|hash| !is_sha256(hash))
        || (terminal == EventRunTerminal::Succeeded) != result_hash.is_some()
    {
        return Err(AutonomyError::InvalidInput(
            "event terminal summary or result receipt is invalid".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let current = assert_owned_lease(&mut tx, lease.goal.id, lease.run.id, worker_id).await?;
    if current.source_kind != "bundle_event" {
        return Err(AutonomyError::InvalidInput(
            "only a Bundle event goal may use event terminalization".into(),
        ));
    }
    let retry_provider =
        terminal == EventRunTerminal::ProviderFailure && current.attempt < current.max_attempts;
    let (run_status, goal_status, event_kind, outcome, verification_state) = match terminal {
        EventRunTerminal::Succeeded => (
            "succeeded",
            "succeeded",
            "succeeded",
            "succeeded",
            "verified",
        ),
        EventRunTerminal::ProviderFailure if retry_provider => (
            "failed",
            "retry_wait",
            "retry_scheduled",
            "provider_retry",
            "failed",
        ),
        EventRunTerminal::ProviderFailure => (
            "failed_provider",
            "failed_provider",
            "failed_provider",
            "failed_provider",
            "failed",
        ),
        EventRunTerminal::PolicyFailure => (
            "failed_policy",
            "failed_policy",
            "failed_policy",
            "failed_policy",
            "failed",
        ),
        EventRunTerminal::StaleSubject => (
            "stale_subject",
            "stale_subject",
            "stale_subject",
            "stale_subject",
            "failed",
        ),
    };
    let evidence_refs =
        result_hash.map_or_else(Vec::new, |hash| vec![format!("event-result-sha256:{hash}")]);
    sqlx::query(
        r#"UPDATE autonomy_goal_runs SET status = $4, outcome = $5,
             verification_state = $6, verification_summary = $7, evidence_refs = $8,
             finished_at = NOW(), updated_at = NOW()
           WHERE tenant_id = $1 AND goal_id = $2 AND id = $3 AND status = 'running'"#,
    )
    .bind(current.tenant_id)
    .bind(current.id)
    .bind(lease.run.id)
    .bind(run_status)
    .bind(outcome)
    .bind(verification_state)
    .bind(summary)
    .bind(serde_json::to_value(&evidence_refs).expect("string evidence serializes"))
    .execute(&mut *tx)
    .await?;
    if let Some(hash) = result_hash {
        sqlx::query(
            r#"INSERT INTO autonomy_event_receipts
               (job_id, tenant_id, run_id, subject_revision, result_hash)
               VALUES ($1,$2,$3,$4,$5)
               ON CONFLICT (job_id) DO UPDATE SET result_hash = EXCLUDED.result_hash
               WHERE autonomy_event_receipts.subject_revision = EXCLUDED.subject_revision
                 AND autonomy_event_receipts.result_hash = EXCLUDED.result_hash"#,
        )
        .bind(current.id)
        .bind(current.tenant_id)
        .bind(lease.run.id)
        .bind(&current.target_revision)
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    }
    let query = format!(
        r#"UPDATE autonomy_goals SET status = $3,
             next_run_at = CASE WHEN $3 = 'retry_wait'
                                THEN NOW() + make_interval(secs => LEAST(60, attempt * attempt))
                                ELSE next_run_at END,
             lease_owner = NULL, lease_expires_at = NULL, heartbeat_at = NULL,
             last_outcome = $4, last_verification = $5, last_finished_at = NOW(),
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 RETURNING {GOAL_COLUMNS}"#,
    );
    let goal = sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(current.tenant_id)
        .bind(current.id)
        .bind(goal_status)
        .bind(outcome)
        .bind(summary)
        .fetch_one(&mut *tx)
        .await?;
    append_event(
        &mut tx,
        &goal,
        Some(lease.run.id),
        goal.service_actor_user_id,
        event_kind,
        summary,
        serde_json::json!({
            "outcome": outcome,
            "subject_revision": current.target_revision,
            "result_hash": result_hash,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(goal)
}

pub async fn list_goals(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<Vec<AutonomyGoalRow>, AutonomyError> {
    let query = format!(
        "SELECT {GOAL_COLUMNS} FROM autonomy_goals WHERE tenant_id = $1 ORDER BY CASE status WHEN 'context_required' THEN 0 WHEN 'safe_stopped' THEN 1 WHEN 'running' THEN 2 ELSE 3 END, updated_at DESC, id DESC LIMIT $2"
    );
    Ok(sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(tenant_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await?)
}

pub async fn get_goal(
    pool: &PgPool,
    tenant_id: Uuid,
    goal_id: Uuid,
) -> Result<AutonomyGoalRow, AutonomyError> {
    let query =
        format!("SELECT {GOAL_COLUMNS} FROM autonomy_goals WHERE tenant_id = $1 AND id = $2");
    sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(tenant_id)
        .bind(goal_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AutonomyError::NotFound)
}

pub async fn list_runs(
    pool: &PgPool,
    tenant_id: Uuid,
    goal_id: Uuid,
    limit: i64,
) -> Result<Vec<AutonomyRunRow>, AutonomyError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM autonomy_goals WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant_id)
    .bind(goal_id)
    .fetch_one(pool)
    .await?;
    if !exists {
        return Err(AutonomyError::NotFound);
    }
    let query = format!(
        "SELECT {RUN_COLUMNS} FROM autonomy_goal_runs WHERE tenant_id = $1 AND goal_id = $2 ORDER BY started_at DESC, id DESC LIMIT $3"
    );
    Ok(sqlx::query_as::<_, AutonomyRunRow>(&query)
        .bind(tenant_id)
        .bind(goal_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await?)
}

pub async fn resume_goal(
    pool: &PgPool,
    actor: SpaceActor,
    goal_id: Uuid,
    expected_revision: i64,
) -> Result<AutonomyGoalRow, AutonomyError> {
    let current_query =
        format!("SELECT {GOAL_COLUMNS} FROM autonomy_goals WHERE tenant_id = $1 AND id = $2");
    let current = sqlx::query_as::<_, AutonomyGoalRow>(&current_query)
        .bind(actor.tenant_id)
        .bind(goal_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AutonomyError::NotFound)?;
    let space_id = current
        .acting_space_id
        .ok_or(AutonomyError::ContextForbidden)?;
    spaces::require_role(pool, actor, space_id, SpaceRole::Manager, true)
        .await
        .map_err(|error| match error {
            spaces::KnowledgeSpaceError::Database(error) => AutonomyError::Database(error),
            _ => AutonomyError::ContextForbidden,
        })?;
    let query = format!(
        r#"UPDATE autonomy_goals SET status = 'ready', attempt = 0, next_run_at = NOW(),
             last_outcome = NULL, last_verification = NULL, revision = revision + 1,
             updated_at = NOW() WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND context_state = 'ready' AND status IN ('paused','safe_stopped')
             RETURNING {GOAL_COLUMNS}"#,
    );
    let mut tx = pool.begin().await?;
    let goal = sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(actor.tenant_id)
        .bind(goal_id)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AutonomyError::Conflict)?;
    append_event(
        &mut tx,
        &goal,
        None,
        Some(actor.user_id),
        "ready",
        "Manager resumed the autonomous goal after correction",
        serde_json::json!({"previous_status": current.status}),
    )
    .await?;
    tx.commit().await?;
    Ok(goal)
}

struct ResolvedContext {
    state: &'static str,
    space_id: Option<Uuid>,
    requested_by: Option<Uuid>,
    service_actor: Option<Uuid>,
    effective_role: Option<&'static str>,
}

async fn resolve_context(
    pool: &PgPool,
    goal_id: Uuid,
    input: &SyncBundleSchedule,
    existing_service_actor: Option<Uuid>,
    allow_service_grant: bool,
) -> Result<ResolvedContext, AutonomyError> {
    let (Some(space_id), Some(user_id)) = (input.acting_space_id, input.requested_by_user_id)
    else {
        return Ok(unresolved("missing"));
    };
    let actor = SpaceActor {
        tenant_id: input.tenant_id,
        user_id,
    };
    let space =
        match spaces::require_role(pool, actor, space_id, SpaceRole::Contributor, true).await {
            Ok(space) => space,
            Err(spaces::KnowledgeSpaceError::Database(error)) => {
                return Err(AutonomyError::Database(error));
            }
            Err(_) => {
                return Ok(ResolvedContext {
                    state: "actor_forbidden",
                    space_id: Some(space_id),
                    requested_by: Some(user_id),
                    service_actor: existing_service_actor,
                    effective_role: None,
                })
            }
        };
    if !matches!(space.kind.as_str(), "project" | "team") {
        return Ok(ResolvedContext {
            state: "unsupported_space",
            space_id: Some(space_id),
            requested_by: Some(user_id),
            service_actor: existing_service_actor,
            effective_role: None,
        });
    }
    let effective = spaces::effective_spaces(pool, actor)
        .await
        .map_err(|error| match error {
            spaces::KnowledgeSpaceError::Database(error) => AutonomyError::Database(error),
            _ => AutonomyError::ContextForbidden,
        })?
        .into_iter()
        .find(|item| item.space.id == space_id)
        .ok_or(AutonomyError::ContextForbidden)?;
    let service = service_principals::ensure_autonomy_agent(pool, input.tenant_id, goal_id).await?;
    let service_principal = service.user_id.to_string();
    let grant_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM knowledge_space_grants
              WHERE tenant_id = $1 AND space_id = $2 AND principal_kind = 'user'
                AND principal_id = $3 AND role IN ('contributor','curator','manager')
                AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > NOW()))"#,
    )
    .bind(input.tenant_id)
    .bind(space_id)
    .bind(&service_principal)
    .fetch_one(pool)
    .await?;
    if !grant_exists {
        if !allow_service_grant {
            return Ok(ResolvedContext {
                state: "service_grant_required",
                space_id: Some(space_id),
                requested_by: Some(user_id),
                service_actor: Some(service.user_id),
                effective_role: Some(effective.effective_role.as_str()),
            });
        }
        match spaces::upsert_grant(
            pool,
            actor,
            space_id,
            spaces::PrincipalKind::User,
            &service_principal,
            SpaceRole::Contributor,
            None,
        )
        .await
        {
            Ok(_) => {}
            Err(spaces::KnowledgeSpaceError::Database(error)) => {
                return Err(AutonomyError::Database(error));
            }
            Err(_) => return Ok(unresolved("service_grant_required")),
        }
    }
    Ok(ResolvedContext {
        state: "ready",
        space_id: Some(space_id),
        requested_by: Some(user_id),
        service_actor: Some(service.user_id),
        effective_role: Some(effective.effective_role.as_str()),
    })
}

fn unresolved(state: &'static str) -> ResolvedContext {
    ResolvedContext {
        state,
        space_id: None,
        requested_by: None,
        service_actor: None,
        effective_role: None,
    }
}

async fn assert_owned_lease(
    tx: &mut Transaction<'_, Postgres>,
    goal_id: Uuid,
    run_id: Uuid,
    worker_id: &str,
) -> Result<AutonomyGoalRow, AutonomyError> {
    let query = format!(
        r#"SELECT {GOAL_COLUMNS} FROM autonomy_goals g
           WHERE g.id = $1 AND g.status = 'running' AND g.lease_owner = $2
             AND g.lease_expires_at > NOW()
             AND EXISTS (SELECT 1 FROM autonomy_goal_runs r
                          WHERE r.goal_id = g.id AND r.id = $3 AND r.status = 'running')
           FOR UPDATE"#,
    );
    sqlx::query_as::<_, AutonomyGoalRow>(&query)
        .bind(goal_id)
        .bind(worker_id)
        .bind(run_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AutonomyError::LeaseLost)
}

async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    goal: &AutonomyGoalRow,
    run_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    event_kind: &str,
    summary: &str,
    details: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO autonomy_goal_events
           (tenant_id, goal_id, run_id, event_kind, actor_user_id, summary, details)
           VALUES ($1,$2,$3,$4,$5,$6,$7)"#,
    )
    .bind(goal.tenant_id)
    .bind(goal.id)
    .bind(run_id)
    .bind(event_kind)
    .bind(actor_user_id)
    .bind(summary.chars().take(1024).collect::<String>())
    .bind(details)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_schedule(input: &SyncBundleSchedule) -> Result<(), AutonomyError> {
    let bounded = |value: &str, max: usize| !value.is_empty() && value.len() <= max;
    if !bounded(&input.goal_key, 300)
        || !bounded(&input.goal, 1000)
        || !bounded(&input.owner_bundle_id, 128)
        || !bounded(&input.recipe_id, 128)
        || !bounded(&input.target_kind, 64)
        || !bounded(&input.target_id, 256)
        || !bounded(&input.target_revision, 256)
        || !bounded(input.target_label.trim(), 300)
        || input.package_manifest_sha256.len() != 64
        || !input
            .package_manifest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !(10..=2_592_000).contains(&input.interval.as_secs())
        || !(5..=3_600).contains(&input.max_wall_time.as_secs())
        || !(1..=10).contains(&input.max_attempts)
    {
        return Err(AutonomyError::InvalidInput(
            "signed schedule identity or budget is outside the supported bounds".into(),
        ));
    }
    Ok(())
}

fn validate_bundle_event(input: &EnqueueBundleEvent) -> Result<(), AutonomyError> {
    let bounded = |value: &str, max: usize| !value.is_empty() && value.len() <= max;
    if !bounded(&input.event_kind, 128)
        || !bounded(&input.subject_bundle_id, 128)
        || !bounded(&input.subject_kind, 128)
        || !bounded(&input.subject_id, 256)
        || !is_sha256(&input.subject_revision)
        || !input.event_payload.is_object()
        || !bounded(&input.owner_bundle_id, 128)
        || !bounded(&input.recipe_id, 128)
        || !is_sha256(&input.package_manifest_sha256)
        || !bounded(&input.agent_role_id, 128)
        || !bounded(&input.result_gadget, 256)
        || !bounded(input.goal.trim(), 1_000)
        || !matches!(
            input.effective_role.as_str(),
            "contributor" | "curator" | "manager"
        )
        || !(5..=3_600).contains(&input.max_wall_seconds)
        || !(1..=10).contains(&input.max_attempts)
        || !input.agent_profile_snapshot.is_object()
        || input
            .agent_profile_snapshot
            .as_object()
            .is_some_and(|value| value.is_empty())
    {
        return Err(AutonomyError::InvalidInput(
            "signed Bundle event identity, context, profile, or budget is invalid".into(),
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_worker(worker_id: &str, lease_seconds: i32) -> Result<(), AutonomyError> {
    if worker_id.is_empty() || worker_id.len() > 128 || !(5..=300).contains(&lease_seconds) {
        return Err(AutonomyError::InvalidInput(
            "worker id or lease duration is outside the supported bounds".into(),
        ));
    }
    Ok(())
}

fn validate_finish(finish: &RunFinish) -> Result<(), AutonomyError> {
    if finish.outcome.is_empty()
        || finish.outcome.len() > 64
        || finish.verification_state.is_empty()
        || finish.verification_state.len() > 64
        || finish.verification_summary.is_empty()
        || finish.verification_summary.len() > 2000
        || finish.evidence_refs.len() > 128
        || finish
            .evidence_refs
            .iter()
            .any(|reference| reference.is_empty() || reference.len() > 512)
    {
        return Err(AutonomyError::InvalidInput(
            "autonomous outcome or evidence is outside the supported bounds".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_bounds_reject_memory_only_fast_loops() {
        let input = SyncBundleSchedule {
            goal_key: "bundle:recipe:tenant:target".into(),
            goal: "Keep target observable".into(),
            tenant_id: Uuid::new_v4(),
            owner_bundle_id: "server-administrator".into(),
            recipe_id: "server-duty-cycle".into(),
            package_manifest_sha256: "a".repeat(64),
            target_kind: "ssh".into(),
            target_id: "edge-one".into(),
            target_revision: Uuid::new_v4().to_string(),
            target_label: "Edge one".into(),
            acting_space_id: None,
            requested_by_user_id: None,
            interval: Duration::from_secs(5),
            max_wall_time: Duration::from_secs(100),
            max_attempts: 3,
        };
        assert!(validate_schedule(&input).is_err());
    }
}
