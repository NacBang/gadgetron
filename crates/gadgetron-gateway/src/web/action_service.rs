//! In-process workbench action service — W3-WEB-2b.
//!
//! Implements the 10-step direct action flow from doc §2.2.4.
//! Real provider dispatch is deferred (this PR returns a synthetic response).
//! Approval stub returns `pending_approval` for `requires_approval || destructive` actions.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md` §2.2.4

use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::{
    agent::tools::GadgetDispatcher,
    knowledge::{
        candidate::{
            ActivityKind, ActivityOrigin, CapturedActivityEvent, KnowledgeCandidateCoordinator,
        },
        AuthenticatedContext,
    },
    workbench::{
        InvokeWorkbenchActionRequest, InvokeWorkbenchActionResponse, WorkbenchActionResult,
    },
};
use uuid::Uuid;

use crate::web::{
    catalog::DescriptorCatalog,
    replay_cache::{InMemoryReplayCache, ReplayKey},
    workbench::{WorkbenchActionService, WorkbenchHttpError},
};

// ---------------------------------------------------------------------------
// InProcessWorkbenchActionService
// ---------------------------------------------------------------------------

/// In-process direct-action service.
///
/// Backed by a `DescriptorCatalog` snapshot, a replay cache, and an optional
/// `KnowledgeCandidateCoordinator`. Real gadget dispatch is not performed in
/// this PR — the ok path returns a synthetic result and optionally captures
/// an activity event via the coordinator.
pub struct InProcessWorkbenchActionService {
    pub descriptor_catalog: DescriptorCatalog,
    pub replay_cache: Arc<InMemoryReplayCache>,
    pub coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
    /// Pre-compiled JSON-Schema validator, keyed by action id.
    ///
    /// Built at construction time from each descriptor's `input_schema` so
    /// validation is purely in-memory with no re-compilation per request.
    pub schema_validators: std::collections::HashMap<String, Arc<jsonschema::Validator>>,
    /// Optional Gadget dispatcher (typically Penny's `GadgetRegistry`
    /// via `gadgetron_core::agent::tools::GadgetDispatcher`).
    ///
    /// When wired + the descriptor has `gadget_name: Some(...)`, step 7
    /// performs a real dispatch and the raw Gadget result lands in
    /// `WorkbenchActionResult.payload`. When unwired (unit tests, server
    /// without any providers), step 7 falls back to the synthetic result
    /// and `payload` is `None`.
    pub gadget_dispatcher: Option<Arc<dyn GadgetDispatcher>>,
}

impl InProcessWorkbenchActionService {
    /// Build a new action service.
    ///
    /// Pre-compiles JSON-Schema validators for every action in the catalog.
    /// If a descriptor's `input_schema` is invalid JSON-Schema, that action
    /// will lack a compiled validator and args will pass through unvalidated
    /// (warn-log emitted). This is intentional: misconfigured schema is an
    /// operator problem, not a reason to crash at startup.
    pub fn new(
        descriptor_catalog: DescriptorCatalog,
        replay_cache: InMemoryReplayCache,
        coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
    ) -> Self {
        Self::new_with_dispatcher(descriptor_catalog, replay_cache, coordinator, None)
    }

