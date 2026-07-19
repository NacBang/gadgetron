use std::collections::BTreeMap;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use gadgetron_bundle_sdk::{
    DatabaseOrderDirection, DatabaseSelectRequest, GadgetResult, HostResponse,
    InvocationLeaseToken, LocalId,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    alerts::valid_metric_pattern,
    host_error,
    operational::{id, select, table, SharedBroker, READ_PERMISSION},
    telemetry::metric_samples,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetricSeriesInput {
    target_id: String,
    metric: String,
    #[serde(default = "empty_labels")]
    labels: Value,
    #[serde(default)]
    range: MetricRange,
    #[serde(default)]
    interval: MetricInterval,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
enum MetricRange {
    #[serde(rename = "5m")]
    Minutes5,
    #[serde(rename = "30m")]
    Minutes30,
    #[default]
    #[serde(rename = "24h")]
    Hours24,
    #[serde(rename = "7d")]
    Days7,
    #[serde(rename = "30d")]
    Days30,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
enum MetricInterval {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "5s")]
    Seconds5,
    #[serde(rename = "1m")]
    Minute1,
    #[serde(rename = "5m")]
    Minutes5,
    #[serde(rename = "15m")]
    Minutes15,
    #[serde(rename = "1h")]
    Hour1,
    #[serde(rename = "6h")]
    Hours6,
    #[serde(rename = "1d")]
    Day1,
}

impl MetricRange {
    fn label(self) -> &'static str {
        match self {
            Self::Minutes5 => "5m",
            Self::Minutes30 => "30m",
            Self::Hours24 => "24h",
            Self::Days7 => "7d",
            Self::Days30 => "30d",
        }
    }

    fn duration(self) -> Duration {
        match self {
            Self::Minutes5 => Duration::minutes(5),
            Self::Minutes30 => Duration::minutes(30),
            Self::Hours24 => Duration::hours(24),
            Self::Days7 => Duration::days(7),
            Self::Days30 => Duration::days(30),
        }
    }

    fn resolve(self, requested: MetricInterval) -> Option<MetricInterval> {
        match (self, requested) {
            (Self::Minutes5, MetricInterval::Auto) => Some(MetricInterval::Seconds5),
            (Self::Minutes30, MetricInterval::Auto) => Some(MetricInterval::Minutes5),
            (Self::Hours24, MetricInterval::Auto) => Some(MetricInterval::Minutes15),
            (Self::Days7, MetricInterval::Auto) => Some(MetricInterval::Hour1),
            (Self::Days30, MetricInterval::Auto) => Some(MetricInterval::Hours6),
            (Self::Minutes5, MetricInterval::Seconds5 | MetricInterval::Minute1)
            | (Self::Minutes30, MetricInterval::Minute1 | MetricInterval::Minutes5)
            | (
                Self::Hours24,
                MetricInterval::Minutes5 | MetricInterval::Minutes15 | MetricInterval::Hour1,
            )
            | (Self::Days7, MetricInterval::Hour1 | MetricInterval::Hours6)
            | (Self::Days30, MetricInterval::Hours6 | MetricInterval::Day1) => Some(requested),
            _ => None,
        }
    }
}

impl MetricInterval {
    fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Seconds5 => "5s",
            Self::Minute1 => "1m",
            Self::Minutes5 => "5m",
            Self::Minutes15 => "15m",
            Self::Hour1 => "1h",
            Self::Hours6 => "6h",
            Self::Day1 => "1d",
        }
    }

    fn seconds(self) -> i64 {
        match self {
            Self::Auto => 0,
            Self::Seconds5 => 5,
            Self::Minute1 => 60,
            Self::Minutes5 => 300,
            Self::Minutes15 => 900,
            Self::Hour1 => 3_600,
            Self::Hours6 => 21_600,
            Self::Day1 => 86_400,
        }
    }

    fn source(self) -> Self {
        match self {
            Self::Minutes15 => Self::Minutes5,
            Self::Day1 => Self::Hours6,
            interval => interval,
        }
    }

    fn relation(self) -> &'static str {
        match self.source() {
            Self::Seconds5 => "server_metric_history_5s",
            Self::Minute1 => "server_metric_history_1m",
            Self::Minutes5 => "server_metric_history_5m",
            Self::Hour1 => "server_metric_history_1h",
            Self::Hours6 => "server_metric_history_6h",
            Self::Auto | Self::Minutes15 | Self::Day1 => {
                unreachable!("source interval resolves to a persisted tier")
            }
        }
    }
}

