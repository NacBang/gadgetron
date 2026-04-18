//! Penny shared surface awareness — per-turn bootstrap types and assembler trait.
//!
//! Authority: `docs/design/phase2/13-penny-shared-surface-loop.md` §2.1.1.
//!
//! # What lives here
//!
//! - [`PennySharedContextHealth`] — health signal carried in every bootstrap.
//! - [`PennyTurnBootstrap`] — the full per-turn digest assembled before Penny
//!   sees any user input.
//! - [`PennyActivityDigest`], [`PennyCandidateDigest`], [`PennyApprovalDigest`]
//!   — per-item summaries that keep raw payloads (paths, secrets, stack traces)
//!   out of the prompt.
//! - [`PennyCandidateDecisionRequest`] / [`PennyCandidateDecisionReceipt`] —
//!   Penny's typed decision path for knowledge candidates.
//! - [`PennyTurnContextAssembler`] — the trait gateway implements to produce a
//!   `PennyTurnBootstrap` for each incoming request.
//!
//! # What does NOT live here
//!
//! - Bootstrap assembly implementation — that lives in
//!   `gadgetron-gateway::penny::shared_context`.
//! - Prompt rendering — `render_penny_shared_context` also lives in the
//!   gateway crate so it can consume the assembled bootstrap.
//! - Gadget provider — `gadgetron-penny::workbench_awareness`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::GadgetronError,
    knowledge::candidate::{CandidateDecisionKind, KnowledgeCandidateDisposition},
    knowledge::AuthenticatedContext,
};

/// Overall health signal for a `PennyTurnBootstrap`.
///
/// `Degraded` does NOT prevent Penny from responding; it causes the bootstrap
/// to carry `degraded_reasons` explaining which components are unavailable.
/// Per doc §2.3: `require_explicit_degraded_notice = true` is non-negotiable.
///
/// `#[non_exhaustive]` is intentionally absent here — the binary `Healthy /
/// Degraded` distinction is load-bearing in rendering rules and must remain
/// exhaustive so callers can write `match` without a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PennySharedContextHealth {
    /// All three shared-surface reads (activity, candidates, approvals)
    /// succeeded.
    Healthy,
    /// At least one read failed; `degraded_reasons` carries the details.
    Degraded,
}

/// Per-turn bootstrap digest injected into Penny's context before user input.
///
/// The bootstrap is assembled fresh on every request — session resume does not
/// skip this step (doc §2.2.1 rule 1).
///
/// Fields map directly to the `<gadgetron_shared_context>` block format
/// described in doc §2.2.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyTurnBootstrap {
    /// The gateway's request UUID for this Penny turn (set by
    /// `request_id_middleware`). Used for correlation in traces and
    /// `penny_turn_bootstrap_build` span.
    pub request_id: Uuid,
    /// Conversation identifier, if the client supplied one. `None` for
    /// stateless / first-turn requests.
    pub conversation_id: Option<String>,
    /// Wall-clock when the bootstrap was assembled. Rendered as RFC 3339 in
    /// the prompt block.
    pub generated_at: DateTime<Utc>,
    /// `Healthy` when all three reads succeeded; `Degraded` otherwise.
    pub health: PennySharedContextHealth,
    /// Plain-language descriptions of each partial failure (one entry per
    /// failed component). Empty when `health == Healthy`.
    pub degraded_reasons: Vec<String>,
    /// Newest-first activity entries, capped by `bootstrap_activity_limit`.
    pub recent_activity: Vec<PennyActivityDigest>,
    /// Unresolved candidates (pending Penny or user decision), capped by
    /// `bootstrap_candidate_limit`.
    pub pending_candidates: Vec<PennyCandidateDigest>,
    /// Pending operator-visible approvals, capped by `bootstrap_approval_limit`.
    pub pending_approvals: Vec<PennyApprovalDigest>,
}

/// Summary of a single shared-surface activity event.
///
/// Raw event rows, internal paths, and tool argument dumps MUST NOT appear
/// here — only human-readable summaries (doc §2.4.1 STRIDE row 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyActivityDigest {
    /// Activity event identifier. Used by `workbench.request_evidence` to
    /// drill down.
    pub activity_event_id: Uuid,
    /// The gateway request that triggered this event, if any.
    pub request_id: Option<Uuid>,
    /// Where the event originated (`"penny"`, `"user_direct"`, `"system"`).
    /// Stringified from `WorkbenchActivityOrigin` via `Debug` or `to_string`.
    pub origin: String,
    /// What kind of event this is (`"chat_turn"`, `"direct_action"`,
    /// `"system_event"`).
    pub kind: String,
    /// Short human-readable title (clipped to `digest_summary_chars` in prompt
    /// rendering). This is the primary display text.
    pub title: String,
    /// Optional longer description (also clipped in rendering).
    pub summary: String,
    /// Source bundle identifier, if the event originates from a bundle action.
    pub source_bundle: Option<String>,
    /// When the event was recorded.
    pub created_at: DateTime<Utc>,
}

/// Summary of a single unresolved knowledge candidate.
///
/// `materialization_status` carries `None` for candidates that have not been
/// accepted yet, and a status string (e.g. `"failed"`, `"pending"`) for
/// accepted-but-not-yet-written candidates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyCandidateDigest {
    /// Candidate identifier used by `workbench.candidate_decide`.
    pub candidate_id: Uuid,
    /// Activity event that produced this candidate.
    pub activity_event_id: Uuid,
    /// Human-readable summary of what the candidate proposes to write.
    pub summary: String,
    /// Current lifecycle state.
    pub disposition: KnowledgeCandidateDisposition,
    /// Intended knowledge-store path, if already determined.
    pub proposed_path: Option<String>,
    /// Whether a user confirmation step is required before writeback.
    pub requires_user_confirmation: bool,
    /// Materialization outcome for accepted candidates. `None` if not yet
    /// attempted; `Some("pending")` / `Some("success")` / `Some("failed")`
    /// for in-flight or completed writebacks.
    pub materialization_status: Option<String>,
}

