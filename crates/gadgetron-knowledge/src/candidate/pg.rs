//! Postgres-backed implementation of the Knowledge Candidate lifecycle contract.
//!
//! Authority: `docs/design/core/knowledge-candidate-curation.md` §2.1, D-20260418-21.
//!
//! # Storage model
//!
//! Three append-only tables, all defined in migration `20260418000001_activity_capture.sql`:
//! - `activity_events` — immutable fact rows (one per `append_activity` call).
//! - `knowledge_candidates` — projection rows; `disposition` is the single mutable column.
//! - `candidate_decisions` — append-only decision log (one row per `decide_candidate` call).
//!
//! # Error mapping
//!
//! `sqlx::Error` is mapped to `GadgetronError` using the same helper as the
//! xaas quota / validator layer — `pg_to_gadgetron` below. The function is
//! a private copy of `gadgetron_xaas::error::sqlx_to_gadgetron` to avoid a
//! circular crate dependency (knowledge → xaas is not permitted per the
//! workspace dependency graph).

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gadgetron_core::{
    error::{DatabaseErrorKind, GadgetronError, KnowledgeErrorKind},
    knowledge::{
        candidate::{
            ActivityCaptureStore, CandidateDecision, CandidateDecisionKind, CandidateHint,
            CaptureResult, CapturedActivityEvent, KnowledgeCandidate,
            KnowledgeCandidateDisposition,
        },
        AuthenticatedContext,
    },
};
use sqlx::PgPool;
use uuid::Uuid;

use super::{enum_snake_case_label, resolve_initial_disposition};

// ---------------------------------------------------------------------------
// Error mapping — mirrors gadgetron_xaas::error::sqlx_to_gadgetron without
// the circular crate dependency.
// ---------------------------------------------------------------------------

fn pg_to_gadgetron(e: sqlx::Error) -> GadgetronError {
    let kind = match &e {
        sqlx::Error::RowNotFound => DatabaseErrorKind::RowNotFound,
        sqlx::Error::PoolTimedOut => DatabaseErrorKind::PoolTimeout,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => DatabaseErrorKind::ConnectionFailed,
        sqlx::Error::Database(_) => DatabaseErrorKind::Constraint,
        sqlx::Error::Migrate(_) => DatabaseErrorKind::MigrationFailed,
        _ => DatabaseErrorKind::Other,
    };
    GadgetronError::Database {
        kind,
        message: e.to_string(),
    }
}

// ---------------------------------------------------------------------------
// PgActivityCaptureStore
// ---------------------------------------------------------------------------

/// Postgres-backed append-only store for activity events, candidates, and decisions.
///
/// Uses `sqlx::PgPool` for async Postgres access. Construction is builder-style:
///
/// ```ignore
/// let store = PgActivityCaptureStore::new(pool)
///     .with_confirmation_gate(vec!["org_write".into(), "destructive_action".into()]);
/// ```
///
/// `require_user_confirmation_for` holds a list of tag strings; any hint whose
/// `tags` list contains at least one of these strings gets an initial disposition
/// of `PendingUserConfirmation` instead of `PendingPennyDecision`.
#[derive(Debug, Clone)]
pub struct PgActivityCaptureStore {
    pool: PgPool,
    require_user_confirmation_for: Vec<String>,
}

impl PgActivityCaptureStore {
    /// Create a new store backed by `pool`. The confirmation gate is empty by
    /// default: all candidates start with `PendingPennyDecision`.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            require_user_confirmation_for: Vec::new(),
        }
    }

    /// Override the confirmation gate. Hints whose `tags` intersect with
    /// `gates` receive an initial disposition of `PendingUserConfirmation`.
    pub fn with_confirmation_gate(mut self, gates: Vec<String>) -> Self {
        self.require_user_confirmation_for = gates;
        self
    }
}

// ---------------------------------------------------------------------------
// ActivityCaptureStore impl
// ---------------------------------------------------------------------------

