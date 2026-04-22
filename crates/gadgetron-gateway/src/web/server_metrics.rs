//! Read-side surface for the `host_metrics` timeseries store.
//!
//! `GET /api/v1/web/workbench/servers/{host_id}/metrics`
//!     ?metric=<name>
//!     &from=<rfc3339>
//!     &to=<rfc3339>
//!     &bucket=<auto|raw|5s|1m|5m|1h>
//!
//! Spec: `docs/design/phase2/16-server-metrics-timeseries.md` §2.4.
//!
//! Tier selection:
//!
//! | window      | tier |
//! |------------:|:-----|
//! |    ≤ 10 min | raw            (`host_metrics`)        |
//! |    ≤  2 hr  | 5 s aggregate  (`host_metrics_5s`)     |
//! |    ≤  2 day | 1 m aggregate  (`host_metrics_1m`)     |
//! |    ≤ 30 day | 5 m aggregate  (`host_metrics_5m`)     |
//! |     > 30 day| 1 h aggregate  (`host_metrics_1h`)     |
//!
//! The window-based switch keeps the response payload between roughly
//! 300 and 2 000 points — enough resolution for visual detail, never
//! enough to choke the browser.
//!
//! All queries are tenant-leading: the `tenant_id` from the request's
//! `TenantContext` is the FIRST WHERE clause. The composite index
//! `(tenant_id, host_id, metric, ts DESC)` guarantees a cross-tenant
//! caller cannot warm-scan another tenant's chunk even by accident.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use gadgetron_core::context::TenantContext;
use gadgetron_core::error::GadgetronError;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::server::AppState;
use crate::web::workbench::WorkbenchHttpError;

/// Query parameters. Defaults: `from = now - 5 min`, `to = now`,
/// `bucket = auto`.
#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    pub metric: String,
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub bucket: Option<String>,
    /// Hard cap on returned points. Server-side max stays at 5 000 even
    /// if the client asks for more — guards against rogue queries that
    /// would bloat the response.
    #[serde(default)]
    pub max_points: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct MetricsResponse {
    pub host_id: Uuid,
    pub metric: String,
    pub unit: Option<String>,
    /// Resolution actually used (`raw`, `5s`, `1m`, `5m`, `1h`).
    pub resolution: &'static str,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub points: Vec<MetricPoint>,
    /// Continuous-aggregate refresh lag in seconds. Always 0 for
    /// `raw` tier; for aggregates this is `now() - max(bucket)` —
    /// when > the tier interval, the UI may want to stitch the tail
    /// from raw.
    pub refresh_lag_seconds: i64,
    /// Number of samples that didn't make it into `host_metrics` over
    /// the response window. v0.2 returns 0 — `IngestionCounters` is
    /// process-local and not yet persisted per (host, ts_bucket).
    pub dropped_frames: u64,
}

#[derive(Debug, Serialize)]
pub struct MetricPoint {
    pub ts: DateTime<Utc>,
    pub avg: f64,
    pub min: f64,
    pub max: f64,
    pub samples: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Raw,
    Sec5,
    Min1,
    Min5,
    Hour1,
}

impl Tier {
    fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Sec5 => "5s",
            Self::Min1 => "1m",
            Self::Min5 => "5m",
            Self::Hour1 => "1h",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "raw" => Some(Self::Raw),
            "5s" => Some(Self::Sec5),
            "1m" => Some(Self::Min1),
            "5m" => Some(Self::Min5),
            "1h" => Some(Self::Hour1),
            _ => None,
        }
    }

    fn auto(window: ChronoDuration) -> Self {
        if window <= ChronoDuration::minutes(10) {
            Self::Raw
        } else if window <= ChronoDuration::hours(2) {
            Self::Sec5
        } else if window <= ChronoDuration::days(2) {
            Self::Min1
        } else if window <= ChronoDuration::days(30) {
            Self::Min5
        } else {
            Self::Hour1
        }
    }

    fn relation(self) -> &'static str {
        match self {
            Self::Raw => "host_metrics",
            Self::Sec5 => "host_metrics_5s",
            Self::Min1 => "host_metrics_1m",
            Self::Min5 => "host_metrics_5m",
            Self::Hour1 => "host_metrics_1h",
        }
    }
}