#[derive(Clone, Debug)]
struct SeriesPoint {
    timestamp: DateTime<Utc>,
    average: f64,
    minimum: f64,
    maximum: f64,
    samples: u64,
}

pub(crate) async fn catalog(lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let health = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            ["target_id".into(), "host_id".into(), "status".into()],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let stats = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("host_stats_latest"),
            ["host_id".into(), "stats".into(), "fetched_at".into()],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let source_truncated = health.truncated || stats.truncated;
    let stats_by_host: BTreeMap<&str, &BTreeMap<String, Value>> = stats
        .rows
        .iter()
        .filter_map(|row| Some((row.get("host_id")?.as_str()?, row)))
        .collect();
    let mut rows = Vec::new();
    for target in &health.rows {
        let Some(host_id) = target.get("host_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(snapshot) = stats_by_host.get(host_id) else {
            continue;
        };
        let Some(stats) = snapshot.get("stats") else {
            continue;
        };
        rows.extend(overview_rows(
            target.get("target_id").cloned().unwrap_or(Value::Null),
            target.get("status").cloned().unwrap_or(Value::Null),
            json!(host_id),
            stats,
            snapshot.get("fetched_at").cloned().unwrap_or(Value::Null),
        ));
    }
    rows.sort_by(|left, right| {
        left["target_id"]
            .as_str()
            .cmp(&right["target_id"].as_str())
            .then_with(|| left["metric"].as_str().cmp(&right["metric"].as_str()))
    });
    let truncated = source_truncated || rows.len() > 500;
    rows.truncate(500);
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "count": rows.len(),
        "rows": rows,
        "truncated": truncated,
    })))
}

pub(crate) fn live_overview(
    target_id: &str,
    stats: &Value,
    observed_at: &str,
    duration_ms: u64,
) -> Value {
    let warnings = stats
        .get("warnings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rows = overview_rows(
        json!(target_id),
        json!("live"),
        Value::Null,
        stats,
        json!(observed_at),
    );
    json!({
        "target_id": target_id,
        "mode": "live",
        "observed_at": observed_at,
        "duration_ms": duration_ms,
        "collector_warnings": warnings,
        "count": rows.len(),
        "rows": rows,
        "truncated": false,
    })
}

fn overview_rows(
    target_id: Value,
    status: Value,
    host_id: Value,
    stats: &Value,
    observed_at: Value,
) -> Vec<Value> {
    metric_samples(stats)
        .into_iter()
        .map(|sample| {
            let presentation = metric_presentation(&sample.metric, &sample.labels);
            json!({
                "status": status,
                "target_id": target_id,
                "target_label": stats.get("hostname").cloned().unwrap_or(Value::Null),
                "host_id": host_id,
                "metric": sample.metric,
                "latest": sample.value,
                "unit": sample.unit,
                "observed_at": observed_at,
                "labels": sample.labels,
                "presentation": presentation,
            })
        })
        .collect()
}

