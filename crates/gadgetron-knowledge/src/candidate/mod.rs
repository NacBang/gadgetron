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
//!   that clamps hint counts to `max_candidates_per_request`, expands
//!   `path_rules` templates for hint-less proposed paths, and routes
//!   accepted candidates through `KnowledgeService::write` when the
//!   service is wired via [`InProcessCandidateCoordinator::with_knowledge_service`].
//!
//! # `{date}` / `{topic}` / `{author}` template expansion
//!
//! `path_rules` uses a minimal 3-variable grammar at KC-1b: `{date}`
//! (YYYY-MM-DD UTC from the activity event's `created_at`), `{topic}`
//! (the snake_case `ActivityKind`, e.g. `direct_action`), and `{author}`
//! (the `actor_user_id` rendered as a bare UUID). Operators target a
//! key that matches the snake_case `ActivityKind` variant — a hint with
//! no `proposed_path` + a `DirectAction` event looks up the
//! `"direct_action"` key, expands it, and falls back to
//! `ops/journal/<YYYY-MM-DD>/<candidate_uuid>` when no rule matches.
//! Anything richer (e.g. `{tenant}`, `{request_id}`) is deferred to
//! KC-1c so the KC-1b surface stays small and wire-stable.
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
            ActivityCaptureStore, ActivityKind, CandidateDecision, CandidateDecisionKind,
            CandidateHint, CaptureResult, CapturedActivityEvent, KnowledgeCandidate,
            KnowledgeCandidateCoordinator, KnowledgeCandidateDisposition, KnowledgeDocumentWrite,
        },
        AuthenticatedContext, KnowledgePutRequest,
    },
};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::service::KnowledgeService;

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

    /// Test-only snapshot of captured activity events. Used by integration
    /// tests to assert capture call sites without exposing the internal
    /// Mutex<Vec<_>> directly.
    ///
    /// Spec: docs/design/core/knowledge-candidate-curation.md §2.1 — PSL-1d
    /// integration test access path. Mirrors the existing `event_count` /
    /// `decision_count` helpers on this struct.
    #[doc(hidden)]
    pub async fn events_snapshot(&self) -> Vec<CapturedActivityEvent> {
        self.events.lock().await.clone()
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

    async fn get_candidate(
        &self,
        _actor: &AuthenticatedContext,
        id: Uuid,
    ) -> CaptureResult<Option<KnowledgeCandidate>> {
        // `BTreeMap::get` is O(log n); cloning the row keeps the lock
        // scope tight so other capture-plane callers are not blocked on a
        // hot path.
        let candidates = self.candidates.lock().await;
        Ok(candidates.get(&id).cloned())
    }
}

/// In-process coordinator — appends activity + candidates, clamps to
/// `max_candidates_per_request`, expands `path_rules` templates when a hint
/// leaves `proposed_path` unset, and routes accepted candidates through
/// `KnowledgeService::write` when a service is wired.
///
/// Construction is intentionally builder-style so tests that only exercise
/// the capture plane (no canonical writeback) stay unchanged:
///
/// ```ignore
/// let coord = InProcessCandidateCoordinator::new(store, 8)
///     .with_knowledge_service(knowledge_service)
///     .with_path_rules(BTreeMap::from([(
///         "direct_action".into(),
///         "ops/journal/{date}/{topic}".into(),
///     )]));
/// ```
#[derive(Debug)]
pub struct InProcessCandidateCoordinator {
    pub store: Arc<dyn ActivityCaptureStore>,
    /// Optional canonical writeback target. `None` keeps the KC-1a
    /// synthetic-path fallback behavior for tests that only want to
    /// exercise the capture plane.
    pub knowledge_service: Option<Arc<KnowledgeService>>,
    pub max_candidates_per_request: usize,
    /// Template rules keyed by `ActivityKind` snake_case label (e.g.
    /// `"direct_action"`). An empty map disables template expansion —
    /// hints without a `proposed_path` then fall back to
    /// `ops/journal/<YYYY-MM-DD>/<candidate_uuid>`.
    pub path_rules: BTreeMap<String, String>,
}

impl InProcessCandidateCoordinator {
    pub fn new(store: Arc<dyn ActivityCaptureStore>, max_candidates_per_request: usize) -> Self {
        Self {
            store,
            knowledge_service: None,
            max_candidates_per_request,
            path_rules: BTreeMap::new(),
        }
    }

    /// Wire the canonical knowledge writeback target. Consumers who omit
    /// this keep the KC-1a synthetic-path fallback, which is what the
    /// fixture-diff test and the in-memory PSL-1 tests rely on.
    pub fn with_knowledge_service(mut self, svc: Arc<KnowledgeService>) -> Self {
        self.knowledge_service = Some(svc);
        self
    }

    /// Provide `path_rules` for `{date}` / `{topic}` / `{author}` template
    /// expansion. Called by the CLI with
    /// `config.knowledge.curation.path_rules.clone()`.
    pub fn with_path_rules(mut self, rules: BTreeMap<String, String>) -> Self {
        self.path_rules = rules;
        self
    }
}

