//! Knowledge candidate lifecycle types.
//!
//! W3-KC-1 (doc `docs/design/core/knowledge-candidate-curation.md`) will
//! extend this module with `KnowledgeCandidate`, `CapturedActivityEvent`,
//! `CandidateHint`, `CandidateDecision`, and the coordinator traits. This
//! PR (W3-PSL-1) only lands the two enums so `gadgetron-core::agent::shared_context`
//! can reference them.

use serde::{Deserialize, Serialize};

/// Lifecycle disposition of a knowledge candidate.
///
/// Candidates move through these states as Penny and users make decisions.
/// `#[non_exhaustive]` allows KC-1 to add states (e.g. `EscalatedToUser`)
/// without breaking existing match arms in callers.
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
/// `#[non_exhaustive]` keeps the door open for KC-1 additions.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
