//! Workbench gateway routes.
//!
//! Endpoints mounted at `/api/v1/web/workbench/`:
//!   GET  /bootstrap                        → `get_workbench_bootstrap`
//!   GET  /activity                         → `list_workbench_activity`
//!   GET  /requests/:request_id/evidence    → `get_workbench_request_evidence`
//!   GET  /knowledge-status                 → `get_knowledge_status`
//!   GET  /capabilities                     → `get_capability_projection`
//!   GET  /views                            → `list_views`
//!   GET  /views/:view_id/data              → `load_view_data`
//!   GET  /actions                          → `list_actions`
//!   POST /actions/:action_id               → `invoke_action`

use std::{
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use gadgetron_core::{
    agent::ConversationAgentProfile,
    error::{DatabaseErrorKind, GadgetronError, PennyErrorKind},
    policy::{PolicyDocument, PolicyInput},
    workbench::{
        InvokeWorkbenchActionRequest, InvokeWorkbenchActionResponse, WorkbenchActivityResponse,
        WorkbenchBootstrapResponse, WorkbenchCapabilityProjectionResponse,
        WorkbenchKnowledgeStatusResponse, WorkbenchRegisteredActionsResponse,
        WorkbenchRegisteredViewsResponse, WorkbenchRequestEvidenceResponse, WorkbenchViewData,
    },
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::server::AppState;

// ---------------------------------------------------------------------------
// Actor carrier
// ---------------------------------------------------------------------------

/// Lightweight actor carrier.
///
/// Handlers extract the `TenantContext` from request extensions but
/// forward only this carrier to projection/action services.
pub use gadgetron_core::knowledge::AuthenticatedContext;

// ---------------------------------------------------------------------------
// Projection service trait
// ---------------------------------------------------------------------------

/// Read-model projection contract for workbench endpoints.
///
/// Implementors assemble health data, activity, evidence, and descriptor
/// listings from knowledge and other subsystems. Handlers delegate to this
/// trait to remain testable without the full knowledge stack.
#[async_trait]
pub trait WorkbenchProjectionService: Send + Sync {
    // --- bootstrap / activity / evidence ---

    /// Gateway version, knowledge health summary, active plugs.
    async fn bootstrap(&self) -> Result<WorkbenchBootstrapResponse, WorkbenchHttpError>;

    /// Recent activity feed (Penny turns, direct actions, system events).
    /// `limit` is already clamped to `[1, 100]` by the handler.
    async fn activity(&self, limit: u32) -> Result<WorkbenchActivityResponse, WorkbenchHttpError>;

    /// Per-request evidence (tool traces, citations, candidates).
    async fn request_evidence(
        &self,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, WorkbenchHttpError>;

    // --- descriptor catalog + knowledge status + view data ---

    /// Knowledge plane readiness (canonical / search / relation plugs).
    async fn knowledge_status(
        &self,
    ) -> Result<WorkbenchKnowledgeStatusResponse, WorkbenchHttpError>;

    /// Actor-visible registered views. `actor_scopes` drives the
    /// descriptor-visibility filter — only views whose
    /// `required_scope` is `None` OR satisfied by the supplied scopes
    /// are surfaced. The handler threads `ctx.scopes` from the
    /// auth middleware straight through.
    async fn views(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchRegisteredViewsResponse, WorkbenchHttpError>;

    /// View payload for a single registered view. `actor_scopes` is
    /// consulted to reject requests for views the caller cannot see
    /// (returns `ViewNotFound` — 404 — to avoid leaking existence of
    /// scope-gated views per doc §2.4.1).
    async fn view_data(
        &self,
        actor: &gadgetron_core::context::TenantContext,
        view_id: &str,
    ) -> Result<WorkbenchViewData, WorkbenchHttpError>;

    /// Actor-visible registered actions. Same scope-filter semantics
    /// as `views`.
    async fn actions(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchRegisteredActionsResponse, WorkbenchHttpError>;

    /// Actor-visible signed Bundle capability aggregate. Legacy projection
    /// implementations return the stable empty Core-only response.
    async fn capabilities(
        &self,
        _actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchCapabilityProjectionResponse, WorkbenchHttpError> {
        Ok(WorkbenchCapabilityProjectionResponse::default())
    }

    async fn contribution_data(
        &self,
        _actor: &gadgetron_core::context::TenantContext,
        contribution_id: &str,
    ) -> Result<gadgetron_core::workbench::WorkbenchContributionData, WorkbenchHttpError> {
        Err(WorkbenchHttpError::ContributionNotFound {
            contribution_id: contribution_id.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Action service trait
// ---------------------------------------------------------------------------

/// Direct-action invocation contract.
#[async_trait]
pub trait WorkbenchActionService: Send + Sync {
    /// Execute a direct action: validates args, checks replay cache, fans out
    /// to audit/activity/candidate capture, returns the result envelope.
    ///
    /// `actor_scopes` drives the step-3 scope check against
    /// `ActionDescriptor.required_scope`. The handler threads
    /// `TenantContext.scopes` straight through from the auth middleware —
    /// no more placeholder `[Scope::OpenAiCompat]` hardcoding.
    async fn invoke(
        &self,
        actor: &AuthenticatedContext,
        actor_scopes: &[gadgetron_core::context::Scope],
        action_id: &str,
        request: InvokeWorkbenchActionRequest,
    ) -> Result<InvokeWorkbenchActionResponse, WorkbenchHttpError>;

    /// Resume an already-approved request. Called by the approval
    /// endpoint after `ApprovalStore::mark_approved`. Skips steps
    /// 1-6 (actor resolution, descriptor lookup, scope check, schema
    /// validation, replay cache, approval gate) because those already
    /// ran at the original invoke. Jumps to step 7 (dispatch) with
    /// the persisted args and returns the ok/error response.
    ///
    /// Default impl returns `NotImplemented` for services that don't
    /// support resume.
    async fn resume_approval(
        &self,
        _actor: &AuthenticatedContext,
        _actor_scopes: &[gadgetron_core::context::Scope],
        _approval: gadgetron_core::workbench::ApprovalRequest,
    ) -> Result<InvokeWorkbenchActionResponse, WorkbenchHttpError> {
        Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "approval resume is not supported by this action service".into(),
        )))
    }
}

// ---------------------------------------------------------------------------
// Gateway-local service wrapper
// ---------------------------------------------------------------------------

/// Workbench service bundle held in `AppState`.
///
/// `actions` is `Option` so builds that do not wire the action service
/// (e.g. read-only admin views) still compile and boot cleanly.
#[derive(Clone)]
pub struct GatewayWorkbenchService {
    pub projection: Arc<dyn WorkbenchProjectionService>,
    /// `None` → `POST /actions/:id` returns 501 Not Implemented.
    pub actions: Option<Arc<dyn WorkbenchActionService>>,
    /// Approval store for persisting `pending_approval` records.
    /// `None` → `POST /approvals/:id/*` returns 501 and the action
    /// service skips the persistence call in step 6 (falls back to
    /// bare `Uuid::new_v4()` so older tests keep passing).
    pub approval_store: Option<Arc<dyn gadgetron_core::workbench::ApprovalStore>>,
    /// Shared R3.2b evaluator used by tool, action, Review and background paths.
    /// Production PostgreSQL wiring always sets it; no-db/test fixtures may
    /// retain the legacy compatibility gates with `None`.
    pub policy_evaluator: Option<Arc<dyn gadgetron_core::policy::PolicyEvaluator>>,
    /// Live catalog supplying Core-normalized policy metadata for every
    /// in-process and signed Bundle Gadget.
    pub gadget_catalog: Option<Arc<dyn gadgetron_core::agent::tools::GadgetCatalog>>,
    /// Shared `Arc<ArcSwap<CatalogSnapshot>>` — the same handle both
    /// `projection` and `actions` hold. Exposed here so
    /// `POST /api/v1/web/workbench/admin/reload-catalog` can atomically
    /// swap in a fresh snapshot (catalog + validators together).
    /// `None` in legacy test fixtures that don't wire a workbench;
    /// production always sets it.
    pub descriptor_catalog: Option<Arc<arc_swap::ArcSwap<crate::web::catalog::CatalogSnapshot>>>,
    /// Optional file path the reload handler reads on every call.
    /// When set, reload parses this TOML file and adds it to the Core
    /// catalog; when unset, reload uses the Core catalog alone. Cloned
    /// from `WebConfig.catalog_path` at startup.
    pub catalog_path: Option<String>,
    /// Optional bundles directory for multi-bundle aggregation.
    /// When set, reload scans every `<dir>/<bundle>/bundle.toml` and
    /// merges them into one additive catalog. It selects the external
    /// descriptor source ahead of `catalog_path`; Core descriptors remain
    /// present. Cloned from `WebConfig.bundles_dir` at startup.
    pub bundles_dir: Option<String>,
    /// Bundle signing trust anchors. Cloned from
    /// `WebConfig.bundle_signing` at startup. Empty list +
    /// `require_signature = false` preserves the unsigned-install
    /// behavior for deployments that haven't rotated to signed
    /// bundles yet.
    pub bundle_signing: gadgetron_core::config::BundleSigningConfig,
    /// Generic external-runtime lifecycle manager. `None` on unsupported
    /// platforms, in legacy tests, or when no bundles directory is configured.
    pub runtime_manager: Option<Arc<crate::web::bundle_runtime::BundleRuntimeManager>>,
    /// Live mode matrix for `PATCH /workbench/agent/modes`. Seeded
    /// from the on-disk `[agent.gadgets]` at startup; the editor in
    /// the Side Panel swaps in a new `GadgetsConfig` on each save.
    /// `None` in legacy fixtures that don't wire a workbench.
    pub gadget_modes: Option<Arc<arc_swap::ArcSwap<gadgetron_core::agent::GadgetsConfig>>>,
    /// Trait-object handle to the Penny `GadgetRegistry` so the
    /// `PATCH` handler can ask it to rebuild its derived sets
    /// (`allowed_names`, `ask_names`) against the new config without
    /// the gateway depending on `gadgetron-penny` directly.
    pub gadget_mode_reconfigurer:
        Option<Arc<dyn gadgetron_core::agent::tools::GadgetModeReconfigurer>>,
    /// Live Penny brain settings. Seeded from `[agent.brain]` at startup,
    /// overlaid from DB when available, and swapped by the Management-scoped
    /// `/admin/agent/brain` endpoint. Running Claude Code subprocesses keep
    /// their env/args; the next Penny turn reads the new snapshot.
    pub agent_brain: Option<Arc<arc_swap::ArcSwap<gadgetron_core::agent::AgentConfig>>>,
    /// Frozen base `AgentConfig` — used by the `PATCH` handler to
    /// rebuild a full `AgentConfig` with the new `.gadgets` slot so
    /// the reconfigurer's `reconfigure(&AgentConfig)` signature is
    /// satisfied. The registry only reads `.gadgets` internally, so
    /// the other fields can be anything consistent with startup.
    pub agent_config_base: Option<Arc<gadgetron_core::agent::AgentConfig>>,
    /// R2 tenant Domain Vault physical layout. `None` when `[knowledge]`
    /// is disabled; DB-only/no-knowledge fixtures keep the endpoints unavailable.
    pub vault_layout: Option<Arc<gadgetron_knowledge::vault::TenantVaultLayout>>,
}

// ---------------------------------------------------------------------------
// HTTP error type
// ---------------------------------------------------------------------------

/// Gateway-local error wrapper for workbench endpoints.
///
/// `GadgetronError` is for shared infrastructure errors (auth, quota, DB).
/// The gateway-local variants MUST NOT be added to the shared core error
/// taxonomy per D-12 (§3.4).
///
/// Error shape follows the OpenAI envelope (§2.4.1):
/// `{ "error": { "message": "…", "type": "…", "code": "…" } }`
#[derive(Debug)]
pub enum WorkbenchHttpError {
    /// Propagate an existing infrastructure error.
    Core(GadgetronError),
    /// The requested `request_id` does not exist or is not visible to the actor.
    RequestNotFound { request_id: Uuid },
    /// The requested view id is not registered / not visible to this actor.
    ViewNotFound { view_id: String },
    /// The requested action id is not registered / not visible to this actor.
    ActionNotFound { action_id: String },
    /// The `args` field failed JSON-Schema validation.
    ActionInvalidArgs { detail: String },
    /// The instance policy has disabled direct action invocations.
    DirectActionsDisabled,
    /// The requested approval id does not exist.
    ApprovalNotFound,
    /// No chat job matches the requested conversation / job id, OR the
    /// matching job belongs to another tenant / user. One variant for
    /// both so the response never leaks whether a foreign job exists.
    JobNotFound,
    /// The approval has already been resolved (approved or denied).
    ApprovalAlreadyResolved { state: String },
    /// The actor's tenant differs from the approval's tenant.
    ApprovalForbidden,
    /// The signed contribution is absent or not visible to this actor.
    ContributionNotFound { contribution_id: String },
    /// Optimistic Bundle control-plane revision/source digest changed.
    BundleConflict { detail: String },
    /// A signed Bundle control-plane operation reached a safe, actionable failure.
    BundleOperationFailed { code: String, detail: String },
    /// Knowledge Space/object is absent or deliberately hidden by tenant ACL.
    KnowledgeNotFound,
    /// Effective Space role is below the operation's requirement.
    KnowledgeForbidden,
    /// Optimistic revision or active-share uniqueness conflict.
    KnowledgeConflict,
    /// Client supplied an invalid Space/Vault/share contract value.
    KnowledgeInvalidInput { detail: String },
    /// Tenant has no policy revision or requested history row is absent.
    PolicyNotFound,
    /// Optimistic policy revision changed.
    PolicyConflict { current_revision: i64 },
    /// Client supplied an invalid policy document or preview input.
    PolicyInvalidInput { detail: String },
    /// Versioned policy denied the actual execution.
    PolicyDenied { detail: String },
    /// Policy storage/evaluator was unavailable on a protected path.
    PolicyUnavailable { detail: String },
    /// Approved request no longer matches its policy-bound input.
    PolicyBindingMismatch,
    /// Manager-owned records are absent or deliberately hidden by tenant scope.
    ManagerNotFound,
    /// Manager record lifecycle or optimistic revision changed.
    ManagerConflict,
    /// Client supplied an invalid oversight, directive, or webhook contract.
    ManagerInvalidInput { detail: String },
    /// A legacy API key without a real user cannot issue Manager decisions.
    ManagerIdentityRequired,
    /// Original source is retained but fetch/extraction could not complete.
    KnowledgeSourceFailed {
        source_id: uuid::Uuid,
        code: String,
        detail: String,
    },
}

impl From<GadgetronError> for WorkbenchHttpError {
    fn from(err: GadgetronError) -> Self {
        Self::Core(err)
    }
}

/// Extract the Postgres pool from `AppState`, returning a 503-shape
/// `WorkbenchHttpError::Core(Config(...))` when the pool is unwired
/// (no-db serve mode). Centralizes the per-handler pre-check so each
/// call site collapses from 5 lines to 1. `what` names the operation
/// (user-facing string in the error body).
fn require_pg_pool<'a>(
    state: &'a AppState,
    what: &str,
) -> Result<&'a sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "{what} requires Postgres (no pool configured)"
        )))
    })
}

impl IntoResponse for WorkbenchHttpError {
    fn into_response(self) -> Response {
        match self {
            WorkbenchHttpError::Core(err) => {
                let status = StatusCode::from_u16(err.http_status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let body = json!({
                    "error": {
                        "message": err.error_message(),
                        "type": err.error_type(),
                        "code": err.error_code(),
                    }
                });
                (status, Json(body)).into_response()
            }
            WorkbenchHttpError::RequestNotFound { request_id } => {
                let body = json!({
                    "error": {
                        "message": format!(
                            "Request {} not found or is not visible to the current user. \
                             Verify the request_id or refresh the shell.",
                            request_id
                        ),
                        "type": "invalid_request_error",
                        "code": "workbench_request_not_found",
                    }
                });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::ViewNotFound { view_id } => {
                let body = json!({
                    "error": {
                        "message": format!(
                            "View '{}' is not visible to the current user or has been removed. \
                             Refresh the shell.",
                            view_id
                        ),
                        "type": "invalid_request_error",
                        "code": "workbench_view_not_found",
                    }
                });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::ActionNotFound { action_id } => {
                let body = json!({
                    "error": {
                        "message": format!(
                            "Action '{}' is not visible to the current user or has been removed. \
                             Refresh the shell.",
                            action_id
                        ),
                        "type": "invalid_request_error",
                        "code": "workbench_action_not_found",
                    }
                });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::ActionInvalidArgs { detail } => {
                let body = json!({
                    "error": {
                        "message": format!(
                            "Action input does not match the descriptor schema. \
                             Please review the form and try again. Detail: {}",
                            detail
                        ),
                        "type": "invalid_request_error",
                        "code": "workbench_action_invalid_args",
                    }
                });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            WorkbenchHttpError::DirectActionsDisabled => {
                let body = json!({
                    "error": {
                        "message": "This instance has disabled direct actions. \
                                    Use the Penny conversation path or contact \
                                    your administrator to change the policy.",
                        "type": "permission_error",
                        "code": "forbidden",
                    }
                });
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            WorkbenchHttpError::ApprovalNotFound => {
                let body = json!({
                    "error": {
                        "message": "Approval not found. It may have expired or \
                                    been removed. Re-invoke the action to get \
                                    a fresh approval id.",
                        "type": "invalid_request_error",
                        "code": "workbench_approval_not_found",
                    }
                });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::JobNotFound => {
                let body = json!({
                    "error": {
                        "message": "No chat job found. The job may have \
                                    finished and been reaped, or never \
                                    existed in this process.",
                        "type": "invalid_request_error",
                        "code": "workbench_job_not_found",
                    }
                });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::ApprovalAlreadyResolved { state } => {
                let body = json!({
                    "error": {
                        "message": format!(
                            "Approval has already been resolved (state={state}). \
                             Approvals can only be resolved once.",
                        ),
                        "type": "invalid_request_error",
                        "code": "workbench_approval_already_resolved",
                        "state": state,
                    }
                });
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            WorkbenchHttpError::ApprovalForbidden => {
                let body = json!({
                    "error": {
                        "message": "Approval belongs to a different tenant. \
                                    Ask the owning tenant's administrator to \
                                    resolve it.",
                        "type": "permission_error",
                        "code": "forbidden",
                    }
                });
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            WorkbenchHttpError::ContributionNotFound { contribution_id } => {
                let body = json!({
                    "error": {
                        "message": format!(
                            "Contribution {contribution_id:?} is unavailable or not visible to the current user. Refresh the shell or enable its Bundle."
                        ),
                        "type": "invalid_request_error",
                        "code": "workbench_contribution_not_found",
                    }
                });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::BundleConflict { detail } => {
                let body = json!({
                    "error": {
                        "message": detail,
                        "type": "invalid_request_error",
                        "code": "bundle_control_conflict",
                    }
                });
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            WorkbenchHttpError::BundleOperationFailed { code, detail } => {
                let body = json!({
                    "error": {
                        "message": detail,
                        "type": "invalid_request_error",
                        "code": code,
                    }
                });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            WorkbenchHttpError::KnowledgeNotFound => {
                let body = json!({"error": {
                    "message": "Knowledge resource not found or not visible to this actor.",
                    "type": "invalid_request_error",
                    "code": "knowledge_not_found"
                }});
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::KnowledgeForbidden => {
                let body = json!({"error": {
                    "message": "The actor does not have the required Knowledge Space role.",
                    "type": "permission_error",
                    "code": "knowledge_forbidden"
                }});
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            WorkbenchHttpError::KnowledgeConflict => {
                let body = json!({"error": {
                    "message": "Knowledge revision or active state changed. Refresh and retry.",
                    "type": "invalid_request_error",
                    "code": "knowledge_revision_conflict"
                }});
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            WorkbenchHttpError::KnowledgeInvalidInput { detail } => {
                let body = json!({"error": {
                    "message": detail,
                    "type": "invalid_request_error",
                    "code": "knowledge_invalid_input"
                }});
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            WorkbenchHttpError::PolicyNotFound => {
                let body = json!({"error": {
                    "message": "Policy revision was not found for this tenant.",
                    "type": "invalid_request_error",
                    "code": "policy_not_found"
                }});
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::PolicyConflict { current_revision } => {
                let body = json!({"error": {
                    "message": "Policy changed since it was loaded. Refresh and create a new revision.",
                    "type": "invalid_request_error",
                    "code": "policy_revision_conflict",
                    "current_revision": current_revision
                }});
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            WorkbenchHttpError::PolicyInvalidInput { detail } => {
                let body = json!({"error": {
                    "message": detail,
                    "type": "invalid_request_error",
                    "code": "policy_invalid_input"
                }});
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            WorkbenchHttpError::PolicyDenied { detail } => {
                let body = json!({"error": {
                    "message": detail,
                    "type": "permission_error",
                    "code": "policy_denied"
                }});
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            WorkbenchHttpError::PolicyUnavailable { detail } => {
                let body = json!({"error": {
                    "message": detail,
                    "type": "server_error",
                    "code": "policy_unavailable"
                }});
                (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
            }
            WorkbenchHttpError::PolicyBindingMismatch => {
                let body = json!({"error": {
                    "message": "Approved request no longer matches its policy-bound input.",
                    "type": "permission_error",
                    "code": "policy_binding_mismatch"
                }});
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            WorkbenchHttpError::ManagerNotFound => {
                let body = json!({"error": {
                    "message": "Manager record was not found or is not visible to this tenant.",
                    "type": "invalid_request_error",
                    "code": "manager_record_not_found"
                }});
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            WorkbenchHttpError::ManagerConflict => {
                let body = json!({"error": {
                    "message": "Manager record state or revision changed. Refresh before continuing.",
                    "type": "invalid_request_error",
                    "code": "manager_record_conflict"
                }});
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            WorkbenchHttpError::ManagerInvalidInput { detail } => {
                let body = json!({"error": {
                    "message": detail,
                    "type": "invalid_request_error",
                    "code": "manager_invalid_input"
                }});
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            WorkbenchHttpError::ManagerIdentityRequired => {
                let body = json!({"error": {
                    "message": "Manager decisions require a signed-in user identity; rotate this legacy API key or use the web session.",
                    "type": "permission_error",
                    "code": "manager_identity_required"
                }});
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            WorkbenchHttpError::KnowledgeSourceFailed {
                source_id,
                code,
                detail,
            } => {
                let body = json!({"error": {
                    "message": detail,
                    "type": "source_ingestion_error",
                    "code": code,
                    "source_id": source_id
                }});
                (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Query types
// ---------------------------------------------------------------------------

/// Query parameters for `GET /activity`.
#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    #[serde(default = "default_activity_limit")]
    pub limit: u32,
}

fn default_activity_limit() -> u32 {
    50
}

/// Maximum entries returned per activity request (§2.3).
pub const ACTIVITY_LIMIT_MAX: u32 = 100;
/// Minimum entries (avoids callers passing 0).
pub const ACTIVITY_LIMIT_MIN: u32 = 1;

// ---------------------------------------------------------------------------
// Handlers — bootstrap / activity / evidence
// ---------------------------------------------------------------------------

/// `GET /bootstrap` — gateway version, plug health, knowledge summary.
pub async fn get_workbench_bootstrap(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchBootstrapResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.bootstrap().await?;
    Ok(Json(resp))
}

/// `GET /activity` — recent workbench activity feed (limit clamped to [1,100]).
pub async fn list_workbench_activity(
    State(state): State<AppState>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<WorkbenchActivityResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let limit = query.limit.clamp(ACTIVITY_LIMIT_MIN, ACTIVITY_LIMIT_MAX);
    let resp = svc.projection.activity(limit).await?;
    Ok(Json(resp))
}

/// `GET /requests/:request_id/evidence` — per-request evidence.
pub async fn get_workbench_request_evidence(
    State(state): State<AppState>,
    Path(request_id): Path<Uuid>,
) -> Result<Json<WorkbenchRequestEvidenceResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.request_evidence(request_id).await?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// Handlers — descriptor catalog + knowledge status + view data
// ---------------------------------------------------------------------------

/// `GET /knowledge-status` — knowledge plane readiness.
pub async fn get_knowledge_status(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchKnowledgeStatusResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.knowledge_status().await?;
    Ok(Json(resp))
}

/// `GET /views` — actor-visible registered views.
pub async fn list_views(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<WorkbenchRegisteredViewsResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.views(&ctx.scopes).await?;
    Ok(Json(resp))
}

/// `GET /views/:view_id/data` — payload for a single registered view.
pub async fn load_view_data(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(view_id): Path<String>,
) -> Result<Json<WorkbenchViewData>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.view_data(&ctx, &view_id).await?;
    Ok(Json(resp))
}

/// `GET /actions` — actor-visible registered actions.
pub async fn list_actions(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<WorkbenchRegisteredActionsResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.actions(&ctx.scopes).await?;
    Ok(Json(resp))
}

/// `GET /capabilities` — one actor-filtered immutable Bundle capability snapshot.
pub async fn get_capability_projection(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<WorkbenchCapabilityProjectionResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    Ok(Json(svc.projection.capabilities(&ctx.scopes).await?))
}

pub async fn get_contribution_data(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(contribution_id): Path<String>,
) -> Result<Json<gadgetron_core::workbench::WorkbenchContributionData>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    Ok(Json(
        svc.projection
            .contribution_data(&ctx, &contribution_id)
            .await?,
    ))
}

/// `GET /approvals/pending` — list pending approvals for the caller's
/// tenant, newest first. Powers Review Center → Exceptions.
pub async fn list_pending_approvals(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let store = svc.approval_store.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "approval store is not wired in this build".into(),
        ))
    })?;
    let rows = store
        .list_pending(ctx.tenant_id)
        .await
        .map_err(|e| WorkbenchHttpError::Core(GadgetronError::Provider(e.to_string())))?;
    Ok(Json(
        serde_json::json!({ "approvals": rows, "count": rows.len() }),
    ))
}

/// `POST /approvals/:approval_id/approve` — resolve a pending Review.
///
/// Looks up the approval record, marks it Approved on behalf of the
/// calling actor. Workbench actions resume here; bounded Tool/Bundle callers
/// observe the Approved state and revalidate before their own dispatch.
///
///   - 404 when the id is unknown
///   - 409 when the approval has already been resolved
///   - 403 when the caller's tenant differs from the approval's tenant
///   - 501 when the approval_store or action service isn't wired
pub async fn approve_action(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(approval_id): Path<Uuid>,
) -> Result<Json<InvokeWorkbenchActionResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let store = svc.approval_store.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "approval store is not wired in this build".into(),
        ))
    })?;
    let action_svc = svc.actions.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "action service is not wired in this build".into(),
        ))
    })?;
    let actor = AuthenticatedContext {
        api_key_id: ctx.api_key_id,
        tenant_id: ctx.tenant_id,
        // Real owning user id from ValidatedKey.user_id
        // via TenantContext.actor_user_id. Some(..) for real users
        // (cookie sessions + backfilled api_keys); None for legacy
        // keys pre-backfill keys.
        real_user_id: ctx.actor_user_id,
    };
    let approved = store
        .mark_approved(approval_id, &actor)
        .await
        .map_err(approval_error_to_http)?;
    // Publish the approval resolution on the live feed BEFORE
    // dispatching — operators see the decision immediately even if
    // the subsequent dispatch takes a while.
    state.activity_bus.publish(
        gadgetron_core::activity_bus::ActivityEvent::ApprovalResolved {
            tenant_id: actor.tenant_id,
            approval_id: approved.id,
            action_id: approved.action_id.clone(),
            state: approved.state.as_str().to_string(),
            resolved_by_user_id: approved
                .resolved_by_user_id
                .or(actor.real_user_id)
                .unwrap_or(actor.api_key_id),
        },
    );
    let action_id = approved.action_id.clone();
    let approval_id = approved.id;
    if approved.resume_strategy == gadgetron_core::workbench::ApprovalResumeStrategy::WaitingCaller
    {
        tracing::info!(
            target: "workbench.approval",
            %approval_id,
            action_id = %action_id,
            "approval released a bounded tool/background caller"
        );
        return Ok(Json(InvokeWorkbenchActionResponse {
            result: gadgetron_core::workbench::WorkbenchActionResult {
                status: "ok".into(),
                approval_id: Some(approval_id),
                activity_event_id: None,
                audit_event_id: None,
                refresh_view_ids: Vec::new(),
                knowledge_candidates: Vec::new(),
                payload: None,
            },
        }));
    }
    // Compatibility records created before resume_strategy was added may use
    // a tool name rather than a Workbench descriptor id. Their external caller
    // owns dispatch, so suppress only ActionNotFound for that legacy shape.
    match action_svc
        .resume_approval(&actor, &ctx.scopes, approved)
        .await
    {
        Ok(resp) => {
            publish_action_activity(&state, &actor, &action_id, &resp, None);
            Ok(Json(resp))
        }
        Err(WorkbenchHttpError::ActionNotFound { .. }) => {
            tracing::info!(
                target: "workbench.approval",
                %approval_id,
                action_id = %action_id,
                "approve resolved an MCP-tool approval (no workbench descriptor); \
                 dispatch will run via the forwarding poll loop"
            );
            Ok(Json(InvokeWorkbenchActionResponse {
                result: gadgetron_core::workbench::WorkbenchActionResult {
                    status: "ok".into(),
                    approval_id: Some(approval_id),
                    activity_event_id: None,
                    audit_event_id: None,
                    refresh_view_ids: Vec::new(),
                    knowledge_candidates: Vec::new(),
                    payload: None,
                },
            }))
        }
        Err(e) => Err(e),
    }
}

/// `POST /approvals/:approval_id/deny` — refuse a `pending_approval`.
///
/// Marks the record as Denied with an optional reason body; does NOT
/// dispatch the gadget. Returns the resolved record so the caller can
/// display the timestamp + reason.
pub async fn deny_action(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(approval_id): Path<Uuid>,
    Json(body): Json<DenyApprovalRequest>,
) -> Result<Json<DenyApprovalResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let store = svc.approval_store.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "approval store is not wired in this build".into(),
        ))
    })?;
    let actor = AuthenticatedContext {
        api_key_id: ctx.api_key_id,
        tenant_id: ctx.tenant_id,
        // Real owning user id from ValidatedKey.user_id
        // via TenantContext.actor_user_id. Some(..) for real users
        // (cookie sessions + backfilled api_keys); None for legacy
        // keys pre-backfill keys.
        real_user_id: ctx.actor_user_id,
    };
    let denied = store
        .mark_denied(approval_id, &actor, body.reason)
        .await
        .map_err(approval_error_to_http)?;
    state.activity_bus.publish(
        gadgetron_core::activity_bus::ActivityEvent::ApprovalResolved {
            tenant_id: actor.tenant_id,
            approval_id: denied.id,
            action_id: denied.action_id.clone(),
            state: denied.state.as_str().to_string(),
            resolved_by_user_id: denied
                .resolved_by_user_id
                .or(actor.real_user_id)
                .unwrap_or(actor.api_key_id),
        },
    );
    Ok(Json(DenyApprovalResponse {
        id: denied.id,
        state: denied.state.as_str().to_string(),
        resolved_at: denied.resolved_at,
        resolved_by_user_id: denied.resolved_by_user_id,
        reason: denied.deny_reason,
    }))
}

/// Body payload for `POST /approvals/:id/deny`. Reason is optional —
/// operators may deny silently.
#[derive(Debug, Deserialize, Default)]
pub struct DenyApprovalRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

/// Response shape for `POST /approvals/:id/deny`.
#[derive(Debug, serde::Serialize)]
pub struct DenyApprovalResponse {
    pub id: Uuid,
    pub state: String,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub resolved_by_user_id: Option<Uuid>,
    pub reason: Option<String>,
}

/// `GET /workbench/agent/modes` — return the live `[agent.gadgets]`
/// matrix so the Side Panel → Tool Modes editor can seed its dropdowns
/// with the actual runtime state.
///
/// Returns 501 when the workbench wiring didn't provision
/// `gadget_modes` (e.g. binary started with no Penny registry).
pub async fn get_agent_modes(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let modes = svc.gadget_modes.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "agent.modes editor is not wired in this build (no Penny registry?)".into(),
        ))
    })?;
    let snapshot = modes.load_full();
    Ok(Json(serde_json::json!({
        "gadgets": &*snapshot,
    })))
}

/// `PATCH /workbench/agent/modes` — replace the live `[agent.gadgets]`
/// matrix. Body is the full `GadgetsConfig` (keeps the wire format
/// simple and deterministic — PATCHing the whole struct is effectively
/// PUT semantics but matches the REST idiom).
///
/// Behavior:
/// 1. Validate the incoming matrix (`GadgetsConfig::validate` — V1/V5/V6/V14)
/// 2. Atomically swap into the shared `ArcSwap`
/// 3. Call `GadgetModeReconfigurer::reconfigure` so the Penny registry
///    rebuilds its `allowed_names` + `ask_names` sets
///
/// The new modes take effect on the NEXT Penny dispatch and NEXT
/// Claude Code subprocess spawn — running subprocesses keep their
/// `--allowed-tools` list. For Ask-flow verification this is the
/// expected behavior: the next approval-gated Bundle call lands on the
/// approval card path.
pub async fn patch_agent_modes(
    State(state): State<AppState>,
    Json(body): Json<gadgetron_core::agent::GadgetsConfig>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let modes = svc.gadget_modes.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "agent.modes editor is not wired in this build (no Penny registry?)".into(),
        ))
    })?;
    body.validate().map_err(WorkbenchHttpError::Core)?;
    modes.store(Arc::new(body.clone()));
    if let (Some(reconfig), Some(base)) = (
        svc.gadget_mode_reconfigurer.as_ref(),
        svc.agent_config_base.as_ref(),
    ) {
        let mut next = (**base).clone();
        next.gadgets = body.clone();
        reconfig.reconfigure(&next);
    }
    Ok(Json(serde_json::json!({
        "gadgets": &body,
    })))
}

#[derive(Debug, Deserialize)]
pub struct CreatePolicyRevisionRequest {
    pub expected_revision: i64,
    pub document: PolicyDocument,
}

#[derive(Debug, Deserialize)]
pub struct CreateLegacyPolicyRevisionRequest {
    pub expected_revision: i64,
    pub gadgets: gadgetron_core::agent::GadgetsConfig,
}

#[derive(Debug, Deserialize)]
pub struct PreviewPolicyRequest {
    #[serde(default)]
    pub revision: Option<i64>,
    pub input: PolicyInput,
}

#[derive(Debug, Deserialize)]
pub struct PolicyDecisionQuery {
    #[serde(default = "default_policy_decision_limit")]
    pub limit: i64,
}

fn default_policy_decision_limit() -> i64 {
    50
}

fn enforcement_coverage(state: &AppState) -> serde_json::Value {
    let Some(service) = state.workbench.as_ref() else {
        return json!({
            "overall": "unavailable",
            "tool_calls": "unavailable",
            "background_jobs": "unavailable",
            "bundle_gadgets": "unavailable",
            "review_resume": "unavailable"
        });
    };
    let common = service.policy_evaluator.is_some();
    let catalog = service.gadget_catalog.is_some();
    let tool_calls =
        common && catalog && state.tool_catalog.is_some() && state.gadget_dispatcher.is_some();
    let background_jobs = common;
    let bundle_gadgets = common && catalog;
    let review_resume = common && service.approval_store.is_some() && service.actions.is_some();
    let status = |ready| if ready { "enforced" } else { "unavailable" };
    json!({
        "overall": status(tool_calls && background_jobs && bundle_gadgets && review_resume),
        "tool_calls": status(tool_calls),
        "background_jobs": status(background_jobs),
        "bundle_gadgets": status(bundle_gadgets),
        "review_resume": status(review_resume)
    })
}

async fn ensure_active_policy(
    state: &AppState,
    ctx: &gadgetron_core::context::TenantContext,
) -> Result<gadgetron_xaas::policy::PolicyRevision, WorkbenchHttpError> {
    let service = require_workbench(state)?;
    let modes = service.gadget_modes.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "policy compatibility source is not wired in this build".into(),
        ))
    })?;
    let pool = require_pg_pool(state, "versioned policy")?;
    let snapshot = modes.load_full();
    gadgetron_xaas::policy::ensure_legacy_policy(pool, ctx.tenant_id, ctx.actor_user_id, &snapshot)
        .await
        .map_err(policy_store_error_to_http)
}

