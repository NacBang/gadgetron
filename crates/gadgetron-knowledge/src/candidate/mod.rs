//! In-memory implementations of the Knowledge Candidate lifecycle contract.
//!
//! Authority: `docs/design/core/knowledge-candidate-curation.md` §2.2 and
//! D-20260418-17. This module is the KC-1 "in-memory slice" that replaces
//! the PSL-1 stubs in `gadgetron-gateway::penny::shared_context` with real
//! behavior. KC-1b migrates the storage to Postgres via testcontainers; KC-1c
//! adds canonical writeback + audit correlation.
//!
//! # What lives here
//!
//! - [`InMemoryActivityCaptureStore`] — `tokio::sync::Mutex`-guarded
//!   `Vec` / `BTreeMap` persistence that satisfies
//!   [`gadgetron_core::knowledge::candidate::ActivityCaptureStore`].
//! - [`InProcessCandidateCoordinator`] — a `KnowledgeCandidateCoordinator`
//!   that clamps hint counts to `max_candidates_per_request` and delegates
//!   append / decide / list calls to the store above.
//!
//! # Concurrency model
//!
//! We use `tokio::sync::Mutex` rather than `std::sync::Mutex` so the locks
//! can be held across `.await` boundaries without tripping Send issues when
//! the store is hosted inside a multi-threaded tokio runtime. The critical
//! sections are short (pure in-memory manipulation) so contention is never
//! the bottleneck; Postgres-backed storage in KC-1b replaces this with row-
//! level locking instead of a process-global mutex.
//!
//! # Error model
//!
//! Unknown `candidate_id` → `GadgetronError::Knowledge { kind: DocumentNotFound, ... }`.
//! "Materialize called on non-accepted candidate" → `GadgetronError::Knowledge
//! { kind: InvalidQuery, ... }`. Both reuse existing error kinds so callers
//! get typed 404 / 400 responses without a new enum variant.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::{
    error::{GadgetronError, KnowledgeErrorKind},
    knowledge::{
        candidate::{
            ActivityCaptureStore, CandidateDecision, CandidateDecisionKind, CandidateHint,
            CaptureResult, CapturedActivityEvent, KnowledgeCandidate,
            KnowledgeCandidateCoordinator, KnowledgeCandidateDisposition, KnowledgeDocumentWrite,
        },
        AuthenticatedContext,
    },
};
use tokio::sync::Mutex;
use uuid::Uuid;

/// In-memory append-only store for activity events, candidates, and decisions.
///
/// Suitable for unit tests, the KC-1 gateway slice, and the fixture-diff
/// contract test. Not persistent — restart drops all rows.
#[derive(Debug, Default)]
pub struct InMemoryActivityCaptureStore {
    events: Mutex<Vec<CapturedActivityEvent>>,
    candidates: Mutex<BTreeMap<Uuid, KnowledgeCandidate>>,
    decisions: Mutex<Vec<CandidateDecision>>,
}

impl InMemoryActivityCaptureStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test helper: snapshot the number of recorded events.
    #[doc(hidden)]
    pub async fn event_count(&self) -> usize {
        self.events.lock().await.len()
    }

    /// Test helper: snapshot the number of recorded decisions.
    #[doc(hidden)]
    pub async fn decision_count(&self) -> usize {
        self.decisions.lock().await.len()
    }
}

#[async_trait]
impl ActivityCaptureStore for InMemoryActivityCaptureStore {
    async fn append_activity(
        &self,
        _actor: &AuthenticatedContext,
        event: CapturedActivityEvent,
    ) -> CaptureResult<()> {
        let mut events = self.events.lock().await;
        events.push(event);
        Ok(())
    }

