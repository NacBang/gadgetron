//! Workbench-awareness Gadget provider for Penny.
//!
//! Authority: `docs/design/phase2/13-penny-shared-surface-loop.md` §2.1.3
//!
//! # Gadgets exposed
//!
//! | Gadget | Tier | Purpose |
//! |---|---|---|
//! | `workbench.activity_recent` | `Read` | Read recent shared activity |
//! | `workbench.request_evidence` | `Read` | Drill into a specific request |
//! | `workbench.candidates_pending` | `Read` | List pending knowledge candidates |
//! | `workbench.candidate_decide` | `Write` | Accept / reject / escalate a candidate |
//!
//! # Actor binding
//!
//! `WorkbenchAwarenessGadgetProvider` is NOT a process-global singleton — it is
//! constructed per Penny request with the actor snapshot from `AuthenticatedContext`.
//! PSL-1b wires this at the request ingress boundary.
//!
//! # KC-1 stubs
//!
//! `workbench.candidates_pending` always returns `Ok([])` in PSL-1.
//! `workbench.candidate_decide` always returns `Err(Denied)` in PSL-1.

use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::{
    agent::shared_context::PennyCandidateDecisionRequest,
    agent::tools::{GadgetError, GadgetProvider, GadgetResult, GadgetSchema, GadgetTier},
    error::GadgetronError,
    knowledge::candidate::CandidateDecisionKind,
    knowledge::AuthenticatedContext,
};
use serde_json::{json, Value};
use uuid::Uuid;

/// Trait alias used by this module — defined in `gadgetron-gateway::penny`.
/// Re-required here because `gadgetron-penny` cannot depend on `gadgetron-gateway`
/// (circular). The trait is defined in the dependency of both: note that we
/// use the re-exported trait path via a generic bound.
///
/// Actually `PennySharedSurfaceService` lives in `gadgetron-gateway`. To avoid
/// a circular dependency (`gadgetron-penny` → `gadgetron-gateway` → `gadgetron-penny`),
/// this provider is generic over a trait `S` that the caller supplies. The caller
/// (always inside `gadgetron-gateway` or a test) provides an `Arc<S>` that satisfies
/// the `PennySharedSurfaceService` contract.
///
/// The concrete trait bound is expressed as an associated supertrait below.
pub trait WorkbenchService: Send + Sync + 'static {
    fn recent_activity_boxed(
        &self,
        actor: AuthenticatedContext,
        limit: u32,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Vec<gadgetron_core::agent::shared_context::PennyActivityDigest>,
                        GadgetronError,
                    >,
                > + Send
                + '_,
        >,
    >;

    fn request_evidence_boxed(
        &self,
        actor: AuthenticatedContext,
        request_id: Uuid,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gadgetron_core::workbench::WorkbenchRequestEvidenceResponse,
                        GadgetronError,
                    >,
                > + Send
                + '_,
        >,
    >;

    fn pending_candidates_boxed(
        &self,
        actor: AuthenticatedContext,
        limit: u32,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Vec<gadgetron_core::agent::shared_context::PennyCandidateDigest>,
                        GadgetronError,
                    >,
                > + Send
                + '_,
        >,
    >;

    fn decide_candidate_boxed(
        &self,
        actor: AuthenticatedContext,
        request: PennyCandidateDecisionRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gadgetron_core::agent::shared_context::PennyCandidateDecisionReceipt,
                        GadgetronError,
                    >,
                > + Send
                + '_,
        >,
    >;
}

/// Penny Gadget provider for workbench awareness.
///
/// Bound to a specific actor (`AuthenticatedContext`) at construction time.
/// PSL-1b constructs one per Penny request; it MUST NOT be cached globally.
pub struct WorkbenchAwarenessGadgetProvider<S: WorkbenchService> {
    pub actor: AuthenticatedContext,
    pub service: Arc<S>,
    pub config: gadgetron_core::agent::config::SharedContextConfig,
}

#[async_trait]
impl<S: WorkbenchService> GadgetProvider for WorkbenchAwarenessGadgetProvider<S> {
    fn category(&self) -> &'static str {
        "workbench"
    }

    fn gadget_schemas(&self) -> Vec<GadgetSchema> {
        vec![
            GadgetSchema {
                name: "workbench.activity_recent".to_string(),
                tier: GadgetTier::Read,
                description: "Read recent shared activity entries from the workbench surface. \
                               Returns the newest-first feed of chat turns, direct actions, \
                               and system events visible to the current actor."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 50,
                            "description": "Maximum entries to return. Default 10."
                        }
                    }
                }),
                idempotent: Some(true),
            },
            GadgetSchema {
                name: "workbench.request_evidence".to_string(),
                tier: GadgetTier::Read,
                description: "Retrieve evidence for a specific gateway request: \
                               tool traces, citations, and knowledge candidates."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["request_id"],
                    "properties": {
                        "request_id": {
                            "type": "string",
                            "format": "uuid",
                            "description": "UUID of the gateway request to inspect."
                        }
                    }
                }),
                idempotent: Some(true),
            },
            GadgetSchema {
                name: "workbench.candidates_pending".to_string(),
                tier: GadgetTier::Read,
                description: "List pending knowledge candidates awaiting Penny or user decision. \
                               Returns candidates with unresolved disposition."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 20,
                            "description": "Maximum candidates to return. Default 5."
                        }
                    }
                }),
                idempotent: Some(true),
            },
            GadgetSchema {
                name: "workbench.candidate_decide".to_string(),
                tier: GadgetTier::Write,
                description: "Record a disposition decision (accept / reject / escalate_to_user) \
                               for a knowledge candidate. Accept triggers canonical writeback."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["candidate_id", "decision"],
                    "properties": {
                        "candidate_id": {
                            "type": "string",
                            "format": "uuid",
                            "description": "UUID of the candidate to decide."
                        },
                        "decision": {
                            "type": "string",
                            "enum": ["accept", "reject", "escalate_to_user"],
                            "description": "The disposition decision."
                        },
                        "rationale": {
                            "type": "string",
                            "description": "Optional plain-language rationale stored in the audit trail."
                        }
                    }
                }),
                idempotent: Some(false),
            },
        ]
    }

    async fn call(&self, name: &str, args: Value) -> Result<GadgetResult, GadgetError> {
        match name {
            "workbench.activity_recent" => self.call_activity_recent(&args).await,
            "workbench.request_evidence" => self.call_request_evidence(&args).await,
            "workbench.candidates_pending" => self.call_candidates_pending(&args).await,
            "workbench.candidate_decide" => self.call_candidate_decide(&args).await,
            other => Err(GadgetError::UnknownGadget(other.to_string())),
        }
    }
}