#[async_trait]
impl ActivityCaptureStore for PgActivityCaptureStore {
    async fn append_activity(
        &self,
        _actor: &AuthenticatedContext,
        event: CapturedActivityEvent,
    ) -> CaptureResult<()> {
        sqlx::query(
            "INSERT INTO activity_events (
                id, tenant_id, actor_user_id, request_id,
                origin, kind, title, summary,
                source_bundle, source_capability, audit_event_id,
                facts, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(event.id)
        .bind(event.tenant_id)
        .bind(event.actor_user_id)
        .bind(event.request_id)
        .bind(enum_snake_case_label(&event.origin))
        .bind(enum_snake_case_label(&event.kind))
        .bind(&event.title)
        .bind(&event.summary)
        .bind(&event.source_bundle)
        .bind(&event.source_capability)
        .bind(event.audit_event_id)
        .bind(&event.facts)
        .bind(event.created_at)
        .execute(&self.pool)
        .await
        .map_err(pg_to_gadgetron)?;
        Ok(())
    }

    async fn append_candidate(
        &self,
        _actor: &AuthenticatedContext,
        activity_event_id: Uuid,
        hint: CandidateHint,
    ) -> CaptureResult<KnowledgeCandidate> {
        let disposition = resolve_initial_disposition(&hint, &self.require_user_confirmation_for);
        let disposition_label = enum_snake_case_label(&disposition);

        let candidate_id = Uuid::new_v4();
        let now = Utc::now();

        // Provenance matches InMemory shape: hint_reason + hint_tags.
        let mut provenance: BTreeMap<String, String> = BTreeMap::new();
        if let Some(reason) = hint.reason.as_deref() {
            provenance.insert("hint_reason".to_string(), reason.to_string());
        }
        if !hint.tags.is_empty() {
            provenance.insert("hint_tags".to_string(), hint.tags.join(","));
        }
        let provenance_json = serde_json::to_value(&provenance).unwrap_or_default();

        // INSERT with subselect: tenant_id and actor_user_id come from the
        // activity_events row so we never need a separate SELECT round-trip.
        // RETURNING lets us confirm the row was written and retrieve identity.
        let row = sqlx::query_as::<_, (Uuid, Uuid)>(
            "INSERT INTO knowledge_candidates (
                id, activity_event_id, tenant_id, actor_user_id,
                summary, proposed_path, provenance, disposition, created_at
            )
            SELECT $1, $2, ae.tenant_id, ae.actor_user_id, $3, $4, $5, $6, $7
            FROM activity_events ae
            WHERE ae.id = $2
            RETURNING tenant_id, actor_user_id",
        )
        .bind(candidate_id)
        .bind(activity_event_id)
        .bind(&hint.summary)
        .bind(&hint.proposed_path)
        .bind(&provenance_json)
        .bind(&disposition_label)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_to_gadgetron)?;

