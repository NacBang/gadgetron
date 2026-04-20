use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Per-entry correlation key. A single `request_id` can produce multiple
    /// audit entries (Ok + Err, streaming start + stream_interrupted, PR 6
    /// Drop-guard Ok + stream-end amendment) and each row needs its own
    /// identity so downstream consumers — PostgreSQL `audit_log.id`,
    /// `CapturedActivityEvent.audit_event_id` on the shared-surface plane —
    /// can JOIN without ambiguity.
    ///
    /// Callers generate this via `Uuid::new_v4()` at the point of emit and
    /// pass the same UUID to any correlated capture / write side so the
    /// audit row and the activity row agree on correlation identity.
    ///
    /// Distinct from `request_id`: `request_id` is the HTTP request scope
    /// (one per inbound request), `event_id` is the audit-row scope (one
    /// per `AuditWriter::send`). Setting `event_id = request_id` is a
    /// bug — covered by the `event_id_distinct_from_request_id` unit test.
    ///
    /// Spec: D-20260418-24 (drift-fix PR 5).
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub api_key_id: Uuid,
    /// Owning user (ISSUE 19 plumbing). Populated from
    /// `ValidatedKey.user_id` when available (both Bearer and
    /// cookie-session paths). `None` for legacy keys predating the
    /// ISSUE 14 TASK 14.1 backfill. Persistence into
    /// `audit_log.actor_user_id` follows in ISSUE 20 when the pg
    /// audit consumer lands.
    pub actor_user_id: Option<Uuid>,
    /// Populated from `ValidatedKey.api_key_id` when a real Bearer
    /// key is used. `None` for cookie-session callers where
    /// `api_key_id == Uuid::nil()` (sentinel). Lets downstream
    /// distinguish "API-key activity" from "web UI session activity"
    /// without string-matching on the sentinel UUID.
    pub actor_api_key_id: Option<Uuid>,
    pub request_id: Uuid,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub status: AuditStatus,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_cents: i64,
    pub latency_ms: i32,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditStatus {
    Ok,
    Error,
    StreamInterrupted,
    // Phase 2 may add `Throttled`, `RateLimited` variants.
}

impl AuditStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::StreamInterrupted => "stream_interrupted",
        }
    }
}

pub struct AuditWriter {
    tx: tokio::sync::mpsc::Sender<AuditEntry>,
    dropped: Arc<AtomicU64>,
}

