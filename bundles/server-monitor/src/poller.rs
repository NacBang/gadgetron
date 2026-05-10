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
//!
//! The UI still polls too — it needs the live snapshot cards. With
//! OpenSSH `ControlMaster=auto` the two pollers multiplex onto a
//! single TCP/SSH session per host, so the extra load is negligible.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::collectors::collect_stats;
use crate::inventory::InventoryStore;
use crate::metrics::{stats_to_samples, try_ship, IngestionCounters, MetricSample};
use crate::ssh::SshTarget;

/// Polling loop cadence + per-host deadline.
#[derive(Debug, Clone, Copy)]
pub struct PollerConfig {
    pub interval: Duration,
    pub per_host_timeout: Duration,
}

impl Default for PollerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            per_host_timeout: Duration::from_secs(8),
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
) {
    tracing::info!(
        target: "server_monitor_poller",
        interval_secs = cfg.interval.as_secs_f64(),
        "background poller started"
    );
    let mut ticker = tokio::time::interval(cfg.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
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
        let mut set: JoinSet<()> = JoinSet::new();
        for rec in hosts {
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
            set.spawn(async move {
                let started = std::time::Instant::now();
                match tokio::time::timeout(timeout, collect_stats(&target)).await {
                    Ok(Ok(stats)) => {
                        let _ = inv.mark_ok(rec.id, Utc::now()).await;
                        let samples = stats_to_samples(rec.tenant_id, rec.id, &stats);
                        try_ship(Some(&sender), &counters, samples);
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
        // Drain the set — we don't collect return values but we need
        // the tasks to finish (or be dropped on next tick) before the
        // ticker fires again. `JoinSet::join_next` awaits each task
        // in completion order; a slow host doesn't block fast ones
        // because they all run concurrently.
        while let Some(res) = set.join_next().await {
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
    }
}
