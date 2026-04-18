//! Knowledge candidate lifecycle types and traits.
//!
//! Authority: `docs/design/core/knowledge-candidate-curation.md` (Approved
//! 2026-04-18). The capture plane / semantic plane split documented there is
//! realized as two traits here:
//!
//! - [`ActivityCaptureStore`] — append-only activity + candidate store.
//! - [`KnowledgeCandidateCoordinator`] — the "capture → candidate → optional
//!   materialization" lifecycle orchestrator that Penny and direct-action
//!   callers both talk to.
//!
//! # What lives here
//!
//! The **contract** side of the candidate lifecycle: wire types, enums, and
//! trait signatures. No storage, no coordinator logic. The in-memory
//! implementation lives in `gadgetron-knowledge::candidate`; Postgres-backed
//! stores land in KC-1b.
//!
//! # What does NOT live here
//!
//! - Persistence (SQL, migrations) — `gadgetron-xaas` will own that.
//! - Canonical knowledge writes — delegated through
//!   [`KnowledgeCandidateCoordinator::materialize_accepted_candidate`] to
//!   `KnowledgeService::write` at KC-1b. KC-1 returns `proposed_path` only.
//! - Penny prompt rendering / curation heuristics — those live in
//!   `gadgetron-penny`.
//!
//! # Wire stability
//!
//! `ActivityOrigin`, `ActivityKind`, `KnowledgeCandidateDisposition`, and
//! `CandidateDecisionKind` all use `#[serde(rename_all = "snake_case")]`.
//! Those strings appear in audit JSON and (eventually) in `gadgetron.toml`
//! path-rule keys, so renaming any variant is a wire-break.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::GadgetronError;

/// Result alias used by every trait method in this module.
///
/// Pinned to [`GadgetronError`] so candidate-plane failures surface through
/// the same HTTP / error-code taxonomy as every other subsystem.
pub type CaptureResult<T> = Result<T, GadgetronError>;

/// Origin of a captured activity event.
///
/// `#[non_exhaustive]` so KC-1b / KC-1c can add surfaces (e.g. `ExternalRuntime`)
/// without breaking match arms in callers.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOrigin {
    /// A human user triggered the action directly (workbench, CLI).
    UserDirect,
    /// Penny triggered the action while running a turn on the user's behalf.
    Penny,
    /// System-level event (scheduler tick, startup, shutdown, reconcile).
    System,
}

/// Kind of activity captured.
///
/// Candidate heuristics in KC-1b will key off these variants (e.g. only
/// `DirectAction` and `GadgetToolCall` produce operational-journal candidates
/// by default). `#[non_exhaustive]` to allow the taxonomy to grow.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// Human-initiated action through workbench / CLI.
    DirectAction,
    /// Bundle gadget tool call (Penny-driven or direct).
    GadgetToolCall,
    /// Approval request / decision recorded on the audit trail.
    ApprovalDecision,
    /// Runtime-observed state transition (e.g. node failure, restart loop).
    RuntimeObservation,
    /// Canonical knowledge writeback (materialization of an accepted candidate).
    KnowledgeWriteback,
}

/// A captured activity event — the immutable fact row.
///
/// Carries everything the candidate projection and audit trail need to
/// reconstruct "who did what, when" without round-tripping to any other
/// subsystem. `facts` is free-form JSON so capture-site payloads (tool args,
/// workbench command shape, runtime signal) can be stored without a
/// pre-registered schema. `audit_event_id` ties back to the existing audit
/// pipeline when present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedActivityEvent {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub request_id: Option<Uuid>,
    pub origin: ActivityOrigin,
    pub kind: ActivityKind,
    pub title: String,
    pub summary: String,
    pub source_bundle: Option<String>,
    pub source_capability: Option<String>,
    pub audit_event_id: Option<Uuid>,
    pub facts: Value,
    pub created_at: DateTime<Utc>,
}

/// Lifecycle disposition of a knowledge candidate.
///
/// Candidates move through these states as Penny and users make decisions.
/// `#[non_exhaustive]` allows KC-1b to add states (e.g. `Superseded`) without
/// breaking existing match arms in callers.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeCandidateDisposition {
    /// Penny has not yet rendered a decision; awaiting Penny's next turn.
    PendingPennyDecision,
    /// Penny has requested user confirmation before accepting.
    PendingUserConfirmation,
    /// Candidate was accepted; canonical writeback may be in progress.
    Accepted,
    /// Candidate was rejected and will not be written back.
    Rejected,
}