/// Return the tenant's active immutable policy revision. The first read
/// performs the explicit Auto/Ask/Never compatibility migration once.
pub async fn get_active_policy(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let revision = ensure_active_policy(&state, &ctx).await?;
    Ok(Json(json!({
        "policy": revision,
        "enforcement_coverage": enforcement_coverage(&state)
    })))
}

/// Publish a new typed policy document without mutating prior revisions.
pub async fn create_policy_revision(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<CreatePolicyRevisionRequest>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let _ = ensure_active_policy(&state, &ctx).await?;
    let pool = require_pg_pool(&state, "versioned policy")?;
    let revision = gadgetron_xaas::policy::create_revision(
        pool,
        ctx.tenant_id,
        ctx.actor_user_id,
        body.expected_revision,
        gadgetron_xaas::policy::PolicyRevisionSource::Manager,
        &body.document,
        None,
    )
    .await
    .map_err(policy_store_error_to_http)?;
    Ok(Json(json!({
        "policy": revision,
        "enforcement_coverage": enforcement_coverage(&state)
    })))
}

/// Publish a compatibility revision from the complete legacy mode matrix.
pub async fn create_legacy_policy_revision(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<CreateLegacyPolicyRevisionRequest>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let _ = ensure_active_policy(&state, &ctx).await?;
    let pool = require_pg_pool(&state, "versioned policy")?;
    let document = PolicyDocument::from_legacy_gadget_modes(&body.gadgets).map_err(|error| {
        WorkbenchHttpError::PolicyInvalidInput {
            detail: error.to_string(),
        }
    })?;
    let revision = gadgetron_xaas::policy::create_revision(
        pool,
        ctx.tenant_id,
        ctx.actor_user_id,
        body.expected_revision,
        gadgetron_xaas::policy::PolicyRevisionSource::Manager,
        &document,
        Some(&body.gadgets),
    )
    .await
    .map_err(policy_store_error_to_http)?;
    Ok(Json(json!({
        "policy": revision,
        "enforcement_coverage": enforcement_coverage(&state)
    })))
}

/// Evaluate one normalized input without dispatching or writing an event.
pub async fn preview_policy(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<PreviewPolicyRequest>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let active = ensure_active_policy(&state, &ctx).await?;
    let revision = if let Some(number) = body.revision {
        gadgetron_xaas::policy::policy_revision(
            require_pg_pool(&state, "versioned policy")?,
            ctx.tenant_id,
            active.identity.policy_id,
            number,
        )
        .await
        .map_err(policy_store_error_to_http)?
    } else {
        active
    };
    let trace = revision
        .document
        .evaluate(revision.identity, &body.input)
        .map_err(|error| WorkbenchHttpError::PolicyInvalidInput {
            detail: error.to_string(),
        })?;
    let trace_hash = trace
        .digest()
        .map_err(|error| WorkbenchHttpError::PolicyInvalidInput {
            detail: error.to_string(),
        })?;
    Ok(Json(json!({
        "trace": trace,
        "trace_hash": trace_hash,
        "enforcement_coverage": "preview_only"
    })))
}

/// Read actual persisted decisions. Preview calls never appear here.
pub async fn list_policy_decisions(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Query(query): Query<PolicyDecisionQuery>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let decisions = gadgetron_xaas::policy::recent_decisions(
        require_pg_pool(&state, "policy decisions")?,
        ctx.tenant_id,
        query.limit,
    )
    .await
    .map_err(policy_store_error_to_http)?;
    Ok(Json(json!({
        "decisions": decisions,
        "count": decisions.len()
    })))
}

fn policy_store_error_to_http(
    error: gadgetron_xaas::policy::PolicyStoreError,
) -> WorkbenchHttpError {
    match error {
        gadgetron_xaas::policy::PolicyStoreError::NotFound => WorkbenchHttpError::PolicyNotFound,
        gadgetron_xaas::policy::PolicyStoreError::RevisionConflict { current_revision } => {
            WorkbenchHttpError::PolicyConflict { current_revision }
        }
        gadgetron_xaas::policy::PolicyStoreError::Policy(error) => {
            WorkbenchHttpError::PolicyInvalidInput {
                detail: error.to_string(),
            }
        }
        error @ (gadgetron_xaas::policy::PolicyStoreError::TraceMismatch
        | gadgetron_xaas::policy::PolicyStoreError::InvalidPersisted(_)) => {
            WorkbenchHttpError::PolicyInvalidInput {
                detail: error.to_string(),
            }
        }
        gadgetron_xaas::policy::PolicyStoreError::Database(error) => {
            WorkbenchHttpError::Core(GadgetronError::Database {
                kind: DatabaseErrorKind::Other,
                message: error.to_string(),
            })
        }
    }
}

/// `GET /api/v1/web/workbench/admin/agent/brain` — return the current
/// DB-backed Penny brain settings for this tenant, falling back to the
/// startup `[agent.brain]` snapshot when no row has been saved yet.
pub async fn get_agent_brain_settings(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<gadgetron_core::agent::AgentBrainSettings>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let brain = svc.agent_brain.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "agent brain settings are not wired in this build".into(),
        ))
    })?;
    let pool = require_pg_pool(&state, "agent brain settings")?;
    let saved = gadgetron_xaas::agent_brain::get_agent_brain_settings(pool, ctx.tenant_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "agent brain settings query: {e}"
            )))
        })?;
    if let Some(saved) = saved {
        return Ok(Json(saved));
    }

    let snapshot = brain.load_full();
    Ok(Json(gadgetron_core::agent::AgentBrainSettings::from_agent(
        &snapshot,
        gadgetron_core::agent::AgentBrainSettingsSource::ConfigFile,
        None,
        None,
    )))
}

/// `PATCH /api/v1/web/workbench/admin/agent/brain` — validate, persist,
/// and hot-swap the Penny brain settings. The new settings affect the next
/// Claude Code subprocess spawned by Penny.
pub async fn patch_agent_brain_settings(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<PatchAgentBrainSettingsRequest>,
) -> Result<Json<gadgetron_core::agent::AgentBrainSettings>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let brain = svc.agent_brain.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "agent brain settings are not wired in this build".into(),
        ))
    })?;
    let (mut body, runtime_auth_token) = normalize_agent_brain_settings_patch(body)?;
    if let Some(token) = runtime_auth_token {
        std::env::set_var(&token.env_name, &token.value);
    }
    let pool = require_pg_pool(&state, "agent brain settings")?;
    if body.llm_endpoint_id.is_some() {
        let profile = canonicalize_registered_endpoint_profile(
            pool,
            ctx.tenant_id,
            ConversationAgentProfile {
                backend: body.backend,
                llm_endpoint_id: body.llm_endpoint_id,
                model: body.model.clone(),
                effort: body.effort,
                model_source: body.model_source,
                local_base_url: body.local_base_url.clone(),
                local_api_key_env: body.local_api_key_env.clone(),
            },
        )
        .await
        .map_err(WorkbenchHttpError::Core)?;
        body.backend = profile.backend;
        body.model = profile.model;
        body.local_base_url = profile.local_base_url;
        body.local_api_key_env = profile.local_api_key_env;
        body.external_base_url = if body.backend == gadgetron_core::agent::AgentBackend::ClaudeCode
        {
            body.local_base_url.clone()
        } else {
            String::new()
        };
        body.external_auth_token_env =
            if body.backend == gadgetron_core::agent::AgentBackend::ClaudeCode {
                body.local_api_key_env.clone()
            } else {
                String::new()
            };
    }
    let current_agent = brain.load_full();
    // Apply the high-level admin axes (agent/model_source/local_*/effort)
    // to derive a complete next AgentConfig. This is what powers the
    // "Mode = Claude / Codex" + "Model = Default / Local" + "Effort"
    // switch — overlay_agent rewrites `runtime`, `brain.mode`,
    // `brain.external_*`, `codex.auth_mode`, and `codex.compatible_*_env`
    // accordingly. `overlay_brain` (the legacy raw fields) is now folded
    // into this so callers don't double-apply.
    let next_agent = body.overlay_agent(&current_agent);
    next_agent
        .validate_with_env(
            &std::collections::HashMap::new(),
            &gadgetron_core::agent::config::StdEnv,
        )
        .map_err(|e| {
            // The HTTP layer collapses every `Config` error into a generic
            // "check your gadgetron.toml" string for the API consumer, which
            // hides the actual reason from operators trying to debug live
            // settings. Log the specific cause at WARN before forwarding so
            // /tmp/gadgetron-serve.log surfaces it.
            tracing::warn!(
                target: "workbench.admin.agent_brain",
                mode = %next_agent.brain.mode.as_str(),
                external_base_url_len = next_agent.brain.external_base_url.len(),
                external_auth_token_env = %next_agent.brain.external_auth_token_env,
                error = %e,
                "agent brain settings validate_with_env rejected"
            );
            WorkbenchHttpError::Core(e)
        })?;

    let saved = gadgetron_xaas::agent_brain::upsert_agent_brain_settings(
        pool,
        ctx.tenant_id,
        ctx.actor_user_id,
        &body,
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "agent brain settings update: {e}"
        )))
    })?;
    brain.store(Arc::new(next_agent));
    tracing::info!(
        target: "workbench.admin.agent_brain",
        tenant_id = %ctx.tenant_id,
        actor_user_id = ?ctx.actor_user_id,
        mode = %saved.mode.as_str(),
        backend = ?saved.backend,
        model_source = ?saved.model_source,
        effort = ?saved.effort,
        has_model = !saved.model.is_empty(),
        has_base_url = !saved.external_base_url.is_empty(),
        has_auth_token_env = !saved.external_auth_token_env.is_empty(),
        custom_model_option = saved.custom_model_option,
        "agent brain settings updated"
    );

    Ok(Json(saved))
}

// ---------------------------------------------------------------------------
// LLM endpoint registry — admin control plane
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct CreateLlmEndpointRequest {
    pub name: String,
    pub kind: String,
    pub protocol: String,
    pub base_url: String,
    #[serde(default)]
    pub target_kind: Option<String>,
    #[serde(default)]
    pub target_host_id: Option<Uuid>,
    #[serde(default)]
    pub upstream_endpoint_id: Option<Uuid>,
    #[serde(default)]
    pub listen_port: Option<u16>,
    #[serde(default)]
    pub auth_token_env: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateCcrBridgeRequest {
    pub name: String,
    pub target_kind: String,
    #[serde(default)]
    pub target_host_id: Option<Uuid>,
    pub base_url: String,
    pub port: u16,
    #[serde(default)]
    pub auth_token_env: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AutoDetectLlmEndpointRequest {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
    /// Optional model override. OpenAI endpoints otherwise use the first
    /// discovered model; Anthropic gateways require an explicit model id.
    #[serde(default)]
    pub model_id: Option<String>,
    /// Persisted credential reference. The value itself is write-only and
    /// remains process-local.
    #[serde(default)]
    pub auth_token_env: Option<String>,
    #[serde(default)]
    pub auth_token_value: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct ProbeLlmEndpointRequest {
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub auth_token_value: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ListLlmEndpointsResponse {
    pub endpoints: Vec<gadgetron_xaas::llm_endpoints::LlmEndpointRow>,
    pub returned: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct DeleteLlmEndpointResponse {
    pub deleted: bool,
    pub endpoint_id: uuid::Uuid,
}

#[derive(Debug, serde::Serialize)]
pub struct ProbeLlmEndpointResponse {
    /// True only when the selected model produced the expected function/tool
    /// call. Connection state remains available on `endpoint.health_status`.
    pub ok: bool,
    pub endpoint: gadgetron_xaas::llm_endpoints::LlmEndpointRow,
    pub models: Vec<String>,
    pub message: String,
}

pub type AutoDetectLlmEndpointResponse = ProbeLlmEndpointResponse;

#[derive(Debug, serde::Serialize)]
pub struct UseLlmEndpointResponse {
    pub endpoint: gadgetron_xaas::llm_endpoints::LlmEndpointRow,
    pub brain: gadgetron_core::agent::AgentBrainSettings,
}

#[derive(Debug, serde::Deserialize)]
pub struct UseLlmEndpointRequest {
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub external_auth_token_value: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct AvailableLlmEndpointModel {
    pub endpoint_id: Uuid,
    pub endpoint_name: String,
    pub backend: &'static str,
    pub protocol: String,
    pub model_id: String,
}

#[derive(Debug, serde::Serialize)]
pub struct AvailableLlmEndpointModelsResponse {
    pub models: Vec<AvailableLlmEndpointModel>,
}

const DEFAULT_PENNY_EXTERNAL_AUTH_TOKEN_ENV: &str = "PENNY_CCR_AUTH_TOKEN";
const DEFAULT_PENNY_OPENAI_AUTH_TOKEN_ENV: &str = "PENNY_LOCAL_LLM_API_KEY";

#[derive(Debug, serde::Deserialize)]
pub struct PatchAgentBrainSettingsRequest {
    #[serde(flatten)]
    pub settings: gadgetron_core::agent::UpdateAgentBrainSettingsRequest,
    #[serde(default)]
    pub external_auth_token_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeAuthToken {
    env_name: String,
    value: String,
}

fn normalize_agent_brain_settings_patch(
    body: PatchAgentBrainSettingsRequest,
) -> Result<
    (
        gadgetron_core::agent::UpdateAgentBrainSettingsRequest,
        Option<RuntimeAuthToken>,
    ),
    WorkbenchHttpError,
> {
    let mut settings = body.settings;
    settings.effort = settings
        .effort
        .for_backend_model(settings.backend, &settings.model);
    settings.external_auth_token_env = settings.external_auth_token_env.trim().to_string();
    let token_value = normalize_optional_text(body.external_auth_token_value);

    if settings.mode == gadgetron_core::agent::BrainMode::ClaudeMax {
        settings.external_base_url.clear();
        settings.external_auth_token_env.clear();
        settings.custom_model_option = false;
        return Ok((settings, None));
    }

    if let Some(value) = token_value {
        if value.contains('\0') {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "agent brain auth token must not contain NUL bytes".into(),
            )));
        }
        if settings.external_auth_token_env.is_empty() {
            settings.external_auth_token_env = DEFAULT_PENNY_EXTERNAL_AUTH_TOKEN_ENV.to_string();
        }
        if !is_valid_runtime_auth_env_name(&settings.external_auth_token_env) {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "agent brain auth token env must match [A-Z_][A-Z0-9_]*".into(),
            )));
        }
        let token = RuntimeAuthToken {
            env_name: settings.external_auth_token_env.clone(),
            value,
        };
        return Ok((settings, Some(token)));
    }

    Ok((settings, None))
}

fn is_valid_runtime_auth_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModel>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiModel {
    id: String,
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn resolve_endpoint_auth(
    auth_token_env: Option<String>,
    auth_token_value: Option<String>,
    default_env: &str,
) -> Result<(Option<String>, Option<String>), WorkbenchHttpError> {
    let mut env_name = normalize_optional_text(auth_token_env);
    let supplied = normalize_optional_text(auth_token_value);
    if supplied.is_some() && env_name.is_none() {
        env_name = Some(default_env.to_string());
    }
    validate_optional_env_name(env_name.as_deref())?;
    if let Some(value) = supplied.as_deref() {
        if value.contains('\0') {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "endpoint auth token must not contain NUL bytes".into(),
            )));
        }
        if let Some(name) = env_name.as_deref() {
            std::env::set_var(name, value);
        }
    }
    let resolved = supplied.or_else(|| {
        env_name
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.trim().is_empty())
    });
    Ok((env_name, resolved))
}

fn endpoint_alias_or_host_port(alias: Option<String>, host: &str, port: u16) -> String {
    normalize_optional_text(alias).unwrap_or_else(|| format!("{}:{}", host.trim(), port))
}

fn validate_endpoint_base_url(value: &str) -> Result<String, WorkbenchHttpError> {
    let parsed = reqwest::Url::parse(value.trim()).map_err(|_| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint URL must be a valid http:// or https:// URL".into(),
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint URL must contain only http(s) scheme, host, port, and optional /v1 path"
                .into(),
        )));
    }
    let path = parsed.path().trim_end_matches('/');
    if !matches!(path, "" | "/v1") {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint URL path must be empty or /v1".into(),
        )));
    }
    if let Some(host) = parsed.host_str() {
        let normalized = host.trim_matches(['[', ']']).to_ascii_lowercase();
        if normalized == "169.254.169.254" || normalized == "metadata.google.internal" {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "cloud metadata endpoints are not valid LLM targets".into(),
            )));
        }
        if let Ok(ip) = normalized.parse::<std::net::IpAddr>() {
            let unsafe_address = match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_link_local() || ip.is_unspecified() || ip.is_multicast()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_unicast_link_local() || ip.is_unspecified() || ip.is_multicast()
                }
            };
            if unsafe_address {
                return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                    "link-local, unspecified, and multicast LLM targets are blocked".into(),
                )));
            }
        }
    }
    Ok(value.trim().trim_end_matches('/').to_string())
}

fn validate_llm_endpoint_request(
    body: &CreateLlmEndpointRequest,
) -> Result<(), WorkbenchHttpError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint name must be 1..=80 bytes".into(),
        )));
    }
    if !matches!(
        body.kind.as_str(),
        "vllm" | "sglang" | "openai_compatible" | "anthropic_proxy" | "ccr"
    ) {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint kind must be vllm, sglang, openai_compatible, anthropic_proxy, or ccr".into(),
        )));
    }
    if !matches!(
        body.protocol.as_str(),
        "openai_chat" | "openai_responses" | "anthropic_messages"
    ) {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint protocol must be openai_chat, openai_responses, or anthropic_messages".into(),
        )));
    }
    validate_endpoint_base_url(&body.base_url)?;
    if body
        .model_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|model| model.len() > 256)
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint model_id must be at most 256 bytes".into(),
        )));
    }
    let target_kind = body
        .target_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("external");
    validate_llm_endpoint_target(target_kind, body.target_host_id)?;
    if body.listen_port == Some(0) {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint listen_port must be 1..=65535".into(),
        )));
    }
    validate_optional_env_name(body.auth_token_env.as_deref())?;
    Ok(())
}

fn validate_ccr_bridge_request(body: &CreateCcrBridgeRequest) -> Result<(), WorkbenchHttpError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "CCR bridge name must be 1..=80 bytes".into(),
        )));
    }
    validate_llm_endpoint_target(body.target_kind.trim(), body.target_host_id)?;
    validate_endpoint_base_url(&body.base_url)?;
    if body.port == 0 {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "CCR bridge port must be 1..=65535".into(),
        )));
    }
    validate_optional_env_name(body.auth_token_env.as_deref())?;
    Ok(())
}

fn validate_llm_endpoint_target(
    target_kind: &str,
    target_host_id: Option<Uuid>,
) -> Result<(), WorkbenchHttpError> {
    match target_kind {
        "external" => {
            if target_host_id.is_some() {
                return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                    "external endpoints must not include target_host_id".into(),
                )));
            }
        }
        "local" => {
            if target_host_id.is_some() {
                return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                    "local endpoints must not include target_host_id".into(),
                )));
            }
        }
        "registered_server" => {
            if target_host_id.is_none() {
                return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                    "registered_server endpoints require target_host_id".into(),
                )));
            }
        }
        _ => {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "endpoint target_kind must be external, local, or registered_server".into(),
            )));
        }
    }
    Ok(())
}

fn validate_optional_env_name(value: Option<&str>) -> Result<(), WorkbenchHttpError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    if value.len() > 128 {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "auth token env var name must be at most 128 bytes".into(),
        )));
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Ok(());
    };
    if !(first == '_' || first.is_ascii_uppercase())
        || chars.any(|c| !(c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit()))
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "auth token env var name must match [A-Z_][A-Z0-9_]*".into(),
        )));
    }
    Ok(())
}

fn validate_autodetect_request(
    body: &AutoDetectLlmEndpointRequest,
) -> Result<String, WorkbenchHttpError> {
    let host = body.host.trim();
    if host.is_empty() || host.len() > 253 {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint host must be 1..=253 bytes".into(),
        )));
    }
    if body.port == 0 {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint port must be 1..=65535".into(),
        )));
    }
    let scheme = body
        .scheme
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("http");
    if !matches!(scheme, "http" | "https") {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint scheme must be http or https".into(),
        )));
    }
    if normalize_optional_text(body.alias.clone())
        .as_deref()
        .is_some_and(|alias| alias.len() > 80)
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint alias must be at most 80 bytes".into(),
        )));
    }
    validate_optional_env_name(body.auth_token_env.as_deref())?;
    if body
        .model_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|model| model.len() > 256)
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint model_id must be at most 256 bytes".into(),
        )));
    }
    if body
        .auth_token_value
        .as_deref()
        .is_some_and(|value| value.contains('\0'))
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint auth token must not contain NUL bytes".into(),
        )));
    }
    validate_endpoint_base_url(&format!("{scheme}://{host}:{}", body.port))
}

fn llm_endpoint_error_to_http(
    op: &str,
    err: gadgetron_xaas::llm_endpoints::LlmEndpointError,
) -> WorkbenchHttpError {
    match err {
        gadgetron_xaas::llm_endpoints::LlmEndpointError::NotFound => {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("{op}: endpoint not found")))
        }
        other => WorkbenchHttpError::Core(GadgetronError::Config(format!("{op}: {other}"))),
    }
}

fn openai_models_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

fn openai_responses_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/responses")
    } else {
        format!("{base}/v1/responses")
    }
}

fn anthropic_messages_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

fn codex_runtime_base_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        base.to_string()
    } else {
        format!("{base}/v1")
    }
}

fn endpoint_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        // A first local-model request may include model cold-load plus CPU
        // reasoning before it emits the required function_call. This route is
        // Management-scoped and explicitly invoked, so allow a bounded probe
        // window instead of misclassifying a slow Responses endpoint as
        // unsupported after the generic 20-second HTTP timeout.
        .timeout(Duration::from_secs(180))
        // A registered URL is an SSRF-capable egress target. Never follow a
        // target-controlled redirect to metadata or another network zone.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

fn with_openai_auth(
    request: reqwest::RequestBuilder,
    auth_token: Option<&str>,
) -> reqwest::RequestBuilder {
    match auth_token.filter(|value| !value.trim().is_empty()) {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

fn with_anthropic_auth(
    request: reqwest::RequestBuilder,
    auth_token: Option<&str>,
) -> reqwest::RequestBuilder {
    match auth_token.filter(|value| !value.trim().is_empty()) {
        Some(token) => request.bearer_auth(token).header("x-api-key", token),
        None => request,
    }
}

async fn response_json(
    response: reqwest::Response,
    operation: &str,
) -> Result<serde_json::Value, String> {
    const MAX_PROBE_BODY: usize = 1024 * 1024;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("read {operation} response: {error}"))?;
    if bytes.len() > MAX_PROBE_BODY {
        return Err(format!("{operation} response exceeded 1 MiB"));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {operation}: {error}"))
}

async fn probe_openai_base_url(
    base_url: &str,
    auth_token: Option<&str>,
) -> Result<(Vec<String>, i32), String> {
    let client = endpoint_http_client()?;
    let started = Instant::now();
    let url = openai_models_url(base_url);
    let res = with_openai_auth(client.get(&url), auth_token)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("GET {url}: HTTP {}", res.status()));
    }
    let body: OpenAiModelsResponse =
        serde_json::from_value(response_json(res, "/v1/models").await?)
            .map_err(|error| format!("parse /v1/models model list: {error}"))?;
    let mut models = body
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| {
            !model.is_empty()
                && model.len() <= 256
                && !model.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
        })
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    let elapsed = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
    Ok((models, elapsed))
}

#[derive(Debug)]
enum ToolRouteProbe {
    Passed { status: u16 },
    Failed { status: Option<u16>, error: String },
    Absent { status: u16 },
}

async fn probe_openai_responses_tool(
    base_url: &str,
    model_id: &str,
    auth_token: Option<&str>,
) -> ToolRouteProbe {
    let client = match endpoint_http_client() {
        Ok(client) => client,
        Err(error) => {
            return ToolRouteProbe::Failed {
                status: None,
                error,
            }
        }
    };
    let url = openai_responses_url(base_url);
    let body = json!({
        "model": model_id,
        "input": "You must call gadgetron_capability_probe exactly once.",
        // Small reasoning models may spend several hundred tokens deciding
        // how to honor a required tool call. Keep the probe bounded without
        // truncating the function_call item itself.
        "max_output_tokens": 1024,
        "tools": [{
            "type": "function",
            "name": "gadgetron_capability_probe",
            "description": "A no-op capability probe. Call it exactly once.",
            "parameters": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }],
        "tool_choice": "required"
    });
    let response = match with_openai_auth(client.post(&url), auth_token)
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return ToolRouteProbe::Failed {
                status: None,
                error: format!("POST {url}: {error}"),
            }
        }
    };
    let status = response.status().as_u16();
    if matches!(status, 404 | 405) {
        return ToolRouteProbe::Absent { status };
    }
    if !response.status().is_success() {
        return ToolRouteProbe::Failed {
            status: Some(status),
            error: format!("POST {url}: HTTP {status}"),
        };
    }
    let payload = match response_json(response, "/v1/responses").await {
        Ok(payload) => payload,
        Err(error) => {
            return ToolRouteProbe::Failed {
                status: Some(status),
                error,
            }
        }
    };
    let passed = payload
        .get("output")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
                    && item.get("name").and_then(serde_json::Value::as_str)
                        == Some("gadgetron_capability_probe")
            })
        });
    if passed {
        ToolRouteProbe::Passed { status }
    } else {
        ToolRouteProbe::Failed {
            status: Some(status),
            error: "Responses request succeeded but returned no gadgetron_capability_probe function_call"
                .into(),
        }
    }
}

#[derive(Debug)]
struct EndpointCapabilityProbe {
    protocol: &'static str,
    models: Vec<String>,
    model_id: Option<String>,
    latency_ms: i32,
    runtime_compatibility: &'static str,
    tool_status: &'static str,
    tool_model_id: Option<String>,
    tool_error: Option<String>,
    details: serde_json::Value,
    message: String,
}

impl EndpointCapabilityProbe {
    fn ready(&self) -> bool {
        self.tool_status == "passed"
    }
}

async fn probe_openai_endpoint(
    base_url: &str,
    preferred_model: Option<&str>,
    auth_token: Option<&str>,
) -> Result<EndpointCapabilityProbe, String> {
    let started = Instant::now();
    let (models, models_latency_ms) = probe_openai_base_url(base_url, auth_token).await?;
    let model_id = preferred_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| models.first().cloned());
    let Some(model_id) = model_id else {
        return Ok(EndpointCapabilityProbe {
            protocol: "openai_chat",
            models,
            model_id: None,
            latency_ms: models_latency_ms,
            runtime_compatibility: "unverified",
            tool_status: "untested",
            tool_model_id: None,
            tool_error: Some("No model id was returned; select a model and probe again".into()),
            details: json!({"models_reachable": true, "responses_tool_call": null}),
            message: "Connected to /v1/models, but no model is available for a tool smoke".into(),
        });
    };
    let probe = probe_openai_responses_tool(base_url, &model_id, auth_token).await;
    let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
    match probe {
        ToolRouteProbe::Passed { status } => Ok(EndpointCapabilityProbe {
            protocol: "openai_responses",
            models,
            model_id: Some(model_id.clone()),
            latency_ms,
            runtime_compatibility: "codex_exec",
            tool_status: "passed",
            tool_model_id: Some(model_id),
            tool_error: None,
            details: json!({"models_reachable": true, "responses_status": status, "responses_tool_call": true}),
            message: "Connected; OpenAI Responses function call passed; ready for Codex Exec".into(),
        }),
        ToolRouteProbe::Absent { status } => Ok(EndpointCapabilityProbe {
            protocol: "openai_chat",
            models,
            model_id: Some(model_id.clone()),
            latency_ms,
            runtime_compatibility: "bridge_required",
            tool_status: "unsupported",
            tool_model_id: Some(model_id),
            tool_error: Some(format!("Responses route absent (HTTP {status})")),
            details: json!({"models_reachable": true, "responses_status": status, "responses_tool_call": false}),
            message: "Connected, but this is Chat Completions-only; register and start an Anthropic bridge"
                .into(),
        }),
        ToolRouteProbe::Failed { status, error } => Ok(EndpointCapabilityProbe {
            protocol: if status.is_some() { "openai_responses" } else { "openai_chat" },
            models,
            model_id: Some(model_id.clone()),
            latency_ms,
            runtime_compatibility: if status.is_some() { "codex_exec" } else { "unverified" },
            tool_status: "failed",
            tool_model_id: Some(model_id),
            tool_error: Some(error.clone()),
            details: json!({"models_reachable": true, "responses_status": status, "responses_tool_call": false}),
            message: format!("Connected, but Responses function-call smoke failed: {error}"),
        }),
    }
}

async fn probe_anthropic_health(base_url: &str, auth_token: Option<&str>) -> Result<i32, String> {
    let client = endpoint_http_client()?;
    let started = Instant::now();
    let base = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{base}/health");
    let res = with_anthropic_auth(client.get(&url), auth_token)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("GET {url}: HTTP {}", res.status()));
    }
    let elapsed = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
    Ok(elapsed)
}

async fn probe_anthropic_endpoint(
    base_url: &str,
    preferred_model: Option<&str>,
    auth_token: Option<&str>,
) -> Result<EndpointCapabilityProbe, String> {
    let started = Instant::now();
    let Some(model_id) = preferred_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
    else {
        let latency_ms = probe_anthropic_health(base_url, auth_token).await?;
        return Ok(EndpointCapabilityProbe {
            protocol: "anthropic_messages",
            models: Vec::new(),
            model_id: None,
            latency_ms,
            runtime_compatibility: "unverified",
            tool_status: "untested",
            tool_model_id: None,
            tool_error: Some(
                "Anthropic-compatible endpoints require a model id for tool smoke".into(),
            ),
            details: json!({"health_reachable": true, "messages_tool_use": null}),
            message: "Connected to /health; enter a model id and probe to verify tool use".into(),
        });
    };
    let client = endpoint_http_client()?;
    let url = anthropic_messages_url(base_url);
    let body = json!({
        "model": model_id,
        "max_tokens": 64,
        "messages": [{
            "role": "user",
            "content": "Call gadgetron_capability_probe exactly once with nonce endpoint-smoke."
        }],
        "tools": [{
            "name": "gadgetron_capability_probe",
            "description": "A no-op capability probe. Call it exactly once with the supplied nonce.",
            "input_schema": {
                "type": "object",
                "properties": { "nonce": { "type": "string" } },
                "required": ["nonce"],
                "additionalProperties": false
            }
        }],
        "tool_choice": {"type": "tool", "name": "gadgetron_capability_probe"}
    });
    let response = with_anthropic_auth(client.post(&url), auth_token)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("POST {url}: {error}"))?;
    let status = response.status().as_u16();
    if matches!(status, 404 | 405) {
        return Err(format!("POST {url}: HTTP {status} (Messages route absent)"));
    }
    let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
    if !response.status().is_success() {
        let error = format!("POST {url}: HTTP {status}");
        return Ok(EndpointCapabilityProbe {
            protocol: "anthropic_messages",
            models: vec![model_id.clone()],
            model_id: Some(model_id.clone()),
            latency_ms,
            runtime_compatibility: "claude_code",
            tool_status: "failed",
            tool_model_id: Some(model_id),
            tool_error: Some(error.clone()),
            details: json!({"messages_status": status, "messages_tool_use": false}),
            message: format!("Messages route found, but tool-use smoke failed: {error}"),
        });
    }
    let payload = response_json(response, "/v1/messages").await?;
    let passed = payload
        .get("content")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("tool_use")
                    && item.get("name").and_then(serde_json::Value::as_str)
                        == Some("gadgetron_capability_probe")
            })
        });
    let error = (!passed).then(|| {
        "Messages request succeeded but returned no gadgetron_capability_probe tool_use".to_string()
    });
    Ok(EndpointCapabilityProbe {
        protocol: "anthropic_messages",
        models: vec![model_id.clone()],
        model_id: Some(model_id.clone()),
        latency_ms,
        runtime_compatibility: "claude_code",
        tool_status: if passed { "passed" } else { "failed" },
        tool_model_id: Some(model_id),
        tool_error: error.clone(),
        details: json!({"messages_status": status, "messages_tool_use": passed}),
        message: if passed {
            "Connected; Anthropic Messages tool use passed; ready for Claude Code".into()
        } else {
            format!(
                "Connected, but Messages tool-use smoke failed: {}",
                error.unwrap_or_default()
            )
        },
    })
}

async fn probe_endpoint_capabilities(
    base_url: &str,
    preferred_protocol: Option<&str>,
    preferred_model: Option<&str>,
    auth_token: Option<&str>,
) -> Result<EndpointCapabilityProbe, String> {
    if preferred_protocol == Some("anthropic_messages") {
        return probe_anthropic_endpoint(base_url, preferred_model, auth_token).await;
    }
    match probe_openai_endpoint(base_url, preferred_model, auth_token).await {
        Ok(probe) => Ok(probe),
        Err(openai_error) => probe_anthropic_endpoint(base_url, preferred_model, auth_token)
            .await
            .map_err(|anthropic_error| {
                format!(
                    "OpenAI probe failed: {openai_error}; Anthropic probe failed: {anthropic_error}"
                )
            }),
    }
}

pub async fn list_llm_endpoints_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<ListLlmEndpointsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "llm endpoint listing")?;
    let endpoints = gadgetron_xaas::llm_endpoints::list_llm_endpoints(pool, ctx.tenant_id)
        .await
        .map_err(|e| llm_endpoint_error_to_http("list_llm_endpoints", e))?;
    let returned = endpoints.len();
    Ok(Json(ListLlmEndpointsResponse {
        endpoints,
        returned,
    }))
}