fn metric_presentation(metric: &str, labels: &Value) -> Option<Value> {
    let (label, group, visual, minimum, maximum) = if metric == "cpu.util" {
        ("CPU utilization".into(), "Compute", "bar", 0.0, Some(100.0))
    } else if metric == "cpu.load_1m" {
        (
            "Load average · 1 min".into(),
            "Compute",
            "number",
            0.0,
            None,
        )
    } else if metric == "mem.used_percent" {
        ("Memory used".into(), "Memory", "bar", 0.0, Some(100.0))
    } else if metric == "mem.available_bytes" {
        ("Memory available".into(), "Memory", "number", 0.0, None)
    } else if metric == "mem.swap_used_bytes" {
        ("Swap used".into(), "Memory", "number", 0.0, None)
    } else if metric == "disk.used_percent" && is_human_disk(labels) {
        ("Disk used".into(), "Storage", "bar", 0.0, Some(100.0))
    } else if metric == "temp.celsius" {
        ("Temperature".into(), "Thermal", "gauge", 0.0, Some(100.0))
    } else if metric == "uptime_seconds" {
        ("Uptime".into(), "System", "number", 0.0, None)
    } else if metric.starts_with("nic.") && metric.ends_with("_bps") {
        let direction = if metric.ends_with("rx_bps") {
            "Receive"
        } else {
            "Transmit"
        };
        (
            format!("Network {direction}"),
            "Network",
            "number",
            0.0,
            None,
        )
    } else if metric == "power.psu_watts" {
        ("Server power".into(), "Power", "number", 0.0, None)
    } else if metric == "power.gpu_watts" {
        ("GPU power total".into(), "Power", "number", 0.0, None)
    } else if let Some(rest) = metric.strip_prefix("gpu.") {
        let mut parts = rest.split('.');
        let index = parts.next().unwrap_or("?");
        match parts.next().unwrap_or("") {
            "util" => (
                format!("GPU {index} utilization"),
                "Accelerators",
                "bar",
                0.0,
                Some(100.0),
            ),
            "mem_used_percent" => (
                format!("GPU {index} memory used"),
                "Accelerators",
                "bar",
                0.0,
                Some(100.0),
            ),
            "temp" => (
                format!("GPU {index} temperature"),
                "Accelerators",
                "gauge",
                0.0,
                Some(110.0),
            ),
            "mem_temp" => (
                format!("GPU {index} memory temperature"),
                "Accelerators",
                "gauge",
                0.0,
                Some(110.0),
            ),
            "power_w" => (
                format!("GPU {index} power"),
                "Accelerators",
                "gauge",
                0.0,
                labels.get("power_limit_w").and_then(Value::as_f64),
            ),
            "mem_total_mib" => (
                format!("GPU {index} memory capacity"),
                "Accelerators",
                "number",
                0.0,
                None,
            ),
            "ecc_dbe" => (
                format!("GPU {index} uncorrected ECC"),
                "Accelerators",
                "number",
                0.0,
                None,
            ),
            "xid" => (
                format!("GPU {index} last XID"),
                "Accelerators",
                "number",
                0.0,
                None,
            ),
            "throttle_bits" => (
                format!("GPU {index} throttle flags"),
                "Accelerators",
                "number",
                0.0,
                None,
            ),
            _ => return None,
        }
    } else {
        return None;
    };
    Some(json!({
        "label": label,
        "group": group,
        "visual": visual,
        "min": minimum,
        "max": maximum,
    }))
}

fn is_human_disk(labels: &Value) -> bool {
    let mount = labels.get("mount").and_then(Value::as_str).unwrap_or("");
    !mount.is_empty()
        && !mount.starts_with("/snap/")
        && !mount.starts_with("/mnt/wsl")
        && !mount.starts_with("/usr/lib/wsl")
        && mount != "/init"
}

