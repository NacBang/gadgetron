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
}

/// Insert a single billing event. Fire-and-forget: callers
/// typically `tokio::spawn` this or run it inside a broader
/// `record_post` that already logs DB failures.
pub async fn insert_billing_event(
    pool: &PgPool,
    tenant_id: Uuid,
    kind: BillingEventKind,
    cost_cents: i64,
    source_event_id: Option<Uuid>,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO billing_events
            (tenant_id, event_kind, source_event_id, cost_cents, model, provider)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id)
    .bind(kind.as_str())
    .bind(source_event_id)
    .bind(cost_cents)
    .bind(model)
    .bind(provider)
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
                       model, provider, created_at
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
                       model, provider, created_at
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