/// The kind of decision recorded against a candidate.
///
/// Used in `PennyCandidateDecisionRequest` and candidate audit events.
/// `#[non_exhaustive]` keeps the door open for KC-1b additions.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDecisionKind {
    /// Accept the candidate; trigger canonical writeback.
    Accept,
    /// Reject the candidate; no writeback.
    Reject,
    /// Penny cannot decide unilaterally; surface to the user for confirmation.
    EscalateToUser,
}

/// A knowledge candidate — the projection of activity event(s) into a
/// "should we remember this?" record.
///
/// `proposed_path` is `Option<String>` in KC-1. KC-1b will narrow it to a
/// typed `KnowledgePath` (see TODO below). `provenance` is a deterministic
/// `BTreeMap<String, String>` so serialization is byte-stable for the
/// fixture-diff contract tests and for audit replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeCandidate {
    pub id: Uuid,
    pub activity_event_id: Uuid,
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub summary: String,
    // TODO KC-1b: narrow `proposed_path` to `Option<gadgetron_core::knowledge::KnowledgePath>`
    // once that type exists (or is introduced in KC-1b). Keeping `String` here
    // avoids a cross-crate ordering dependency for KC-1.
    pub proposed_path: Option<String>,
    pub provenance: BTreeMap<String, String>,
    pub disposition: KnowledgeCandidateDisposition,
    pub created_at: DateTime<Utc>,
}

/// Advisory hint from a bundle / gadget / runtime about a candidate the
/// capture coordinator should consider creating.
///
/// Per the authority doc §2.4 rule 1-2: hints are advisory and never
/// authoritative — core re-injects actor/tenant/capability before persisting,
/// and `proposed_path` is sanitized before being retained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateHint {
    pub summary: String,
    pub proposed_path: Option<String>,
    pub tags: Vec<String>,
    pub reason: Option<String>,
}

/// A decision against a candidate, recorded on the append-only decision log.
///
/// Exactly one of `decided_by_user_id.is_some()` or `decided_by_penny == true`
/// must hold; enforcement is at the coordinator layer, not in the type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateDecision {
    pub candidate_id: Uuid,
    pub decision: CandidateDecisionKind,
    pub decided_by_user_id: Option<Uuid>,
    pub decided_by_penny: bool,
    pub rationale: Option<String>,
}

/// Canonical-store write payload for a materialized candidate.
///
/// KC-1 uses this shape so the coordinator signature is already the one
/// KC-1b needs — at KC-1b we replace the `materialize_accepted_candidate`
/// body with a real `KnowledgeService::write` call, not the trait signature.
///
/// # TODO KC-1b
///
/// Narrow `path: String` → `KnowledgePath` and `content: String` → typed
/// `KnowledgeDocumentWrite` once the knowledge-plane write contract lands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDocumentWrite {
    pub path: String,
    pub content: String,
    pub provenance: BTreeMap<String, String>,
}

/// Append-only activity + candidate storage contract.
///
/// `Debug` is required so implementations compose into `AppState` via
/// `Arc<dyn ActivityCaptureStore + Debug>` consistently with the other
/// trait-object patterns in `gadgetron-core`. `list_candidates` is a
/// read-side projection helper — implementations MUST return newest-first
/// ordering and MUST respect `only_pending` by returning only
/// `PendingPennyDecision` / `PendingUserConfirmation` rows when the flag
/// is set.
#[async_trait]
pub trait ActivityCaptureStore: Send + Sync + std::fmt::Debug {
    /// Append a captured activity event to the append-only event log.
    async fn append_activity(
        &self,
        actor: &crate::knowledge::AuthenticatedContext,
        event: CapturedActivityEvent,
    ) -> CaptureResult<()>;

    /// Append a candidate row tied to an activity event and return the
    /// stored row. Initial disposition MUST be `PendingPennyDecision`
    /// (authority doc §2.1 design rule 2).
    async fn append_candidate(
        &self,
        actor: &crate::knowledge::AuthenticatedContext,
        activity_event_id: Uuid,
        hint: CandidateHint,
    ) -> CaptureResult<KnowledgeCandidate>;

    /// Record a decision for an existing candidate and return the updated
    /// candidate row. Unknown candidate id MUST return a
    /// `GadgetronError::Knowledge { kind: DocumentNotFound, ... }` so callers
    /// can distinguish "never created" from "backend down".
    async fn decide_candidate(
        &self,
        actor: &crate::knowledge::AuthenticatedContext,
        decision: CandidateDecision,
    ) -> CaptureResult<KnowledgeCandidate>;

