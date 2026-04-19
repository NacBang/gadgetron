//! Workbench gateway routes — W3-WEB-2 + W3-WEB-2b.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md`
//!
//! Endpoints mounted at `/api/v1/web/workbench/`:
//!   GET  /bootstrap                        → `get_workbench_bootstrap`
//!   GET  /activity                         → `list_workbench_activity`
//!   GET  /requests/:request_id/evidence    → `get_workbench_request_evidence`
//!   GET  /knowledge-status                 → `get_knowledge_status`
//!   GET  /views                            → `list_views`
//!   GET  /views/:view_id/data              → `load_view_data`
//!   GET  /actions                          → `list_actions`
//!   POST /actions/:action_id               → `invoke_action`

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use gadgetron_core::{
    error::GadgetronError,
    workbench::{
        InvokeWorkbenchActionRequest, InvokeWorkbenchActionResponse, WorkbenchActivityResponse,
        WorkbenchBootstrapResponse, WorkbenchKnowledgeStatusResponse,
        WorkbenchRegisteredActionsResponse, WorkbenchRegisteredViewsResponse,
        WorkbenchRequestEvidenceResponse, WorkbenchViewData,
    },
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::server::AppState;

// ---------------------------------------------------------------------------
// Actor placeholder (doc-10 deferred)
// ---------------------------------------------------------------------------

/// Lightweight actor carrier used until doc-10 promotes identity.
///
/// `AuthenticatedContext` is currently a ZST placeholder. Handlers extract
/// the `TenantContext` from request extensions but forward only this carrier
/// to projection/action services. When doc-10 lands, this type will carry
/// `tenant_id` + `user_id` + scopes.
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
    // --- W3-WEB-2 (bootstrap / activity / evidence) ---

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

    // --- W3-WEB-2b (descriptor catalog + knowledge status + view data) ---

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
        actor_scopes: &[gadgetron_core::context::Scope],
        view_id: &str,
    ) -> Result<WorkbenchViewData, WorkbenchHttpError>;

    /// Actor-visible registered actions. Same scope-filter semantics
    /// as `views`.
    async fn actions(
        &self,
        actor_scopes: &[gadgetron_core::context::Scope],
    ) -> Result<WorkbenchRegisteredActionsResponse, WorkbenchHttpError>;
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
    /// Shared `Arc<ArcSwap<CatalogSnapshot>>` — the same handle both
    /// `projection` and `actions` hold. Exposed here so
    /// `POST /api/v1/web/workbench/admin/reload-catalog` (ISSUE 8 TASK 8.2)
    /// can atomically swap in a fresh snapshot (catalog + validators
    /// together, ISSUE 8 TASK 8.3). `None` in legacy test fixtures
    /// that don't wire a workbench; production always sets it.
    pub descriptor_catalog: Option<Arc<arc_swap::ArcSwap<crate::web::catalog::CatalogSnapshot>>>,
    /// Optional file path the reload handler reads on every call
    /// (ISSUE 8 TASK 8.4). When set, reload parses this TOML file
    /// and swaps in the resulting catalog; when unset, reload falls
    /// back to `DescriptorCatalog::seed_p2b()`. Cloned from
    /// `WebConfig.catalog_path` at startup.
    pub catalog_path: Option<String>,
    /// Optional bundles directory for multi-bundle aggregation
    /// (ISSUE 9 TASK 9.2). When set, reload scans every
    /// `<dir>/<bundle>/bundle.toml` and merges them into one
    /// catalog — winning over `catalog_path` if both are
    /// configured. Cloned from `WebConfig.bundles_dir` at startup.
    pub bundles_dir: Option<String>,
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
    /// The approval has already been resolved (approved or denied).
    ApprovalAlreadyResolved { state: String },
    /// The actor's tenant differs from the approval's tenant.
    ApprovalForbidden,
}

