//! Workbench gateway projection types.
//!
//! Shared wire types used by `gadgetron-gateway` and any future consumers
//! (e.g. `gadgetron-web` shell). All heavy orchestration logic lives in
//! `gadgetron-gateway::web::workbench` and `::web::projection`.
//!
//! Authority doc: `docs/design/core/workbench-shared-types.md`
//! Phase: [P2B]

pub mod approval;

pub use approval::{ApprovalError, ApprovalRequest, ApprovalState, ApprovalStore};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

/// Top-level bootstrap payload returned by `GET /api/v1/web/workbench/bootstrap`.
///
/// The shell calls this on mount to determine whether all knowledge plugs are
/// healthy and what the default model is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchBootstrapResponse {
    pub gateway_version: String,
    pub default_model: Option<String>,
    pub active_plugs: Vec<PlugHealth>,
    pub degraded_reasons: Vec<String>,
    pub knowledge: WorkbenchKnowledgeSummary,
}

/// Per-plug health entry emitted inside [`WorkbenchBootstrapResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlugHealth {
    /// Plug identifier string (Arc<str> flattened to owned String on the wire).
    pub id: String,
    /// Plug role: one of `"canonical"`, `"search"`, `"relation"`, `"extractor"`.
    pub role: String,
    pub healthy: bool,
    /// Human-readable note, e.g. `"stale index >30s"`.
    pub note: Option<String>,
}

/// Summarised knowledge-plane readiness included in bootstrap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchKnowledgeSummary {
    pub canonical_ready: bool,
    pub search_ready: bool,
    pub relation_ready: bool,
    pub last_ingest_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

/// Where a workbench activity entry originated.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActivityOrigin {
    Penny,
    UserDirect,
    System,
}

/// What kind of event a workbench activity entry represents.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActivityKind {
    ChatTurn,
    DirectAction,
    SystemEvent,
}

/// A single entry in the workbench activity feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActivityEntry {
    pub event_id: Uuid,
    pub at: DateTime<Utc>,
    pub origin: WorkbenchActivityOrigin,
    pub kind: WorkbenchActivityKind,
    pub title: String,
    pub request_id: Option<Uuid>,
    pub summary: Option<String>,
}

/// Paginated response for `GET /api/v1/web/workbench/activity`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActivityResponse {
    pub entries: Vec<WorkbenchActivityEntry>,
    pub is_truncated: bool,
}

// ---------------------------------------------------------------------------
// Request evidence
// ---------------------------------------------------------------------------

/// Full evidence projection for a specific gateway request.
///
/// Returned by `GET /api/v1/web/workbench/requests/:request_id/evidence`.
/// Evidence projection from Penny traces wires in PSL-1; this struct is the
/// stable wire shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRequestEvidenceResponse {
    pub request_id: Uuid,
    pub tool_traces: Vec<ToolTraceSummary>,
    pub citations: Vec<CitationSummary>,
    /// Untyped; KC-1 will narrow this to a concrete type.
    pub candidates: Vec<serde_json::Value>,
}

/// Summary of a single gadget tool invocation within a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTraceSummary {
    pub gadget_name: String,
    /// SHA-256 hex prefix (16 chars) of the raw args — not the raw args.
    pub args_digest: String,
    /// One of `"success"`, `"denied"`, `"error"`.
    pub outcome: String,
    pub latency_ms: u64,
}

/// A knowledge citation surfaced inside a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationSummary {
    /// Markdown label, e.g. `"^1"`.
    pub label: String,
    pub page_name: String,
    pub anchor: Option<String>,
}

// ---------------------------------------------------------------------------
// Descriptor types — W3-WEB-2b
// ---------------------------------------------------------------------------

/// Where a view descriptor may be placed in the workbench shell.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchViewPlacement {
    LeftRail,
    CenterMain,
    EvidencePane,
}

/// Where an action descriptor may be surfaced.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActionPlacement {
    LeftRail,
    CenterMain,
    EvidencePane,
    ContextMenu,
}

/// Visual renderer the shell should use for a view payload.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchRendererKind {
    Table,
    Timeline,
    Cards,
    MarkdownDoc,
}