/// Safe, non-secret projection used by the per-chat model selector. Only an
/// endpoint/model pair with an actual tool-call pass is exposed.
pub async fn list_available_llm_endpoint_models_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<AvailableLlmEndpointModelsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "available LLM endpoint listing")?;
    let endpoints = gadgetron_xaas::llm_endpoints::list_llm_endpoints(pool, ctx.tenant_id)
        .await
        .map_err(|error| llm_endpoint_error_to_http("list_llm_endpoints", error))?;
    let models = endpoints
        .into_iter()
        .filter(|endpoint| endpoint.health_status == "ok" && endpoint.tool_status == "passed")
        .filter_map(|endpoint| {
            let backend = match endpoint.runtime_compatibility.as_str() {
                "codex_exec" if endpoint.protocol == "openai_responses" => "codex_exec",
                "claude_code" if endpoint.protocol == "anthropic_messages" => "claude_code",
                _ => return None,
            };
            let model_id = endpoint.tool_model_id?;
            Some(AvailableLlmEndpointModel {
                endpoint_id: endpoint.id,
                endpoint_name: endpoint.name,
                backend,
                protocol: endpoint.protocol,
                model_id,
            })
        })
        .collect();
    Ok(Json(AvailableLlmEndpointModelsResponse { models }))
}

/// Resolve a client-selected registry id to the canonical execution snapshot.
/// This is called by both profile PATCH and first-turn chat handling before
/// any URL/env metadata is trusted or persisted.
pub(crate) async fn canonicalize_registered_endpoint_profile(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    mut profile: ConversationAgentProfile,
) -> Result<ConversationAgentProfile, GadgetronError> {
    if profile.model_source != gadgetron_core::agent::ModelSource::Local {
        return Ok(profile);
    }
    let Some(endpoint_id) = profile.llm_endpoint_id else {
        // Legacy raw-config profiles remain valid only through the existing
        // exact default/current fingerprint check. New UI selections always
        // carry a registry id.
        return Ok(profile);
    };
    let endpoint = gadgetron_xaas::llm_endpoints::get_llm_endpoint(pool, tenant_id, endpoint_id)
        .await
        .map_err(|error| GadgetronError::Config(format!("local endpoint lookup: {error}")))?;
    if endpoint.health_status != "ok" || endpoint.tool_status != "passed" {
        return Err(GadgetronError::Config(
            "selected local endpoint is not Penny-ready; run its model tool probe first".into(),
        ));
    }
    let expected_backend = match endpoint.runtime_compatibility.as_str() {
        "codex_exec" if endpoint.protocol == "openai_responses" => {
            gadgetron_core::agent::AgentBackend::CodexExec
        }
        "claude_code" if endpoint.protocol == "anthropic_messages" => {
            gadgetron_core::agent::AgentBackend::ClaudeCode
        }
        "bridge_required" => {
            return Err(GadgetronError::Config(
                "selected endpoint is Chat Completions-only and requires a running bridge".into(),
            ))
        }
        _ => {
            return Err(GadgetronError::Config(
                "selected endpoint has no supported Penny runtime adapter".into(),
            ))
        }
    };
    if profile.backend != expected_backend {
        return Err(GadgetronError::Config(format!(
            "selected endpoint requires {} runtime",
            expected_backend.as_str()
        )));
    }
    let verified_model = endpoint.tool_model_id.ok_or_else(|| {
        GadgetronError::Config("selected endpoint has no tool-verified model id".into())
    })?;
    if !profile.model.trim().is_empty() && profile.model.trim() != verified_model {
        return Err(GadgetronError::Config(
            "selected model has not passed this endpoint's tool probe".into(),
        ));
    }
    profile.model = verified_model;
    profile.local_base_url = if expected_backend == gadgetron_core::agent::AgentBackend::CodexExec {
        codex_runtime_base_url(&endpoint.base_url)
    } else {
        endpoint.base_url
    };
    profile.local_api_key_env = endpoint.auth_token_env.unwrap_or_default();
    Ok(profile)
}

pub async fn create_llm_endpoint_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<CreateLlmEndpointRequest>,
) -> Result<Json<gadgetron_xaas::llm_endpoints::LlmEndpointRow>, WorkbenchHttpError> {
    validate_llm_endpoint_request(&body)?;
    let pool = require_pg_pool(&state, "llm endpoint creation")?;
    let model_id = normalize_optional_text(body.model_id);
    let target_kind = body
        .target_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("external");
    let auth_token_env = normalize_optional_text(body.auth_token_env);
    let row = gadgetron_xaas::llm_endpoints::create_llm_endpoint_with_target(
        pool,
        ctx.tenant_id,
        gadgetron_xaas::llm_endpoints::LlmEndpointCreate {
            name: body.name.trim(),
            kind: body.kind.trim(),
            protocol: body.protocol.trim(),
            base_url: body.base_url.trim().trim_end_matches('/'),
            target_kind,
            target_host_id: body.target_host_id,
            upstream_endpoint_id: body.upstream_endpoint_id,
            listen_port: body.listen_port.map(i32::from),
            auth_token_env: auth_token_env.as_deref(),
            model_id: model_id.as_deref(),
        },
    )
    .await
    .map_err(|e| llm_endpoint_error_to_http("create_llm_endpoint", e))?;
    Ok(Json(row))
}

pub async fn create_ccr_bridge_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(upstream_endpoint_id): axum::extract::Path<uuid::Uuid>,
    Json(body): Json<CreateCcrBridgeRequest>,
) -> Result<Json<gadgetron_xaas::llm_endpoints::LlmEndpointRow>, WorkbenchHttpError> {
    validate_ccr_bridge_request(&body)?;
    let pool = require_pg_pool(&state, "CCR bridge creation")?;
    let upstream =
        gadgetron_xaas::llm_endpoints::get_llm_endpoint(pool, ctx.tenant_id, upstream_endpoint_id)
            .await
            .map_err(|e| llm_endpoint_error_to_http("get_llm_endpoint", e))?;
    if !matches!(
        upstream.protocol.as_str(),
        "openai_chat" | "openai_responses"
    ) {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "CCR bridge upstream must be an OpenAI-compatible endpoint".into(),
        )));
    }
    let auth_token_env = normalize_optional_text(body.auth_token_env);
    let row = gadgetron_xaas::llm_endpoints::create_llm_endpoint_with_target(
        pool,
        ctx.tenant_id,
        gadgetron_xaas::llm_endpoints::LlmEndpointCreate {
            name: body.name.trim(),
            kind: "ccr",
            protocol: "anthropic_messages",
            base_url: body.base_url.trim().trim_end_matches('/'),
            target_kind: body.target_kind.trim(),
            target_host_id: body.target_host_id,
            upstream_endpoint_id: Some(upstream.id),
            listen_port: Some(i32::from(body.port)),
            auth_token_env: auth_token_env.as_deref(),
            model_id: upstream.model_id.as_deref(),
        },
    )
    .await
    .map_err(|e| llm_endpoint_error_to_http("create_ccr_bridge", e))?;
    Ok(Json(row))
}

async fn persist_endpoint_capability(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
    probe: &EndpointCapabilityProbe,
) -> Result<gadgetron_xaas::llm_endpoints::LlmEndpointRow, WorkbenchHttpError> {
    gadgetron_xaas::llm_endpoints::update_llm_endpoint_capability(
        pool,
        tenant_id,
        endpoint_id,
        gadgetron_xaas::llm_endpoints::LlmEndpointCapabilityUpdate {
            protocol: probe.protocol,
            model_id: probe.model_id.as_deref(),
            discovered_models: &probe.models,
            health_status: "ok",
            last_error: None,
            last_latency_ms: Some(probe.latency_ms),
            runtime_compatibility: probe.runtime_compatibility,
            tool_status: probe.tool_status,
            tool_model_id: probe.tool_model_id.as_deref(),
            last_tool_error: probe.tool_error.as_deref(),
            capability_details: &probe.details,
        },
    )
    .await
    .map_err(|error| llm_endpoint_error_to_http("update_llm_endpoint_capability", error))
}

async fn persist_endpoint_probe_failure(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    endpoint_id: Uuid,
    protocol: &str,
    model_id: Option<&str>,
    message: &str,
) -> Result<gadgetron_xaas::llm_endpoints::LlmEndpointRow, WorkbenchHttpError> {
    let details = json!({"probe_error": true});
    gadgetron_xaas::llm_endpoints::update_llm_endpoint_capability(
        pool,
        tenant_id,
        endpoint_id,
        gadgetron_xaas::llm_endpoints::LlmEndpointCapabilityUpdate {
            protocol,
            model_id,
            discovered_models: &[],
            health_status: "error",
            last_error: Some(message),
            last_latency_ms: None,
            runtime_compatibility: "unverified",
            tool_status: "untested",
            tool_model_id: None,
            last_tool_error: None,
            capability_details: &details,
        },
    )
    .await
    .map_err(|error| llm_endpoint_error_to_http("update_llm_endpoint_capability", error))
}

pub async fn autodetect_llm_endpoint_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<AutoDetectLlmEndpointRequest>,
) -> Result<Json<AutoDetectLlmEndpointResponse>, WorkbenchHttpError> {
    let base_url = validate_autodetect_request(&body)?;
    let alias = endpoint_alias_or_host_port(body.alias.clone(), &body.host, body.port);
    let preferred_model = normalize_optional_text(body.model_id.clone());
    let (auth_token_env, auth_token) = resolve_endpoint_auth(
        body.auth_token_env.clone(),
        body.auth_token_value.clone(),
        DEFAULT_PENNY_OPENAI_AUTH_TOKEN_ENV,
    )?;
    let pool = require_pg_pool(&state, "llm endpoint autodetect")?;

    match probe_endpoint_capabilities(
        &base_url,
        None,
        preferred_model.as_deref(),
        auth_token.as_deref(),
    )
    .await
    {
        Ok(probe) => {
            let kind = if probe.protocol == "anthropic_messages" {
                "anthropic_proxy"
            } else {
                "openai_compatible"
            };
            let endpoint = gadgetron_xaas::llm_endpoints::upsert_llm_endpoint_by_name(
                pool,
                ctx.tenant_id,
                gadgetron_xaas::llm_endpoints::LlmEndpointUpsert {
                    name: &alias,
                    kind,
                    protocol: probe.protocol,
                    base_url: &base_url,
                    auth_token_env: auth_token_env.as_deref(),
                    model_id: probe.model_id.as_deref(),
                },
            )
            .await
            .map_err(|e| llm_endpoint_error_to_http("autodetect_llm_endpoint", e))?;
            let endpoint =
                persist_endpoint_capability(pool, ctx.tenant_id, endpoint.id, &probe).await?;
            Ok(Json(AutoDetectLlmEndpointResponse {
                ok: probe.ready(),
                endpoint,
                models: probe.models,
                message: probe.message,
            }))
        }
        Err(message) => {
            let endpoint = gadgetron_xaas::llm_endpoints::upsert_llm_endpoint_by_name(
                pool,
                ctx.tenant_id,
                gadgetron_xaas::llm_endpoints::LlmEndpointUpsert {
                    name: &alias,
                    kind: "openai_compatible",
                    protocol: "openai_chat",
                    base_url: &base_url,
                    auth_token_env: auth_token_env.as_deref(),
                    model_id: preferred_model.as_deref(),
                },
            )
            .await
            .map_err(|error| llm_endpoint_error_to_http("autodetect_llm_endpoint", error))?;
            let endpoint = persist_endpoint_probe_failure(
                pool,
                ctx.tenant_id,
                endpoint.id,
                "openai_chat",
                preferred_model.as_deref(),
                &message,
            )
            .await?;
            Ok(Json(AutoDetectLlmEndpointResponse {
                ok: false,
                endpoint,
                models: Vec::new(),
                message,
            }))
        }
    }
}

pub async fn delete_llm_endpoint_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(endpoint_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<DeleteLlmEndpointResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "llm endpoint deletion")?;
    gadgetron_xaas::llm_endpoints::delete_llm_endpoint(pool, ctx.tenant_id, endpoint_id)
        .await
        .map_err(|e| llm_endpoint_error_to_http("delete_llm_endpoint", e))?;
    Ok(Json(DeleteLlmEndpointResponse {
        deleted: true,
        endpoint_id,
    }))
}

pub async fn probe_llm_endpoint_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(endpoint_id): axum::extract::Path<uuid::Uuid>,
    body: Option<Json<ProbeLlmEndpointRequest>>,
) -> Result<Json<ProbeLlmEndpointResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "llm endpoint probe")?;
    let endpoint =
        gadgetron_xaas::llm_endpoints::get_llm_endpoint(pool, ctx.tenant_id, endpoint_id)
            .await
            .map_err(|e| llm_endpoint_error_to_http("get_llm_endpoint", e))?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let preferred_model =
        normalize_optional_text(body.model_id).or_else(|| endpoint.model_id.clone());
    let default_auth_env = if endpoint.protocol == "anthropic_messages" {
        DEFAULT_PENNY_EXTERNAL_AUTH_TOKEN_ENV
    } else {
        DEFAULT_PENNY_OPENAI_AUTH_TOKEN_ENV
    };
    let (auth_token_env, auth_token) = resolve_endpoint_auth(
        endpoint.auth_token_env.clone(),
        body.auth_token_value,
        default_auth_env,
    )?;
    if auth_token_env != endpoint.auth_token_env {
        gadgetron_xaas::llm_endpoints::update_llm_endpoint_auth_env(
            pool,
            ctx.tenant_id,
            endpoint_id,
            auth_token_env.as_deref(),
        )
        .await
        .map_err(|error| llm_endpoint_error_to_http("update_llm_endpoint_auth_env", error))?;
    }

    match probe_endpoint_capabilities(
        &endpoint.base_url,
        Some(&endpoint.protocol),
        preferred_model.as_deref(),
        auth_token.as_deref(),
    )
    .await
    {
        Ok(probe) => {
            let next =
                persist_endpoint_capability(pool, ctx.tenant_id, endpoint_id, &probe).await?;
            Ok(Json(ProbeLlmEndpointResponse {
                ok: probe.ready(),
                endpoint: next,
                models: probe.models,
                message: probe.message,
            }))
        }
        Err(message) => {
            let next = persist_endpoint_probe_failure(
                pool,
                ctx.tenant_id,
                endpoint_id,
                &endpoint.protocol,
                preferred_model.as_deref(),
                &message,
            )
            .await?;
            Ok(Json(ProbeLlmEndpointResponse {
                ok: false,
                endpoint: next,
                models: Vec::new(),
                message,
            }))
        }
    }
}

pub async fn use_llm_endpoint_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(endpoint_id): axum::extract::Path<uuid::Uuid>,
    body: Option<Json<UseLlmEndpointRequest>>,
) -> Result<Json<UseLlmEndpointResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let brain = svc.agent_brain.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "agent brain settings are not wired in this build".into(),
        ))
    })?;
    let pool = require_pg_pool(&state, "llm endpoint use")?;
    let endpoint =
        gadgetron_xaas::llm_endpoints::get_llm_endpoint(pool, ctx.tenant_id, endpoint_id)
            .await
            .map_err(|e| llm_endpoint_error_to_http("get_llm_endpoint", e))?;
    if endpoint.runtime_compatibility == "bridge_required" {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "This endpoint is Chat Completions-only. Register and start an Anthropic bridge, then run its tool probe before selecting it for Penny."
                .into(),
        )));
    }
    if endpoint.health_status != "ok" || endpoint.tool_status != "passed" {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint is connected but not Penny-ready; run a model tool probe and fix the reported error first"
                .into(),
        )));
    }

    let body = body
        .map(|Json(body)| body)
        .unwrap_or(UseLlmEndpointRequest {
            model_id: None,
            external_auth_token_value: None,
        });
    let selected_model = normalize_optional_text(body.model_id)
        .or_else(|| endpoint.tool_model_id.clone())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "endpoint has no tool-verified model id".into(),
            ))
        })?;
    if endpoint.tool_model_id.as_deref() != Some(selected_model.as_str()) {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "selected model has not passed this endpoint's tool probe; probe that model first"
                .into(),
        )));
    }
    let is_anthropic = endpoint.runtime_compatibility == "claude_code";
    let expected_protocol = if is_anthropic {
        "anthropic_messages"
    } else {
        "openai_responses"
    };
    if endpoint.protocol != expected_protocol
        || !matches!(
            endpoint.runtime_compatibility.as_str(),
            "claude_code" | "codex_exec"
        )
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "endpoint protocol and runtime compatibility do not form a supported Penny adapter"
                .into(),
        )));
    }
    let default_auth_env = if is_anthropic {
        DEFAULT_PENNY_EXTERNAL_AUTH_TOKEN_ENV
    } else {
        DEFAULT_PENNY_OPENAI_AUTH_TOKEN_ENV
    };
    let (auth_token_env, _) = resolve_endpoint_auth(
        endpoint.auth_token_env.clone(),
        body.external_auth_token_value,
        default_auth_env,
    )?;
    if auth_token_env != endpoint.auth_token_env {
        gadgetron_xaas::llm_endpoints::update_llm_endpoint_auth_env(
            pool,
            ctx.tenant_id,
            endpoint_id,
            auth_token_env.as_deref(),
        )
        .await
        .map_err(|error| llm_endpoint_error_to_http("update_llm_endpoint_auth_env", error))?;
    }
    let external_auth_token_env = auth_token_env.unwrap_or_default();

    let current_agent = brain.load_full();
    let endpoint_backend = if is_anthropic {
        gadgetron_core::agent::AgentBackend::ClaudeCode
    } else {
        gadgetron_core::agent::AgentBackend::CodexExec
    };
    let request = gadgetron_core::agent::UpdateAgentBrainSettingsRequest {
        mode: if is_anthropic {
            gadgetron_core::agent::BrainMode::ExternalProxy
        } else {
            gadgetron_core::agent::BrainMode::ClaudeMax
        },
        external_base_url: if is_anthropic {
            endpoint.base_url.clone()
        } else {
            String::new()
        },
        model: selected_model.clone(),
        external_auth_token_env: if is_anthropic {
            external_auth_token_env.clone()
        } else {
            String::new()
        },
        custom_model_option: endpoint.model_id.is_some(),
        backend: endpoint_backend,
        llm_endpoint_id: Some(endpoint.id),
        model_source: gadgetron_core::agent::ModelSource::Local,
        local_base_url: if is_anthropic {
            endpoint.base_url.clone()
        } else {
            codex_runtime_base_url(&endpoint.base_url)
        },
        local_api_key_env: external_auth_token_env.clone(),
        effort: current_agent
            .brain
            .effort
            .for_backend_model(endpoint_backend, &selected_model),
    };
    let next_agent = request.overlay_agent(&current_agent);
    next_agent
        .validate_with_env(
            &std::collections::HashMap::new(),
            &gadgetron_core::agent::config::StdEnv,
        )
        .map_err(WorkbenchHttpError::Core)?;
    let saved = gadgetron_xaas::agent_brain::upsert_agent_brain_settings(
        pool,
        ctx.tenant_id,
        ctx.actor_user_id,
        &request,
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "agent brain settings update: {e}"
        )))
    })?;
    brain.store(Arc::new(next_agent));
    Ok(Json(UseLlmEndpointResponse {
        endpoint,
        brain: saved,
    }))
}

/// Map `ApprovalError` → `WorkbenchHttpError` keeping the status-code
/// contract callers rely on.
fn approval_error_to_http(err: gadgetron_core::workbench::ApprovalError) -> WorkbenchHttpError {
    use gadgetron_core::workbench::ApprovalError as E;
    match err {
        E::NotFound => WorkbenchHttpError::ApprovalNotFound,
        E::AlreadyResolved { current_state } => WorkbenchHttpError::ApprovalAlreadyResolved {
            state: current_state.as_str().to_string(),
        },
        E::CrossTenant => WorkbenchHttpError::ApprovalForbidden,
        E::Backend(msg) => WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "approval store error: {msg}"
        ))),
    }
}

/// Query parameters for `GET /usage/summary`.
#[derive(Debug, Deserialize, Default)]
pub struct UsageSummaryQuery {
    /// Hours of history to aggregate over. Default 24, max 168 (one week).
    pub window_hours: Option<i32>,
}

/// Response shape for `GET /usage/summary` — per-tenant rollup over
/// a sliding time window across the three audit planes:
/// `audit_log` (chat), `action_audit_events` (workbench direct actions),
/// `tool_audit_events` (Penny tool calls).
#[derive(Debug, serde::Serialize)]
pub struct UsageSummaryResponse {
    pub window_hours: i32,
    pub chat: UsageChatStats,
    pub actions: UsageActionStats,
    pub tools: UsageToolStats,
}

#[derive(Debug, serde::Serialize, Default)]
pub struct UsageChatStats {
    pub requests: i64,
    pub errors: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_cents: i64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, serde::Serialize, Default)]
pub struct UsageActionStats {
    pub total: i64,
    pub success: i64,
    pub error: i64,
    pub pending_approval: i64,
    pub avg_elapsed_ms: f64,
}

#[derive(Debug, serde::Serialize, Default)]
pub struct UsageToolStats {
    pub total: i64,
    pub errors: i64,
}

/// Per-window stats (daily or monthly) returned by
/// `GET /quota/status` response. Mirrors the `quota_configs`
/// column set with a precomputed `remaining_cents` so the UI doesn't
/// have to subtract on every render.
#[derive(Debug, serde::Serialize)]
pub struct QuotaWindowStats {
    pub used_cents: i64,
    pub limit_cents: i64,
    pub remaining_cents: i64,
}

/// Response for `GET /api/v1/web/workbench/quota/status`.
#[derive(Debug, serde::Serialize)]
pub struct QuotaStatusResponse {
    /// UTC date the `daily` counter refers to. When a new day
    /// starts, the counter zeros on the first request (via
    /// `PgQuotaEnforcer`'s CASE rollover) — this field lets the UI
    /// tell "your daily quota just reset" from "your daily quota
    /// is lightly used".
    pub usage_day: chrono::NaiveDate,
    pub daily: QuotaWindowStats,
    pub monthly: QuotaWindowStats,
}

/// `GET /api/v1/web/workbench/quota/status` — current-tenant quota
/// snapshot. Scope: `OpenAiCompat` because end-users checking their
/// own usage don't need Management.
///
/// Returns 503 when `pg_pool` isn't configured OR 404 when the
/// tenant has no `quota_configs` row (bootstrap bug — operator
/// should inspect).
pub async fn get_quota_status(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<QuotaStatusResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "quota status")?;
    // Projected columns include `usage_day` so the response reflects
    // the post-rollover state without the handler doing the math.
    // CASE zeroing is intentional: the operator saw rollover intent
    // in the migration; the UI should too.
    let row: Option<(i64, i64, i64, i64, chrono::NaiveDate)> = sqlx::query_as(
        r#"
        SELECT
            CASE WHEN usage_day = CURRENT_DATE THEN daily_used_cents ELSE 0 END,
            daily_limit_cents,
            CASE WHEN DATE_TRUNC('month', usage_day::timestamp)
                   = DATE_TRUNC('month', CURRENT_DATE::timestamp)
                THEN monthly_used_cents
                ELSE 0
            END,
            monthly_limit_cents,
            CASE WHEN usage_day = CURRENT_DATE THEN usage_day ELSE CURRENT_DATE END
        FROM quota_configs
        WHERE tenant_id = $1
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "quota status query failed: {e}"
        )))
    })?;

    // No quota_configs row = tenant hasn't been provisioned with an
    // explicit quota yet. Fall back to the schema defaults so the UI
    // renders "fresh tenant, full quota" instead of a 400. Future
    // work auto-inserts the default row on tenant create; until then,
    // this read-side default keeps the endpoint usable.
    let (daily_used, daily_limit, monthly_used, monthly_limit, usage_day) =
        row.unwrap_or((0, 1_000_000, 0, 10_000_000, chrono::Utc::now().date_naive()));

    Ok(Json(QuotaStatusResponse {
        usage_day,
        daily: QuotaWindowStats {
            used_cents: daily_used,
            limit_cents: daily_limit,
            remaining_cents: (daily_limit - daily_used).max(0),
        },
        monthly: QuotaWindowStats {
            used_cents: monthly_used,
            limit_cents: monthly_limit,
            remaining_cents: (monthly_limit - monthly_used).max(0),
        },
    }))
}

/// Query parameters for `GET /admin/billing/events`.
#[derive(Debug, Deserialize, Default)]
pub struct BillingEventsQuery {
    /// ISO-8601 timestamp. Only events with `created_at >= since`
    /// are returned. Absent = no lower bound.
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    /// Result cap (default 100, clamped `[1, 500]`).
    pub limit: Option<i64>,
}

/// Response shape for `GET /admin/billing/events`.
#[derive(Debug, serde::Serialize)]
pub struct BillingEventsResponse {
    pub events: Vec<gadgetron_xaas::billing::BillingEventRow>,
    pub returned: usize,
}

/// `GET /api/v1/web/workbench/admin/billing/events` — tenant-scoped
/// billing ledger query. **Management scope** because this is invoice
/// / billing data. Tenant boundary is
/// pinned by the handler — callers cannot read another tenant's
/// ledger regardless of query params.
///
/// Returns newest-first. Each row is one billable event.
pub async fn list_billing_events(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Query(query): axum::extract::Query<BillingEventsQuery>,
) -> Result<Json<BillingEventsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "billing events query")?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let events = gadgetron_xaas::billing::events::query_billing_events(
        pool,
        ctx.tenant_id,
        query.since,
        limit,
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "billing events query failed: {e}"
        )))
    })?;
    let returned = events.len();
    Ok(Json(BillingEventsResponse { events, returned }))
}

/// `GET /api/v1/web/workbench/admin/billing/insert-failures` —
/// process-local counter of billing-event INSERT failures per
/// `BillingEventKind`. **Management scope** (shares the
/// RBAC gate with `/admin/billing/events` because operators already
/// have Management to read the ledger).
///
/// Operators poll this for their SLO alert — any non-zero value
/// indicates the chat / tool / action ledger is diverging from the
/// `quota_configs` usage counters. The counter is in-memory and
/// resets on restart; long-horizon reconciliation is future work.
pub async fn admin_billing_insert_failures(
    State(state): State<AppState>,
    axum::Extension(_ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Json<gadgetron_xaas::billing::BillingFailureSnapshot> {
    Json(state.billing_failures.snapshot())
}

// ---------------------------------------------------------------------------
// Admin user CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, Default)]
pub struct ListUsersQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct ListUsersResponse {
    pub users: Vec<gadgetron_xaas::identity::UserRow>,
    pub returned: usize,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub display_name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub role: gadgetron_xaas::identity::Role,
    /// Plaintext password. Required for `member` + `admin`; MUST be
    /// absent for `service` (400 otherwise).
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateUserProfileRequest {
    pub display_name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// Optional. When provided, replaces the user's group memberships
    /// with this exact set (transactional). When absent, group
    /// memberships are not touched. Unknown ids fail the request
    /// before any mutation.
    #[serde(default)]
    pub group_ids: Option<Vec<String>>,
    /// Optional. When provided, updates the user's role (Admin /
    /// Member / Service). Demoting the last active admin is rejected
    /// with the same single-admin guard as DELETE.
    #[serde(default)]
    pub role: Option<gadgetron_xaas::identity::Role>,
}

#[derive(Debug, serde::Serialize)]
pub struct DeleteUserResponse {
    pub deleted: bool,
    pub user_id: uuid::Uuid,
}

/// `GET /api/v1/web/workbench/admin/users` — list users in the caller's tenant.
/// Management scope. Tenant boundary pinned by handler.
pub async fn list_users_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Query(query): axum::extract::Query<ListUsersQuery>,
) -> Result<Json<ListUsersResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "user listing")?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let users = gadgetron_xaas::identity::list_users(pool, ctx.tenant_id, limit)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("list_users failed: {e}")))
        })?;
    let returned = users.len();
    Ok(Json(ListUsersResponse { users, returned }))
}

/// `POST /api/v1/web/workbench/admin/users` — admin creates a user in
/// the caller's tenant.
pub async fn create_user_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<CreateUserRequest>,
) -> Result<Json<gadgetron_xaas::identity::UserRow>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "user creation")?;
    let row = gadgetron_xaas::identity::create_user(
        pool,
        ctx.tenant_id,
        &body.email,
        &body.display_name,
        body.avatar_url.as_deref(),
        body.role,
        body.password.as_deref(),
    )
    .await
    .map_err(|e| WorkbenchHttpError::Core(GadgetronError::Config(format!("create_user: {e}"))))?;
    Ok(Json(row))
}

/// `PATCH /api/v1/web/workbench/admin/users/{user_id}` — update editable
/// profile fields for an existing user in the caller's tenant.
pub async fn update_user_profile_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(user_id): axum::extract::Path<uuid::Uuid>,
    Json(body): Json<UpdateUserProfileRequest>,
) -> Result<Json<gadgetron_xaas::identity::UserRow>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "user profile update")?;

    if let Some(group_ids) = body.group_ids.as_deref() {
        gadgetron_xaas::groups::sync_user_groups(
            pool,
            ctx.tenant_id,
            user_id,
            group_ids,
            ctx.actor_user_id,
        )
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("sync_user_groups: {e}")))
        })?;
    }

    if let Some(new_role) = body.role {
        gadgetron_xaas::identity::update_user_role(pool, ctx.tenant_id, user_id, new_role)
            .await
            .map_err(|e| match e {
                gadgetron_xaas::identity::IdentityError::LastAdmin => WorkbenchHttpError::Core(
                    GadgetronError::Config("cannot demote the last active admin".to_string()),
                ),
                gadgetron_xaas::identity::IdentityError::NotFound => {
                    WorkbenchHttpError::Core(GadgetronError::Config("user not found".to_string()))
                }
                _ => WorkbenchHttpError::Core(GadgetronError::Config(format!(
                    "update_user_role: {e}"
                ))),
            })?;
    }

    let row = gadgetron_xaas::identity::update_user_profile(
        pool,
        ctx.tenant_id,
        user_id,
        body.display_name.trim(),
        body.avatar_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!("update_user_profile: {e}")))
    })?;
    Ok(Json(row))
}

/// `DELETE /api/v1/web/workbench/admin/users/{user_id}` — with the
/// single-admin guard.
pub async fn delete_user_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(user_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<DeleteUserResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "user deletion")?;
    gadgetron_xaas::identity::delete_user(pool, ctx.tenant_id, user_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("delete_user: {e}")))
        })?;
    Ok(Json(DeleteUserResponse {
        deleted: true,
        user_id,
    }))
}

// ---------------------------------------------------------------------------
// Per-user chat conversations (left-rail sidebar).
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub struct ListConversationsResponse {
    pub conversations: Vec<gadgetron_xaas::conversations::ConversationRow>,
}

#[derive(Debug, serde::Serialize)]
pub struct ConversationAgentProfileResponse {
    pub profile: ConversationAgentProfile,
    /// False means this is only the current new-chat default projection. The
    /// first PATCH or chat turn atomically pins the profile to the row.
    pub pinned: bool,
}

fn default_conversation_profile(state: &AppState) -> ConversationAgentProfile {
    let agent = state
        .workbench
        .as_ref()
        .and_then(|service| service.agent_brain.as_ref())
        .map(|brain| brain.load_full())
        .unwrap_or_else(|| state.agent_config.clone());
    ConversationAgentProfile::from_agent(&agent)
}

fn conversation_profile_error_to_http(
    conversation_id: Uuid,
    error: gadgetron_xaas::conversations::ConversationError,
) -> WorkbenchHttpError {
    use gadgetron_xaas::conversations::ConversationError;
    match error {
        ConversationError::NotFound | ConversationError::OwnershipMismatch => {
            WorkbenchHttpError::Core(GadgetronError::Config("conversation not found".into()))
        }
        ConversationError::AgentBackendPinned { pinned, requested } => {
            WorkbenchHttpError::Core(GadgetronError::Penny {
                kind: PennyErrorKind::AgentBackendPinned {
                    conversation_id: conversation_id.to_string(),
                    pinned,
                    requested,
                },
                message: "conversation agent runtime is already pinned".into(),
            })
        }
        ConversationError::InvalidAgentProfile(reason) => {
            WorkbenchHttpError::Core(GadgetronError::Config(reason))
        }
        ConversationError::Db(error) => WorkbenchHttpError::Core(GadgetronError::Database {
            kind: DatabaseErrorKind::QueryFailed,
            message: format!("conversation agent profile: {error}"),
        }),
    }
}

/// Read a conversation's stored runtime/model/effort profile. A freshly
/// minted client-side conversation id has no row yet, so return the live
/// new-chat default with `pinned=false` instead of forcing an eager INSERT.
pub async fn get_conversation_agent_profile_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<ConversationAgentProfileResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "conversation agent profile")?;
    let user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::Core(GadgetronError::TenantNotFound))?;
    let saved = gadgetron_xaas::conversations::get_conversation_agent_profile(
        pool,
        id,
        ctx.tenant_id,
        user_id,
    )
    .await
    .map_err(|error| conversation_profile_error_to_http(id, error))?;
    let pinned = saved.is_some();
    Ok(Json(ConversationAgentProfileResponse {
        profile: saved.unwrap_or_else(|| default_conversation_profile(&state)),
        pinned,
    }))
}

/// Save per-chat model/effort and atomically pin its backend. A cross-runtime
/// PATCH returns HTTP 409; clients should offer to start a new conversation.
pub async fn patch_conversation_agent_profile_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Json(profile): Json<ConversationAgentProfile>,
) -> Result<Json<ConversationAgentProfileResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "conversation agent profile")?;
    let user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::Core(GadgetronError::TenantNotFound))?;

    // Keep the job's effective profile immutable for its entire generation,
    // even when a non-browser caller bypasses the disabled UI selector.
    if let Some(job) = state.chat_jobs.active_for_conversation(id).await {
        if job_visible_to(&job, &ctx) && !job.snapshot().await.is_finished {
            return Err(WorkbenchHttpError::Core(GadgetronError::Penny {
                kind: PennyErrorKind::SessionConcurrent {
                    conversation_id: id.to_string(),
                },
                message: "conversation already has an active generation".into(),
            }));
        }
    }

    let profile = canonicalize_registered_endpoint_profile(pool, ctx.tenant_id, profile)
        .await
        .map_err(WorkbenchHttpError::Core)?;
    let current = gadgetron_xaas::conversations::get_conversation_agent_profile(
        pool,
        id,
        ctx.tenant_id,
        user_id,
    )
    .await
    .map_err(|error| conversation_profile_error_to_http(id, error))?;
    if profile.llm_endpoint_id.is_none() {
        profile
            .validate_client_selection(current.as_ref(), &default_conversation_profile(&state))
            .map_err(|reason| WorkbenchHttpError::Core(GadgetronError::Config(reason)))?;
    }
    let saved = gadgetron_xaas::conversations::upsert_conversation_agent_profile(
        pool,
        id,
        ctx.tenant_id,
        user_id,
        &profile,
    )
    .await
    .map_err(|error| conversation_profile_error_to_http(id, error))?;
    Ok(Json(ConversationAgentProfileResponse {
        profile: saved,
        pinned: true,
    }))
}

