//! Penny shared surface service and bootstrap assembler — gateway implementation.
//!
//! Authority: `docs/design/phase2/13-penny-shared-surface-loop.md` §2.1.2
//!
//! # Service layer
//!
//! [`PennySharedSurfaceService`] is the gateway-local trait with 5 async
//! methods. [`InProcessPennySharedSurfaceService`] implements it by delegating
//! to the [`WorkbenchProjectionService`] wired in W3-WEB-2.
//!
//! # Assembler
//!
//! [`DefaultPennyTurnContextAssembler`] implements
//! `gadgetron_core::agent::shared_context::PennyTurnContextAssembler`. It
//! runs the three read calls in parallel (via `futures::join!`), captures
//! individual failures as `health = Degraded + degraded_reasons`, and never
//! fails the whole bootstrap unless identity is broken (which is the caller's
//! responsibility).
//!
//! # Renderer
//!
//! [`render_penny_shared_context`] is a pure, deterministic function.
//! No I/O; no `async`. Designed for testing and for the PSL-1b wiring step.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use gadgetron_core::{
    agent::config::SharedContextConfig,
    agent::shared_context::{
        PennyActivityDigest, PennyApprovalDigest, PennyCandidateDecisionReceipt,
        PennyCandidateDecisionRequest, PennyCandidateDigest, PennySharedContextHealth,
        PennyTurnBootstrap, PennyTurnContextAssembler,
    },
    error::GadgetronError,
    knowledge::{
        candidate::{
            ActivityCaptureStore, CandidateDecision, KnowledgeCandidate,
            KnowledgeCandidateCoordinator, KnowledgeCandidateDisposition,
        },
        AuthenticatedContext,
    },
    workbench::WorkbenchRequestEvidenceResponse,
};
use uuid::Uuid;

use crate::web::workbench::{WorkbenchHttpError, WorkbenchProjectionService};

// ---------------------------------------------------------------------------
// PennySharedSurfaceService trait
// ---------------------------------------------------------------------------

