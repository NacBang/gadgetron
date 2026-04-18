//! Workbench gateway projection types.
//!
//! Shared wire types used by `gadgetron-gateway` and any future consumers
//! (e.g. `gadgetron-web` shell). All heavy orchestration logic lives in
//! `gadgetron-gateway::web::workbench` and `::web::projection`.
//!
//! Authority doc: `docs/design/core/workbench-shared-types.md`
//! Phase: [P2B]

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