impl AuditWriter {
    pub fn new(channel_capacity: usize) -> (Self, tokio::sync::mpsc::Receiver<AuditEntry>) {
        let (tx, rx) = tokio::sync::mpsc::channel(channel_capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        (Self { tx, dropped }, rx)
    }

    pub fn send(&self, entry: AuditEntry) {
        if self.tx.try_send(entry).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("audit entry dropped — channel full");
        }
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Async pg consumer task — pulls `AuditEntry` off the receiver and
/// INSERTs one row per entry into `audit_log`. ISSUE 21 / PR follow-up.
/// Exits cleanly when the channel closes (all writers dropped).
///
/// Errors are logged as `warn` but never propagated — audit loss is
/// preferable to a crash loop in the writer that would drop every
/// subsequent entry too. Mirrors `run_action_audit_writer`.
///
/// Columns: reuses the ISSUE 14 TASK 14.1 `actor_user_id` +
/// `actor_api_key_id` additions so a cookie-session caller (ISSUE 16
/// → nil-sentinel `api_key_id`) persists with `actor_api_key_id =
/// NULL` and the Bearer caller gets the real key id.
pub async fn run_audit_log_writer(
    mut rx: tokio::sync::mpsc::Receiver<AuditEntry>,
    pool: sqlx::PgPool,
) {
    while let Some(entry) = rx.recv().await {
        // Keep the legacy tracing line — a downstream scraper / the
        // e2e harness's gadgetron.log audit-presence gates still read
        // these. The pg INSERT is a side effect of the same event.
        tracing::info!(
            target: "audit",
            tenant_id = %entry.tenant_id,
            api_key_id = %entry.api_key_id,
            request_id = %entry.request_id,
            status = entry.status.as_str(),
            input_tokens = entry.input_tokens,
            output_tokens = entry.output_tokens,
            latency_ms = entry.latency_ms,
            "audit"
        );
        // Skip pg INSERT when the caller is unauthenticated
        // (emit_auth_failure_audit uses Uuid::nil() for tenant_id +
        // api_key_id). The tracing line above is sufficient for SOC2
        // CC6.7 attestation on the 401 path; a real row would violate
        // the audit_log_tenant_id_fkey FK against tenants(id).
        if entry.tenant_id == uuid::Uuid::nil() {
            continue;
        }
        let result = sqlx::query(
            r#"
            INSERT INTO audit_log (
                id, tenant_id, api_key_id, request_id, model, provider,
                status, input_tokens, output_tokens, cost_cents, latency_ms,
                actor_user_id, actor_api_key_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(entry.event_id)
        .bind(entry.tenant_id)
        .bind(entry.api_key_id)
        .bind(entry.request_id)
        .bind(entry.model.as_deref())
        .bind(entry.provider.as_deref())
        .bind(entry.status.as_str())
        .bind(entry.input_tokens)
        .bind(entry.output_tokens)
        .bind(entry.cost_cents)
        .bind(entry.latency_ms)
        .bind(entry.actor_user_id)
        .bind(entry.actor_api_key_id)
        .execute(&pool)
        .await;
        if let Err(e) = result {
            tracing::warn!(
                target: "audit",
                event_id = %entry.event_id,
                error = %e,
                "failed to persist audit_log row (continuing)"
            );
        }
    }
    tracing::info!(target: "audit", "audit_log writer exiting — channel closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry() -> AuditEntry {
        AuditEntry {
            event_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            actor_user_id: None,
            actor_api_key_id: None,
            request_id: Uuid::new_v4(),
            model: Some("gpt-4o-mini".to_string()),
            provider: Some("openai".to_string()),
            status: AuditStatus::Ok,
            input_tokens: 100,
            output_tokens: 50,
            cost_cents: 0,
            latency_ms: 250,
        }
    }

    #[tokio::test]
    async fn send_delivers_to_receiver() {
        let (writer, mut rx) = AuditWriter::new(16);
        writer.send(make_entry());
        let received = rx.recv().await.unwrap();
        assert_eq!(received.status, AuditStatus::Ok);
        assert_eq!(received.input_tokens, 100);
    }

    #[tokio::test]
    async fn send_multiple_entries() {
        let (writer, mut rx) = AuditWriter::new(16);
        for _ in 0..5 {
            writer.send(make_entry());
        }
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn drops_when_channel_full() {
        let (writer, _rx) = AuditWriter::new(2);
        writer.send(make_entry());
        writer.send(make_entry());
        writer.send(make_entry());
        assert!(writer.dropped_count() >= 1);
    }

    #[tokio::test]
    async fn dropped_count_starts_at_zero() {
        let (writer, _rx) = AuditWriter::new(16);
        assert_eq!(writer.dropped_count(), 0);
    }

    #[test]
    fn audit_status_as_str() {
        assert_eq!(AuditStatus::Ok.as_str(), "ok");
        assert_eq!(AuditStatus::Error.as_str(), "error");
        assert_eq!(
            AuditStatus::StreamInterrupted.as_str(),
            "stream_interrupted"
        );
    }

    #[tokio::test]
    async fn entry_fields_preserved() {
        let (writer, mut rx) = AuditWriter::new(16);
        let entry = AuditEntry {
            event_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            api_key_id: Uuid::nil(),
            actor_user_id: None,
            actor_api_key_id: None,
            request_id: Uuid::nil(),
            model: None,
            provider: None,
            status: AuditStatus::StreamInterrupted,
            input_tokens: 0,
            output_tokens: 0,
            cost_cents: 0,
            latency_ms: 0,
        };
        writer.send(entry);
        let received = rx.recv().await.unwrap();
        assert_eq!(received.status, AuditStatus::StreamInterrupted);
        assert!(received.model.is_none());
    }

    // Drift-fix PR 5 (D-20260418-24): event_id contract tests

    #[tokio::test]
    async fn event_id_round_trips_through_send_recv() {
        let (writer, mut rx) = AuditWriter::new(16);
        let expected = Uuid::new_v4();
        let mut entry = make_entry();
        entry.event_id = expected;
        writer.send(entry);
        let received = rx.recv().await.unwrap();
        assert_eq!(
            received.event_id, expected,
            "event_id must round-trip verbatim through the channel"
        );
    }

    #[tokio::test]
    async fn event_id_unique_across_multiple_sends() {
        let (writer, mut rx) = AuditWriter::new(16);
        writer.send(make_entry());
        writer.send(make_entry());
        writer.send(make_entry());
        let mut seen: Vec<Uuid> = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            assert!(
                !seen.contains(&entry.event_id),
                "duplicate event_id {} emitted — make_entry MUST call Uuid::new_v4() per call",
                entry.event_id
            );
            seen.push(entry.event_id);
        }
        assert_eq!(seen.len(), 3);
    }

    #[tokio::test]
    async fn event_id_distinct_from_request_id_on_same_request_multiple_entries() {
        // A single request can produce multiple audit entries — for example
        // streaming emits an `Ok` at dispatch then (PR 6) a `StreamInterrupted`
        // or token-amended entry at stream-end. Each audit row must carry its
        // own `event_id` so `audit_log.id` stays a real primary key; the
        // shared `request_id` stitches them together under trace continuity.
        let (writer, mut rx) = AuditWriter::new(16);
        let request_id = Uuid::new_v4();
        for _ in 0..2 {
            let mut entry = make_entry();
            entry.request_id = request_id;
            writer.send(entry);
        }
        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert_eq!(first.request_id, second.request_id, "request_id shared");
        assert_ne!(
            first.event_id, second.event_id,
            "event_id MUST differ between two audit rows on the same request"
        );
        assert_ne!(
            first.event_id, first.request_id,
            "event_id and request_id MUST NOT collapse into the same Uuid"
        );
    }
}
