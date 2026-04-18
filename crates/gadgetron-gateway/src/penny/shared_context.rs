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
    knowledge::AuthenticatedContext,
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
pub struct InProcessPennySharedSurfaceService {
    pub workbench_projection: Arc<dyn WorkbenchProjectionService>,
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
        _actor: &AuthenticatedContext,
        limit: u32,
    ) -> Result<Vec<PennyCandidateDigest>, GadgetronError> {
        // KC-1 will replace this stub with a real candidate projection query.
        let _limit = limit.clamp(1, 20);
        Ok(vec![])
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
        _actor: &AuthenticatedContext,
        _request: PennyCandidateDecisionRequest,
    ) -> Result<PennyCandidateDecisionReceipt, GadgetronError> {
        // KC-1 will wire the real KnowledgeCandidateCoordinator here.
        Err(GadgetronError::Penny {
            kind: gadgetron_core::error::PennyErrorKind::ToolDenied {
                reason: "W3-KC-1 pending (see doc 71): candidate decisions are not yet wired"
                    .to_string(),
            },
            message: "decide_candidate is not implemented until W3-KC-1".to_string(),
        })
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
            let disposition = format!("{:?}", c.disposition).to_lowercase();
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
        InProcessPennySharedSurfaceService {
            workbench_projection: Arc::new(FakeProjectionEmpty),
        }
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
    async fn decide_candidate_returns_unsupported() {
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
                    reason.contains("W3-KC-1"),
                    "reason must mention W3-KC-1: {reason}"
                );
            }
            other => panic!("expected ToolDenied, got {other:?}"),
        }
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
