//! Thin sqlx wrapper around `log_findings`, `log_scan_cursor`,
//! `log_scan_config`. All methods are tenant-scoped at the WHERE
//! clause; callers pass the tenant id explicitly.
//!
//! Folding rule: identical (host_id, source, category) entries seen
//! within `FOLD_WINDOW` get rolled into the existing open finding
//! (count++, ts_last bumped). A fresh row is only inserted when the
//! prior finding is dismissed OR is older than the window — keeps
//! flapping kernels from drowning the UI but still surfaces new
//! incidents after an operator clears the previous batch.

use crate::model::{Classification, Finding};
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub const FOLD_WINDOW: Duration = Duration::hours(1);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn list_open(
    pool: &PgPool,
    tenant_id: Uuid,
    host_filter: Option<Uuid>,
    severity_filter: Option<&str>,
    limit: i64,
) -> Result<Vec<Finding>, StoreError> {
    let limit = limit.clamp(1, 1000);
    // Build the query with optional host / severity filters.
    let rows = match (host_filter, severity_filter) {
        (Some(h), Some(s)) => {
            sqlx::query_as::<_, Finding>(
                "SELECT * FROM log_findings WHERE tenant_id = $1 AND host_id = $2 \
                 AND severity = $3 AND dismissed_at IS NULL ORDER BY ts_last DESC LIMIT $4",
            )
            .bind(tenant_id)
            .bind(h)
            .bind(s)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (Some(h), None) => {
            sqlx::query_as::<_, Finding>(
                "SELECT * FROM log_findings WHERE tenant_id = $1 AND host_id = $2 \
                 AND dismissed_at IS NULL ORDER BY ts_last DESC LIMIT $3",
            )
            .bind(tenant_id)
            .bind(h)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (None, Some(s)) => {
            sqlx::query_as::<_, Finding>(
                "SELECT * FROM log_findings WHERE tenant_id = $1 AND severity = $2 \
                 AND dismissed_at IS NULL ORDER BY ts_last DESC LIMIT $3",
            )
            .bind(tenant_id)
            .bind(s)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (None, None) => {
            sqlx::query_as::<_, Finding>(
                "SELECT * FROM log_findings WHERE tenant_id = $1 \
                 AND dismissed_at IS NULL ORDER BY ts_last DESC LIMIT $2",
            )
            .bind(tenant_id)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

/// Per-host counts grouped by severity. Powers the dashboard badge.
pub async fn counts_by_host(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<(Uuid, String, i64)>, StoreError> {
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT host_id, severity, count(*) \
         FROM log_findings \
         WHERE tenant_id = $1 AND dismissed_at IS NULL \
         GROUP BY host_id, severity",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn upsert_finding(
    pool: &PgPool,
    tenant_id: Uuid,
    host_id: Uuid,
    source: &str,
    cls: &Classification,
    excerpt: &str,
    classified_by: &str,
) -> Result<Uuid, StoreError> {
    let now = Utc::now();
    let cutoff = now - FOLD_WINDOW;
    // Look for an OPEN finding within the fold window matching the
    // same category — that's our roll-up target.
    let existing: Option<(Uuid, i32)> = sqlx::query_as(
        "SELECT id, count FROM log_findings \
         WHERE tenant_id = $1 AND host_id = $2 AND source = $3 AND category = $4 \
         AND dismissed_at IS NULL AND ts_last >= $5 \
         ORDER BY ts_last DESC LIMIT 1",
    )
    .bind(tenant_id)
    .bind(host_id)
    .bind(source)
    .bind(&cls.category)
    .bind(cutoff)
    .fetch_optional(pool)
    .await?;

    if let Some((id, _count)) = existing {
        sqlx::query(
            "UPDATE log_findings \
             SET count = count + 1, ts_last = $1, excerpt = $2 \
             WHERE id = $3",
        )
        .bind(now)
        .bind(truncate(excerpt, 1024))
        .bind(id)
        .execute(pool)
        .await?;
        Ok(id)
    } else {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO log_findings \
                (tenant_id, host_id, source, severity, category, summary, excerpt, \
                 ts_first, ts_last, classified_by, cause, solution, remediation) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9, $10, $11, $12) \
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(host_id)
        .bind(source)
        .bind(cls.severity.as_str())
        .bind(&cls.category)
        .bind(&cls.summary)
        .bind(truncate(excerpt, 1024))
        .bind(now)
        .bind(classified_by)
        .bind(cls.cause.as_deref())
        .bind(cls.solution.as_deref())
        .bind(cls.remediation.as_ref())
        .fetch_one(pool)
        .await?;
        Ok(id)
    }
}

/// Fetch the remediation JSON (and host_id) for one finding so the
/// "승인 실행" path can dispatch the embedded action.
pub async fn get_remediation(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<Option<(Uuid, serde_json::Value)>, StoreError> {
    let row: Option<(Uuid, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT host_id, remediation FROM log_findings \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(h, r)| r.map(|r| (h, r))))
}

pub async fn dismiss(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    actor_user_id: Option<Uuid>,
) -> Result<bool, StoreError> {
    let affected = sqlx::query(
        "UPDATE log_findings SET dismissed_at = now(), dismissed_by = $1 \
         WHERE id = $2 AND tenant_id = $3 AND dismissed_at IS NULL",
    )
    .bind(actor_user_id)
    .bind(id)
    .bind(tenant_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(affected > 0)
}

pub async fn get_cursor(
    pool: &PgPool,
    host_id: Uuid,
    source: &str,
) -> Result<Option<(String, DateTime<Utc>)>, StoreError> {
    let row: Option<(Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT last_cursor, last_scanned_at FROM log_scan_cursor \
         WHERE host_id = $1 AND source = $2",
    )
    .bind(host_id)
    .bind(source)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(c, t)| c.map(|cur| (cur, t))))
}

pub async fn set_cursor(
    pool: &PgPool,
    host_id: Uuid,
    source: &str,
    cursor: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO log_scan_cursor (host_id, source, last_cursor, last_scanned_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (host_id, source) DO UPDATE SET \
             last_cursor = EXCLUDED.last_cursor, last_scanned_at = now()",
    )
    .bind(host_id)
    .bind(source)
    .bind(cursor)
    .execute(pool)
    .await?;
    Ok(())
}

/// Scan status per host: most-recent scan timestamp across any
/// source + the per-host interval/enabled config (or defaults).
/// Powers the UI's "last scan: 30s ago · interval: 120s" line.
pub async fn list_scan_status(
    pool: &PgPool,
) -> Result<Vec<(Uuid, Option<DateTime<Utc>>, i32, bool)>, StoreError> {
    let rows: Vec<(Uuid, Option<DateTime<Utc>>, Option<i32>, Option<bool>)> = sqlx::query_as(
        "SELECT c.host_id, \
                MAX(c.last_scanned_at) AS last_scanned, \
                cfg.interval_secs, \
                cfg.enabled \
         FROM log_scan_cursor c \
         LEFT JOIN log_scan_config cfg ON cfg.host_id = c.host_id \
         WHERE c.source <> '_meta' \
         GROUP BY c.host_id, cfg.interval_secs, cfg.enabled",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(h, t, i, e)| (h, t, i.unwrap_or(120), e.unwrap_or(true)))
        .collect())
}

pub async fn get_config(pool: &PgPool, host_id: Uuid) -> Result<Option<(i32, bool)>, StoreError> {
    let row: Option<(i32, bool)> =
        sqlx::query_as("SELECT interval_secs, enabled FROM log_scan_config WHERE host_id = $1")
            .bind(host_id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

pub async fn set_config(
    pool: &PgPool,
    host_id: Uuid,
    interval_secs: i32,
    enabled: bool,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO log_scan_config (host_id, interval_secs, enabled, updated_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (host_id) DO UPDATE SET \
             interval_secs = EXCLUDED.interval_secs, enabled = EXCLUDED.enabled, \
             updated_at = now()",
    )
    .bind(host_id)
    .bind(interval_secs)
    .bind(enabled)
    .execute(pool)
    .await?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}
