//! Gadget audit event types.
//!
//! The `GadgetAuditEvent` enum is the wire-format for per-Gadget-call audit
//! records emitted from `gadgetron-penny::stream::event_to_chat_chunks`
//! and consumed by a concrete persistence backend in `gadgetron-xaas`.
//! The `GadgetAuditEventSink` trait abstracts "where the event goes" so
//! `gadgetron-penny` does not have to depend on the persistence layer.
//!
//! Design notes:
//! - `conversation_id` and `claude_session_uuid` are declared `Option<String>`
//!   from day one so A5-A7 (native session integration) can populate them
//!   without another schema migration. Turns without a conversation_id
//!   (stateless fallback) serialize both fields as `None` / SQL `NULL`.
//! - `owner_id` and `tenant_id` (added in PR A7.5 Type 1 Decision #1 per the
//!   2026-04-16 office-hours design doc) are also `Option<String>` from day
//!   one. P2A is single-user desktop and always writes `None` for both, but
//!   the fields exist so multi-tenant rollout in P2B/P2C needs NO schema
//!   migration — only a flip from `None` to `Some(x)` at the emit site.
//!   Cost: 2 extra `None` literals per emit, one `NULL` column per row.
//!   Savings: ~1 week of migration + backfill + semantic re-alignment when
//!   the second principal arrives.
//! - Dispatching is fire-and-forget via `send(&self, ...)`. Blocking /
//!   async-aware writers can wrap the call in a spawn internally. Callers
//!   MUST NOT `.await` or otherwise block the chat stream on an audit
//!   write — audit loss is preferable to stream stall.

use std::sync::Arc;

/// A single Gadget-call audit record.
///
/// Currently only one variant (`GadgetCallCompleted`). Approval-flow events
/// — `GadgetApprovalRequested`, `GadgetApprovalGranted`, `GadgetApprovalDenied`,
/// `GadgetApprovalTimeout`, `GadgetApprovalCancelled` — are deferred to P2B
/// per ADR-P2A-06.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GadgetAuditEvent {
    /// One Gadget invocation finished (success OR error). Emitted from
    /// the Penny stream on a `ToolUse` / `ToolResult` boundary.
    GadgetCallCompleted {
        /// The Gadget name as seen by the `GadgetRegistry` — e.g.
        /// `"wiki.write"`, `"web.search"`.
        ///
        /// **DB column mapping (P2B writer):** this field is persisted to
        /// SQL column `tool_audit_events.tool_name TEXT NOT NULL` (migration
        /// `20260416000001_tool_audit_events.sql:11`). The P2B DB writer
        /// loop MUST map this Rust field to the `tool_name` column name in
        /// its INSERT statement; the column name is a wire-frozen contract
        /// per ADR-P2A-10 security review (D-20260418-05). Do NOT rename
        /// the SQL column to `gadget_name`; downstream SIEM / BI queries
        /// filter on `tool_name`.
        gadget_name: String,
        /// The Gadget's tier at schema definition time. Denormalized
        /// into the audit record so downstream queries can filter by
        /// tier without joining against a live registry snapshot.
        tier: GadgetTier,
        /// The `GadgetProvider::category()` the Gadget came from — e.g.
        /// `"knowledge"`, `"infrastructure"`. Informational.
        category: String,
        /// Success vs Error outcome.
        outcome: GadgetCallOutcome,
        /// Wall-clock milliseconds between the `ToolUse` event and
        /// its matching `ToolResult` event. For P2A this is a
        /// best-effort number — precise `id`-based correlation lands
        /// with Step 21 fake-claude infrastructure.
        elapsed_ms: u64,
        /// Gadgetron-side conversation identifier, populated when
        /// `ChatRequest.conversation_id.is_some()`. `None` in P2A PR A4
        /// pre-A5; A5-A7 wire the value through.
        conversation_id: Option<String>,
        /// Claude Code native session UUID backing the conversation,
        /// populated by A6 (native session integration). `None` for
        /// stateless-mode turns and for all P2A PR A4 emissions.
        claude_session_uuid: Option<String>,
        /// Owner identifier — the principal whose credentials drove the
        /// request. `None` in P2A single-user mode. Flip to `Some(x)`
        /// in P2B/P2C when multi-tenant arrives. Added via Type 1 Decision
        /// #1 from the 2026-04-16 office-hours design doc.
        owner_id: Option<String>,
        /// Tenant identifier — the logical tenancy boundary under which
        /// this call was made. `None` in P2A single-tenant mode. Flip to
        /// `Some(x)` when a second tenant arrives. Held separate from
        /// `owner_id` because a single tenant can have multiple owners
        /// (e.g., a team account with member users).
        tenant_id: Option<String>,
    },
}

