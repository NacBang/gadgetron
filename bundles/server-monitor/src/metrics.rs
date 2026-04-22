//! Timeseries ingestion path — §16 of the phase2 design doc.
//!
//! `server.stats` runs on the polling hot path (see `gadgets.rs::call_stats`).
//! After it returns a fresh `ServerStats`, the handler turns the snapshot
//! into a batch of `MetricSample` rows and `try_send`s them over a bounded
//! mpsc. A separate tokio task (`run_metrics_writer`) drains the channel,
//! buffers until `BATCH_MAX` rows or `FLUSH_INTERVAL` ticks, then issues a
//! single `INSERT ... SELECT FROM UNNEST(...)` against `host_metrics`.
//!
//! The hot path is never blocked: on `try_send` failure the batch is
//! dropped and `samples_dropped_total` increments. The writer catches
//! up on its own schedule; an overloaded DB can only ever cost
//! telemetry, not stats availability.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::collectors::ServerStats;

/// One row destined for the `host_metrics` hypertable.
///
/// `labels` MUST be a JSON object whose keys are in the §4.5 allowlist
/// (`source` / `gpu_index` / `gpu_name` / `iface_kind` / `chip` /
/// `mount`). The DB also enforces this via the CHECK constraint — the
/// app-side guard exists so a mistaken key fails loud in a unit test
/// rather than at the `INSERT` boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub tenant_id: Uuid,
    pub host_id: Uuid,
    pub ts: DateTime<Utc>,
    pub metric: String,
    pub value: f64,
    pub unit: Option<String>,
    pub labels: Value,
}

/// Batch trigger sizing. 500 rows per INSERT maps to ~2 ms on the
/// TimescaleDB substrate; pushing higher marginally improves throughput
/// but delays the first graph refresh after a restart. 500 + 500 ms
/// flush is the balanced default.
pub const BATCH_MAX: usize = 500;
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Shared counter surface — readable from `server.info` or a future
/// `/metrics/ingestion/status` endpoint. Atomic so the hot path doesn't
/// block on a mutex for every drop.
#[derive(Debug, Clone, Default)]
pub struct IngestionCounters {
    pub samples_enqueued: Arc<AtomicU64>,
    pub samples_dropped: Arc<AtomicU64>,
    pub batches_flushed: Arc<AtomicU64>,
    pub batch_errors: Arc<AtomicU64>,
}

impl IngestionCounters {
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.samples_enqueued.load(Ordering::Relaxed),
            self.samples_dropped.load(Ordering::Relaxed),
            self.batches_flushed.load(Ordering::Relaxed),
            self.batch_errors.load(Ordering::Relaxed),
        )
    }
}