/// `GET /api/v1/web/workbench/conversations` — list the calling user's
/// non-deleted conversations, newest first. Tenant + user boundary
/// enforced via the SQL WHERE clause.
pub async fn list_conversations_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<ListConversationsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "conversation listing")?;
    let user_id = match ctx.actor_user_id {
        Some(u) => u,
        None => {
            return Ok(Json(ListConversationsResponse {
                conversations: vec![],
            }))
        }
    };
    let rows = gadgetron_xaas::conversations::list_conversations_for_user(
        pool,
        ctx.tenant_id,
        user_id,
        200,
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!("list_conversations: {e}")))
    })?;
    Ok(Json(ListConversationsResponse {
        conversations: rows,
    }))
}

/// `DELETE /api/v1/web/workbench/conversations/{id}` — soft-delete.
/// Only the owner can delete; cross-user attempts 404.
pub async fn delete_conversation_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "conversation delete")?;
    let user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::Core(GadgetronError::TenantNotFound))?;
    gadgetron_xaas::conversations::delete_conversation(pool, ctx.tenant_id, user_id, id)
        .await
        .map_err(|e| match e {
            gadgetron_xaas::conversations::ConversationError::NotFound
            | gadgetron_xaas::conversations::ConversationError::OwnershipMismatch => {
                WorkbenchHttpError::Core(GadgetronError::Config("conversation not found".into()))
            }
            gadgetron_xaas::conversations::ConversationError::Db(e) => {
                WorkbenchHttpError::Core(GadgetronError::Config(format!("delete: {e}")))
            }
            gadgetron_xaas::conversations::ConversationError::AgentBackendPinned { .. }
            | gadgetron_xaas::conversations::ConversationError::InvalidAgentProfile(_) => {
                WorkbenchHttpError::Core(GadgetronError::Config(
                    "conversation profile conflict".into(),
                ))
            }
        })?;
    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}

#[derive(Debug, serde::Deserialize)]
pub struct RenameConversationRequest {
    pub title: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryBlock {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolUse {
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryMessage {
    pub role: String,
    /// Human-readable text-only projection of the message (legacy
    /// consumers + simple displays). Use `blocks` for structured render.
    pub content: String,
    pub blocks: Vec<HistoryBlock>,
    pub ts: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ConversationMessagesResponse {
    pub messages: Vec<HistoryMessage>,
}

/// `GET /api/v1/web/workbench/conversations/{id}/active-job` —
/// return JSON metadata for the in-flight or recently-completed
/// chat-completion job tied to this conversation. PostgreSQL-backed
/// deployments also return a recent terminal snapshot recovered after a
/// process restart. The frontend polls this on chat mount to
/// decide whether to attach to `/jobs/{job_id}/sync` and display the
/// "생성 중" indicator.
///
/// Tenant + user boundary: the job's stored `tenant_id` and
/// `user_id` must match the caller's `TenantContext`. Cross-tenant
/// inspection is a 404 (not a 403) so we don't leak the existence
/// of jobs in other tenants.
pub async fn get_conversation_active_job_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<crate::chat_jobs::JobSnapshot>, WorkbenchHttpError> {
    if let Some(job) = state.chat_jobs.active_for_conversation(id).await {
        if !job_visible_to(&job, &ctx) {
            return Err(WorkbenchHttpError::JobNotFound);
        }
        return Ok(Json(job.snapshot().await));
    }
    let Some(user_id) = ctx.actor_user_id else {
        return Err(WorkbenchHttpError::JobNotFound);
    };
    let snapshot = state
        .chat_jobs
        .latest_terminal_for_conversation(ctx.tenant_id, user_id, id)
        .await
        .map_err(|error| {
            WorkbenchHttpError::Core(GadgetronError::Database {
                kind: DatabaseErrorKind::QueryFailed,
                message: format!("chat job lookup: {error}"),
            })
        })?;
    snapshot.map(Json).ok_or(WorkbenchHttpError::JobNotFound)
}

/// `GET /api/v1/web/workbench/jobs/{job_id}/sync?since=N` —
/// SSE replay + live tail of a chat-completion job's chunk buffer.
/// `since` defaults to 0 (replay from the start). The response is
/// the same byte-for-byte SSE stream the original
/// `POST /v1/chat/completions` foreground client received — minus
/// the leading `event: job` frame (the caller already knows the
/// job id from the URL). When the producer marks the job
/// Complete / Error / Cancelled, the stream terminates.
///
/// Tenant + user boundary: same rule as `active-job` above. A
/// cross-tenant lookup returns 404 so the existence of foreign
/// jobs is not leaked.
#[derive(Debug, serde::Deserialize)]
pub struct JobSyncQuery {
    #[serde(default)]
    pub since: usize,
}

pub async fn get_job_sync_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(job_id): axum::extract::Path<uuid::Uuid>,
    axum::extract::Query(query): axum::extract::Query<JobSyncQuery>,
) -> axum::response::Response {
    let Some(job) = state.chat_jobs.get(job_id).await else {
        return WorkbenchHttpError::JobNotFound.into_response();
    };
    if !job_visible_to(&job, &ctx) {
        return WorkbenchHttpError::JobNotFound.into_response();
    }
    crate::handlers::build_job_response(job, query.since)
}

/// Shared tenant + user visibility rule for the resumable-stream
/// endpoints. The job's stored `tenant_id` must match the caller's;
/// when both sides carry a user id, those must match too (legacy
/// Bearer keys without `user_id` skip the user check).
fn job_visible_to(
    job: &crate::chat_jobs::JobState,
    ctx: &gadgetron_core::context::TenantContext,
) -> bool {
    if job.tenant_id != ctx.tenant_id {
        return false;
    }
    if let (Some(actor), Some(owner)) = (ctx.actor_user_id, job.user_id) {
        if actor != owner {
            return false;
        }
    }
    true
}

/// `POST /api/v1/web/workbench/jobs/{job_id}/cancel` — ask the
/// producer to stop an in-flight generation. The producer abandons
/// the upstream stream at the next chunk boundary (killing the Penny
/// subprocess via the usual drop chain), persists the partial
/// assistant text, appends the `[DONE]` terminator for attached
/// subscribers, and flips the job to `cancelled`.
///
/// Returns the job snapshot at request time — the status transition
/// is asynchronous, so an immediately-following `active-job` poll may
/// still briefly read `streaming`. Idempotent: cancelling a finished
/// (or already-cancelled) job is a no-op that returns the snapshot.
///
/// Tenant + user boundary: same `job_visible_to` rule as the other
/// resumable-stream endpoints; invisible jobs 404.
pub async fn cancel_job_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(job_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<crate::chat_jobs::JobSnapshot>, WorkbenchHttpError> {
    let Some(job) = state.chat_jobs.get(job_id).await else {
        return Err(WorkbenchHttpError::JobNotFound);
    };
    if !job_visible_to(&job, &ctx) {
        return Err(WorkbenchHttpError::JobNotFound);
    }
    job.request_cancel();
    Ok(Json(job.snapshot().await))
}

/// Response body for `GET /api/v1/web/workbench/jobs/active`.
#[derive(Debug, serde::Serialize)]
pub struct ActiveJobsResponse {
    pub jobs: Vec<crate::chat_jobs::JobSnapshot>,
}

/// `GET /api/v1/web/workbench/jobs/active` — every still-streaming
/// chat job visible to the caller, in one response. The sidebar polls
/// THIS endpoint once per interval to drive its per-conversation
/// "생성 중" indicators, instead of polling
/// `/conversations/{id}/active-job` once per row.
///
/// Visibility: same tenant + user rule as the per-job endpoints
/// (`job_visible_to`). Finished jobs are omitted — the indicator only
/// cares about live generations.
pub async fn list_active_jobs_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Json<ActiveJobsResponse> {
    let mut jobs = Vec::new();
    for job in state.chat_jobs.all_jobs().await {
        if !job_visible_to(&job, &ctx) {
            continue;
        }
        let snapshot = job.snapshot().await;
        if snapshot.is_finished {
            continue;
        }
        jobs.push(snapshot);
    }
    Json(ActiveJobsResponse { jobs })
}

/// `GET /api/v1/web/workbench/conversations/{id}/messages` — read
/// the DB-backed transcript for this conversation and return the user and
/// assistant messages in order. Older Claude-only conversations may not have
/// DB transcript rows, so the handler falls back to the legacy Claude Code
/// jsonl lookup.
pub async fn get_conversation_messages_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<ConversationMessagesResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "conversation history")?;
    let user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::Core(GadgetronError::TenantNotFound))?;

    let owned: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM conversations \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| WorkbenchHttpError::Core(GadgetronError::Config(format!("lookup: {e}"))))?;
    if owned.is_none() {
        tracing::warn!(
            target: "conversations.history",
            conv_id = %id,
            user_id = %user_id,
            "ownership check miss — user has no row with this id"
        );
        return Ok(Json(ConversationMessagesResponse { messages: vec![] }));
    }

    match gadgetron_xaas::conversations::list_messages(pool, id, ctx.tenant_id, user_id).await {
        Ok(rows) if !rows.is_empty() => {
            let messages = rows
                .into_iter()
                .map(|row| HistoryMessage {
                    role: row.role,
                    content: row.content.clone(),
                    blocks: vec![HistoryBlock::Text { text: row.content }],
                    ts: Some(row.created_at.to_rfc3339()),
                })
                .collect();
            return Ok(Json(ConversationMessagesResponse { messages }));
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                target: "conversations.history",
                conv_id = %id,
                error = %e,
                "DB transcript lookup failed; falling back to legacy jsonl"
            );
        }
    }

    let candidates = claude_jsonl_candidates(id);
    tracing::info!(
        target: "conversations.history",
        conv_id = %id,
        candidates = candidates.len(),
        "jsonl candidates enumerated"
    );
    let mut history = Vec::new();
    for path in &candidates {
        tracing::info!(
            target: "conversations.history",
            path = %path.display(),
            exists = path.exists(),
            "trying candidate"
        );
        if let Ok(msgs) = parse_claude_jsonl(path) {
            tracing::info!(
                target: "conversations.history",
                path = %path.display(),
                count = msgs.len(),
                "parsed jsonl"
            );
            history = msgs;
            if !history.is_empty() {
                break;
            }
        }
    }
    Ok(Json(ConversationMessagesResponse { messages: history }))
}

fn claude_jsonl_candidates(id: uuid::Uuid) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        // Claude Code session path: ~/.claude/projects/<slug>/<session-uuid>.jsonl
        // The slug is Claude Code's slugified cwd — we'll scan the
        // `projects` dir and pick any subdir that contains our uuid.
        let projects = home.join(".claude/projects");
        if let Ok(rd) = std::fs::read_dir(&projects) {
            for entry in rd.flatten() {
                let p = entry.path().join(format!("{id}.jsonl"));
                if p.exists() {
                    out.push(p);
                }
            }
        }
        out.push(
            home.join(".gadgetron/penny/work")
                .join(format!("{id}.jsonl")),
        );
    }
    out
}

/// Remove the `<gadgetron_shared_context>...</gadgetron_shared_context>`
/// and `<gadgetron_user>...</gadgetron_user>` blocks that the chat
/// handler injects at the top of every user turn. They're noise for
/// the transcript view — the operator wants to see their question,
/// not the ambient context Penny saw.
fn strip_penny_context_prefix(s: &str) -> String {
    let mut out = s.to_string();
    for tag in ["gadgetron_shared_context", "gadgetron_user"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(a) = out.find(&open) {
            let Some(b_rel) = out[a..].find(&close) else {
                break;
            };
            let b = a + b_rel + close.len();
            out.replace_range(a..b, "");
        }
    }
    out.trim().to_string()
}

fn parse_claude_jsonl(path: &std::path::Path) -> std::io::Result<Vec<HistoryMessage>> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let ty = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "user" && ty != "assistant" {
            continue;
        }
        let msg = val.get("message");
        let role = msg
            .and_then(|m| m.get("role"))
            .and_then(|v| v.as_str())
            .unwrap_or(ty)
            .to_string();
        let raw_content = msg.and_then(|m| m.get("content"));
        let mut blocks: Vec<HistoryBlock> = Vec::new();
        match raw_content {
            Some(serde_json::Value::String(s)) if !s.is_empty() => {
                let cleaned = strip_penny_context_prefix(s);
                if !cleaned.is_empty() {
                    blocks.push(HistoryBlock::Text { text: cleaned });
                }
            }
            Some(serde_json::Value::Array(parts)) => {
                for p in parts {
                    let part_type = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match part_type {
                        "text" => {
                            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                if !t.is_empty() {
                                    blocks.push(HistoryBlock::Text {
                                        text: t.to_string(),
                                    });
                                }
                            }
                        }
                        "thinking" => {
                            if let Some(t) = p.get("thinking").and_then(|v| v.as_str()) {
                                if !t.is_empty() {
                                    blocks.push(HistoryBlock::Reasoning {
                                        text: t.to_string(),
                                    });
                                }
                            }
                        }
                        "tool_use" => {
                            let name = p
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_string();
                            let input = p.get("input").cloned().unwrap_or(serde_json::Value::Null);
                            blocks.push(HistoryBlock::ToolUse { name, input });
                        }
                        "tool_result" => {
                            let id = p
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let content_val = p.get("content");
                            let result_text = match content_val {
                                Some(serde_json::Value::String(s)) => s.clone(),
                                Some(serde_json::Value::Array(arr)) => arr
                                    .iter()
                                    .filter_map(|c| c.get("text").and_then(|v| v.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                Some(v) => serde_json::to_string(v).unwrap_or_default(),
                                None => String::new(),
                            };
                            if !result_text.is_empty() {
                                blocks.push(HistoryBlock::ToolResult {
                                    tool_use_id: id,
                                    content: result_text,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if blocks.is_empty() {
            continue;
        }
        // Flat legacy text projection (first text block).
        let content = blocks
            .iter()
            .find_map(|b| match b {
                HistoryBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let ts = val
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(HistoryMessage {
            role,
            content,
            blocks,
            ts,
        });
    }
    Ok(out)
}

/// `PATCH /api/v1/web/workbench/conversations/{id}` — rename.
pub async fn rename_conversation_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(body): Json<RenameConversationRequest>,
) -> Result<Json<serde_json::Value>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "conversation rename")?;
    let user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::Core(GadgetronError::TenantNotFound))?;
    gadgetron_xaas::conversations::rename_conversation(
        pool,
        ctx.tenant_id,
        user_id,
        id,
        &body.title,
    )
    .await
    .map_err(|e| match e {
        gadgetron_xaas::conversations::ConversationError::NotFound
        | gadgetron_xaas::conversations::ConversationError::OwnershipMismatch => {
            WorkbenchHttpError::Core(GadgetronError::Config("conversation not found".into()))
        }
        gadgetron_xaas::conversations::ConversationError::Db(e) => {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("rename: {e}")))
        }
        gadgetron_xaas::conversations::ConversationError::AgentBackendPinned { .. }
        | gadgetron_xaas::conversations::ConversationError::InvalidAgentProfile(_) => {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "conversation profile conflict".into(),
            ))
        }
    })?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id })))
}

// ---------------------------------------------------------------------------
// User self-service API keys
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub struct ListKeysResponse {
    pub keys: Vec<gadgetron_xaas::identity_keys::KeyRow>,
    pub returned: usize,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateKeyRequest {
    /// Human-readable label (e.g. "ci-deploy", "alice-laptop").
    #[serde(default)]
    pub label: Option<String>,
    /// Requested scopes. MUST be a subset of the caller's own scopes.
    /// Empty defaults to `["openai_compat"]`.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// "live" or "test" — matches api_keys.kind CHECK constraint.
    /// Defaults to "live".
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct RevokeKeyResponse {
    pub revoked: bool,
    pub key_id: uuid::Uuid,
}

/// `GET /api/v1/web/workbench/keys` — list keys owned by the calling
/// user (matched by caller's api_key.user_id). Tenant-bounded.
pub async fn list_my_keys_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<ListKeysResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "key listing")?;
    let owner = gadgetron_xaas::identity_keys::caller_user_id(pool, ctx.api_key_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "caller_user_id lookup: {e}"
            )))
        })?;
    let keys = gadgetron_xaas::identity_keys::list_keys(pool, ctx.tenant_id, owner)
        .await
        .map_err(|e| WorkbenchHttpError::Core(GadgetronError::Config(format!("list_keys: {e}"))))?;
    let returned = keys.len();
    Ok(Json(ListKeysResponse { keys, returned }))
}

/// `POST /api/v1/web/workbench/keys` — create a new key for the caller.
/// Raw key is returned EXACTLY ONCE. Scope narrowing: requested scopes
/// must be a subset of the caller's effective scopes.
pub async fn create_my_key_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<CreateKeyRequest>,
) -> Result<Json<gadgetron_xaas::identity_keys::NewKeyResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "key creation")?;
    let kind = body.kind.as_deref().unwrap_or("live");
    if !matches!(kind, "live" | "test") {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "kind must be live or test, got '{kind}'"
        ))));
    }
    let requested_scopes: Vec<String> = if body.scopes.is_empty() {
        vec!["openai_compat".into()]
    } else {
        body.scopes.clone()
    };
    // Scope-narrowing check: caller's scopes (as Display strings) must
    // be a superset of requested_scopes.
    let caller_scope_strs: std::collections::HashSet<String> =
        ctx.scopes.iter().map(|s| s.as_str().to_string()).collect();
    for s in &requested_scopes {
        if !caller_scope_strs.contains(s) {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "requested scope '{s}' exceeds caller's own scopes"
            ))));
        }
    }

    let owner = gadgetron_xaas::identity_keys::caller_user_id(pool, ctx.api_key_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "caller_user_id lookup: {e}"
            )))
        })?;

    let resp = gadgetron_xaas::identity_keys::create_key(
        pool,
        ctx.tenant_id,
        owner,
        body.label.as_deref(),
        &requested_scopes,
        kind,
    )
    .await
    .map_err(|e| WorkbenchHttpError::Core(GadgetronError::Config(format!("create_key: {e}"))))?;
    Ok(Json(resp))
}

/// `DELETE /api/v1/web/workbench/keys/{key_id}` — revoke a key that
/// the caller owns. Idempotent (re-revoke → 200).
pub async fn revoke_my_key_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(key_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<RevokeKeyResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "key revoke")?;
    let owner = gadgetron_xaas::identity_keys::caller_user_id(pool, ctx.api_key_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "caller_user_id lookup: {e}"
            )))
        })?;
    gadgetron_xaas::identity_keys::revoke_key(pool, ctx.tenant_id, owner, key_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("revoke_key: {e}")))
        })?;
    Ok(Json(RevokeKeyResponse {
        revoked: true,
        key_id,
    }))
}

// ---------------------------------------------------------------------------
// Admin audit_log query
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ListAuditLogQuery {
    pub actor_user_id: Option<uuid::Uuid>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct ListAuditLogResponse {
    pub rows: Vec<gadgetron_xaas::audit::writer::AuditLogRow>,
    pub returned: usize,
}

/// `GET /api/v1/web/workbench/admin/audit/log` — Management-scoped
/// tenant-pinned audit log query. Newest-first. Caller's tenant
/// always pinned by handler; `actor_user_id` + `since` + `limit`
/// (default 100, clamped `[1, 500]`) are optional narrowing filters.
pub async fn list_audit_log_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Query(query): axum::extract::Query<ListAuditLogQuery>,
) -> Result<Json<ListAuditLogResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "admin audit log query")?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let rows = gadgetron_xaas::audit::writer::query_audit_log(
        pool,
        ctx.tenant_id,
        query.actor_user_id,
        query.since,
        limit,
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!("audit log query: {e}")))
    })?;
    let returned = rows.len();
    Ok(Json(ListAuditLogResponse { rows, returned }))
}

// ---------------------------------------------------------------------------
// Teams + team_members CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub struct ListTeamsResponse {
    pub teams: Vec<gadgetron_xaas::teams::TeamRow>,
    pub returned: usize,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateTeamRequest {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ListMembersResponse {
    pub members: Vec<gadgetron_xaas::teams::TeamMemberRow>,
    pub returned: usize,
}

#[derive(Debug, serde::Deserialize)]
pub struct AddMemberRequest {
    pub user_id: uuid::Uuid,
    #[serde(default = "default_member_role")]
    pub role: String,
}
fn default_member_role() -> String {
    "member".into()
}

#[derive(Debug, serde::Serialize)]
pub struct SimpleOkResponse {
    pub ok: bool,
}

pub async fn list_teams_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<ListTeamsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "teams")?;
    let teams = gadgetron_xaas::teams::list_teams(pool, ctx.tenant_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("list_teams: {e}")))
        })?;
    let returned = teams.len();
    Ok(Json(ListTeamsResponse { teams, returned }))
}

pub async fn create_team_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<CreateTeamRequest>,
) -> Result<Json<gadgetron_xaas::teams::TeamRow>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "teams")?;
    let row = gadgetron_xaas::teams::create_team(
        pool,
        ctx.tenant_id,
        &body.id,
        &body.display_name,
        body.description.as_deref(),
        None,
    )
    .await
    .map_err(|e| WorkbenchHttpError::Core(GadgetronError::Config(format!("create_team: {e}"))))?;
    Ok(Json(row))
}

pub async fn delete_team_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(team_id): axum::extract::Path<String>,
) -> Result<Json<SimpleOkResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "teams")?;
    gadgetron_xaas::teams::delete_team(pool, ctx.tenant_id, &team_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("delete_team: {e}")))
        })?;
    Ok(Json(SimpleOkResponse { ok: true }))
}

pub async fn list_team_members_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(team_id): axum::extract::Path<String>,
) -> Result<Json<ListMembersResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "teams")?;
    let members = gadgetron_xaas::teams::list_team_members(pool, ctx.tenant_id, &team_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("list_team_members: {e}")))
        })?;
    let returned = members.len();
    Ok(Json(ListMembersResponse { members, returned }))
}

pub async fn add_team_member_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(team_id): axum::extract::Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> Result<Json<gadgetron_xaas::teams::TeamMemberRow>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "teams")?;
    let row = gadgetron_xaas::teams::add_team_member(
        pool,
        ctx.tenant_id,
        &team_id,
        body.user_id,
        &body.role,
        None,
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!("add_team_member: {e}")))
    })?;
    Ok(Json(row))
}

pub async fn remove_team_member_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path((team_id, user_id)): axum::extract::Path<(String, uuid::Uuid)>,
) -> Result<Json<SimpleOkResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "teams")?;
    gadgetron_xaas::teams::remove_team_member(pool, ctx.tenant_id, &team_id, user_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("remove_team_member: {e}")))
        })?;
    Ok(Json(SimpleOkResponse { ok: true }))
}

// ---------------------------------------------------------------------------
// Groups + user_groups admin handlers.
//
// Groups are an access-permission bucket, orthogonal to teams
// (collaboration) and role (admin/member/service settings privilege).
// Membership CRUD only — permission policies attached to groups are
// a Phase 2 concern.
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub struct ListGroupsResponse {
    pub groups: Vec<gadgetron_xaas::groups::GroupRow>,
    pub returned: usize,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateGroupRequest {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ListGroupMembersResponse {
    pub members: Vec<gadgetron_xaas::groups::UserGroupRow>,
    pub returned: usize,
}

#[derive(Debug, serde::Deserialize)]
pub struct AddGroupMemberRequest {
    pub user_id: uuid::Uuid,
}

pub async fn list_groups_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<ListGroupsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "groups")?;
    let groups = gadgetron_xaas::groups::list_groups(pool, ctx.tenant_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("list_groups: {e}")))
        })?;
    let returned = groups.len();
    Ok(Json(ListGroupsResponse { groups, returned }))
}

pub async fn create_group_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(body): Json<CreateGroupRequest>,
) -> Result<Json<gadgetron_xaas::groups::GroupRow>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "groups")?;
    let row = gadgetron_xaas::groups::create_group(
        pool,
        ctx.tenant_id,
        &body.id,
        &body.display_name,
        body.description.as_deref(),
        ctx.actor_user_id,
    )
    .await
    .map_err(|e| WorkbenchHttpError::Core(GadgetronError::Config(format!("create_group: {e}"))))?;
    Ok(Json(row))
}

pub async fn delete_group_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(group_id): axum::extract::Path<String>,
) -> Result<Json<SimpleOkResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "groups")?;
    gadgetron_xaas::groups::delete_group(pool, ctx.tenant_id, &group_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("delete_group: {e}")))
        })?;
    Ok(Json(SimpleOkResponse { ok: true }))
}

pub async fn list_group_members_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(group_id): axum::extract::Path<String>,
) -> Result<Json<ListGroupMembersResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "groups")?;
    let members = gadgetron_xaas::groups::list_group_members(pool, ctx.tenant_id, &group_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("list_group_members: {e}")))
        })?;
    let returned = members.len();
    Ok(Json(ListGroupMembersResponse { members, returned }))
}

pub async fn add_group_member_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(group_id): axum::extract::Path<String>,
    Json(body): Json<AddGroupMemberRequest>,
) -> Result<Json<gadgetron_xaas::groups::UserGroupRow>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "groups")?;
    let row = gadgetron_xaas::groups::add_user_to_group(
        pool,
        ctx.tenant_id,
        &group_id,
        body.user_id,
        ctx.actor_user_id,
    )
    .await
    .map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!("add_group_member: {e}")))
    })?;
    Ok(Json(row))
}

pub async fn remove_group_member_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path((group_id, user_id)): axum::extract::Path<(String, uuid::Uuid)>,
) -> Result<Json<SimpleOkResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "groups")?;
    gadgetron_xaas::groups::remove_user_from_group(pool, ctx.tenant_id, &group_id, user_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("remove_group_member: {e}")))
        })?;
    Ok(Json(SimpleOkResponse { ok: true }))
}

pub async fn list_user_groups_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Path(user_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<ListGroupsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "groups")?;
    let groups = gadgetron_xaas::groups::list_groups_for_user(pool, ctx.tenant_id, user_id)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!("list_user_groups: {e}")))
        })?;
    let returned = groups.len();
    Ok(Json(ListGroupsResponse { groups, returned }))
}

/// `GET /usage/summary` — tenant-scoped operations rollup.
///
/// Aggregates over the past `window_hours` (default 24, clamped
/// `[1, 168]`) for the authenticated actor's tenant. Runs three
/// aggregate queries against the three audit tables in parallel
/// and folds the results into a single response shape. The tenant
/// boundary is PINNED by the handler — callers cannot read another
/// tenant's usage regardless of query params.
///
/// Returns 503-shape when `pg_pool` isn't configured.
pub async fn get_usage_summary(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Query(query): axum::extract::Query<UsageSummaryQuery>,
) -> Result<Json<UsageSummaryResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "usage summary")?;
    let window_hours = query.window_hours.unwrap_or(24).clamp(1, 168);
    let since = chrono::Utc::now() - chrono::Duration::hours(window_hours as i64);
    let tenant_id_text = ctx.tenant_id.to_string();

    // `audit_log` stores `tenant_id` as UUID; the two newer audit
    // tables store it as TEXT. We pass both so the queries bind
    // correctly.
    let (chat_row, action_row, tool_row) = tokio::join!(
        sqlx::query_as::<_, ChatRollup>(
            r#"SELECT COUNT(*)::bigint AS requests,
                      COALESCE(SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END), 0)::bigint AS errors,
                      COALESCE(SUM(input_tokens), 0)::bigint AS total_input_tokens,
                      COALESCE(SUM(output_tokens), 0)::bigint AS total_output_tokens,
                      COALESCE(SUM(cost_cents), 0)::bigint AS total_cost_cents,
                      COALESCE(AVG(latency_ms)::float8, 0.0) AS avg_latency_ms
               FROM audit_log
               WHERE tenant_id = $1 AND timestamp >= $2"#,
        )
        .bind(ctx.tenant_id)
        .bind(since)
        .fetch_one(pool),
        sqlx::query_as::<_, ActionRollup>(
            r#"SELECT COUNT(*)::bigint AS total,
                      COALESCE(SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END), 0)::bigint AS success,
                      COALESCE(SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END), 0)::bigint AS error,
                      COALESCE(SUM(CASE WHEN outcome = 'pending_approval' THEN 1 ELSE 0 END), 0)::bigint AS pending_approval,
                      COALESCE(AVG(elapsed_ms)::float8, 0.0) AS avg_elapsed_ms
               FROM action_audit_events
               WHERE tenant_id = $1 AND created_at >= $2"#,
        )
        .bind(&tenant_id_text)
        .bind(since)
        .fetch_one(pool),
        sqlx::query_as::<_, ToolRollup>(
            r#"SELECT COUNT(*)::bigint AS total,
                      COALESCE(SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END), 0)::bigint AS errors
               FROM tool_audit_events
               WHERE tenant_id = $1 AND created_at >= $2"#,
        )
        .bind(&tenant_id_text)
        .bind(since)
        .fetch_one(pool),
    );

    let chat = chat_row.map_err(usage_sql_err)?;
    let actions = action_row.map_err(usage_sql_err)?;
    let tools = tool_row.map_err(usage_sql_err)?;

    Ok(Json(UsageSummaryResponse {
        window_hours,
        chat: UsageChatStats {
            requests: chat.requests,
            errors: chat.errors,
            total_input_tokens: chat.total_input_tokens,
            total_output_tokens: chat.total_output_tokens,
            total_cost_cents: chat.total_cost_cents,
            avg_latency_ms: chat.avg_latency_ms,
        },
        actions: UsageActionStats {
            total: actions.total,
            success: actions.success,
            error: actions.error,
            pending_approval: actions.pending_approval,
            avg_elapsed_ms: actions.avg_elapsed_ms,
        },
        tools: UsageToolStats {
            total: tools.total,
            errors: tools.errors,
        },
    }))
}

/// Tight rollup structs — not exposed publicly. Serialize path
/// goes through `UsageSummaryResponse` which lives in the response
/// shape above.
#[derive(sqlx::FromRow)]
struct ChatRollup {
    requests: i64,
    errors: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cost_cents: i64,
    avg_latency_ms: f64,
}

#[derive(sqlx::FromRow)]
struct ActionRollup {
    total: i64,
    success: i64,
    error: i64,
    pending_approval: i64,
    avg_elapsed_ms: f64,
}

#[derive(sqlx::FromRow)]
struct ToolRollup {
    total: i64,
    errors: i64,
}

fn usage_sql_err(e: sqlx::Error) -> WorkbenchHttpError {
    WorkbenchHttpError::Core(GadgetronError::Config(format!(
        "usage summary query failed: {e}"
    )))
}

/// Query parameters for `GET /audit/events`.
#[derive(Debug, Deserialize)]
pub struct AuditEventsQuery {
    /// Restrict to a specific action id (e.g. `wiki-write`).
    pub action_id: Option<String>,
    /// Only events at or after this RFC3339 timestamp.
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    /// Row cap. Defaults to 100, clamped to `[1, 500]`.
    pub limit: Option<i64>,
}

/// Response shape for `GET /audit/events`.
#[derive(Debug, serde::Serialize)]
pub struct AuditEventsResponse {
    pub events: Vec<gadgetron_xaas::audit::ActionAuditRow>,
    pub returned: usize,
}

/// `GET /audit/events` — tenant-scoped read over `action_audit_events`.
///
/// Filters: `action_id` (exact match), `since` (inclusive), `limit`
/// (default 100, max 500). The tenant boundary is ALWAYS pinned to the
/// authenticated actor's tenant — the caller cannot read another
/// tenant's audit trail regardless of query params.
/// Returns 503-shape when the server has no Postgres pool.
pub async fn list_audit_events(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Query(query): axum::extract::Query<AuditEventsQuery>,
) -> Result<Json<AuditEventsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "audit event query")?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let filter = gadgetron_xaas::audit::ActionAuditQueryFilter {
        tenant_id: ctx.tenant_id.to_string(),
        action_id: query.action_id,
        since: query.since,
        limit,
    };
    let events = gadgetron_xaas::audit::query_action_audit_events(pool, &filter)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "audit event query failed: {e}"
            )))
        })?;
    let returned = events.len();
    Ok(Json(AuditEventsResponse { events, returned }))
}

/// Query parameters for `GET /audit/tool-events` (Penny tool-call
/// audit).
#[derive(Debug, Deserialize)]
pub struct ToolAuditEventsQuery {
    pub tool_name: Option<String>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub limit: Option<i64>,
}

/// Response shape for `GET /audit/tool-events`.
#[derive(Debug, serde::Serialize)]
pub struct ToolAuditEventsResponse {
    pub events: Vec<gadgetron_xaas::audit::ToolAuditRow>,
    pub returned: usize,
}

/// `GET /audit/tool-events` — tenant-scoped read over
/// `tool_audit_events` (Penny tool-call trail).
///
/// Mirrors `/audit/events` shape but queries the OTHER audit plane.
/// Both endpoints exist so the dashboard can pull action + tool
/// events into two side-by-side panels without a UNION at DB level.
pub async fn list_tool_audit_events(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    axum::extract::Query(query): axum::extract::Query<ToolAuditEventsQuery>,
) -> Result<Json<ToolAuditEventsResponse>, WorkbenchHttpError> {
    let pool = require_pg_pool(&state, "tool audit query")?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let filter = gadgetron_xaas::audit::ToolAuditQueryFilter {
        tenant_id: ctx.tenant_id.to_string(),
        tool_name: query.tool_name,
        since: query.since,
        limit,
    };
    let events = gadgetron_xaas::audit::query_tool_audit_events(pool, &filter)
        .await
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "tool audit query failed: {e}"
            )))
        })?;
    let returned = events.len();
    Ok(Json(ToolAuditEventsResponse { events, returned }))
}

/// `POST /actions/:action_id` — direct action invocation.
///
/// Requires the action service to be wired in `GatewayWorkbenchService`.
/// Returns `501 Not Implemented` when the action service is absent.
pub async fn invoke_action(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(action_id): Path<String>,
    Json(request): Json<InvokeWorkbenchActionRequest>,
) -> Result<Json<InvokeWorkbenchActionResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let action_svc = svc.actions.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "action service is not wired in this build".into(),
        ))
    })?;
    // Drift-fix follow-up to PR 7: build a real AuthenticatedContext
    // from the authenticated request instead of the old system sentinel.
    // `actor.api_key_id` is the placeholder identity until a
    // real user table lands; `actor.tenant_id` is the real tenant.
    let actor = AuthenticatedContext {
        api_key_id: ctx.api_key_id,
        tenant_id: ctx.tenant_id,
        // Real owning user id from ValidatedKey.user_id
        // via TenantContext.actor_user_id. Some(..) for real users
        // (cookie sessions + backfilled api_keys); None for legacy
        // keys pre-backfill keys.
        real_user_id: ctx.actor_user_id,
    };
    let arguments_summary = truncate_args_for_activity(&request.args);
    let resp = action_svc
        .invoke(&actor, &ctx.scopes, &action_id, request)
        .await?;
    publish_action_activity(&state, &actor, &action_id, &resp, arguments_summary);
    Ok(Json(resp))
}