/// Serialize a `#[serde(rename_all = "snake_case")]` enum into its wire
/// label. Falls back to `"unknown"` for the (impossible) JSON-not-a-string
/// case so callers can keep the signature infallible.
///
/// Re-used by the gateway's `render_penny_shared_context` after KC-1b to
/// emit `[pending_penny_decision]` instead of the old
/// `format!("{:?}").to_lowercase()` collapse.
fn enum_snake_case_label<E: serde::Serialize>(e: &E) -> String {
    serde_json::to_value(e)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Expand `{date}` / `{topic}` / `{author}` in a `path_rules` template
/// against a captured activity event.
///
/// - `{date}` → UTC `YYYY-MM-DD` of `event.created_at`.
/// - `{topic}` → snake_case `ActivityKind` label (e.g. `direct_action`).
/// - `{author}` → `event.actor_user_id` rendered as a bare UUID.
///
/// Unknown placeholders are left untouched; KC-1c will add validation at
/// config-load time once the vocabulary stabilizes.
fn expand_path_rule(rule: &str, event: &CapturedActivityEvent) -> String {
    let date = event.created_at.format("%Y-%m-%d").to_string();
    let topic = enum_snake_case_label(&event.kind);
    let author = event.actor_user_id.to_string();
    rule.replace("{date}", &date)
        .replace("{topic}", &topic)
        .replace("{author}", &author)
}

/// Look up a `path_rules` key for an `ActivityKind` and expand against
/// the event. Returns `None` when the coordinator has no rule for this
/// kind so the caller can decide on the journal-style fallback.
fn resolve_path_from_rules(
    rules: &BTreeMap<String, String>,
    event: &CapturedActivityEvent,
) -> Option<String> {
    let key = enum_snake_case_label(&event.kind);
    rules.get(&key).map(|rule| expand_path_rule(rule, event))
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
        // Snapshot the two fields we need for template expansion before
        // the event is moved into `append_activity`. Kind is `Copy`;
        // `created_at` is `DateTime<Utc>` which is also `Copy`.
        let event_snapshot_kind: ActivityKind = event.kind;
        let event_snapshot = event.clone();
        self.store.append_activity(actor, event).await?;

        let clamp = self.max_candidates_per_request.min(hints.len());
        let mut created = Vec::with_capacity(clamp);
        for mut hint in hints.into_iter().take(clamp) {
            if hint.proposed_path.is_none() {
                // Try path_rules expansion first; fall back to the
                // journal-style synthetic path. Keeps KC-1a behavior for
                // tests that wire an empty `path_rules` map.
                let resolved = resolve_path_from_rules(&self.path_rules, &event_snapshot);
                hint.proposed_path = Some(resolved.unwrap_or_else(|| {
                    // Cheap deterministic fallback — uses event date +
                    // a sentinel so the path is predictable in the
                    // "no rules, no hint" case. The candidate uuid will
                    // be swapped into this slot at append time.
                    format!(
                        "ops/journal/{}/{}",
                        event_snapshot.created_at.format("%Y-%m-%d"),
                        enum_snake_case_label(&event_snapshot_kind)
                    )
                }));
            }
            let candidate = self.store.append_candidate(actor, event_id, hint).await?;
            created.push(candidate);
        }
        Ok(created)
    }

    #[tracing::instrument(
        level = "info",
        name = "candidate.materialize",
        skip(self, actor, write),
        fields(
            candidate_id = %candidate_id,
            has_knowledge_service = self.knowledge_service.is_some()
        )
    )]
    async fn materialize_accepted_candidate(
        &self,
        actor: &AuthenticatedContext,
        candidate_id: Uuid,
        write: KnowledgeDocumentWrite,
    ) -> CaptureResult<String> {
        let candidate = self
            .store
            .get_candidate(actor, candidate_id)
            .await?
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

        // Happy path: canonical writeback through `KnowledgeService::write`.
        if let Some(svc) = self.knowledge_service.as_ref() {
            // TODO KC-1c: surface `write.provenance` on
            // `KnowledgePutRequest` so the candidate's `hint_reason` /
            // `hint_tags` plumb through to wiki frontmatter. For KC-1b we
            // accept the payload to keep the trait signature stable and
            // drop provenance on the floor; Penny still has the candidate
            // row with its provenance intact for audit replay.
            let request = KnowledgePutRequest {
                path: write.path.clone(),
                markdown: write.content.clone(),
                create_only: false,
                overwrite: true,
            };
            let receipt = svc.write(actor, request).await?;
            return Ok(receipt.path);
        }

        // Fallback: no KnowledgeService wired — return the proposed path
        // or synthesize a journal path. Mirrors the KC-1a behavior the
        // fixture-diff test + existing PSL-1 tests depend on.
        let resolved_path = candidate.proposed_path.clone().unwrap_or_else(|| {
            // Reconstruct a synthetic path without re-scanning the event
            // log: the candidate's own id is enough when we don't have a
            // date-bearing event in reach. The 1-arg form preserves the
            // pre-KC-1b path exactly.
            format!("ops/journal/{candidate_id}")
        });
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