/// Summary of a pending operator approval request.
///
/// Raw approver rationale or full request payload MUST NOT appear here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyApprovalDigest {
    /// Approval request identifier.
    pub approval_id: Uuid,
    /// Short human-readable title of what requires approval.
    pub title: String,
    /// Risk classification tier (e.g. `"destructive"`, `"elevated"`, `"normal"`).
    pub risk_tier: String,
    /// When the approval request was created.
    pub requested_at: DateTime<Utc>,
}

/// Penny's request to record a disposition decision for a knowledge candidate.
///
/// Sent through `workbench.candidate_decide`; routed to
/// `KnowledgeCandidateCoordinator::decide_candidate` (KC-1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyCandidateDecisionRequest {
    /// The candidate being decided.
    pub candidate_id: Uuid,
    /// The decision kind.
    pub decision: CandidateDecisionKind,
    /// Optional plain-language rationale logged in the audit trail.
    pub rationale: Option<String>,
}

/// Receipt returned after a disposition decision is recorded.
///
/// `materialization_status` reflects the outcome of the writeback attempt
/// triggered by `Accept`. Per doc §2.1.3: `materialization_status = "failed"`
/// does NOT cause `disposition` to revert — these are independent states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennyCandidateDecisionReceipt {
    /// The candidate that was decided.
    pub candidate_id: Uuid,
    /// Updated disposition after the decision was recorded.
    pub disposition: KnowledgeCandidateDisposition,
    /// Canonical store path where the candidate was written, if any.
    pub canonical_path: Option<String>,
    /// Writeback outcome for `Accepted` decisions.
    pub materialization_status: Option<String>,
    /// Activity event associated with the decision for audit correlation.
    pub activity_event_id: Uuid,
}

/// Trait that gateway implements to assemble a `PennyTurnBootstrap` before
/// each Penny request.
///
/// # Contract
///
/// - Called once per Penny request, after `AuthenticatedContext` is resolved.
/// - `conversation_id` is `None` for stateless / first-turn requests.
/// - Partial shared-surface failures MUST NOT cause this method to return
///   `Err`; they MUST surface as `health = Degraded` + `degraded_reasons`.
/// - The only case that returns `Err` is when `AuthenticatedContext` itself
///   cannot be resolved upstream of this call (identity failure) — the caller
///   is responsible for that check.
///
/// # Thread-safety
///
/// Implementors MUST be `Send + Sync` so they can be held in `Arc<dyn
/// PennyTurnContextAssembler>` behind `AppState`.
#[async_trait]
pub trait PennyTurnContextAssembler: Send + Sync {
    async fn build(
        &self,
        actor: &AuthenticatedContext,
        conversation_id: Option<&str>,
        request_id: Uuid,
    ) -> Result<PennyTurnBootstrap, GadgetronError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Arc;

    fn fixed_uuid() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 18, 12, 0, 0).unwrap()
    }

    /// `PennyTurnBootstrap` must serialize with the expected field names so the
    /// prompt renderer can rely on the wire shape.
    #[test]
    fn penny_turn_bootstrap_serializes_with_expected_field_names() {
        let bootstrap = PennyTurnBootstrap {
            request_id: fixed_uuid(),
            conversation_id: Some("conv-abc".to_string()),
            generated_at: fixed_time(),
            health: PennySharedContextHealth::Healthy,
            degraded_reasons: vec![],
            recent_activity: vec![],
            pending_candidates: vec![],
            pending_approvals: vec![],
        };

        let value = serde_json::to_value(&bootstrap).unwrap();

        // Check required top-level keys are present and named correctly.
        assert!(value.get("request_id").is_some(), "missing request_id");
        assert!(
            value.get("conversation_id").is_some(),
            "missing conversation_id"
        );
        assert!(value.get("generated_at").is_some(), "missing generated_at");
        assert!(value.get("health").is_some(), "missing health");
        assert!(
            value.get("degraded_reasons").is_some(),
            "missing degraded_reasons"
        );
        assert!(
            value.get("recent_activity").is_some(),
            "missing recent_activity"
        );
        assert!(
            value.get("pending_candidates").is_some(),
            "missing pending_candidates"
        );
        assert!(
            value.get("pending_approvals").is_some(),
            "missing pending_approvals"
        );

        assert_eq!(value["health"], "healthy");
        assert_eq!(value["request_id"], "00000000-0000-0000-0000-000000000001");
    }

    /// `PennySharedContextHealth` must serialize as snake_case wire strings.
    #[test]
    fn health_enum_wire_strings_are_snake_case() {
        assert_eq!(
            serde_json::to_string(&PennySharedContextHealth::Healthy).unwrap(),
            "\"healthy\""
        );
        assert_eq!(
            serde_json::to_string(&PennySharedContextHealth::Degraded).unwrap(),
            "\"degraded\""
        );
    }

    /// Assembler trait object-safety witness: `Arc<dyn PennyTurnContextAssembler>`
    /// must compile, proving the trait is object-safe.
    #[test]
    fn assembler_is_object_safe() {
        // If this function signature compiles, the trait is object-safe.
        fn _accepts_boxed(_: Arc<dyn PennyTurnContextAssembler>) {}
        // Just verifying it compiles — no runtime assertion needed.
    }
}