/// Fan-out to the live-feed WebSocket after an action completes.
/// Reads the response envelope and fires a matching
/// `ActivityEvent::ActionCompleted` with the outcome as the client
/// will see it. Fire-and-forget.
fn publish_action_activity(
    state: &AppState,
    actor: &AuthenticatedContext,
    action_id: &str,
    resp: &InvokeWorkbenchActionResponse,
    arguments_summary: Option<String>,
) {
    let Some(audit_event_id) = resp.result.audit_event_id else {
        return;
    };
    let outcome = match resp.result.status.as_str() {
        "ok" => "success",
        "pending_approval" => "pending_approval",
        _ => "error",
    };
    state.activity_bus.publish(
        gadgetron_core::activity_bus::ActivityEvent::ActionCompleted {
            tenant_id: actor.tenant_id,
            audit_event_id,
            action_id: action_id.to_string(),
            gadget_name: None,
            outcome: outcome.to_string(),
            error_code: None,
            elapsed_ms: 0,
            arguments_summary,
        },
    );
}

/// Render an action/tool argument JSON into a short one-line preview
/// (≤200 chars). Keeps the live bus payload bounded regardless of
/// how verbose the caller's args happen to be. `null` / empty-object
/// arguments yield `None` so the evidence pane can skip the line
/// rather than render an empty string.
pub fn truncate_args_for_activity(args: &serde_json::Value) -> Option<String> {
    if args.is_null() {
        return None;
    }
    if let serde_json::Value::Object(map) = args {
        if map.is_empty() {
            return None;
        }
    }
    let rendered = serde_json::to_string(args).ok()?;
    const MAX: usize = 200;
    if rendered.chars().count() <= MAX {
        Some(rendered)
    } else {
        let mut out: String = rendered.chars().take(MAX).collect();
        out.push('…');
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// Route factory
// ---------------------------------------------------------------------------

/// Build the workbench sub-router.
///
/// Mount with `.nest("/api/v1/web/workbench", workbench_routes())` in
/// `server.rs` AFTER the scope exception in `middleware/scope.rs` is in place.
pub fn workbench_routes() -> Router<AppState> {
    Router::new()
        // bootstrap / activity / evidence
        .route("/bootstrap", get(get_workbench_bootstrap))
        .route("/activity", get(list_workbench_activity))
        .route(
            "/requests/{request_id}/evidence",
            get(get_workbench_request_evidence),
        )
        // descriptor catalog + knowledge status + view data
        .route("/knowledge-status", get(get_knowledge_status))
        .route("/capabilities", get(get_capability_projection))
        .route(
            "/contributions/{contribution_id}/data",
            get(get_contribution_data),
        )
        .route("/views", get(list_views))
        .route("/views/{view_id}/data", get(load_view_data))
        .route("/actions", get(list_actions))
        .route("/actions/{action_id}", post(invoke_action))
        // Approval flow.
        .route("/approvals/pending", get(list_pending_approvals))
        .route("/approvals/{approval_id}/approve", post(approve_action))
        .route("/approvals/{approval_id}/deny", post(deny_action))
        // Side Panel → Tool Modes editor (per-tool approval config).
        .route("/agent/modes", get(get_agent_modes))
        .route("/agent/modes", axum::routing::patch(patch_agent_modes))
        // Tenant-scoped audit event query.
        .route("/audit/events", get(list_audit_events))
        // Penny tool-call audit surface.
        .route("/audit/tool-events", get(list_tool_audit_events))
        // Operator usage summary rollup.
        .route("/usage/summary", get(get_usage_summary))
        // Quota status (current tenant).
        .route("/quota/status", get(get_quota_status))
        // Billing events ledger query.
        .route("/admin/billing/events", get(list_billing_events))
        .route(
            "/admin/billing/insert-failures",
            get(admin_billing_insert_failures),
        )
        // Penny Claude Code LLM gateway settings.
        .route("/admin/agent/brain", get(get_agent_brain_settings))
        .route(
            "/admin/agent/brain",
            axum::routing::patch(patch_agent_brain_settings),
        )
        // Tenant-scoped immutable policy model and read-only decision preview.
        .route("/admin/policy", get(get_active_policy))
        .route("/admin/policy/revisions", post(create_policy_revision))
        .route(
            "/admin/policy/legacy-revisions",
            post(create_legacy_policy_revision),
        )
        .route("/admin/policy/preview", post(preview_policy))
        .route("/admin/policy/decisions", get(list_policy_decisions))
        // LLM endpoint registry and Penny attachment.
        .route("/admin/llm/endpoints", get(list_llm_endpoints_handler))
        .route("/admin/llm/endpoints", post(create_llm_endpoint_handler))
        .route(
            "/admin/llm/endpoints/autodetect",
            post(autodetect_llm_endpoint_handler),
        )
        .route(
            "/admin/llm/endpoints/{endpoint_id}/ccr",
            post(create_ccr_bridge_handler),
        )
        .route(
            "/admin/llm/endpoints/{endpoint_id}",
            axum::routing::delete(delete_llm_endpoint_handler),
        )
        .route(
            "/admin/llm/endpoints/{endpoint_id}/probe",
            post(probe_llm_endpoint_handler),
        )
        .route(
            "/admin/llm/endpoints/{endpoint_id}/use",
            post(use_llm_endpoint_handler),
        )
        // Safe endpoint/model projection for authenticated chat users.
        .route(
            "/llm/endpoints/available",
            get(list_available_llm_endpoint_models_handler),
        )
        // Live activity WebSocket feed.
        .route("/events/ws", get(events_ws_handler))
        .merge(super::knowledge_spaces::routes())
        .merge(super::knowledge_sources::routes())
        .merge(super::knowledge_graph::routes())
        .merge(super::knowledge_collections::routes())
        .merge(super::knowledge_ontology::routes())
        .merge(super::knowledge_jobs::routes())
        .merge(super::autonomy::routes())
        .merge(super::manager_oversight::routes())
        // Admin catalog hot-reload.
        .route("/admin/reload-catalog", post(reload_catalog_handler))
        .route(
            "/admin/knowledge/ai-roles",
            get(get_core_knowledge_agent_roles_handler),
        )
        .route(
            "/admin/knowledge/ai-roles/{role_id}",
            axum::routing::put(put_core_knowledge_agent_role_handler)
                .delete(delete_core_knowledge_agent_role_handler),
        )
        // Bundle discovery.
        .route("/admin/bundles", get(list_bundles_handler))
        // Signed external-runtime lifecycle.
        .route(
            "/admin/bundles/runtime",
            get(list_bundle_runtime_status_handler),
        )
        .route(
            "/admin/bundles/dependency-plan",
            get(get_bundle_dependency_plan_handler),
        )
        .route(
            "/admin/bundle-sets/inspect",
            post(inspect_bundle_set_handler),
        )
        .route("/admin/bundle-sets/apply", post(apply_bundle_set_handler))
        .route(
            "/admin/bundles/{bundle_id}/runtime",
            get(get_bundle_runtime_status_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/enable",
            post(enable_bundle_runtime_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/disable",
            post(disable_bundle_runtime_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/job-recipes/{recipe_id}/start",
            post(start_bundle_job_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/jobs/{job_id}",
            get(poll_bundle_job_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/jobs/{job_id}/cancel",
            post(cancel_bundle_job_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/permissions",
            get(get_bundle_permission_grant_handler)
                .put(grant_bundle_permissions_handler)
                .delete(revoke_bundle_permissions_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/settings",
            get(get_bundle_settings_handler).put(put_bundle_settings_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/ai-roles",
            get(get_bundle_knowledge_agent_roles_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/ai-roles/{role_id}",
            axum::routing::put(put_bundle_knowledge_agent_role_handler)
                .delete(delete_bundle_knowledge_agent_role_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/export",
            get(export_bundle_package_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/ssh/targets",
            get(list_bundle_ssh_targets_handler).post(bootstrap_bundle_ssh_target_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/ssh/targets/{target_id}",
            axum::routing::put(put_bundle_ssh_target_handler)
                .delete(delete_bundle_ssh_target_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/ssh/targets/{target_id}/setup",
            post(reapply_bundle_ssh_target_setup_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/ssh/secrets",
            get(list_bundle_ssh_secrets_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/ssh/secrets/{secret_id}",
            axum::routing::put(put_bundle_ssh_secret_handler)
                .delete(delete_bundle_ssh_secret_handler),
        )
        // Bundle uninstall.
        .route(
            "/admin/bundles/{bundle_id}",
            axum::routing::delete(uninstall_bundle_handler),
        )
        // Admin user CRUD.
        .route("/admin/users", get(list_users_handler))
        .route("/admin/users", post(create_user_handler))
        .route(
            "/admin/users/{user_id}",
            axum::routing::patch(update_user_profile_handler),
        )
        .route(
            "/admin/users/{user_id}",
            axum::routing::delete(delete_user_handler),
        )
        // User self-service API keys.
        .route("/keys", get(list_my_keys_handler))
        .route("/keys", post(create_my_key_handler))
        .route(
            "/keys/{key_id}",
            axum::routing::delete(revoke_my_key_handler),
        )
        // Teams + members CRUD.
        .route("/admin/teams", get(list_teams_handler))
        .route("/admin/teams", post(create_team_handler))
        .route(
            "/admin/teams/{team_id}",
            axum::routing::delete(delete_team_handler),
        )
        .route(
            "/admin/teams/{team_id}/members",
            get(list_team_members_handler),
        )
        .route(
            "/admin/teams/{team_id}/members",
            post(add_team_member_handler),
        )
        .route(
            "/admin/teams/{team_id}/members/{user_id}",
            axum::routing::delete(remove_team_member_handler),
        )
        // Groups + members CRUD (access-permission bucket; orthogonal to teams).
        .route("/admin/groups", get(list_groups_handler))
        .route("/admin/groups", post(create_group_handler))
        .route(
            "/admin/groups/{group_id}",
            axum::routing::delete(delete_group_handler),
        )
        .route(
            "/admin/groups/{group_id}/members",
            get(list_group_members_handler),
        )
        .route(
            "/admin/groups/{group_id}/members",
            post(add_group_member_handler),
        )
        .route(
            "/admin/groups/{group_id}/members/{user_id}",
            axum::routing::delete(remove_group_member_handler),
        )
        // Per-user group memberships (read; write goes through PATCH /admin/users/{id}).
        .route(
            "/admin/users/{user_id}/groups",
            get(list_user_groups_handler),
        )
        // Admin audit_log query endpoint.
        .route("/admin/audit/log", get(list_audit_log_handler))
        // Per-user chat conversations (left-rail sidebar).
        .route("/conversations", get(list_conversations_handler))
        .route(
            "/conversations/{id}",
            axum::routing::delete(delete_conversation_handler).patch(rename_conversation_handler),
        )
        .route(
            "/conversations/{id}/messages",
            get(get_conversation_messages_handler),
        )
        .route(
            "/conversations/{id}/agent-profile",
            get(get_conversation_agent_profile_handler)
                .patch(patch_conversation_agent_profile_handler),
        )
        // Resumable-stream endpoints. `active-job` returns JSON
        // metadata (job_id, status, chunk_count). `jobs/{id}/sync`
        // returns an SSE stream that replays buffered chunks from
        // `?since=N` and follows the live tail until the producer
        // marks the job complete. Tenant + user boundary is
        // re-checked inside each handler (the job carries them).
        .route(
            "/conversations/{id}/active-job",
            get(get_conversation_active_job_handler),
        )
        .route("/jobs/active", get(list_active_jobs_handler))
        .route("/jobs/{job_id}/sync", get(get_job_sync_handler))
        .route(
            "/jobs/{job_id}/cancel",
            axum::routing::post(cancel_job_handler),
        )
}

/// Signed package envelopes include base64 runtime bytes and therefore use a
/// larger, still bounded body budget than ordinary Workbench JSON.
pub fn bundle_source_routes() -> Router<AppState> {
    Router::new()
        .route("/admin/bundles", post(install_bundle_handler))
        .route(
            "/admin/bundles/inspect",
            post(inspect_bundle_source_handler),
        )
        .route(
            "/admin/bundles/install",
            post(install_bundle_source_handler),
        )
        .route(
            "/admin/bundles/{bundle_id}/upgrade",
            axum::routing::put(upgrade_bundle_source_handler),
        )
}

// ---------------------------------------------------------------------------
// POST/DELETE /api/v1/web/workbench/admin/bundles
// ---------------------------------------------------------------------------

/// Request body for `POST /admin/bundles`. Operator sends the full
/// manifest text; handler validates + writes to disk.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallBundleRequest {
    /// Complete `bundle.toml` text. Must include a `[bundle]` table
    /// with a valid `id` (id drives the install directory name).
    pub bundle_toml: String,
    /// Optional Ed25519 detached signature over `bundle_toml`.
    /// Hex-encoded 64-byte signature. When
    /// `web.bundle_signing.require_signature = true`, a missing
    /// signature is rejected before any filesystem IO. When present,
    /// the handler verifies it against every key in
    /// `web.bundle_signing.public_keys_hex`; any match accepts.
    #[serde(default)]
    pub signature_hex: Option<String>,
    /// Optional versioned public SDK `package.toml`. The legacy catalog remains a
    /// separate transitional file until the external runtime host consumes
    /// this contract directly.
    #[serde(default)]
    pub package_toml: Option<String>,
    /// Optional Ed25519 detached signature over `package_toml`. A package may
    /// never reuse the catalog signature because each signed byte sequence is
    /// independently auditable.
    #[serde(default)]
    pub package_signature_hex: Option<String>,
    /// Optional base64-encoded runtime entry bytes. The signed package
    /// manifest pins these bytes through `runtime.entry_sha256`; mismatches
    /// are rejected before the install directory is created.
    #[serde(default)]
    pub runtime_artifact_base64: Option<String>,
    /// Base64 package assets keyed by the exact relative paths declared by
    /// domain schema, seed-asset and migration descriptors.
    #[serde(default)]
    pub package_assets_base64: std::collections::BTreeMap<String, String>,
}

/// Portable signed Bundle package source used by the commercial control plane.
/// Inline envelopes are produced by a local `.gadgetron-bundle.json` file;
/// URL sources are fetched by Core so Bundle runtimes never receive egress.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BundlePackageSource {
    Inline { envelope: InstallBundleRequest },
    Url { url: String },
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectBundleSourceRequest {
    pub source: BundlePackageSource,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallBundleSourceRequest {
    pub source: BundlePackageSource,
    /// Digest returned by inspect. Apply is rejected when the exact portable
    /// envelope bytes changed between the two operator actions.
    pub expected_source_sha256: String,
}

#[derive(Debug, serde::Serialize)]
pub struct BundleInstallInspection {
    pub bundle_id: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_class: Option<gadgetron_bundle_sdk::BundleClass>,
    pub source_sha256: String,
    pub package_manifest_sha256: Option<String>,
    pub contract: &'static str,
    pub action_count: usize,
    pub view_count: usize,
    pub permission_ids: Vec<String>,
    pub settings_declared: bool,
    pub runtime_kind: Option<String>,
    pub installable: bool,
    pub upgradeable: bool,
    pub warnings: Vec<String>,
}

struct ValidatedBundleInstall {
    bundle: crate::web::catalog::BundleMetadata,
    bundle_class: Option<gadgetron_bundle_sdk::BundleClass>,
    package_manifest_sha256: Option<String>,
    runtime_artifact: Option<(gadgetron_bundle_sdk::RelativePath, Vec<u8>)>,
    package_assets: Vec<(gadgetron_bundle_sdk::RelativePath, Vec<u8>)>,
    domain_schemas: Vec<gadgetron_bundle_sdk::DomainSchemaDescriptor>,
    permission_ids: Vec<String>,
    settings_declared: bool,
    runtime_kind: Option<String>,
}

/// Response for `POST /admin/bundles`.
#[derive(Debug, serde::Serialize)]
pub struct InstallBundleResponse {
    pub installed: bool,
    /// Installed bundle's id — matches the directory name under
    /// `bundles_dir`.
    pub bundle_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_class: Option<gadgetron_bundle_sdk::BundleClass>,
    /// Absolute path of the written `bundle.toml` (operator can
    /// `cat` / `grep` this to verify).
    pub manifest_path: String,
    /// Public package contract installed alongside the legacy catalog, when
    /// supplied by the operator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_manifest_path: Option<String>,
    /// SHA-256 over the exact validated `package.toml` bytes. The runtime
    /// handshake must return this digest before Core can enable capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_manifest_sha256: Option<String>,
    /// Installed runtime entry when artifact bytes were supplied and matched
    /// the signed package digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_artifact_path: Option<String>,
    pub package_asset_count: usize,
    /// `catalog_only` for legacy installs; `bundle_sdk_v1` when a validated
    /// public package contract was installed.
    pub contract: &'static str,
    /// Runtime lifecycle is deliberately separate from metadata installation.
    pub runtime_state: &'static str,
    /// Hint to the operator that the live catalog hasn't changed
    /// yet — trigger `POST /admin/reload-catalog` or `SIGHUP` to
    /// pick up the new bundle. Keeps install idempotent: installing
    /// the same manifest twice doesn't rapid-fire reload the
    /// workbench underneath running requests.
    pub reload_hint: &'static str,
}

/// Response for `DELETE /admin/bundles/{id}`.
#[derive(Debug, serde::Serialize)]
pub struct UninstallBundleResponse {
    pub uninstalled: bool,
    pub bundle_id: String,
    pub runtime_disabled: bool,
    pub state_preserved: bool,
    pub reload_hint: &'static str,
}

/// Verify an optional Ed25519 signature against the configured trust
/// anchors.
///
/// Policy:
/// - `require_signature = true` + no signature → reject.
/// - Signature present → verify against every configured public
///   key; any match accepts. No match → reject.
/// - `require_signature = false` + no signature → accept (legacy
///   unsigned behavior preserved for back-compat).
/// - Signature supplied but `public_keys_hex` empty → reject (no
///   trust anchors means we can't validate; better to fail loud
///   than silently accept unverified input).
fn verify_detached_signature(
    cfg: &gadgetron_core::config::BundleSigningConfig,
    artifact: &'static str,
    message: &[u8],
    signature_hex: Option<&str>,
) -> Result<(), WorkbenchHttpError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let Some(sig_hex) = signature_hex else {
        if cfg.require_signature {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "signed artifact: {artifact} signature required but none supplied \
                     (web.bundle_signing.require_signature = true)"
            ))));
        }
        return Ok(());
    };

    let sig_bytes = hex::decode(sig_hex).map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "signed artifact: {artifact} signature is not valid hex: {e}",
        )))
    })?;
    let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().map_err(|_| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "signed artifact: {artifact} signature must be 64 bytes (got {})",
            sig_bytes.len()
        )))
    })?;
    let signature = Signature::from_bytes(&sig_arr);

    if cfg.public_keys_hex.is_empty() {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "signed artifact: {artifact} signature supplied but no trust anchors \
                 configured (web.bundle_signing.public_keys_hex is empty)"
        ))));
    }

    for (idx, pk_hex) in cfg.public_keys_hex.iter().enumerate() {
        let Ok(pk_bytes) = hex::decode(pk_hex) else {
            tracing::warn!(
                target: "workbench.admin",
                key_index = idx,
                "bundle signing: configured public key is not valid hex; skipping"
            );
            continue;
        };
        let Ok(pk_arr) = <[u8; 32]>::try_from(pk_bytes.as_slice()) else {
            tracing::warn!(
                target: "workbench.admin",
                key_index = idx,
                expected_bytes = 32,
                got_bytes = pk_bytes.len(),
                "bundle signing: configured public key has wrong length; skipping"
            );
            continue;
        };
        if let Ok(vk) = VerifyingKey::from_bytes(&pk_arr) {
            if vk.verify(message, &signature).is_ok() {
                tracing::info!(
                    target: "workbench.admin",
                    key_index = idx,
                    artifact,
                    "signed artifact: detached signature verified"
                );
                return Ok(());
            }
        }
    }

    Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
        "signed artifact: {artifact} signature did not verify against any configured \
             trust anchor (web.bundle_signing.public_keys_hex)"
    ))))
}

#[cfg(test)]
pub(crate) fn verify_required_detached_signature(
    cfg: &gadgetron_core::config::BundleSigningConfig,
    artifact: &'static str,
    message: &[u8],
    signature_hex: &str,
) -> Result<(), WorkbenchHttpError> {
    if cfg.public_keys_hex.is_empty() {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle enable: {artifact} signature cannot be verified because no trust anchors are configured"
        ))));
    }
    verify_detached_signature(cfg, artifact, message, Some(signature_hex))
}

/// Validate a bundle id against the reserved character set.
/// **Wire-frozen** — these are the only characters the filesystem
/// layer will accept for a bundle directory name. Any caller-
/// provided id outside this set must be rejected BEFORE it touches
/// the filesystem to prevent path traversal (`..`, slashes, null
/// bytes) or platform-specific filename weirdness.
fn validate_bundle_id(id: &str) -> Result<(), WorkbenchHttpError> {
    gadgetron_bundle_sdk::BundleId::parse_legacy(id)
        .map(|_| ())
        .map_err(|error| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "bundle id validation failed: {error}"
            )))
        })
}

pub(crate) fn validate_package_contract(
    package_toml: &str,
    catalog_id: &str,
    catalog_version: &str,
) -> Result<gadgetron_bundle_host::ValidatedPackageContract, WorkbenchHttpError> {
    let core_version = semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: invalid Core build version: {error}"
        )))
    })?;
    let contract =
        gadgetron_bundle_host::ValidatedPackageContract::parse(package_toml, &core_version)
            .map_err(|error| {
                WorkbenchHttpError::Core(GadgetronError::Config(format!(
                    "bundle install: package.toml validation failed: {error}"
                )))
            })?;
    let package = contract.manifest();

    if package.bundle.id.as_str() != catalog_id {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: package.toml Bundle id {:?} does not exactly match catalog id {catalog_id:?}",
            package.bundle.id.as_str()
        ))));
    }
    let package_version = package.bundle.version.to_string();
    if package_version != catalog_version {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: package.toml version {package_version:?} does not exactly match catalog version {catalog_version:?}"
        ))));
    }
    Ok(contract)
}

// The request ceiling includes base64 expansion, signed manifests and bounded
// assets. Keep it route-local so ordinary Workbench JSON retains Axum's smaller
// default limit.
pub(crate) const MAX_BUNDLE_SOURCE_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_BUNDLE_RUNTIME_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
const MAX_BUNDLE_PACKAGE_ASSET_BYTES: usize = 512 * 1024;

fn validate_runtime_artifact(
    package: Option<&gadgetron_bundle_host::ValidatedPackageContract>,
    encoded: Option<&str>,
) -> Result<Option<(gadgetron_bundle_sdk::RelativePath, Vec<u8>)>, WorkbenchHttpError> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use sha2::{Digest, Sha256};

    let package = match (package, encoded) {
        (None, None) => return Ok(None),
        (None, Some(_)) => {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "bundle install: runtime_artifact_base64 requires package_toml".into(),
            )))
        }
        (Some(package), None)
            if matches!(
                package.manifest().runtime.kind,
                gadgetron_bundle_sdk::RuntimeKind::Subprocess
                    | gadgetron_bundle_sdk::RuntimeKind::Wasm
            ) =>
        {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "bundle install: subprocess and Wasm packages require runtime_artifact_base64"
                    .into(),
            )))
        }
        (Some(_), None) => return Ok(None),
        (Some(package), Some(_)) => package,
    };
    let encoded = encoded.expect("runtime artifact presence matched above");
    match package.manifest().runtime.kind {
        gadgetron_bundle_sdk::RuntimeKind::Subprocess | gadgetron_bundle_sdk::RuntimeKind::Wasm => {
        }
        _ => {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "bundle install: runtime artifact bytes are supported only for subprocess or Wasm packages".into(),
            )));
        }
    }
    let bytes = STANDARD.decode(encoded).map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: runtime artifact is not valid base64: {error}"
        )))
    })?;
    if bytes.is_empty() || bytes.len() > MAX_BUNDLE_RUNTIME_ARTIFACT_BYTES {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: runtime artifact must contain 1-{MAX_BUNDLE_RUNTIME_ARTIFACT_BYTES} bytes (got {})",
            bytes.len()
        ))));
    }
    let expected = package
        .manifest()
        .runtime
        .entry_sha256
        .as_deref()
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "bundle install: package runtime does not pin entry_sha256".into(),
            ))
        })?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: runtime artifact digest mismatch (expected {expected}, got {actual})"
        ))));
    }
    let relative = gadgetron_bundle_sdk::RelativePath::new(
        package.manifest().runtime.entry.clone(),
    )
    .map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: invalid runtime entry path: {error}"
        )))
    })?;
    Ok(Some((relative, bytes)))
}

fn validate_package_assets(
    package: Option<&gadgetron_bundle_host::ValidatedPackageContract>,
    encoded_assets: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<(gadgetron_bundle_sdk::RelativePath, Vec<u8>)>, WorkbenchHttpError> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use sha2::{Digest, Sha256};

    let Some(package) = package else {
        if encoded_assets.is_empty() {
            return Ok(Vec::new());
        }
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "bundle install: package_assets_base64 requires package_toml".into(),
        )));
    };
    let capabilities = &package.manifest().capabilities;
    let mut expected = Vec::new();
    expected.extend(
        capabilities
            .domain_schemas
            .iter()
            .map(|schema| (&schema.schema_path, schema.sha256.as_str())),
    );
    expected.extend(
        capabilities
            .seed_assets
            .iter()
            .map(|asset| (&asset.path, asset.sha256.as_str())),
    );
    expected.extend(
        capabilities
            .migrations
            .iter()
            .map(|migration| (&migration.path, migration.sha256.as_str())),
    );
    if expected.len() != encoded_assets.len()
        || expected
            .iter()
            .any(|(path, _)| !encoded_assets.contains_key(path.as_str()))
    {
        let expected_paths: Vec<_> = expected.iter().map(|(path, _)| path.as_str()).collect();
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: package assets must exactly match declared hashed paths {expected_paths:?}"
        ))));
    }

    let mut decoded = Vec::with_capacity(expected.len());
    let mut total = 0_usize;
    for (path, expected_digest) in expected {
        let encoded = encoded_assets
            .get(path.as_str())
            .expect("declared package asset presence checked");
        let bytes = STANDARD.decode(encoded).map_err(|error| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "bundle install: package asset {:?} is not valid base64: {error}",
                path.as_str()
            )))
        })?;
        total = total.saturating_add(bytes.len());
        if bytes.is_empty() || total > MAX_BUNDLE_PACKAGE_ASSET_BYTES {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "bundle install: hashed package assets must contain 1-{MAX_BUNDLE_PACKAGE_ASSET_BYTES} total bytes"
            ))));
        }
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != expected_digest {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "bundle install: package asset {:?} digest mismatch (expected {expected_digest}, got {actual})",
                path.as_str()
            ))));
        }
        if let Some(schema) = capabilities
            .domain_schemas
            .iter()
            .find(|schema| schema.schema_path.as_str() == path.as_str())
        {
            gadgetron_bundle_sdk::DomainOntology::parse_json(&bytes, schema.version).map_err(
                |error| {
                    WorkbenchHttpError::Core(GadgetronError::Config(format!(
                        "bundle install: domain schema {:?} is invalid: {error}",
                        schema.id.as_str()
                    )))
                },
            )?;
        }
        decoded.push((path.clone(), bytes));
    }
    Ok(decoded)
}

fn write_install_file(path: &std::path::Path, bytes: &[u8], unix_mode: u32) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(unix_mode);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub(crate) fn require_runtime_manager(
    state: &AppState,
) -> Result<Arc<crate::web::bundle_runtime::BundleRuntimeManager>, WorkbenchHttpError> {
    require_workbench(state)?
        .runtime_manager
        .clone()
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "external Bundle runtime manager is unavailable; configure [web] bundles_dir and use a supported Linux sandbox host".into(),
            ))
        })
}

pub async fn list_bundle_runtime_status_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::web::bundle_runtime::BundleRuntimeStatus>>, WorkbenchHttpError> {
    Ok(Json(require_runtime_manager(&state)?.list().await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleDependencyPlanQuery {
    #[serde(default)]
    pub change: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
}

pub async fn get_bundle_dependency_plan_handler(
    State(state): State<AppState>,
    Query(query): Query<BundleDependencyPlanQuery>,
) -> Result<Json<gadgetron_bundle_sdk::BundleDependencyPlan>, WorkbenchHttpError> {
    let change = match query.change.as_deref().unwrap_or("none") {
        "none" if query.bundle_id.is_none() => gadgetron_bundle_sdk::BundleLifecycleChange::None,
        "enable" | "disable" => {
            let bundle_id = query.bundle_id.ok_or_else(|| {
                WorkbenchHttpError::Core(GadgetronError::Config(
                    "dependency preview requires bundle_id for enable or disable".into(),
                ))
            })?;
            let bundle_id = gadgetron_bundle_sdk::BundleId::new(bundle_id).map_err(|error| {
                WorkbenchHttpError::Core(GadgetronError::Config(format!(
                    "dependency preview Bundle id is invalid: {error}"
                )))
            })?;
            if query.change.as_deref() == Some("enable") {
                gadgetron_bundle_sdk::BundleLifecycleChange::Enable { bundle_id }
            } else {
                gadgetron_bundle_sdk::BundleLifecycleChange::Disable { bundle_id }
            }
        }
        "none" => {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "bundle_id is not accepted when change=none".into(),
            )))
        }
        other => {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "unknown dependency preview change {other:?}; expected none, enable, or disable"
            ))))
        }
    };
    Ok(Json(
        require_runtime_manager(&state)?
            .dependency_plan(change)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectBundleSetRequest {
    pub set_toml: String,
    #[serde(default)]
    pub signature_hex: Option<String>,
}

pub async fn inspect_bundle_set_handler(
    State(state): State<AppState>,
    Json(request): Json<InspectBundleSetRequest>,
) -> Result<Json<gadgetron_bundle_sdk::BundleSetPlan>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    verify_detached_signature(
        &svc.bundle_signing,
        "Bundle Set",
        request.set_toml.as_bytes(),
        request.signature_hex.as_deref(),
    )?;
    Ok(Json(
        require_runtime_manager(&state)?
            .inspect_bundle_set(&request.set_toml)
            .await?,
    ))
}

pub async fn apply_bundle_set_handler(
    State(state): State<AppState>,
    Json(request): Json<InspectBundleSetRequest>,
) -> Result<Json<crate::web::bundle_runtime::BundleSetApplyOutcome>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    verify_detached_signature(
        &svc.bundle_signing,
        "Bundle Set",
        request.set_toml.as_bytes(),
        request.signature_hex.as_deref(),
    )?;
    Ok(Json(
        require_runtime_manager(&state)?
            .apply_bundle_set(&request.set_toml)
            .await?,
    ))
}

pub async fn get_bundle_runtime_status_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<Json<crate::web::bundle_runtime::BundleRuntimeStatus>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?.status(&bundle_id).await?,
    ))
}

pub async fn enable_bundle_runtime_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(bundle_id): Path<String>,
) -> Result<Json<crate::web::bundle_runtime::BundleRuntimeStatus>, WorkbenchHttpError> {
    let status = require_runtime_manager(&state)?.enable(&bundle_id).await?;
    sync_bundle_vault_owner_state(
        &state,
        &ctx,
        &bundle_id,
        gadgetron_xaas::knowledge_spaces::VaultOwnerState::Enabled,
    )
    .await?;
    Ok(Json(status))
}

pub async fn disable_bundle_runtime_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(bundle_id): Path<String>,
) -> Result<Json<crate::web::bundle_runtime::BundleRuntimeStatus>, WorkbenchHttpError> {
    let status = require_runtime_manager(&state)?.disable(&bundle_id).await?;
    sync_bundle_vault_owner_state(
        &state,
        &ctx,
        &bundle_id,
        gadgetron_xaas::knowledge_spaces::VaultOwnerState::OwnerUnavailable,
    )
    .await?;
    Ok(Json(status))
}

async fn sync_bundle_vault_owner_state(
    state: &AppState,
    ctx: &gadgetron_core::context::TenantContext,
    bundle_id: &str,
    owner_state: gadgetron_xaas::knowledge_spaces::VaultOwnerState,
) -> Result<(), WorkbenchHttpError> {
    let Some(pool) = state.pg_pool.as_ref() else {
        return Ok(());
    };
    gadgetron_xaas::knowledge_spaces::set_bundle_owner_state_system(
        pool,
        ctx.tenant_id,
        bundle_id,
        owner_state,
    )
    .await
    .map(|_| ())
    .map_err(Into::into)
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartBundleJobRequest {
    #[serde(default)]
    pub parameters: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelBundleJobRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

pub async fn start_bundle_job_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path((bundle_id, recipe_id)): Path<(String, String)>,
    Json(request): Json<StartBundleJobRequest>,
) -> Result<Json<gadgetron_bundle_sdk::JobAccepted>, WorkbenchHttpError> {
    let context = gadgetron_bundle_sdk::InvocationContext::new(
        ctx.tenant_id.to_string(),
        ctx.actor_user_id.unwrap_or(ctx.api_key_id).to_string(),
        ctx.request_id.to_string(),
    )
    .with_scopes(ctx.scopes.iter().map(ToString::to_string));
    Ok(Json(
        require_runtime_manager(&state)?
            .start_job(&bundle_id, &recipe_id, request.parameters, context)
            .await?,
    ))
}

pub async fn poll_bundle_job_handler(
    State(state): State<AppState>,
    Path((bundle_id, job_id)): Path<(String, String)>,
) -> Result<Json<gadgetron_bundle_sdk::JobStatusReport>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?
            .poll_job(&bundle_id, &job_id)
            .await?,
    ))
}