    /// Read-only projection — newest-first, filtered to pending dispositions
    /// when `only_pending` is true. `limit` is the maximum number of rows
    /// returned; callers clamp at their own layer (e.g. Penny bootstrap
    /// clamps to [1, 20]).
    async fn list_candidates(
        &self,
        actor: &crate::knowledge::AuthenticatedContext,
        limit: usize,
        only_pending: bool,
    ) -> CaptureResult<Vec<KnowledgeCandidate>>;

    /// Fetch a single candidate by id.
    ///
    /// Returns `Ok(None)` when the candidate is not present so callers can
    /// distinguish "absent" from "backend down" — KC-1b's materialization
    /// fast-path uses this instead of scanning `list_candidates` with
    /// `usize::MAX`. No default impl: breaking the trait is fine because
    /// the only concrete impl is `InMemoryActivityCaptureStore`, and
    /// KC-1c's Postgres impl will implement this directly.
    async fn get_candidate(
        &self,
        actor: &crate::knowledge::AuthenticatedContext,
        id: Uuid,
    ) -> CaptureResult<Option<KnowledgeCandidate>>;
}

/// Capture-plane coordinator — the public entry point that direct-action
/// surfaces, Penny, and external runtime callers all use.
///
/// Responsibilities:
///
/// 1. Atomically append an activity event + zero-to-many candidates.
/// 2. Enforce the per-request candidate cap (authority doc §2.3 rule
///    `max_candidates_per_request`).
/// 3. Materialize accepted candidates via `KnowledgeService::write` (KC-1b).
///
/// KC-1 wires only steps 1-2 and a stub for step 3 that returns the
/// `proposed_path` without touching a real store. KC-1b replaces the stub
/// with a real `KnowledgeService::write` call.
#[async_trait]
pub trait KnowledgeCandidateCoordinator: Send + Sync + std::fmt::Debug {
    /// Capture an activity event plus zero-to-many candidate hints. Returns
    /// the stored candidates (hints that exceeded the per-request cap are
    /// dropped at the boundary; the returned Vec reflects what was persisted).
    async fn capture_action(
        &self,
        actor: &crate::knowledge::AuthenticatedContext,
        event: CapturedActivityEvent,
        hints: Vec<CandidateHint>,
    ) -> CaptureResult<Vec<KnowledgeCandidate>>;