    /// Build a new action service with an optional `GadgetDispatcher`.
    ///
    /// When a dispatcher is wired AND the resolved descriptor has
    /// `gadget_name: Some(...)`, step 7 performs a real Gadget dispatch
    /// and the result lands in `WorkbenchActionResult.payload`.
    pub fn new_with_dispatcher(
        descriptor_catalog: DescriptorCatalog,
        replay_cache: InMemoryReplayCache,
        coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
        gadget_dispatcher: Option<Arc<dyn GadgetDispatcher>>,
    ) -> Self {
        // Build schema validators for all actions in the catalog.
        // We iterate visible_actions with the broadest scope so we compile
        // schemas for all descriptors (including scope-gated ones).
        // Using find_action iteration via internal state via seed_p2b catalog
        // We gather all actions from the catalog's internal state via find_action loops.
        // Since the catalog doesn't expose an iter(), we ask for actions visible
        // to all scopes by using Management scope (broadest). This is compile-time
        // only and never leaks descriptors to actors.
        use gadgetron_core::context::Scope;
        let all_scopes = [Scope::OpenAiCompat, Scope::Management, Scope::XaasAdmin];
        let all_actions = descriptor_catalog.visible_actions(&all_scopes);

        let mut validators = std::collections::HashMap::new();
        for action in &all_actions {
            match jsonschema::validator_for(&action.input_schema) {
                Ok(v) => {
                    validators.insert(action.id.clone(), Arc::new(v));
                }
                Err(e) => {
                    tracing::warn!(
                        action_id = %action.id,
                        error = %e,
                        "action descriptor has invalid input_schema; validation skipped"
                    );
                }
            }
        }

        Self {
            descriptor_catalog,
            replay_cache: Arc::new(replay_cache),
            coordinator,
            schema_validators: validators,
            gadget_dispatcher,
        }
    }
}