pub async fn cancel_bundle_job_handler(
    State(state): State<AppState>,
    Path((bundle_id, job_id)): Path<(String, String)>,
    Json(request): Json<CancelBundleJobRequest>,
) -> Result<Json<gadgetron_bundle_sdk::JobStatusReport>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?
            .cancel_job(&bundle_id, &job_id, request.reason)
            .await?,
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantBundlePermissionsRequest {
    /// Optimistic-lock digest shown by install/runtime status. This prevents an
    /// approval prepared for an older package from authorizing replacement bytes.
    pub package_manifest_sha256: String,
    /// Exact signed permission ids selected by the operator.
    pub permission_ids: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct RevokeBundlePermissionsResponse {
    pub bundle_id: String,
    pub revoked: bool,
    pub runtime_disabled: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBundleSettingsRequest {
    #[serde(default)]
    pub expected_revision: Option<String>,
    pub values: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct BundleKnowledgeAgentRoleView {
    pub declaration: crate::web::bundle_runtime::BundleKnowledgeAgentRoleProjection,
    pub override_profile:
        Option<gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileOverride>,
    pub effective: gadgetron_xaas::knowledge_agent_profiles::EffectiveKnowledgeRoleSelection,
}

#[derive(Debug, serde::Serialize)]
pub struct BundleKnowledgeAgentRolesResponse {
    pub bundle_id: String,
    pub package_manifest_sha256: String,
    pub global: gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection,
    pub roles: Vec<BundleKnowledgeAgentRoleView>,
    pub collections: Vec<crate::web::bundle_runtime::BundleCollectionProfileProjection>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBundleKnowledgeAgentRoleRequest {
    #[serde(default)]
    pub expected_revision: Option<i64>,
    pub selection: gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteBundleKnowledgeAgentRoleRequest {
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CoreKnowledgeAgentRoleDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

const CORE_KNOWLEDGE_AGENT_ROLES: [CoreKnowledgeAgentRoleDescriptor; 4] = [
    CoreKnowledgeAgentRoleDescriptor {
        id: "source_scout",
        label: "Source Scout",
        description: "Finds coverage gaps and proposes sources without collecting them.",
    },
    CoreKnowledgeAgentRoleDescriptor {
        id: "researcher",
        label: "Researcher",
        description: "Builds cited dossiers and structured knowledge candidates.",
    },
    CoreKnowledgeAgentRoleDescriptor {
        id: "gardener",
        label: "Gardener / Distiller",
        description: "Turns reviewed findings into clear, connected knowledge changes.",
    },
    CoreKnowledgeAgentRoleDescriptor {
        id: "insight_synthesizer",
        label: "Insight Synthesizer",
        description: "Connects multiple sources with verified outcomes into reviewable insights.",
    },
];

#[derive(Debug, serde::Serialize)]
pub struct CoreKnowledgeAgentRoleView {
    pub role: CoreKnowledgeAgentRoleDescriptor,
    pub override_profile:
        Option<gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileOverride>,
    pub effective: gadgetron_xaas::knowledge_agent_profiles::EffectiveKnowledgeRoleSelection,
}

#[derive(Debug, serde::Serialize)]
pub struct CoreKnowledgeAgentRolesResponse {
    pub global: gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection,
    pub roles: Vec<CoreKnowledgeAgentRoleView>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutCoreKnowledgeAgentRoleRequest {
    #[serde(default)]
    pub expected_revision: Option<i64>,
    pub selection: gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteCoreKnowledgeAgentRoleRequest {
    pub expected_revision: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct DeleteBundleSshRecordResponse {
    pub bundle_id: String,
    pub id: String,
    pub deleted: bool,
}

pub async fn get_bundle_permission_grant_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<Json<Option<crate::web::bundle_grants::BundlePermissionGrant>>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?.permission_grant(&bundle_id)?,
    ))
}

pub async fn grant_bundle_permissions_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Json(request): Json<GrantBundlePermissionsRequest>,
) -> Result<Json<crate::web::bundle_grants::BundlePermissionGrant>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?
            .grant_permissions(
                &bundle_id,
                &request.package_manifest_sha256,
                request.permission_ids,
            )
            .await?,
    ))
}

pub async fn revoke_bundle_permissions_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<Json<RevokeBundlePermissionsResponse>, WorkbenchHttpError> {
    let manager = require_runtime_manager(&state)?;
    let was_active = manager
        .status(&bundle_id)
        .await
        .map(|status| {
            matches!(
                status.state,
                crate::web::bundle_runtime::BundleRuntimeState::Enabled
                    | crate::web::bundle_runtime::BundleRuntimeState::Probing
            )
        })
        .unwrap_or(false);
    let revoked = manager.revoke_permissions(&bundle_id).await?;
    Ok(Json(RevokeBundlePermissionsResponse {
        bundle_id,
        revoked,
        runtime_disabled: was_active,
    }))
}

pub async fn get_bundle_settings_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<Json<crate::web::bundle_runtime::BundleSettingsProjection>, WorkbenchHttpError> {
    Ok(Json(require_runtime_manager(&state)?.settings(&bundle_id)?))
}

pub async fn put_bundle_settings_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Json(request): Json<PutBundleSettingsRequest>,
) -> Result<Json<crate::web::bundle_runtime::BundleSettingsProjection>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?
            .put_settings(
                &bundle_id,
                request.expected_revision.as_deref(),
                request.values,
            )
            .await?,
    ))
}

pub async fn get_bundle_knowledge_agent_roles_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<BundleKnowledgeAgentRolesResponse>, WorkbenchHttpError> {
    Ok(Json(
        bundle_knowledge_agent_roles_response(&state, &ctx, &bundle_id).await?,
    ))
}

pub async fn get_core_knowledge_agent_roles_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<CoreKnowledgeAgentRolesResponse>, WorkbenchHttpError> {
    Ok(Json(
        core_knowledge_agent_roles_response(&state, &ctx).await?,
    ))
}

pub async fn put_core_knowledge_agent_role_handler(
    State(state): State<AppState>,
    Path(role_id): Path<String>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(request): Json<PutCoreKnowledgeAgentRoleRequest>,
) -> Result<Json<CoreKnowledgeAgentRolesResponse>, WorkbenchHttpError> {
    require_core_knowledge_agent_role(&role_id)?;
    let actor_user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::KnowledgeForbidden)?;
    let pool = require_pg_pool(&state, "Awakening AI role settings")?;
    let requested_effort = request.selection.effort;
    let profile = canonicalize_registered_endpoint_profile(
        pool,
        ctx.tenant_id,
        request.selection.into_profile(),
    )
    .await
    .map_err(WorkbenchHttpError::Core)?;
    if profile
        .effort
        .for_backend_model(profile.backend, &profile.model)
        != requested_effort
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "selected effort is unavailable for this runtime and model".to_string(),
        )));
    }
    let selection =
        gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection::from_profile(&profile);
    gadgetron_xaas::knowledge_agent_profiles::upsert_role_profile_override(
        pool,
        ctx.tenant_id,
        actor_user_id,
        gadgetron_xaas::knowledge_agent_profiles::UpsertKnowledgeRoleProfile {
            scope: gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileScope::Core,
            bundle_id: None,
            role_id: &role_id,
            expected_revision: request.expected_revision,
            selection: &selection,
        },
    )
    .await
    .map_err(knowledge_role_profile_error_to_http)?;
    Ok(Json(
        core_knowledge_agent_roles_response(&state, &ctx).await?,
    ))
}

pub async fn delete_core_knowledge_agent_role_handler(
    State(state): State<AppState>,
    Path(role_id): Path<String>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(request): Json<DeleteCoreKnowledgeAgentRoleRequest>,
) -> Result<Json<CoreKnowledgeAgentRolesResponse>, WorkbenchHttpError> {
    require_core_knowledge_agent_role(&role_id)?;
    gadgetron_xaas::knowledge_agent_profiles::delete_role_profile_override(
        require_pg_pool(&state, "Awakening AI role settings")?,
        ctx.tenant_id,
        gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileScope::Core,
        None,
        &role_id,
        request.expected_revision,
    )
    .await
    .map_err(knowledge_role_profile_error_to_http)?;
    Ok(Json(
        core_knowledge_agent_roles_response(&state, &ctx).await?,
    ))
}

async fn core_knowledge_agent_roles_response(
    state: &AppState,
    ctx: &gadgetron_core::context::TenantContext,
) -> Result<CoreKnowledgeAgentRolesResponse, WorkbenchHttpError> {
    let global_profile = default_conversation_profile(state);
    let global = gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection::from_profile(
        &global_profile,
    );
    let pool = require_pg_pool(state, "Awakening AI role settings")?;
    let mut roles = Vec::with_capacity(CORE_KNOWLEDGE_AGENT_ROLES.len());
    for role in CORE_KNOWLEDGE_AGENT_ROLES {
        let override_profile = gadgetron_xaas::knowledge_agent_profiles::get_role_profile_override(
            pool,
            ctx.tenant_id,
            gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileScope::Core,
            None,
            role.id,
        )
        .await
        .map_err(knowledge_role_profile_error_to_http)?;
        let effective = gadgetron_xaas::knowledge_agent_profiles::resolve_role_profile(
            pool,
            ctx.tenant_id,
            &global_profile,
            role.id,
            None,
        )
        .await
        .map_err(knowledge_role_profile_error_to_http)?;
        roles.push(CoreKnowledgeAgentRoleView {
            role,
            override_profile,
            effective,
        });
    }
    Ok(CoreKnowledgeAgentRolesResponse { global, roles })
}

fn require_core_knowledge_agent_role(
    role_id: &str,
) -> Result<CoreKnowledgeAgentRoleDescriptor, WorkbenchHttpError> {
    CORE_KNOWLEDGE_AGENT_ROLES
        .iter()
        .copied()
        .find(|role| role.id == role_id)
        .ok_or_else(|| WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Unknown Awakening AI role".to_string(),
        })
}

pub async fn put_bundle_knowledge_agent_role_handler(
    State(state): State<AppState>,
    Path((bundle_id, role_id)): Path<(String, String)>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(request): Json<PutBundleKnowledgeAgentRoleRequest>,
) -> Result<Json<BundleKnowledgeAgentRolesResponse>, WorkbenchHttpError> {
    let actor_user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::KnowledgeForbidden)?;
    let projection = require_runtime_manager(&state)?.knowledge_profiles(&bundle_id)?;
    if !projection
        .roles
        .iter()
        .any(|role| role.role.id.as_str() == role_id)
    {
        return Err(WorkbenchHttpError::BundleOperationFailed {
            code: "bundle_ai_role_not_declared".to_string(),
            detail: "This Bundle does not declare the selected AI role".to_string(),
        });
    }
    let pool = require_pg_pool(&state, "Bundle AI role settings")?;
    let requested_effort = request.selection.effort;
    let profile = canonicalize_registered_endpoint_profile(
        pool,
        ctx.tenant_id,
        request.selection.into_profile(),
    )
    .await
    .map_err(WorkbenchHttpError::Core)?;
    if profile
        .effort
        .for_backend_model(profile.backend, &profile.model)
        != requested_effort
    {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "selected effort is unavailable for this runtime and model".to_string(),
        )));
    }
    let selection =
        gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection::from_profile(&profile);
    gadgetron_xaas::knowledge_agent_profiles::upsert_role_profile_override(
        pool,
        ctx.tenant_id,
        actor_user_id,
        gadgetron_xaas::knowledge_agent_profiles::UpsertKnowledgeRoleProfile {
            scope: gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileScope::Bundle,
            bundle_id: Some(&bundle_id),
            role_id: &role_id,
            expected_revision: request.expected_revision,
            selection: &selection,
        },
    )
    .await
    .map_err(knowledge_role_profile_error_to_http)?;
    Ok(Json(
        bundle_knowledge_agent_roles_response(&state, &ctx, &bundle_id).await?,
    ))
}

pub async fn delete_bundle_knowledge_agent_role_handler(
    State(state): State<AppState>,
    Path((bundle_id, role_id)): Path<(String, String)>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(request): Json<DeleteBundleKnowledgeAgentRoleRequest>,
) -> Result<Json<BundleKnowledgeAgentRolesResponse>, WorkbenchHttpError> {
    require_runtime_manager(&state)?.knowledge_profiles(&bundle_id)?;
    gadgetron_xaas::knowledge_agent_profiles::delete_role_profile_override(
        require_pg_pool(&state, "Bundle AI role settings")?,
        ctx.tenant_id,
        gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileScope::Bundle,
        Some(&bundle_id),
        &role_id,
        request.expected_revision,
    )
    .await
    .map_err(knowledge_role_profile_error_to_http)?;
    Ok(Json(
        bundle_knowledge_agent_roles_response(&state, &ctx, &bundle_id).await?,
    ))
}

async fn bundle_knowledge_agent_roles_response(
    state: &AppState,
    ctx: &gadgetron_core::context::TenantContext,
    bundle_id: &str,
) -> Result<BundleKnowledgeAgentRolesResponse, WorkbenchHttpError> {
    let projection = require_runtime_manager(state)?.knowledge_profiles(bundle_id)?;
    let global_profile = default_conversation_profile(state);
    let global = gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleSelection::from_profile(
        &global_profile,
    );
    let pool = require_pg_pool(state, "Bundle AI role settings")?;
    let mut roles = Vec::with_capacity(projection.roles.len());
    for declaration in projection.roles {
        let role_id = declaration.role.id.as_str();
        let core_role_id = declaration.role.core_role.as_str();
        let override_profile = gadgetron_xaas::knowledge_agent_profiles::get_role_profile_override(
            pool,
            ctx.tenant_id,
            gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileScope::Bundle,
            Some(bundle_id),
            role_id,
        )
        .await
        .map_err(knowledge_role_profile_error_to_http)?;
        let effective = gadgetron_xaas::knowledge_agent_profiles::resolve_role_profile(
            pool,
            ctx.tenant_id,
            &global_profile,
            core_role_id,
            Some((bundle_id, role_id)),
        )
        .await
        .map_err(knowledge_role_profile_error_to_http)?;
        roles.push(BundleKnowledgeAgentRoleView {
            declaration,
            override_profile,
            effective,
        });
    }
    Ok(BundleKnowledgeAgentRolesResponse {
        bundle_id: projection.bundle_id,
        package_manifest_sha256: projection.package_manifest_sha256,
        global,
        roles,
        collections: projection.collections,
    })
}

fn knowledge_role_profile_error_to_http(
    error: gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileError,
) -> WorkbenchHttpError {
    use gadgetron_xaas::knowledge_agent_profiles::KnowledgeRoleProfileError;
    match error {
        KnowledgeRoleProfileError::InvalidInput(detail)
        | KnowledgeRoleProfileError::InvalidPersisted(detail) => {
            WorkbenchHttpError::Core(GadgetronError::Config(detail))
        }
        KnowledgeRoleProfileError::Conflict => WorkbenchHttpError::BundleConflict {
            detail: "AI role settings changed; refresh before saving".to_string(),
        },
        KnowledgeRoleProfileError::Database(error) => {
            WorkbenchHttpError::Core(GadgetronError::Database {
                kind: DatabaseErrorKind::QueryFailed,
                message: format!("Knowledge AI role profile: {error}"),
            })
        }
    }
}

pub async fn list_bundle_ssh_targets_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<crate::web::bundle_targets::BundleSshTargetList>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?.list_ssh_targets(ctx.tenant_id, &bundle_id)?,
    ))
}

pub async fn bootstrap_bundle_ssh_target_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(mut request): Json<crate::web::bundle_targets::BootstrapBundleSshTargetRequest>,
) -> Result<Json<crate::web::bundle_targets::BootstrapBundleSshTargetResponse>, WorkbenchHttpError>
{
    if let Some(user_id) = ctx.actor_user_id {
        request.bind_registration_actor(user_id);
    }
    request.acting_space_id = match (state.pg_pool.as_ref(), ctx.actor_user_id) {
        (Some(pool), Some(user_id)) => {
            resolve_bootstrap_acting_space(pool, ctx.tenant_id, user_id, request.acting_space_id)
                .await
        }
        _ => None,
    };
    let context = bootstrap_invocation_context(&ctx, request.acting_space_id);
    Ok(Json(
        require_runtime_manager(&state)?
            .bootstrap_ssh_target(ctx.tenant_id, &bundle_id, request, context)
            .await?,
    ))
}

fn bootstrap_invocation_context(
    ctx: &gadgetron_core::context::TenantContext,
    acting_space_id: Option<Uuid>,
) -> gadgetron_bundle_sdk::InvocationContext {
    let mut context = gadgetron_bundle_sdk::InvocationContext::new(
        ctx.tenant_id.to_string(),
        ctx.actor_user_id.unwrap_or(ctx.api_key_id).to_string(),
        ctx.request_id.to_string(),
    )
    .with_scopes(ctx.scopes.iter().map(ToString::to_string));
    if let Some(acting_space_id) = acting_space_id {
        context = context.with_acting_space_id(acting_space_id.to_string());
    }
    context
}

async fn resolve_bootstrap_acting_space(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    requested_space_id: Option<Uuid>,
) -> Option<Uuid> {
    let effective_spaces = match gadgetron_xaas::knowledge_spaces::effective_spaces(
        pool,
        gadgetron_xaas::knowledge_spaces::SpaceActor { tenant_id, user_id },
    )
    .await
    {
        Ok(spaces) => spaces,
        Err(error) => {
            tracing::warn!(
                target: "bundle_targets",
                tenant_id = %tenant_id,
                user_id = %user_id,
                detail = %error,
                "SSH target registration could not validate its operating Space"
            );
            return None;
        }
    };
    let is_operating_space = |candidate: &&gadgetron_xaas::knowledge_spaces::EffectiveSpaceRow| {
        candidate.space.status == "active"
            && matches!(candidate.space.kind.as_str(), "project" | "team")
            && candidate.effective_role != gadgetron_xaas::knowledge_spaces::SpaceRole::Viewer
    };
    if let Some(requested_space) = requested_space_id.and_then(|requested_space_id| {
        effective_spaces
            .iter()
            .filter(is_operating_space)
            .find(|candidate| candidate.space.id == requested_space_id)
            .map(|candidate| candidate.space.id)
    }) {
        return Some(requested_space);
    }
    let default_space_id = match gadgetron_xaas::default_onboarding::ensure_default_team_onboarding(
        pool, tenant_id, user_id,
    )
    .await
    {
        Ok(Some(topology)) => Some(topology.space_id),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(
                target: "bundle_targets",
                tenant_id = %tenant_id,
                user_id = %user_id,
                detail = %error,
                "SSH target registration could not resolve the actor's default Team Space"
            );
            None
        }
    };
    default_space_id.or_else(|| {
        effective_spaces
            .iter()
            .filter(is_operating_space)
            .find(|candidate| candidate.space.kind == "team")
            .map(|candidate| candidate.space.id)
    })
}

pub async fn reapply_bundle_ssh_target_setup_handler(
    State(state): State<AppState>,
    Path((bundle_id, target_id)): Path<(String, String)>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(request): Json<crate::web::bundle_targets::ReapplyBundleSshTargetSetupRequest>,
) -> Result<Json<crate::web::bundle_targets::ReapplyBundleSshTargetSetupResponse>, WorkbenchHttpError>
{
    let context = gadgetron_bundle_sdk::InvocationContext::new(
        ctx.tenant_id.to_string(),
        ctx.actor_user_id.unwrap_or(ctx.api_key_id).to_string(),
        ctx.request_id.to_string(),
    )
    .with_scopes(ctx.scopes.iter().map(ToString::to_string));
    Ok(Json(
        require_runtime_manager(&state)?
            .reapply_ssh_target_setup(ctx.tenant_id, &bundle_id, &target_id, request, context)
            .await?,
    ))
}

pub async fn put_bundle_ssh_target_handler(
    State(state): State<AppState>,
    Path((bundle_id, target_id)): Path<(String, String)>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(mut request): Json<crate::web::bundle_targets::PutBundleSshTargetRequest>,
) -> Result<Json<crate::web::bundle_targets::BundleSshTarget>, WorkbenchHttpError> {
    if let Some(user_id) = ctx.actor_user_id {
        request.bind_registration_actor(user_id);
    }
    Ok(Json(
        require_runtime_manager(&state)?
            .put_ssh_target(ctx.tenant_id, &bundle_id, &target_id, request)
            .await?,
    ))
}

pub async fn delete_bundle_ssh_target_handler(
    State(state): State<AppState>,
    Path((bundle_id, target_id)): Path<(String, String)>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<DeleteBundleSshRecordResponse>, WorkbenchHttpError> {
    let context = gadgetron_bundle_sdk::InvocationContext::new(
        ctx.tenant_id.to_string(),
        ctx.actor_user_id.unwrap_or(ctx.api_key_id).to_string(),
        ctx.request_id.to_string(),
    )
    .with_scopes(ctx.scopes.iter().map(ToString::to_string));
    let deleted = require_runtime_manager(&state)?
        .delete_ssh_target(ctx.tenant_id, &bundle_id, &target_id, context)
        .await?;
    Ok(Json(DeleteBundleSshRecordResponse {
        bundle_id,
        id: target_id,
        deleted,
    }))
}

pub async fn list_bundle_ssh_secrets_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<crate::web::bundle_targets::BundleSshSecretList>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?.list_ssh_secrets(ctx.tenant_id, &bundle_id)?,
    ))
}

pub async fn put_bundle_ssh_secret_handler(
    State(state): State<AppState>,
    Path((bundle_id, secret_id)): Path<(String, String)>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Json(request): Json<crate::web::bundle_targets::PutBundleSshSecretRequest>,
) -> Result<Json<crate::web::bundle_targets::BundleSshSecretMetadata>, WorkbenchHttpError> {
    Ok(Json(
        require_runtime_manager(&state)?
            .put_ssh_secret(ctx.tenant_id, &bundle_id, &secret_id, request)
            .await?,
    ))
}

pub async fn delete_bundle_ssh_secret_handler(
    State(state): State<AppState>,
    Path((bundle_id, secret_id)): Path<(String, String)>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
) -> Result<Json<DeleteBundleSshRecordResponse>, WorkbenchHttpError> {
    let deleted = require_runtime_manager(&state)?
        .delete_ssh_secret(ctx.tenant_id, &bundle_id, &secret_id)
        .await?;
    Ok(Json(DeleteBundleSshRecordResponse {
        bundle_id,
        id: secret_id,
        deleted,
    }))
}

fn validate_bundle_install(
    svc: &GatewayWorkbenchService,
    req: &InstallBundleRequest,
) -> Result<(ValidatedBundleInstall, usize, usize), WorkbenchHttpError> {
    verify_detached_signature(
        &svc.bundle_signing,
        "catalog",
        req.bundle_toml.as_bytes(),
        req.signature_hex.as_deref(),
    )?;
    match (
        req.package_toml.as_deref(),
        req.package_signature_hex.as_deref(),
    ) {
        (Some(package_toml), signature) => verify_detached_signature(
            &svc.bundle_signing,
            "package",
            package_toml.as_bytes(),
            signature,
        )?,
        (None, Some(_)) => {
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(
                "bundle install: package_signature_hex was supplied without package_toml".into(),
            )));
        }
        (None, None) => {}
    }

    let file: crate::web::catalog::CatalogFile = toml::from_str(&req.bundle_toml).map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: TOML parse failed: {e}",
        )))
    })?;
    let bundle = file.bundle.ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "bundle install: manifest must declare a [bundle] table with id + version".into(),
        ))
    })?;
    validate_bundle_id(&bundle.id)?;
    let package = req
        .package_toml
        .as_deref()
        .map(|source| validate_package_contract(source, &bundle.id, &bundle.version))
        .transpose()?;
    let package_manifest_sha256 = package
        .as_ref()
        .map(|contract| contract.manifest_sha256().to_string());
    let runtime_artifact =
        validate_runtime_artifact(package.as_ref(), req.runtime_artifact_base64.as_deref())?;
    let package_assets = validate_package_assets(package.as_ref(), &req.package_assets_base64)?;
    let domain_schemas = package
        .as_ref()
        .map(|contract| contract.manifest().capabilities.domain_schemas.clone())
        .unwrap_or_default();
    let permission_ids = package
        .as_ref()
        .map(|contract| {
            contract
                .manifest()
                .permissions
                .iter()
                .map(|permission| permission.id.as_str().to_string())
                .collect()
        })
        .unwrap_or_default();
    let settings_declared = package
        .as_ref()
        .is_some_and(|contract| contract.manifest().capabilities.settings_schema.is_some());
    let runtime_kind = package
        .as_ref()
        .map(|contract| format!("{:?}", contract.manifest().runtime.kind).to_lowercase());
    let bundle_class = package
        .as_ref()
        .and_then(|contract| contract.manifest().bundle.class);
    let action_count = file.actions.len();
    let view_count = file.views.len();
    Ok((
        ValidatedBundleInstall {
            bundle,
            bundle_class,
            package_manifest_sha256,
            runtime_artifact,
            package_assets,
            domain_schemas,
            permission_ids,
            settings_declared,
            runtime_kind,
        },
        action_count,
        view_count,
    ))
}

fn source_digest(envelope: &InstallBundleRequest) -> Result<String, WorkbenchHttpError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(envelope).map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle source cannot be encoded: {error}"
        )))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(super) fn ontology_registry_http_error(
    error: gadgetron_knowledge::OntologyRegistryError,
) -> WorkbenchHttpError {
    match error {
        error @ gadgetron_knowledge::OntologyRegistryError::RevisionConflict { .. } => {
            WorkbenchHttpError::BundleConflict {
                detail: format!("Bundle ontology registration conflict: {error}"),
            }
        }
        gadgetron_knowledge::OntologyRegistryError::Database(error) => {
            WorkbenchHttpError::Core(GadgetronError::Database {
                kind: DatabaseErrorKind::QueryFailed,
                message: format!("Bundle ontology registration failed: {error}"),
            })
        }
        error => WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: ontology registration failed: {error}"
        ))),
    }
}

async fn resolve_bundle_source(
    source: BundlePackageSource,
) -> Result<InstallBundleRequest, WorkbenchHttpError> {
    let url = match source {
        BundlePackageSource::Inline { envelope } => return Ok(envelope),
        BundlePackageSource::Url { url } => url,
    };
    let fetched = super::safe_fetch::fetch_public_https(
        &url,
        super::safe_fetch::SafeFetchPolicy {
            max_bytes: MAX_BUNDLE_SOURCE_REQUEST_BYTES,
            max_redirects: 0,
            allowed_content_types: &["application/json", "application/vnd.gadgetron.bundle+json"],
            allowed_domains: None,
            timeout: std::time::Duration::from_secs(20),
        },
    )
    .await
    .map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle source fetch failed: {error}"
        )))
    })?;
    serde_json::from_slice(&fetched.bytes).map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle source JSON is invalid: {error}"
        )))
    })
}

pub async fn inspect_bundle_source_handler(
    State(state): State<AppState>,
    Json(request): Json<InspectBundleSourceRequest>,
) -> Result<Json<BundleInstallInspection>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let envelope = resolve_bundle_source(request.source).await?;
    let source_sha256 = source_digest(&envelope)?;
    let (plan, action_count, view_count) = validate_bundle_install(&svc, &envelope)?;
    let target_exists = svc
        .bundles_dir
        .as_deref()
        .is_some_and(|dir| std::path::Path::new(dir).join(&plan.bundle.id).exists());
    let warnings = if target_exists {
        vec!["This Bundle is already installed; use upgrade when the version changes.".into()]
    } else {
        Vec::new()
    };
    Ok(Json(BundleInstallInspection {
        bundle_id: plan.bundle.id,
        version: plan.bundle.version,
        bundle_class: plan.bundle_class,
        source_sha256,
        package_manifest_sha256: plan.package_manifest_sha256,
        contract: if envelope.package_toml.is_some() {
            "bundle_sdk_v1"
        } else {
            "catalog_only"
        },
        action_count,
        view_count,
        permission_ids: plan.permission_ids,
        settings_declared: plan.settings_declared,
        runtime_kind: plan.runtime_kind,
        installable: !target_exists,
        upgradeable: target_exists,
        warnings,
    }))
}

pub async fn install_bundle_source_handler(
    State(state): State<AppState>,
    Json(request): Json<InstallBundleSourceRequest>,
) -> Result<Json<InstallBundleResponse>, WorkbenchHttpError> {
    let envelope = resolve_bundle_source(request.source).await?;
    let actual = source_digest(&envelope)?;
    if actual != request.expected_source_sha256 {
        return Err(WorkbenchHttpError::BundleConflict {
            detail: "Bundle source changed after inspection; inspect it again before installing"
                .into(),
        });
    }
    install_bundle_handler(State(state), Json(envelope)).await
}

pub async fn upgrade_bundle_source_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Json(request): Json<InstallBundleSourceRequest>,
) -> Result<Json<InstallBundleResponse>, WorkbenchHttpError> {
    validate_bundle_id(&bundle_id)?;
    let envelope = resolve_bundle_source(request.source).await?;
    if source_digest(&envelope)? != request.expected_source_sha256 {
        return Err(WorkbenchHttpError::BundleConflict {
            detail: "Bundle source changed after inspection; inspect it again before upgrading"
                .into(),
        });
    }
    let svc = require_workbench(&state)?;
    let (plan, _, _) = validate_bundle_install(&svc, &envelope)?;
    if plan.bundle.id != bundle_id {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle upgrade id mismatch: route {bundle_id:?}, package {:?}",
            plan.bundle.id
        ))));
    }
    let root = std::path::Path::new(svc.bundles_dir.as_deref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "bundle upgrade requires `[web] bundles_dir` to be configured".into(),
        ))
    })?);
    let target = root.join(&bundle_id);
    if !target.is_dir() {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle upgrade: {bundle_id:?} is not installed"
        ))));
    }
    if let Some(manager) = svc.runtime_manager.as_ref() {
        manager.disable(&bundle_id).await?;
    }
    let backup = root.join(format!(".{bundle_id}.upgrade-backup-{}", Uuid::new_v4()));
    std::fs::rename(&target, &backup).map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle upgrade: cannot stage rollback package: {error}"
        )))
    })?;
    match install_bundle_handler(State(state), Json(envelope)).await {
        Ok(response) => {
            if let Err(error) = std::fs::remove_dir_all(&backup) {
                tracing::warn!(bundle_id, %error, "bundle upgrade succeeded but backup cleanup failed");
            }
            Ok(response)
        }
        Err(error) => {
            if target.exists() {
                let _ = std::fs::remove_dir_all(&target);
            }
            if let Err(restore_error) = std::fs::rename(&backup, &target) {
                return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
                    "bundle upgrade failed and rollback could not be restored: {restore_error}; original error: {error:?}"
                ))));
            }
            Err(error)
        }
    }
}

pub async fn export_bundle_package_handler(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<Response, WorkbenchHttpError> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    validate_bundle_id(&bundle_id)?;
    let svc = require_workbench(&state)?;
    let root = std::path::Path::new(svc.bundles_dir.as_deref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "bundle export requires `[web] bundles_dir` to be configured".into(),
        ))
    })?);
    let package_root = root.join(&bundle_id);
    let read_text = |name: &str| -> Result<String, WorkbenchHttpError> {
        std::fs::read_to_string(package_root.join(name)).map_err(|error| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "bundle export: cannot read {name}: {error}"
            )))
        })
    };
    let bundle_toml = read_text("bundle.toml")?;
    let signature_hex = package_root
        .join("catalog.sig")
        .exists()
        .then(|| read_text("catalog.sig"))
        .transpose()?
        .map(|value| value.trim().to_string());
    let package_toml = package_root
        .join("package.toml")
        .exists()
        .then(|| read_text("package.toml"))
        .transpose()?;
    let package_signature_hex = package_root
        .join("package.sig")
        .exists()
        .then(|| read_text("package.sig"))
        .transpose()?
        .map(|value| value.trim().to_string());
    let mut runtime_artifact_base64 = None;
    let mut package_assets_base64 = std::collections::BTreeMap::new();
    if let Some(source) = package_toml.as_deref() {
        let catalog: crate::web::catalog::CatalogFile =
            toml::from_str(&bundle_toml).map_err(|error| {
                WorkbenchHttpError::Core(GadgetronError::Config(format!(
                    "bundle export: catalog is invalid: {error}"
                )))
            })?;
        let metadata = catalog.bundle.ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "bundle export: catalog Bundle metadata is missing".into(),
            ))
        })?;
        let contract = validate_package_contract(source, &metadata.id, &metadata.version)?;
        let manifest = contract.manifest();
        if matches!(
            manifest.runtime.kind,
            gadgetron_bundle_sdk::RuntimeKind::Subprocess | gadgetron_bundle_sdk::RuntimeKind::Wasm
        ) {
            let bytes =
                std::fs::read(package_root.join(&manifest.runtime.entry)).map_err(|error| {
                    WorkbenchHttpError::Core(GadgetronError::Config(format!(
                        "bundle export: cannot read runtime artifact: {error}"
                    )))
                })?;
            runtime_artifact_base64 = Some(STANDARD.encode(bytes));
        }
        let paths = manifest
            .capabilities
            .domain_schemas
            .iter()
            .map(|item| &item.schema_path)
            .chain(
                manifest
                    .capabilities
                    .seed_assets
                    .iter()
                    .map(|item| &item.path),
            )
            .chain(
                manifest
                    .capabilities
                    .migrations
                    .iter()
                    .map(|item| &item.path),
            );
        for path in paths {
            let bytes = std::fs::read(package_root.join(path.as_str())).map_err(|error| {
                WorkbenchHttpError::Core(GadgetronError::Config(format!(
                    "bundle export: cannot read asset {:?}: {error}",
                    path.as_str()
                )))
            })?;
            package_assets_base64.insert(path.as_str().to_string(), STANDARD.encode(bytes));
        }
    }
    let envelope = InstallBundleRequest {
        bundle_toml,
        signature_hex,
        package_toml,
        package_signature_hex,
        runtime_artifact_base64,
        package_assets_base64,
    };
    let disposition = format!("attachment; filename=\"{bundle_id}.gadgetron-bundle.json\"");
    Ok((
        [(axum::http::header::CONTENT_DISPOSITION, disposition)],
        Json(envelope),
    )
        .into_response())
}

