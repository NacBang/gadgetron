//! Tool audit event types.
//!
//! The `ToolAuditEvent` enum is the wire-format for per-tool-call audit
//! records emitted from `gadgetron-kairos::stream::event_to_chat_chunks`
//! and consumed by a concrete persistence backend in `gadgetron-xaas`.
//! The `ToolAuditEventSink` trait abstracts "where the event goes" so
//! `gadgetron-kairos` does not have to depend on the persistence layer.
//!
//! Design notes:
//! - `conversation_id` and `claude_session_uuid` are declared `Option<String>`
//!   from day one so A5-A7 (native session integration) can populate them
//!   without another schema migration. Turns without a conversation_id
//!   (stateless fallback) serialize both fields as `None` / SQL `NULL`.
//! - Dispatching is fire-and-forget via `send(&self, ...)`. Blocking /
//!   async-aware writers can wrap the call in a spawn internally. Callers
//!   MUST NOT `.await` or otherwise block the chat stream on an audit
//!   write — audit loss is preferable to stream stall.

use std::sync::Arc;

/// A single tool-call audit record.
///
/// Currently only one variant (`ToolCallCompleted`). Approval-flow events
/// — `ToolApprovalRequested`, `ToolApprovalGranted`, `ToolApprovalDenied`,
/// `ToolApprovalTimeout`, `ToolApprovalCancelled` — are deferred to P2B
/// per ADR-P2A-06.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolAuditEvent {
    /// One tool invocation finished (success OR error). Emitted from
    /// the Kairos stream on a `ToolUse` / `ToolResult` boundary.
    ToolCallCompleted {
        /// The tool name as seen by the `McpToolRegistry` — e.g.
        /// `"wiki.write"`, `"web.search"`.
        tool_name: String,
        /// The tool's tier at schema definition time. Denormalized
        /// into the audit record so downstream queries can filter by
        /// tier without joining against a live registry snapshot.
        tier: ToolTier,
        /// The `McpToolProvider::category()` the tool came from — e.g.
        /// `"knowledge"`, `"infrastructure"`. Informational.
        category: String,
        /// Success vs Error outcome.
        outcome: ToolCallOutcome,
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
    },
}

/// Tool-call outcome — success or error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallOutcome {
    Success,
    /// The tool returned `is_error: true` or the provider raised an
    /// `McpError`. The `error_code` is the short-form
    /// `McpError::error_code()` value (e.g. `"mcp_denied"`).
    Error {
        error_code: &'static str,
    },
}

/// Tier copy used in audit records. Kept separate from
/// `gadgetron_core::agent::tools::Tier` so the audit crate doesn't pick
/// up the full agent-tool module as a transitive dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTier {
    Read,
    Write,
    Destructive,
}

/// Denormalized metadata snapshot used by the Kairos audit emitter to
/// look up `(tier, category)` from a bare tool name on each stream-json
/// `ToolUse` event. Populated by `McpToolRegistryBuilder::freeze` from
/// every registered provider's `category()` + `tool_schemas()` output.
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    pub tier: ToolTier,
    pub category: String,
}

impl ToolTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
        }
    }
}

impl From<crate::agent::tools::Tier> for ToolTier {
    fn from(t: crate::agent::tools::Tier) -> Self {
        match t {
            crate::agent::tools::Tier::Read => Self::Read,
            crate::agent::tools::Tier::Write => Self::Write,
            crate::agent::tools::Tier::Destructive => Self::Destructive,
        }
    }
}

/// Sink for `ToolAuditEvent`. Implementors deliver events to their
/// persistence backend (PostgreSQL, tracing, test capture, etc.).
///
/// The trait is deliberately minimal and synchronous-looking. Async
/// writers wrap a bounded channel or `tokio::spawn` internally. Callers
/// MUST treat this as fire-and-forget — no `.await`, no `Result`, no
/// back-pressure.
pub trait ToolAuditEventSink: Send + Sync + std::fmt::Debug {
    fn send(&self, event: ToolAuditEvent);
}

/// Default sink that drops every event on the floor. Used when audit
/// persistence is not configured (e.g. running `gadgetron serve` without
/// a database) and as the test harness default.
#[derive(Debug, Clone, Default)]
pub struct NoopToolAuditEventSink;

impl ToolAuditEventSink for NoopToolAuditEventSink {
    fn send(&self, _event: ToolAuditEvent) {}
}

impl NoopToolAuditEventSink {
    pub fn new_arc() -> Arc<dyn ToolAuditEventSink> {
        Arc::new(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_tier_as_str_is_stable() {
        assert_eq!(ToolTier::Read.as_str(), "read");
        assert_eq!(ToolTier::Write.as_str(), "write");
        assert_eq!(ToolTier::Destructive.as_str(), "destructive");
    }

    #[test]
    fn noop_sink_drops_events() {
        let sink = NoopToolAuditEventSink;
        sink.send(ToolAuditEvent::ToolCallCompleted {
            tool_name: "wiki.write".into(),
            tier: ToolTier::Write,
            category: "knowledge".into(),
            outcome: ToolCallOutcome::Success,
            elapsed_ms: 42,
            conversation_id: None,
            claude_session_uuid: None,
        });
        // No panic, no side effect — success is defined by compile + run.
    }

    #[test]
    fn from_agent_tier_conversion_preserves_variant() {
        use crate::agent::tools::Tier;
        assert_eq!(ToolTier::from(Tier::Read), ToolTier::Read);
        assert_eq!(ToolTier::from(Tier::Write), ToolTier::Write);
        assert_eq!(ToolTier::from(Tier::Destructive), ToolTier::Destructive);
    }
}