pub(crate) async fn series(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: MetricSeriesInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => {
            return host_error(
                "invalid-arguments",
                "target_id, exact metric, supported range and interval are required",
            )
        }
    };
    let target = match LocalId::new(input.target_id) {
        Ok(target) => target,
        Err(_) => return host_error("invalid-arguments", "target_id is not canonical"),
    };
    if input.metric.contains('*') || !valid_metric_pattern(&input.metric) {
        return host_error(
            "invalid-arguments",
            "metric must be an exact dot-separated metric name",
        );
    }
    if !input.labels.is_object()
        || serde_json::to_vec(&input.labels).map_or(true, |encoded| encoded.len() > 2_048)
    {
        return host_error(
            "invalid-arguments",
            "labels must be a bounded exact metric-series object",
        );
    }
    let Some(effective_interval) = input.range.resolve(input.interval) else {
        return host_error(
            "invalid-arguments",
            "interval is not supported for the requested history range",
        );
    };
    let target_row = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            ["host_id".into(), "status".into()],
        )
        .with_filter("target_id", json!(target.as_str()))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let Some(target_row) = target_row else {
        return host_error(
            "target-not-monitored",
            "target has no active monitoring state",
        );
    };
    let Some(host_id) = target_row.get("host_id").and_then(Value::as_str) else {
        return host_error("metric-state-invalid", "target host identity is invalid");
    };
    let source_interval = effective_interval.source();
    let expected_source_points =
        (input.range.duration().num_seconds() / source_interval.seconds() + 4).clamp(10, 500);
    let selected = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table(source_interval.relation()),
            [
                "bucket".into(),
                "avg".into(),
                "min".into(),
                "max".into(),
                "samples".into(),
                "unit".into(),
            ],
        )
        .with_filter("host_id", json!(host_id))
        .with_filter("metric", json!(input.metric))
        .with_filter("labels", input.labels.clone())
        .with_order("bucket", DatabaseOrderDirection::Descending)
        .with_limit(expected_source_points as u32),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let requested_to = Utc::now();
    let requested_from = requested_to - input.range.duration();
    let mut unit = selected
        .rows
        .iter()
        .find_map(|row| row.get("unit"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut source_tier = source_interval.label();
    let mut source_truncated = selected.truncated;
    let mut source_points = selected
        .rows
        .iter()
        .filter_map(parse_series_point)
        .filter(|point| point.timestamp >= requested_from && point.timestamp < requested_to)
        .collect::<Vec<_>>();
    if source_points.is_empty() && input.range == MetricRange::Minutes5 {
        let raw = match select(
            &broker,
            DatabaseSelectRequest::new(
                lease,
                id(READ_PERMISSION),
                table("host_metrics"),
                ["ts".into(), "value".into(), "unit".into()],
            )
            .with_filter("host_id", json!(host_id))
            .with_filter("metric", json!(input.metric))
            .with_filter("labels", input.labels.clone())
            .with_order("ts", DatabaseOrderDirection::Descending)
            .with_limit(304),
        )
        .await
        {
            Ok(rows) => rows,
            Err(response) => return response,
        };
        unit = raw
            .rows
            .iter()
            .find_map(|row| row.get("unit"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        source_points = raw
            .rows
            .iter()
            .filter_map(parse_raw_series_point)
            .filter(|point| point.timestamp >= requested_from && point.timestamp < requested_to)
            .collect();
        source_tier = "raw";
        source_truncated = raw.truncated;
    }
    let points = rebucket(source_points, effective_interval);
    let (coverage, gaps, partial, coverage_truncated) = history_coverage(
        &points,
        requested_from,
        requested_to,
        effective_interval,
        source_truncated,
    );
    let values: Vec<f64> = points.iter().map(|point| point.average).collect();
    let minimum = values.iter().copied().reduce(f64::min);
    let maximum = values.iter().copied().reduce(f64::max);
    let latest = values.last().copied();
    let point_rows = points
        .iter()
        .map(|point| {
            json!({
                "ts": timestamp(point.timestamp),
                "value": point.average,
                "avg": point.average,
                "min": point.minimum,
                "max": point.maximum,
                "samples": point.samples,
                "source_tier": source_tier,
            })
        })
        .collect::<Vec<_>>();
    let presentation = metric_presentation(&input.metric, &input.labels);
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "target_id": target.as_str(),
        "host_id": host_id,
        "metric": input.metric,
        "labels": input.labels,
        "presentation": presentation,
        "unit": unit,
        "requested_range": input.range.label(),
        "effective_interval": effective_interval.label(),
        "requested_from": timestamp(requested_from),
        "requested_to": timestamp(requested_to),
        "coverage": coverage,
        "gaps": gaps,
        "points": point_rows,
        "summary": {
            "samples": values.len(),
            "minimum": minimum,
            "maximum": maximum,
            "latest": latest,
        },
        "partial": partial,
        "truncated": coverage_truncated,
    })))
}

