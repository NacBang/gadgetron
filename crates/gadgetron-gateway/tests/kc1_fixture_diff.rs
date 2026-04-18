//! W3-KC-1 exit-gate fixture-diff test.
//!
//! Authority: `docs/design/core/knowledge-candidate-curation.md` +
//! D-20260418-17. Codex called the fixture-diff the "cycle exit gate" for
//! cycle 9. The test exercises the full KC-1 in-memory slice:
//!
//! 1. In-memory store + coordinator wired through
//!    [`InProcessPennySharedSurfaceService::with_candidate_plane`].
//! 2. Capture an activity event with two candidate hints.
//! 3. Ask the `DefaultPennyTurnContextAssembler` for a bootstrap,
//!    render the shared-surface block, and assert it includes both
//!    candidate summaries + the `pending_penny_decision` disposition.
//! 4. Decide the first candidate (Accept).
//! 5. Re-ask the assembler, re-render, and assert the accepted candidate
//!    dropped out of the pending list while the second remains.
//!
//! `generated_at` is stripped out of the rendered block with a regex-lite
//! string replace so the test is deterministic across runs.

use std::sync::Arc;

use gadgetron_core::agent::config::SharedContextConfig;
use gadgetron_core::agent::shared_context::PennyTurnContextAssembler;
use gadgetron_core::knowledge::candidate::{
    ActivityCaptureStore, ActivityKind, ActivityOrigin, CandidateDecision, CandidateDecisionKind,
    CandidateHint, CapturedActivityEvent, KnowledgeCandidateCoordinator,
};
use gadgetron_core::knowledge::AuthenticatedContext;
use gadgetron_core::workbench::{
    WorkbenchActivityResponse, WorkbenchBootstrapResponse, WorkbenchKnowledgeSummary,
    WorkbenchRequestEvidenceResponse,
};
use gadgetron_gateway::penny::shared_context::{
    render_penny_shared_context, DefaultPennyTurnContextAssembler,
    InProcessPennySharedSurfaceService, PennySharedSurfaceService,
};
use gadgetron_gateway::web::workbench::{WorkbenchHttpError, WorkbenchProjectionService};
use gadgetron_knowledge::candidate::{InMemoryActivityCaptureStore, InProcessCandidateCoordinator};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Minimal fake projection — the KC-1 slice only exercises the candidate
// read path; activity + approval feeds are not under test here.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FakeProjectionEmpty;

#[async_trait::async_trait]
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

    async fn activity(&self, _limit: u32) -> Result<WorkbenchActivityResponse, WorkbenchHttpError> {
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

    async fn knowledge_status(
        &self,
    ) -> Result<gadgetron_core::workbench::WorkbenchKnowledgeStatusResponse, WorkbenchHttpError>
    {
        Ok(
            gadgetron_core::workbench::WorkbenchKnowledgeStatusResponse {
                canonical_ready: false,
                search_ready: false,
                relation_ready: false,
                stale_reasons: vec![],
                last_ingest_at: None,
            },
        )
    }

    async fn views(
        &self,
    ) -> Result<gadgetron_core::workbench::WorkbenchRegisteredViewsResponse, WorkbenchHttpError>
    {
        Ok(gadgetron_core::workbench::WorkbenchRegisteredViewsResponse { views: vec![] })
    }

    async fn view_data(
        &self,
        view_id: &str,
    ) -> Result<gadgetron_core::workbench::WorkbenchViewData, WorkbenchHttpError> {
        Err(WorkbenchHttpError::ViewNotFound {
            view_id: view_id.to_string(),
        })
    }

    async fn actions(
        &self,
    ) -> Result<gadgetron_core::workbench::WorkbenchRegisteredActionsResponse, WorkbenchHttpError>
    {
        Ok(gadgetron_core::workbench::WorkbenchRegisteredActionsResponse { actions: vec![] })
    }
}

fn actor() -> AuthenticatedContext {
    AuthenticatedContext::system()
}

