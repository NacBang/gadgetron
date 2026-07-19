//! PostgreSQL authority for Manager outcome history, corrective directives,
//! terminal exceptions, and webhook delivery state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const MAX_LIST_LIMIT: i64 = 200;

#[derive(Debug, thiserror::Error)]
pub enum ManagerOversightError {
    #[error("manager record not found")]
    NotFound,
    #[error("manager record revision changed")]
    Conflict,
    #[error("invalid manager record: {0}")]
    InvalidInput(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OversightRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub source_kind: String,
    pub source_id: String,
    pub actor_user_id: Option<Uuid>,
    pub agent_label: String,
    pub agent_role: String,
    pub goal: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_revision: Option<String>,
    pub policy_decision: String,
    pub policy_revision: Option<String>,
    pub evidence_refs: Value,
    pub current_stage: String,
    pub outcome: String,
    pub verification_state: String,
    pub action_summary: String,
    pub before_summary: Option<String>,
    pub after_summary: Option<String>,
    pub rollback_summary: Option<String>,
    pub duration_ms: i64,
    pub cost_minor_units: i64,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OversightEvent {
    pub id: i64,
    pub tenant_id: Uuid,
    pub oversight_id: Uuid,
    pub stage: String,
    pub state: String,
    pub summary: String,
    pub evidence_refs: Value,
    pub actor_user_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ManagerException {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub oversight_id: Uuid,
    pub directive_id: Option<Uuid>,
    pub severity: String,
    pub summary: String,
    pub state: String,
    pub acknowledged_by_user_id: Option<Uuid>,
    pub resolved_by_user_id: Option<Uuid>,
    pub revision: i64,
    pub occurred_at: DateTime<Utc>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ExceptionEvent {
    pub id: i64,
    pub tenant_id: Uuid,
    pub exception_id: Uuid,
    pub state: String,
    pub actor_user_id: Option<Uuid>,
    pub summary: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CorrectiveDirective {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub oversight_id: Uuid,
    pub target_kind: String,
    pub target_id: String,
    pub target_revision: Option<String>,
    pub issued_by_user_id: Uuid,
    pub instruction: String,
    pub desired_outcome: String,
    pub constraints: Value,
    pub priority: String,
    pub state: String,
    pub plan_summary: Option<String>,
    pub execution_summary: Option<String>,
    pub verification_summary: Option<String>,
    pub before_summary: Option<String>,
    pub after_summary: Option<String>,
    pub evidence_refs: Value,
    pub due_at: Option<DateTime<Utc>>,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DirectiveEvent {
    pub id: i64,
    pub tenant_id: Uuid,
    pub directive_id: Uuid,
    pub state: String,
    pub summary: String,
    pub actor_user_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WebhookDelivery {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub exception_id: Uuid,
    pub idempotency_key: String,
    pub state: String,
    pub attempt_count: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub last_http_status: Option<i32>,
    pub last_error_code: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebhookSettingsView {
    pub enabled: bool,
    pub configured: bool,
    pub destination_host: Option<String>,
    pub revision: i64,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OversightDetail {
    pub record: OversightRecord,
    pub events: Vec<OversightEvent>,
    pub exception: Option<ManagerException>,
    pub delivery: Option<WebhookDelivery>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirectiveDetail {
    pub directive: CorrectiveDirective,
    pub events: Vec<DirectiveEvent>,
    pub oversight: OversightDetail,
}

#[derive(Debug, Clone)]
pub struct StageEventInput {
    pub stage: String,
    pub state: String,
    pub summary: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RecordOutcomeInput {
    pub tenant_id: Uuid,
    pub source_kind: String,
    pub source_id: String,
    pub actor_user_id: Option<Uuid>,
    pub agent_label: String,
    pub agent_role: String,
    pub goal: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_revision: Option<String>,
    pub policy_decision: String,
    pub policy_revision: Option<String>,
    pub evidence_refs: Vec<String>,
    pub current_stage: String,
    pub outcome: String,
    pub verification_state: String,
    pub action_summary: String,
    pub before_summary: Option<String>,
    pub after_summary: Option<String>,
    pub rollback_summary: Option<String>,
    pub duration_ms: i64,
    pub cost_minor_units: i64,
    pub events: Vec<StageEventInput>,
    pub exception_severity: Option<String>,
    pub exception_summary: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateDirectiveInput {
    pub target_kind: String,
    pub target_id: String,
    #[serde(default)]
    pub target_revision: Option<String>,
    pub instruction: String,
    pub desired_outcome: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub priority: String,
    #[serde(default)]
    pub due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransitionDirectiveInput {
    pub expected_revision: i64,
    pub state: String,
    pub summary: String,
    #[serde(default)]
    pub plan_summary: Option<String>,
    #[serde(default)]
    pub execution_summary: Option<String>,
    #[serde(default)]
    pub verification_summary: Option<String>,
    #[serde(default)]
    pub before_summary: Option<String>,
    #[serde(default)]
    pub after_summary: Option<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WebhookSettingsInput {
    pub enabled: bool,
    pub endpoint_url: Option<String>,
    pub destination_host: Option<String>,
    pub review_base_url: Option<String>,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DueWebhookDelivery {
    pub delivery_id: Uuid,
    pub tenant_id: Uuid,
    pub exception_id: Uuid,
    pub idempotency_key: String,
    pub attempt_count: i32,
    pub endpoint_url: String,
    pub review_base_url: String,
    pub severity: String,
    pub summary: String,
    pub occurred_at: DateTime<Utc>,
}

const OVERSIGHT_COLUMNS: &str = "id, tenant_id, source_kind, source_id, actor_user_id, agent_label, agent_role, goal, target_kind, target_id, target_revision, policy_decision, policy_revision, evidence_refs, current_stage, outcome, verification_state, action_summary, before_summary, after_summary, rollback_summary, duration_ms, cost_minor_units, revision, created_at, updated_at, finished_at";
const EVENT_COLUMNS: &str =
    "id, tenant_id, oversight_id, stage, state, summary, evidence_refs, actor_user_id, occurred_at";
const DIRECTIVE_COLUMNS: &str = "id, tenant_id, oversight_id, target_kind, target_id, target_revision, issued_by_user_id, instruction, desired_outcome, constraints, priority, state, plan_summary, execution_summary, verification_summary, before_summary, after_summary, evidence_refs, due_at, revision, created_at, updated_at, finished_at";
const DIRECTIVE_EVENT_COLUMNS: &str =
    "id, tenant_id, directive_id, state, summary, actor_user_id, occurred_at";
const EXCEPTION_COLUMNS: &str = "id, tenant_id, oversight_id, directive_id, severity, summary, state, acknowledged_by_user_id, resolved_by_user_id, revision, occurred_at, acknowledged_at, resolved_at, updated_at";
const EXCEPTION_EVENT_COLUMNS: &str =
    "id, tenant_id, exception_id, state, actor_user_id, summary, occurred_at";
const DELIVERY_COLUMNS: &str = "id, tenant_id, exception_id, idempotency_key, state, attempt_count, next_attempt_at, last_http_status, last_error_code, delivered_at, created_at, updated_at";

pub async fn record_outcome(
    pool: &PgPool,
    input: RecordOutcomeInput,
) -> Result<OversightRecord, ManagerOversightError> {
    validate_record_input(&input)?;
    let mut tx = pool.begin().await?;
    if let Some(existing) = oversight_by_source(
        &mut tx,
        input.tenant_id,
        &input.source_kind,
        &input.source_id,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(existing);
    }
    let id = Uuid::new_v4();
    let query = format!(
        "INSERT INTO manager_oversight_records (id, tenant_id, source_kind, source_id, actor_user_id, agent_label, agent_role, goal, target_kind, target_id, target_revision, policy_decision, policy_revision, evidence_refs, current_stage, outcome, verification_state, action_summary, before_summary, after_summary, rollback_summary, duration_ms, cost_minor_units, finished_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,CASE WHEN $16 IN ('pending','pending_review') THEN NULL ELSE NOW() END) RETURNING {OVERSIGHT_COLUMNS}"
    );
    let record = sqlx::query_as::<_, OversightRecord>(&query)
        .bind(id)
        .bind(input.tenant_id)
        .bind(&input.source_kind)
        .bind(&input.source_id)
        .bind(input.actor_user_id)
        .bind(&input.agent_label)
        .bind(&input.agent_role)
        .bind(&input.goal)
        .bind(&input.target_kind)
        .bind(&input.target_id)
        .bind(&input.target_revision)
        .bind(&input.policy_decision)
        .bind(&input.policy_revision)
        .bind(serde_json::to_value(&input.evidence_refs).expect("string vec serializes"))
        .bind(&input.current_stage)
        .bind(&input.outcome)
        .bind(&input.verification_state)
        .bind(&input.action_summary)
        .bind(&input.before_summary)
        .bind(&input.after_summary)
        .bind(&input.rollback_summary)
        .bind(input.duration_ms)
        .bind(input.cost_minor_units)
        .fetch_one(&mut *tx)
        .await?;
    for event in &input.events {
        insert_oversight_event(&mut tx, &record, event, input.actor_user_id).await?;
    }
    if let (Some(severity), Some(summary)) = (
        input.exception_severity.as_deref(),
        input.exception_summary.as_deref(),
    ) {
        insert_exception(
            &mut tx,
            &record,
            None,
            severity,
            summary,
            input.actor_user_id,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(record)
}

pub async fn list_oversight(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<Vec<OversightRecord>, ManagerOversightError> {
    let query = format!(
        "SELECT {OVERSIGHT_COLUMNS} FROM manager_oversight_records WHERE tenant_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2"
    );
    Ok(sqlx::query_as::<_, OversightRecord>(&query)
        .bind(tenant_id)
        .bind(limit.clamp(1, MAX_LIST_LIMIT))
        .fetch_all(pool)
        .await?)
}

pub async fn oversight_detail(
    pool: &PgPool,
    tenant_id: Uuid,
    oversight_id: Uuid,
) -> Result<OversightDetail, ManagerOversightError> {
    let query = format!(
        "SELECT {OVERSIGHT_COLUMNS} FROM manager_oversight_records WHERE tenant_id = $1 AND id = $2"
    );
    let record = sqlx::query_as::<_, OversightRecord>(&query)
        .bind(tenant_id)
        .bind(oversight_id)
        .fetch_optional(pool)
        .await?
        .ok_or(ManagerOversightError::NotFound)?;
    let query = format!(
        "SELECT {EVENT_COLUMNS} FROM manager_oversight_events WHERE tenant_id = $1 AND oversight_id = $2 ORDER BY id"
    );
    let events = sqlx::query_as::<_, OversightEvent>(&query)
        .bind(tenant_id)
        .bind(oversight_id)
        .fetch_all(pool)
        .await?;
    let query = format!(
        "SELECT {EXCEPTION_COLUMNS} FROM manager_exceptions WHERE tenant_id = $1 AND oversight_id = $2"
    );
    let exception = sqlx::query_as::<_, ManagerException>(&query)
        .bind(tenant_id)
        .bind(oversight_id)
        .fetch_optional(pool)
        .await?;
    let delivery = match exception.as_ref() {
        Some(exception) => delivery_for_exception(pool, tenant_id, exception.id).await?,
        None => None,
    };
    Ok(OversightDetail {
        record,
        events,
        exception,
        delivery,
    })
}

pub async fn create_directive(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    input: CreateDirectiveInput,
) -> Result<DirectiveDetail, ManagerOversightError> {
    validate_target(
        &input.target_kind,
        &input.target_id,
        input.target_revision.as_deref(),
    )?;
    validate_text("instruction", &input.instruction, 4_000)?;
    validate_text("desired_outcome", &input.desired_outcome, 2_000)?;
    validate_string_list("constraints", &input.constraints, 32, 500)?;
    if !matches!(input.priority.as_str(), "normal" | "urgent") {
        return Err(invalid("priority must be normal or urgent"));
    }
    let directive_id = Uuid::new_v4();
    let oversight_id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let record_query = format!(
        "INSERT INTO manager_oversight_records (id, tenant_id, source_kind, source_id, actor_user_id, agent_label, agent_role, goal, target_kind, target_id, target_revision, policy_decision, evidence_refs, current_stage, outcome, verification_state, action_summary) VALUES ($1,$2,'directive',$3,$4,'Manager','corrective_directive',$5,$6,$7,$8,'review','[]'::jsonb,'target','pending','pending',$9) RETURNING {OVERSIGHT_COLUMNS}"
    );
    let record = sqlx::query_as::<_, OversightRecord>(&record_query)
        .bind(oversight_id)
        .bind(tenant_id)
        .bind(directive_id.to_string())
        .bind(user_id)
        .bind(&input.desired_outcome)
        .bind(&input.target_kind)
        .bind(&input.target_id)
        .bind(&input.target_revision)
        .bind(&input.instruction)
        .fetch_one(&mut *tx)
        .await?;
    insert_oversight_event(
        &mut tx,
        &record,
        &StageEventInput {
            stage: "target".into(),
            state: "recorded".into(),
            summary: format!(
                "Correction targets {} {}",
                input.target_kind, input.target_id
            ),
            evidence_refs: Vec::new(),
        },
        Some(user_id),
    )
    .await?;
    let directive_query = format!(
        "INSERT INTO manager_directives (id, tenant_id, oversight_id, target_kind, target_id, target_revision, issued_by_user_id, instruction, desired_outcome, constraints, priority, state, due_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'issued',$12) RETURNING {DIRECTIVE_COLUMNS}"
    );
    let directive = sqlx::query_as::<_, CorrectiveDirective>(&directive_query)
        .bind(directive_id)
        .bind(tenant_id)
        .bind(oversight_id)
        .bind(&input.target_kind)
        .bind(&input.target_id)
        .bind(&input.target_revision)
        .bind(user_id)
        .bind(&input.instruction)
        .bind(&input.desired_outcome)
        .bind(serde_json::to_value(&input.constraints).expect("string vec serializes"))
        .bind(&input.priority)
        .bind(input.due_at)
        .fetch_one(&mut *tx)
        .await?;
    insert_directive_event(
        &mut tx,
        &directive,
        user_id,
        "issued",
        "Manager issued the corrective directive",
    )
    .await?;
    tx.commit().await?;
    directive_detail(pool, tenant_id, directive.id).await
}

pub async fn list_directives(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<Vec<CorrectiveDirective>, ManagerOversightError> {
    let query = format!(
        "SELECT {DIRECTIVE_COLUMNS} FROM manager_directives WHERE tenant_id = $1 ORDER BY CASE priority WHEN 'urgent' THEN 0 ELSE 1 END, created_at DESC, id DESC LIMIT $2"
    );
    Ok(sqlx::query_as::<_, CorrectiveDirective>(&query)
        .bind(tenant_id)
        .bind(limit.clamp(1, MAX_LIST_LIMIT))
        .fetch_all(pool)
        .await?)
}

pub async fn directive_detail(
    pool: &PgPool,
    tenant_id: Uuid,
    directive_id: Uuid,
) -> Result<DirectiveDetail, ManagerOversightError> {
    let query = format!(
        "SELECT {DIRECTIVE_COLUMNS} FROM manager_directives WHERE tenant_id = $1 AND id = $2"
    );
    let directive = sqlx::query_as::<_, CorrectiveDirective>(&query)
        .bind(tenant_id)
        .bind(directive_id)
        .fetch_optional(pool)
        .await?
        .ok_or(ManagerOversightError::NotFound)?;
    let query = format!(
        "SELECT {DIRECTIVE_EVENT_COLUMNS} FROM manager_directive_events WHERE tenant_id = $1 AND directive_id = $2 ORDER BY id"
    );
    let events = sqlx::query_as::<_, DirectiveEvent>(&query)
        .bind(tenant_id)
        .bind(directive_id)
        .fetch_all(pool)
        .await?;
    let oversight = oversight_detail(pool, tenant_id, directive.oversight_id).await?;
    Ok(DirectiveDetail {
        directive,
        events,
        oversight,
    })
}

pub async fn transition_directive(
    pool: &PgPool,
    tenant_id: Uuid,
    directive_id: Uuid,
    user_id: Uuid,
    input: TransitionDirectiveInput,
) -> Result<DirectiveDetail, ManagerOversightError> {
    validate_text("summary", &input.summary, 2_000)?;
    validate_optional_text("plan_summary", input.plan_summary.as_deref(), 4_000)?;
    validate_optional_text(
        "execution_summary",
        input.execution_summary.as_deref(),
        4_000,
    )?;
    validate_optional_text(
        "verification_summary",
        input.verification_summary.as_deref(),
        4_000,
    )?;
    validate_optional_text("before_summary", input.before_summary.as_deref(), 4_000)?;
    validate_optional_text("after_summary", input.after_summary.as_deref(), 4_000)?;
    validate_string_list("evidence_refs", &input.evidence_refs, 64, 500)?;
    let mut tx = pool.begin().await?;
    let directive = directive_for_update(&mut tx, tenant_id, directive_id).await?;
    if directive.revision != input.expected_revision {
        return Err(ManagerOversightError::Conflict);
    }
    validate_transition(&directive, &input)?;
    let query = format!(
        "UPDATE manager_directives SET state = $3, plan_summary = COALESCE($4, plan_summary), execution_summary = COALESCE($5, execution_summary), verification_summary = COALESCE($6, verification_summary), before_summary = COALESCE($7, before_summary), after_summary = COALESCE($8, after_summary), evidence_refs = CASE WHEN jsonb_array_length($9::jsonb) > 0 THEN $9::jsonb ELSE evidence_refs END, revision = revision + 1, updated_at = NOW(), finished_at = CASE WHEN $3 IN ('resolved','failed','escalated') THEN NOW() ELSE NULL END WHERE tenant_id = $1 AND id = $2 AND revision = $10 RETURNING {DIRECTIVE_COLUMNS}"
    );
    let updated = sqlx::query_as::<_, CorrectiveDirective>(&query)
        .bind(tenant_id)
        .bind(directive_id)
        .bind(&input.state)
        .bind(&input.plan_summary)
        .bind(&input.execution_summary)
        .bind(&input.verification_summary)
        .bind(&input.before_summary)
        .bind(&input.after_summary)
        .bind(serde_json::to_value(&input.evidence_refs).expect("string vec serializes"))
        .bind(input.expected_revision)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ManagerOversightError::Conflict)?;
    insert_directive_event(&mut tx, &updated, user_id, &input.state, &input.summary).await?;
    apply_directive_to_oversight(&mut tx, &updated, user_id, &input).await?;
    tx.commit().await?;
    directive_detail(pool, tenant_id, directive_id).await
}

pub async fn list_exceptions(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<Vec<ManagerException>, ManagerOversightError> {
    let query = format!(
        "SELECT {EXCEPTION_COLUMNS} FROM manager_exceptions WHERE tenant_id = $1 ORDER BY CASE state WHEN 'open' THEN 0 WHEN 'acknowledged' THEN 1 ELSE 2 END, CASE severity WHEN 'critical' THEN 0 WHEN 'error' THEN 1 ELSE 2 END, occurred_at DESC LIMIT $2"
    );
    Ok(sqlx::query_as::<_, ManagerException>(&query)
        .bind(tenant_id)
        .bind(limit.clamp(1, MAX_LIST_LIMIT))
        .fetch_all(pool)
        .await?)
}

pub async fn exception_events(
    pool: &PgPool,
    tenant_id: Uuid,
    exception_id: Uuid,
) -> Result<Vec<ExceptionEvent>, ManagerOversightError> {
    let query = format!(
        "SELECT {EXCEPTION_EVENT_COLUMNS} FROM manager_exception_events WHERE tenant_id = $1 AND exception_id = $2 ORDER BY id"
    );
    Ok(sqlx::query_as::<_, ExceptionEvent>(&query)
        .bind(tenant_id)
        .bind(exception_id)
        .fetch_all(pool)
        .await?)
}

pub async fn transition_exception(
    pool: &PgPool,
    tenant_id: Uuid,
    exception_id: Uuid,
    user_id: Uuid,
    expected_revision: i64,
    next_state: &str,
    summary: &str,
) -> Result<ManagerException, ManagerOversightError> {
    validate_text("summary", summary, 1_000)?;
    if !matches!(next_state, "acknowledged" | "resolved") {
        return Err(invalid("exception state must be acknowledged or resolved"));
    }
    let mut tx = pool.begin().await?;
    let current_query = format!(
        "SELECT {EXCEPTION_COLUMNS} FROM manager_exceptions WHERE tenant_id = $1 AND id = $2 FOR UPDATE"
    );
    let current = sqlx::query_as::<_, ManagerException>(&current_query)
        .bind(tenant_id)
        .bind(exception_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ManagerOversightError::NotFound)?;
    if current.revision != expected_revision
        || !matches!(
            (current.state.as_str(), next_state),
            ("open", "acknowledged") | ("open", "resolved") | ("acknowledged", "resolved")
        )
    {
        return Err(ManagerOversightError::Conflict);
    }
    let query = format!(
        "UPDATE manager_exceptions SET state = $3, acknowledged_by_user_id = CASE WHEN $3 = 'acknowledged' THEN $4 ELSE acknowledged_by_user_id END, acknowledged_at = CASE WHEN $3 = 'acknowledged' THEN NOW() ELSE acknowledged_at END, resolved_by_user_id = CASE WHEN $3 = 'resolved' THEN $4 ELSE resolved_by_user_id END, resolved_at = CASE WHEN $3 = 'resolved' THEN NOW() ELSE resolved_at END, revision = revision + 1, updated_at = NOW() WHERE tenant_id = $1 AND id = $2 AND revision = $5 RETURNING {EXCEPTION_COLUMNS}"
    );
    let updated = sqlx::query_as::<_, ManagerException>(&query)
        .bind(tenant_id)
        .bind(exception_id)
        .bind(next_state)
        .bind(user_id)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ManagerOversightError::Conflict)?;
    sqlx::query("INSERT INTO manager_exception_events (tenant_id, exception_id, state, actor_user_id, summary) VALUES ($1,$2,$3,$4,$5)")
        .bind(tenant_id)
        .bind(exception_id)
        .bind(next_state)
        .bind(user_id)
        .bind(summary)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(updated)
}

pub async fn webhook_settings(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<WebhookSettingsView, ManagerOversightError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        enabled: bool,
        endpoint_url: Option<String>,
        destination_host: Option<String>,
        revision: i64,
        updated_at: DateTime<Utc>,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT enabled, endpoint_url, destination_host, revision, updated_at FROM manager_webhook_settings WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some(row) => WebhookSettingsView {
            enabled: row.enabled,
            configured: row.endpoint_url.is_some(),
            destination_host: row.destination_host,
            revision: row.revision,
            updated_at: Some(row.updated_at),
        },
        None => WebhookSettingsView {
            enabled: false,
            configured: false,
            destination_host: None,
            revision: 0,
            updated_at: None,
        },
    })
}

pub async fn update_webhook_settings(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    input: WebhookSettingsInput,
) -> Result<WebhookSettingsView, ManagerOversightError> {
    let mut tx = pool.begin().await?;
    #[derive(sqlx::FromRow)]
    struct Current {
        endpoint_url: Option<String>,
        destination_host: Option<String>,
        review_base_url: Option<String>,
        revision: i64,
    }
    let current = sqlx::query_as::<_, Current>(
        "SELECT endpoint_url, destination_host, review_base_url, revision FROM manager_webhook_settings WHERE tenant_id = $1 FOR UPDATE",
    )
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    let current_revision = current.as_ref().map_or(0, |row| row.revision);
    if current_revision != input.expected_revision {
        return Err(ManagerOversightError::Conflict);
    }
    let endpoint_url = input
        .endpoint_url
        .or_else(|| current.as_ref().and_then(|row| row.endpoint_url.clone()));
    let destination_host = input.destination_host.or_else(|| {
        current
            .as_ref()
            .and_then(|row| row.destination_host.clone())
    });
    let review_base_url = input
        .review_base_url
        .or_else(|| current.as_ref().and_then(|row| row.review_base_url.clone()));
    if input.enabled
        && (endpoint_url.is_none() || destination_host.is_none() || review_base_url.is_none())
    {
        return Err(invalid(
            "enabled webhook requires a destination and Review base URL",
        ));
    }
    if current.is_some() {
        sqlx::query("UPDATE manager_webhook_settings SET enabled = $2, endpoint_url = $3, destination_host = $4, review_base_url = $5, updated_by_user_id = $6, revision = revision + 1, updated_at = NOW() WHERE tenant_id = $1 AND revision = $7")
            .bind(tenant_id)
            .bind(input.enabled)
            .bind(&endpoint_url)
            .bind(&destination_host)
            .bind(&review_base_url)
            .bind(user_id)
            .bind(input.expected_revision)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("INSERT INTO manager_webhook_settings (tenant_id, enabled, endpoint_url, destination_host, review_base_url, updated_by_user_id) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(tenant_id)
            .bind(input.enabled)
            .bind(&endpoint_url)
            .bind(&destination_host)
            .bind(&review_base_url)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    if input.enabled {
        sqlx::query(
            "INSERT INTO manager_webhook_deliveries (tenant_id, exception_id, idempotency_key, state) SELECT e.tenant_id, e.id, 'manager-exception:' || e.id::text, 'pending' FROM manager_exceptions e LEFT JOIN manager_webhook_deliveries d ON d.tenant_id = e.tenant_id AND d.exception_id = e.id WHERE e.tenant_id = $1 AND e.state <> 'resolved' AND d.id IS NULL",
        )
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    webhook_settings(pool, tenant_id).await
}

pub async fn list_deliveries(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<Vec<WebhookDelivery>, ManagerOversightError> {
    let query = format!(
        "SELECT {DELIVERY_COLUMNS} FROM manager_webhook_deliveries WHERE tenant_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2"
    );
    Ok(sqlx::query_as::<_, WebhookDelivery>(&query)
        .bind(tenant_id)
        .bind(limit.clamp(1, MAX_LIST_LIMIT))
        .fetch_all(pool)
        .await?)
}

pub async fn claim_due_deliveries(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<DueWebhookDelivery>, ManagerOversightError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query_as::<_, DueWebhookDelivery>(
        r#"SELECT d.id AS delivery_id, d.tenant_id, d.exception_id,
                  d.idempotency_key, d.attempt_count + 1 AS attempt_count,
                  s.endpoint_url, s.review_base_url, e.severity, e.summary, e.occurred_at
           FROM manager_webhook_deliveries d
           JOIN manager_webhook_settings s ON s.tenant_id = d.tenant_id AND s.enabled
           JOIN manager_exceptions e ON e.tenant_id = d.tenant_id AND e.id = d.exception_id
           WHERE d.state IN ('pending','failed_retryable')
             AND d.next_attempt_at <= NOW() AND d.attempt_count < 4
           ORDER BY d.next_attempt_at, d.created_at
           FOR UPDATE OF d SKIP LOCKED LIMIT $1"#,
    )
    .bind(limit.clamp(1, 50))
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        sqlx::query("UPDATE manager_webhook_deliveries SET attempt_count = $2, updated_at = NOW() WHERE id = $1")
            .bind(row.delivery_id)
            .bind(row.attempt_count)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(rows)
}

pub async fn finish_delivery(
    pool: &PgPool,
    delivery_id: Uuid,
    attempt_count: i32,
    state: &str,
    http_status: Option<u16>,
    error_code: Option<&str>,
    retry_after_seconds: i64,
) -> Result<(), ManagerOversightError> {
    if !matches!(state, "sent" | "failed_retryable" | "failed_terminal") {
        return Err(invalid("invalid webhook delivery state"));
    }
    sqlx::query(
        "UPDATE manager_webhook_deliveries SET state = $3, last_http_status = $4, last_error_code = $5, next_attempt_at = NOW() + make_interval(secs => $6), delivered_at = CASE WHEN $3 = 'sent' THEN NOW() ELSE delivered_at END, updated_at = NOW() WHERE id = $1 AND attempt_count = $2",
    )
    .bind(delivery_id)
    .bind(attempt_count)
    .bind(state)
    .bind(http_status.map(i32::from))
    .bind(error_code)
    .bind(retry_after_seconds.max(0))
    .execute(pool)
    .await?;
    Ok(())
}

async fn oversight_by_source(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    source_kind: &str,
    source_id: &str,
) -> Result<Option<OversightRecord>, ManagerOversightError> {
    let query = format!(
        "SELECT {OVERSIGHT_COLUMNS} FROM manager_oversight_records WHERE tenant_id = $1 AND source_kind = $2 AND source_id = $3"
    );
    Ok(sqlx::query_as::<_, OversightRecord>(&query)
        .bind(tenant_id)
        .bind(source_kind)
        .bind(source_id)
        .fetch_optional(&mut **tx)
        .await?)
}

async fn directive_for_update(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    directive_id: Uuid,
) -> Result<CorrectiveDirective, ManagerOversightError> {
    let query = format!(
        "SELECT {DIRECTIVE_COLUMNS} FROM manager_directives WHERE tenant_id = $1 AND id = $2 FOR UPDATE"
    );
    sqlx::query_as::<_, CorrectiveDirective>(&query)
        .bind(tenant_id)
        .bind(directive_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ManagerOversightError::NotFound)
}

async fn delivery_for_exception(
    pool: &PgPool,
    tenant_id: Uuid,
    exception_id: Uuid,
) -> Result<Option<WebhookDelivery>, ManagerOversightError> {
    let query = format!(
        "SELECT {DELIVERY_COLUMNS} FROM manager_webhook_deliveries WHERE tenant_id = $1 AND exception_id = $2"
    );
    Ok(sqlx::query_as::<_, WebhookDelivery>(&query)
        .bind(tenant_id)
        .bind(exception_id)
        .fetch_optional(pool)
        .await?)
}

async fn insert_oversight_event(
    tx: &mut Transaction<'_, Postgres>,
    record: &OversightRecord,
    event: &StageEventInput,
    actor_user_id: Option<Uuid>,
) -> Result<(), ManagerOversightError> {
    validate_stage_event(event)?;
    sqlx::query("INSERT INTO manager_oversight_events (tenant_id, oversight_id, stage, state, summary, evidence_refs, actor_user_id) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(record.tenant_id)
        .bind(record.id)
        .bind(&event.stage)
        .bind(&event.state)
        .bind(&event.summary)
        .bind(serde_json::to_value(&event.evidence_refs).expect("string vec serializes"))
        .bind(actor_user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_directive_event(
    tx: &mut Transaction<'_, Postgres>,
    directive: &CorrectiveDirective,
    user_id: Uuid,
    state: &str,
    summary: &str,
) -> Result<(), ManagerOversightError> {
    sqlx::query("INSERT INTO manager_directive_events (tenant_id, directive_id, state, summary, actor_user_id) VALUES ($1,$2,$3,$4,$5)")
        .bind(directive.tenant_id)
        .bind(directive.id)
        .bind(state)
        .bind(summary)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_exception(
    tx: &mut Transaction<'_, Postgres>,
    record: &OversightRecord,
    directive_id: Option<Uuid>,
    severity: &str,
    summary: &str,
    actor_user_id: Option<Uuid>,
) -> Result<ManagerException, ManagerOversightError> {
    if !matches!(severity, "warning" | "error" | "critical") {
        return Err(invalid(
            "exception severity must be warning, error, or critical",
        ));
    }
    validate_text("exception_summary", summary, 300)?;
    let query = format!(
        "INSERT INTO manager_exceptions (tenant_id, oversight_id, directive_id, severity, summary) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (tenant_id, oversight_id) DO UPDATE SET updated_at = manager_exceptions.updated_at RETURNING {EXCEPTION_COLUMNS}"
    );
    let exception = sqlx::query_as::<_, ManagerException>(&query)
        .bind(record.tenant_id)
        .bind(record.id)
        .bind(directive_id)
        .bind(severity)
        .bind(summary)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO manager_exception_events (tenant_id, exception_id, state, actor_user_id, summary) SELECT $1,$2,'open',$3,$4 WHERE NOT EXISTS (SELECT 1 FROM manager_exception_events WHERE tenant_id = $1 AND exception_id = $2)")
        .bind(record.tenant_id)
        .bind(exception.id)
        .bind(actor_user_id)
        .bind(summary)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO manager_webhook_deliveries (tenant_id, exception_id, idempotency_key, state) SELECT $1,$2,$3,'pending' WHERE EXISTS (SELECT 1 FROM manager_webhook_settings WHERE tenant_id = $1 AND enabled) ON CONFLICT (tenant_id, exception_id) DO NOTHING")
        .bind(record.tenant_id)
        .bind(exception.id)
        .bind(format!("manager-exception:{}", exception.id))
        .execute(&mut **tx)
        .await?;
    Ok(exception)
}

async fn apply_directive_to_oversight(
    tx: &mut Transaction<'_, Postgres>,
    directive: &CorrectiveDirective,
    user_id: Uuid,
    input: &TransitionDirectiveInput,
) -> Result<(), ManagerOversightError> {
    let record_query = format!(
        "SELECT {OVERSIGHT_COLUMNS} FROM manager_oversight_records WHERE tenant_id = $1 AND id = $2 FOR UPDATE"
    );
    let record = sqlx::query_as::<_, OversightRecord>(&record_query)
        .bind(directive.tenant_id)
        .bind(directive.oversight_id)
        .fetch_one(&mut **tx)
        .await?;
    let (stage, event_state, outcome, verification, finished) = match input.state.as_str() {
        "acknowledged" => ("target", "completed", "pending", "pending", false),
        "planned" => ("plan", "completed", "pending", "pending", false),
        "executing" => ("execute", "started", "pending", "pending", false),
        "verifying" => ("verify", "started", "pending", "pending", false),
        "resolved" => ("verify", "completed", "succeeded", "verified", true),
        "failed" => (
            stage_for_state(&record.current_stage),
            "failed",
            "failed",
            "failed",
            true,
        ),
        "escalated" => (
            stage_for_state(&record.current_stage),
            "failed",
            "safe_stopped",
            "failed",
            true,
        ),
        _ => return Ok(()),
    };
    let evidence_refs = if input.evidence_refs.is_empty() {
        record.evidence_refs.clone()
    } else {
        serde_json::to_value(&input.evidence_refs).expect("string vec serializes")
    };
    sqlx::query("UPDATE manager_oversight_records SET current_stage = $3, outcome = $4, verification_state = $5, evidence_refs = $6, before_summary = COALESCE($7, before_summary), after_summary = COALESCE($8, after_summary), action_summary = $9, revision = revision + 1, updated_at = NOW(), finished_at = CASE WHEN $10 THEN NOW() ELSE NULL END WHERE tenant_id = $1 AND id = $2")
        .bind(record.tenant_id)
        .bind(record.id)
        .bind(stage)
        .bind(outcome)
        .bind(verification)
        .bind(evidence_refs)
        .bind(&input.before_summary)
        .bind(&input.after_summary)
        .bind(&input.summary)
        .bind(finished)
        .execute(&mut **tx)
        .await?;
    insert_oversight_event(
        tx,
        &record,
        &StageEventInput {
            stage: stage.to_string(),
            state: event_state.to_string(),
            summary: input.summary.clone(),
            evidence_refs: input.evidence_refs.clone(),
        },
        Some(user_id),
    )
    .await?;
    if matches!(input.state.as_str(), "failed" | "escalated") {
        let severity = if input.state == "escalated" {
            "critical"
        } else {
            "error"
        };
        insert_exception(
            tx,
            &record,
            Some(directive.id),
            severity,
            &input.summary,
            Some(user_id),
        )
        .await?;
    }
    Ok(())
}

fn validate_record_input(input: &RecordOutcomeInput) -> Result<(), ManagerOversightError> {
    if !matches!(
        input.source_kind.as_str(),
        "workbench_action" | "bundle_job" | "knowledge_job" | "knowledge_event" | "directive"
    ) {
        return Err(invalid("unsupported source_kind"));
    }
    validate_text("source_id", &input.source_id, 300)?;
    validate_text("agent_label", &input.agent_label, 200)?;
    validate_text("agent_role", &input.agent_role, 100)?;
    validate_text("goal", &input.goal, 2_000)?;
    validate_target(
        &input.target_kind,
        &input.target_id,
        input.target_revision.as_deref(),
    )?;
    if !matches!(
        input.policy_decision.as_str(),
        "auto" | "review" | "deny" | "unknown"
    ) {
        return Err(invalid("unsupported policy_decision"));
    }
    if !matches!(
        input.current_stage.as_str(),
        "target" | "plan" | "execute" | "verify"
    ) {
        return Err(invalid("unsupported current_stage"));
    }
    if !matches!(
        input.outcome.as_str(),
        "pending" | "pending_review" | "succeeded" | "failed" | "safe_stopped" | "cancelled"
    ) {
        return Err(invalid("unsupported outcome"));
    }
    if !matches!(
        input.verification_state.as_str(),
        "pending" | "verified" | "failed" | "not_provided"
    ) {
        return Err(invalid("unsupported verification_state"));
    }
    validate_text("action_summary", &input.action_summary, 2_000)?;
    validate_optional_text("before_summary", input.before_summary.as_deref(), 4_000)?;
    validate_optional_text("after_summary", input.after_summary.as_deref(), 4_000)?;
    validate_optional_text("rollback_summary", input.rollback_summary.as_deref(), 4_000)?;
    validate_string_list("evidence_refs", &input.evidence_refs, 64, 500)?;
    if input.duration_ms < 0 || input.cost_minor_units < 0 {
        return Err(invalid("duration and cost cannot be negative"));
    }
    if input.events.is_empty() || input.events.len() > 16 {
        return Err(invalid("outcome history must contain 1..16 events"));
    }
    for event in &input.events {
        validate_stage_event(event)?;
    }
    if input.exception_severity.is_some() != input.exception_summary.is_some() {
        return Err(invalid(
            "exception severity and summary must be provided together",
        ));
    }
    Ok(())
}

fn validate_stage_event(event: &StageEventInput) -> Result<(), ManagerOversightError> {
    if !matches!(
        event.stage.as_str(),
        "target" | "plan" | "execute" | "verify"
    ) {
        return Err(invalid("unsupported event stage"));
    }
    if !matches!(
        event.state.as_str(),
        "recorded" | "started" | "completed" | "failed" | "skipped"
    ) {
        return Err(invalid("unsupported event state"));
    }
    validate_text("event_summary", &event.summary, 2_000)?;
    validate_string_list("event_evidence_refs", &event.evidence_refs, 64, 500)
}

fn validate_target(
    kind: &str,
    id: &str,
    revision: Option<&str>,
) -> Result<(), ManagerOversightError> {
    if !matches!(
        kind,
        "action" | "job" | "configuration" | "knowledge_revision"
    ) {
        return Err(invalid("unsupported target_kind"));
    }
    validate_text("target_id", id, 512)?;
    validate_optional_text("target_revision", revision, 256)
}

fn validate_transition(
    current: &CorrectiveDirective,
    input: &TransitionDirectiveInput,
) -> Result<(), ManagerOversightError> {
    let valid = matches!(
        (current.state.as_str(), input.state.as_str()),
        ("issued", "acknowledged")
            | ("acknowledged", "planned")
            | ("planned", "executing")
            | ("executing", "verifying")
            | ("verifying", "resolved")
            | (
                "issued" | "acknowledged" | "planned" | "executing" | "verifying",
                "failed" | "escalated"
            )
    );
    if !valid {
        return Err(ManagerOversightError::Conflict);
    }
    if input.state == "planned" && input.plan_summary.as_deref().map_or(true, str::is_empty) {
        return Err(invalid("planned transition requires plan_summary"));
    }
    if input.state == "verifying"
        && input
            .execution_summary
            .as_deref()
            .map_or(true, str::is_empty)
        && current.execution_summary.is_none()
    {
        return Err(invalid("verifying transition requires execution_summary"));
    }
    if input.state == "resolved"
        && (input
            .verification_summary
            .as_deref()
            .map_or(true, str::is_empty)
            || input.evidence_refs.is_empty())
    {
        return Err(invalid(
            "resolved transition requires verification_summary and Evidence references",
        ));
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, max: usize) -> Result<(), ManagerOversightError> {
    let len = value.trim().chars().count();
    if len == 0 || len > max {
        return Err(invalid(format!("{name} must contain 1..{max} characters")));
    }
    Ok(())
}

fn validate_optional_text(
    name: &str,
    value: Option<&str>,
    max: usize,
) -> Result<(), ManagerOversightError> {
    if let Some(value) = value {
        validate_text(name, value, max)?;
    }
    Ok(())
}

fn validate_string_list(
    name: &str,
    values: &[String],
    max_items: usize,
    max_chars: usize,
) -> Result<(), ManagerOversightError> {
    if values.len() > max_items {
        return Err(invalid(format!("{name} exceeds {max_items} items")));
    }
    for value in values {
        validate_text(name, value, max_chars)?;
    }
    Ok(())
}

fn stage_for_state(current: &str) -> &str {
    match current {
        "target" | "plan" | "execute" | "verify" => current,
        _ => "verify",
    }
}

fn invalid(detail: impl Into<String>) -> ManagerOversightError {
    ManagerOversightError::InvalidInput(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directive(state: &str) -> CorrectiveDirective {
        CorrectiveDirective {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            oversight_id: Uuid::new_v4(),
            target_kind: "job".into(),
            target_id: "job-one".into(),
            target_revision: None,
            issued_by_user_id: Uuid::new_v4(),
            instruction: "Correct the result".into(),
            desired_outcome: "Verified state".into(),
            constraints: Value::Array(Vec::new()),
            priority: "normal".into(),
            state: state.into(),
            plan_summary: None,
            execution_summary: None,
            verification_summary: None,
            before_summary: None,
            after_summary: None,
            evidence_refs: Value::Array(Vec::new()),
            due_at: None,
            revision: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            finished_at: None,
        }
    }

    fn transition(state: &str) -> TransitionDirectiveInput {
        TransitionDirectiveInput {
            expected_revision: 1,
            state: state.into(),
            summary: "State changed".into(),
            plan_summary: None,
            execution_summary: None,
            verification_summary: None,
            before_summary: None,
            after_summary: None,
            evidence_refs: Vec::new(),
        }
    }

    #[test]
    fn directive_lifecycle_rejects_skips_and_unproven_resolution() {
        assert!(matches!(
            validate_transition(&directive("issued"), &transition("planned")),
            Err(ManagerOversightError::Conflict)
        ));
        let mut resolving = transition("resolved");
        assert!(validate_transition(&directive("verifying"), &resolving).is_err());
        resolving.verification_summary = Some("Desired state observed".into());
        resolving.evidence_refs = vec!["outcome:one".into()];
        assert!(validate_transition(&directive("verifying"), &resolving).is_ok());
    }
}