impl<S: WorkbenchService> WorkbenchAwarenessGadgetProvider<S> {
    async fn call_activity_recent(&self, args: &Value) -> Result<GadgetResult, GadgetError> {
        let limit: u32 = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(50) as u32)
            .unwrap_or(10);

        let digests = self
            .service
            .recent_activity_boxed(self.actor, limit)
            .await
            .map_err(|e| GadgetError::Execution(e.to_string()))?;

        Ok(GadgetResult {
            content: serde_json::to_value(&digests)
                .map_err(|e| GadgetError::Execution(format!("serialization failed: {e}")))?,
            is_error: false,
        })
    }

    async fn call_request_evidence(&self, args: &Value) -> Result<GadgetResult, GadgetError> {
        let request_id_str = args
            .get("request_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GadgetError::InvalidArgs("request_id is required".to_string()))?;

        let request_id = Uuid::parse_str(request_id_str)
            .map_err(|_| GadgetError::InvalidArgs(format!("invalid UUID: {request_id_str}")))?;

        let evidence = self
            .service
            .request_evidence_boxed(self.actor, request_id)
            .await
            .map_err(|e| match &e {
                GadgetronError::Knowledge {
                    kind: gadgetron_core::error::KnowledgeErrorKind::DocumentNotFound { .. },
                    ..
                } => GadgetError::InvalidArgs(format!(
                    "request {request_id} not found or not visible to this actor"
                )),
                _ => GadgetError::Execution(e.to_string()),
            })?;

        Ok(GadgetResult {
            content: serde_json::to_value(&evidence)
                .map_err(|e| GadgetError::Execution(format!("serialization failed: {e}")))?,
            is_error: false,
        })
    }

    async fn call_candidates_pending(&self, args: &Value) -> Result<GadgetResult, GadgetError> {
        let limit: u32 = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(20) as u32)
            .unwrap_or(5);

        // KC-1 will replace the stub with a real query.
        let candidates = self
            .service
            .pending_candidates_boxed(self.actor, limit)
            .await
            .map_err(|e| GadgetError::Execution(e.to_string()))?;

        Ok(GadgetResult {
            content: serde_json::to_value(&candidates)
                .map_err(|e| GadgetError::Execution(format!("serialization failed: {e}")))?,
            is_error: false,
        })
    }

    async fn call_candidate_decide(&self, args: &Value) -> Result<GadgetResult, GadgetError> {
        let candidate_id_str = args
            .get("candidate_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GadgetError::InvalidArgs("candidate_id is required".to_string()))?;

        let candidate_id = Uuid::parse_str(candidate_id_str).map_err(|_| {
            GadgetError::InvalidArgs(format!("invalid UUID for candidate_id: {candidate_id_str}"))
        })?;

        let decision_str = args
            .get("decision")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GadgetError::InvalidArgs("decision is required".to_string()))?;

        let decision = match decision_str {
            "accept" => CandidateDecisionKind::Accept,
            "reject" => CandidateDecisionKind::Reject,
            "escalate_to_user" => CandidateDecisionKind::EscalateToUser,
            other => {
                return Err(GadgetError::InvalidArgs(format!(
                    "unknown decision value {other:?}; must be 'accept', 'reject', or 'escalate_to_user'"
                )))
            }
        };

        let rationale = args
            .get("rationale")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let req = PennyCandidateDecisionRequest {
            candidate_id,
            decision,
            rationale,
        };

        // Always Err(Denied) in PSL-1; KC-1 wires the real coordinator.
        self.service
            .decide_candidate_boxed(self.actor, req)
            .await
            .map(|receipt| GadgetResult {
                content: serde_json::to_value(&receipt).unwrap_or(Value::Null),
                is_error: false,
            })
            .map_err(|e| match e {
                GadgetronError::Penny {
                    kind: gadgetron_core::error::PennyErrorKind::ToolDenied { reason },
                    ..
                } => GadgetError::Denied { reason },
                other => GadgetError::Execution(other.to_string()),
            })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::{
        agent::shared_context::PennyCandidateDecisionReceipt,
        error::{GadgetronError, PennyErrorKind},
        workbench::WorkbenchRequestEvidenceResponse,
    };
    use std::sync::Arc;
    use uuid::Uuid;

    // ----------------------------------------------------------------
    // Fake WorkbenchService implementations for testing
    // ----------------------------------------------------------------

    /// A fake that succeeds for activity (empty) and fails everything else.
    struct FakeSuccessService;

    impl WorkbenchService for FakeSuccessService {
        fn recent_activity_boxed(
            &self,
            _actor: AuthenticatedContext,
            _limit: u32,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<gadgetron_core::agent::shared_context::PennyActivityDigest>,
                            GadgetronError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(vec![]) })
        }

        fn request_evidence_boxed(
            &self,
            _actor: AuthenticatedContext,
            request_id: Uuid,
        ) -> std::pin::Pin<
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
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<gadgetron_core::agent::shared_context::PennyCandidateDigest>,
                            GadgetronError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(vec![]) })
        }

        fn decide_candidate_boxed(
            &self,
            _actor: AuthenticatedContext,
            _req: PennyCandidateDecisionRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<PennyCandidateDecisionReceipt, GadgetronError>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Err(GadgetronError::Penny {
                    kind: PennyErrorKind::ToolDenied {
                        reason:
                            "W3-KC-1 pending (see doc 71): candidate decisions are not yet wired"
                                .to_string(),
                    },
                    message: "not wired".to_string(),
                })
            })
        }
    }

    fn make_provider() -> WorkbenchAwarenessGadgetProvider<FakeSuccessService> {
        WorkbenchAwarenessGadgetProvider {
            actor: AuthenticatedContext,
            service: Arc::new(FakeSuccessService),
            config: gadgetron_core::agent::config::SharedContextConfig::default(),
        }
    }

    #[tokio::test]
    async fn activity_recent_dispatches_to_service() {
        let provider = make_provider();
        let result = provider
            .call("workbench.activity_recent", serde_json::json!({"limit": 5}))
            .await;
        assert!(result.is_ok(), "activity_recent should succeed: {result:?}");
        let gadget_result = result.unwrap();
        assert!(!gadget_result.is_error);
    }

    #[tokio::test]
    async fn request_evidence_dispatches_to_service() {
        let provider = make_provider();
        let id = Uuid::new_v4();
        let result = provider
            .call(
                "workbench.request_evidence",
                serde_json::json!({"request_id": id.to_string()}),
            )
            .await;
        assert!(
            result.is_ok(),
            "request_evidence should succeed: {result:?}"
        );
        let gadget_result = result.unwrap();
        assert!(!gadget_result.is_error);
    }

    #[tokio::test]
    async fn candidates_pending_returns_empty_stub_result() {
        let provider = make_provider();
        let result = provider
            .call("workbench.candidates_pending", serde_json::json!({}))
            .await;
        assert!(result.is_ok());
        let gadget_result = result.unwrap();
        assert!(!gadget_result.is_error);
        assert_eq!(gadget_result.content, serde_json::json!([]));
    }

    #[tokio::test]
    async fn candidate_decide_returns_denied_result() {
        let provider = make_provider();
        let result = provider
            .call(
                "workbench.candidate_decide",
                serde_json::json!({
                    "candidate_id": Uuid::new_v4().to_string(),
                    "decision": "accept"
                }),
            )
            .await;
        assert!(result.is_err(), "decide should fail with Denied in PSL-1");
        match result.unwrap_err() {
            GadgetError::Denied { reason } => {
                assert!(
                    reason.contains("W3-KC-1"),
                    "reason must mention W3-KC-1: {reason}"
                );
            }
            other => panic!("expected Denied, got {other:?}"),
        }
    }
}