/// `POST /api/v1/web/workbench/admin/bundles` — install a bundle by
/// writing its manifest to `{bundles_dir}/{id}/bundle.toml`. Does
/// NOT reload the live catalog (operator composes install + reload
/// deliberately).
///
/// Requires scope: **Management**.
///
/// Errors:
/// - 503 `Config` when `bundles_dir` isn't wired.
/// - 400 `Config` on invalid TOML, missing `[bundle]` table, or
///   malformed bundle id (reserved chars, too long).
/// - 409 via `Config` when a bundle with that id already exists —
///   explicit uninstall is required before re-install to avoid
///   silent overwrites.
pub async fn install_bundle_handler(
    State(state): State<AppState>,
    Json(req): Json<InstallBundleRequest>,
) -> Result<Json<InstallBundleResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let dir_cfg = svc.bundles_dir.as_deref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "bundle install requires `[web] bundles_dir` to be configured".into(),
        ))
    })?;

    let (plan, _, _) = validate_bundle_install(&svc, &req)?;
    let bundle_meta = &plan.bundle;
    let bundle_class = plan.bundle_class;
    let package_manifest_sha256 = plan.package_manifest_sha256.clone();
    let runtime_artifact = &plan.runtime_artifact;
    let package_assets = &plan.package_assets;
    let has_package = req.package_toml.is_some();
    let ontology_pool = if plan.domain_schemas.is_empty() {
        None
    } else {
        Some(require_pg_pool(&state, "Bundle ontology registration")?.clone())
    };

    let dir = std::path::Path::new(dir_cfg);
    std::fs::create_dir_all(dir).map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: cannot create bundles directory {dir:?}: {error}"
        )))
    })?;
    let target_dir = dir.join(&bundle_meta.id);
    if target_dir.exists() {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: directory {target_dir:?} already exists — uninstall first",
        ))));
    }

    let staging_dir = dir.join(format!(".{}.installing-{}", bundle_meta.id, Uuid::new_v4()));
    std::fs::create_dir(&staging_dir).map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: cannot create staging directory {staging_dir:?}: {e}",
        )))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging_dir, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                let _ = std::fs::remove_dir_all(&staging_dir);
                WorkbenchHttpError::Core(GadgetronError::Config(format!(
                    "bundle install: cannot secure staging directory: {error}"
                )))
            },
        )?;
    }

    let install_result =
        (|| -> std::io::Result<(Option<std::path::PathBuf>, Option<std::path::PathBuf>)> {
            let package_path = if has_package {
                let path = staging_dir.join("package.toml");
                write_install_file(
                    &path,
                    req.package_toml
                        .as_deref()
                        .expect("validated package source is present")
                        .as_bytes(),
                    0o400,
                )?;
                Some(path)
            } else {
                None
            };
            if let Some(signature) = req.signature_hex.as_deref() {
                write_install_file(
                    &staging_dir.join("catalog.sig"),
                    signature.as_bytes(),
                    0o400,
                )?;
            }
            if let Some(signature) = req.package_signature_hex.as_deref() {
                write_install_file(
                    &staging_dir.join("package.sig"),
                    signature.as_bytes(),
                    0o400,
                )?;
            }
            let runtime_path = if let Some((relative, bytes)) = runtime_artifact.as_ref() {
                let path = staging_dir.join(relative.as_str());
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_install_file(&path, bytes, 0o500)?;
                Some(path)
            } else {
                None
            };
            for (relative, bytes) in package_assets {
                let path = staging_dir.join(relative.as_str());
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_install_file(&path, bytes, 0o400)?;
            }
            // Catalog is written last and hidden staging directories are excluded
            // from reload, so no partial package can enter the live descriptor set.
            write_install_file(
                &staging_dir.join("bundle.toml"),
                req.bundle_toml.as_bytes(),
                0o400,
            )?;
            std::fs::File::open(&staging_dir)?.sync_all()?;
            Ok((package_path, runtime_path))
        })();
    let (staged_package_path, staged_runtime_path) = match install_result {
        Ok(paths) => paths,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "bundle install: cannot stage package: {error}"
            ))));
        }
    };
    if let Some(pool) = ontology_pool {
        let schemas: Vec<_> = plan
            .domain_schemas
            .iter()
            .map(|descriptor| {
                let bytes = package_assets
                    .iter()
                    .find(|(path, _)| path == &descriptor.schema_path)
                    .map(|(_, bytes)| bytes.as_slice())
                    .expect("validated domain schema has matching package bytes");
                gadgetron_knowledge::OntologySchemaRegistration { descriptor, bytes }
            })
            .collect();
        let owner_bundle_id = gadgetron_bundle_sdk::BundleId::new(bundle_meta.id.clone())
            .expect("validated SDK package has a canonical Bundle id");
        let registry = gadgetron_knowledge::OntologyRegistry::new(pool);
        let registration = gadgetron_knowledge::OntologyPackageRegistration {
            owner_bundle_id: &owner_bundle_id,
            package_version: &bundle_meta.version,
            package_manifest_sha256: package_manifest_sha256
                .as_deref()
                .expect("domain schemas require a validated package manifest"),
            schemas: &schemas,
        };
        if let Err(error) = registry.register_package(registration).await {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(ontology_registry_http_error(error));
        }
    }
    if let Err(error) = std::fs::rename(&staging_dir, &target_dir) {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle install: atomic publish to {target_dir:?} failed: {error}"
        ))));
    }
    if let Ok(parent) = std::fs::File::open(dir) {
        let _ = parent.sync_all();
    }
    if let Some(manager) = svc.runtime_manager.as_ref() {
        manager.refresh_installed_row_enrichments();
    }

    let manifest_path = target_dir.join("bundle.toml");
    let package_manifest_path = staged_package_path.map(|path| {
        target_dir.join(
            path.file_name()
                .expect("staged package manifest has a filename"),
        )
    });
    let runtime_artifact_path = staged_runtime_path.map(|path| {
        target_dir.join(
            path.strip_prefix(&staging_dir)
                .expect("staged runtime is inside staging directory"),
        )
    });

    tracing::info!(
        target: "workbench.admin",
        bundle_id = %bundle_meta.id,
        bundle_version = %bundle_meta.version,
        manifest_path = %manifest_path.display(),
        package_manifest_path = ?package_manifest_path,
        package_manifest_sha256 = ?package_manifest_sha256,
        runtime_artifact_path = ?runtime_artifact_path,
        package_asset_count = package_assets.len(),
        contract = if has_package { "bundle_sdk_v1" } else { "catalog_only" },
        "bundle installed (external runtime remains disabled until explicit enable)"
    );

    Ok(Json(InstallBundleResponse {
        installed: true,
        bundle_id: bundle_meta.id.clone(),
        bundle_class,
        manifest_path: manifest_path.to_string_lossy().into_owned(),
        package_manifest_path: package_manifest_path
            .map(|path| path.to_string_lossy().into_owned()),
        package_manifest_sha256,
        runtime_artifact_path: runtime_artifact_path
            .map(|path| path.to_string_lossy().into_owned()),
        package_asset_count: package_assets.len(),
        contract: if has_package {
            "bundle_sdk_v1"
        } else {
            "catalog_only"
        },
        runtime_state: if has_package {
            "installed_not_enabled"
        } else {
            "not_applicable"
        },
        reload_hint: "reload publishes catalog metadata only; POST /admin/bundles/{id}/enable runs the signed sandbox health gate",
    }))
}

/// `DELETE /api/v1/web/workbench/admin/bundles/{bundle_id}` —
/// remove a bundle directory. Like install, does NOT reload the
/// live catalog automatically.
///
/// Errors:
/// - 503 `Config` when `bundles_dir` isn't wired.
/// - 400 `Config` on malformed bundle id.
/// - 404 `Config` when the bundle directory doesn't exist.
pub async fn uninstall_bundle_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    Path(bundle_id): Path<String>,
) -> Result<Json<UninstallBundleResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let dir_cfg = svc.bundles_dir.as_deref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "bundle uninstall requires `[web] bundles_dir` to be configured".into(),
        ))
    })?;
    validate_bundle_id(&bundle_id)?;

    let dir = std::path::Path::new(dir_cfg);
    let target_dir = dir.join(&bundle_id);
    if !target_dir.exists() {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle uninstall: {target_dir:?} does not exist",
        ))));
    }

    let runtime_disabled = if let Some(manager) = svc.runtime_manager.as_ref() {
        if manager.status(&bundle_id).await.is_ok() {
            manager.disable(&bundle_id).await?;
            true
        } else {
            false
        }
    } else {
        false
    };

    sync_bundle_vault_owner_state(
        &state,
        &ctx,
        &bundle_id,
        gadgetron_xaas::knowledge_spaces::VaultOwnerState::OwnerUnavailable,
    )
    .await?;

    std::fs::remove_dir_all(&target_dir).map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundle uninstall: cannot remove {target_dir:?}: {e}",
        )))
    })?;
    if let Some(manager) = svc.runtime_manager.as_ref() {
        manager.refresh_installed_row_enrichments();
    }

    tracing::info!(
        target: "workbench.admin",
        bundle_id = %bundle_id,
        "bundle uninstalled (operator must reload to deactivate)"
    );

    Ok(Json(UninstallBundleResponse {
        uninstalled: true,
        bundle_id,
        runtime_disabled,
        state_preserved: true,
        reload_hint: "POST /api/v1/web/workbench/admin/reload-catalog or send SIGHUP to activate",
    }))
}

// ---------------------------------------------------------------------------
// GET /api/v1/web/workbench/admin/bundles
// ---------------------------------------------------------------------------

/// One bundle entry in the discovery response.
#[derive(Debug, serde::Serialize)]
pub struct BundleDiscoveryEntry {
    /// Bundle metadata from the manifest's `[bundle]` table. `None`
    /// when the TOML file didn't declare one — admin UI should show
    /// a placeholder + nudge the operator to add `[bundle]` because
    /// marketplace operations need the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<crate::web::catalog::BundleMetadata>,
    /// Signed package product class. Missing means a legacy catalog or
    /// manifest that must remain visibly unclassified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_class: Option<gadgetron_bundle_sdk::BundleClass>,
    /// Absolute path to the `bundle.toml` file on disk — useful for
    /// SRE runbooks that want to `cat`/`grep` the manifest directly.
    pub source_path: String,
    /// Number of actions the manifest ships. Pre-computed so the
    /// admin UI doesn't have to re-parse every manifest.
    pub action_count: usize,
    /// Number of views the manifest ships.
    pub view_count: usize,
    /// Signed package projection for control-plane actions. Legacy catalogs
    /// remain visible but have no digest, permissions, settings or runtime.
    pub contract: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provided_capabilities: Vec<gadgetron_bundle_sdk::ProvidedCapability>,
    #[serde(
        default,
        skip_serializing_if = "gadgetron_bundle_sdk::BundleDependencies::is_empty"
    )]
    pub dependencies: gadgetron_bundle_sdk::BundleDependencies,
    pub settings_declared: bool,
    pub agent_role_count: usize,
    pub target_profile_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<crate::web::bundle_runtime::BundleRuntimeStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_grant: Option<crate::web::bundle_grants::BundlePermissionGrant>,
    /// Bounded per-Bundle error. One broken package must not hide healthy
    /// entries from the Manager.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Response shape for `GET /admin/bundles`.
#[derive(Debug, serde::Serialize)]
pub struct ListBundlesResponse {
    /// Directory that was scanned (mirrors `[web] bundles_dir`).
    pub bundles_dir: String,
    /// All discoverable bundles at the time of the call. Order
    /// matches `DescriptorCatalog::from_bundle_dir` (subdir-sorted)
    /// so admin tooling can rely on deterministic listing.
    pub bundles: Vec<BundleDiscoveryEntry>,
    /// Total count — convenience for clients that don't want to
    /// `.len()` the array.
    pub count: usize,
}

/// `GET /api/v1/web/workbench/admin/bundles` — enumerate every
/// bundle under `[web] bundles_dir` without touching the live
/// catalog.
///
/// Read-only discovery: each entry reports its manifest metadata,
/// action/view counts, and the absolute path of its `bundle.toml`.
/// Unlike `/admin/reload-catalog` this does NOT ArcSwap a new
/// snapshot — operators can list what's on disk without affecting
/// any in-flight request.
///
/// Requires scope: **Management** (admin subtree).
///
/// Returns `Config` (503-class) when `bundles_dir` is not wired (a
/// deployment using `catalog_path` or the seed fallback has no
/// directory to enumerate).
pub async fn list_bundles_handler(
    State(state): State<AppState>,
) -> Result<Json<ListBundlesResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let dir_cfg = svc.bundles_dir.as_deref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "bundle discovery requires `[web] bundles_dir` to be configured".into(),
        ))
    })?;
    let dir = std::path::Path::new(dir_cfg);
    let md = std::fs::metadata(dir).map_err(|e| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundles dir: cannot stat {dir:?}: {e}",
        )))
    })?;
    if !md.is_dir() {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "bundles dir: {dir:?} is not a directory",
        ))));
    }

    let mut subdirs: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "bundles dir: cannot read {dir:?}: {e}",
            )))
        })?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();

    let mut bundles = Vec::new();
    for sub in subdirs {
        let manifest = sub.join("bundle.toml");
        if !manifest.exists() {
            continue;
        }
        let c = match crate::web::catalog::DescriptorCatalog::from_toml_file(&manifest) {
            Ok(catalog) => catalog,
            Err(error) => {
                bundles.push(BundleDiscoveryEntry {
                    bundle: None,
                    bundle_class: None,
                    source_path: manifest.to_string_lossy().into_owned(),
                    action_count: 0,
                    view_count: 0,
                    contract: "invalid",
                    package_manifest_sha256: None,
                    permission_ids: Vec::new(),
                    provided_capabilities: Vec::new(),
                    dependencies: gadgetron_bundle_sdk::BundleDependencies::default(),
                    settings_declared: false,
                    agent_role_count: 0,
                    target_profile_count: 0,
                    runtime: None,
                    permission_grant: None,
                    detail: Some(format!("{error:?}").chars().take(2_048).collect()),
                });
                continue;
            }
        };
        use gadgetron_core::context::Scope;
        let all_scopes = [Scope::OpenAiCompat, Scope::Management, Scope::XaasAdmin];
        let mut contract = "catalog_only";
        let mut package_manifest_sha256 = None;
        let mut bundle_class = None;
        let mut permission_ids = Vec::new();
        let mut provided_capabilities = Vec::new();
        let mut dependencies = gadgetron_bundle_sdk::BundleDependencies::default();
        let mut settings_declared = false;
        let mut agent_role_count = 0;
        let mut target_profile_count = 0;
        let mut detail = None;
        let package_path = sub.join("package.toml");
        if package_path.exists() {
            match std::fs::read_to_string(&package_path)
                .map_err(|error| format!("cannot read package.toml: {error}"))
                .and_then(|source| {
                    let bundle = c.bundle().ok_or_else(|| {
                        "package.toml requires catalog Bundle metadata".to_string()
                    })?;
                    validate_package_contract(&source, &bundle.id, &bundle.version)
                        .map_err(|error| format!("{error:?}"))
                }) {
                Ok(package) => {
                    contract = "bundle_sdk_v1";
                    package_manifest_sha256 = Some(package.manifest_sha256().to_string());
                    bundle_class = package.manifest().bundle.class;
                    permission_ids = package
                        .manifest()
                        .permissions
                        .iter()
                        .map(|permission| permission.id.as_str().to_string())
                        .collect();
                    provided_capabilities = package.manifest().capabilities.provides.clone();
                    dependencies = package.manifest().dependencies.clone();
                    settings_declared = package.manifest().capabilities.settings_schema.is_some();
                    agent_role_count = package.manifest().capabilities.agent_roles.len();
                    target_profile_count = package.manifest().capabilities.target_profiles.len();
                }
                Err(error) => detail = Some(error.chars().take(2_048).collect()),
            }
        }
        let runtime = if let (Some(_), Some(manager)) = (
            package_manifest_sha256.as_ref(),
            svc.runtime_manager.as_ref(),
        ) {
            match manager
                .status(c.bundle().map(|bundle| bundle.id.as_str()).unwrap_or(""))
                .await
            {
                Ok(status) => Some(status),
                Err(error) => {
                    if detail.is_none() {
                        detail = Some(format!("{error:?}").chars().take(2_048).collect());
                    }
                    None
                }
            }
        } else {
            None
        };
        let permission_grant = svc.runtime_manager.as_ref().and_then(|manager| {
            c.bundle()
                .and_then(|bundle| manager.permission_grant(&bundle.id).ok().flatten())
        });
        bundles.push(BundleDiscoveryEntry {
            bundle: c.bundle().cloned(),
            bundle_class,
            source_path: manifest.to_string_lossy().into_owned(),
            action_count: c.visible_actions(&all_scopes).len(),
            view_count: c.visible_views(&all_scopes).len(),
            contract,
            package_manifest_sha256,
            permission_ids,
            provided_capabilities,
            dependencies,
            settings_declared,
            agent_role_count,
            target_profile_count,
            runtime,
            permission_grant,
            detail,
        });
    }

    let count = bundles.len();
    Ok(Json(ListBundlesResponse {
        bundles_dir: dir_cfg.to_string(),
        bundles,
        count,
    }))
}

// ---------------------------------------------------------------------------
// POST /api/v1/web/workbench/admin/reload-catalog
// ---------------------------------------------------------------------------

/// Response shape for the catalog reload endpoint. Matches what the
/// admin UI / CLI tooling needs to confirm the swap landed.
#[derive(Debug, serde::Serialize)]
pub struct ReloadCatalogResponse {
    /// Always `true` on HTTP 200. Clients key observability on this
    /// flag rather than on the HTTP status so a structured audit log
    /// can quote the exact wire field.
    pub reloaded: bool,
    /// Number of actions in the catalog AFTER the swap.
    pub action_count: usize,
    /// Number of views in the catalog AFTER the swap.
    pub view_count: usize,
    /// Catalog source identifier — one of `"seed_p2b"` (hand-coded
    /// fallback) or `"config_file"` (TOML at `web.catalog_path`).
    /// Future sources can widen this without breaking clients —
    /// wire-stable enum; unknown values should be tolerated.
    pub source: &'static str,
    /// When `source == "config_file"`, the absolute path that was
    /// read. Absent for `seed_p2b` so a curious operator can tell
    /// at a glance where the catalog came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Bundle metadata carried by the freshly loaded catalog. `Some`
    /// when the TOML file declared a `[bundle]` table; absent for
    /// `seed_p2b` and anonymous flat catalogs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<crate::web::catalog::BundleMetadata>,
    /// Contributing bundles when the catalog came from a bundle
    /// directory. Empty in every other case so the admin UI can
    /// distinguish "single bundle loaded" from "N bundles aggregated"
    /// without a special flag.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub bundles: Vec<crate::web::catalog::BundleMetadata>,
}

/// Atomically swap the in-memory `DescriptorCatalog` for a fresh one.
///
/// Core descriptors are always present. Descriptors loaded from
/// `bundles_dir` or `catalog_path` are validated, merged with Core, and then
/// published through one ArcSwap so requests see a consistent catalog and
/// validator set.
///
/// Requires scope: **Management** (like `/nodes`, `/models/deploy`, etc).
///
/// Returns 503 `{error: {code: "catalog_unwired", message: "..."}}` when
/// no workbench is configured (rare — only headless test builds).
///
pub async fn reload_catalog_handler(
    State(state): State<AppState>,
) -> Result<Json<ReloadCatalogResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    perform_catalog_reload(&svc).map(Json)
}

/// Do the actual catalog reload work, shared between the HTTP handler
/// (`reload_catalog_handler`) and the SIGHUP-driven reloader
/// (`spawn_sighup_reloader`). Producing a
/// `ReloadCatalogResponse` means both paths log identical telemetry,
/// so an operator watching logs sees the same wire shape whether the
/// reload came from `curl` or `kill -HUP`.
pub fn perform_catalog_reload(
    svc: &GatewayWorkbenchService,
) -> Result<ReloadCatalogResponse, WorkbenchHttpError> {
    let catalog_handle = svc.descriptor_catalog.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "catalog reload requires a configured workbench with a descriptor catalog handle"
                .into(),
        ))
    })?;

    // Select one external descriptor source, then add it to the Core seed.
    // Parse, collision, and IO failures leave the old snapshot live.
    let bundles_dir_cfg = svc.bundles_dir.as_deref();
    let catalog_path_cfg = svc.catalog_path.as_deref();
    let (fresh_catalog, source_label, source_path) = if let Some(dir) = bundles_dir_cfg {
        let path = std::path::Path::new(dir);
        match crate::web::catalog::DescriptorCatalog::from_bundle_dir(path) {
            Ok(c) => (
                c.with_core_seed(true).map_err(WorkbenchHttpError::Core)?,
                "bundles_dir",
                Some(dir.to_string()),
            ),
            Err(e) => return Err(WorkbenchHttpError::Core(e)),
        }
    } else if let Some(p) = catalog_path_cfg {
        let path = std::path::Path::new(p);
        match crate::web::catalog::DescriptorCatalog::from_toml_file(path) {
            Ok(c) => {
                let allow_direct_actions = c.allow_direct_actions();
                (
                    c.with_core_seed(allow_direct_actions)
                        .map_err(WorkbenchHttpError::Core)?,
                    "config_file",
                    Some(p.to_string()),
                )
            }
            Err(e) => return Err(WorkbenchHttpError::Core(e)),
        }
    } else {
        (
            crate::web::catalog::DescriptorCatalog::seed_p2b(),
            "seed_p2b",
            None,
        )
    };
    let fresh = fresh_catalog.into_snapshot();

    // Pre-compute the response counts + bundle metadata from the
    // same snapshot we're about to publish, so the response is
    // consistent with the swap we just performed.
    use gadgetron_core::context::Scope;
    let all_scopes = [Scope::OpenAiCompat, Scope::Management, Scope::XaasAdmin];
    let action_count = fresh.catalog.visible_actions(&all_scopes).len();
    let view_count = fresh.catalog.visible_views(&all_scopes).len();
    let bundle = fresh.catalog.bundle().cloned();
    let bundles: Vec<_> = fresh.catalog.contributing_bundles().to_vec();

    catalog_handle.store(Arc::new(fresh));

    tracing::info!(
        target: "workbench.admin",
        action_count = action_count,
        view_count = view_count,
        source = source_label,
        source_path = source_path.as_deref().unwrap_or(""),
        bundle_id = bundle.as_ref().map(|b| b.id.as_str()).unwrap_or(""),
        bundle_version = bundle.as_ref().map(|b| b.version.as_str()).unwrap_or(""),
        "descriptor catalog reloaded (CatalogSnapshot = catalog + validators)"
    );

    Ok(ReloadCatalogResponse {
        reloaded: true,
        action_count,
        view_count,
        source: source_label,
        source_path,
        bundle,
        bundles,
    })
}

/// Spawn a background task that watches for SIGHUP and triggers a
/// catalog reload each time.
///
/// Standard operator workflow: edit `catalog_path`, then
/// `kill -HUP <gadgetron-pid>`. No HTTP endpoint required, no
/// per-service auth ceremony, no cluster-aware fan-out — just the
/// POSIX primitive operators already know. Each HUP triggers the
/// same `perform_catalog_reload` code path as the HTTP handler, so
/// the audit trail + logs look identical.
///
/// Unix-only. On non-Unix platforms this function is a no-op —
/// operators on Windows use the HTTP endpoint instead.
#[cfg(unix)]
pub fn spawn_sighup_reloader(workbench: Arc<GatewayWorkbenchService>) {
    use tokio::signal::unix::{signal, SignalKind};
    tokio::spawn(async move {
        let mut stream = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "workbench.admin",
                    error = %e,
                    "SIGHUP reloader: failed to install signal handler; catalog hot-reload via HUP will not work"
                );
                return;
            }
        };
        tracing::info!(
            target: "workbench.admin",
            "SIGHUP reloader armed — kill -HUP <pid> triggers a descriptor catalog reload"
        );
        while stream.recv().await.is_some() {
            match perform_catalog_reload(&workbench) {
                Ok(resp) => {
                    tracing::info!(
                        target: "workbench.admin",
                        trigger = "sighup",
                        action_count = resp.action_count,
                        view_count = resp.view_count,
                        source = resp.source,
                        source_path = resp.source_path.as_deref().unwrap_or(""),
                        "SIGHUP reload complete"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "workbench.admin",
                        trigger = "sighup",
                        error = %format!("{e:?}"),
                        "SIGHUP reload failed; running snapshot preserved"
                    );
                }
            }
        }
    });
}

#[cfg(not(unix))]
pub fn spawn_sighup_reloader(_workbench: Arc<GatewayWorkbenchService>) {
    tracing::info!(
        target: "workbench.admin",
        "SIGHUP reloader is Unix-only; use POST /admin/reload-catalog on this platform"
    );
}

/// `GET /events/ws` — WebSocket upgrade + tenant-filtered activity
/// feed. Subscribers receive `ActivityEvent` JSON messages in real
/// time; non-matching tenants are filtered out handler-side.
///
/// Protocol is a simple stream of JSON text frames, one event per
/// frame. The client SHOULD close the socket when it no longer
/// needs updates; the server closes when the broadcast channel
/// lags (client will see an `Lagged` frame and must reconnect /
/// re-sync via `/usage/summary`).
pub async fn events_ws_handler(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<gadgetron_core::context::TenantContext>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    let bus = state.activity_bus.clone();
    let tenant_id = ctx.tenant_id;
    ws.on_upgrade(move |socket| events_ws_session(socket, bus, tenant_id))
}

