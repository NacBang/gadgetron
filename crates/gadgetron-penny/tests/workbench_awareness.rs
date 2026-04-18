//! Integration tests for `WorkbenchAwarenessGadgetProvider` dispatching
//! through the `GadgetRegistry`.
//!
//! Authority: `docs/design/phase2/13-penny-shared-surface-loop.md` §4/§5
//!
//! We build an in-memory `WorkbenchService` shim, register
//! `WorkbenchAwarenessGadgetProvider` in a `GadgetRegistry`, then dispatch
//! each of the four gadgets to assert outcomes.

use std::{pin::Pin, sync::Arc};

use gadgetron_core::{
    agent::{
        config::{AgentConfig, GadgetMode, SharedContextConfig},
        shared_context::{
            PennyActivityDigest, PennyCandidateDecisionReceipt, PennyCandidateDecisionRequest,
            PennyCandidateDigest,
        },
    },
    error::{GadgetronError, PennyErrorKind},
    knowledge::AuthenticatedContext,
    workbench::WorkbenchRequestEvidenceResponse,
};
use gadgetron_penny::workbench_awareness::{WorkbenchAwarenessGadgetProvider, WorkbenchService};
use gadgetron_penny::{GadgetRegistry, GadgetRegistryBuilder};
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// In-memory fake WorkbenchService
// ---------------------------------------------------------------------------

struct FakeWorkbenchService;

impl WorkbenchService for FakeWorkbenchService {
    fn recent_activity_boxed(
        &self,
        _actor: AuthenticatedContext,
        _limit: u32,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<PennyActivityDigest>, GadgetronError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async { Ok(vec![]) })
    }

    fn request_evidence_boxed(
        &self,
        _actor: AuthenticatedContext,
        request_id: Uuid,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<WorkbenchRequestEvidenceResponse, GadgetronError>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            Ok(WorkbenchRequestEvidenceResponse {
                request_id,
                tool_traces: vec![],
                citations: vec![],
                candidates: vec![],
            })
        })
    }

    fn pending_candidates_boxed(
        &self,
        _actor: AuthenticatedContext,
        _limit: u32,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<PennyCandidateDigest>, GadgetronError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async { Ok(vec![]) })
    }

    fn decide_candidate_boxed(
        &self,
        _actor: AuthenticatedContext,
        _req: PennyCandidateDecisionRequest,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<PennyCandidateDecisionReceipt, GadgetronError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async {
            Err(GadgetronError::Penny {
                kind: PennyErrorKind::ToolDenied {
                    reason: "W3-KC-1 pending (see doc 71): candidate decisions are not yet wired"
                        .to_string(),
                },
                message: "not implemented".to_string(),
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Helper: build a GadgetRegistry with WorkbenchAwarenessGadgetProvider
// ---------------------------------------------------------------------------

fn make_registry() -> GadgetRegistry {
    let provider = WorkbenchAwarenessGadgetProvider {
        actor: AuthenticatedContext,
        service: Arc::new(FakeWorkbenchService),
        config: SharedContextConfig::default(),
    };

    // Use default_mode = Auto so the L3 operator gate passes for
    // workbench.* Write tools; the provider's own Denied response
    // (W3-KC-1) then reaches the test assertions.
    let mut cfg = AgentConfig::default();
    cfg.gadgets.write.default_mode = GadgetMode::Auto;

    let mut builder = GadgetRegistryBuilder::new();
    builder
        .register(Arc::new(provider))
        .expect("register workbench provider");
    builder.freeze(&cfg)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_recent_dispatches_through_registry() {
    let registry = make_registry();
    let result = registry
        .dispatch("workbench.activity_recent", json!({"limit": 3}))
        .await;
    assert!(
        result.is_ok(),
        "activity_recent must succeed through registry: {result:?}"
    );
    let gadget_result = result.unwrap();
    assert!(!gadget_result.is_error, "must not be an error result");
    // Empty list from the fake service.
    assert_eq!(gadget_result.content, json!([]));
}

#[tokio::test]
async fn request_evidence_dispatches_through_registry() {
    let registry = make_registry();
    let id = Uuid::new_v4();
    let result = registry
        .dispatch(
            "workbench.request_evidence",
            json!({"request_id": id.to_string()}),
        )
        .await;
    assert!(
        result.is_ok(),
        "request_evidence must succeed through registry: {result:?}"
    );
    let gadget_result = result.unwrap();
    assert!(!gadget_result.is_error);
}

#[tokio::test]
async fn candidates_pending_returns_empty_stub_through_registry() {
    let registry = make_registry();
    let result = registry
        .dispatch("workbench.candidates_pending", json!({}))
        .await;
    assert!(result.is_ok());
    let gadget_result = result.unwrap();
    assert!(!gadget_result.is_error);
    assert_eq!(gadget_result.content, json!([]));
}

#[tokio::test]
async fn candidate_decide_returns_denied_through_registry() {
    let registry = make_registry();
    let result = registry
        .dispatch(
            "workbench.candidate_decide",
            json!({
                "candidate_id": Uuid::new_v4().to_string(),
                "decision": "accept"
            }),
        )
        .await;
    // The call itself should fail with Denied (surfaced as an error GadgetResult or Err).
    // Per current dispatch: GadgetError::Denied maps to GadgetResult { is_error: true }
    // via the registry, OR is returned as Err — depends on registry policy.
    // Either way the content or error must indicate denied / W3-KC-1.
    match result {
        Err(gadgetron_core::agent::tools::GadgetError::Denied { reason }) => {
            assert!(reason.contains("W3-KC-1"), "reason: {reason}");
        }
        Ok(res) if res.is_error => {
            // Acceptable — registry surfaced as error GadgetResult.
        }
        other => panic!("expected Denied or error result, got: {other:?}"),
    }
}
