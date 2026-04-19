//! In-process activity event bus — live feed for `/web/dashboard`.
//!
//! ISSUE 4 TASK 4.3 wires operator-facing events onto a
//! `tokio::sync::broadcast` so WebSocket clients can subscribe and
//! see activity in ≤ 1 tick. Event producers (chat handler, action
//! service, approval endpoints) publish fire-and-forget; subscribers
//! filter by `tenant_id` client-side.
//!
//! Channel capacity is bounded (`DEFAULT_CAPACITY`) so a slow
//! subscriber can't memory-stall the producer. Lagged subscribers
//! get `RecvError::Lagged(n)` and can catch up by GET-ing
//! `/usage/summary` for the current rollup.
//!
//! Scope boundary: this bus does NOT replace the audit log. Audit
//! rows persist to Postgres; this bus is an ephemeral live tap.
//! If the server restarts, in-flight events are lost — that's
//! acceptable; subscribers reconnect and see new events from the
//! reconnect moment onward.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Default broadcast capacity. 1024 entries × ~512 bytes per
/// serialized `ActivityEvent` ≈ 512 KiB upper bound per bus, which
/// is cheap relative to the benefit (no lagged subscribers during
/// normal load).
pub const DEFAULT_CAPACITY: usize = 1024;

/// Event envelope. Subscribers filter by `.tenant_id()` to avoid
/// leaking one tenant's activity to another's WebSocket.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityEvent {
    /// One chat completion finished (ok or error). Emitted from the
    /// non-streaming handler and from `StreamEndGuard` on drop.
    ChatCompleted {
        tenant_id: Uuid,
        request_id: Uuid,
        model: String,
        /// `"ok"` / `"error"` / `"stream_interrupted"` — matches
        /// `AuditStatus::as_str()`.
        status: String,
        input_tokens: i64,
        output_tokens: i64,
        cost_cents: i64,
        latency_ms: i64,
    },
    /// One workbench direct-action invocation completed. Mirrors
    /// `ActionAuditEvent::DirectActionCompleted` but keeps the event
    /// family deliberately separate — audit persists, bus doesn't.
    ActionCompleted {
        tenant_id: Uuid,
        audit_event_id: Uuid,
        action_id: String,
        gadget_name: Option<String>,
        /// `"success"` / `"error"` / `"pending_approval"`.
        outcome: String,
        error_code: Option<String>,
        elapsed_ms: i64,
    },
    /// An approval was resolved (approve or deny). Emitted from the
    /// approval handler after `ApprovalStore::mark_*`.
    ApprovalResolved {
        tenant_id: Uuid,
        approval_id: Uuid,
        action_id: String,
        /// `"approved"` / `"denied"`.
        state: String,
        resolved_by_user_id: Uuid,
    },
    /// One Penny tool call finished (success or error). Emitted from
    /// the GadgetAuditEventWriter when an `ActivityBus` handle is
    /// wired (ISSUE 5 TASK 5.3) — mirrors the `ActionCompleted`
    /// shape so the dashboard can treat both audit planes uniformly
    /// in its live feed.
    ///
    /// `tenant_id` is `Uuid::nil()` when the upstream
    /// `GadgetAuditEvent` reports a missing / None tenant_id
    /// string — the dashboard filters those out client-side.
    ToolCallCompleted {
        tenant_id: Uuid,
        tool_name: String,
        category: String,
        /// `"read"` / `"write"` / `"destructive"`.
        tier: String,
        /// `"success"` / `"error"`.
        outcome: String,
        error_code: Option<String>,
        elapsed_ms: i64,
        conversation_id: Option<String>,
    },
}

impl ActivityEvent {
    /// Tenant filter key — subscribers drop events whose
    /// `tenant_id` differs from their own.
    pub fn tenant_id(&self) -> Uuid {
        match self {
            Self::ChatCompleted { tenant_id, .. } => *tenant_id,
            Self::ActionCompleted { tenant_id, .. } => *tenant_id,
            Self::ApprovalResolved { tenant_id, .. } => *tenant_id,
            Self::ToolCallCompleted { tenant_id, .. } => *tenant_id,
        }
    }
}

