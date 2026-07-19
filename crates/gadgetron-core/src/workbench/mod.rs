//! Workbench gateway projection types.
//!
//! Shared wire types used by `gadgetron-gateway` and any future consumers
//! (e.g. `gadgetron-web` shell). All heavy orchestration logic lives in
//! `gadgetron-gateway::web::workbench` and `::web::projection`.

pub mod approval;

pub use approval::{
    ApprovalError, ApprovalPolicyBinding, ApprovalRequest, ApprovalResumeStrategy, ApprovalState,
    ApprovalStore,
};

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
/// This struct is the stable wire shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRequestEvidenceResponse {
    pub request_id: Uuid,
    pub tool_traces: Vec<ToolTraceSummary>,
    pub citations: Vec<CitationSummary>,
    /// Untyped; will narrow to a concrete type later.
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
// Descriptor types
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
    List,
    Detail,
    Graph,
    Form,
    Timeline,
    Dashboard,
    Cards,
    Calendar,
    Map,
    Telemetry,
    Timeseries,
    Operation,
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
    /// Optional signed profile rendered by the Core Knowledge collection UI.
    #[serde(default)]
    pub collection_profile: Option<String>,
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
    /// Opaque digest of the actor-visible signed Bundle capability snapshot.
    /// `None` means no dynamic Bundle surface is wired in this build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_revision: Option<String>,
    pub views: Vec<WorkbenchViewDescriptor>,
}

/// Response for `GET /api/v1/web/workbench/actions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchRegisteredActionsResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_revision: Option<String>,
    pub actions: Vec<WorkbenchActionDescriptor>,
}

/// One enabled Bundle represented in the actor-visible capability aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchCapabilityBundle {
    pub bundle_id: String,
    pub bundle_version: String,
    pub package_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_revision: Option<String>,
    pub published_at_ms: u64,
    pub gadget_names: Vec<String>,
    pub workspace_ids: Vec<String>,
    pub action_ids: Vec<String>,
    pub contribution_ids: Vec<String>,
}

/// Signed declarative contribution after Bundle-local references have been
/// expanded into globally stable ids. The response contains no HTML, CSS,
/// scripts, remote asset URLs, credentials, or executable paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchUiContributionDescriptor {
    pub id: String,
    pub owner_bundle: String,
    pub kind: WorkbenchUiContributionKind,
    pub label: String,
    pub placement: WorkbenchUiContributionPlacement,
    pub order_hint: i32,
    pub icon: WorkbenchUiIconToken,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigation_section: Option<WorkbenchNavigationSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_registry: Option<WorkbenchTargetRegistryKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_profile: Option<WorkbenchTargetProfileDescriptor>,
    pub required_scopes: Vec<String>,
    pub empty_state: String,
    pub error_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gadget_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_schema_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<WorkbenchRendererKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchTargetProfileDescriptor {
    pub id: String,
    pub label: String,
    pub default: bool,
    pub allowed_operations: Vec<String>,
    #[serde(default)]
    pub setup_features: Vec<String>,
    pub bootstrap_input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_route: Option<WorkbenchTargetSshRouteDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkbenchTargetSshRouteDescriptor {
    SshParent {
        activation_parameter: String,
        activation_value: String,
        parent_target_parameter: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkbenchUiContributionKind {
    Workspace,
    Navigation,
    DashboardWidget,
    Command,
    SearchResult,
    SubjectContext,
    ToolResult,
    ReviewPresentation,
    JobPresentation,
    KnowledgeContribution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkbenchUiContributionPlacement {
    Main,
    PrimaryNavigation,
    SecondaryNavigation,
    Dashboard,
    CommandPalette,
    ContextMenu,
    Search,
    PennyContext,
    ToolResult,
    Review,
    Jobs,
    Knowledge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkbenchNavigationSection {
    Workspace,
    Knowledge,
    Operations,
    Diagnostics,
    Planning,
    Oversight,
    Management,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkbenchTargetRegistryKind {
    Ssh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkbenchUiIconToken {
    Activity,
    Calendar,
    Dashboard,
    Document,
    Fleet,
    Graph,
    Jobs,
    Knowledge,
    List,
    Logs,
    Map,
    Review,
    Search,
    Settings,
    Table,
    Terminal,
    Timeline,
}

/// One actor-filtered immutable discovery response. Consumers replace the
/// whole value when `revision` changes; they never join independently fetched
/// nav/view/action revisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchCapabilityProjectionResponse {
    pub revision: String,
    pub bundles: Vec<WorkbenchCapabilityBundle>,
    pub ui_contributions: Vec<WorkbenchUiContributionDescriptor>,
    pub views: Vec<WorkbenchViewDescriptor>,
    pub actions: Vec<WorkbenchActionDescriptor>,
}

impl Default for WorkbenchCapabilityProjectionResponse {
    fn default() -> Self {
        Self {
            revision: "0".repeat(64),
            bundles: Vec::new(),
            ui_contributions: Vec::new(),
            views: Vec::new(),
            actions: Vec::new(),
        }
    }
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
    /// Exact signed capability snapshot used to select a dynamic Bundle
    /// workspace. Static Core views omit this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_revision: Option<String>,
    /// Typed renderer payload. Raw HTML is forbidden; use typed renderer shapes.
    pub payload: serde_json::Value,
}

/// Data returned for one actor-visible signed UI contribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchContributionData {
    pub contribution_id: String,
    pub capability_revision: String,
    /// Typed renderer payload. Raw HTML/script/style/remote code is forbidden.
    pub payload: serde_json::Value,
}

/// Live declarative workspace surface supplied by enabled external Bundles.
///
/// Implementors publish only signed, enabled, healthy descriptors. Discovery
/// is actor-filtered and data reads return through the same authenticated
/// Gadget boundary as chat and direct actions. Core owns this generic seam;
/// domain Bundle crates never link into the gateway.
#[async_trait::async_trait]
pub trait DynamicWorkbenchSurface: Send + Sync + 'static {
    fn visible_views(&self, actor_scopes: &[crate::context::Scope])
        -> Vec<WorkbenchViewDescriptor>;

    fn visible_actions(
        &self,
        actor_scopes: &[crate::context::Scope],
    ) -> Vec<WorkbenchActionDescriptor>;

    fn find_action(
        &self,
        actor_scopes: &[crate::context::Scope],
        action_id: &str,
    ) -> Option<WorkbenchActionDescriptor>;

    /// Return one actor-filtered immutable Bundle capability aggregate.
    /// Existing surface implementors may rely on the empty default until they
    /// adopt the signed contribution contract.
    fn capability_projection(
        &self,
        _actor_scopes: &[crate::context::Scope],
    ) -> WorkbenchCapabilityProjectionResponse {
        WorkbenchCapabilityProjectionResponse::default()
    }

    async fn load_view_data(
        &self,
        context: crate::agent::tools::GadgetDispatchContext,
        actor_scopes: &[crate::context::Scope],
        view_id: &str,
    ) -> Result<WorkbenchViewData, crate::agent::tools::GadgetError>;

    /// Load data for a signed contribution whose Gadget reference is fixed by
    /// the enabled package. The caller supplies no Gadget name or arguments.
    async fn load_contribution_data(
        &self,
        _context: crate::agent::tools::GadgetDispatchContext,
        _actor_scopes: &[crate::context::Scope],
        contribution_id: &str,
    ) -> Result<WorkbenchContributionData, crate::agent::tools::GadgetError> {
        Err(crate::agent::tools::GadgetError::UnknownGadget(
            contribution_id.to_string(),
        ))
    }
}
