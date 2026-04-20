//! Billing event writer + query helpers (ISSUE 12 TASK 12.1).

use sqlx::PgPool;
use uuid::Uuid;

/// Kind of billable event. Wire-frozen — the `billing_events.event_kind`
/// column has a CHECK constraint matching these strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingEventKind {
    Chat,
    Tool,
    Action,
}

impl BillingEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Tool => "tool",
            Self::Action => "action",
        }
    }
}

/// One `billing_events` row — projection for query endpoints +
/// test helpers.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct BillingEventRow {
    pub id: i64,
    pub tenant_id: Uuid,
    pub event_kind: String,
    pub source_event_id: Option<Uuid>,
    pub cost_cents: i64,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Owning user (ISSUE 23). Denormalized projection of
    /// `audit_log.actor_user_id` for query-ergonomic per-user spend
    /// reports — NOT the source of truth (audit_log is). Invoice
    /// materializer SHOULD prefer the audit_log join when both are
    /// present.
    ///
    /// NULL for:
    ///   * legacy events pre-dating the
    ///     `20260420000005_billing_events_actor_user_id.sql` migration
    ///   * chat path (QuotaToken doesn't thread user_id until ISSUE 24)
    ///   * action path (AuthenticatedContext.user_id is api_key_id
    ///     placeholder until ISSUE 24; see security review)
    ///   * callers whose `ValidatedKey.user_id` is None (legacy
    ///     api_keys pre-ISSUE-14 backfill)
    pub actor_user_id: Option<Uuid>,
}

/// Owned, `'static`-safe insert payload for `insert_billing_event`.
/// Replaces the pre-v0.5.16 8-arg flat parameter list. Owned `String`
/// fields (not `&str`) so callers can move the struct into a
/// `tokio::spawn` closure without lifetime gymnastics — the tool +
/// action paths already do this.
///
/// Use one of the typed constructors (`chat`, `tool`, `action`) to
/// encode the kind + default cost invariant; then layer the
/// `with_*` builders for the optional fields. The direct struct
/// literal form is intentionally pub'd so new paths can extend
/// the shape without going through a constructor.
#[derive(Debug, Clone)]
pub struct BillingEventInsert {
    pub tenant_id: Uuid,
    pub kind: BillingEventKind,
    pub cost_cents: i64,
    pub source_event_id: Option<Uuid>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub actor_user_id: Option<Uuid>,
}

impl BillingEventInsert {
    /// Chat event — enforcer hot-path. `cost_cents` is the live
    /// request cost from `record_post`. Optional fields (including
    /// `actor_user_id`) default to NULL; ISSUE 24 will populate
    /// `actor_user_id` from `QuotaToken.user_id` once that field
    /// lands.
    pub fn chat(tenant_id: Uuid, cost_cents: i64) -> Self {
        Self {
            tenant_id,
            kind: BillingEventKind::Chat,
            cost_cents,
            source_event_id: None,
            model: None,
            provider: None,
            actor_user_id: None,
        }
    }

    /// Tool invocation (`/v1/tools/{name}/invoke`). `cost_cents = 0`
    /// today — the invoice materializer (TASK 12.3, deferred)
    /// attributes cost at aggregation time from the underlying
    /// model call. `gadget_name` is stored in the `model` column,
    /// not a dedicated slot — legacy mapping matches the pre-refactor
    /// call at `handlers.rs`. ISSUE 24 queues the real model
    /// identifier into its own field.
    pub fn tool(tenant_id: Uuid, gadget_name: String) -> Self {
        Self {
            tenant_id,
            kind: BillingEventKind::Tool,
            cost_cents: 0,
            source_event_id: None,
            model: Some(gadget_name),
            provider: None,
            actor_user_id: None,
        }
    }

    /// Direct / approved action terminal. `source_event_id` pins
    /// the row to the `audit_log` event the action emitted —
    /// operator reconciliation JOINs on this. Same legacy
    /// `gadget_name → model` mapping as `tool`.
    pub fn action(tenant_id: Uuid, audit_event_id: Uuid, gadget_name: Option<String>) -> Self {
        Self {
            tenant_id,
            kind: BillingEventKind::Action,
            cost_cents: 0,
            source_event_id: Some(audit_event_id),
            model: gadget_name,
            provider: None,
            actor_user_id: None,
        }
    }

    /// Set `actor_user_id` (ISSUE 23). NULL is always acceptable
    /// (column is nullable); populated rows enable per-user spend
    /// queries without a join. See `billing_events` migration
    /// comment for the FK-less rationale.
    pub fn with_actor_user(mut self, actor_user_id: Option<Uuid>) -> Self {
        self.actor_user_id = actor_user_id;
        self
    }
}

/// Insert a single billing event. Fire-and-forget: callers
/// typically `tokio::spawn` this or run it inside a broader
/// `record_post` that already logs DB failures.
pub async fn insert_billing_event(
    pool: &PgPool,
    event: BillingEventInsert,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO billing_events
            (tenant_id, event_kind, source_event_id, cost_cents, model, provider, actor_user_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(event.tenant_id)
    .bind(event.kind.as_str())
    .bind(event.source_event_id)
    .bind(event.cost_cents)
    .bind(event.model)
    .bind(event.provider)
    .bind(event.actor_user_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Query a tenant's billing events over a time window (for the
/// admin billing endpoint). Default ordering is newest-first so
/// invoice views show the latest activity at the top.
pub async fn query_billing_events(
    pool: &PgPool,
    tenant_id: Uuid,
    since: Option<chrono::DateTime<chrono::Utc>>,
    limit: i64,
) -> Result<Vec<BillingEventRow>, sqlx::Error> {
    let rows = match since {
        Some(s) => {
            sqlx::query_as::<_, BillingEventRow>(
                r#"
                SELECT id, tenant_id, event_kind, source_event_id, cost_cents,
                       model, provider, created_at, actor_user_id
                FROM billing_events
                WHERE tenant_id = $1 AND created_at >= $2
                ORDER BY created_at DESC
                LIMIT $3
                "#,
            )
            .bind(tenant_id)
            .bind(s)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, BillingEventRow>(
                r#"
                SELECT id, tenant_id, event_kind, source_event_id, cost_cents,
                       model, provider, created_at, actor_user_id
                FROM billing_events
                WHERE tenant_id = $1
                ORDER BY created_at DESC
                LIMIT $2
                "#,
            )
            .bind(tenant_id)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billing_event_kind_strings_are_wire_frozen() {
        // The `billing_events.event_kind` CHECK constraint matches
        // these exact strings; renaming a variant without a migration
        // breaks inserts silently.
        assert_eq!(BillingEventKind::Chat.as_str(), "chat");
        assert_eq!(BillingEventKind::Tool.as_str(), "tool");
        assert_eq!(BillingEventKind::Action.as_str(), "action");
    }
}