/// Handle holding the shared `broadcast::Sender`. Clone-cheap
/// (wraps `Arc` internally). Publishers call `send`; the WebSocket
/// handler calls `subscribe`.
#[derive(Clone)]
pub struct ActivityBus {
    tx: broadcast::Sender<ActivityEvent>,
}

impl ActivityBus {
    /// New bus with `DEFAULT_CAPACITY`. Spawned receivers share
    /// the channel via `subscribe`.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        // `broadcast::channel` returns (tx, rx); we drop the rx
        // because fresh subscribers use `tx.subscribe()`.
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe for a fresh receiver. Each subscriber starts
    /// reading at the CURRENT tail of the channel — no backfill.
    pub fn subscribe(&self) -> broadcast::Receiver<ActivityEvent> {
        self.tx.subscribe()
    }

    /// Fire-and-forget publish. `send` errors only when there are
    /// zero subscribers; we swallow that case since the bus is
    /// opportunistic — no operator online, no emission needed.
    pub fn publish(&self, event: ActivityEvent) {
        let _ = self.tx.send(event);
    }

    /// Current live-subscriber count (test helper + ops dashboard).
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for ActivityBus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ActivityBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityBus")
            .field("subscriber_count", &self.subscriber_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(tenant: Uuid) -> ActivityEvent {
        ActivityEvent::ChatCompleted {
            tenant_id: tenant,
            request_id: Uuid::new_v4(),
            model: "gpt-4o".into(),
            status: "ok".into(),
            input_tokens: 100,
            output_tokens: 200,
            cost_cents: 5,
            latency_ms: 320,
        }
    }

    #[tokio::test]
    async fn subscribe_then_publish_roundtrips() {
        let bus = ActivityBus::new();
        let mut rx = bus.subscribe();
        let t = Uuid::new_v4();
        bus.publish(chat(t));
        let got = rx.recv().await.expect("event");
        assert_eq!(got.tenant_id(), t);
    }

    #[tokio::test]
    async fn publish_before_subscribe_drops() {
        let bus = ActivityBus::new();
        // No subscribers yet — publish succeeds but no delivery.
        bus.publish(chat(Uuid::new_v4()));
        // Subscribe AFTER → fresh receiver starts at tail.
        let mut rx = bus.subscribe();
        // Second publish: this one we receive.
        let t = Uuid::new_v4();
        bus.publish(chat(t));
        let got = rx.recv().await.unwrap();
        assert_eq!(got.tenant_id(), t);
    }

    #[tokio::test]
    async fn subscriber_count_tracks_subscribes() {
        let bus = ActivityBus::new();
        assert_eq!(bus.subscriber_count(), 0);
        let _r1 = bus.subscribe();
        let _r2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
    }

    #[tokio::test]
    async fn event_serializes_with_type_tag() {
        let t = Uuid::parse_str("00000000-0000-0000-0000-000000000042").unwrap();
        let e = chat(t);
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""type":"chat_completed""#));
        assert!(s.contains(r#""status":"ok""#));
    }

    #[tokio::test]
    async fn tenant_id_dispatches_across_variants() {
        let t = Uuid::new_v4();
        let a = ActivityEvent::ActionCompleted {
            tenant_id: t,
            audit_event_id: Uuid::new_v4(),
            action_id: "wiki-write".into(),
            gadget_name: Some("wiki.write".into()),
            outcome: "success".into(),
            error_code: None,
            elapsed_ms: 12,
        };
        let p = ActivityEvent::ApprovalResolved {
            tenant_id: t,
            approval_id: Uuid::new_v4(),
            action_id: "wiki-delete".into(),
            state: "approved".into(),
            resolved_by_user_id: Uuid::new_v4(),
        };
        assert_eq!(a.tenant_id(), t);
        assert_eq!(p.tenant_id(), t);
    }
}