    async fn append_candidate(
        &self,
        _actor: &AuthenticatedContext,
        activity_event_id: Uuid,
        hint: CandidateHint,
    ) -> CaptureResult<KnowledgeCandidate> {
        // Tenant / actor are re-injected from the event context at KC-1b;
        // for KC-1 we look up the event and mirror its identity fields so
        // the candidate still shows a consistent tenant/actor pair.
        let (tenant_id, actor_user_id) = {
            let events = self.events.lock().await;
            let found = events.iter().find(|e| e.id == activity_event_id);
            match found {
                Some(e) => (e.tenant_id, e.actor_user_id),
                None => {
                    return Err(GadgetronError::Knowledge {
                        kind: KnowledgeErrorKind::DocumentNotFound {
                            path: format!("activity_event/{activity_event_id}"),
                        },
                        message: format!(
                            "cannot append candidate: activity event {activity_event_id} \
                             not found in capture store"
                        ),
                    });
                }
            }
        };

        let mut provenance: BTreeMap<String, String> = BTreeMap::new();
        if let Some(reason) = hint.reason.as_deref() {
            provenance.insert("hint_reason".to_string(), reason.to_string());
        }
        if !hint.tags.is_empty() {
            // BTreeMap serialization is deterministic, so joining tags with
            // a comma keeps the provenance bytes stable for fixture-diff.
            provenance.insert("hint_tags".to_string(), hint.tags.join(","));
        }

        let candidate = KnowledgeCandidate {
            id: Uuid::new_v4(),
            activity_event_id,
            tenant_id,
            actor_user_id,
            summary: hint.summary,
            proposed_path: hint.proposed_path,
            provenance,
            disposition: KnowledgeCandidateDisposition::PendingPennyDecision,
            created_at: chrono::Utc::now(),
        };

        let mut candidates = self.candidates.lock().await;
        candidates.insert(candidate.id, candidate.clone());
        Ok(candidate)
    }

    async fn decide_candidate(
        &self,
        _actor: &AuthenticatedContext,
        decision: CandidateDecision,
    ) -> CaptureResult<KnowledgeCandidate> {
        let mut candidates = self.candidates.lock().await;
        let candidate = candidates.get_mut(&decision.candidate_id).ok_or_else(|| {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::DocumentNotFound {
                    path: format!("candidate/{}", decision.candidate_id),
                },
                message: format!(
                    "cannot decide candidate {candidate_id}: not found in capture store",
                    candidate_id = decision.candidate_id
                ),
            }
        })?;

        let next_disposition = match decision.decision {
            CandidateDecisionKind::Accept => KnowledgeCandidateDisposition::Accepted,
            CandidateDecisionKind::Reject => KnowledgeCandidateDisposition::Rejected,
            CandidateDecisionKind::EscalateToUser => {
                KnowledgeCandidateDisposition::PendingUserConfirmation
            }
            // Any future `CandidateDecisionKind` variant falls through as
            // "leave disposition alone" until an explicit arm is added;
            // `#[non_exhaustive]` requires the wildcard.
            _ => {
                return Err(GadgetronError::Knowledge {
                    kind: KnowledgeErrorKind::InvalidQuery {
                        reason: format!(
                            "unsupported decision kind {:?}; KC-1 supports Accept / Reject / EscalateToUser only",
                            decision.decision
                        ),
                    },
                    message: "unknown candidate decision kind".to_string(),
                });
            }
        };

        candidate.disposition = next_disposition;
        let snapshot = candidate.clone();

        // Release the candidate lock before touching decisions to avoid
        // accidentally holding both at once; tokio::Mutex only allows one
        // holder per task anyway, but the two-lock pattern matches what a
        // real KC-1b DB transaction would do.
        drop(candidates);

        let mut decisions = self.decisions.lock().await;
        decisions.push(decision);

        Ok(snapshot)
    }

    async fn list_candidates(
        &self,
        _actor: &AuthenticatedContext,
        limit: usize,
        only_pending: bool,
    ) -> CaptureResult<Vec<KnowledgeCandidate>> {
        let candidates = self.candidates.lock().await;
        let mut rows: Vec<KnowledgeCandidate> = candidates
            .values()
            .filter(|c| {
                !only_pending
                    || matches!(
                        c.disposition,
                        KnowledgeCandidateDisposition::PendingPennyDecision
                            | KnowledgeCandidateDisposition::PendingUserConfirmation
                    )
            })
            .cloned()
            .collect();
        // Newest-first; stable tie-break on id so the fixture-diff test gets
        // a deterministic ordering when two candidates share a timestamp.
        rows.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        rows.truncate(limit);
        Ok(rows)
    }
}