pub async fn list_server_metrics(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<TenantContext>,
    Path(host_id): Path<Uuid>,
    Query(query): Query<MetricsQuery>,
) -> Result<Json<MetricsResponse>, WorkbenchHttpError> {
    let pool = require_pool(&state)?;
    let to = query.to.unwrap_or_else(Utc::now);
    let from = query
        .from
        .unwrap_or_else(|| to - ChronoDuration::minutes(5));
    if from >= to {
        return Err(WorkbenchHttpError::Core(GadgetronError::Config(
            "metrics query: `from` must be earlier than `to`".into(),
        )));
    }
    let window = to - from;
    let tier = match query.bucket.as_deref().unwrap_or("auto") {
        "auto" => Tier::auto(window),
        explicit => Tier::parse(explicit).ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "metrics query: unknown bucket '{explicit}' (expected auto|raw|5s|1m|5m|1h)"
            )))
        })?,
    };
    let max_points = query.max_points.unwrap_or(5_000).clamp(1, 5_000) as i64;

    let (points, unit) = fetch_points(
        pool,
        ctx.tenant_id,
        host_id,
        &query.metric,
        tier,
        from,
        to,
        max_points,
    )
    .await?;

    let refresh_lag_seconds = match tier {
        Tier::Raw => 0,
        _ => fetch_refresh_lag(pool, tier).await.unwrap_or(0),
    };

    Ok(Json(MetricsResponse {
        host_id,
        metric: query.metric,
        unit,
        resolution: tier.label(),
        from,
        to,
        points,
        refresh_lag_seconds,
        dropped_frames: 0,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn fetch_points(
    pool: &PgPool,
    tenant_id: Uuid,
    host_id: Uuid,
    metric: &str,
    tier: Tier,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    max_points: i64,
) -> Result<(Vec<MetricPoint>, Option<String>), WorkbenchHttpError> {
    // Raw tier and aggregate tiers have different column shapes — raw
    // is `(ts, value, unit)`, aggregates are `(bucket, avg, min, max,
    // samples)`. Normalize into the same `MetricPoint` shape so the
    // client doesn't care which tier it got.
    match tier {
        Tier::Raw => {
            let rows: Vec<(DateTime<Utc>, f64, Option<String>)> = sqlx::query_as(
                r#"
                SELECT ts, value, unit
                FROM host_metrics
                WHERE tenant_id = $1
                  AND host_id   = $2
                  AND metric    = $3
                  AND ts       >= $4
                  AND ts        < $5
                ORDER BY ts ASC
                LIMIT $6
                "#,
            )
            .bind(tenant_id)
            .bind(host_id)
            .bind(metric)
            .bind(from)
            .bind(to)
            .bind(max_points)
            .fetch_all(pool)
            .await
            .map_err(map_db_err)?;
            let unit = rows.first().and_then(|r| r.2.clone());
            let points = rows
                .into_iter()
                .map(|(ts, value, _unit)| MetricPoint {
                    ts,
                    avg: value,
                    min: value,
                    max: value,
                    samples: 1,
                })
                .collect();
            Ok((points, unit))
        }
        _ => {
            // Continuous-aggregate views: use the `bucket` column.
            // `samples` is `bigint` in the 5s rollup but becomes `numeric`
            // from the 1m tier upward — Postgres widens `SUM(bigint)` to
            // `numeric` to avoid overflow. sqlx can't decode `numeric`
            // into `i64` without the `bigdecimal` feature, so we cast
            // back to `bigint` in SQL. Overflow is not a real concern
            // here (samples per hour ≪ 2^63).
            let sql = format!(
                "SELECT bucket AS ts, avg, min, max, samples::bigint \
                 FROM {rel} \
                 WHERE tenant_id = $1 AND host_id = $2 AND metric = $3 \
                   AND bucket >= $4 AND bucket < $5 \
                 ORDER BY bucket ASC LIMIT $6",
                rel = tier.relation(),
            );
            let rows: Vec<(DateTime<Utc>, f64, f64, f64, i64)> = sqlx::query_as(&sql)
                .bind(tenant_id)
                .bind(host_id)
                .bind(metric)
                .bind(from)
                .bind(to)
                .bind(max_points)
                .fetch_all(pool)
                .await
                .map_err(map_db_err)?;
            // Pull the unit from the most recent matching raw row —
            // aggregates don't carry unit (it's the same across the
            // metric definition). One extra cheap lookup; the index
            // makes it microseconds.
            let unit: Option<String> = sqlx::query_scalar(
                "SELECT unit FROM host_metrics \
                 WHERE tenant_id = $1 AND host_id = $2 AND metric = $3 \
                 ORDER BY ts DESC LIMIT 1",
            )
            .bind(tenant_id)
            .bind(host_id)
            .bind(metric)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
            let points = rows
                .into_iter()
                .map(|(ts, avg, min, max, samples)| MetricPoint {
                    ts,
                    avg,
                    min,
                    max,
                    samples,
                })
                .collect();
            Ok((points, unit))
        }
    }
}

async fn fetch_refresh_lag(pool: &PgPool, tier: Tier) -> Option<i64> {
    let sql = format!(
        "SELECT EXTRACT(EPOCH FROM (NOW() - max(bucket)))::BIGINT FROM {rel}",
        rel = tier.relation(),
    );
    sqlx::query_scalar::<_, Option<i64>>(&sql)
        .fetch_one(pool)
        .await
        .ok()
        .flatten()
}

fn require_pool(state: &AppState) -> Result<&PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "metrics history requires Postgres (no pool configured)".into(),
        ))
    })
}

fn map_db_err(e: sqlx::Error) -> WorkbenchHttpError {
    WorkbenchHttpError::Core(GadgetronError::Config(format!(
        "metrics history query failed: {e}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_tier_picks_raw_for_short_windows() {
        assert_eq!(Tier::auto(ChronoDuration::minutes(5)), Tier::Raw);
        assert_eq!(Tier::auto(ChronoDuration::minutes(10)), Tier::Raw);
    }

    #[test]
    fn auto_tier_steps_through_each_bucket() {
        assert_eq!(Tier::auto(ChronoDuration::minutes(11)), Tier::Sec5);
        assert_eq!(Tier::auto(ChronoDuration::hours(1)), Tier::Sec5);
        assert_eq!(Tier::auto(ChronoDuration::hours(3)), Tier::Min1);
        assert_eq!(Tier::auto(ChronoDuration::days(1)), Tier::Min1);
        assert_eq!(Tier::auto(ChronoDuration::days(7)), Tier::Min5);
        assert_eq!(Tier::auto(ChronoDuration::days(60)), Tier::Hour1);
    }

    #[test]
    fn parse_tier_rejects_unknown() {
        assert!(Tier::parse("garbage").is_none());
        assert_eq!(Tier::parse("raw"), Some(Tier::Raw));
        assert_eq!(Tier::parse("5s"), Some(Tier::Sec5));
    }
}
