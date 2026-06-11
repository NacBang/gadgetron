//! Server-side background poller.
//!
//! Without this, host telemetry is collected ONLY when the /web/servers
//! page is open — any gap (tab closed, browser minimized) leaves a
//! hole in `host_metrics`. Running the same `collect_stats` loop as a
//! background tokio task keeps the timeseries continuous regardless
//! of UI state. Per-host work is parallelized via `JoinSet` so slow
//! hosts don't starve fast ones.
//!
//! Design:
//!   - 1 Hz tick (configurable via `PollerConfig::interval`).
//!   - At each tick: read the full inventory, spawn one task per host,
//!     collect stats, ship samples to the metrics writer, update
//!     `last_ok_at`.
//!   - Inventory is re-read on every tick → host add / remove takes
//!     effect on the NEXT tick with no restart needed.
//!   - Per-host timeout bounds worst-case stall at ~8 s (SSH connect
//!     timeout + collect script).
//!   - Ticks never block on stragglers: a host whose previous collect
//!     is still in flight is skipped this tick (per-host guard), so one
//!     dead host can't degrade the whole fleet's cadence or let every
//!     `host_stats_latest` snapshot go stale (ISSUE 42).
//!
//! The UI polls the workbench action too, but since ISSUE 38 that path
//! reads the `host_stats_latest` snapshot row this poller maintains —
//! per-host SSH load stays ONE collector per tick no matter how many
//! browsers are watching.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use sqlx::PgPool;

use chrono::Utc;
use uuid::Uuid;

use crate::gadgets::collect_full_stats;
use crate::inventory::InventoryStore;
use crate::metrics::{stats_to_samples, try_ship, IngestionCounters, MetricSample};
use crate::snapshot::upsert_snapshot;
use crate::ssh::SshTarget;
use crate::topology::collect_topology;

/// Polling loop cadence + per-host deadline.
#[derive(Debug, Clone, Copy)]
pub struct PollerConfig {
    pub interval: Duration,
    pub per_host_timeout: Duration,
    /// How stale a host's `network_scanned_at` may get before the
    /// poller re-runs the (heavier, but rare) topology scan. Topology
    /// changes on cabling/VLAN work, not per-second — 5 min is plenty
    /// (design doc 20 §4.1).
    pub topology_refresh: Duration,
}

impl Default for PollerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            per_host_timeout: Duration::from_secs(8),
            topology_refresh: Duration::from_secs(300),
        }
    }
}