impl From<GadgetronError> for WorkbenchHttpError {
    fn from(err: GadgetronError) -> Self {
        Self::Core(err)
    }
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
// Handlers — W3-WEB-2
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
// Handlers — W3-WEB-2b
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
    let resp = svc.projection.view_data(&ctx.scopes, &view_id).await?;
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

/// `POST /approvals/:approval_id/approve` — resolve a `pending_approval`
/// into an `ok` dispatch.
///
/// Looks up the approval record, marks it Approved on behalf of the
/// calling actor, then hands it to `action_svc.resume_approval` which
/// dispatches the stored gadget with the persisted args. Errors map
/// to HTTP as follows:
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
        user_id: ctx.api_key_id,
        tenant_id: ctx.tenant_id,
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
            resolved_by_user_id: approved.resolved_by_user_id.unwrap_or(actor.user_id),
        },
    );
    let action_id = approved.action_id.clone();
    let resp = action_svc
        .resume_approval(&actor, &ctx.scopes, approved)
        .await?;
    publish_action_activity(&state, &actor, &action_id, &resp);
    Ok(Json(resp))
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
        user_id: ctx.api_key_id,
        tenant_id: ctx.tenant_id,
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
            resolved_by_user_id: denied.resolved_by_user_id.unwrap_or(actor.user_id),
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
    let pool = state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "usage summary requires Postgres (no pool configured)".into(),
        ))
    })?;
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
    let pool = state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "audit event query requires Postgres (no pool configured)".into(),
        ))
    })?;
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
/// audit, ISSUE 5 TASK 5.2).
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
    let pool = state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "tool audit query requires Postgres (no pool configured)".into(),
        ))
    })?;
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
    // `actor.user_id = api_key_id` is the placeholder identity until a
    // real user table lands; `actor.tenant_id` is the real tenant.
    let actor = AuthenticatedContext {
        user_id: ctx.api_key_id,
        tenant_id: ctx.tenant_id,
    };
    let resp = action_svc
        .invoke(&actor, &ctx.scopes, &action_id, request)
        .await?;
    publish_action_activity(&state, &actor, &action_id, &resp);
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
        },
    );
}

// ---------------------------------------------------------------------------
// Route factory
// ---------------------------------------------------------------------------

/// Build the workbench sub-router (W3-WEB-2 + W3-WEB-2b).
///
/// Mount with `.nest("/api/v1/web/workbench", workbench_routes())` in
/// `server.rs` AFTER the scope exception in `middleware/scope.rs` is in place.
pub fn workbench_routes() -> Router<AppState> {
    Router::new()
        // W3-WEB-2
        .route("/bootstrap", get(get_workbench_bootstrap))
        .route("/activity", get(list_workbench_activity))
        .route(
            "/requests/{request_id}/evidence",
            get(get_workbench_request_evidence),
        )
        // W3-WEB-2b
        .route("/knowledge-status", get(get_knowledge_status))
        .route("/views", get(list_views))
        .route("/views/{view_id}/data", get(load_view_data))
        .route("/actions", get(list_actions))
        .route("/actions/{action_id}", post(invoke_action))
        // ISSUE 3 TASK 3.3 — real approval flow.
        .route("/approvals/{approval_id}/approve", post(approve_action))
        .route("/approvals/{approval_id}/deny", post(deny_action))
        // ISSUE 3 TASK 3.4 — tenant-scoped audit event query.
        .route("/audit/events", get(list_audit_events))
        // ISSUE 5 TASK 5.2 — Penny tool-call audit surface.
        .route("/audit/tool-events", get(list_tool_audit_events))
        // ISSUE 4 TASK 4.1 — operator usage summary rollup.
        .route("/usage/summary", get(get_usage_summary))
        // ISSUE 4 TASK 4.3 — live activity WebSocket feed.
        .route("/events/ws", get(events_ws_handler))
        // ISSUE 8 TASK 8.2 — admin catalog hot-reload.
        .route("/admin/reload-catalog", post(reload_catalog_handler))
}

// ---------------------------------------------------------------------------
// POST /api/v1/web/workbench/admin/reload-catalog — ISSUE 8 TASK 8.2
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
    /// Bundle metadata carried by the freshly loaded catalog (ISSUE
    /// 9 TASK 9.1). `Some` when the TOML file declared a `[bundle]`
    /// table; absent for `seed_p2b` and anonymous flat catalogs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<crate::web::catalog::BundleMetadata>,
    /// Contributing bundles when the catalog came from a bundle
    /// directory (ISSUE 9 TASK 9.2). Empty in every other case so
    /// the admin UI can distinguish "single bundle loaded" from "N
    /// bundles aggregated" without a special flag.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub bundles: Vec<crate::web::catalog::BundleMetadata>,
}

