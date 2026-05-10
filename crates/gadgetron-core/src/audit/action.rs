//! Direct-action audit event types (workbench dispatch path).
//!
//! Penny-originated tool calls already flow through
//! [`GadgetAuditEventSink`](super::GadgetAuditEventSink) carrying
//! `GadgetCallCompleted`. Workbench direct actions are a *session-less*
//! cousin — they dispatch a Gadget via [`GadgetDispatcher`] without a
//! conversation / Claude session attached. Two concerns motivate a
//! separate event type rather than reusing `GadgetAuditEvent`:
//!
//! 1. **Identity**: direct actions need a `ActionAuditEvent.event_id` so
//!    the HTTP response (`WorkbenchActionResult.audit_event_id`) can echo
//!    back a stable handle the caller can look up. Penny's event stream
//!    doesn't have that requirement today.
//! 2. **Schema isolation**: direct actions carry `action_id` (the
//!    workbench-catalog entry that was invoked) alongside the concrete
//!    Gadget name. Penny events don't have an `action_id` concept. Keeping
//!    them in a separate table / sink stream avoids a forced NULLABLE in
//!    the Penny audit schema.
//!
//! Sink semantics match `GadgetAuditEventSink` — fire-and-forget; callers
//! MUST NOT `.await`. Async writers wrap a bounded channel or
//! `tokio::spawn` internally.
//!
//! [`GadgetDispatcher`]: crate::agent::tools::GadgetDispatcher

use std::sync::Arc;
use uuid::Uuid;

/// A single direct-action audit record.
///
/// Emitted from [`InProcessWorkbenchActionService::invoke`] step 7a after
/// a Gadget dispatch completes (success OR error). The `event_id` is the
/// same UUID that lands in `WorkbenchActionResult.audit_event_id` so a
/// caller can correlate their response with the persisted row.
///
/// `#[non_exhaustive]` reserves room for `ActionApprovalRequested` /
/// `ActionApprovalGranted` / etc. in the approval-flow ISSUE.
///
/// [`InProcessWorkbenchActionService::invoke`]: (gadgetron-gateway crate)
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionAuditEvent {
    /// One direct-action invocation finished. Fields mirror the
    /// `tool_audit_events` shape where overlap exists so downstream SQL
    /// can UNION the two streams when querying a unified "what did the
    /// system do" feed.
    DirectActionCompleted {
        /// Stable id for this event. Echoed back to the HTTP response as
        /// `WorkbenchActionResult.audit_event_id` and used as the SQL PK.
        event_id: Uuid,
        /// The workbench catalog entry id — e.g. `"wiki-write"`.
        action_id: String,
        /// The Gadget the action dispatched to, or `None` when the
        /// action has `gadget_name: None` (no real dispatch, synthetic
        /// result path).
        gadget_name: Option<String>,
        /// Acting principal + tenant.
        actor_user_id: Uuid,
        tenant_id: Uuid,
        /// Ok / Err outcome of the dispatched Gadget.
        outcome: ActionAuditOutcome,
        /// Wall-clock ms between the workbench handler entering step 7a
        /// and the sink emit.
        elapsed_ms: u64,
    },
}

/// Direct-action outcome — success or error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionAuditOutcome {
    /// Dispatch returned `Ok` and the action response was built normally.
    Success,
    /// Dispatch returned `Err` (and the action handler surfaced it as an
    /// HTTP error). `error_code` is `GadgetError::error_code()`.
    Error { error_code: String },
    /// Action short-circuited at step 6 (approval required). Dispatch
    /// didn't run; the event just records that the caller was handed a
    /// `pending_approval` response. Approval-flow ISSUE will refine this.
    PendingApproval,
}

/// Sink for `ActionAuditEvent`. Implementors deliver events to their
/// persistence backend. Fire-and-forget — see module docs.
pub trait ActionAuditSink: Send + Sync + std::fmt::Debug {
    fn send(&self, event: ActionAuditEvent);
}

/// Default sink that drops every event. Used when the server runs
/// without Postgres audit persistence (`--no-db`) and as the test
/// harness default.
#[derive(Debug, Clone, Default)]
pub struct NoopActionAuditSink;

impl ActionAuditSink for NoopActionAuditSink {
    fn send(&self, _event: ActionAuditEvent) {}
}

impl NoopActionAuditSink {
    pub fn new_arc() -> Arc<dyn ActionAuditSink> {
        Arc::new(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sink_accepts_every_variant() {
        let sink = NoopActionAuditSink;
        sink.send(ActionAuditEvent::DirectActionCompleted {
            event_id: Uuid::new_v4(),
            action_id: "wiki-write".into(),
            gadget_name: Some("wiki.write".into()),
            actor_user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            outcome: ActionAuditOutcome::Success,
            elapsed_ms: 5,
        });
        sink.send(ActionAuditEvent::DirectActionCompleted {
            event_id: Uuid::new_v4(),
            action_id: "knowledge-search".into(),
            gadget_name: Some("wiki.search".into()),
            actor_user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            outcome: ActionAuditOutcome::Error {
                error_code: "gadget_execution".into(),
            },
            elapsed_ms: 2,
        });
        sink.send(ActionAuditEvent::DirectActionCompleted {
            event_id: Uuid::new_v4(),
            action_id: "admin-destroy".into(),
            gadget_name: None,
            actor_user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            outcome: ActionAuditOutcome::PendingApproval,
            elapsed_ms: 1,
        });
    }

    #[test]
    fn new_arc_constructs_dyn_sink() {
        let s: Arc<dyn ActionAuditSink> = NoopActionAuditSink::new_arc();
        s.send(ActionAuditEvent::DirectActionCompleted {
            event_id: Uuid::nil(),
            action_id: "x".into(),
            gadget_name: None,
            actor_user_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            outcome: ActionAuditOutcome::Success,
            elapsed_ms: 0,
        });
    }
}