/// In-process coordinator — appends activity + candidates, clamps to
/// `max_candidates_per_request`, and stubs materialization until KC-1b.
#[derive(Debug)]
pub struct InProcessCandidateCoordinator {
    pub store: Arc<dyn ActivityCaptureStore>,
    pub max_candidates_per_request: usize,
}

impl InProcessCandidateCoordinator {
    pub fn new(store: Arc<dyn ActivityCaptureStore>, max_candidates_per_request: usize) -> Self {
        Self {
            store,
            max_candidates_per_request,
        }
    }
}

#[async_trait]
impl KnowledgeCandidateCoordinator for InProcessCandidateCoordinator {
    async fn capture_action(
        &self,
        actor: &AuthenticatedContext,
        event: CapturedActivityEvent,
        hints: Vec<CandidateHint>,
    ) -> CaptureResult<Vec<KnowledgeCandidate>> {
        let event_id = event.id;
        self.store.append_activity(actor, event).await?;

        let clamp = self.max_candidates_per_request.min(hints.len());
        let mut created = Vec::with_capacity(clamp);
        for hint in hints.into_iter().take(clamp) {
            let candidate = self.store.append_candidate(actor, event_id, hint).await?;
            created.push(candidate);
        }
        Ok(created)
    }

    async fn materialize_accepted_candidate(
        &self,
        actor: &AuthenticatedContext,
        candidate_id: Uuid,
        _write: KnowledgeDocumentWrite,
    ) -> CaptureResult<String> {
        // KC-1: find the candidate via the read projection (limit high
        // enough to match any row; only_pending=false so Accepted rows
        // surface too). Replace with a typed `get_candidate` helper at
        // KC-1b when we have a real store index by id.
        let list = self
            .store
            .list_candidates(actor, usize::MAX, /*only_pending=*/ false)
            .await?;
        let candidate = list
            .into_iter()
            .find(|c| c.id == candidate_id)
            .ok_or_else(|| GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::DocumentNotFound {
                    path: format!("candidate/{candidate_id}"),
                },
                message: format!(
                    "cannot materialize candidate {candidate_id}: not found in capture store"
                ),
            })?;