/// Atomically swap the in-memory `DescriptorCatalog` for a fresh one.
///
/// Ships as part of ISSUE 8 TASK 8.2. Today the only source is the
/// hand-coded `DescriptorCatalog::seed_p2b()` — so a "reload" produces
/// an identical catalog. The value in the endpoint right now is
/// proving the ArcSwap plumbing lands a fresh `Arc<DescriptorCatalog>`
/// atomically while in-flight requests keep reading their snapshot.
/// TASK 8.3 swaps the source to a config-file watcher.
///
/// Requires scope: **Management** (like `/nodes`, `/models/deploy`, etc).
///
/// Returns 503 `{error: {code: "catalog_unwired", message: "..."}}` when
/// no workbench is configured (rare — only headless test builds).
///
/// **Known limitation (TASK 8.2):** schema validators on
/// `InProcessWorkbenchActionService` are pre-compiled at service
/// construction time and are NOT rebuilt by this endpoint. Because
/// the TASK 8.2 source (`seed_p2b`) produces identical schemas,
/// validators remain correct. When TASK 8.3 adds a file-based source
/// with potentially changed schemas, validator rebuild must land with
/// it — either by rebuilding the whole action service on swap, or by
/// moving validators into the ArcSwap alongside the catalog.
pub async fn reload_catalog_handler(
    State(state): State<AppState>,
) -> Result<Json<ReloadCatalogResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    perform_catalog_reload(&svc).map(Json)
}

/// Do the actual catalog reload work, shared between the HTTP handler
/// (`reload_catalog_handler`) and the SIGHUP-driven reloader
/// (`spawn_sighup_reloader`, ISSUE 8 TASK 8.5). Producing a
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

    // Source precedence: `bundles_dir` (ISSUE 9 TASK 9.2) wins over
    // `catalog_path` (ISSUE 8 TASK 8.4) wins over the hardcoded
    // `seed_p2b()` fallback. Parse / IO failures surface as 500
    // with the error message — the old snapshot stays live so a
    // bad edit can't take the workbench down.
    let bundles_dir_cfg = svc.bundles_dir.as_deref();
    let catalog_path_cfg = svc.catalog_path.as_deref();
    let (fresh_catalog, source_label, source_path) = if let Some(dir) = bundles_dir_cfg {
        let path = std::path::Path::new(dir);
        match crate::web::catalog::DescriptorCatalog::from_bundle_dir(path) {
            Ok(c) => (c, "bundles_dir", Some(dir.to_string())),
            Err(e) => return Err(WorkbenchHttpError::Core(e)),
        }
    } else if let Some(p) = catalog_path_cfg {
        let path = std::path::Path::new(p);
        match crate::web::catalog::DescriptorCatalog::from_toml_file(path) {
            Ok(c) => (c, "config_file", Some(p.to_string())),
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
/// catalog reload each time (ISSUE 8 TASK 8.5).
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
                        if event.tenant_id() != tenant_id {
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
                descriptor_catalog: None,
                catalog_path: None,
                bundles_dir: None,
            })),
            penny_shared_surface: None,
            penny_assembler: None,
            agent_config: Arc::new(gadgetron_core::agent::config::AgentConfig::default()),
            activity_capture_store: None,
            candidate_coordinator: None,
            activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
            tool_catalog: None,
            gadget_dispatcher: None,
            tool_audit_sink: std::sync::Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
        }
    }

    // ---------------------------------------------------------------
    // Test 1: scope mapping
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn scope_guard_maps_workbench_path_to_openai_compat() {
        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.0.0-test",
            descriptor_catalog: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                DescriptorCatalog::seed_p2b().into_snapshot(),
            )),
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
        };
        let resp = proj.bootstrap().await.unwrap();
        assert!(resp.active_plugs.is_empty());
        assert!(resp
            .degraded_reasons
            .iter()
            .any(|r| r.contains("knowledge service not wired")),);
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
}