/// Hot-path helper: try to ship a batch into the writer without
/// blocking. Drops on full and bumps `samples_dropped`. Safe to call
/// even when `sender` is `None` — no-op.
pub fn try_ship(
    sender: Option<&mpsc::Sender<Vec<MetricSample>>>,
    counters: &IngestionCounters,
    samples: Vec<MetricSample>,
) {
    let Some(sender) = sender else {
        return;
    };
    let count = samples.len() as u64;
    match sender.try_send(samples) {
        Ok(()) => {
            counters.samples_enqueued.fetch_add(count, Ordering::Relaxed);
        }
        Err(_) => {
            counters.samples_dropped.fetch_add(count, Ordering::Relaxed);
            tracing::warn!(
                target: "server_monitor_metrics",
                dropped = count,
                "metrics sample batch dropped — ingestion channel full"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ServerStats → Vec<MetricSample>
// ---------------------------------------------------------------------------

/// Fan out a single `ServerStats` snapshot into one row per metric.
///
/// Expected fan-out for a CPU+RAM+2-GPU+3-NIC+8-temp host ≈ 40 samples.
/// The metric naming convention matches §2.1.2 of the design doc —
/// hierarchical `<family>[.<instance>][.<leaf>]` strings Penny can
/// pattern-match against natural-language questions.
pub fn stats_to_samples(
    tenant_id: Uuid,
    host_id: Uuid,
    stats: &ServerStats,
) -> Vec<MetricSample> {
    let ts = stats.fetched_at;
    let mut out: Vec<MetricSample> = Vec::with_capacity(48);

    let mk = |metric: String, value: f64, unit: Option<&str>, labels: Value| MetricSample {
        tenant_id,
        host_id,
        ts,
        metric,
        value,
        unit: unit.map(String::from),
        labels,
    };

    // --- CPU ---
    if let Some(cpu) = &stats.cpu {
        out.push(mk("cpu.util".into(), cpu.util_pct as f64, Some("pct"), json!({})));
        out.push(mk("cpu.load_1m".into(), cpu.load_1m as f64, None, json!({})));
        out.push(mk("cpu.load_5m".into(), cpu.load_5m as f64, None, json!({})));
        out.push(mk(
            "cpu.cores".into(),
            cpu.cores as f64,
            None,
            json!({}),
        ));
    }

    // --- Memory ---
    if let Some(mem) = &stats.mem {
        out.push(mk(
            "mem.used_bytes".into(),
            mem.used_bytes as f64,
            Some("bytes"),
            json!({}),
        ));
        out.push(mk(
            "mem.available_bytes".into(),
            mem.available_bytes as f64,
            Some("bytes"),
            json!({}),
        ));
        out.push(mk(
            "mem.total_bytes".into(),
            mem.total_bytes as f64,
            Some("bytes"),
            json!({}),
        ));
        if mem.swap_total_bytes > 0 {
            out.push(mk(
                "mem.swap_used_bytes".into(),
                mem.swap_used_bytes as f64,
                Some("bytes"),
                json!({}),
            ));
        }
    }

    // --- Disks ---
    for disk in &stats.disks {
        let labels = json!({ "mount": disk.mount });
        out.push(mk(
            format!("disk.{}.used_bytes", disk.mount),
            disk.used_bytes as f64,
            Some("bytes"),
            labels.clone(),
        ));
        out.push(mk(
            format!("disk.{}.total_bytes", disk.mount),
            disk.total_bytes as f64,
            Some("bytes"),
            labels,
        ));
    }

    // --- Temperature sensors ---
    for temp in &stats.temps {
        out.push(mk(
            format!("temp.{}.{}", temp.chip, temp.label),
            temp.celsius as f64,
            Some("celsius"),
            json!({ "chip": temp.chip }),
        ));
    }

    // --- GPUs ---
    for gpu in &stats.gpus {
        let labels = json!({
            "gpu_index": gpu.index,
            "gpu_name":  gpu.name,
            "source":    gpu.source,
        });
        if let Some(util) = gpu.util_pct {
            out.push(mk(
                format!("gpu.{}.util", gpu.index),
                util as f64,
                Some("pct"),
                labels.clone(),
            ));
        }
        if let Some(mem_used) = gpu.mem_used_mib {
            out.push(mk(
                format!("gpu.{}.mem_used_mib", gpu.index),
                mem_used as f64,
                Some("mib"),
                labels.clone(),
            ));
        }
        if let Some(mem_total) = gpu.mem_total_mib {
            out.push(mk(
                format!("gpu.{}.mem_total_mib", gpu.index),
                mem_total as f64,
                Some("mib"),
                labels.clone(),
            ));
        }
        if let Some(temp) = gpu.temp_c {
            out.push(mk(
                format!("gpu.{}.temp", gpu.index),
                temp as f64,
                Some("celsius"),
                labels.clone(),
            ));
        }
        if let Some(power) = gpu.power_w {
            out.push(mk(
                format!("gpu.{}.power_w", gpu.index),
                power as f64,
                Some("watts"),
                labels.clone(),
            ));
        }
        // DCGM-only telemetry. Stored as timeseries so operators can
        // see the event moment in the chart (e.g. a stepped ECC
        // counter, a throttle bitmask that flipped non-zero during a
        // benchmark run).
        if let Some(mt) = gpu.mem_temp_c {
            out.push(mk(
                format!("gpu.{}.mem_temp", gpu.index),
                mt as f64,
                Some("celsius"),
                labels.clone(),
            ));
        }
        if let Some(dbe) = gpu.ecc_dbe_total {
            out.push(mk(
                format!("gpu.{}.ecc_dbe", gpu.index),
                dbe as f64,
                Some("count"),
                labels.clone(),
            ));
        }
        if let Some(xid) = gpu.xid_last {
            out.push(mk(
                format!("gpu.{}.xid", gpu.index),
                xid as f64,
                Some("code"),
                labels.clone(),
            ));
        }
        if let Some(thr) = gpu.throttle_reasons {
            out.push(mk(
                format!("gpu.{}.throttle_bits", gpu.index),
                thr as f64,
                Some("bitmask"),
                labels,
            ));
        }
    }

    // --- NICs ---
    for net in &stats.network {
        // v0.2 doesn't probe for wifi/ib kind — default to ethernet.
        // Future: parse /sys/class/net/<iface>/type.
        let labels = json!({ "iface_kind": "ethernet" });
        out.push(mk(
            format!("nic.{}.rx_bps", net.iface),
            net.rx_bps,
            Some("bytes_per_sec"),
            labels.clone(),
        ));
        out.push(mk(
            format!("nic.{}.tx_bps", net.iface),
            net.tx_bps,
            Some("bytes_per_sec"),
            labels.clone(),
        ));
        out.push(mk(
            format!("nic.{}.rx_bytes_total", net.iface),
            net.rx_bytes_total as f64,
            Some("bytes"),
            labels.clone(),
        ));
        out.push(mk(
            format!("nic.{}.tx_bytes_total", net.iface),
            net.tx_bytes_total as f64,
            Some("bytes"),
            labels,
        ));
    }

    // --- Power ---
    if let Some(power) = &stats.power {
        if let Some(psu) = power.psu_watts {
            out.push(mk(
                "power.psu_watts".into(),
                psu as f64,
                Some("watts"),
                json!({ "source": "ipmitool" }),
            ));
        }
        if let Some(gpu_w) = power.gpu_watts {
            out.push(mk(
                "power.gpu_watts".into(),
                gpu_w as f64,
                Some("watts"),
                json!({ "source": "nvidia-smi" }),
            ));
        }
    }

    // --- Uptime ---
    if let Some(uptime) = stats.uptime_secs {
        out.push(mk(
            "host.uptime_secs".into(),
            uptime as f64,
            Some("seconds"),
            json!({}),
        ));
    }

    out
}

// ---------------------------------------------------------------------------
// Background writer task
// ---------------------------------------------------------------------------

/// Spawn this with `tokio::spawn`. Returns when the channel closes
/// (i.e. every sender dropped) — graceful shutdown path on gadgetron
/// stop. Any remaining buffered samples are flushed first.
pub async fn run_metrics_writer(
    mut rx: mpsc::Receiver<Vec<MetricSample>>,
    pool: PgPool,
    counters: IngestionCounters,
) {
    let mut buf: Vec<MetricSample> = Vec::with_capacity(BATCH_MAX * 2);
    let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
    // `interval()` fires immediately on first tick — drop the zeroth
    // so the first flush happens after FLUSH_INTERVAL, not at t=0.
    ticker.tick().await;

    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Some(batch) => {
                        buf.extend(batch);
                        if buf.len() >= BATCH_MAX {
                            flush(&pool, &mut buf, &counters).await;
                        }
                    }
                    None => break, // senders gone → drain and exit
                }
            }
            _ = ticker.tick() => {
                if !buf.is_empty() {
                    flush(&pool, &mut buf, &counters).await;
                }
            }
        }
    }
    if !buf.is_empty() {
        flush(&pool, &mut buf, &counters).await;
    }
    tracing::info!(
        target: "server_monitor_metrics",
        "metrics writer exiting — channel closed"
    );
}