    /// Materialize an `Accepted` candidate to the canonical knowledge store.
    ///
    /// Returns the canonical path the candidate was written to. KC-1
    /// returns `candidate.proposed_path` (or a synthesized
    /// `"ops/journal/<uuid>"` fallback) without touching any KnowledgeStore;
    /// KC-1b replaces the body with a real `KnowledgeService::write` call.
    ///
    /// MUST return `GadgetronError::Knowledge { kind: InvalidQuery, ... }`
    /// when the candidate's latest disposition is not `Accepted`, so Penny
    /// gets a typed 400 when it tries to materialize a pending candidate.
    async fn materialize_accepted_candidate(
        &self,
        actor: &crate::knowledge::AuthenticatedContext,
        candidate_id: Uuid,
        write: KnowledgeDocumentWrite,
    ) -> CaptureResult<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_uuid(last: u8) -> Uuid {
        let mut bytes = [0u8; 16];
        bytes[15] = last;
        Uuid::from_bytes(bytes)
    }

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 18, 12, 0, 0).unwrap()
    }

    #[test]
    fn knowledge_candidate_disposition_round_trips_snake_case() {
        let cases = [
            (
                KnowledgeCandidateDisposition::PendingPennyDecision,
                "\"pending_penny_decision\"",
            ),
            (
                KnowledgeCandidateDisposition::PendingUserConfirmation,
                "\"pending_user_confirmation\"",
            ),
            (KnowledgeCandidateDisposition::Accepted, "\"accepted\""),
            (KnowledgeCandidateDisposition::Rejected, "\"rejected\""),
        ];
        for (variant, expected_wire) in cases {
            let serialized = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                serialized, expected_wire,
                "wire string for {variant:?} should be {expected_wire}"
            );
            let back: KnowledgeCandidateDisposition = serde_json::from_str(&serialized).unwrap();
            assert_eq!(back, variant, "round-trip failed for {variant:?}");
        }
    }

    #[test]
    fn candidate_decision_kind_round_trips_snake_case() {
        let cases = [
            (CandidateDecisionKind::Accept, "\"accept\""),
            (CandidateDecisionKind::Reject, "\"reject\""),
            (
                CandidateDecisionKind::EscalateToUser,
                "\"escalate_to_user\"",
            ),
        ];
        for (variant, expected_wire) in cases {
            let serialized = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                serialized, expected_wire,
                "wire string for {variant:?} should be {expected_wire}"
            );
            let back: CandidateDecisionKind = serde_json::from_str(&serialized).unwrap();
            assert_eq!(back, variant, "round-trip failed for {variant:?}");
        }
    }

    #[test]
    fn activity_origin_round_trips() {
        let cases = [
            (ActivityOrigin::UserDirect, "\"user_direct\""),
            (ActivityOrigin::Penny, "\"penny\""),
            (ActivityOrigin::System, "\"system\""),
        ];
        for (variant, expected_wire) in cases {
            let serialized = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                serialized, expected_wire,
                "wire for {variant:?} should be {expected_wire}"
            );
            let back: ActivityOrigin = serde_json::from_str(&serialized).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn activity_kind_round_trips() {
        let cases = [
            (ActivityKind::DirectAction, "\"direct_action\""),
            (ActivityKind::GadgetToolCall, "\"gadget_tool_call\""),
            (ActivityKind::ApprovalDecision, "\"approval_decision\""),
            (ActivityKind::RuntimeObservation, "\"runtime_observation\""),
            (ActivityKind::KnowledgeWriteback, "\"knowledge_writeback\""),
        ];
        for (variant, expected_wire) in cases {
            let serialized = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                serialized, expected_wire,
                "wire for {variant:?} should be {expected_wire}"
            );
            let back: ActivityKind = serde_json::from_str(&serialized).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn captured_activity_event_serializes_all_fields() {
        let event = CapturedActivityEvent {
            id: fixed_uuid(1),
            tenant_id: fixed_uuid(2),
            actor_user_id: fixed_uuid(3),
            request_id: Some(fixed_uuid(4)),
            origin: ActivityOrigin::UserDirect,
            kind: ActivityKind::DirectAction,
            title: "restart-service".to_string(),
            summary: "restarted ops/grafana via workbench".to_string(),
            source_bundle: Some("bundle.ops".to_string()),
            source_capability: Some("service.restart".to_string()),
            audit_event_id: Some(fixed_uuid(5)),
            facts: serde_json::json!({"service": "grafana"}),
            created_at: fixed_time(),
        };
        let value = serde_json::to_value(&event).unwrap();
        for key in [
            "id",
            "tenant_id",
            "actor_user_id",
            "request_id",
            "origin",
            "kind",
            "title",
            "summary",
            "source_bundle",
            "source_capability",
            "audit_event_id",
            "facts",
            "created_at",
        ] {
            assert!(
                value.get(key).is_some(),
                "CapturedActivityEvent must serialize field {key:?}"
            );
        }
        // Round-trip.
        let back: CapturedActivityEvent = serde_json::from_value(value).unwrap();
        assert_eq!(back.id, event.id);
        assert_eq!(back.origin, ActivityOrigin::UserDirect);
        assert_eq!(back.kind, ActivityKind::DirectAction);
    }

    #[test]
    fn knowledge_candidate_serializes_disposition() {
        let mut provenance = BTreeMap::new();
        provenance.insert("bundle".to_string(), "ops".to_string());
        let candidate = KnowledgeCandidate {
            id: fixed_uuid(7),
            activity_event_id: fixed_uuid(8),
            tenant_id: fixed_uuid(9),
            actor_user_id: fixed_uuid(10),
            summary: "note".to_string(),
            proposed_path: Some("ops/journal/test".to_string()),
            provenance,
            disposition: KnowledgeCandidateDisposition::PendingPennyDecision,
            created_at: fixed_time(),
        };
        let value = serde_json::to_value(&candidate).unwrap();
        assert_eq!(value["disposition"], "pending_penny_decision");
        assert_eq!(value["proposed_path"], "ops/journal/test");
        assert_eq!(value["provenance"]["bundle"], "ops");
        let back: KnowledgeCandidate = serde_json::from_value(value).unwrap();
        assert_eq!(back.disposition, candidate.disposition);
        assert_eq!(back.proposed_path, candidate.proposed_path);
    }
}