/// Gateway-side contract for Penny shared surface reads and candidate decisions.
///
/// Implementors MUST NOT create separate DB query paths — they MUST delegate to
/// the same backing services used by the workbench UI routes (same
/// `WorkbenchProjectionService` / same actor filter).
///
/// All methods take `actor: &AuthenticatedContext` so actor-based filtering is
/// enforced at every call site. W3-KC-1 will lift this to a richer payload;
/// the ZST carrier keeps the shape stable now.
#[async_trait]
pub trait PennySharedSurfaceService: Send + Sync {
    /// Fetch recent activity entries, newest-first.
    ///
    /// `limit` is clamped to `[1, 50]` by the implementation.
    async fn recent_activity(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyActivityDigest>, GadgetronError>;

    /// Fetch pending knowledge candidates (unresolved disposition).
    ///
    /// `limit` is clamped to `[1, 20]` by the implementation. Always returns
    /// `Ok(vec![])` in PSL-1; KC-1 wires the real source.
    async fn pending_candidates(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyCandidateDigest>, GadgetronError>;

    /// Fetch pending operator approval requests.
    ///
    /// `limit` is clamped to `[0, 10]` by the implementation. Always returns
    /// `Ok(vec![])` in PSL-1; the approval store wires in PSL-1b.
    async fn pending_approvals(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyApprovalDigest>, GadgetronError>;

    /// Fetch evidence for a specific gateway request (tool traces, citations,
    /// candidates). Delegates to `WorkbenchProjectionService::request_evidence`.
    async fn request_evidence(
        &self,
        actor: &AuthenticatedContext,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, GadgetronError>;

    /// Record a disposition decision for a knowledge candidate.
    ///
    /// Always returns `Err(GadgetronError::Penny { kind: ToolDenied, … })` in
    /// PSL-1; KC-1 wires the real coordinator.
    async fn decide_candidate(
        &self,
        actor: &AuthenticatedContext,
        request: PennyCandidateDecisionRequest,
    ) -> Result<PennyCandidateDecisionReceipt, GadgetronError>;
}

// ---------------------------------------------------------------------------
// Arc<dyn PennySharedSurfaceService> blanket impl
// ---------------------------------------------------------------------------

/// Allow `DefaultPennyTurnContextAssembler` to be instantiated with
/// `Arc<dyn PennySharedSurfaceService>` as its generic parameter.
///
/// This makes the handler code simpler: it can pass `service.clone()` (where
/// `service: &Arc<dyn PennySharedSurfaceService>`) directly to the assembler
/// without a wrapper type.
#[async_trait]
impl PennySharedSurfaceService for Arc<dyn PennySharedSurfaceService> {
    async fn recent_activity(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyActivityDigest>, GadgetronError> {
        (**self).recent_activity(actor, limit).await
    }

    async fn pending_candidates(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyCandidateDigest>, GadgetronError> {
        (**self).pending_candidates(actor, limit).await
    }

    async fn pending_approvals(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyApprovalDigest>, GadgetronError> {
        (**self).pending_approvals(actor, limit).await
    }

    async fn request_evidence(
        &self,
        actor: &AuthenticatedContext,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, GadgetronError> {
        (**self).request_evidence(actor, request_id).await
    }

    async fn decide_candidate(
        &self,
        actor: &AuthenticatedContext,
        request: PennyCandidateDecisionRequest,
    ) -> Result<PennyCandidateDecisionReceipt, GadgetronError> {
        (**self).decide_candidate(actor, request).await
    }
}

// ---------------------------------------------------------------------------
// InProcessPennySharedSurfaceService
// ---------------------------------------------------------------------------

/// Default implementation that delegates to the in-process workbench projection.
///
/// Wired at startup with the same `Arc<dyn WorkbenchProjectionService>` that
/// the workbench HTTP routes use — ensures both surfaces read from the same
/// backing store with the same filter rules (doc §2.1.2 design rule 1).
///
/// # W3-KC-1 — optional candidate wiring
///
/// `candidate_store` and `candidate_coordinator` are `Option` so existing
/// PSL-1 callers (and tests that predate KC-1) continue to compile with
/// `None`. When `Some`, `pending_candidates` serves a live projection and
/// `decide_candidate` routes through the real coordinator; when `None`,
/// `pending_candidates` returns `Ok(vec![])` and `decide_candidate` returns
/// the preserved PSL-1 `ToolDenied` stub so the gateway still boots cleanly
/// against production TOML that hasn't wired the KC-1 store yet.
pub struct InProcessPennySharedSurfaceService {
    pub workbench_projection: Arc<dyn WorkbenchProjectionService>,
    /// Capture-plane store backing `pending_candidates` and `decide_candidate`.
    /// `None` preserves the PSL-1 behavior.
    pub candidate_store: Option<Arc<dyn ActivityCaptureStore>>,
    /// Capture-plane coordinator — reserved for future use by `capture_action`
    /// through this service. Held as a field now so the wiring surface is in
    /// place even though KC-1's read paths only need `candidate_store`.
    pub candidate_coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
}

impl InProcessPennySharedSurfaceService {
    /// Construct with just the workbench projection (matches existing PSL-1
    /// call sites). Leaves the capture-plane wiring unset.
    pub fn new(workbench_projection: Arc<dyn WorkbenchProjectionService>) -> Self {
        Self {
            workbench_projection,
            candidate_store: None,
            candidate_coordinator: None,
        }
    }

    /// Wire the KC-1 capture-plane dependencies onto an existing service.
    ///
    /// Returns `self` so construction can be chained:
    ///
    /// ```ignore
    /// let svc = InProcessPennySharedSurfaceService::new(projection)
    ///     .with_candidate_plane(store, coordinator);
    /// ```
    pub fn with_candidate_plane(
        mut self,
        store: Arc<dyn ActivityCaptureStore>,
        coordinator: Arc<dyn KnowledgeCandidateCoordinator>,
    ) -> Self {
        self.candidate_store = Some(store);
        self.candidate_coordinator = Some(coordinator);
        self
    }
}

#[async_trait]
impl PennySharedSurfaceService for InProcessPennySharedSurfaceService {
    async fn recent_activity(
        &self,
        _actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyActivityDigest>, GadgetronError> {
        let limit = limit.clamp(1, 50);
        let resp = self
            .workbench_projection
            .activity(limit)
            .await
            .map_err(workbench_err_to_gadgetron)?;

        let digests = resp
            .entries
            .into_iter()
            .map(|e| PennyActivityDigest {
                activity_event_id: e.event_id,
                request_id: e.request_id,
                origin: format!("{:?}", e.origin),
                kind: format!("{:?}", e.kind),
                title: e.title,
                summary: e.summary.clone().unwrap_or_default(),
                source_bundle: None,
                created_at: e.at,
            })
            .collect();

        Ok(digests)
    }

    async fn pending_candidates(
        &self,
        actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyCandidateDigest>, GadgetronError> {
        let limit = limit.clamp(1, 20);
        // W3-KC-1: when the capture store is wired, serve a live projection.
        // Preserves the PSL-1 empty-vec behavior when `candidate_store` is None
        // so the gateway still boots cleanly against production TOML that
        // hasn't wired KC-1 yet.
        let Some(store) = self.candidate_store.as_ref() else {
            return Ok(vec![]);
        };
        let rows = store
            .list_candidates(actor, limit as usize, /*only_pending=*/ true)
            .await?;
        Ok(rows.into_iter().map(candidate_to_digest).collect())
    }

    async fn pending_approvals(
        &self,
        _actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyApprovalDigest>, GadgetronError> {
        // Approval store not yet wired; PSL-1b / xaas-platform-lead will wire.
        let _limit = limit.clamp(0, 10);
        Ok(vec![])
    }

    async fn request_evidence(
        &self,
        _actor: &AuthenticatedContext,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, GadgetronError> {
        self.workbench_projection
            .request_evidence(request_id)
            .await
            .map_err(workbench_err_to_gadgetron)
    }

    async fn decide_candidate(
        &self,
        actor: &AuthenticatedContext,
        request: PennyCandidateDecisionRequest,
    ) -> Result<PennyCandidateDecisionReceipt, GadgetronError> {
        // W3-KC-1: when the capture store is wired, route decisions through
        // the real coordinator. Preserves the PSL-1 `ToolDenied` stub when
        // `candidate_store` is None so callers still get a typed 403-equivalent
        // if the capture plane hasn't been started yet.
        let Some(store) = self.candidate_store.as_ref() else {
            return Err(GadgetronError::Penny {
                kind: gadgetron_core::error::PennyErrorKind::ToolDenied {
                    reason:
                        "W3-KC-1 capture store is not wired for this gateway; cannot record decisions"
                            .to_string(),
                },
                message: "decide_candidate requires the capture-plane store to be wired"
                    .to_string(),
            });
        };

        let decision = CandidateDecision {
            candidate_id: request.candidate_id,
            decision: request.decision,
            decided_by_user_id: None,
            decided_by_penny: true,
            rationale: request.rationale,
        };
        let updated = store.decide_candidate(actor, decision).await?;
        Ok(PennyCandidateDecisionReceipt {
            candidate_id: updated.id,
            disposition: updated.disposition,
            canonical_path: updated.proposed_path.clone(),
            // KC-1 does not actually materialize yet; that lands in KC-1b.
            materialization_status: None,
            activity_event_id: updated.activity_event_id,
        })
    }
}

/// Map a `KnowledgeCandidate` from the capture plane into the Penny-facing
/// digest shape. Kept as a free function so unit tests can exercise the
/// mapping without constructing a full service.
fn candidate_to_digest(candidate: KnowledgeCandidate) -> PennyCandidateDigest {
    let requires_user_confirmation = matches!(
        candidate.disposition,
        KnowledgeCandidateDisposition::PendingUserConfirmation
    );
    PennyCandidateDigest {
        candidate_id: candidate.id,
        activity_event_id: candidate.activity_event_id,
        summary: candidate.summary,
        disposition: candidate.disposition,
        proposed_path: candidate.proposed_path,
        requires_user_confirmation,
        // KC-1 does not materialize; KC-1b adds status tracking.
        materialization_status: None,
    }
}

/// Map a `WorkbenchHttpError` to a `GadgetronError`.
///
/// `WorkbenchHttpError::Core(e)` passes through the wrapped error directly.
/// `WorkbenchHttpError::RequestNotFound` maps to `GadgetronError::Knowledge`
/// with `DocumentNotFound` so callers get a typed 404.
fn workbench_err_to_gadgetron(err: WorkbenchHttpError) -> GadgetronError {
    match err {
        WorkbenchHttpError::Core(e) => e,
        WorkbenchHttpError::RequestNotFound { request_id } => GadgetronError::Knowledge {
            kind: gadgetron_core::error::KnowledgeErrorKind::DocumentNotFound {
                path: format!("request/{request_id}"),
            },
            message: format!(
                "Request {request_id} not found or is not visible to the current actor. \
                 Verify the request_id or refresh the workbench."
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// DefaultPennyTurnContextAssembler
// ---------------------------------------------------------------------------

/// Assembles a `PennyTurnBootstrap` before each Penny turn.
///
/// Runs the three read calls (`recent_activity`, `pending_candidates`,
/// `pending_approvals`) in parallel using `futures::join!`. Individual
/// failures degrade the bootstrap rather than returning an error.
///
/// `S` must implement [`PennySharedSurfaceService`].
pub struct DefaultPennyTurnContextAssembler<S: PennySharedSurfaceService> {
    pub service: Arc<S>,
    pub config: SharedContextConfig,
}

#[async_trait]
impl<S> PennyTurnContextAssembler for DefaultPennyTurnContextAssembler<S>
where
    S: PennySharedSurfaceService + Send + Sync + 'static,
{
    async fn build(
        &self,
        actor: &AuthenticatedContext,
        conversation_id: Option<&str>,
        request_id: Uuid,
    ) -> Result<PennyTurnBootstrap, GadgetronError> {
        let activity_limit = self.config.bootstrap_activity_limit;
        let candidate_limit = self.config.bootstrap_candidate_limit;
        let approval_limit = self.config.bootstrap_approval_limit;

        // Run the three reads in parallel; capture individual failures.
        let (activity_res, candidates_res, approvals_res) = futures::join!(
            self.service.recent_activity(actor, activity_limit),
            self.service.pending_candidates(actor, candidate_limit),
            self.service.pending_approvals(actor, approval_limit),
        );

        let mut health = PennySharedContextHealth::Healthy;
        let mut degraded_reasons: Vec<String> = Vec::new();

        let recent_activity = match activity_res {
            Ok(v) => v,
            Err(e) => {
                health = PennySharedContextHealth::Degraded;
                degraded_reasons.push(format!("activity feed unavailable: {e}"));
                vec![]
            }
        };

        let pending_candidates = match candidates_res {
            Ok(v) => v,
            Err(e) => {
                health = PennySharedContextHealth::Degraded;
                degraded_reasons.push(format!("candidate feed unavailable: {e}"));
                vec![]
            }
        };

        let pending_approvals = match approvals_res {
            Ok(v) => v,
            Err(e) => {
                health = PennySharedContextHealth::Degraded;
                degraded_reasons.push(format!("approval feed unavailable: {e}"));
                vec![]
            }
        };

        tracing::debug!(
            request_id = %request_id,
            conversation_id = conversation_id.unwrap_or("<none>"),
            activity_count = recent_activity.len(),
            candidate_count = pending_candidates.len(),
            approval_count = pending_approvals.len(),
            health = ?health,
            "penny_turn_bootstrap_build"
        );

        if matches!(health, PennySharedContextHealth::Degraded) {
            for reason in &degraded_reasons {
                tracing::warn!(
                    request_id = %request_id,
                    reason = reason.as_str(),
                    "penny_shared_context_partial_failure"
                );
            }
        }

        Ok(PennyTurnBootstrap {
            request_id,
            conversation_id: conversation_id.map(|s| s.to_string()),
            generated_at: Utc::now(),
            health,
            degraded_reasons,
            recent_activity,
            pending_candidates,
            pending_approvals,
        })
    }
}

// ---------------------------------------------------------------------------
// render_penny_shared_context
// ---------------------------------------------------------------------------

/// Render a `PennyTurnBootstrap` as the deterministic `<gadgetron_shared_context>`
/// text block injected before each Penny turn.
///
/// # Format (doc §2.2.2)
///
/// ```text
/// <gadgetron_shared_context>
/// generated_at: <RFC3339>
/// health: <healthy|degraded>
/// degraded_reasons:
/// - ...
/// recent_activity:
/// - [<kind>] <title_clipped>
/// pending_candidates:
/// - [<disposition>] <summary_clipped>
/// pending_approvals:
/// - [<risk_tier>] <title_clipped>
/// </gadgetron_shared_context>
/// ```
///
/// # Rules
///
/// - Empty lists render as `recent_activity: []` (not omitted).
/// - Each summary/title is clipped to `digest_summary_chars` Unicode code
///   points with `…` appended when clipped.
/// - Raw JSON, file paths, DB keys, and stack traces MUST NOT appear.
/// - Pure function; no I/O; no `async`.
pub fn render_penny_shared_context(
    bootstrap: &PennyTurnBootstrap,
    digest_summary_chars: usize,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("<gadgetron_shared_context>\n");

    // generated_at — RFC 3339
    out.push_str(&format!(
        "generated_at: {}\n",
        bootstrap.generated_at.to_rfc3339()
    ));

    // health
    let health_str = match bootstrap.health {
        PennySharedContextHealth::Healthy => "healthy",
        PennySharedContextHealth::Degraded => "degraded",
    };
    out.push_str(&format!("health: {health_str}\n"));

    // degraded_reasons — always render the header, even when empty
    out.push_str("degraded_reasons:\n");
    for reason in &bootstrap.degraded_reasons {
        out.push_str(&format!("- {}\n", reason));
    }

    // recent_activity
    if bootstrap.recent_activity.is_empty() {
        out.push_str("recent_activity: []\n");
    } else {
        out.push_str("recent_activity:\n");
        for entry in &bootstrap.recent_activity {
            let clipped = clip_to_code_points(&entry.title, digest_summary_chars);
            out.push_str(&format!("- [{}] {}\n", entry.kind, clipped));
        }
    }

    // pending_candidates
    if bootstrap.pending_candidates.is_empty() {
        out.push_str("pending_candidates: []\n");
    } else {
        out.push_str("pending_candidates:\n");
        for c in &bootstrap.pending_candidates {
            // Wire-stable snake_case: emits the same string that
            // `#[serde(rename_all = "snake_case")]` produces over JSON
            // (e.g. `pending_penny_decision`). The prior
            // `format!("{:?}").to_lowercase()` collapsed to
            // `pendingpennydecision`, which silently diverged from every
            // other audit-plane consumer of this enum.
            let disposition = enum_snake_case_label(&c.disposition);
            let clipped = clip_to_code_points(&c.summary, digest_summary_chars);
            out.push_str(&format!("- [{disposition}] {clipped}\n"));
        }
    }

    // pending_approvals
    if bootstrap.pending_approvals.is_empty() {
        out.push_str("pending_approvals: []\n");
    } else {
        out.push_str("pending_approvals:\n");
        for a in &bootstrap.pending_approvals {
            let clipped = clip_to_code_points(&a.title, digest_summary_chars);
            out.push_str(&format!("- [{}] {}\n", a.risk_tier, clipped));
        }
    }

    out.push_str("</gadgetron_shared_context>");
    out
}

/// Clip a string to at most `max_chars` Unicode scalar values (code points),
/// appending `…` if the string was truncated.
fn clip_to_code_points(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut result = String::with_capacity(max_chars + 3);
    for (count, c) in s.chars().enumerate() {
        if count == max_chars {
            // There are more chars; we need to clip.
            result.push('…');
            return result;
        }
        result.push(c);
    }
    result
}

/// Serialize a `#[serde(rename_all = "snake_case")]` enum into its wire
/// label (e.g. `KnowledgeCandidateDisposition::PendingPennyDecision` →
/// `"pending_penny_decision"`).
///
/// Falls back to `"unknown"` for the (impossible-in-practice) case where
/// serde produces a non-string JSON value — keeping the signature
/// infallible lets the renderer stay a pure function with no `Result`
/// plumbing. This mirrors the same helper on the capture-plane side in
/// `gadgetron_knowledge::candidate::enum_snake_case_label`.
fn enum_snake_case_label<E: serde::Serialize>(e: &E) -> String {
    serde_json::to_value(e)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use gadgetron_core::agent::shared_context::PennySharedContextHealth;
    use gadgetron_core::knowledge::AuthenticatedContext;
    use gadgetron_core::workbench::WorkbenchKnowledgeSummary;
    use gadgetron_core::workbench::{WorkbenchActivityResponse, WorkbenchBootstrapResponse};
    use std::sync::Arc;
    use uuid::Uuid;

    // ----------------------------------------------------------------
    // Minimal fake WorkbenchProjectionService
    // ----------------------------------------------------------------

    struct FakeProjectionEmpty;

    #[async_trait]
    impl WorkbenchProjectionService for FakeProjectionEmpty {
        async fn bootstrap(&self) -> Result<WorkbenchBootstrapResponse, WorkbenchHttpError> {
            Ok(WorkbenchBootstrapResponse {
                gateway_version: "0.0.0-test".into(),
                default_model: None,
                active_plugs: vec![],
                degraded_reasons: vec![],
                knowledge: WorkbenchKnowledgeSummary {
                    canonical_ready: false,
                    search_ready: false,
                    relation_ready: false,
                    last_ingest_at: None,
                },
            })
        }
        async fn activity(
            &self,
            _limit: u32,
        ) -> Result<WorkbenchActivityResponse, WorkbenchHttpError> {
            Ok(WorkbenchActivityResponse {
                entries: vec![],
                is_truncated: false,
            })
        }
        async fn request_evidence(
            &self,
            request_id: Uuid,
        ) -> Result<WorkbenchRequestEvidenceResponse, WorkbenchHttpError> {
            Err(WorkbenchHttpError::RequestNotFound { request_id })
        }
    }

    fn make_service() -> InProcessPennySharedSurfaceService {
        InProcessPennySharedSurfaceService::new(Arc::new(FakeProjectionEmpty))
    }

    fn actor() -> AuthenticatedContext {
        AuthenticatedContext
    }

    // ----------------------------------------------------------------
    // InProcessPennySharedSurfaceService tests
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn recent_activity_delegates_to_workbench_projection() {
        let svc = make_service();
        let result = svc.recent_activity(&actor(), 10).await;
        assert!(result.is_ok(), "should succeed: {result:?}");
        assert!(result.unwrap().is_empty(), "stub returns empty list");
    }

    #[tokio::test]
    async fn pending_candidates_returns_empty_stub() {
        let svc = make_service();
        let result = svc.pending_candidates(&actor(), 5).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn pending_approvals_returns_empty_stub() {
        let svc = make_service();
        let result = svc.pending_approvals(&actor(), 3).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn request_evidence_propagates_projection_result() {
        let svc = make_service();
        let request_id = Uuid::new_v4();
        let err = svc
            .request_evidence(&actor(), request_id)
            .await
            .unwrap_err();
        // Should map to GadgetronError::Knowledge (DocumentNotFound)
        match err {
            GadgetronError::Knowledge {
                kind: gadgetron_core::error::KnowledgeErrorKind::DocumentNotFound { path },
                ..
            } => {
                assert!(
                    path.contains(&request_id.to_string()),
                    "path must include request_id: {path}"
                );
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pending_candidates_returns_empty_when_store_none() {
        // Preserves the PSL-1 behavior — when `candidate_store` is None,
        // the read serves an empty Vec without contacting any backend.
        let svc = make_service();
        let rows = svc.pending_candidates(&actor(), 5).await.unwrap();
        assert!(rows.is_empty(), "must return empty when store is None");
    }

    #[tokio::test]
    async fn decide_candidate_denied_when_store_none() {
        // Preserves the PSL-1 stub behavior as a typed 403-equivalent so
        // production TOML that hasn't wired KC-1 still gives Penny a clean
        // error rather than a panic.
        let svc = make_service();
        let req = PennyCandidateDecisionRequest {
            candidate_id: Uuid::new_v4(),
            decision: gadgetron_core::knowledge::candidate::CandidateDecisionKind::Accept,
            rationale: None,
        };
        let err = svc.decide_candidate(&actor(), req).await.unwrap_err();
        match &err {
            GadgetronError::Penny {
                kind: gadgetron_core::error::PennyErrorKind::ToolDenied { reason },
                ..
            } => {
                assert!(
                    reason.contains("capture store"),
                    "reason must mention capture store: {reason}"
                );
            }
            other => panic!("expected ToolDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pending_candidates_returns_live_when_store_some() {
        use gadgetron_core::knowledge::candidate::{
            ActivityKind, ActivityOrigin, CandidateHint, CapturedActivityEvent,
        };
        use gadgetron_knowledge::candidate::{
            InMemoryActivityCaptureStore, InProcessCandidateCoordinator,
        };

        let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
        let coord: Arc<dyn KnowledgeCandidateCoordinator> = Arc::new(
            InProcessCandidateCoordinator::new(store.clone(), /*max=*/ 8),
        );
        let svc = InProcessPennySharedSurfaceService::new(Arc::new(FakeProjectionEmpty))
            .with_candidate_plane(store.clone(), coord.clone());

        let event = CapturedActivityEvent {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            actor_user_id: Uuid::new_v4(),
            request_id: None,
            origin: ActivityOrigin::UserDirect,
            kind: ActivityKind::DirectAction,
            title: "demo".into(),
            summary: "demo event".into(),
            source_bundle: None,
            source_capability: None,
            audit_event_id: None,
            facts: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        let hints = vec![CandidateHint {
            summary: "demo candidate".into(),
            proposed_path: Some("ops/journal/demo".into()),
            tags: vec!["ops".into()],
            reason: Some("live_test".into()),
        }];
        coord.capture_action(&actor(), event, hints).await.unwrap();

        let rows = svc.pending_candidates(&actor(), 5).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].summary, "demo candidate");
        assert_eq!(
            rows[0].disposition,
            gadgetron_core::knowledge::candidate::KnowledgeCandidateDisposition::PendingPennyDecision
        );
        assert!(!rows[0].requires_user_confirmation);
    }

    #[tokio::test]
    async fn decide_candidate_returns_receipt_when_store_some() {
        use gadgetron_core::knowledge::candidate::{
            ActivityKind, ActivityOrigin, CandidateDecisionKind, CandidateHint,
            CapturedActivityEvent, KnowledgeCandidateDisposition,
        };
        use gadgetron_knowledge::candidate::{
            InMemoryActivityCaptureStore, InProcessCandidateCoordinator,
        };

        let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
        let coord: Arc<dyn KnowledgeCandidateCoordinator> =
            Arc::new(InProcessCandidateCoordinator::new(store.clone(), 8));
        let svc = InProcessPennySharedSurfaceService::new(Arc::new(FakeProjectionEmpty))
            .with_candidate_plane(store.clone(), coord.clone());

        let event = CapturedActivityEvent {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            actor_user_id: Uuid::new_v4(),
            request_id: None,
            origin: ActivityOrigin::Penny,
            kind: ActivityKind::GadgetToolCall,
            title: "tool".into(),
            summary: "tool call".into(),
            source_bundle: None,
            source_capability: None,
            audit_event_id: None,
            facts: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        let hints = vec![CandidateHint {
            summary: "tool call summary".into(),
            proposed_path: Some("ops/journal/tool".into()),
            tags: vec![],
            reason: None,
        }];
        let created = coord.capture_action(&actor(), event, hints).await.unwrap();
        let candidate_id = created[0].id;

        let receipt = svc
            .decide_candidate(
                &actor(),
                PennyCandidateDecisionRequest {
                    candidate_id,
                    decision: CandidateDecisionKind::Accept,
                    rationale: Some("worth saving".into()),
                },
            )
            .await
            .unwrap();

        assert_eq!(receipt.candidate_id, candidate_id);
        assert_eq!(receipt.disposition, KnowledgeCandidateDisposition::Accepted);
        assert_eq!(receipt.canonical_path.as_deref(), Some("ops/journal/tool"));
        assert!(receipt.materialization_status.is_none());
    }

    // ----------------------------------------------------------------
    // render_penny_shared_context tests
    // ----------------------------------------------------------------

    fn empty_bootstrap() -> PennyTurnBootstrap {
        PennyTurnBootstrap {
            request_id: Uuid::nil(),
            conversation_id: None,
            generated_at: chrono::Utc.with_ymd_and_hms(2026, 4, 18, 14, 3, 0).unwrap(),
            health: PennySharedContextHealth::Healthy,
            degraded_reasons: vec![],
            recent_activity: vec![],
            pending_candidates: vec![],
            pending_approvals: vec![],
        }
    }

    #[test]
    fn render_empty_bootstrap_includes_all_section_headers_with_brackets() {
        let bootstrap = empty_bootstrap();
        let rendered = render_penny_shared_context(&bootstrap, 240);

        assert!(
            rendered.contains("<gadgetron_shared_context>"),
            "must have opening tag"
        );
        assert!(
            rendered.contains("</gadgetron_shared_context>"),
            "must have closing tag"
        );
        assert!(
            rendered.contains("recent_activity: []"),
            "empty activity must render as []"
        );
        assert!(
            rendered.contains("pending_candidates: []"),
            "empty candidates must render as []"
        );
        assert!(
            rendered.contains("pending_approvals: []"),
            "empty approvals must render as []"
        );
        assert!(rendered.contains("health: healthy"));
        assert!(rendered.contains("generated_at:"));
    }

    #[test]
    fn render_truncates_summary_at_digest_summary_chars() {
        use chrono::TimeZone;
        let long_title = "a".repeat(300);
        let mut bootstrap = empty_bootstrap();
        bootstrap.recent_activity.push(PennyActivityDigest {
            activity_event_id: Uuid::nil(),
            request_id: None,
            origin: "penny".to_string(),
            kind: "chat_turn".to_string(),
            title: long_title.clone(),
            summary: String::new(),
            source_bundle: None,
            created_at: chrono::Utc.with_ymd_and_hms(2026, 4, 18, 0, 0, 0).unwrap(),
        });

        let rendered = render_penny_shared_context(&bootstrap, 100);
        // The rendered title should end with …
        assert!(
            rendered.contains('…'),
            "long title must be clipped with ellipsis"
        );
        // The full 300-char title must NOT appear verbatim
        assert!(
            !rendered.contains(&long_title),
            "full long title must not appear"
        );
    }

    #[test]
    fn render_preserves_degraded_reasons_verbatim() {
        let mut bootstrap = empty_bootstrap();
        bootstrap.health = PennySharedContextHealth::Degraded;
        bootstrap
            .degraded_reasons
            .push("pending approval store unavailable; counts may be stale".to_string());

        let rendered = render_penny_shared_context(&bootstrap, 240);
        assert!(rendered.contains("health: degraded"));
        assert!(rendered.contains("pending approval store unavailable"));
    }

    // ----------------------------------------------------------------
    // Assembler tests
    // ----------------------------------------------------------------

    /// Fake service where all reads succeed.
    struct AllSucceedService;

    #[async_trait]
    impl PennySharedSurfaceService for AllSucceedService {
        async fn recent_activity(
            &self,
            _actor: &AuthenticatedContext,
            _limit: u32,
        ) -> Result<Vec<PennyActivityDigest>, GadgetronError> {
            Ok(vec![])
        }
        async fn pending_candidates(
            &self,
            _actor: &AuthenticatedContext,
            _limit: u32,
        ) -> Result<Vec<PennyCandidateDigest>, GadgetronError> {
            Ok(vec![])
        }
        async fn pending_approvals(
            &self,
            _actor: &AuthenticatedContext,
            _limit: u32,
        ) -> Result<Vec<PennyApprovalDigest>, GadgetronError> {
            Ok(vec![])
        }
        async fn request_evidence(
            &self,
            _actor: &AuthenticatedContext,
            _request_id: Uuid,
        ) -> Result<WorkbenchRequestEvidenceResponse, GadgetronError> {
            Err(GadgetronError::Forbidden)
        }
        async fn decide_candidate(
            &self,
            _actor: &AuthenticatedContext,
            _request: PennyCandidateDecisionRequest,
        ) -> Result<PennyCandidateDecisionReceipt, GadgetronError> {
            Err(GadgetronError::Forbidden)
        }
    }

    /// Fake service where `pending_candidates` always fails.
    struct CandidateFail;

    #[async_trait]
    impl PennySharedSurfaceService for CandidateFail {
        async fn recent_activity(
            &self,
            _actor: &AuthenticatedContext,
            _limit: u32,
        ) -> Result<Vec<PennyActivityDigest>, GadgetronError> {
            Ok(vec![])
        }
        async fn pending_candidates(
            &self,
            _actor: &AuthenticatedContext,
            _limit: u32,
        ) -> Result<Vec<PennyCandidateDigest>, GadgetronError> {
            Err(GadgetronError::Config("candidate projection down".into()))
        }
        async fn pending_approvals(
            &self,
            _actor: &AuthenticatedContext,
            _limit: u32,
        ) -> Result<Vec<PennyApprovalDigest>, GadgetronError> {
            Ok(vec![])
        }
        async fn request_evidence(
            &self,
            _actor: &AuthenticatedContext,
            _request_id: Uuid,
        ) -> Result<WorkbenchRequestEvidenceResponse, GadgetronError> {
            Err(GadgetronError::Forbidden)
        }
        async fn decide_candidate(
            &self,
            _actor: &AuthenticatedContext,
            _request: PennyCandidateDecisionRequest,
        ) -> Result<PennyCandidateDecisionReceipt, GadgetronError> {
            Err(GadgetronError::Forbidden)
        }
    }

    #[tokio::test]
    async fn build_healthy_when_all_reads_succeed() {
        let assembler = DefaultPennyTurnContextAssembler {
            service: Arc::new(AllSucceedService),
            config: SharedContextConfig::default(),
        };
        let bootstrap = assembler
            .build(&actor(), Some("conv-1"), Uuid::new_v4())
            .await
            .unwrap();

        assert_eq!(bootstrap.health, PennySharedContextHealth::Healthy);
        assert!(bootstrap.degraded_reasons.is_empty());
        assert_eq!(bootstrap.conversation_id, Some("conv-1".to_string()));
    }

    #[tokio::test]
    async fn build_degraded_when_one_read_fails() {
        let assembler = DefaultPennyTurnContextAssembler {
            service: Arc::new(CandidateFail),
            config: SharedContextConfig::default(),
        };
        let bootstrap = assembler
            .build(&actor(), None, Uuid::new_v4())
            .await
            .unwrap();

        assert_eq!(bootstrap.health, PennySharedContextHealth::Degraded);
        assert!(!bootstrap.degraded_reasons.is_empty());
        assert!(
            bootstrap
                .degraded_reasons
                .iter()
                .any(|r| r.contains("candidate feed unavailable")),
            "degraded reasons must name the failed component: {:?}",
            bootstrap.degraded_reasons
        );
    }
}