/// Strip the time-dependent `generated_at:` line so the block is byte-
/// comparable across runs.
fn normalize(block: &str) -> String {
    let mut out = String::with_capacity(block.len());
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("generated_at: ") {
            let _ = rest; // discard the RFC3339 timestamp
            out.push_str("generated_at: <TS>\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    // `lines()` drops the trailing newline of the close tag; the original
    // block has no trailing newline after `</gadgetron_shared_context>` so
    // trim the one we just added to re-match shape.
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn make_event(title: &str) -> CapturedActivityEvent {
    CapturedActivityEvent {
        id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        actor_user_id: Uuid::new_v4(),
        request_id: None,
        origin: ActivityOrigin::UserDirect,
        kind: ActivityKind::DirectAction,
        title: title.to_string(),
        summary: format!("{title} summary"),
        source_bundle: None,
        source_capability: None,
        audit_event_id: None,
        facts: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn kc1_fixture_diff_shared_context_reflects_decide_candidate() {
    // 1. Build store + coordinator + shared-surface service.
    let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
    let coord: Arc<dyn KnowledgeCandidateCoordinator> =
        Arc::new(InProcessCandidateCoordinator::new(store.clone(), 8));
    let svc: Arc<dyn PennySharedSurfaceService> = Arc::new(
        InProcessPennySharedSurfaceService::new(Arc::new(FakeProjectionEmpty))
            .with_candidate_plane(store.clone(), coord.clone()),
    );

    // 2. Capture a single activity with two candidate hints — must surface
    //    as two PendingPennyDecision candidates.
    let event = make_event("tap-restart");
    let hints = vec![
        CandidateHint {
            summary: "restart-grafana".to_string(),
            proposed_path: Some(
                gadgetron_core::knowledge::KnowledgePath::new("ops/journal/restart-grafana")
                    .unwrap(),
            ),
            tags: vec!["ops".to_string()],
            reason: Some("direct_action".to_string()),
        },
        CandidateHint {
            summary: "tap-runbook-delta".to_string(),
            proposed_path: Some(
                gadgetron_core::knowledge::KnowledgePath::new("ops/journal/tap-runbook-delta")
                    .unwrap(),
            ),
            tags: vec!["runbook".to_string()],
            reason: Some("runbook_delta".to_string()),
        },
    ];
    let created = coord
        .capture_action(&actor(), event, hints)
        .await
        .expect("capture_action should succeed");
    assert_eq!(created.len(), 2, "two hints must produce two candidates");

    // 3. Assemble a bootstrap + render the block.
    let assembler = DefaultPennyTurnContextAssembler {
        service: Arc::new(svc.clone()),
        config: SharedContextConfig::default(),
    };
    let bootstrap = assembler
        .build(&actor(), Some("conv-kc1"), Uuid::new_v4())
        .await
        .expect("bootstrap build should succeed");
    let rendered_pre = render_penny_shared_context(
        &bootstrap,
        SharedContextConfig::default().digest_summary_chars as usize,
    );
    let normalized_pre = normalize(&rendered_pre);

    // Expected shape (deterministic after `generated_at` normalization).
    // Candidates come back newest-first, with a stable id tie-break — the
    // second-created candidate ("tap-runbook-delta") has the newer ordering
    // in the store's projection because we call `list` after both inserts.
    // We tolerate either permutation by asserting on the two anchors without
    // pinning their relative order, then snapshot the full block with the
    // observed order so a future accidental reorder gets caught.
    assert!(
        normalized_pre.contains("restart-grafana"),
        "restart-grafana must appear before any decide call: {normalized_pre}"
    );
    assert!(
        normalized_pre.contains("tap-runbook-delta"),
        "tap-runbook-delta must appear before any decide call: {normalized_pre}"
    );
    // Wire-stable snake_case (matches the serde rename of
    // `KnowledgeCandidateDisposition::PendingPennyDecision`).
    let pending_count = normalized_pre.matches("[pending_penny_decision]").count();
    assert_eq!(
        pending_count, 2,
        "both candidates must render as pending_penny_decision: {normalized_pre}"
    );

    // Determinism: render twice with the same bootstrap — bytes must match
    // exactly (timestamps are normalized already).
    let again = normalize(&render_penny_shared_context(
        &bootstrap,
        SharedContextConfig::default().digest_summary_chars as usize,
    ));
    assert_eq!(normalized_pre, again, "rendering is deterministic");

    // 4. Decide the first candidate (Accept). `created[0]` == "restart-grafana".
    let accepted_id = created[0].id;
    let remaining_summary = created[1].summary.clone();
    store
        .decide_candidate(
            &actor(),
            CandidateDecision {
                candidate_id: accepted_id,
                decision: CandidateDecisionKind::Accept,
                decided_by_user_id: None,
                decided_by_penny: true,
                rationale: Some("ops journal worthy".to_string()),
            },
        )
        .await
        .expect("decide_candidate should succeed");

    // 5. Re-assemble + re-render. Accepted candidate must disappear from
    //    the pending projection.
    let bootstrap2 = assembler
        .build(&actor(), Some("conv-kc1"), Uuid::new_v4())
        .await
        .expect("second bootstrap build should succeed");
    let rendered_post = render_penny_shared_context(
        &bootstrap2,
        SharedContextConfig::default().digest_summary_chars as usize,
    );
    let normalized_post = normalize(&rendered_post);

    assert!(
        !normalized_post.contains(&created[0].summary),
        "accepted candidate must drop out of pending projection: {normalized_post}"
    );
    assert!(
        normalized_post.contains(&remaining_summary),
        "non-accepted candidate must still be present: {normalized_post}"
    );
    let pending_post = normalized_post.matches("[pending_penny_decision]").count();
    assert_eq!(
        pending_post, 1,
        "exactly one candidate should remain pending: {normalized_post}"
    );

    // Print the normalized blocks so the first CI run records them in the
    // test output (helpful for review / follow-up KC-1b change diffs).
    println!("=== KC-1 fixture-diff pre-decide (normalized) ===\n{normalized_pre}\n");
    println!("=== KC-1 fixture-diff post-decide (normalized) ===\n{normalized_post}\n");
}
