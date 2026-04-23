//! Background scanner. One task per registered host; each task wakes
//! on its configured interval (default 120 s), pulls only NEW lines
//! from each enabled source via SSH, classifies, persists.
//!
//! Sources implemented in v1:
//!   - `dmesg`   — kernel ring buffer; cursor = ISO timestamp string
//!   - `journal` — `journalctl -p err..emerg`; cursor = `--cursor` token
//!   - `auth`    — `/var/log/auth.log`; cursor = byte offset
//!
//! All SSH commands wrap with `timeout 10s` so a wedged target
//! doesn't stall the whole scanner. ControlMaster from server-monitor
//! makes per-tick cost cheap.

use crate::llm::{Classifier, MAX_LLM_PER_TICK};
use crate::model::Severity;
use crate::rules;
use crate::store;
use chrono::{DateTime, SecondsFormat, Utc};
use gadgetron_bundle_server_monitor::{HostRecord, InventoryStore};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ScannerConfig {
    pub default_interval: Duration,
    /// Maximum new lines to read per source per tick. Cap protects
    /// the LLM bill and keeps the SSH payload bounded after a long
    /// outage.
    pub max_lines_per_tick: usize,
    pub per_host_timeout: Duration,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            default_interval: Duration::from_secs(120),
            max_lines_per_tick: 500,
            per_host_timeout: Duration::from_secs(10),
        }
    }
}

const SOURCES: &[&str] = &["dmesg", "journal", "auth"];

pub async fn run_background_scanner(
    inventory: Arc<InventoryStore>,
    pool: PgPool,
    classifier: Option<Arc<dyn Classifier>>,
    cfg: ScannerConfig,
) {
    tracing::info!(
        target: "log_analyzer.scanner",
        default_interval_secs = cfg.default_interval.as_secs_f64(),
        "log-analyzer background scanner started"
    );
    // Single-loop scheduler: each tick (every 30 s) iterates hosts
    // and decides per-host whether enough time has elapsed since its
    // own last_scanned. Avoids a JoinSet per host that would survive
    // host removal awkwardly. For larger fleets (>50), upgrade to
    // per-host JoinSet keyed on inventory list.
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let hosts = match inventory.load().await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(target: "log_analyzer.scanner", error=%e, "inventory load failed");
                continue;
            }
        };
        for rec in hosts {
            // Per-host config; default applies if no row.
            let (interval_secs, enabled) = match store::get_config(&pool, rec.id).await {
                Ok(Some((i, e))) => (i as u64, e),
                Ok(None) => (cfg.default_interval.as_secs(), true),
                Err(_) => (cfg.default_interval.as_secs(), true),
            };
            if !enabled {
                continue;
            }
            // Race the per-source scan inside a timeout so a stuck
            // SSH session can't pin one host's loop.
            let due = match store::get_cursor(&pool, rec.id, "_meta").await {
                Ok(Some((_, when))) => {
                    Utc::now().signed_duration_since(when).num_seconds() as u64 >= interval_secs
                }
                _ => true,
            };
            if !due {
                continue;
            }
            let _ = store::set_cursor(&pool, rec.id, "_meta", "tick").await;

            for source in SOURCES {
                let pool_c = pool.clone();
                let classifier_c = classifier.clone();
                let rec_c = rec.clone();
                let inv_c = inventory.clone();
                let timeout = cfg.per_host_timeout;
                let max_lines = cfg.max_lines_per_tick;
                let source = source.to_string();
                let _ = tokio::time::timeout(timeout * 2, async move {
                    if let Err(e) = scan_one(
                        &pool_c,
                        &inv_c,
                        &rec_c,
                        &source,
                        classifier_c.as_deref(),
                        max_lines,
                        timeout,
                    )
                    .await
                    {
                        tracing::warn!(
                            target: "log_analyzer.scanner",
                            host = %rec_c.host,
                            source = %source,
                            error = %e,
                            "scan failed"
                        );
                    }
                })
                .await;
            }
        }
    }
}

async fn scan_one(
    pool: &PgPool,
    inventory: &InventoryStore,
    rec: &HostRecord,
    source: &str,
    classifier: Option<&dyn Classifier>,
    max_lines: usize,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let target = to_target(rec, inventory.root().join("known_hosts"));
    let cursor = store::get_cursor(pool, rec.id, source).await.ok().flatten();

    let (lines, new_cursor) = match source {
        "dmesg" => {
            fetch_dmesg(
                &target,
                cursor.as_ref().map(|(c, _)| c.as_str()),
                max_lines,
                timeout,
            )
            .await?
        }
        "journal" => {
            fetch_journal(
                &target,
                cursor.as_ref().map(|(c, _)| c.as_str()),
                max_lines,
                timeout,
            )
            .await?
        }
        "auth" => {
            fetch_auth(
                &target,
                cursor.as_ref().map(|(c, _)| c.as_str()),
                max_lines,
                timeout,
            )
            .await?
        }
        _ => return Ok(()),
    };

    if !lines.is_empty() {
        let mut llm_budget = MAX_LLM_PER_TICK;
        for line in &lines {
            if let Some(cls) = rules::classify(line) {
                let _ =
                    store::upsert_finding(pool, rec.tenant_id, rec.id, source, &cls, line, "rule")
                        .await;
            } else if let Some(c) = classifier {
                if llm_budget == 0 || !rules::looks_error_ish(line) {
                    continue;
                }
                if let Some(cls) = c.classify(line).await {
                    // Only persist non-info LLM verdicts to keep noise
                    // down; info means "Penny saw it but it's not
                    // actionable", and surfacing every benign line
                    // would defeat the dismiss-once UX.
                    if cls.severity != Severity::Info {
                        let _ = store::upsert_finding(
                            pool,
                            rec.tenant_id,
                            rec.id,
                            source,
                            &cls,
                            line,
                            "penny",
                        )
                        .await;
                    }
                }
                llm_budget = llm_budget.saturating_sub(1);
            }
        }
    }
    if let Some(nc) = new_cursor {
        store::set_cursor(pool, rec.id, source, &nc).await?;
    }
    Ok(())
}

