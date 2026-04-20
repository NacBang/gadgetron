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

use gadgetron_core::activity_bus::{ActivityBus, ActivityEvent};
use gadgetron_core::audit::{GadgetAuditEvent, GadgetAuditEventSink, GadgetCallOutcome};
use gadgetron_core::knowledge::candidate::{
    ActivityKind, ActivityOrigin, CapturedActivityEvent, KnowledgeCandidateCoordinator,
};
use gadgetron_core::knowledge::AuthenticatedContext;
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Fire-and-forget `GadgetAuditEventSink` that pushes events onto a
/// bounded `mpsc::Sender`. Drops on channel-full and counts drops
/// atomically so operators can see if the DB writer is falling behind.
///
/// When an `ActivityBus` handle is wired via `with_activity_bus`,
/// each `send` ALSO publishes a corresponding
/// `ActivityEvent::ToolCallCompleted` onto the bus so the /events/ws
/// live feed + /web/dashboard see Penny tool calls in real time
/// (ISSUE 5 TASK 5.3).
#[derive(Debug)]
pub struct GadgetAuditEventWriter {
    tx: mpsc::Sender<GadgetAuditEvent>,
    dropped: Arc<AtomicU64>,
    activity_bus: Option<ActivityBus>,
    coordinator: Option<Arc<dyn KnowledgeCandidateCoordinator>>,
}

impl GadgetAuditEventWriter {
    pub fn new(channel_capacity: usize) -> (Self, mpsc::Receiver<GadgetAuditEvent>) {
        let (tx, rx) = mpsc::channel(channel_capacity);
        (
            Self {
                tx,
                dropped: Arc::new(AtomicU64::new(0)),
                activity_bus: None,
                coordinator: None,
            },
            rx,
        )
    }

    /// Attach an `ActivityBus` for live-feed mirroring. Returns a new
    /// writer — the original consumer channel receiver is unchanged.
    pub fn with_activity_bus(mut self, bus: ActivityBus) -> Self {
        self.activity_bus = Some(bus);
        self
    }

    /// Attach a `KnowledgeCandidateCoordinator` so every Penny tool
    /// call ALSO captures a `CapturedActivityEvent` with
    /// `ActivityOrigin::Penny` — surfaces Penny-originated tool calls
    /// in `/workbench/activity` alongside direct workbench actions
    /// (ISSUE 6 TASK 6.1).
    pub fn with_coordinator(mut self, coord: Arc<dyn KnowledgeCandidateCoordinator>) -> Self {
        self.coordinator = Some(coord);
        self
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl GadgetAuditEventSink for GadgetAuditEventWriter {
    fn send(&self, event: GadgetAuditEvent) {
        // Mirror to the activity bus BEFORE moving the event into the
        // mpsc channel. Subscribers filter by tenant client-side; we
        // never leak to another tenant because the event carries
        // `tenant_id` as-is.
        if let Some(bus) = &self.activity_bus {
            if let Some(activity) = gadget_audit_to_activity(&event) {
                bus.publish(activity);
            }
        }
        // ISSUE 6 TASK 6.1: fan out to the capture coordinator so the
        // /workbench/activity feed shows Penny tool calls with the
        // correct `ActivityOrigin::Penny` attribution.
        if let Some(coord) = self.coordinator.clone() {
            if let Some(captured) = gadget_audit_to_captured(&event) {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let actor = captured_actor(&captured);
                    handle.spawn(async move {
                        if let Err(e) = coord.capture_action(&actor, captured, vec![]).await {
                            tracing::warn!(
                                target: "penny_audit",
                                error = %e,
                                "capture_action failed for Penny tool call (non-fatal)"
                            );
                        }
                    });
                }
            }
        }
        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                target: "penny_audit",
                "tool audit event dropped — channel full"
            );
        }
    }
}