/// Run the background poller until the sender is dropped. Intended to
/// be spawned via `tokio::spawn` from `gadgetron serve` startup.
///
/// `inventory` is the same store the `ServerMonitorProvider` uses, so
/// both paths see the same host list + key files + `last_ok_at`.
pub async fn run_background_poller(
    inventory: Arc<InventoryStore>,
    metrics_sender: mpsc::Sender<Vec<MetricSample>>,
    counters: IngestionCounters,
    cfg: PollerConfig,
    pg_pool: Option<PgPool>,
) {
    tracing::info!(
        target: "server_monitor_poller",
        interval_secs = cfg.interval.as_secs_f64(),
        "background poller started"
    );
    let mut ticker = tokio::time::interval(cfg.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Hosts with a collect still in flight from a previous tick. Such a
    // host is skipped (no session pile-up) while everyone else keeps the
    // 1 Hz cadence. Plain std Mutex — critical sections are insert /
    // remove only, never held across an await.
    let in_flight: Arc<std::sync::Mutex<HashSet<Uuid>>> =
        Arc::new(std::sync::Mutex::new(HashSet::new()));
    let mut set: JoinSet<()> = JoinSet::new();
    loop {
        ticker.tick().await;
        // Reap finished tasks without blocking. Draining here instead
        // would cap the WHOLE fleet's cadence at the slowest host's
        // timeout (one dead host: 1 s ticks → 8 s ticks) and let every
        // `host_stats_latest` snapshot go stale at once.
        while let Some(res) = set.try_join_next() {
            if let Err(e) = res {
                if !e.is_cancelled() {
                    tracing::warn!(
                        target: "server_monitor_poller",
                        error = ?e,
                        "poll task panicked"
                    );
                }
            }
        }
        if metrics_sender.is_closed() {
            tracing::info!(
                target: "server_monitor_poller",
                "metrics channel closed — poller exiting"
            );
            break;
        }
        let hosts = match inventory.load().await {
            Ok(hs) => hs,
            Err(e) => {
                tracing::warn!(
                    target: "server_monitor_poller",
                    error = %e,
                    "inventory list failed; retrying next tick"
                );
                continue;
            }
        };
        if hosts.is_empty() {
            continue;
        }
        let known_hosts = inventory.root().join("known_hosts");
        for rec in hosts {
            if !lock_in_flight(&in_flight).insert(rec.id) {
                continue; // previous collect for this host still running
            }
            let guard = InFlightGuard {
                id: rec.id,
                set: Arc::clone(&in_flight),
            };
            let target = SshTarget {
                host: rec.host.clone(),
                user: rec.ssh_user.clone(),
                port: rec.ssh_port,
                key_path: Some(rec.key_path.clone()),
                known_hosts: known_hosts.clone(),
            };
            let inv = Arc::clone(&inventory);
            let sender = metrics_sender.clone();
            let counters = counters.clone();
            let timeout = cfg.per_host_timeout;
            let topology_refresh = cfg.topology_refresh;
            let pool = pg_pool.clone();
            set.spawn(async move {
                let _guard = guard;
                let started = std::time::Instant::now();
                // collect_full_stats also covers gadgetini (when enabled)
                // and the inventory `last_ok_at` bookkeeping, so the
                // snapshot row matches what `server.stats` used to build.
                match tokio::time::timeout(timeout, collect_full_stats(&inv, &rec, &target)).await {
                    Ok(Ok(stats)) => {
                        let samples = stats_to_samples(rec.tenant_id, rec.id, &stats);
                        try_ship(Some(&sender), &counters, samples);
                        if let Some(pool) = pool.as_ref() {
                            upsert_snapshot(pool, rec.tenant_id, rec.id, &stats).await;
                        }
                        // Low-frequency topology rescan (ISSUE 39):
                        // first tick after registration fills it in
                        // (scanned_at = None), then every
                        // `topology_refresh`. Compared in i64 so a
                        // future-dated stamp (NTP step back) reads as
                        // fresh instead of wrapping into a rescan-every-
                        // tick loop.
                        let needs_topology = rec.network_scanned_at.is_none_or(|at| {
                            Utc::now().signed_duration_since(at).num_seconds()
                                >= topology_refresh.as_secs() as i64
                        });
                        if needs_topology {
                            match tokio::time::timeout(timeout, collect_topology(&target)).await {
                                Ok(Ok(scan)) => {
                                    // `modify` holds the store lock across
                                    // read-modify-write, so concurrent
                                    // writers (alias edits, stamps) are
                                    // never rolled back.
                                    let _ = inv
                                        .modify(rec.id, move |cur| {
                                            cur.network_interfaces = scan.interfaces;
                                            cur.lldp_raw = scan.lldp_raw;
                                            cur.network_scanned_at = Some(Utc::now());
                                        })
                                        .await;
                                }
                                Ok(Err(e)) => tracing::warn!(
                                    target: "server_monitor_poller",
                                    host = %rec.host,
                                    error = %e,
                                    "topology scan failed — keeping previous interfaces"
                                ),
                                Err(_) => tracing::warn!(
                                    target: "server_monitor_poller",
                                    host = %rec.host,
                                    "topology scan timed out — keeping previous interfaces"
                                ),
                            }
                        }
                        tracing::debug!(
                            target: "server_monitor_poller",
                            host = %rec.host,
                            ms = started.elapsed().as_millis() as u64,
                            "tick ok"
                        );
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            target: "server_monitor_poller",
                            host = %rec.host,
                            error = %e,
                            "collect_stats failed"
                        );
                    }
                    Err(_) => {
                        tracing::warn!(
                            target: "server_monitor_poller",
                            host = %rec.host,
                            timeout_secs = timeout.as_secs_f64(),
                            "collect_stats timed out"
                        );
                    }
                }
            });
        }
    }
    // Shutdown: let in-progress collects finish so snapshot / inventory
    // writes aren't aborted mid-flight.
    while set.join_next().await.is_some() {}
}

fn lock_in_flight(
    set: &std::sync::Mutex<HashSet<Uuid>>,
) -> std::sync::MutexGuard<'_, HashSet<Uuid>> {
    // Poisoning only means a poll task panicked between insert and
    // remove; the set itself is still a valid HashSet.
    set.lock().unwrap_or_else(|p| p.into_inner())
}

/// Removes the host id from the in-flight set when its task ends —
/// including on panic, since Drop runs during unwind.
struct InFlightGuard {
    id: Uuid,
    set: Arc<std::sync::Mutex<HashSet<Uuid>>>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        lock_in_flight(&self.set).remove(&self.id);
    }
}