fn to_target(
    rec: &HostRecord,
    known_hosts: std::path::PathBuf,
) -> gadgetron_bundle_server_monitor::ssh::SshTarget {
    gadgetron_bundle_server_monitor::ssh::SshTarget {
        host: rec.host.clone(),
        user: rec.ssh_user.clone(),
        port: rec.ssh_port,
        key_path: Some(rec.key_path.clone()),
        known_hosts,
    }
}

async fn fetch_dmesg(
    target: &gadgetron_bundle_server_monitor::ssh::SshTarget,
    cursor: Option<&str>,
    max_lines: usize,
    timeout: Duration,
) -> Result<(Vec<String>, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
    // Cursor is ISO-8601; first scan with no cursor reads only the
    // last hour to avoid drowning on first run.
    let since = match cursor {
        Some(c) => c.to_string(),
        None => {
            (Utc::now() - chrono::Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
        }
    };
    let cmd = format!(
        "timeout {t}s sudo -n /usr/bin/dmesg --time-format=iso --since='{since}' 2>/dev/null | tail -n {n}",
        t = timeout.as_secs(),
        n = max_lines,
    );
    let out = gadgetron_bundle_server_monitor::ssh::exec(target, &cmd).await?;
    let lines: Vec<String> = out
        .stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .collect();
    let new_cursor = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    Ok((lines, Some(new_cursor)))
}

async fn fetch_journal(
    target: &gadgetron_bundle_server_monitor::ssh::SshTarget,
    cursor: Option<&str>,
    max_lines: usize,
    timeout: Duration,
) -> Result<(Vec<String>, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
    // -p 0..3 keeps to err+ severities (we only care about errors).
    // First run: read last 30 minutes to anchor the cursor without
    // surfacing ancient noise.
    let cursor_arg = match cursor {
        Some(c) => format!("--after-cursor={}", shell_q(c)),
        None => "--since='-30min'".to_string(),
    };
    let cmd = format!(
        "timeout {t}s sudo -n /usr/bin/journalctl -p 0..3 --no-pager --show-cursor \
         {cursor_arg} -n {n} 2>/dev/null",
        t = timeout.as_secs(),
        n = max_lines,
    );
    let out = gadgetron_bundle_server_monitor::ssh::exec(target, &cmd).await?;
    let mut lines: Vec<String> = Vec::new();
    let mut new_cursor: Option<String> = None;
    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if let Some(c) = trimmed.strip_prefix("-- cursor: ") {
            new_cursor = Some(c.to_string());
        } else if !trimmed.is_empty() && !trimmed.starts_with("-- ") && !trimmed.starts_with("--") {
            lines.push(line.to_string());
        }
    }
    Ok((lines, new_cursor.or_else(|| cursor.map(|s| s.to_string()))))
}

async fn fetch_auth(
    target: &gadgetron_bundle_server_monitor::ssh::SshTarget,
    cursor: Option<&str>,
    max_lines: usize,
    timeout: Duration,
) -> Result<(Vec<String>, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
    // Cursor = byte offset into /var/log/auth.log. First run: tail
    // last `max_lines` entries to seed.
    let prev_offset: Option<u64> = cursor.and_then(|c| c.parse().ok());
    let cmd = match prev_offset {
        Some(off) => format!(
            "timeout {t}s sudo -n /usr/bin/tail -c +{plus} /var/log/auth.log 2>/dev/null | head -c 1048576",
            t = timeout.as_secs(),
            plus = off + 1,
        ),
        None => format!(
            "timeout {t}s sudo -n /usr/bin/tail -n {n} /var/log/auth.log 2>/dev/null",
            t = timeout.as_secs(),
            n = max_lines,
        ),
    };
    let out = gadgetron_bundle_server_monitor::ssh::exec(target, &cmd).await?;
    let chunk = out.stdout;
    let lines: Vec<String> = chunk
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(max_lines)
        .map(|s| s.to_string())
        .collect();
    // Compute the new offset: previous + bytes we just consumed.
    let consumed = chunk.len() as u64;
    let new_offset = prev_offset.unwrap_or(0) + consumed;
    Ok((lines, Some(new_offset.to_string())))
}

fn shell_q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[allow(dead_code)]
fn _unused(_: DateTime<Utc>) {}