async fn events_ws_session(
    mut socket: axum::extract::ws::WebSocket,
    bus: gadgetron_core::activity_bus::ActivityBus,
    tenant_id: Uuid,
) {
    use axum::extract::ws::Message;
    use tokio::sync::broadcast::error::RecvError;
    let mut rx = bus.subscribe();
    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(event) => {
                        // Deliver to this subscriber when the event tenant
                        // matches OR when the event tenant is nil — the Penny
                        // tool-call emission path (`session.rs::emit_tool_audit_if_needed`)
                        // hardcodes `tenant_id: None` because `LlmProvider::chat_stream`
                        // does not yet thread TenantContext. Treating nil as
                        // "deliver to all authenticated subscribers" keeps the
                        // evidence pane live until that threading lands.
                        if event.tenant_id() != tenant_id
                            && event.tenant_id() != Uuid::nil()
                        {
                            continue;
                        }
                        let Ok(text) = serde_json::to_string(&event) else {
                            continue;
                        };
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Send a structured lag notice + close so the
                        // client knows to reconnect. Silent drop
                        // would hide a real problem.
                        let notice = serde_json::json!({
                            "type": "lag",
                            "missed": n,
                            "message": "subscriber lagged; reconnect to resume",
                        });
                        let _ = socket
                            .send(Message::Text(notice.to_string().into()))
                            .await;
                        break;
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            // Drain client frames — mostly keepalives / a client
            // close. We don't interpret client messages today but
            // reading them keeps the socket healthy.
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the workbench service from state, returning a 503-equivalent if
/// it has not been wired (i.e. `state.workbench` is `None`).
fn require_workbench(state: &AppState) -> Result<Arc<GatewayWorkbenchService>, WorkbenchHttpError> {
    state.workbench.clone().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "workbench service is not wired in this build".into(),
        ))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_bundle_signing {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn make_req(body: &str, sig: Option<&[u8; 64]>) -> InstallBundleRequest {
        InstallBundleRequest {
            bundle_toml: body.to_string(),
            signature_hex: sig.map(hex::encode),
            package_toml: None,
            package_signature_hex: None,
            runtime_artifact_base64: None,
            package_assets_base64: std::collections::BTreeMap::new(),
        }
    }

    fn verify_req(
        cfg: &gadgetron_core::config::BundleSigningConfig,
        req: &InstallBundleRequest,
    ) -> Result<(), WorkbenchHttpError> {
        verify_detached_signature(
            cfg,
            "catalog",
            req.bundle_toml.as_bytes(),
            req.signature_hex.as_deref(),
        )
    }

    fn package_toml(id: &str, version: &str) -> String {
        format!(
            r#"
manifest_version = 1

[bundle]
id = "{id}"
version = "{version}"
publisher = "gadgetron.project"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <1.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/runtime"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 60
"#
        )
    }

    #[test]
    fn verify_accepts_unsigned_when_not_required() {
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![],
            require_signature: false,
        };
        verify_req(&cfg, &make_req("[bundle]\nid=\"x\"\nversion=\"1\"", None))
            .expect("unsigned install is allowed when not required");
    }

    #[test]
    fn verify_rejects_unsigned_when_required() {
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![],
            require_signature: true,
        };
        let err = verify_req(&cfg, &make_req("x", None))
            .expect_err("require_signature=true must reject unsigned");
        assert!(format!("{:?}", err).contains("signature required"));
    }

    #[test]
    fn verify_accepts_valid_signature() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();
        let body = "[bundle]\nid=\"x\"\nversion=\"1\"";
        let sig = sk.sign(body.as_bytes());
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![hex::encode(vk.to_bytes())],
            require_signature: true,
        };
        verify_req(&cfg, &make_req(body, Some(&sig.to_bytes())))
            .expect("valid signature must pass");
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();
        let sig = sk.sign(b"original");
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![hex::encode(vk.to_bytes())],
            require_signature: true,
        };
        let err = verify_req(&cfg, &make_req("tampered", Some(&sig.to_bytes())))
            .expect_err("tampered body must reject");
        assert!(format!("{:?}", err).contains("did not verify"));
    }

    #[test]
    fn verify_rejects_unknown_key() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let untrusted_sk = SigningKey::from_bytes(&[42u8; 32]);
        let body = "[bundle]\nid=\"x\"\nversion=\"1\"";
        let sig = untrusted_sk.sign(body.as_bytes());
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![hex::encode(sk.verifying_key().to_bytes())],
            require_signature: true,
        };
        let err = verify_req(&cfg, &make_req(body, Some(&sig.to_bytes())))
            .expect_err("unknown key must reject");
        assert!(format!("{:?}", err).contains("did not verify"));
    }

    #[test]
    fn verify_rejects_signature_without_trust_anchors() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let body = "x";
        let sig = sk.sign(body.as_bytes());
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![],
            require_signature: false,
        };
        let err = verify_req(&cfg, &make_req(body, Some(&sig.to_bytes())))
            .expect_err("signature with no trust anchors must reject");
        assert!(format!("{:?}", err).contains("no trust anchors"));
    }

    #[test]
    fn package_contract_must_match_catalog_identity_and_version() {
        validate_package_contract(
            &package_toml("server-administrator", "1.0.0"),
            "server-administrator",
            "1.0.0",
        )
        .expect("matching versioned SDK package is accepted");

        let id_error = validate_package_contract(
            &package_toml("restaurant-research", "1.0.0"),
            "server-administrator",
            "1.0.0",
        )
        .expect_err("package and catalog ids must match");
        assert!(format!("{id_error:?}").contains("does not exactly match catalog id"));

        let version_error = validate_package_contract(
            &package_toml("server-administrator", "1.0.1"),
            "server-administrator",
            "1.0.0",
        )
        .expect_err("package and catalog versions must match");
        assert!(format!("{version_error:?}").contains("does not exactly match catalog version"));
    }

    #[test]
    fn signature_policy_applies_to_package_as_a_separate_artifact() {
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![],
            require_signature: true,
        };
        let package = package_toml("server-administrator", "1.0.0");
        let error = verify_detached_signature(&cfg, "package", package.as_bytes(), None)
            .expect_err("required package signature cannot be inherited from catalog");
        assert!(format!("{error:?}").contains("package signature required"));
    }

    #[test]
    fn runtime_artifact_must_match_the_signed_package_digest() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use sha2::{Digest, Sha256};

        let bytes = b"#!/bin/true\n";
        let digest = hex::encode(Sha256::digest(bytes));
        let source = package_toml("artifact-test", "0.1.0").replace(&"a".repeat(64), &digest);
        let contract = validate_package_contract(&source, "artifact-test", "0.1.0").unwrap();
        let encoded = STANDARD.encode(bytes);
        let (path, decoded) = validate_runtime_artifact(Some(&contract), Some(&encoded))
            .unwrap()
            .expect("artifact is present");
        assert_eq!(path.as_str(), "bin/runtime");
        assert_eq!(decoded, bytes);

        let mismatch = STANDARD.encode(b"tampered");
        let error = validate_runtime_artifact(Some(&contract), Some(&mismatch)).unwrap_err();
        assert!(format!("{error:?}").contains("digest mismatch"));
        let error = validate_runtime_artifact(Some(&contract), None).unwrap_err();
        assert!(format!("{error:?}").contains("require runtime_artifact_base64"));
        assert!(validate_runtime_artifact(None, Some(&encoded)).is_err());

        let oversized = STANDARD.encode(vec![0_u8; MAX_BUNDLE_RUNTIME_ARTIFACT_BYTES + 1]);
        let error = validate_runtime_artifact(Some(&contract), Some(&oversized)).unwrap_err();
        assert!(format!("{error:?}").contains(&MAX_BUNDLE_RUNTIME_ARTIFACT_BYTES.to_string()));
    }

    #[test]
    fn package_assets_must_exactly_match_declared_paths_and_digests() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use sha2::{Digest, Sha256};

        let bytes = br#"{"properties":{"entities":{"const":["Place"]},"relations":{"const":["applies_to"]}}}"#;
        let digest = hex::encode(Sha256::digest(bytes));
        let source = format!(
            "{}\n[[capabilities.domain_schemas]]\nid = \"asset-test-schema\"\nversion = 1\nschema_path = \"schema/domain.json\"\nsha256 = \"{digest}\"\n",
            package_toml("asset-test", "0.1.0")
        );
        let contract = validate_package_contract(&source, "asset-test", "0.1.0").unwrap();
        let mut assets = std::collections::BTreeMap::new();
        assets.insert("schema/domain.json".into(), STANDARD.encode(bytes));

        let decoded = validate_package_assets(Some(&contract), &assets).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0.as_str(), "schema/domain.json");
        assert_eq!(decoded[0].1, bytes);

        let missing = std::collections::BTreeMap::new();
        assert!(validate_package_assets(Some(&contract), &missing).is_err());
        assets.insert("schema/domain.json".into(), STANDARD.encode(b"tampered"));
        let error = validate_package_assets(Some(&contract), &assets).unwrap_err();
        assert!(format!("{error:?}").contains("digest mismatch"));
        assert!(validate_package_assets(None, &assets).is_err());
    }

    #[test]
    fn package_install_rejects_digest_correct_invalid_domain_ontology() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use sha2::{Digest, Sha256};

        let bytes = br#"{"format_version":2,"types":[{"id":"Place","label":"Place","family":"entity"}],"relations":[]}"#;
        let digest = hex::encode(Sha256::digest(bytes));
        let source = format!(
            "{}\n[[capabilities.domain_schemas]]\nid = \"invalid-domain\"\nversion = 1\nschema_path = \"schema/domain.json\"\nsha256 = \"{digest}\"\n",
            package_toml("invalid-domain", "0.1.0")
        );
        let contract = validate_package_contract(&source, "invalid-domain", "0.1.0").unwrap();
        let mut assets = std::collections::BTreeMap::new();
        assets.insert("schema/domain.json".into(), STANDARD.encode(bytes));

        let error = validate_package_assets(Some(&contract), &assets)
            .expect_err("unsupported ontology format must fail before install");
        assert!(format!("{error:?}").contains("domain_schema.format_version"));
    }

    #[test]
    fn runtime_enable_requires_trust_even_when_legacy_install_does_not() {
        let cfg = gadgetron_core::config::BundleSigningConfig {
            public_keys_hex: vec![],
            require_signature: false,
        };
        let error =
            verify_required_detached_signature(&cfg, "package", b"package", "00").unwrap_err();
        assert!(format!("{error:?}").contains("no trust anchors"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::scope::scope_guard_middleware;
    use crate::server::AppState;
    use crate::test_helpers::{lazy_pool, TEST_AUDIT_CAPACITY, VALID_TOKEN};
    use crate::web::catalog::DescriptorCatalog;
    use crate::web::projection::InProcessWorkbenchProjection;

    use axum::{body::Body, http::Request, middleware};
    use gadgetron_core::context::Scope;
    use gadgetron_xaas::audit::writer::AuditWriter;
    use gadgetron_xaas::auth::validator::{KeyValidator, ValidatedKey};
    use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
    use std::{collections::HashMap, sync::Arc};
    use tower::ServiceExt;
    use uuid::Uuid;

    // ---------------------------------------------------------------
    // Helper — MockKeyValidator
    // ---------------------------------------------------------------

    struct MockKeyValidator {
        result: Arc<ValidatedKey>,
    }

    impl MockKeyValidator {
        fn new(scopes: Vec<Scope>) -> Self {
            Self {
                result: Arc::new(ValidatedKey {
                    api_key_id: Uuid::new_v4(),
                    tenant_id: Uuid::new_v4(),
                    scopes,
                    user_id: None,
                }),
            }
        }
    }

    #[async_trait::async_trait]
    impl KeyValidator for MockKeyValidator {
        async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
            Ok(self.result.clone())
        }
        async fn invalidate(&self, _key_hash: &str) {}
    }

    fn make_state_with_workbench(
        scopes: Vec<Scope>,
        projection: Arc<dyn WorkbenchProjectionService>,
    ) -> AppState {
        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        AppState {
            key_validator: Arc::new(MockKeyValidator::new(scopes)),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: Some(lazy_pool()),
            no_db: false,
            tui_tx: None,
            workbench: Some(Arc::new(GatewayWorkbenchService {
                projection,
                actions: None,
                approval_store: None,
                policy_evaluator: None,
                gadget_catalog: None,
                descriptor_catalog: None,
                catalog_path: None,
                bundles_dir: None,
                bundle_signing: Default::default(),
                runtime_manager: None,
                gadget_modes: None,
                gadget_mode_reconfigurer: None,
                agent_brain: None,
                agent_config_base: None,
                vault_layout: None,
            })),
            penny_shared_surface: None,
            penny_assembler: None,
            agent_config: Arc::new(gadgetron_core::agent::config::AgentConfig::default()),
            google_oauth: None,
            activity_capture_store: None,
            candidate_coordinator: None,
            activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
            tool_catalog: None,
            gadget_dispatcher: None,
            tool_audit_sink: std::sync::Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
            billing_failures: std::sync::Arc::new(
                gadgetron_xaas::billing::BillingFailureCounter::new(),
            ),
            chat_jobs: std::sync::Arc::new(crate::chat_jobs::JobStore::new()),
        }
    }

    // ---------------------------------------------------------------
    // Test 1: scope mapping
    // ---------------------------------------------------------------

    #[test]
    fn create_user_request_deserializes_avatar_url() {
        let body: CreateUserRequest = serde_json::from_value(serde_json::json!({
            "email": "alice@example.com",
            "display_name": "Alice Kim",
            "avatar_url": "https://cdn.example.com/alice.png",
            "role": "member",
            "password": "temporary"
        }))
        .expect("create user request");

        assert_eq!(body.email, "alice@example.com");
        assert_eq!(body.display_name, "Alice Kim");
        assert_eq!(
            body.avatar_url.as_deref(),
            Some("https://cdn.example.com/alice.png")
        );
    }

    #[test]
    fn update_user_profile_request_deserializes_avatar_url() {
        let body: UpdateUserProfileRequest = serde_json::from_value(serde_json::json!({
            "display_name": "Robert Lee",
            "avatar_url": "data:image/jpeg;base64,avatar"
        }))
        .expect("update user profile request");

        assert_eq!(body.display_name, "Robert Lee");
        assert_eq!(
            body.avatar_url.as_deref(),
            Some("data:image/jpeg;base64,avatar")
        );
    }

    #[test]
    fn create_llm_endpoint_request_deserializes_openai_endpoint() {
        let body: CreateLlmEndpointRequest = serde_json::from_value(serde_json::json!({
            "name": "Gemma 4",
            "kind": "vllm",
            "protocol": "openai_chat",
            "base_url": "http://10.100.1.5:8100",
            "model_id": "cyankiwi/gemma-4-31B-it-AWQ-4bit"
        }))
        .expect("create llm endpoint request");

        assert_eq!(body.name, "Gemma 4");
        assert_eq!(body.kind, "vllm");
        assert_eq!(body.protocol, "openai_chat");
        assert_eq!(body.base_url, "http://10.100.1.5:8100");
        assert_eq!(
            body.model_id.as_deref(),
            Some("cyankiwi/gemma-4-31B-it-AWQ-4bit")
        );
    }

    #[test]
    fn patch_agent_brain_settings_request_accepts_token_value_without_env_name() {
        let body: PatchAgentBrainSettingsRequest = serde_json::from_value(serde_json::json!({
            "mode": "external_proxy",
            "external_base_url": "http://10.100.1.5:8101",
            "model": "gemma4",
            "external_auth_token_env": "",
            "external_auth_token_value": "test-secret-token",
            "custom_model_option": false
        }))
        .expect("patch agent brain request");

        let (settings, token) =
            normalize_agent_brain_settings_patch(body).expect("patch with token value is valid");

        assert_eq!(
            settings.mode,
            gadgetron_core::agent::BrainMode::ExternalProxy
        );
        assert_eq!(settings.external_base_url, "http://10.100.1.5:8101");
        assert_eq!(settings.model, "gemma4");
        assert_eq!(settings.external_auth_token_env, "PENNY_CCR_AUTH_TOKEN");
        let token = token.expect("runtime token");
        assert_eq!(token.env_name, "PENNY_CCR_AUTH_TOKEN");
        assert_eq!(token.value, "test-secret-token");
    }

    #[test]
    fn patch_agent_brain_settings_request_clears_external_fields_for_claude_max() {
        let body: PatchAgentBrainSettingsRequest = serde_json::from_value(serde_json::json!({
            "mode": "claude_max",
            "external_base_url": "http://127.0.0.1:8080",
            "model": "",
            "external_auth_token_env": "PENNY_CCR_AUTH_TOKEN",
            "external_auth_token_value": "test-secret-token",
            "custom_model_option": true
        }))
        .expect("patch agent brain request");

        let (settings, token) =
            normalize_agent_brain_settings_patch(body).expect("claude max patch is valid");

        assert_eq!(settings.mode, gadgetron_core::agent::BrainMode::ClaudeMax);
        assert_eq!(settings.external_base_url, "");
        assert_eq!(settings.external_auth_token_env, "");
        assert!(!settings.custom_model_option);
        assert_eq!(token, None);
    }

    #[test]
    fn patch_agent_brain_settings_normalizes_codex_max_to_xhigh() {
        let body: PatchAgentBrainSettingsRequest = serde_json::from_value(serde_json::json!({
            "mode": "claude_max",
            "backend": "codex_exec",
            "model": "gpt-5.5",
            "model_source": "default",
            "effort": "max"
        }))
        .expect("codex settings patch");

        let (settings, _) =
            normalize_agent_brain_settings_patch(body).expect("codex patch is valid");
        assert_eq!(settings.effort, gadgetron_core::agent::AgentEffort::Xhigh);
    }

    #[test]
    fn patch_agent_brain_settings_preserves_gpt_5_6_max() {
        let body: PatchAgentBrainSettingsRequest = serde_json::from_value(serde_json::json!({
            "mode": "claude_max",
            "backend": "codex_exec",
            "model": "gpt-5.6-sol",
            "model_source": "default",
            "effort": "max"
        }))
        .expect("GPT-5.6 settings patch");

        let (settings, _) =
            normalize_agent_brain_settings_patch(body).expect("GPT-5.6 patch is valid");
        assert_eq!(settings.effort, gadgetron_core::agent::AgentEffort::Max);
    }

    #[test]
    fn patch_agent_brain_settings_applies_ultra_capability_ceiling() {
        let supported: PatchAgentBrainSettingsRequest = serde_json::from_value(serde_json::json!({
            "mode": "claude_max",
            "backend": "codex_exec",
            "model": "gpt-5.6-terra",
            "model_source": "default",
            "effort": "ultra"
        }))
        .expect("GPT-5.6 Terra settings patch");
        let (supported, _) =
            normalize_agent_brain_settings_patch(supported).expect("Ultra patch is valid");
        assert_eq!(supported.effort, gadgetron_core::agent::AgentEffort::Ultra);

        let luna: PatchAgentBrainSettingsRequest = serde_json::from_value(serde_json::json!({
            "mode": "claude_max",
            "backend": "codex_exec",
            "model": "gpt-5.6-luna",
            "model_source": "default",
            "effort": "ultra"
        }))
        .expect("GPT-5.6 Luna settings patch");
        let (luna, _) = normalize_agent_brain_settings_patch(luna).expect("Luna patch is valid");
        assert_eq!(luna.effort, gadgetron_core::agent::AgentEffort::Max);
    }

    #[test]
    fn use_llm_endpoint_request_accepts_write_only_token_value() {
        let body: UseLlmEndpointRequest = serde_json::from_value(serde_json::json!({
            "external_auth_token_value": "test-secret-token"
        }))
        .expect("use endpoint request");

        assert_eq!(
            body.external_auth_token_value.as_deref(),
            Some("test-secret-token")
        );
    }

    #[test]
    fn autodetect_llm_endpoint_request_accepts_host_and_port_only() {
        let body: AutoDetectLlmEndpointRequest = serde_json::from_value(serde_json::json!({
            "host": "10.100.1.5",
            "port": 8100,
            "alias": "gemma4"
        }))
        .expect("autodetect request");

        assert_eq!(body.host, "10.100.1.5");
        assert_eq!(body.port, 8100);
        assert_eq!(body.scheme.as_deref(), None);
        assert_eq!(body.alias.as_deref(), Some("gemma4"));
    }

    #[test]
    fn create_ccr_bridge_request_accepts_local_target() {
        let body: CreateCcrBridgeRequest = serde_json::from_value(serde_json::json!({
            "name": "gemma4-ccr",
            "target_kind": "local",
            "base_url": "http://127.0.0.1:3456",
            "port": 3456,
            "auth_token_env": "PENNY_CCR_AUTH_TOKEN"
        }))
        .expect("ccr bridge request");

        assert_eq!(body.name, "gemma4-ccr");
        assert_eq!(body.target_kind, "local");
        assert_eq!(body.target_host_id, None);
        assert_eq!(body.base_url, "http://127.0.0.1:3456");
        assert_eq!(body.port, 3456);
        assert_eq!(body.auth_token_env.as_deref(), Some("PENNY_CCR_AUTH_TOKEN"));
        validate_ccr_bridge_request(&body).expect("local bridge request is valid");
    }

    #[test]
    fn create_ccr_bridge_request_requires_host_for_registered_server() {
        let body: CreateCcrBridgeRequest = serde_json::from_value(serde_json::json!({
            "name": "gemma4-ccr",
            "target_kind": "registered_server",
            "base_url": "http://10.100.1.5:3456",
            "port": 3456
        }))
        .expect("ccr bridge request");

        assert!(validate_ccr_bridge_request(&body).is_err());
    }

    #[tokio::test]
    async fn active_jobs_lists_only_visible_streaming_jobs() {
        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.0.0-test",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
            dynamic_workbench: None,
        });
        let state = make_state_with_workbench(vec![Scope::OpenAiCompat], projection);

        let tenant = Uuid::new_v4();
        let user = Uuid::new_v4();
        let other_tenant = Uuid::new_v4();
        let other_user = Uuid::new_v4();

        // Visible: same tenant + same user, still streaming.
        let visible = state
            .chat_jobs
            .create(Uuid::new_v4(), Some(user), tenant, "penny".into())
            .await;
        // Omitted: finished.
        let finished = state
            .chat_jobs
            .create(Uuid::new_v4(), Some(user), tenant, "penny".into())
            .await;
        finished.mark_complete().await;
        // Omitted: cross-tenant.
        state
            .chat_jobs
            .create(Uuid::new_v4(), Some(user), other_tenant, "penny".into())
            .await;
        // Omitted: same tenant, different user.
        state
            .chat_jobs
            .create(Uuid::new_v4(), Some(other_user), tenant, "penny".into())
            .await;

        let ctx = gadgetron_core::context::TenantContext {
            tenant_id: tenant,
            api_key_id: Uuid::new_v4(),
            scopes: vec![Scope::OpenAiCompat],
            quota_snapshot: Arc::new(gadgetron_core::context::QuotaSnapshot {
                daily_limit_cents: 0,
                daily_used_cents: 0,
                monthly_limit_cents: 0,
                monthly_used_cents: 0,
            }),
            request_id: Uuid::new_v4(),
            started_at: std::time::Instant::now(),
            actor_user_id: Some(user),
            actor_api_key_id: None,
        };

        let Json(resp) =
            list_active_jobs_handler(axum::extract::State(state), axum::Extension(ctx)).await;
        assert_eq!(resp.jobs.len(), 1, "only the visible streaming job");
        assert_eq!(resp.jobs[0].job_id, visible.job_id);
        assert!(matches!(
            resp.jobs[0].status,
            crate::chat_jobs::JobStatus::Streaming
        ));
    }

    fn test_ctx(tenant: Uuid, user: Option<Uuid>) -> gadgetron_core::context::TenantContext {
        gadgetron_core::context::TenantContext {
            tenant_id: tenant,
            api_key_id: Uuid::new_v4(),
            scopes: vec![Scope::OpenAiCompat],
            quota_snapshot: Arc::new(gadgetron_core::context::QuotaSnapshot {
                daily_limit_cents: 0,
                daily_used_cents: 0,
                monthly_limit_cents: 0,
                monthly_used_cents: 0,
            }),
            request_id: Uuid::new_v4(),
            started_at: std::time::Instant::now(),
            actor_user_id: user,
            actor_api_key_id: None,
        }
    }

    #[tokio::test]
    async fn cancel_job_sets_flag_and_enforces_visibility() {
        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.0.0-test",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
            dynamic_workbench: None,
        });
        let state = make_state_with_workbench(vec![Scope::OpenAiCompat], projection);

        let tenant = Uuid::new_v4();
        let user = Uuid::new_v4();
        let job = state
            .chat_jobs
            .create(Uuid::new_v4(), Some(user), tenant, "penny".into())
            .await;

        // Cross-user cancel must 404 and must NOT set the flag.
        let foreign = cancel_job_handler(
            axum::extract::State(state.clone()),
            axum::Extension(test_ctx(tenant, Some(Uuid::new_v4()))),
            axum::extract::Path(job.job_id),
        )
        .await;
        assert!(matches!(foreign, Err(WorkbenchHttpError::JobNotFound)));
        assert!(!job.is_cancel_requested());

        // Owner cancel sets the flag and returns the snapshot.
        let owned = cancel_job_handler(
            axum::extract::State(state),
            axum::Extension(test_ctx(tenant, Some(user))),
            axum::extract::Path(job.job_id),
        )
        .await
        .expect("owner cancel succeeds");
        assert!(job.is_cancel_requested());
        assert_eq!(owned.0.job_id, job.job_id);
    }

    #[tokio::test]
    async fn scope_guard_maps_workbench_path_to_openai_compat() {
        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.0.0-test",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
            dynamic_workbench: None,
        });
        let state = make_state_with_workbench(vec![Scope::OpenAiCompat], projection);

        let app = axum::Router::new()
            .nest("/api/v1/web/workbench", workbench_routes())
            .route(
                "/api/v1/nodes",
                axum::routing::get(|| async { StatusCode::OK }),
            )
            .layer(middleware::from_fn_with_state(
                state.clone(),
                scope_guard_middleware,
            ))
            .layer(middleware::from_fn(
                crate::middleware::tenant_context::tenant_context_middleware,
            ))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::auth::auth_middleware,
            ))
            .layer(middleware::from_fn(
                crate::middleware::request_id::request_id_middleware,
            ))
            .with_state(state);

        // Workbench bootstrap: OpenAiCompat key → 200.
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/web/workbench/bootstrap")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "OpenAiCompat key must reach workbench bootstrap"
        );

        // Atomic Bundle discovery is part of the same OpenAiCompat surface.
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/web/workbench/capabilities")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let projection: WorkbenchCapabilityProjectionResponse =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(projection.revision, "0".repeat(64));
        assert!(projection.bundles.is_empty());
        assert!(projection.ui_contributions.is_empty());

        // Contribution data shares the authenticated/scope-filtered surface;
        // Core-only returns a non-leaking 404 rather than exposing a route gap.
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/web/workbench/contributions/hidden.widget/data")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Bundle jobs are Manager operations even though their eventual
        // workspace projections are visible through OpenAiCompat.
        let req = Request::builder()
            .method("POST")
            .uri(
                "/api/v1/web/workbench/admin/bundles/server-administrator/job-recipes/server-duty-cycle/start",
            )
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"parameters":{"target_id":"edge-one"}}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Bundle job start must require Management scope"
        );

        // Management route: OpenAiCompat key → 403.
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/nodes")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "/api/v1/nodes must require Management scope"
        );
    }

    // ---------------------------------------------------------------
    // Test 2: bootstrap with knowledge=None
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn bootstrap_returns_empty_when_knowledge_is_none() {
        let proj = InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.1.0",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: None,
        };
        let resp = proj.bootstrap().await.unwrap();
        assert!(resp.active_plugs.is_empty());
        assert!(resp
            .degraded_reasons
            .iter()
            .any(|r| r.contains("knowledge service not wired")),);
    }

    #[tokio::test]
    async fn core_workbench_router_does_not_expose_domain_metrics_routes() {
        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.1.0",
            descriptor_catalog: Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: None,
        });
        let state = make_state_with_workbench(vec![Scope::Management], projection);
        let app = workbench_routes().with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/servers/example-id/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ---------------------------------------------------------------
    // Test 3: bootstrap with knowledge wired (2 plugs)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn bootstrap_lists_plugs_when_knowledge_wired() {
        use gadgetron_core::bundle::PlugId;
        use gadgetron_core::knowledge::{
            AuthenticatedContext, KnowledgeDocument, KnowledgePutRequest, KnowledgeQueryMode,
            KnowledgeWriteReceipt,
        };
        use gadgetron_knowledge::service::KnowledgeServiceBuilder;

        #[derive(Debug)]
        struct FakeStore {
            id: PlugId,
        }
        #[async_trait::async_trait]
        impl gadgetron_core::knowledge::KnowledgeStore for FakeStore {
            fn plug_id(&self) -> &PlugId {
                &self.id
            }
            async fn list(&self, _: &AuthenticatedContext) -> Result<Vec<String>, GadgetronError> {
                Ok(vec![])
            }
            async fn get(
                &self,
                _: &AuthenticatedContext,
                _: &str,
            ) -> Result<Option<KnowledgeDocument>, GadgetronError> {
                Ok(None)
            }
            async fn put(
                &self,
                _: &AuthenticatedContext,
                req: KnowledgePutRequest,
            ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
                Ok(KnowledgeWriteReceipt {
                    path: req.path,
                    canonical_plug: self.id.clone(),
                    revision: "r0".into(),
                    derived_failures: vec![],
                })
            }
            async fn delete(
                &self,
                _: &AuthenticatedContext,
                _: &str,
            ) -> Result<(), GadgetronError> {
                Ok(())
            }
            async fn rename(
                &self,
                _: &AuthenticatedContext,
                _from: &str,
                to: &str,
            ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
                Ok(KnowledgeWriteReceipt {
                    path: to.to_string(),
                    canonical_plug: self.id.clone(),
                    revision: "r0".into(),
                    derived_failures: vec![],
                })
            }
        }

        #[derive(Debug)]
        struct FakeIndex {
            id: PlugId,
        }
        #[async_trait::async_trait]
        impl gadgetron_core::knowledge::KnowledgeIndex for FakeIndex {
            fn plug_id(&self) -> &PlugId {
                &self.id
            }
            fn mode(&self) -> KnowledgeQueryMode {
                KnowledgeQueryMode::Keyword
            }
            async fn search(
                &self,
                _: &AuthenticatedContext,
                _: &gadgetron_core::knowledge::KnowledgeQuery,
            ) -> Result<Vec<gadgetron_core::knowledge::KnowledgeHit>, GadgetronError> {
                Ok(vec![])
            }
            async fn reset(&self) -> Result<(), GadgetronError> {
                Ok(())
            }
            async fn apply(
                &self,
                _: &AuthenticatedContext,
                _: gadgetron_core::knowledge::KnowledgeChangeEvent,
            ) -> Result<(), GadgetronError> {
                Ok(())
            }
        }

        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(Arc::new(FakeStore {
                id: PlugId::new("canonical-wiki").unwrap(),
            }))
            .add_index(Arc::new(FakeIndex {
                id: PlugId::new("keyword-search").unwrap(),
            }))
            .build()
            .unwrap();

        let proj = InProcessWorkbenchProjection {
            knowledge: Some(svc),
            gateway_version: "0.1.0",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: None,
        };
        let resp = proj.bootstrap().await.unwrap();
        assert_eq!(resp.active_plugs.len(), 2);
    }

    // ---------------------------------------------------------------
    // Test 4: activity returns empty
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn activity_returns_empty_with_is_truncated_false() {
        let proj = InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.1.0",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: None,
        };
        let resp = proj.activity(50).await.unwrap();
        assert!(resp.entries.is_empty());
        assert!(!resp.is_truncated);
    }

    // ---------------------------------------------------------------
    // Test 5: activity limit clamped to 100
    // ---------------------------------------------------------------

    #[test]
    fn activity_limit_clamped_to_100() {
        let clamped = 5000_u32.clamp(ACTIVITY_LIMIT_MIN, ACTIVITY_LIMIT_MAX);
        assert_eq!(clamped, 100);
    }

    // ---------------------------------------------------------------
    // Test 6: activity default limit is 50
    // ---------------------------------------------------------------

    #[test]
    fn activity_limit_default_50() {
        assert_eq!(default_activity_limit(), 50);
    }

    // ---------------------------------------------------------------
    // Test 7: request_evidence returns not found
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn request_evidence_returns_not_found_for_unknown_request() {
        let proj = InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.1.0",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: None,
        };
        let id = Uuid::new_v4();
        let err = proj.request_evidence(id).await.unwrap_err();
        match err {
            WorkbenchHttpError::RequestNotFound { request_id } => {
                assert_eq!(request_id, id);
            }
            other => panic!("expected RequestNotFound, got: {:?}", other),
        }
    }

    // ---------------------------------------------------------------
    // Test 8: WorkbenchHttpError serializes OpenAI shape — all variants
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn workbench_http_error_serializes_openai_shape_request_not_found() {
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let err = WorkbenchHttpError::RequestNotFound { request_id: id };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["type"], "invalid_request_error");
        assert_eq!(value["error"]["code"], "workbench_request_not_found");
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("00000000-0000-0000-0000-000000000001"));
    }

    #[tokio::test]
    async fn workbench_http_error_view_not_found() {
        let err = WorkbenchHttpError::ViewNotFound {
            view_id: "my-view".into(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["type"], "invalid_request_error");
        assert_eq!(value["error"]["code"], "workbench_view_not_found");
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("my-view"));
    }

    #[tokio::test]
    async fn workbench_http_error_action_not_found() {
        let err = WorkbenchHttpError::ActionNotFound {
            action_id: "my-action".into(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["type"], "invalid_request_error");
        assert_eq!(value["error"]["code"], "workbench_action_not_found");
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("my-action"));
    }

    #[tokio::test]
    async fn workbench_http_error_action_invalid_args() {
        let err = WorkbenchHttpError::ActionInvalidArgs {
            detail: "query is required".into(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["type"], "invalid_request_error");
        assert_eq!(value["error"]["code"], "workbench_action_invalid_args");
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("query is required"));
    }

    #[tokio::test]
    async fn workbench_http_error_direct_actions_disabled() {
        let err = WorkbenchHttpError::DirectActionsDisabled;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["type"], "permission_error");
        assert_eq!(value["error"]["code"], "forbidden");
    }

    // ---------------------------------------------------------------
    // Test 9: WorkbenchHttpError::Core propagates existing shape
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn workbench_http_error_core_propagates_existing_shape() {
        let err = WorkbenchHttpError::Core(GadgetronError::Forbidden);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["type"], "permission_error");
        assert!(!value["error"]["code"].as_str().unwrap_or("").is_empty());
    }

    #[test]
    fn responses_probe_url_normalizes_v1_once() {
        assert_eq!(
            openai_responses_url("http://127.0.0.1:8000"),
            "http://127.0.0.1:8000/v1/responses"
        );
        assert_eq!(
            openai_responses_url("http://127.0.0.1:8000/v1/"),
            "http://127.0.0.1:8000/v1/responses"
        );
    }

    #[test]
    fn endpoint_request_accepts_openai_responses_protocol() {
        let body = CreateLlmEndpointRequest {
            name: "local-responses".into(),
            kind: "openai_compatible".into(),
            protocol: "openai_responses".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
            target_kind: None,
            target_host_id: None,
            upstream_endpoint_id: None,
            listen_port: None,
            auth_token_env: None,
            model_id: Some("local-model".into()),
        };
        assert!(validate_llm_endpoint_request(&body).is_ok());
    }

    async fn spawn_probe_server(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind probe server");
        let address = listener.local_addr().expect("probe address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve probe fixture");
        });
        (format!("http://{address}"), task)
    }

    #[tokio::test]
    async fn responses_probe_requires_an_actual_function_call() {
        let app = axum::Router::new()
            .route(
                "/v1/models",
                axum::routing::get(|| async {
                    Json(json!({"data": [{"id": "local-tool-model"}]}))
                }),
            )
            .route(
                "/v1/responses",
                axum::routing::post(|Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(body["max_output_tokens"], 1024);
                    assert_eq!(body["tool_choice"], "required");
                    assert_eq!(body["tools"][0]["parameters"]["properties"], json!({}));
                    assert!(body["tools"][0].get("strict").is_none());
                    Json(json!({
                        "output": [{
                            "type": "function_call",
                            "name": "gadgetron_capability_probe",
                            "arguments": "{\"nonce\":\"endpoint-smoke\"}"
                        }]
                    }))
                }),
            );
        let (base_url, server) = spawn_probe_server(app).await;

        let probe = probe_openai_endpoint(&base_url, None, None)
            .await
            .expect("probe succeeds");
        assert!(probe.ready());
        assert_eq!(probe.protocol, "openai_responses");
        assert_eq!(probe.runtime_compatibility, "codex_exec");
        assert_eq!(probe.tool_model_id.as_deref(), Some("local-tool-model"));
        server.abort();
    }

    #[tokio::test]
    async fn models_without_responses_is_bridge_required_not_tool_ready() {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async { Json(json!({"data": [{"id": "chat-only-model"}]})) }),
        );
        let (base_url, server) = spawn_probe_server(app).await;

        let probe = probe_openai_endpoint(&base_url, None, None)
            .await
            .expect("models route is connected");
        assert!(!probe.ready());
        assert_eq!(probe.protocol, "openai_chat");
        assert_eq!(probe.runtime_compatibility, "bridge_required");
        assert_eq!(probe.tool_status, "unsupported");
        server.abort();
    }

    #[tokio::test]
    async fn anthropic_probe_requires_an_actual_tool_use_block() {
        let app = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(|| async {
                Json(json!({
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_probe",
                        "name": "gadgetron_capability_probe",
                        "input": {"nonce": "endpoint-smoke"}
                    }],
                    "stop_reason": "tool_use"
                }))
            }),
        );
        let (base_url, server) = spawn_probe_server(app).await;

        let probe = probe_anthropic_endpoint(&base_url, Some("bridge-model"), None)
            .await
            .expect("messages probe succeeds");
        assert!(probe.ready());
        assert_eq!(probe.protocol, "anthropic_messages");
        assert_eq!(probe.runtime_compatibility, "claude_code");
        server.abort();
    }

    #[test]
    fn endpoint_url_validation_blocks_metadata_and_redirect_paths() {
        assert!(validate_endpoint_base_url("http://169.254.169.254").is_err());
        assert!(validate_endpoint_base_url("http://127.0.0.1:8000/v1").is_ok());
        assert!(validate_endpoint_base_url("http://127.0.0.1:8000/redirect").is_err());
    }

    #[test]
    fn portable_bundle_source_digest_is_stable_and_content_bound() {
        let mut envelope = InstallBundleRequest {
            bundle_toml: "[bundle]\nid='sample'\nversion='1.0.0'\n".into(),
            signature_hex: Some("aa".repeat(64)),
            package_toml: None,
            package_signature_hex: None,
            runtime_artifact_base64: None,
            package_assets_base64: Default::default(),
        };
        let before = source_digest(&envelope).unwrap();
        assert_eq!(before, source_digest(&envelope).unwrap());
        envelope.bundle_toml.push('\n');
        assert_ne!(before, source_digest(&envelope).unwrap());
    }

    #[tokio::test]
    async fn bundle_install_registers_signed_domain_ontology_before_publish() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use gadgetron_testing::harness::pg::PgHarness;
        use sha2::{Digest, Sha256};

        let admin_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(admin) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            eprintln!("skipping ontology install test: PostgreSQL unavailable");
            return;
        };
        let vector: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
            "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
        )
        .fetch_optional(&admin)
        .await;
        admin.close().await;
        if !matches!(vector, Ok(Some(_))) {
            eprintln!("skipping ontology install test: pgvector unavailable");
            return;
        }

        let harness = PgHarness::new().await;
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'ontology-http-test')")
            .bind(tenant_id)
            .execute(harness.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
             VALUES ($1, $2, 'ontology-http@test.invalid', 'Ontology Admin', 'admin', 'test')",
        )
        .bind(user_id)
        .bind(tenant_id)
        .execute(harness.pool())
        .await
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.0.0-test",
            descriptor_catalog: Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: None,
        });
        let mut state = make_state_with_workbench(vec![Scope::Management], projection);
        state.pg_pool = Some(harness.pool().clone());
        Arc::get_mut(state.workbench.as_mut().unwrap())
            .expect("test owns the workbench service")
            .bundles_dir = Some(root.path().to_string_lossy().into_owned());

        let bytes = br#"{"properties":{"entities":{"const":["Place"]},"relations":{"const":["applies_to"]}}}"#;
        let digest = hex::encode(Sha256::digest(bytes));
        let package = format!(
            r#"manifest_version = 1

[bundle]
id = "ontology-install-test"
version = "0.1.0"
publisher = "example.publisher"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <1.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "container"
transport = "json_rpc_stdio"
entry = "unused"

[runtime.limits]
memory_mb = 64
open_files = 32
cpu_seconds = 10

[[capabilities.domain_schemas]]
id = "installation-domain"
version = 1
schema_path = "schema/domain.json"
sha256 = "{digest}"
"#
        );
        let request = InstallBundleRequest {
            bundle_toml: "[bundle]\nid = \"ontology-install-test\"\nversion = \"0.1.0\"\n".into(),
            signature_hex: None,
            package_toml: Some(package),
            package_signature_hex: None,
            runtime_artifact_base64: None,
            package_assets_base64: std::collections::BTreeMap::from([(
                "schema/domain.json".into(),
                STANDARD.encode(bytes),
            )]),
        };

        let Json(response) = install_bundle_handler(State(state.clone()), Json(request))
            .await
            .expect("validated ontology package installs");
        assert!(response.installed);
        assert!(root
            .path()
            .join("ontology-install-test/schema/domain.json")
            .is_file());
        let registered: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_ontology_revisions")
                .fetch_one(harness.pool())
                .await
                .unwrap();
        assert_eq!(registered, 1);

        let context = test_ctx(tenant_id, Some(user_id));
        let Json(registry) = crate::web::knowledge_ontology::list_registry_handler(
            State(state.clone()),
            axum::Extension(context.clone()),
        )
        .await
        .expect("tenant admin can inspect registered ontologies");
        assert_eq!(registry.returned, 1);
        assert_eq!(registry.revisions[0].activation_revision, None);
        let revision_id = registry.revisions[0].revision.id;
        let Json(activation) = crate::web::knowledge_ontology::activate_handler(
            State(state),
            axum::Extension(context),
            Path(revision_id),
            Json(crate::web::knowledge_ontology::ActivationRequest {
                expected_activation_revision: 0,
                reason: "Activate the reviewed ontology".into(),
            }),
        )
        .await
        .expect("tenant activation is an explicit HTTP operation");
        assert_eq!(activation.event.activation_revision, 1);
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn bootstrap_http_context_pins_an_operating_space_and_promotes_personal_to_team() {
        use gadgetron_testing::harness::pg::PgHarness;
        use gadgetron_xaas::knowledge_spaces::{self as spaces, CreateProject, SpaceActor};

        let admin_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(admin) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            eprintln!("skipping SSH bootstrap Space test: PostgreSQL unavailable");
            return;
        };
        admin.close().await;

        let harness = PgHarness::new().await;
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'bootstrap-space-fixture')")
            .bind(tenant_id)
            .execute(harness.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) \
             VALUES ($1, $2, $3, 'Bootstrap operator', 'admin', 'test')",
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("bootstrap-{user_id}@example.test"))
        .execute(harness.pool())
        .await
        .unwrap();
        let actor = SpaceActor { tenant_id, user_id };
        let personal = spaces::ensure_personal_space(harness.pool(), actor, "Personal")
            .await
            .unwrap();
        let project = spaces::create_project(
            harness.pool(),
            actor,
            CreateProject {
                slug: "bootstrap-project".into(),
                title: "Bootstrap project".into(),
                goal: "Register project infrastructure".into(),
                policy: serde_json::json!({}),
            },
        )
        .await
        .unwrap();

        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.0.0-test",
            descriptor_catalog: Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::empty().into_snapshot(),
            )),
            dynamic_workbench: None,
        });
        let mut state = make_state_with_workbench(vec![Scope::Management], projection);
        state.pg_pool = Some(harness.pool().clone());
        let app = Router::new()
            .route(
                "/admin/bundles/{bundle_id}/ssh/targets",
                post(bootstrap_bundle_ssh_target_handler),
            )
            .with_state(state)
            .layer(axum::Extension(test_ctx(tenant_id, Some(user_id))));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/bundles/server-administrator/ssh/targets")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "address": "192.0.2.10",
                            "username": "operator",
                            "password": "one-shot-test-password",
                            "acting_space_id": personal.id,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let selected_project = resolve_bootstrap_acting_space(
            harness.pool(),
            tenant_id,
            user_id,
            Some(project.space.id),
        )
        .await;
        assert_eq!(selected_project, Some(project.space.id));

        let promoted =
            resolve_bootstrap_acting_space(harness.pool(), tenant_id, user_id, Some(personal.id))
                .await
                .expect("Personal registration context is promoted to the default Team");
        let default_team: (String, String) = sqlx::query_as(
            "SELECT kind, title FROM knowledge_spaces WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(promoted)
        .fetch_one(harness.pool())
        .await
        .unwrap();
        assert_eq!(default_team, ("team".into(), "Operations".into()));
        assert_eq!(
            resolve_bootstrap_acting_space(harness.pool(), tenant_id, user_id, None).await,
            Some(promoted)
        );
        let invocation_context =
            bootstrap_invocation_context(&test_ctx(tenant_id, Some(user_id)), Some(promoted));
        assert_eq!(
            invocation_context.acting_space_id,
            Some(promoted.to_string()),
            "the registration HTTP context must pin the resolved operating Space into the verification lease"
        );
        harness.cleanup().await;
    }

    #[test]
    fn bundle_url_fetch_allows_only_public_unicast_addresses() {
        use super::super::safe_fetch::is_public_unicast;
        assert!(is_public_unicast("8.8.8.8".parse().unwrap()));
        for blocked in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!is_public_unicast(blocked.parse().unwrap()), "{blocked}");
        }
    }
}
