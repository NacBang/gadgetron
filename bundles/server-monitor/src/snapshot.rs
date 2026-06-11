//! Latest-ServerStats snapshot persistence (ISSUE 38).
//!
//! The 1 Hz background poller UPSERTs one JSONB row per host into
//! `host_stats_latest`; the `server.stats` gadget serves reads from
//! that row. Collection cost is therefore bounded at one SSH collector
//! per host per tick no matter how many browser tabs are polling.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::collectors::ServerStats;

/// Snapshot rows older than this are treated as missing — the gadget
/// then falls back to a direct SSH collect. 5 s covers several poller
/// ticks, so a healthy poller always wins; the fallback only fires when
/// the poller is absent (no pool), not yet warmed up, or wedged.
pub const SNAPSHOT_FRESH_SECS: i64 = 5;

/// Pure freshness predicate, split out so it can be unit-tested without
/// a database.
pub(crate) fn snapshot_is_fresh(
    fetched_at: DateTime<Utc>,
    now: DateTime<Utc>,
    max_age_secs: i64,
) -> bool {
    now.signed_duration_since(fetched_at).num_seconds() < max_age_secs
}

/// Upsert the latest snapshot for a host. Failures are logged and
/// swallowed — snapshot persistence must never break collection.
pub(crate) async fn upsert_snapshot(
    pool: &PgPool,
    tenant_id: Uuid,
    host_id: Uuid,
    stats: &ServerStats,
) {
    let payload = match serde_json::to_value(stats) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "server_monitor_snapshot",
                host_id = %host_id,
                error = %e,
                "snapshot serialize failed"
            );
            return;
        }
    };
    let res = sqlx::query(
        "INSERT INTO host_stats_latest (host_id, tenant_id, stats, fetched_at) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (host_id) DO UPDATE SET \
             tenant_id = EXCLUDED.tenant_id, \
             stats = EXCLUDED.stats, \
             fetched_at = EXCLUDED.fetched_at",
    )
    .bind(host_id)
    .bind(tenant_id)
    .bind(&payload)
    .bind(stats.fetched_at)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(
            target: "server_monitor_snapshot",
            host_id = %host_id,
            error = %e,
            "snapshot upsert failed"
        );
    }
}

/// Load the snapshot for a host if it exists and is fresh. Tenant id is
/// part of the WHERE clause as a defensive cross-tenant guard even
/// though `host_id` is already unique.
pub(crate) async fn load_fresh_snapshot(
    pool: &PgPool,
    tenant_id: Uuid,
    host_id: Uuid,
) -> Option<serde_json::Value> {
    let row: Option<(serde_json::Value, DateTime<Utc>)> = match sqlx::query_as(
        "SELECT stats, fetched_at FROM host_stats_latest \
         WHERE host_id = $1 AND tenant_id = $2",
    )
    .bind(host_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!(
                target: "server_monitor_snapshot",
                host_id = %host_id,
                error = %e,
                "snapshot read failed — falling back to direct collect"
            );
            None
        }
    };
    let (stats, fetched_at) = row?;
    snapshot_is_fresh(fetched_at, Utc::now(), SNAPSHOT_FRESH_SECS).then_some(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn fresh_within_window() {
        let now = Utc::now();
        assert!(snapshot_is_fresh(now - Duration::seconds(2), now, 5));
    }

    #[test]
    fn stale_at_and_past_window() {
        let now = Utc::now();
        assert!(!snapshot_is_fresh(now - Duration::seconds(5), now, 5));
        assert!(!snapshot_is_fresh(now - Duration::seconds(60), now, 5));
    }

    #[test]
    fn future_timestamps_count_as_fresh() {
        // Clock skew between poller and reader must not blank the card.
        let now = Utc::now();
        assert!(snapshot_is_fresh(now + Duration::seconds(3), now, 5));
    }
}