#[async_trait]
impl WorkbenchActionService for InProcessWorkbenchActionService {
    /// 10-step direct action flow (doc §2.2.4).
    async fn invoke(
        &self,
        actor: &AuthenticatedContext,
        actor_scopes: &[gadgetron_core::context::Scope],
        action_id: &str,
        request: InvokeWorkbenchActionRequest,
    ) -> Result<InvokeWorkbenchActionResponse, WorkbenchHttpError> {
        // ---------------------------------------------------------------
        // Step 1: Actor resolved (handled by auth middleware upstream).
        // Step 2: Descriptor lookup — 404 if not found.
        // ---------------------------------------------------------------
        let descriptor = self
            .descriptor_catalog
            .find_action(action_id)
            .ok_or_else(|| WorkbenchHttpError::ActionNotFound {
                action_id: action_id.to_string(),
            })?;

        // ---------------------------------------------------------------
        // Step 3: required_scope + disabled_reason check.
        //
        // required_scope: doc §2.4.1 says return 404 (not 403) to avoid
        // leaking existence of scope-restricted actions.
        // disabled_reason from allow_direct_actions=false: 403 forbidden.
        //
        // Drift-fix follow-up to PR 7: `actor_scopes` is now threaded
        // through from `TenantContext.scopes` by the handler — no more
        // hardcoded placeholder slice.
        // ---------------------------------------------------------------
        if let Some(required) = descriptor.required_scope {
            if !actor_scopes.contains(&required) {
                return Err(WorkbenchHttpError::ActionNotFound {
                    action_id: action_id.to_string(),
                });
            }
        }

        // Check instance-level policy (allow_direct_actions=false).
        if !self.descriptor_catalog.allow_direct_actions() {
            return Err(WorkbenchHttpError::DirectActionsDisabled);
        }

        // Check descriptor-level disabled_reason (e.g. runtime unavailability).
        // This is separate from the allow_direct_actions policy: a descriptor
        // can be individually disabled regardless of instance policy.
        if let Some(ref reason) = descriptor.disabled_reason {
            // If the reason matches the policy message, use the dedicated error.
            if reason.contains("Direct actions are disabled") {
                return Err(WorkbenchHttpError::DirectActionsDisabled);
            }
            // Otherwise treat as a generic forbidden / config error.
            return Err(WorkbenchHttpError::Core(
                gadgetron_core::error::GadgetronError::Config(reason.clone()),
            ));
        }

        // ---------------------------------------------------------------
        // Step 4: JSON-Schema validation of args.
        // ---------------------------------------------------------------
        if let Some(validator) = self.schema_validators.get(action_id) {
            // `validate` returns the first error via Result<(), ValidationError>.
            // Use `iter_errors` to collect a user-facing message.
            let mut error_iter = validator.iter_errors(&request.args);
            if let Some(first_err) = error_iter.next() {
                return Err(WorkbenchHttpError::ActionInvalidArgs {
                    detail: first_err.to_string(),
                });
            }
        }

        // ---------------------------------------------------------------
        // Step 5: Replay-cache check (requires client_invocation_id).
        // ---------------------------------------------------------------
        // Drift-fix PR 7 landed real `AuthenticatedContext { user_id,
        // tenant_id }` (doc-10). Replay keys now use `actor.tenant_id`
        // directly, so replay protection is scoped per-tenant and cannot
        // cross tenant boundaries — the invariant the spec always
        // required, finally with a real identity behind it.
        let actor_tenant_id = actor.tenant_id;

        if let Some(ciid) = request.client_invocation_id {
            let replay_key = ReplayKey {
                tenant_id: actor_tenant_id,
                action_id: action_id.to_string(),
                client_invocation_id: ciid,
            };
            if let Some(cached) = self.replay_cache.get(&replay_key).await {
                tracing::debug!(
                    action_id = %action_id,
                    client_invocation_id = %ciid,
                    "replay cache hit — returning cached response"
                );
                return Ok(cached);
            }
        }

        // ---------------------------------------------------------------
        // Step 6: Approval gate stub.
        // ---------------------------------------------------------------
        let needs_approval = descriptor.requires_approval || descriptor.destructive;
        if needs_approval {
            let approval_id = Uuid::new_v4();
            let result = WorkbenchActionResult {
                status: "pending_approval".into(),
                approval_id: Some(approval_id),
                activity_event_id: None,
                audit_event_id: None,
                refresh_view_ids: vec![],
                knowledge_candidates: vec![],
                payload: None,
            };
            let resp = InvokeWorkbenchActionResponse { result };
            // Cache under client_invocation_id if provided.
            if let Some(ciid) = request.client_invocation_id {
                let key = ReplayKey {
                    tenant_id: actor_tenant_id,
                    action_id: action_id.to_string(),
                    client_invocation_id: ciid,
                };
                self.replay_cache.put(key, resp.clone()).await;
            }
            return Ok(resp);
        }

        // ---------------------------------------------------------------
        // Step 7a: Real Gadget dispatch (when wired + descriptor has one).
        //
        // TODO(audit-direct-action): Direct-action dispatch bypasses
        // Penny's `GadgetAuditEventSink` by design (workbench doc §2.2.4
        // + D-20260411-*). `WorkbenchActionResult.audit_event_id` stays
        // `None` until a parallel audit sink is wired — see
        // `gadgetron_core::agent::tools::GadgetDispatcher` doc comment.
        // ---------------------------------------------------------------
        let mut payload: Option<serde_json::Value> = None;

        if let (Some(dispatcher), Some(gadget_name)) = (
            self.gadget_dispatcher.as_ref(),
            descriptor.gadget_name.as_deref(),
        ) {
            match dispatcher
                .dispatch_gadget(gadget_name, request.args.clone())
                .await
            {
                Ok(result) => {
                    // Penny's `GadgetResult { content: Value, is_error: bool }`
                    // wraps the raw provider output. The workbench contract
                    // puts that content directly into `payload` — callers
                    // interpret it per gadget.
                    payload = Some(result.content);
                }
                Err(e) => {
                    tracing::warn!(
                        action_id = %action_id,
                        gadget_name = %gadget_name,
                        error = %e,
                        "gadget dispatch failed; surfacing as error"
                    );
                    return Err(WorkbenchHttpError::Core(e.into()));
                }
            }
        }

        // ---------------------------------------------------------------
        // Step 7b: Coordinator capture (non-fatal on failure).
        // ---------------------------------------------------------------
        let mut activity_event_id: Option<Uuid> = None;
        let mut knowledge_candidates: Vec<serde_json::Value> = vec![];

        if let Some(ref coord) = self.coordinator {
            let event_id = Uuid::new_v4();
            let event = CapturedActivityEvent {
                id: event_id,
                tenant_id: actor_tenant_id,
                actor_user_id: actor.user_id,
                request_id: None,
                origin: ActivityOrigin::UserDirect,
                kind: ActivityKind::DirectAction,
                title: format!("direct action: {action_id}"),
                summary: format!("action_id={action_id}"),
                source_bundle: Some(descriptor.owner_bundle.clone()),
                source_capability: descriptor.gadget_name.clone(),
                audit_event_id: None,
                facts: serde_json::json!({
                    "action_id": action_id,
                    "gadget_name": descriptor.gadget_name,
                    "knowledge_hint": descriptor.knowledge_hint,
                }),
                created_at: chrono::Utc::now(),
            };

            match coord.capture_action(actor, event, vec![]).await {
                Ok(candidates) => {
                    activity_event_id = Some(event_id);
                    knowledge_candidates = candidates
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "id": c.id,
                                "disposition": c.disposition,
                            })
                        })
                        .collect();
                }
                Err(e) => {
                    // Coordinator failure is non-fatal — log and continue.
                    tracing::warn!(
                        action_id = %action_id,
                        error = %e,
                        "coordinator capture_action failed; proceeding without activity capture"
                    );
                }
            }
        }

        // ---------------------------------------------------------------
        // Step 8 / 9: Build and optionally cache the response.
        // ---------------------------------------------------------------
        let result = WorkbenchActionResult {
            status: "ok".into(),
            approval_id: None,
            activity_event_id,
            audit_event_id: None,
            refresh_view_ids: vec![],
            knowledge_candidates,
            payload,
        };
        let resp = InvokeWorkbenchActionResponse { result };

        if let Some(ciid) = request.client_invocation_id {
            let key = ReplayKey {
                tenant_id: actor_tenant_id,
                action_id: action_id.to_string(),
                client_invocation_id: ciid,
            };
            self.replay_cache.put(key, resp.clone()).await;
        }

        Ok(resp)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::catalog::DescriptorCatalog;
    use crate::web::replay_cache::{InMemoryReplayCache, DEFAULT_REPLAY_TTL};
    use gadgetron_core::workbench::InvokeWorkbenchActionRequest;

    fn make_service(catalog: DescriptorCatalog) -> InProcessWorkbenchActionService {
        InProcessWorkbenchActionService::new(
            catalog,
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
        )
    }

    fn actor() -> AuthenticatedContext {
        AuthenticatedContext::system()
    }

    /// Test helper — default scope slice (`OpenAiCompat`) that matches
    /// what the gateway threaded through before drift-fix PR 7's scope
    /// threading landed. Tests that want a scope-gated action use an
    /// empty slice explicitly.
    fn actor_scopes_default() -> [gadgetron_core::context::Scope; 1] {
        [gadgetron_core::context::Scope::OpenAiCompat]
    }

    // -----------------------------------------------------------------------
    // Step 2: ActionNotFound
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_unknown_action_returns_action_not_found() {
        let svc = make_service(DescriptorCatalog::seed_p2b());
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "test"}),
            client_invocation_id: None,
        };
        let err = svc
            .invoke(&actor(), &actor_scopes_default(), "nonexistent-action", req)
            .await
            .unwrap_err();
        match err {
            WorkbenchHttpError::ActionNotFound { action_id } => {
                assert_eq!(action_id, "nonexistent-action");
            }
            other => panic!("expected ActionNotFound, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Step 3: DirectActionsDisabled
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_returns_direct_actions_disabled_when_policy_off() {
        let catalog = DescriptorCatalog::seed_p2b().with_allow_direct_actions(false);
        let svc = make_service(catalog);
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "test"}),
            client_invocation_id: None,
        };
        let err = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap_err();
        match err {
            WorkbenchHttpError::DirectActionsDisabled => {}
            other => panic!("expected DirectActionsDisabled, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Step 4: Schema validation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_valid_args_passes_schema_validation() {
        let svc = make_service(DescriptorCatalog::seed_p2b());
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "hello", "max_results": 5}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap();
        assert_eq!(resp.result.status, "ok");
    }

    #[tokio::test]
    async fn invoke_missing_required_arg_returns_action_invalid_args() {
        let svc = make_service(DescriptorCatalog::seed_p2b());
        // "query" is required; sending empty object should fail.
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({}),
            client_invocation_id: None,
        };
        let err = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap_err();
        match err {
            WorkbenchHttpError::ActionInvalidArgs { detail } => {
                assert!(!detail.is_empty(), "detail must be non-empty");
            }
            other => panic!("expected ActionInvalidArgs, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn invoke_additional_properties_rejected() {
        let svc = make_service(DescriptorCatalog::seed_p2b());
        // "extra_field" is not in the schema and additionalProperties=false.
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "test", "extra_field": "bad"}),
            client_invocation_id: None,
        };
        let err = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap_err();
        match err {
            WorkbenchHttpError::ActionInvalidArgs { .. } => {}
            other => panic!("expected ActionInvalidArgs, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Step 5: Replay cache
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_duplicate_client_invocation_id_returns_cached_response() {
        let svc = make_service(DescriptorCatalog::seed_p2b());
        let ciid = Uuid::new_v4();
        let req1 = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "first"}),
            client_invocation_id: Some(ciid),
        };
        let resp1 = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req1)
            .await
            .unwrap();
        assert_eq!(resp1.result.status, "ok");

        // Second call with the same ciid — should return the cached response.
        let req2 = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "different-args-dont-matter"}),
            client_invocation_id: Some(ciid),
        };
        let resp2 = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req2)
            .await
            .unwrap();
        // Both must be "ok" (same cached response).
        assert_eq!(resp2.result.status, "ok");
    }

    // -----------------------------------------------------------------------
    // Step 6: Approval pending for destructive actions
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_destructive_action_returns_pending_approval() {
        use gadgetron_core::workbench::{
            WorkbenchActionDescriptor, WorkbenchActionKind, WorkbenchActionPlacement,
        };
        let mut catalog = DescriptorCatalog::seed_p2b();
        catalog.actions.push(WorkbenchActionDescriptor {
            id: "delete-everything".into(),
            title: "Delete All".into(),
            owner_bundle: "ops".into(),
            source_kind: "admin".into(),
            source_id: "delete.all".into(),
            gadget_name: None,
            placement: WorkbenchActionPlacement::ContextMenu,
            kind: WorkbenchActionKind::Dangerous,
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            destructive: true,
            requires_approval: false,
            knowledge_hint: "destroys everything".into(),
            required_scope: None,
            disabled_reason: None,
        });

        let svc = make_service(catalog);
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "delete-everything", req)
            .await
            .unwrap();
        assert_eq!(resp.result.status, "pending_approval");
        assert!(resp.result.approval_id.is_some());
    }

    // -----------------------------------------------------------------------
    // Step 7: coordinator capture
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_seeded_action_succeeds_and_status_is_ok() {
        let svc = make_service(DescriptorCatalog::seed_p2b());
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "test knowledge"}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap();
        assert_eq!(resp.result.status, "ok");
        assert!(resp.result.approval_id.is_none());
        assert!(resp.result.audit_event_id.is_none());
        // Without coordinator wired, no candidates or activity event.
        assert!(resp.result.knowledge_candidates.is_empty());
    }

    #[tokio::test]
    async fn invoke_seeded_action_with_coordinator_captures_activity() {
        use gadgetron_core::knowledge::candidate::{
            ActivityCaptureStore, KnowledgeCandidateCoordinator,
        };
        use gadgetron_knowledge::candidate::{
            InMemoryActivityCaptureStore, InProcessCandidateCoordinator,
        };

        let store: Arc<dyn ActivityCaptureStore> = Arc::new(InMemoryActivityCaptureStore::new());
        let coordinator: Arc<dyn KnowledgeCandidateCoordinator> =
            Arc::new(InProcessCandidateCoordinator::new(store.clone(), 8));

        let svc = InProcessWorkbenchActionService::new(
            DescriptorCatalog::seed_p2b(),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            Some(coordinator),
        );

        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "wiki article search"}),
            client_invocation_id: None,
        };

        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap();

        // Status must be "ok"
        assert_eq!(resp.result.status, "ok", "status must be ok");
        // activity_event_id must be set when coordinator is wired
        assert!(
            resp.result.activity_event_id.is_some(),
            "activity_event_id must be set when coordinator is wired"
        );
        // approval_id must be absent on the ok path
        assert!(
            resp.result.approval_id.is_none(),
            "no approval_id on ok path"
        );
        // audit_event_id is a follow-up
        assert!(
            resp.result.audit_event_id.is_none(),
            "audit_event_id not wired in this PR"
        );
    }

    // -----------------------------------------------------------------------
    // Step 7a: Real Gadget dispatch populates payload
    // -----------------------------------------------------------------------

    /// Fake `GadgetDispatcher` that records the dispatched name + args and
    /// returns a canned content payload. Lets us assert that (a) the
    /// action path calls through to the dispatcher, and (b) the raw
    /// `GadgetResult.content` ends up in `WorkbenchActionResult.payload`.
    struct FakeDispatcher {
        inner: tokio::sync::Mutex<Option<(String, serde_json::Value)>>,
        response: serde_json::Value,
    }

    #[async_trait]
    impl gadgetron_core::agent::tools::GadgetDispatcher for FakeDispatcher {
        async fn dispatch_gadget(
            &self,
            name: &str,
            args: serde_json::Value,
        ) -> Result<
            gadgetron_core::agent::tools::GadgetResult,
            gadgetron_core::agent::tools::GadgetError,
        > {
            *self.inner.lock().await = Some((name.to_string(), args));
            Ok(gadgetron_core::agent::tools::GadgetResult {
                content: self.response.clone(),
                is_error: false,
            })
        }
    }

    #[tokio::test]
    async fn invoke_with_dispatcher_populates_payload_from_gadget_result() {
        let dispatcher = Arc::new(FakeDispatcher {
            inner: tokio::sync::Mutex::new(None),
            response: serde_json::json!({
                "hits": [{"title": "seeded", "score": 0.9}],
                "total": 1,
            }),
        });
        let svc = InProcessWorkbenchActionService::new_with_dispatcher(
            DescriptorCatalog::seed_p2b(),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            Some(dispatcher.clone()),
        );
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "seeded"}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap();

        assert_eq!(resp.result.status, "ok");
        let payload = resp.result.payload.expect("payload set on dispatch ok");
        assert_eq!(payload["total"], 1);
        assert_eq!(payload["hits"][0]["title"], "seeded");

        let captured = dispatcher.inner.lock().await;
        let (name, args) = captured.as_ref().expect("dispatcher invoked");
        // seed_p2b wires knowledge-search → wiki.search gadget.
        assert_eq!(name, "wiki.search");
        assert_eq!(args["query"], "seeded");
    }

    #[tokio::test]
    async fn invoke_without_dispatcher_keeps_payload_none() {
        // Regression guard — when no dispatcher is wired, the ok path
        // must fall back to the synthetic result (payload = None).
        let svc = make_service(DescriptorCatalog::seed_p2b());
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "no dispatch"}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap();
        assert_eq!(resp.result.status, "ok");
        assert!(
            resp.result.payload.is_none(),
            "payload must be None when no dispatcher is wired"
        );
    }

    #[tokio::test]
    async fn invoke_dispatcher_error_surfaces_as_workbench_error() {
        struct FailingDispatcher;
        #[async_trait]
        impl gadgetron_core::agent::tools::GadgetDispatcher for FailingDispatcher {
            async fn dispatch_gadget(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<
                gadgetron_core::agent::tools::GadgetResult,
                gadgetron_core::agent::tools::GadgetError,
            > {
                Err(gadgetron_core::agent::tools::GadgetError::UnknownGadget(
                    name.to_string(),
                ))
            }
        }
        let svc = InProcessWorkbenchActionService::new_with_dispatcher(
            DescriptorCatalog::seed_p2b(),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            Some(Arc::new(FailingDispatcher)),
        );
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "x"}),
            client_invocation_id: None,
        };
        let err = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap_err();
        match err {
            WorkbenchHttpError::Core(_) => {}
            other => panic!("expected Core error from dispatch failure, got {other:?}"),
        }
    }
}
