//! Billing insert failure counter (ISSUE 26).
//!
//! Background: billing writes are fire-and-forget from `record_post`
//! (chat), `handlers.rs` tool spawn, and `action_service::emit_action_billing`.
//! Each site catches errors with `tracing::warn!(target: "billing")`
//! and continues — the request already succeeded and propagating the
//! error would be wrong. But log-only observability is fragile:
//! logs rotate, log backends go down, an attacker can time actions
//! around a DB outage, and repudiation defense weakens silently.
//!
//! This module adds an in-memory `AtomicU64` counter per event kind
//! so the admin endpoint at `GET /api/v1/web/workbench/admin/billing/insert-failures`
//! (Management-scoped) can surface a drift signal. Operators alert on
//! any non-zero value; sustained growth indicates the chat / tool /
//! action ledger is diverging from `quota_configs` usage counters.
//!
//! The counter is process-local; restarts reset it. That's intentional:
//! long-term reconciliation is TASK 12.4 scope (scan `quota_configs`
//! SUM vs `billing_events` SUM and emit per-tenant drift alerts).
//! The counter here is a cheap instrument pointing at the same
//! problem in real time.

use std::sync::atomic::{AtomicU64, Ordering};

use super::events::BillingEventKind;

/// Process-local counter tracking `insert_billing_event` failures
/// per `BillingEventKind`. Operators poll via the admin HTTP endpoint
/// and alert on any non-zero delta over their SLO window.
#[derive(Debug, Default)]
pub struct BillingFailureCounter {
    chat: AtomicU64,
    tool: AtomicU64,
    action: AtomicU64,
}

/// Immutable snapshot returned by the admin endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct BillingFailureSnapshot {
    pub chat: u64,
    pub tool: u64,
    pub action: u64,
}

impl BillingFailureCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one failed `insert_billing_event` call. `Relaxed` is
    /// sufficient — the counter has no cross-field invariants that
    /// require stronger ordering, and readers are per-field too.
    pub fn increment(&self, kind: BillingEventKind) {
        match kind {
            BillingEventKind::Chat => self.chat.fetch_add(1, Ordering::Relaxed),
            BillingEventKind::Tool => self.tool.fetch_add(1, Ordering::Relaxed),
            BillingEventKind::Action => self.action.fetch_add(1, Ordering::Relaxed),
        };
    }

    /// Snapshot the current counts. Each field is loaded independently
    /// — no happens-before relationship between them — so concurrent
    /// writes may land a snapshot where chat and tool counts reflect
    /// different moments. That's acceptable: operators are looking
    /// at rough deltas over SLO windows, not coherent instants.
    pub fn snapshot(&self) -> BillingFailureSnapshot {
        BillingFailureSnapshot {
            chat: self.chat.load(Ordering::Relaxed),
            tool: self.tool.load(Ordering::Relaxed),
            action: self.action.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_starts_at_zero_and_increments_per_kind() {
        let c = BillingFailureCounter::new();
        assert_eq!(
            c.snapshot(),
            BillingFailureSnapshot {
                chat: 0,
                tool: 0,
                action: 0,
            }
        );
        c.increment(BillingEventKind::Chat);
        c.increment(BillingEventKind::Tool);
        c.increment(BillingEventKind::Tool);
        c.increment(BillingEventKind::Action);
        c.increment(BillingEventKind::Action);
        c.increment(BillingEventKind::Action);
        assert_eq!(
            c.snapshot(),
            BillingFailureSnapshot {
                chat: 1,
                tool: 2,
                action: 3,
            }
        );
    }

    #[test]
    fn counter_is_send_sync_and_shareable_via_arc() {
        // Smoke test: the counter lives inside `AppState` as
        // `Arc<BillingFailureCounter>` and is shared across the chat
        // enforcer + tool handler + action service tokio::spawn
        // closures. `&'static` usage + `Send + Sync` are required.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BillingFailureCounter>();
        let c = std::sync::Arc::new(BillingFailureCounter::new());
        let c2 = std::sync::Arc::clone(&c);
        c2.increment(BillingEventKind::Chat);
        assert_eq!(c.snapshot().chat, 1);
    }
}
