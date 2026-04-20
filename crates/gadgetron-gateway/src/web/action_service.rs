//! In-process workbench action service — W3-WEB-2b.
//!
//! Implements the 10-step direct action flow from doc §2.2.4.
//! Real provider dispatch is deferred (this PR returns a synthetic response).
//! Approval stub returns `pending_approval` for `requires_approval || destructive` actions.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md` §2.2.4

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use gadgetron_core::{
    agent::tools::GadgetDispatcher,
    audit::{ActionAuditEvent, ActionAuditOutcome, ActionAuditSink},
    knowledge::{
        candidate::{
            ActivityKind, ActivityOrigin, CapturedActivityEvent, KnowledgeCandidateCoordinator,
        },
        AuthenticatedContext,
    },
    workbench::{
        ApprovalRequest, ApprovalStore, InvokeWorkbenchActionRequest,
        InvokeWorkbenchActionResponse, WorkbenchActionResult,
    },
};
use uuid::Uuid;

use crate::web::{
    catalog::CatalogSnapshot,
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
    /// Shared catalog snapshot — bundles the `DescriptorCatalog` and
    /// its pre-compiled validators, atomically swappable via ISSUE 8
    /// TASK 8.3. Every `invoke` loads the current snapshot once so
    /// catalog reads and validator lookups see a consistent view — a
    /// concurrent reload cannot land a new catalog with stale
    /// validators or vice versa.
    pub descriptor_catalog: Arc<ArcSwap<CatalogSnapshot>>,
    pub replay_cache: Arc<InMemoryReplayCache>,
    pub coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
    /// Optional Gadget dispatcher (typically Penny's `GadgetRegistry`
    /// via `gadgetron_core::agent::tools::GadgetDispatcher`).
    ///
    /// When wired + the descriptor has `gadget_name: Some(...)`, step 7
    /// performs a real dispatch and the raw Gadget result lands in
    /// `WorkbenchActionResult.payload`. When unwired (unit tests, server
    /// without any providers), step 7 falls back to the synthetic result
    /// and `payload` is `None`.
    pub gadget_dispatcher: Option<Arc<dyn GadgetDispatcher>>,
    /// Audit sink for direct-action dispatch events. When wired, the
    /// service emits one `ActionAuditEvent::DirectActionCompleted` per
    /// invocation (success, error, or pending_approval) and echoes the
    /// event's UUID back as `WorkbenchActionResult.audit_event_id`.
    /// `NoopActionAuditSink` is the default when persistence isn't
    /// configured.
    pub audit_sink: Arc<dyn ActionAuditSink>,
    /// Optional approval store. When present, step 6 persists a real
    /// `ApprovalRequest` instead of returning a bare
    /// `Uuid::new_v4()` — the approve endpoint can then look up the
    /// record and resume dispatch.
    pub approval_store: Option<Arc<dyn ApprovalStore>>,
    /// Optional Postgres pool for the ISSUE 12 billing ledger. When
    /// present, successful direct-action + approved-action dispatches
    /// emit a `billing_events` row (kind=Action, source_event_id =
    /// audit_event_id). When `None` (tests, no-DB deploys), billing is
    /// a no-op — audit still fires through `audit_sink`.
    pub pg_pool: Option<sqlx::PgPool>,
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
        descriptor_catalog: Arc<ArcSwap<CatalogSnapshot>>,
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
        descriptor_catalog: Arc<ArcSwap<CatalogSnapshot>>,
        replay_cache: InMemoryReplayCache,
        coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
        gadget_dispatcher: Option<Arc<dyn GadgetDispatcher>>,
    ) -> Self {
        Self::new_full(
            descriptor_catalog,
            replay_cache,
            coordinator,
            gadget_dispatcher,
            Arc::new(gadgetron_core::audit::NoopActionAuditSink),
            None,
        )
    }

    /// Build the full-configured service: dispatcher + audit sink +
    /// approval store. This is the production constructor. Thin
    /// convenience wrappers above default the sink to
    /// `NoopActionAuditSink` and the approval store to `None`.
    pub fn new_full(
        descriptor_catalog: Arc<ArcSwap<CatalogSnapshot>>,
        replay_cache: InMemoryReplayCache,
        coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
        gadget_dispatcher: Option<Arc<dyn GadgetDispatcher>>,
        audit_sink: Arc<dyn ActionAuditSink>,
        approval_store: Option<Arc<dyn ApprovalStore>>,
    ) -> Self {
        // Validators live inside `CatalogSnapshot.validators` — see
        // `DescriptorCatalog::into_snapshot`. Reload atomically swaps
        // both together so we never observe a new catalog against
        // stale validators.
        Self {
            descriptor_catalog,
            replay_cache: Arc::new(replay_cache),
            coordinator,
            gadget_dispatcher,
            audit_sink,
            approval_store,
            pg_pool: None,
        }
    }

    /// Attach a Postgres pool for billing-ledger writes (ISSUE 12 TASK 12.2).
    /// Builder chain so existing `new_full` callers don't break.
    pub fn with_pg_pool(mut self, pool: sqlx::PgPool) -> Self {
        self.pg_pool = Some(pool);
        self
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
        //
        // Load the catalog snapshot once at the top so the rest of the
        // 10-step flow reads a consistent view; a concurrent reload
        // (ISSUE 8 TASK 8.2/8.3) cannot half-swap underneath us, and
        // validator lookups share the same snapshot so we can't
        // validate against a schema the caller's catalog doesn't know.
        // ---------------------------------------------------------------
        let snapshot = self.descriptor_catalog.load();
        let catalog = &snapshot.catalog;
        let descriptor =
            catalog
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
        if !catalog.allow_direct_actions() {
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
        if let Some(validator) = snapshot.validators.get(action_id) {
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

        // Start the audit clock early so step 6 (pending_approval) also
        // reports an elapsed_ms. The sink receives fire-and-forget events
        // at every completion boundary so operators can reconstruct the
        // full invocation timeline later.
        let start_instant = std::time::Instant::now();

        // ---------------------------------------------------------------
        // Step 6: Approval gate.
        //
        // When `approval_store` is wired (production), we persist an
        // `ApprovalRequest` so the `POST /approvals/:id/approve`
        // endpoint can later look it up + resume dispatch. When the
        // store is absent (unit tests, minimal composition), we fall
        // back to a bare `Uuid::new_v4()` — the approval id is still
        // returned but resume is not possible.
        // ---------------------------------------------------------------
        let needs_approval = descriptor.requires_approval || descriptor.destructive;
        if needs_approval {
            let approval_id = Uuid::new_v4();
            if let Some(store) = &self.approval_store {
                let record = ApprovalRequest::new_pending(
                    approval_id,
                    actor,
                    action_id,
                    descriptor.gadget_name.clone(),
                    request.args.clone(),
                );
                if let Err(e) = store.create(record).await {
                    tracing::error!(
                        action_id = %action_id,
                        error = %e,
                        "approval_store.create failed; returning 500 equivalent",
                    );
                    return Err(WorkbenchHttpError::Core(
                        gadgetron_core::error::GadgetronError::Config(format!(
                            "approval store failure: {e}"
                        )),
                    ));
                }
            }
            // Emit audit event for the pending-approval path so the
            // approval queue has a stable id to attach subsequent
            // `ApprovalGranted` / `ApprovalDenied` events to. Echoed
            // into the response.
            let audit_event_id = Uuid::new_v4();
            self.audit_sink
                .send(ActionAuditEvent::DirectActionCompleted {
                    event_id: audit_event_id,
                    action_id: action_id.to_string(),
                    gadget_name: descriptor.gadget_name.clone(),
                    actor_user_id: actor.user_id,
                    tenant_id: actor_tenant_id,
                    outcome: ActionAuditOutcome::PendingApproval,
                    elapsed_ms: start_instant.elapsed().as_millis() as u64,
                });
            let result = WorkbenchActionResult {
                status: "pending_approval".into(),
                approval_id: Some(approval_id),
                activity_event_id: None,
                audit_event_id: Some(audit_event_id),
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
        // Audit: each completion (Ok / Err) emits an
        // `ActionAuditEvent::DirectActionCompleted` with a fresh
        // `event_id`. The same UUID is echoed back as
        // `WorkbenchActionResult.audit_event_id` so callers can look up
        // the persisted row.
        // ---------------------------------------------------------------
        let mut payload: Option<serde_json::Value> = None;
        // Pre-generate the audit event id so it lands in the response
        // and the emitted event with byte-identical value.
        let audit_event_id = Uuid::new_v4();

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
                    // Emit error-outcome audit event before surfacing
                    // the HTTP error — the caller gets a non-200 with
                    // no response body to correlate from, so the audit
                    // trail is the only record of what happened.
                    self.audit_sink
                        .send(ActionAuditEvent::DirectActionCompleted {
                            event_id: audit_event_id,
                            action_id: action_id.to_string(),
                            gadget_name: Some(gadget_name.to_string()),
                            actor_user_id: actor.user_id,
                            tenant_id: actor_tenant_id,
                            outcome: ActionAuditOutcome::Error {
                                error_code: e.error_code().to_string(),
                            },
                            elapsed_ms: start_instant.elapsed().as_millis() as u64,
                        });
                    tracing::warn!(
                        action_id = %action_id,
                        gadget_name = %gadget_name,
                        error = %e,
                        audit_event_id = %audit_event_id,
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
        // Step 8 / 9: Emit success audit + build and optionally cache
        // the response.
        //
        // `audit_event_id` is the pre-generated UUID from Step 7a. The
        // response carries it so callers can correlate with the audit
        // log. The event is emitted here rather than in 7a because a
        // successful dispatch should only be audited once the full
        // workbench response is committed (post-coordinator, past any
        // return-early branches).
        // ---------------------------------------------------------------
        self.audit_sink
            .send(ActionAuditEvent::DirectActionCompleted {
                event_id: audit_event_id,
                action_id: action_id.to_string(),
                gadget_name: descriptor.gadget_name.clone(),
                actor_user_id: actor.user_id,
                tenant_id: actor_tenant_id,
                outcome: ActionAuditOutcome::Success,
                elapsed_ms: start_instant.elapsed().as_millis() as u64,
            });
        emit_action_billing(
            self.pg_pool.as_ref(),
            actor_tenant_id,
            audit_event_id,
            descriptor.gadget_name.clone(),
        );
        let result = WorkbenchActionResult {
            status: "ok".into(),
            approval_id: None,
            activity_event_id,
            audit_event_id: Some(audit_event_id),
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

    /// Resume a previously-approved request. Dispatches the gadget
    /// with the args stored at invoke time, emits the appropriate
    /// audit event, and returns the ok / error response.
    ///
    /// Steps 1-6 of the invoke flow are NOT re-run — they already ran
    /// at the original invoke (scope gate, schema validation,
    /// pending_approval persistence). The approval having been marked
    /// `Approved` by the approval endpoint is the license to dispatch;
    /// this method is the seam where that license gets redeemed.
    async fn resume_approval(
        &self,
        actor: &AuthenticatedContext,
        _actor_scopes: &[gadgetron_core::context::Scope],
        approval: ApprovalRequest,
    ) -> Result<InvokeWorkbenchActionResponse, WorkbenchHttpError> {
        let snapshot = self.descriptor_catalog.load();
        let descriptor = snapshot
            .catalog
            .find_action(&approval.action_id)
            .ok_or_else(|| WorkbenchHttpError::ActionNotFound {
                action_id: approval.action_id.clone(),
            })?;
        let start_instant = std::time::Instant::now();
        let audit_event_id = Uuid::new_v4();
        let mut payload: Option<serde_json::Value> = None;
        if let (Some(dispatcher), Some(gadget_name)) = (
            self.gadget_dispatcher.as_ref(),
            descriptor.gadget_name.as_deref(),
        ) {
            match dispatcher
                .dispatch_gadget(gadget_name, approval.args.clone())
                .await
            {
                Ok(result) => {
                    payload = Some(result.content);
                }
                Err(e) => {
                    self.audit_sink
                        .send(ActionAuditEvent::DirectActionCompleted {
                            event_id: audit_event_id,
                            action_id: approval.action_id.clone(),
                            gadget_name: Some(gadget_name.to_string()),
                            actor_user_id: actor.user_id,
                            tenant_id: actor.tenant_id,
                            outcome: ActionAuditOutcome::Error {
                                error_code: e.error_code().to_string(),
                            },
                            elapsed_ms: start_instant.elapsed().as_millis() as u64,
                        });
                    tracing::warn!(
                        action_id = %approval.action_id,
                        gadget_name = %gadget_name,
                        approval_id = %approval.id,
                        error = %e,
                        audit_event_id = %audit_event_id,
                        "approved-dispatch failed; surfacing as error",
                    );
                    return Err(WorkbenchHttpError::Core(e.into()));
                }
            }
        }
        self.audit_sink
            .send(ActionAuditEvent::DirectActionCompleted {
                event_id: audit_event_id,
                action_id: approval.action_id.clone(),
                gadget_name: descriptor.gadget_name.clone(),
                actor_user_id: actor.user_id,
                tenant_id: actor.tenant_id,
                outcome: ActionAuditOutcome::Success,
                elapsed_ms: start_instant.elapsed().as_millis() as u64,
            });
        emit_action_billing(
            self.pg_pool.as_ref(),
            actor.tenant_id,
            audit_event_id,
            descriptor.gadget_name.clone(),
        );
        let result = WorkbenchActionResult {
            status: "ok".into(),
            approval_id: Some(approval.id),
            activity_event_id: None,
            audit_event_id: Some(audit_event_id),
            refresh_view_ids: vec![],
            knowledge_candidates: vec![],
            payload,
        };
        Ok(InvokeWorkbenchActionResponse { result })
    }
}

/// Fire-and-forget billing emission for a successful action dispatch
/// (ISSUE 12 TASK 12.2). `source_event_id` is the audit event UUID so
/// the ledger joins cleanly to `action_audit_events`. `cost_cents=0`
/// today — dispatcher doesn't surface cost yet; invoice materializer
/// (TASK 12.3) applies per-kind pricing at query time. No-op when
/// `pool` is `None` (test / no-DB deploys).
///
/// **`actor_user_id` is always NULL here, intentionally.** ISSUE 23's
/// security review flipped this from `Some(actor.user_id)` to `None`
/// because `AuthenticatedContext.user_id` at the workbench layer is
/// sourced from `ctx.api_key_id` (see `workbench.rs:~509/554/1433`),
/// so writing `Some(actor.user_id)` would contaminate
/// `billing_events.actor_user_id` with api_key_ids indistinguishable
/// from real users.id at query time. ISSUE 24 adds a real `user_id`
/// field to `AuthenticatedContext` and will reintroduce an
/// `actor_user_id` parameter here sourced from `actor.real_user_id`.
/// Until then, both call sites get the typed-constructor's default
/// NULL via `BillingEventInsert::action(..)` with no `.with_actor_user`.
fn emit_action_billing(
    pool: Option<&sqlx::PgPool>,
    tenant_id: Uuid,
    audit_event_id: Uuid,
    gadget_name: Option<String>,
) {
    let Some(pool) = pool else {
        return;
    };
    let pool = pool.clone();
    tokio::spawn(async move {
        if let Err(e) = gadgetron_xaas::billing::insert_billing_event(
            &pool,
            gadgetron_xaas::billing::BillingEventInsert::action(
                tenant_id,
                audit_event_id,
                gadget_name,
            ),
        )
        .await
        {
            tracing::warn!(
                target: "billing",
                tenant_id = %tenant_id,
                audit_event_id = %audit_event_id,
                error = %e,
                "failed to persist action billing_events row"
            );
        }
    });
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
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(catalog.into_snapshot())),
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
        // audit_event_id is now populated on every ok path (TASK 3.1).
        assert!(
            resp.result.audit_event_id.is_some(),
            "audit_event_id must be set on ok"
        );
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
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
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
        // audit_event_id is populated on every ok path (TASK 3.1).
        assert!(
            resp.result.audit_event_id.is_some(),
            "audit_event_id must be set on ok"
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
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
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
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
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

    // -----------------------------------------------------------------------
    // ISSUE 3 / TASK 3.1 — action audit sink integration
    // -----------------------------------------------------------------------

    /// Capturing sink — collects every event the action service emits so
    /// the tests can assert shape + id correlation. Uses a sync std
    /// mutex because `ActionAuditSink::send` is sync and the push is a
    /// bounded-time Vec operation.
    #[derive(Debug, Default)]
    struct CapturingAuditSink {
        events: std::sync::Mutex<Vec<gadgetron_core::audit::ActionAuditEvent>>,
    }

    impl gadgetron_core::audit::ActionAuditSink for CapturingAuditSink {
        fn send(&self, event: gadgetron_core::audit::ActionAuditEvent) {
            self.events
                .lock()
                .expect("capturing sink mutex poisoned")
                .push(event);
        }
    }

    /// Helper: clone the captured events.
    fn drained_events(
        sink: &Arc<CapturingAuditSink>,
    ) -> Vec<gadgetron_core::audit::ActionAuditEvent> {
        sink.events
            .lock()
            .expect("capturing sink mutex poisoned")
            .clone()
    }

    #[tokio::test]
    async fn invoke_ok_path_emits_audit_event_matching_response_id() {
        let sink = Arc::new(CapturingAuditSink::default());
        let svc = InProcessWorkbenchActionService::new_full(
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            None,
            sink.clone() as Arc<dyn gadgetron_core::audit::ActionAuditSink>,
            None,
        );
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "audit-ok"}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap();
        let response_audit_id = resp.result.audit_event_id.expect("audit_event_id set");

        let events = drained_events(&sink);
        assert_eq!(
            events.len(),
            1,
            "exactly one audit event emitted on the ok path"
        );
        match &events[0] {
            gadgetron_core::audit::ActionAuditEvent::DirectActionCompleted {
                event_id,
                action_id,
                outcome,
                ..
            } => {
                assert_eq!(*event_id, response_audit_id, "event id matches response");
                assert_eq!(action_id, "knowledge-search");
                assert!(matches!(
                    outcome,
                    gadgetron_core::audit::ActionAuditOutcome::Success
                ));
            }
            // ActionAuditEvent is #[non_exhaustive]; approval-flow
            // variants land in a future TASK.
            _ => unreachable!("only DirectActionCompleted shipping today"),
        }
    }

    #[tokio::test]
    async fn invoke_pending_approval_emits_audit_event() {
        use gadgetron_core::workbench::{
            WorkbenchActionDescriptor, WorkbenchActionKind, WorkbenchActionPlacement,
        };
        let mut catalog = DescriptorCatalog::seed_p2b();
        catalog.actions.push(WorkbenchActionDescriptor {
            id: "delete-it".into(),
            title: "Delete".into(),
            owner_bundle: "ops".into(),
            source_kind: "admin".into(),
            source_id: "delete.it".into(),
            gadget_name: None,
            placement: WorkbenchActionPlacement::ContextMenu,
            kind: WorkbenchActionKind::Dangerous,
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            destructive: true,
            requires_approval: false,
            knowledge_hint: "destructive".into(),
            required_scope: None,
            disabled_reason: None,
        });
        let sink = Arc::new(CapturingAuditSink::default());
        let svc = InProcessWorkbenchActionService::new_full(
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(catalog.into_snapshot())),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            None,
            sink.clone() as Arc<dyn gadgetron_core::audit::ActionAuditSink>,
            None,
        );
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "delete-it", req)
            .await
            .unwrap();
        assert_eq!(resp.result.status, "pending_approval");
        let response_audit_id = resp
            .result
            .audit_event_id
            .expect("audit_event_id also set on pending_approval");

        let events = drained_events(&sink);
        assert_eq!(events.len(), 1);
        match &events[0] {
            gadgetron_core::audit::ActionAuditEvent::DirectActionCompleted {
                event_id,
                outcome,
                ..
            } => {
                assert_eq!(*event_id, response_audit_id);
                assert!(matches!(
                    outcome,
                    gadgetron_core::audit::ActionAuditOutcome::PendingApproval
                ));
            }
            _ => unreachable!("only DirectActionCompleted shipping today"),
        }
    }

    #[tokio::test]
    async fn invoke_dispatch_error_emits_error_audit_event() {
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
        let sink = Arc::new(CapturingAuditSink::default());
        let svc = InProcessWorkbenchActionService::new_full(
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            Some(Arc::new(FailingDispatcher)),
            sink.clone() as Arc<dyn gadgetron_core::audit::ActionAuditSink>,
            None,
        );
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"query": "x"}),
            client_invocation_id: None,
        };
        let err = svc
            .invoke(&actor(), &actor_scopes_default(), "knowledge-search", req)
            .await
            .unwrap_err();
        assert!(matches!(err, WorkbenchHttpError::Core(_)));

        let events = drained_events(&sink);
        assert_eq!(events.len(), 1);
        match &events[0] {
            gadgetron_core::audit::ActionAuditEvent::DirectActionCompleted { outcome, .. } => {
                match outcome {
                    gadgetron_core::audit::ActionAuditOutcome::Error { error_code } => {
                        assert!(!error_code.is_empty(), "error_code must be populated");
                    }
                    other => panic!("expected Error outcome, got {other:?}"),
                }
            }
            _ => unreachable!("only DirectActionCompleted shipping today"),
        }
    }

    // -----------------------------------------------------------------------
    // SUBTASK 3.3.2 — approval persistence + resume
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_with_approval_store_persists_pending_record() {
        use crate::web::approval_store::InMemoryApprovalStore;
        use gadgetron_core::workbench::{
            ApprovalState, ApprovalStore, WorkbenchActionDescriptor, WorkbenchActionKind,
            WorkbenchActionPlacement,
        };
        let mut catalog = DescriptorCatalog::seed_p2b();
        catalog.actions.push(WorkbenchActionDescriptor {
            id: "needs-approval".into(),
            title: "Needs Approval".into(),
            owner_bundle: "ops".into(),
            source_kind: "gadget".into(),
            source_id: "wiki.write".into(),
            gadget_name: Some("wiki.write".into()),
            placement: WorkbenchActionPlacement::ContextMenu,
            kind: WorkbenchActionKind::Mutation,
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":true}),
            destructive: false,
            requires_approval: true,
            knowledge_hint: "approval-gated".into(),
            required_scope: None,
            disabled_reason: None,
        });
        let store: Arc<dyn ApprovalStore> = Arc::new(InMemoryApprovalStore::new());
        let svc = InProcessWorkbenchActionService::new_full(
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(catalog.into_snapshot())),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            None,
            Arc::new(gadgetron_core::audit::NoopActionAuditSink),
            Some(store.clone()),
        );
        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"note": "keep this"}),
            client_invocation_id: None,
        };
        let resp = svc
            .invoke(&actor(), &actor_scopes_default(), "needs-approval", req)
            .await
            .unwrap();
        assert_eq!(resp.result.status, "pending_approval");
        let approval_id = resp.result.approval_id.unwrap();
        let persisted = store.get(approval_id).await.unwrap();
        assert_eq!(persisted.state, ApprovalState::Pending);
        assert_eq!(persisted.action_id, "needs-approval");
        assert_eq!(persisted.args["note"], "keep this");
    }

    #[tokio::test]
    async fn resume_approval_dispatches_with_stored_args() {
        use crate::web::approval_store::InMemoryApprovalStore;
        use gadgetron_core::workbench::{
            ApprovalStore, WorkbenchActionDescriptor, WorkbenchActionKind, WorkbenchActionPlacement,
        };

        #[derive(Clone)]
        struct EchoDispatcher;
        #[async_trait]
        impl gadgetron_core::agent::tools::GadgetDispatcher for EchoDispatcher {
            async fn dispatch_gadget(
                &self,
                _name: &str,
                args: serde_json::Value,
            ) -> Result<
                gadgetron_core::agent::tools::GadgetResult,
                gadgetron_core::agent::tools::GadgetError,
            > {
                Ok(gadgetron_core::agent::tools::GadgetResult {
                    content: serde_json::json!({"echo": args}),
                    is_error: false,
                })
            }
        }

        let mut catalog = DescriptorCatalog::seed_p2b();
        catalog.actions.push(WorkbenchActionDescriptor {
            id: "sensitive-write".into(),
            title: "Sensitive Write".into(),
            owner_bundle: "ops".into(),
            source_kind: "gadget".into(),
            source_id: "wiki.write".into(),
            gadget_name: Some("wiki.write".into()),
            placement: WorkbenchActionPlacement::ContextMenu,
            kind: WorkbenchActionKind::Mutation,
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":true}),
            destructive: false,
            requires_approval: true,
            knowledge_hint: "approval-gated".into(),
            required_scope: None,
            disabled_reason: None,
        });
        let store: Arc<dyn ApprovalStore> = Arc::new(InMemoryApprovalStore::new());
        let svc = InProcessWorkbenchActionService::new_full(
            std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(catalog.into_snapshot())),
            InMemoryReplayCache::new(DEFAULT_REPLAY_TTL),
            None,
            Some(Arc::new(EchoDispatcher)),
            Arc::new(gadgetron_core::audit::NoopActionAuditSink),
            Some(store.clone()),
        );

        let req = InvokeWorkbenchActionRequest {
            args: serde_json::json!({"name": "pages/ops/audit", "content": "body"}),
            client_invocation_id: None,
        };
        let invoke_resp = svc
            .invoke(&actor(), &actor_scopes_default(), "sensitive-write", req)
            .await
            .unwrap();
        assert_eq!(invoke_resp.result.status, "pending_approval");
        let approval_id = invoke_resp.result.approval_id.unwrap();

        // Approve (same actor — same tenant boundary).
        let approved = store.mark_approved(approval_id, &actor()).await.unwrap();

        let resume_resp = svc
            .resume_approval(&actor(), &actor_scopes_default(), approved)
            .await
            .unwrap();
        assert_eq!(resume_resp.result.status, "ok");
        let payload = resume_resp.result.payload.unwrap();
        assert_eq!(payload["echo"]["name"], "pages/ops/audit");
        assert_eq!(payload["echo"]["content"], "body");
    }
}