        if candidate.disposition != KnowledgeCandidateDisposition::Accepted {
            return Err(GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery {
                    reason: format!(
                        "candidate disposition must be accepted to materialize; got {:?}",
                        candidate.disposition
                    ),
                },
                message: "cannot materialize a non-accepted candidate".to_string(),
            });
        }

        // KC-1 stub: return the proposed_path, or a synthesized journal path.
        // KC-1b will replace this body with a real `KnowledgeService::write`
        // call and return the canonical `KnowledgePath`.
        let resolved_path = candidate
            .proposed_path
            .unwrap_or_else(|| format!("ops/journal/{candidate_id}"));
        Ok(resolved_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gadgetron_core::knowledge::candidate::{ActivityKind, ActivityOrigin};

    fn actor() -> AuthenticatedContext {
        AuthenticatedContext
    }

    fn make_event(id_seed: u8) -> CapturedActivityEvent {
        let mut bytes = [0u8; 16];
        bytes[15] = id_seed;
        let id = Uuid::from_bytes(bytes);
        bytes[15] = 200;
        let tenant_id = Uuid::from_bytes(bytes);
        bytes[15] = 201;
        let actor_user_id = Uuid::from_bytes(bytes);
        CapturedActivityEvent {
            id,
            tenant_id,
            actor_user_id,
            request_id: None,
            origin: ActivityOrigin::UserDirect,
            kind: ActivityKind::DirectAction,
            title: format!("test-title-{id_seed}"),
            summary: format!("test-summary-{id_seed}"),
            source_bundle: None,
            source_capability: None,
            audit_event_id: None,
            facts: serde_json::json!({}),
            created_at: Utc
                .with_ymd_and_hms(2026, 4, 18, 12, 0, id_seed as u32)
                .unwrap(),
        }
    }

    fn make_hint(n: u32) -> CandidateHint {
        CandidateHint {
            summary: format!("hint summary {n}"),
            proposed_path: Some(format!("ops/journal/hint-{n}")),
            tags: vec!["ops".to_string()],
            reason: Some("direct_action".to_string()),
        }
    }

    // ------------------------------------------------------------------
    // InMemoryActivityCaptureStore tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn inmem_store_append_then_list() {
        let store = InMemoryActivityCaptureStore::new();
        let event = make_event(1);
        let event_id = event.id;
        store.append_activity(&actor(), event).await.unwrap();
        let _c = store
            .append_candidate(&actor(), event_id, make_hint(1))
            .await
            .unwrap();
        let list = store.list_candidates(&actor(), 10, false).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].disposition,
            KnowledgeCandidateDisposition::PendingPennyDecision
        );
        assert_eq!(list[0].activity_event_id, event_id);
        assert_eq!(store.event_count().await, 1);
    }

    #[tokio::test]
    async fn inmem_store_decide_updates_disposition_accept() {
        let store = InMemoryActivityCaptureStore::new();
        let event = make_event(2);
        let event_id = event.id;
        store.append_activity(&actor(), event).await.unwrap();
        let c = store
            .append_candidate(&actor(), event_id, make_hint(2))
            .await
            .unwrap();

        let decision = CandidateDecision {
            candidate_id: c.id,
            decision: CandidateDecisionKind::Accept,
            decided_by_user_id: None,
            decided_by_penny: true,
            rationale: Some("ops-journal worthy".to_string()),
        };
        let updated = store.decide_candidate(&actor(), decision).await.unwrap();
        assert_eq!(updated.disposition, KnowledgeCandidateDisposition::Accepted);
        assert_eq!(store.decision_count().await, 1);
    }

    #[tokio::test]
    async fn inmem_store_decide_updates_disposition_reject() {
        let store = InMemoryActivityCaptureStore::new();
        let event = make_event(3);
        let event_id = event.id;
        store.append_activity(&actor(), event).await.unwrap();
        let c = store
            .append_candidate(&actor(), event_id, make_hint(3))
            .await
            .unwrap();
        let updated = store
            .decide_candidate(
                &actor(),
                CandidateDecision {
                    candidate_id: c.id,
                    decision: CandidateDecisionKind::Reject,
                    decided_by_user_id: None,
                    decided_by_penny: true,
                    rationale: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.disposition, KnowledgeCandidateDisposition::Rejected);
    }

    #[tokio::test]
    async fn inmem_store_decide_updates_disposition_escalate() {
        let store = InMemoryActivityCaptureStore::new();
        let event = make_event(4);
        let event_id = event.id;
        store.append_activity(&actor(), event).await.unwrap();
        let c = store
            .append_candidate(&actor(), event_id, make_hint(4))
            .await
            .unwrap();
        let updated = store
            .decide_candidate(
                &actor(),
                CandidateDecision {
                    candidate_id: c.id,
                    decision: CandidateDecisionKind::EscalateToUser,
                    decided_by_user_id: None,
                    decided_by_penny: true,
                    rationale: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            updated.disposition,
            KnowledgeCandidateDisposition::PendingUserConfirmation
        );
    }

    #[tokio::test]
    async fn inmem_store_list_only_pending_filters() {
        let store = InMemoryActivityCaptureStore::new();
        let event = make_event(5);
        let event_id = event.id;
        store.append_activity(&actor(), event).await.unwrap();
        let a = store
            .append_candidate(&actor(), event_id, make_hint(1))
            .await
            .unwrap();
        let _b = store
            .append_candidate(&actor(), event_id, make_hint(2))
            .await
            .unwrap();
        // Accept the first → it should drop out of pending listings.
        store
            .decide_candidate(
                &actor(),
                CandidateDecision {
                    candidate_id: a.id,
                    decision: CandidateDecisionKind::Accept,
                    decided_by_user_id: None,
                    decided_by_penny: true,
                    rationale: None,
                },
            )
            .await
            .unwrap();
        let pending = store.list_candidates(&actor(), 10, true).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_ne!(pending[0].id, a.id, "accepted candidate must be filtered");
        let all = store.list_candidates(&actor(), 10, false).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn inmem_store_decide_unknown_candidate_errors() {
        let store = InMemoryActivityCaptureStore::new();
        let err = store
            .decide_candidate(
                &actor(),
                CandidateDecision {
                    candidate_id: Uuid::new_v4(),
                    decision: CandidateDecisionKind::Accept,
                    decided_by_user_id: None,
                    decided_by_penny: true,
                    rationale: None,
                },
            )
            .await
            .unwrap_err();
        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::DocumentNotFound { path },
                ..
            } => {
                assert!(path.starts_with("candidate/"), "path was: {path}");
            }
            other => panic!("expected DocumentNotFound, got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // InProcessCandidateCoordinator tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn coordinator_capture_action_clamps_to_max_candidates() {
        let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
        let coord = InProcessCandidateCoordinator::new(store.clone(), /*max=*/ 2);
        let event = make_event(6);
        let hints = vec![make_hint(1), make_hint(2), make_hint(3), make_hint(4)];
        let created = coord.capture_action(&actor(), event, hints).await.unwrap();
        assert_eq!(
            created.len(),
            2,
            "coordinator must clamp to max_candidates_per_request"
        );
    }

    #[tokio::test]
    async fn coordinator_materialize_errors_when_not_accepted() {
        let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
        let coord = InProcessCandidateCoordinator::new(store.clone(), 8);
        let event = make_event(7);
        let hints = vec![make_hint(1)];
        let created = coord.capture_action(&actor(), event, hints).await.unwrap();
        let candidate_id = created[0].id;

        // Default disposition is PendingPennyDecision → materialize must fail.
        let write = KnowledgeDocumentWrite {
            path: "ops/journal/x".to_string(),
            content: "body".to_string(),
            provenance: BTreeMap::new(),
        };
        let err = coord
            .materialize_accepted_candidate(&actor(), candidate_id, write)
            .await
            .unwrap_err();
        match err {
            GadgetronError::Knowledge {
                kind: KnowledgeErrorKind::InvalidQuery { reason },
                ..
            } => {
                assert!(
                    reason.contains("accepted"),
                    "reason must mention disposition; was: {reason}"
                );
            }
            other => panic!("expected InvalidQuery, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn coordinator_materialize_returns_proposed_path_when_accepted() {
        let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
        let coord = InProcessCandidateCoordinator::new(store.clone(), 8);
        let event = make_event(8);
        let hints = vec![CandidateHint {
            summary: "op".into(),
            proposed_path: Some("ops/journal/accepted".into()),
            tags: vec![],
            reason: None,
        }];
        let created = coord.capture_action(&actor(), event, hints).await.unwrap();
        let candidate_id = created[0].id;

        // Accept the candidate via the store (this is what Penny would trigger
        // through `decide_candidate`).
        store
            .decide_candidate(
                &actor(),
                CandidateDecision {
                    candidate_id,
                    decision: CandidateDecisionKind::Accept,
                    decided_by_user_id: None,
                    decided_by_penny: true,
                    rationale: None,
                },
            )
            .await
            .unwrap();

        let write = KnowledgeDocumentWrite {
            path: "ops/journal/accepted".to_string(),
            content: "# body".to_string(),
            provenance: BTreeMap::new(),
        };
        let path = coord
            .materialize_accepted_candidate(&actor(), candidate_id, write)
            .await
            .unwrap();
        assert_eq!(path, "ops/journal/accepted");
    }
}
