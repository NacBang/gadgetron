//! Postgres-backed `ActionAuditSink` for workbench direct-action dispatches.
//!
//! Two halves:
//!
//! - [`ActionAuditEventWriter`] — an `ActionAuditSink` impl backed by a
//!   bounded `mpsc::Sender`. Cheap on the hot path (push+drop-on-full),
//!   fire-and-forget — the action service never awaits.
//! - [`run_action_audit_writer`] — async consumer task that pulls events
//!   off the receiver and INSERTs into `action_audit_events`. Spawns on
//!   a tokio task at serve startup.
//!
//! Schema: `20260419000001_action_audit_events.sql` (see `migrations/`).
//!
//! When `pg_pool` is unavailable (no-db mode, unit tests) `main.rs`
//! installs `NoopActionAuditSink` instead — the writer here is only
//! used when a pool IS configured.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gadgetron_core::audit::{ActionAuditEvent, ActionAuditOutcome, ActionAuditSink};
use sqlx::PgPool;
use tokio::sync::mpsc;

/// Fire-and-forget `ActionAuditSink` that pushes events onto a bounded
/// channel. Drops on full and counts drops atomically so operators
/// can see if the DB writer is falling behind.
#[derive(Debug)]
pub struct ActionAuditEventWriter {
    tx: mpsc::Sender<ActionAuditEvent>,
    dropped: Arc<AtomicU64>,
}

impl ActionAuditEventWriter {
    pub fn new(channel_capacity: usize) -> (Self, mpsc::Receiver<ActionAuditEvent>) {
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

impl ActionAuditSink for ActionAuditEventWriter {
    fn send(&self, event: ActionAuditEvent) {
        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                target: "action_audit",
                "action audit event dropped — channel full"
            );
        }
    }
}

/// Async consumer task — pulls `ActionAuditEvent` off the receiver and
/// INSERTs one row per event into `action_audit_events`. Exits cleanly
/// when the channel closes (all writers dropped).
///
/// Errors are logged but never propagated — audit loss is preferable
/// to a crash loop in the writer that would silently drop every
/// subsequent event too.
pub async fn run_action_audit_writer(
    mut rx: mpsc::Receiver<ActionAuditEvent>,
    pool: PgPool,
) {
    while let Some(event) = rx.recv().await {
        if let Err(e) = insert_event(&pool, &event).await {
            tracing::warn!(
                target: "action_audit",
                error = %e,
                ?event,
                "failed to persist action audit event (continuing)",
            );
        }
    }
    tracing::info!(target: "action_audit", "action audit writer exiting — channel closed");
}

async fn insert_event(pool: &PgPool, event: &ActionAuditEvent) -> Result<(), sqlx::Error> {
    match event {
        ActionAuditEvent::DirectActionCompleted {
            event_id,
            action_id,
            gadget_name,
            actor_user_id,
            tenant_id,
            outcome,
            elapsed_ms,
        } => {
            let (outcome_text, error_code) = match outcome {
                ActionAuditOutcome::Success => ("success", None),
                ActionAuditOutcome::Error { error_code } => ("error", Some(error_code.as_str())),
                ActionAuditOutcome::PendingApproval => ("pending_approval", None),
            };
            sqlx::query(
                r#"
                INSERT INTO action_audit_events
                    (event_id, action_id, gadget_name, actor_user_id, tenant_id,
                     outcome, error_code, elapsed_ms)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(event_id)
            .bind(action_id)
            .bind(gadget_name.as_deref())
            .bind(actor_user_id.to_string())
            .bind(tenant_id.to_string())
            .bind(outcome_text)
            .bind(error_code)
            .bind(*elapsed_ms as i64)
            .execute(pool)
            .await?;
            Ok(())
        }
        // ActionAuditEvent is #[non_exhaustive]; future approval-flow
        // variants (ApprovalGranted etc.) will land with their own
        // insert arms.
        #[allow(unreachable_patterns)]
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_event(outcome: ActionAuditOutcome) -> ActionAuditEvent {
        ActionAuditEvent::DirectActionCompleted {
            event_id: Uuid::new_v4(),
            action_id: "wiki-write".into(),
            gadget_name: Some("wiki.write".into()),
            actor_user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            outcome,
            elapsed_ms: 42,
        }
    }

    #[tokio::test]
    async fn send_delivers_event_to_receiver() {
        let (writer, mut rx) = ActionAuditEventWriter::new(16);
        writer.send(make_event(ActionAuditOutcome::Success));
        let received = rx.recv().await.expect("event");
        match received {
            ActionAuditEvent::DirectActionCompleted { action_id, .. } => {
                assert_eq!(action_id, "wiki-write");
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected ActionAuditEvent variant"),
        }
    }

    #[tokio::test]
    async fn drops_when_channel_full() {
        let (writer, _rx) = ActionAuditEventWriter::new(2);
        writer.send(make_event(ActionAuditOutcome::Success));
        writer.send(make_event(ActionAuditOutcome::Success));
        writer.send(make_event(ActionAuditOutcome::Success)); // dropped
        assert!(writer.dropped_count() >= 1);
    }

    #[tokio::test]
    async fn outcome_variants_round_trip_through_channel() {
        let (writer, mut rx) = ActionAuditEventWriter::new(16);
        writer.send(make_event(ActionAuditOutcome::Success));
        writer.send(make_event(ActionAuditOutcome::Error {
            error_code: "gadget_execution".into(),
        }));
        writer.send(make_event(ActionAuditOutcome::PendingApproval));
        for expected in ["Success", "Error", "PendingApproval"] {
            let evt = rx.recv().await.unwrap();
            let observed = match evt {
                ActionAuditEvent::DirectActionCompleted { outcome, .. } => match outcome {
                    ActionAuditOutcome::Success => "Success",
                    ActionAuditOutcome::Error { .. } => "Error",
                    ActionAuditOutcome::PendingApproval => "PendingApproval",
                },
                #[allow(unreachable_patterns)]
                _ => unreachable!("only DirectActionCompleted shipping today"),
            };
            assert_eq!(observed, expected);
        }
    }
}