/// Gadget-call outcome — success or error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GadgetCallOutcome {
    Success,
    /// The Gadget returned `is_error: true` or the provider raised a
    /// `GadgetError`. The `error_code` is the short-form
    /// `GadgetError::error_code()` value (e.g. `"gadget_denied_by_policy"`).
    Error {
        error_code: &'static str,
    },
}

/// Tier copy used in audit records. Kept separate from
/// `gadgetron_core::agent::tools::GadgetTier` so the audit crate doesn't pick
/// up the full agent-tool module as a transitive dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GadgetTier {
    Read,
    Write,
    Destructive,
}

/// Denormalized metadata snapshot used by the Penny audit emitter to
/// look up `(tier, category)` from a bare Gadget name on each stream-json
/// `ToolUse` event. Populated by `GadgetRegistryBuilder::freeze` from
/// every registered provider's `category()` + `gadget_schemas()` output.
#[derive(Debug, Clone)]
pub struct GadgetMetadata {
    pub tier: GadgetTier,
    pub category: String,
}

impl GadgetTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
        }
    }
}

impl From<crate::agent::tools::GadgetTier> for GadgetTier {
    fn from(t: crate::agent::tools::GadgetTier) -> Self {
        match t {
            crate::agent::tools::GadgetTier::Read => Self::Read,
            crate::agent::tools::GadgetTier::Write => Self::Write,
            crate::agent::tools::GadgetTier::Destructive => Self::Destructive,
        }
    }
}

/// Sink for `GadgetAuditEvent`. Implementors deliver events to their
/// persistence backend (PostgreSQL, tracing, test capture, etc.).
///
/// The trait is deliberately minimal and synchronous-looking. Async
/// writers wrap a bounded channel or `tokio::spawn` internally. Callers
/// MUST treat this as fire-and-forget — no `.await`, no `Result`, no
/// back-pressure.
pub trait GadgetAuditEventSink: Send + Sync + std::fmt::Debug {
    fn send(&self, event: GadgetAuditEvent);
}

/// Default sink that drops every event on the floor. Used when audit
/// persistence is not configured (e.g. running `gadgetron serve` without
/// a database) and as the test harness default.
#[derive(Debug, Clone, Default)]
pub struct NoopGadgetAuditEventSink;

impl GadgetAuditEventSink for NoopGadgetAuditEventSink {
    fn send(&self, _event: GadgetAuditEvent) {}
}

impl NoopGadgetAuditEventSink {
    pub fn new_arc() -> Arc<dyn GadgetAuditEventSink> {
        Arc::new(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gadget_tier_as_str_is_stable() {
        assert_eq!(GadgetTier::Read.as_str(), "read");
        assert_eq!(GadgetTier::Write.as_str(), "write");
        assert_eq!(GadgetTier::Destructive.as_str(), "destructive");
    }

    #[test]
    fn noop_sink_drops_events() {
        let sink = NoopGadgetAuditEventSink;
        sink.send(GadgetAuditEvent::GadgetCallCompleted {
            gadget_name: "wiki.write".into(),
            tier: GadgetTier::Write,
            category: "knowledge".into(),
            outcome: GadgetCallOutcome::Success,
            elapsed_ms: 42,
            conversation_id: None,
            claude_session_uuid: None,
            owner_id: None,
            tenant_id: None,
        });
        // No panic, no side effect — success is defined by compile + run.
    }

    #[test]
    fn gadget_call_completed_has_multi_tenant_fields() {
        // Type 1 Decision #1 regression lock — `owner_id` and `tenant_id`
        // must exist as `Option<String>` fields so multi-tenant rollout
        // needs no schema migration. This test fails if someone ever
        // removes the fields or changes them to non-optional.
        let evt = GadgetAuditEvent::GadgetCallCompleted {
            gadget_name: "wiki.read".into(),
            tier: GadgetTier::Read,
            category: "knowledge".into(),
            outcome: GadgetCallOutcome::Success,
            elapsed_ms: 0,
            conversation_id: Some("c1".into()),
            claude_session_uuid: Some("11111111-2222-3333-4444-555555555555".into()),
            owner_id: Some("_self".into()),
            tenant_id: Some("manycoresoft".into()),
        };
        match evt {
            GadgetAuditEvent::GadgetCallCompleted {
                owner_id,
                tenant_id,
                ..
            } => {
                assert_eq!(owner_id.as_deref(), Some("_self"));
                assert_eq!(tenant_id.as_deref(), Some("manycoresoft"));
            }
        }
    }

    #[test]
    fn from_agent_tier_conversion_preserves_variant() {
        use crate::agent::tools::GadgetTier as AgentTier;
        assert_eq!(GadgetTier::from(AgentTier::Read), GadgetTier::Read);
        assert_eq!(GadgetTier::from(AgentTier::Write), GadgetTier::Write);
        assert_eq!(GadgetTier::from(AgentTier::Destructive), GadgetTier::Destructive);
    }
}
