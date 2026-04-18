//! Concrete `GadgetAuditEventSink` backed by a bounded channel, mirroring
//! the existing `AuditWriter` pattern but for the Gadget-level stream.
//!
//! Spec: ADR-P2A-06 Implementation status addendum item 1 and
//! `04-gadget-registry.md §10`.
//!
//! The receiver side (DB writer loop) lives outside this module — this
//! writer's job is only to accept events fire-and-forget and count
//! drops. The eventual flush to PostgreSQL goes through the migration
//! `20260416000001_tool_audit_events.sql`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gadgetron_core::audit::{GadgetAuditEvent, GadgetAuditEventSink};
use tokio::sync::mpsc;

/// Fire-and-forget `GadgetAuditEventSink` that pushes events onto a
/// bounded `mpsc::Sender`. Drops on channel-full and counts drops
/// atomically so operators can see if the DB writer is falling behind.
#[derive(Debug)]
pub struct GadgetAuditEventWriter {
    tx: mpsc::Sender<GadgetAuditEvent>,
    dropped: Arc<AtomicU64>,
}

impl GadgetAuditEventWriter {
    pub fn new(channel_capacity: usize) -> (Self, mpsc::Receiver<GadgetAuditEvent>) {
        let (tx, rx) = mpsc::channel(channel_capacity);
        (
            Self {
                tx,
                dropped: Arc::new(AtomicU64::new(0)),
            },
            rx,
        )
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl GadgetAuditEventSink for GadgetAuditEventWriter {
    fn send(&self, event: GadgetAuditEvent) {
        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                target: "penny_audit",
                "tool audit event dropped — channel full"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::audit::{GadgetCallOutcome, GadgetTier};

    fn make_event() -> GadgetAuditEvent {
        GadgetAuditEvent::GadgetCallCompleted {
            gadget_name: "wiki.write".to_string(),
            tier: GadgetTier::Write,
            category: "knowledge".to_string(),
            outcome: GadgetCallOutcome::Success,
            elapsed_ms: 42,
            conversation_id: None,
            claude_session_uuid: None,
            owner_id: None,
            tenant_id: None,
        }
    }

    #[tokio::test]
    async fn send_delivers_event_to_receiver() {
        let (writer, mut rx) = GadgetAuditEventWriter::new(16);
        writer.send(make_event());
        let received = rx.recv().await.expect("event");
        match received {
            GadgetAuditEvent::GadgetCallCompleted { gadget_name, .. } => {
                assert_eq!(gadget_name, "wiki.write");
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected GadgetAuditEvent variant"),
        }
    }

    #[tokio::test]
    async fn drops_when_channel_full() {
        let (writer, _rx) = GadgetAuditEventWriter::new(2);
        writer.send(make_event());
        writer.send(make_event());
        writer.send(make_event()); // dropped
        assert!(writer.dropped_count() >= 1);
    }

    #[tokio::test]
    async fn session_fields_default_to_none_in_p2a_pr_a4() {
        // Regression lock: PR A4 ships the schema with the two session
        // fields already declared, but the emitter always passes None
        // until A5-A7 wire native session plumbing through.
        let (writer, mut rx) = GadgetAuditEventWriter::new(16);
        writer.send(make_event());
        let received = rx.recv().await.unwrap();
        match received {
            GadgetAuditEvent::GadgetCallCompleted {
                conversation_id,
                claude_session_uuid,
                ..
            } => {
                assert!(conversation_id.is_none());
                assert!(claude_session_uuid.is_none());
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected GadgetAuditEvent variant"),
        }
    }
}