async fn flush(pool: &PgPool, buf: &mut Vec<MetricSample>, counters: &IngestionCounters) {
    if buf.is_empty() {
        return;
    }
    let batch: Vec<MetricSample> = std::mem::take(buf);
    let n = batch.len();

    // Split into parallel column arrays for the UNNEST-based INSERT —
    // one network round-trip for up to BATCH_MAX rows.
    let mut tenant_ids: Vec<Uuid> = Vec::with_capacity(n);
    let mut host_ids: Vec<Uuid> = Vec::with_capacity(n);
    let mut timestamps: Vec<DateTime<Utc>> = Vec::with_capacity(n);
    let mut metrics: Vec<String> = Vec::with_capacity(n);
    let mut values: Vec<f64> = Vec::with_capacity(n);
    let mut units: Vec<Option<String>> = Vec::with_capacity(n);
    let mut labels: Vec<Value> = Vec::with_capacity(n);
    for s in batch {
        tenant_ids.push(s.tenant_id);
        host_ids.push(s.host_id);
        timestamps.push(s.ts);
        metrics.push(s.metric);
        values.push(s.value);
        units.push(s.unit);
        labels.push(s.labels);
    }

    let res = sqlx::query(
        r#"
        INSERT INTO host_metrics (tenant_id, host_id, ts, metric, value, unit, labels)
        SELECT * FROM UNNEST(
            $1::uuid[],
            $2::uuid[],
            $3::timestamptz[],
            $4::text[],
            $5::float8[],
            $6::text[],
            $7::jsonb[]
        )
        "#,
    )
    .bind(&tenant_ids)
    .bind(&host_ids)
    .bind(&timestamps)
    .bind(&metrics)
    .bind(&values)
    .bind(&units)
    .bind(&labels)
    .execute(pool)
    .await;

    match res {
        Ok(_) => {
            counters.batches_flushed.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                target: "server_monitor_metrics",
                rows = n,
                "metrics batch flushed"
            );
        }
        Err(e) => {
            counters.batch_errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                target: "server_monitor_metrics",
                error = %e,
                rows = n,
                "metrics batch insert failed (samples dropped)"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests (schema-level INSERT tests live in the e2e harness against
// a real TimescaleDB instance — see task #25).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::{
        CpuStats, DiskStats, GpuStats, MemStats, NetworkStats, PowerStats, ServerStats, TempReading,
    };

    fn fixture() -> ServerStats {
        ServerStats {
            cpu: Some(CpuStats { util_pct: 2.5, load_1m: 1.0, load_5m: 0.8, cores: 48 }),
            mem: Some(MemStats {
                total_bytes: 128 * 1024_u64.pow(3),
                used_bytes: 16 * 1024_u64.pow(3),
                available_bytes: 110 * 1024_u64.pow(3),
                swap_used_bytes: 0,
                swap_total_bytes: 2 * 1024_u64.pow(3),
            }),
            disks: vec![DiskStats {
                mount: "/".into(),
                fs: "nvme0n1 (ext4)".into(),
                total_bytes: 500_000_000_000,
                used_bytes: 100_000_000_000,
            }],
            temps: vec![TempReading {
                chip: "k10temp-pci-00c3".into(),
                label: "Tctl".into(),
                celsius: 42.0,
            }],
            gpus: vec![GpuStats {
                index: 0,
                name: "A100".into(),
                util_pct: Some(25.0),
                mem_used_mib: Some(12000),
                mem_total_mib: Some(81920),
                temp_c: Some(55.0),
                power_w: Some(150.0),
                power_limit_w: Some(300.0),
                source: "nvidia-smi",
            }],
            power: Some(PowerStats { psu_watts: None, gpu_watts: Some(150.0) }),
            network: vec![NetworkStats {
                iface: "eth0".into(),
                rx_bps: 1_000_000.0,
                tx_bps: 500_000.0,
                rx_bytes_total: 1_000_000_000,
                tx_bytes_total: 500_000_000,
            }],
            uptime_secs: Some(3600),
            fetched_at: Utc::now(),
            warnings: vec![],
        }
    }

    #[test]
    fn stats_to_samples_covers_all_families() {
        let tenant = Uuid::new_v4();
        let host = Uuid::new_v4();
        let samples = stats_to_samples(tenant, host, &fixture());

        let names: Vec<&str> = samples.iter().map(|s| s.metric.as_str()).collect();

        // CPU family
        assert!(names.contains(&"cpu.util"));
        assert!(names.contains(&"cpu.load_1m"));
        // RAM
        assert!(names.contains(&"mem.used_bytes"));
        assert!(names.contains(&"mem.swap_used_bytes"));
        // Disk with label
        assert!(names.contains(&"disk./.used_bytes"));
        // Temp
        assert!(names.contains(&"temp.k10temp-pci-00c3.Tctl"));
        // GPU
        assert!(names.contains(&"gpu.0.util"));
        assert!(names.contains(&"gpu.0.power_w"));
        // NIC
        assert!(names.contains(&"nic.eth0.rx_bps"));
        assert!(names.contains(&"nic.eth0.tx_bytes_total"));
        // Power
        assert!(names.contains(&"power.gpu_watts"));
        // Uptime
        assert!(names.contains(&"host.uptime_secs"));

        // Tenant + host propagation — every sample carries them.
        for s in &samples {
            assert_eq!(s.tenant_id, tenant);
            assert_eq!(s.host_id, host);
        }
    }

    #[test]
    fn stats_to_samples_labels_stay_in_allowlist() {
        let samples = stats_to_samples(Uuid::nil(), Uuid::nil(), &fixture());
        // The §4.5 allowlist — any key outside this set must not appear.
        const ALLOWED: &[&str] = &[
            "source", "gpu_index", "gpu_name", "iface_kind", "chip", "mount",
        ];
        for s in &samples {
            if let Value::Object(m) = &s.labels {
                for key in m.keys() {
                    assert!(
                        ALLOWED.contains(&key.as_str()),
                        "label key {key:?} not in allowlist for metric {}",
                        s.metric,
                    );
                }
            }
        }
    }

    #[test]
    fn try_ship_increments_dropped_when_channel_full() {
        let (tx, _rx) = mpsc::channel(1);
        let counters = IngestionCounters::default();
        // Fill the single slot.
        tx.try_send(vec![]).unwrap();
        try_ship(Some(&tx), &counters, vec![
            MetricSample {
                tenant_id: Uuid::nil(),
                host_id: Uuid::nil(),
                ts: Utc::now(),
                metric: "x".into(),
                value: 1.0,
                unit: None,
                labels: Value::Null,
            },
        ]);
        assert_eq!(counters.samples_dropped.load(Ordering::Relaxed), 1);
        assert_eq!(counters.samples_enqueued.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn try_ship_noop_without_sender() {
        let counters = IngestionCounters::default();
        try_ship(None, &counters, vec![]);
        assert_eq!(counters.samples_enqueued.load(Ordering::Relaxed), 0);
        assert_eq!(counters.samples_dropped.load(Ordering::Relaxed), 0);
    }
}
