//! PostgreSQL ownership for immutable tenant policy revisions and decisions.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gadgetron_core::{
    agent::GadgetsConfig,
    policy::{
        EnforcementPath, PolicyAuthorization, PolicyDecision, PolicyDecisionTrace, PolicyDocument,
        PolicyError, PolicyEvaluation, PolicyEvaluationError, PolicyEvaluationRequest,
        PolicyEvaluator, PolicyIdentity, PolicyInput, PolicyReviewState,
    },
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRevisionSource {
    LegacyMigration,
    Manager,
    Rollback,
    System,
}

impl PolicyRevisionSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyMigration => "legacy_migration",
            Self::Manager => "manager",
            Self::Rollback => "rollback",
            Self::System => "system",
        }
    }

    fn parse(value: &str) -> Result<Self, PolicyStoreError> {
        match value {
            "legacy_migration" => Ok(Self::LegacyMigration),
            "manager" => Ok(Self::Manager),
            "rollback" => Ok(Self::Rollback),
            "system" => Ok(Self::System),
            _ => Err(PolicyStoreError::InvalidPersisted(format!(
                "unknown policy revision source {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRevision {
    pub tenant_id: Uuid,
    #[serde(flatten)]
    pub identity: PolicyIdentity,
    pub source: PolicyRevisionSource,
    pub document: PolicyDocument,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_modes: Option<GadgetsConfig>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub superseded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecisionEvent {
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub policy: PolicyIdentity,
    pub input: PolicyInput,
    pub input_hash: String,
    pub trace: PolicyDecisionTrace,
    pub trace_hash: String,
    pub decision: PolicyDecision,
    pub enforcement_path: String,
    pub authorization: String,
    pub approval_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

pub struct PolicyDecisionRecord<'a> {
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub enforcement_path: EnforcementPath,
    pub authorization: PolicyAuthorization,
    pub approval_id: Option<Uuid>,
    pub input: &'a PolicyInput,
    pub trace: &'a PolicyDecisionTrace,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyStoreError {
    #[error("policy revision not found")]
    NotFound,
    #[error("policy revision conflict: current revision is {current_revision}")]
    RevisionConflict { current_revision: i64 },
    #[error("invalid persisted policy: {0}")]
    InvalidPersisted(String),
    #[error("policy decision trace does not match the stored revision and input")]
    TraceMismatch,
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, sqlx::FromRow)]
struct PolicyRevisionRow {
    tenant_id: Uuid,
    policy_id: Uuid,
    revision: i64,
    schema_version: i32,
    source: String,
    document: serde_json::Value,
    document_hash: String,
    legacy_modes: Option<serde_json::Value>,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    superseded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, sqlx::FromRow)]
struct PolicyDecisionEventRow {
    event_id: Uuid,
    tenant_id: Uuid,
    policy_id: Uuid,
    policy_revision: i64,
    policy_hash: String,
    input: serde_json::Value,
    input_hash: String,
    trace: serde_json::Value,
    trace_hash: String,
    decision: String,
    enforcement_path: String,
    authorization_state: String,
    approval_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct PgPolicyEvaluator {
    pool: PgPool,
    legacy_modes: GadgetsConfig,
}

impl PgPolicyEvaluator {
    pub fn new(pool: PgPool, legacy_modes: GadgetsConfig) -> Self {
        Self { pool, legacy_modes }
    }

    async fn active(&self, tenant_id: Uuid) -> Result<PolicyRevision, PolicyStoreError> {
        ensure_legacy_policy(&self.pool, tenant_id, None, &self.legacy_modes).await
    }
}

#[async_trait]
impl PolicyEvaluator for PgPolicyEvaluator {
    async fn active_identity(
        &self,
        tenant_id: Uuid,
    ) -> Result<PolicyIdentity, PolicyEvaluationError> {
        self.active(tenant_id)
            .await
            .map(|revision| revision.identity)
            .map_err(evaluation_error)
    }

    async fn evaluate(
        &self,
        request: PolicyEvaluationRequest,
    ) -> Result<PolicyEvaluation, PolicyEvaluationError> {
        let revision = match request.pinned_policy.as_ref() {
            Some(identity) => {
                let revision = policy_revision(
                    &self.pool,
                    request.tenant_id,
                    identity.policy_id,
                    identity.revision,
                )
                .await
                .map_err(evaluation_error)?;
                if revision.identity != *identity {
                    return Err(PolicyEvaluationError {
                        code: "policy_revision_mismatch",
                        detail: "Pinned policy identity no longer matches immutable storage".into(),
                    });
                }
                revision
            }
            None => self
                .active(request.tenant_id)
                .await
                .map_err(evaluation_error)?,
        };
        let trace = revision
            .document
            .evaluate(revision.identity, &request.input)
            .map_err(|error| PolicyEvaluationError {
                code: "policy_input_invalid",
                detail: error.to_string(),
            })?;
        let authorization = authorization_for(trace.decision, request.review_state);
        let event_id = Uuid::new_v4();
        let event = record_decision(
            &self.pool,
            PolicyDecisionRecord {
                tenant_id: request.tenant_id,
                event_id,
                enforcement_path: request.path,
                authorization,
                approval_id: request.approval_id,
                input: &request.input,
                trace: &trace,
            },
        )
        .await
        .map_err(evaluation_error)?;
        Ok(PolicyEvaluation {
            event_id,
            trace: event.trace,
            trace_hash: event.trace_hash,
            authorization,
        })
    }
}

fn authorization_for(
    decision: PolicyDecision,
    review_state: PolicyReviewState,
) -> PolicyAuthorization {
    match decision {
        PolicyDecision::Auto => PolicyAuthorization::Auto,
        PolicyDecision::Deny => PolicyAuthorization::Denied,
        PolicyDecision::Review if review_state == PolicyReviewState::Approved => {
            PolicyAuthorization::ApprovedReview
        }
        PolicyDecision::Review => PolicyAuthorization::PendingReview,
    }
}

fn evaluation_error(error: PolicyStoreError) -> PolicyEvaluationError {
    PolicyEvaluationError {
        code: match &error {
            PolicyStoreError::NotFound => "policy_not_found",
            PolicyStoreError::RevisionConflict { .. } => "policy_revision_conflict",
            PolicyStoreError::TraceMismatch => "policy_trace_mismatch",
            PolicyStoreError::InvalidPersisted(_) => "policy_storage_invalid",
            PolicyStoreError::Policy(_) => "policy_input_invalid",
            PolicyStoreError::Database(_) => "policy_store_unavailable",
        },
        detail: error.to_string(),
    }
}

const REVISION_COLUMNS: &str = "tenant_id, policy_id, revision, schema_version, source, document, document_hash, legacy_modes, created_by, created_at, superseded_at";

pub async fn active_policy(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Option<PolicyRevision>, PolicyStoreError> {
    let query = format!(
        "SELECT {REVISION_COLUMNS} FROM policy_revisions WHERE tenant_id = $1 AND superseded_at IS NULL"
    );
    let row = sqlx::query_as::<_, PolicyRevisionRow>(&query)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await?;
    row.map(row_to_revision).transpose()
}

pub async fn policy_revision(
    pool: &PgPool,
    tenant_id: Uuid,
    policy_id: Uuid,
    revision: i64,
) -> Result<PolicyRevision, PolicyStoreError> {
    let query = format!(
        "SELECT {REVISION_COLUMNS} FROM policy_revisions WHERE tenant_id = $1 AND policy_id = $2 AND revision = $3"
    );
    let row = sqlx::query_as::<_, PolicyRevisionRow>(&query)
        .bind(tenant_id)
        .bind(policy_id)
        .bind(revision)
        .fetch_optional(pool)
        .await?
        .ok_or(PolicyStoreError::NotFound)?;
    row_to_revision(row)
}

pub async fn ensure_legacy_policy(
    pool: &PgPool,
    tenant_id: Uuid,
    created_by: Option<Uuid>,
    modes: &GadgetsConfig,
) -> Result<PolicyRevision, PolicyStoreError> {
    let mut tx = pool.begin().await?;
    lock_tenant(&mut tx, tenant_id).await?;
    if let Some(row) = active_policy_tx(&mut tx, tenant_id).await? {
        tx.commit().await?;
        return row_to_revision(row);
    }

    let document = PolicyDocument::from_legacy_gadget_modes(modes)?;
    let document_hash = document.digest()?;
    let policy_id = Uuid::new_v4();
    let row = insert_revision(
        &mut tx,
        tenant_id,
        policy_id,
        1,
        PolicyRevisionSource::LegacyMigration,
        &document,
        &document_hash,
        Some(modes),
        created_by,
    )
    .await?;
    tx.commit().await?;
    row_to_revision(row)
}

pub async fn create_revision(
    pool: &PgPool,
    tenant_id: Uuid,
    created_by: Option<Uuid>,
    expected_revision: i64,
    source: PolicyRevisionSource,
    document: &PolicyDocument,
    legacy_modes: Option<&GadgetsConfig>,
) -> Result<PolicyRevision, PolicyStoreError> {
    document.validate()?;
    if matches!(source, PolicyRevisionSource::LegacyMigration) {
        return Err(PolicyStoreError::InvalidPersisted(
            "legacy_migration source is reserved for the first revision".to_string(),
        ));
    }
    if let Some(modes) = legacy_modes {
        modes
            .validate()
            .map_err(|error| PolicyStoreError::InvalidPersisted(error.to_string()))?;
        if PolicyDocument::from_legacy_gadget_modes(modes)? != *document {
            return Err(PolicyStoreError::InvalidPersisted(
                "legacy mode snapshot does not match the policy document".to_string(),
            ));
        }
    }

    let mut tx = pool.begin().await?;
    lock_tenant(&mut tx, tenant_id).await?;
    let current = active_policy_tx(&mut tx, tenant_id)
        .await?
        .ok_or(PolicyStoreError::NotFound)?;
    if current.revision != expected_revision {
        return Err(PolicyStoreError::RevisionConflict {
            current_revision: current.revision,
        });
    }
    sqlx::query(
        "UPDATE policy_revisions SET superseded_at = NOW() WHERE tenant_id = $1 AND policy_id = $2 AND revision = $3 AND superseded_at IS NULL",
    )
    .bind(tenant_id)
    .bind(current.policy_id)
    .bind(current.revision)
    .execute(&mut *tx)
    .await?;
    let document_hash = document.digest()?;
    let row = insert_revision(
        &mut tx,
        tenant_id,
        current.policy_id,
        current.revision + 1,
        source,
        document,
        &document_hash,
        legacy_modes,
        created_by,
    )
    .await?;
    tx.commit().await?;
    row_to_revision(row)
}

pub async fn record_decision(
    pool: &PgPool,
    record: PolicyDecisionRecord<'_>,
) -> Result<PolicyDecisionEvent, PolicyStoreError> {
    let PolicyDecisionRecord {
        tenant_id,
        event_id,
        enforcement_path,
        authorization,
        approval_id,
        input,
        trace,
    } = record;
    let revision = policy_revision(
        pool,
        tenant_id,
        trace.policy.policy_id,
        trace.policy.revision,
    )
    .await?;
    let expected = revision
        .document
        .evaluate(revision.identity.clone(), input)?;
    let authorization_matches = matches!(
        (trace.decision, authorization),
        (PolicyDecision::Auto, PolicyAuthorization::Auto)
            | (PolicyDecision::Deny, PolicyAuthorization::Denied)
            | (
                PolicyDecision::Review,
                PolicyAuthorization::PendingReview | PolicyAuthorization::ApprovedReview
            )
    );
    if &expected != trace
        || trace.input_hash != input.digest()?
        || !authorization_matches
        || (authorization == PolicyAuthorization::ApprovedReview && approval_id.is_none())
    {
        return Err(PolicyStoreError::TraceMismatch);
    }
    let trace_hash = trace.digest()?;
    let row = sqlx::query_as::<_, PolicyDecisionEventRow>(
        r#"
        INSERT INTO policy_decision_events
            (event_id, tenant_id, policy_id, policy_revision, policy_hash,
             input, input_hash, trace, trace_hash, decision, enforcement_path,
             authorization_state, approval_id)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
        RETURNING event_id, tenant_id, policy_id, policy_revision, policy_hash,
                  input, input_hash, trace, trace_hash, decision, enforcement_path,
                  authorization_state, approval_id, created_at
        "#,
    )
    .bind(event_id)
    .bind(tenant_id)
    .bind(trace.policy.policy_id)
    .bind(trace.policy.revision)
    .bind(&trace.policy.document_hash)
    .bind(serde_json::to_value(input).map_err(PolicyError::from)?)
    .bind(&trace.input_hash)
    .bind(serde_json::to_value(trace).map_err(PolicyError::from)?)
    .bind(&trace_hash)
    .bind(trace.decision.as_str())
    .bind(enforcement_path.as_str())
    .bind(authorization.as_str())
    .bind(approval_id)
    .fetch_one(pool)
    .await?;
    row_to_decision(row)
}

pub async fn recent_decisions(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<Vec<PolicyDecisionEvent>, PolicyStoreError> {
    let rows = sqlx::query_as::<_, PolicyDecisionEventRow>(
        r#"
        SELECT event_id, tenant_id, policy_id, policy_revision, policy_hash,
               input, input_hash, trace, trace_hash, decision, enforcement_path,
               authorization_state, approval_id, created_at
        FROM policy_decision_events
        WHERE tenant_id = $1
        ORDER BY created_at DESC, event_id DESC
        LIMIT $2
        "#,
    )
    .bind(tenant_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(row_to_decision).collect()
}

async fn lock_tenant(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<(), PolicyStoreError> {
    let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE id = $1 FOR UPDATE")
        .bind(tenant_id)
        .fetch_optional(&mut **tx)
        .await?;
    if found.is_none() {
        return Err(PolicyStoreError::NotFound);
    }
    Ok(())
}

async fn active_policy_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<Option<PolicyRevisionRow>, PolicyStoreError> {
    let query = format!(
        "SELECT {REVISION_COLUMNS} FROM policy_revisions WHERE tenant_id = $1 AND superseded_at IS NULL FOR UPDATE"
    );
    Ok(sqlx::query_as::<_, PolicyRevisionRow>(&query)
        .bind(tenant_id)
        .fetch_optional(&mut **tx)
        .await?)
}

#[allow(clippy::too_many_arguments)]
async fn insert_revision(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    policy_id: Uuid,
    revision: i64,
    source: PolicyRevisionSource,
    document: &PolicyDocument,
    document_hash: &str,
    legacy_modes: Option<&GadgetsConfig>,
    created_by: Option<Uuid>,
) -> Result<PolicyRevisionRow, PolicyStoreError> {
    Ok(sqlx::query_as::<_, PolicyRevisionRow>(
        r#"
        INSERT INTO policy_revisions
            (tenant_id, policy_id, revision, schema_version, source, document,
             document_hash, legacy_modes, created_by)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        RETURNING tenant_id, policy_id, revision, schema_version, source, document,
                  document_hash, legacy_modes, created_by, created_at, superseded_at
        "#,
    )
    .bind(tenant_id)
    .bind(policy_id)
    .bind(revision)
    .bind(document.schema_version as i32)
    .bind(source.as_str())
    .bind(serde_json::to_value(document).map_err(PolicyError::from)?)
    .bind(document_hash)
    .bind(
        legacy_modes
            .map(serde_json::to_value)
            .transpose()
            .map_err(PolicyError::from)?,
    )
    .bind(created_by)
    .fetch_one(&mut **tx)
    .await?)
}

fn row_to_revision(row: PolicyRevisionRow) -> Result<PolicyRevision, PolicyStoreError> {
    let document: PolicyDocument = serde_json::from_value(row.document)
        .map_err(|error| PolicyStoreError::InvalidPersisted(error.to_string()))?;
    document.validate()?;
    if row.schema_version as u32 != document.schema_version
        || row.document_hash != document.digest()?
    {
        return Err(PolicyStoreError::InvalidPersisted(
            "policy document metadata or hash drift".to_string(),
        ));
    }
    let legacy_modes: Option<GadgetsConfig> = row
        .legacy_modes
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| PolicyStoreError::InvalidPersisted(error.to_string()))?;
    if let Some(modes) = &legacy_modes {
        modes
            .validate()
            .map_err(|error| PolicyStoreError::InvalidPersisted(error.to_string()))?;
        if PolicyDocument::from_legacy_gadget_modes(modes)? != document {
            return Err(PolicyStoreError::InvalidPersisted(
                "persisted legacy mode snapshot does not match the policy document".to_string(),
            ));
        }
    }
    Ok(PolicyRevision {
        tenant_id: row.tenant_id,
        identity: PolicyIdentity {
            policy_id: row.policy_id,
            revision: row.revision,
            document_hash: row.document_hash,
        },
        source: PolicyRevisionSource::parse(&row.source)?,
        document,
        legacy_modes,
        created_by: row.created_by,
        created_at: row.created_at,
        superseded_at: row.superseded_at,
    })
}

fn row_to_decision(row: PolicyDecisionEventRow) -> Result<PolicyDecisionEvent, PolicyStoreError> {
    let input: PolicyInput = serde_json::from_value(row.input)
        .map_err(|error| PolicyStoreError::InvalidPersisted(error.to_string()))?;
    let trace: PolicyDecisionTrace = serde_json::from_value(row.trace)
        .map_err(|error| PolicyStoreError::InvalidPersisted(error.to_string()))?;
    let decision = match row.decision.as_str() {
        "auto" => PolicyDecision::Auto,
        "review" => PolicyDecision::Review,
        "deny" => PolicyDecision::Deny,
        other => {
            return Err(PolicyStoreError::InvalidPersisted(format!(
                "unknown decision {other:?}"
            )))
        }
    };
    if row.policy_id != trace.policy.policy_id
        || row.policy_revision != trace.policy.revision
        || row.policy_hash != trace.policy.document_hash
        || row.input_hash != input.digest()?
        || row.input_hash != trace.input_hash
        || row.trace_hash != trace.digest()?
        || decision != trace.decision
    {
        return Err(PolicyStoreError::InvalidPersisted(
            "policy decision event hash or decision drift".to_string(),
        ));
    }
    Ok(PolicyDecisionEvent {
        event_id: row.event_id,
        tenant_id: row.tenant_id,
        policy: PolicyIdentity {
            policy_id: row.policy_id,
            revision: row.policy_revision,
            document_hash: row.policy_hash,
        },
        input,
        input_hash: row.input_hash,
        trace,
        trace_hash: row.trace_hash,
        decision,
        enforcement_path: row.enforcement_path,
        authorization: row.authorization_state,
        approval_id: row.approval_id,
        created_at: row.created_at,
    })
}