/// Convert a `GadgetAuditEvent` into a `CapturedActivityEvent` for
/// the capture coordinator. Returns `None` for variants without a
/// user-visible activity shape.
fn gadget_audit_to_captured(event: &GadgetAuditEvent) -> Option<CapturedActivityEvent> {
    match event {
        GadgetAuditEvent::GadgetCallCompleted {
            gadget_name,
            tier,
            category,
            outcome,
            elapsed_ms,
            conversation_id,
            tenant_id,
            owner_id,
            ..
        } => {
            let (outcome_text, error_code) = match outcome {
                GadgetCallOutcome::Success => ("success", None),
                GadgetCallOutcome::Error { error_code } => ("error", Some(*error_code)),
            };
            let tenant_uuid = tenant_id
                .as_deref()
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or_else(Uuid::nil);
            let actor_uuid = owner_id
                .as_deref()
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or_else(Uuid::nil);
            Some(CapturedActivityEvent {
                id: Uuid::new_v4(),
                tenant_id: tenant_uuid,
                actor_user_id: actor_uuid,
                request_id: None,
                origin: ActivityOrigin::Penny,
                kind: ActivityKind::GadgetToolCall,
                title: format!("penny tool call: {gadget_name}"),
                summary: format!(
                    "outcome={outcome_text}, elapsed_ms={elapsed_ms}{}",
                    error_code
                        .map(|c| format!(", error_code={c}"))
                        .unwrap_or_default()
                ),
                source_bundle: None,
                source_capability: Some(gadget_name.clone()),
                audit_event_id: None,
                facts: serde_json::json!({
                    "gadget_name": gadget_name,
                    "tier": tier.as_str(),
                    "category": category,
                    "outcome": outcome_text,
                    "error_code": error_code,
                    "elapsed_ms": elapsed_ms,
                    "conversation_id": conversation_id,
                }),
                created_at: chrono::Utc::now(),
            })
        }
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

fn captured_actor(ev: &CapturedActivityEvent) -> AuthenticatedContext {
    // `ev.actor_user_id` is the CapturedActivityEvent's snapshot of
    // the api_key_id (matches the pre-ISSUE-24 legacy semantic of
    // `AuthenticatedContext.user_id`). `real_user_id` stays None
    // here — CapturedActivityEvent doesn't carry the owning user_id
    // as a separate field yet; adding it is ISSUE 25 scope.
    AuthenticatedContext {
        api_key_id: ev.actor_user_id,
        tenant_id: ev.tenant_id,
        real_user_id: None,
    }
}

/// Convert a `GadgetAuditEvent` into the matching `ActivityEvent`
/// for the live feed. Returns `None` when the event has no
/// user-visible shape (future approval variants, etc).
fn gadget_audit_to_activity(event: &GadgetAuditEvent) -> Option<ActivityEvent> {
    match event {
        GadgetAuditEvent::GadgetCallCompleted {
            gadget_name,
            tier,
            category,
            outcome,
            elapsed_ms,
            conversation_id,
            tenant_id,
            arguments_summary,
            ..
        } => {
            let (outcome_text, error_code) = match outcome {
                GadgetCallOutcome::Success => ("success".to_string(), None),
                GadgetCallOutcome::Error { error_code } => {
                    ("error".to_string(), Some((*error_code).to_string()))
                }
            };
            let tenant_uuid = tenant_id
                .as_deref()
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or_else(Uuid::nil);
            Some(ActivityEvent::ToolCallCompleted {
                tenant_id: tenant_uuid,
                tool_name: gadget_name.clone(),
                category: category.clone(),
                tier: tier.as_str().to_string(),
                outcome: outcome_text,
                error_code,
                elapsed_ms: *elapsed_ms as i64,
                conversation_id: conversation_id.clone(),
                arguments_summary: arguments_summary.clone(),
            })
        }
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// Async consumer task — pulls `GadgetAuditEvent` off the receiver and
/// INSERTs one row per `GadgetCallCompleted` into `tool_audit_events`.
/// Exits cleanly when the channel closes (all writers dropped).
///
/// Errors are logged but never propagated; audit loss is preferable
/// to a crash loop in the writer.
///
/// Spec: ISSUE 5 TASK 5.1 — wires the Penny tool-call audit trail
/// from Noop to real Postgres persistence. Consumes the event stream
/// that the Penny session emitter pushes.
pub async fn run_gadget_audit_writer(mut rx: mpsc::Receiver<GadgetAuditEvent>, pool: PgPool) {
    while let Some(event) = rx.recv().await {
        if let Err(e) = insert_gadget_event(&pool, &event).await {
            tracing::warn!(
                target: "penny_audit",
                error = %e,
                ?event,
                "failed to persist gadget audit event (continuing)",
            );
        }
    }
    tracing::info!(target: "penny_audit", "gadget audit writer exiting — channel closed");
}

/// Query response row for `GET /api/v1/web/workbench/audit/tool-events`.
/// Mirrors the `tool_audit_events` table schema and parallels the
/// `ActionAuditRow` shape from the action-audit query endpoint so
/// the two HTTP surfaces look symmetric to the dashboard client.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct ToolAuditRow {
    pub id: i64,
    pub tool_name: String,
    pub tier: String,
    pub category: String,
    pub outcome: String,
    pub error_code: Option<String>,
    pub elapsed_ms: i64,
    pub conversation_id: Option<String>,
    pub claude_session_uuid: Option<String>,
    pub owner_id: Option<String>,
    pub tenant_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Filter shape for the tool-audit query endpoint. `tenant_id` is
/// always set by the handler (from the authenticated actor) so a
/// tenant cannot read another tenant's tool-call trail.
#[derive(Debug, Clone, Default)]
pub struct ToolAuditQueryFilter {
    pub tenant_id: String,
    pub tool_name: Option<String>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub limit: i64,
}

/// Query `tool_audit_events` filtered by tenant + optional tool_name +
/// optional `since` timestamp, ordered newest-first. `limit` is clamped
/// to `[1, 500]` by the caller.
///
/// Four prepared SQL variants (same pattern as
/// `query_action_audit_events`) — no dynamic SQL string building.
pub async fn query_tool_audit_events(
    pool: &PgPool,
    filter: &ToolAuditQueryFilter,
) -> Result<Vec<ToolAuditRow>, sqlx::Error> {
    let limit = filter.limit;
    match (filter.tool_name.as_deref(), filter.since) {
        (None, None) => {
            sqlx::query_as::<_, ToolAuditRow>(
                r#"SELECT id, tool_name, tier, category, outcome, error_code,
                          elapsed_ms, conversation_id, claude_session_uuid,
                          owner_id, tenant_id, created_at
                   FROM tool_audit_events
                   WHERE tenant_id = $1
                   ORDER BY created_at DESC
                   LIMIT $2"#,
            )
            .bind(&filter.tenant_id)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        (Some(tool_name), None) => {
            sqlx::query_as::<_, ToolAuditRow>(
                r#"SELECT id, tool_name, tier, category, outcome, error_code,
                          elapsed_ms, conversation_id, claude_session_uuid,
                          owner_id, tenant_id, created_at
                   FROM tool_audit_events
                   WHERE tenant_id = $1 AND tool_name = $2
                   ORDER BY created_at DESC
                   LIMIT $3"#,
            )
            .bind(&filter.tenant_id)
            .bind(tool_name)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        (None, Some(since)) => {
            sqlx::query_as::<_, ToolAuditRow>(
                r#"SELECT id, tool_name, tier, category, outcome, error_code,
                          elapsed_ms, conversation_id, claude_session_uuid,
                          owner_id, tenant_id, created_at
                   FROM tool_audit_events
                   WHERE tenant_id = $1 AND created_at >= $2
                   ORDER BY created_at DESC
                   LIMIT $3"#,
            )
            .bind(&filter.tenant_id)
            .bind(since)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        (Some(tool_name), Some(since)) => {
            sqlx::query_as::<_, ToolAuditRow>(
                r#"SELECT id, tool_name, tier, category, outcome, error_code,
                          elapsed_ms, conversation_id, claude_session_uuid,
                          owner_id, tenant_id, created_at
                   FROM tool_audit_events
                   WHERE tenant_id = $1 AND tool_name = $2 AND created_at >= $3
                   ORDER BY created_at DESC
                   LIMIT $4"#,
            )
            .bind(&filter.tenant_id)
            .bind(tool_name)
            .bind(since)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
    }
}

async fn insert_gadget_event(pool: &PgPool, event: &GadgetAuditEvent) -> Result<(), sqlx::Error> {
    match event {
        GadgetAuditEvent::GadgetCallCompleted {
            gadget_name,
            tier,
            category,
            outcome,
            elapsed_ms,
            conversation_id,
            claude_session_uuid,
            owner_id,
            tenant_id,
            arguments_summary: _,
        } => {
            let (outcome_text, error_code) = match outcome {
                GadgetCallOutcome::Success => ("success", None),
                GadgetCallOutcome::Error { error_code } => ("error", Some(*error_code)),
            };
            sqlx::query(
                r#"
                INSERT INTO tool_audit_events
                    (request_id, tool_name, tier, category, outcome, error_code,
                     elapsed_ms, conversation_id, claude_session_uuid,
                     owner_id, tenant_id)
                VALUES (NULL, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
            )
            .bind(gadget_name)
            .bind(tier.as_str())
            .bind(category)
            .bind(outcome_text)
            .bind(error_code)
            .bind(*elapsed_ms as i64)
            .bind(conversation_id.as_deref())
            .bind(claude_session_uuid.as_deref())
            .bind(owner_id.as_deref())
            .bind(tenant_id.as_deref())
            .execute(pool)
            .await?;
            Ok(())
        }
        // GadgetAuditEvent is #[non_exhaustive]; approval-flow variants
        // land in a future TASK.
        #[allow(unreachable_patterns)]
        _ => Ok(()),
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
            arguments_summary: None,
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

    // -----------------------------------------------------------------------
    // ISSUE 6 TASK 6.1 — coordinator fan-out
    // -----------------------------------------------------------------------

    /// Capturing coordinator that records every `capture_action` call
    /// so tests can assert fan-out happens on `send`.
    #[derive(Debug, Default)]
    struct CapturingCoordinator {
        captured: std::sync::Mutex<Vec<CapturedActivityEvent>>,
    }

    #[async_trait::async_trait]
    impl KnowledgeCandidateCoordinator for CapturingCoordinator {
        async fn capture_action(
            &self,
            _actor: &AuthenticatedContext,
            event: CapturedActivityEvent,
            _hints: Vec<gadgetron_core::knowledge::candidate::CandidateHint>,
        ) -> gadgetron_core::knowledge::candidate::CaptureResult<
            Vec<gadgetron_core::knowledge::candidate::KnowledgeCandidate>,
        > {
            self.captured
                .lock()
                .expect("capturing coord mutex poisoned")
                .push(event);
            Ok(vec![])
        }

        async fn materialize_accepted_candidate(
            &self,
            _actor: &AuthenticatedContext,
            _candidate_id: Uuid,
            _write: gadgetron_core::knowledge::candidate::KnowledgeDocumentWrite,
        ) -> gadgetron_core::knowledge::candidate::CaptureResult<
            gadgetron_core::knowledge::KnowledgePath,
        > {
            unreachable!("test-only coordinator — materialize is never called")
        }
    }

    fn event_with_owner_tenant(tenant: Uuid, owner: Uuid) -> GadgetAuditEvent {
        GadgetAuditEvent::GadgetCallCompleted {
            gadget_name: "wiki.search".into(),
            tier: GadgetTier::Read,
            category: "knowledge".into(),
            outcome: GadgetCallOutcome::Success,
            elapsed_ms: 17,
            conversation_id: Some("conv-42".into()),
            claude_session_uuid: None,
            owner_id: Some(owner.to_string()),
            tenant_id: Some(tenant.to_string()),
            arguments_summary: None,
        }
    }

    #[tokio::test]
    async fn coordinator_fan_out_captures_penny_origin_event() {
        let coord = Arc::new(CapturingCoordinator::default());
        let (writer, _rx) = GadgetAuditEventWriter::new(16);
        let writer =
            writer.with_coordinator(coord.clone() as Arc<dyn KnowledgeCandidateCoordinator>);
        let tenant = Uuid::new_v4();
        let owner = Uuid::new_v4();
        writer.send(event_with_owner_tenant(tenant, owner));

        // Give the spawned capture task a tick.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let events = coord
            .captured
            .lock()
            .expect("capturing coord mutex poisoned")
            .clone();
        assert_eq!(events.len(), 1);
        let captured = &events[0];
        assert_eq!(captured.origin, ActivityOrigin::Penny);
        assert_eq!(captured.kind, ActivityKind::GadgetToolCall);
        assert_eq!(captured.tenant_id, tenant);
        assert_eq!(captured.actor_user_id, owner);
        assert_eq!(captured.source_capability.as_deref(), Some("wiki.search"));
    }

    #[tokio::test]
    async fn no_coordinator_means_no_capture() {
        // Regression guard — the default writer has no coordinator and
        // must not panic / allocate on send.
        let (writer, _rx) = GadgetAuditEventWriter::new(16);
        writer.send(make_event());
        // If this returns, fan-out on an unwired coordinator is a no-op.
    }
}