        let (tenant_id, actor_user_id) = row.ok_or_else(|| GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::DocumentNotFound {
                path: format!("activity_event/{activity_event_id}"),
            },
            message: format!(
                "cannot append candidate: activity event {activity_event_id} not found in capture store"
            ),
        })?;

        Ok(KnowledgeCandidate {
            id: candidate_id,
            activity_event_id,
            tenant_id,
            actor_user_id,
            summary: hint.summary,
            proposed_path: hint.proposed_path,
            provenance,
            disposition,
            created_at: now,
        })
    }

    async fn decide_candidate(
        &self,
        _actor: &AuthenticatedContext,
        decision: CandidateDecision,
    ) -> CaptureResult<KnowledgeCandidate> {
        let next_disposition = match decision.decision {
            CandidateDecisionKind::Accept => KnowledgeCandidateDisposition::Accepted,
            CandidateDecisionKind::Reject => KnowledgeCandidateDisposition::Rejected,
            CandidateDecisionKind::EscalateToUser => {
                KnowledgeCandidateDisposition::PendingUserConfirmation
            }
            _ => {
                return Err(GadgetronError::Knowledge {
                    kind: KnowledgeErrorKind::InvalidQuery {
                        reason: format!(
                            "unsupported decision kind {:?}; KC-1c supports Accept / Reject / EscalateToUser only",
                            decision.decision
                        ),
                    },
                    message: "unknown candidate decision kind".to_string(),
                });
            }
        };
        let next_label = enum_snake_case_label(&next_disposition);

        // Transaction: update candidate disposition + append decision row.
        let mut tx = self.pool.begin().await.map_err(pg_to_gadgetron)?;

        // UPDATE + RETURNING all candidate columns.
        let row: Option<CandidateRow> = sqlx::query_as::<_, CandidateRow>(
            "UPDATE knowledge_candidates
             SET disposition = $1
             WHERE id = $2
             RETURNING id, activity_event_id, tenant_id, actor_user_id,
                       summary, proposed_path, provenance, disposition, created_at",
        )
        .bind(&next_label)
        .bind(decision.candidate_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_to_gadgetron)?;

        let row = row.ok_or_else(|| GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::DocumentNotFound {
                path: format!("candidate/{}", decision.candidate_id),
            },
            message: format!(
                "cannot decide candidate {}: not found in capture store",
                decision.candidate_id
            ),
        })?;

        // Append decision audit row.
        sqlx::query(
            "INSERT INTO candidate_decisions
                (candidate_id, decision, decided_by_user_id, decided_by_penny, rationale)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(decision.candidate_id)
        .bind(enum_snake_case_label(&decision.decision))
        .bind(decision.decided_by_user_id)
        .bind(decision.decided_by_penny)
        .bind(&decision.rationale)
        .execute(&mut *tx)
        .await
        .map_err(pg_to_gadgetron)?;

        tx.commit().await.map_err(pg_to_gadgetron)?;

        row.into_candidate(next_disposition)
    }

    async fn list_candidates(
        &self,
        _actor: &AuthenticatedContext,
        limit: usize,
        only_pending: bool,
    ) -> CaptureResult<Vec<KnowledgeCandidate>> {
        let rows: Vec<CandidateRow> = if only_pending {
            sqlx::query_as::<_, CandidateRow>(
                "SELECT id, activity_event_id, tenant_id, actor_user_id,
                        summary, proposed_path, provenance, disposition, created_at
                 FROM knowledge_candidates
                 WHERE disposition IN ('pending_penny_decision', 'pending_user_confirmation')
                 ORDER BY created_at DESC, id DESC
                 LIMIT $1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(pg_to_gadgetron)?
        } else {
            sqlx::query_as::<_, CandidateRow>(
                "SELECT id, activity_event_id, tenant_id, actor_user_id,
                        summary, proposed_path, provenance, disposition, created_at
                 FROM knowledge_candidates
                 ORDER BY created_at DESC, id DESC
                 LIMIT $1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(pg_to_gadgetron)?
        };

        rows.into_iter()
            .map(|r| r.into_candidate_from_label())
            .collect()
    }

    async fn get_candidate(
        &self,
        _actor: &AuthenticatedContext,
        id: Uuid,
    ) -> CaptureResult<Option<KnowledgeCandidate>> {
        let row: Option<CandidateRow> = sqlx::query_as::<_, CandidateRow>(
            "SELECT id, activity_event_id, tenant_id, actor_user_id,
                    summary, proposed_path, provenance, disposition, created_at
             FROM knowledge_candidates
             WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_to_gadgetron)?;

        row.map(|r| r.into_candidate_from_label()).transpose()
    }
}

// ---------------------------------------------------------------------------
// CandidateRow — sqlx FromRow projection
// ---------------------------------------------------------------------------

/// Raw DB row for `knowledge_candidates`. `disposition` is stored as TEXT and
/// must be decoded back into `KnowledgeCandidateDisposition` via
/// `disposition_from_label`.
#[derive(sqlx::FromRow)]
struct CandidateRow {
    id: Uuid,
    activity_event_id: Uuid,
    tenant_id: Uuid,
    actor_user_id: Uuid,
    summary: String,
    proposed_path: Option<String>,
    /// Stored as JSONB, decoded to `BTreeMap<String, String>`.
    provenance: serde_json::Value,
    disposition: String,
    created_at: DateTime<Utc>,
}

impl CandidateRow {
    /// Decode `disposition` text label to enum, then build `KnowledgeCandidate`.
    fn into_candidate_from_label(self) -> CaptureResult<KnowledgeCandidate> {
        let d =
            disposition_from_label(&self.disposition).ok_or_else(|| GadgetronError::Database {
                kind: DatabaseErrorKind::QueryFailed,
                message: format!(
                    "unrecognised disposition label {:?} in knowledge_candidates row {}",
                    self.disposition, self.id
                ),
            })?;
        self.into_candidate(d)
    }

    /// Build `KnowledgeCandidate` with a pre-resolved `disposition`.
    fn into_candidate(
        self,
        disposition: KnowledgeCandidateDisposition,
    ) -> CaptureResult<KnowledgeCandidate> {
        let provenance: BTreeMap<String, String> =
            serde_json::from_value(self.provenance).unwrap_or_default();
        Ok(KnowledgeCandidate {
            id: self.id,
            activity_event_id: self.activity_event_id,
            tenant_id: self.tenant_id,
            actor_user_id: self.actor_user_id,
            summary: self.summary,
            proposed_path: self.proposed_path,
            provenance,
            disposition,
            created_at: self.created_at,
        })
    }
}

/// Decode the snake_case disposition label stored in Postgres back to the enum.
fn disposition_from_label(label: &str) -> Option<KnowledgeCandidateDisposition> {
    match label {
        "pending_penny_decision" => Some(KnowledgeCandidateDisposition::PendingPennyDecision),
        "pending_user_confirmation" => Some(KnowledgeCandidateDisposition::PendingUserConfirmation),
        "accepted" => Some(KnowledgeCandidateDisposition::Accepted),
        "rejected" => Some(KnowledgeCandidateDisposition::Rejected),
        _ => None,
    }
}