/// Semantic kind of an action (affects approval heuristics and UI affordances).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchActionKind {
    Query,
    Mutation,
    Dangerous,
}

/// Descriptor for a registered workbench view.
///
/// Views are read-only; their data is served via `data_endpoint` which
/// must be a same-origin gateway canonical route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchViewDescriptor {
    pub id: String,
    pub title: String,
    pub owner_bundle: String,
    pub source_kind: String,
    pub source_id: String,
    pub placement: WorkbenchViewPlacement,
    pub renderer: WorkbenchRendererKind,
    /// Same-origin canonical route, e.g. `/api/v1/web/workbench/views/<id>/data`.
    pub data_endpoint: String,
    pub refresh_seconds: Option<u32>,
    #[serde(default)]
    pub action_ids: Vec<String>,
    /// When `Some`, the view is only returned to actors that hold this scope.
    #[serde(default)]
    pub required_scope: Option<crate::context::Scope>,
    /// Human-readable reason why the view is temporarily disabled. Policy-only
    /// — MUST NOT contain secrets, internal paths, or SQL.
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

/// Descriptor for a registered workbench action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActionDescriptor {
    pub id: String,
    pub title: String,
    pub owner_bundle: String,
    pub source_kind: String,
    pub source_id: String,
    pub gadget_name: Option<String>,
    pub placement: WorkbenchActionPlacement,
    pub kind: WorkbenchActionKind,
    /// JSON Schema object used to validate `InvokeWorkbenchActionRequest.args`.
    pub input_schema: serde_json::Value,
    pub destructive: bool,
    pub requires_approval: bool,
    /// Short hint for Penny about what this action does.
    pub knowledge_hint: String,
    /// When `Some`, the action is only returned/executed for actors with this scope.
    #[serde(default)]
    pub required_scope: Option<crate::context::Scope>,
    /// Human-readable reason why the action is disabled. Policy-only.
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

/// Wire body for `POST /api/v1/web/workbench/actions/:action_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeWorkbenchActionRequest {
    /// Arguments matched against the descriptor's `input_schema`.
    pub args: serde_json::Value,
    /// Optional idempotency key for duplicate-click / retry deduplication.
    /// Server holds a 5-minute TTL replay cache keyed on
    /// `(tenant_id, action_id, client_invocation_id)`.
    #[serde(default)]
    pub client_invocation_id: Option<uuid::Uuid>,
}

/// Result payload inside [`InvokeWorkbenchActionResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchActionResult {
    /// `"ok"` on success, `"pending_approval"` when the action is gated.
    pub status: String,
    /// Set when `status == "pending_approval"`.
    pub approval_id: Option<uuid::Uuid>,
    /// Activity event id — set on the ok path when the action was captured.
    pub activity_event_id: Option<uuid::Uuid>,
    /// Audit event id (set when audit fanout has run).
    pub audit_event_id: Option<uuid::Uuid>,
    /// View ids the UI should immediately re-fetch after this action.
    pub refresh_view_ids: Vec<String>,
    /// Knowledge candidates created by this action (summary shape).
    pub knowledge_candidates: Vec<serde_json::Value>,
    /// Optional action-specific payload (gadget return value, etc.).
    pub payload: Option<serde_json::Value>,
}

/// Response envelope for `POST /api/v1/web/workbench/actions/:action_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeWorkbenchActionResponse {
    pub result: WorkbenchActionResult,
}

/// Response for `GET /api/v1/web/workbench/views`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRegisteredViewsResponse {
    pub views: Vec<WorkbenchViewDescriptor>,
}

/// Response for `GET /api/v1/web/workbench/actions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRegisteredActionsResponse {
    pub actions: Vec<WorkbenchActionDescriptor>,
}

/// Response for `GET /api/v1/web/workbench/knowledge-status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchKnowledgeStatusResponse {
    pub canonical_ready: bool,
    pub search_ready: bool,
    pub relation_ready: bool,
    pub stale_reasons: Vec<String>,
    pub last_ingest_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Response for `GET /api/v1/web/workbench/views/:view_id/data`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchViewData {
    pub view_id: String,
    /// Typed renderer payload. Raw HTML is forbidden; use typed renderer shapes.
    pub payload: serde_json::Value,
}