fn parse_series_point(row: &BTreeMap<String, Value>) -> Option<SeriesPoint> {
    let timestamp = DateTime::parse_from_rfc3339(row.get("bucket")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let average = row.get("avg")?.as_f64()?;
    let minimum = row.get("min")?.as_f64()?;
    let maximum = row.get("max")?.as_f64()?;
    let samples = row.get("samples")?.as_u64().or_else(|| {
        row.get("samples")?
            .as_i64()
            .and_then(|value| value.try_into().ok())
    })?;
    (average.is_finite() && minimum.is_finite() && maximum.is_finite() && samples > 0).then_some(
        SeriesPoint {
            timestamp,
            average,
            minimum,
            maximum,
            samples,
        },
    )
}

fn parse_raw_series_point(row: &BTreeMap<String, Value>) -> Option<SeriesPoint> {
    let timestamp = DateTime::parse_from_rfc3339(row.get("ts")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let value = row.get("value")?.as_f64()?;
    value.is_finite().then_some(SeriesPoint {
        timestamp,
        average: value,
        minimum: value,
        maximum: value,
        samples: 1,
    })
}

fn rebucket(points: Vec<SeriesPoint>, interval: MetricInterval) -> Vec<SeriesPoint> {
    #[derive(Default)]
    struct Bucket {
        weighted_sum: f64,
        samples: u64,
        minimum: f64,
        maximum: f64,
    }

    let seconds = interval.seconds();
    let mut buckets = BTreeMap::<i64, Bucket>::new();
    for point in points {
        let bucket_at = point.timestamp.timestamp().div_euclid(seconds) * seconds;
        let bucket = buckets.entry(bucket_at).or_insert_with(|| Bucket {
            minimum: f64::INFINITY,
            maximum: f64::NEG_INFINITY,
            ..Bucket::default()
        });
        bucket.weighted_sum += point.average * point.samples as f64;
        bucket.samples = bucket.samples.saturating_add(point.samples);
        bucket.minimum = bucket.minimum.min(point.minimum);
        bucket.maximum = bucket.maximum.max(point.maximum);
    }
    buckets
        .into_iter()
        .filter_map(|(seconds, bucket)| {
            let timestamp = DateTime::from_timestamp(seconds, 0)?;
            (bucket.samples > 0).then_some(SeriesPoint {
                timestamp,
                average: bucket.weighted_sum / bucket.samples as f64,
                minimum: bucket.minimum,
                maximum: bucket.maximum,
                samples: bucket.samples,
            })
        })
        .collect()
}

fn history_coverage(
    points: &[SeriesPoint],
    requested_from: DateTime<Utc>,
    requested_to: DateTime<Utc>,
    interval: MetricInterval,
    source_truncated: bool,
) -> (Value, Vec<Value>, bool, bool) {
    let step = Duration::seconds(interval.seconds());
    let mut gaps = Vec::new();
    for pair in points.windows(2) {
        if pair[1].timestamp - pair[0].timestamp > step + step / 2 {
            gaps.push(json!({
                "from": timestamp(pair[0].timestamp + step),
                "to": timestamp(pair[1].timestamp),
            }));
        }
    }
    let start = points.first().map(|point| point.timestamp);
    let end = points.last().map(|point| point.timestamp + step);
    let starts_in_range = start.is_some_and(|value| value <= requested_from + step);
    let reaches_present = end.is_some_and(|value| value >= requested_to - step);
    let partial = points.is_empty() || !starts_in_range || !reaches_present || !gaps.is_empty();
    let truncated = source_truncated && !starts_in_range;
    (
        json!({
            "start": start.map(timestamp),
            "end": end.map(timestamp),
            "complete": !partial,
        }),
        gaps,
        partial,
        truncated,
    )
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn empty_labels() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_keeps_human_meaning_in_the_bundle() {
        assert_eq!(
            metric_presentation("cpu.util", &json!({})),
            Some(
                json!({"label":"CPU utilization","group":"Compute","visual":"bar","min":0.0,"max":100.0})
            )
        );
        assert_eq!(
            metric_presentation("temp.celsius", &json!({})).unwrap()["visual"],
            "gauge"
        );
        assert_eq!(
            metric_presentation("gpu.0.power_w", &json!({"power_limit_w":250.0})).unwrap()["max"],
            250.0
        );
        assert_eq!(
            metric_presentation("nic.eth0.rx_bytes_total", &json!({})),
            None
        );
        assert!(metric_presentation("disk.used_percent", &json!({"mount":"/"})).is_some());
        assert_eq!(
            metric_presentation("disk.used_percent", &json!({"mount":"/snap/core/1"})),
            None
        );
    }

    #[test]
    fn live_overview_is_human_shaped_and_does_not_claim_persistence() {
        let payload = live_overview(
            "edge-one",
            &json!({
                "hostname":"compute-01",
                "uptime_seconds":3600,
                "cpu":{"util_percent":42.0,"load_1m":0.5},
                "memory":{"used_percent":50.0,"available_bytes":1024,"swap_used_bytes":0},
                "disks":[],"temperatures":[],"gpus":[],"network":[],
                "power":{"psu_watts":null,"gpu_watts":null},
                "warnings":["lm-sensors is not available"]
            }),
            "2026-07-13T00:00:00Z",
            321,
        );
        assert_eq!(payload["mode"], "live");
        assert_eq!(payload["target_id"], "edge-one");
        assert_eq!(payload["rows"][0]["status"], "live");
        assert_eq!(payload["collector_warnings"].as_array().unwrap().len(), 1);
        assert_eq!(payload["rows"][0]["target_label"], "compute-01");
        assert!(payload.get("persisted").is_none());
    }

    #[test]
    fn history_ranges_resolve_only_their_supported_intervals() {
        assert_eq!(
            MetricRange::Minutes5.resolve(MetricInterval::Auto),
            Some(MetricInterval::Seconds5)
        );
        assert_eq!(
            MetricRange::Hours24.resolve(MetricInterval::Auto),
            Some(MetricInterval::Minutes15)
        );
        assert_eq!(
            MetricRange::Days30.resolve(MetricInterval::Day1),
            Some(MetricInterval::Day1)
        );
        assert_eq!(MetricRange::Days30.resolve(MetricInterval::Minute1), None);
        assert_eq!(
            MetricInterval::Minutes15.relation(),
            "server_metric_history_5m"
        );
        assert_eq!(MetricInterval::Day1.relation(), "server_metric_history_6h");
    }

    #[test]
    fn raw_history_points_keep_the_original_value() {
        let row = BTreeMap::from([
            ("ts".to_owned(), json!("2026-07-14T00:00:05Z")),
            ("value".to_owned(), json!(42.5)),
        ]);
        let point = parse_raw_series_point(&row).unwrap();
        assert_eq!(point.average, 42.5);
        assert_eq!(point.minimum, 42.5);
        assert_eq!(point.maximum, 42.5);
        assert_eq!(point.samples, 1);
    }

    #[test]
    fn weighted_rebucket_preserves_extremes_and_reports_real_gaps() {
        let at = |value: &str| {
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc)
        };
        let points = vec![
            SeriesPoint {
                timestamp: at("2026-07-14T00:00:00Z"),
                average: 10.0,
                minimum: 8.0,
                maximum: 12.0,
                samples: 1,
            },
            SeriesPoint {
                timestamp: at("2026-07-14T00:05:00Z"),
                average: 20.0,
                minimum: 16.0,
                maximum: 24.0,
                samples: 3,
            },
            SeriesPoint {
                timestamp: at("2026-07-14T00:30:00Z"),
                average: 30.0,
                minimum: 29.0,
                maximum: 31.0,
                samples: 2,
            },
        ];
        let buckets = rebucket(points, MetricInterval::Minutes15);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].average, 17.5);
        assert_eq!(buckets[0].minimum, 8.0);
        assert_eq!(buckets[0].maximum, 24.0);
        assert_eq!(buckets[0].samples, 4);

        let (coverage, gaps, partial, truncated) = history_coverage(
            &buckets,
            at("2026-07-14T00:00:00Z"),
            at("2026-07-14T00:45:00Z"),
            MetricInterval::Minutes15,
            false,
        );
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["from"], "2026-07-14T00:15:00Z");
        assert_eq!(gaps[0]["to"], "2026-07-14T00:30:00Z");
        assert_eq!(coverage["complete"], false);
        assert!(partial);
        assert!(!truncated);
    }
}
