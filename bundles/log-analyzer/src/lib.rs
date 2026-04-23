//! Log analyzer bundle — incremental scan of registered hosts'
//! dmesg / journalctl / auth.log into severity-classified findings.
//!
//! Pipeline per tick:
//!   1. For each host with `log_scan_config.enabled = true`,
//!      check `log_scan_cursor` to find "last byte/timestamp/cursor
//!      we already saw".
//!   2. SSH-fetch only NEW lines via dmesg `--since`, journalctl
//!      `--cursor`, or `tail` byte offset.
//!   3. Run `rules::classify` on each line. If matched: insert/update
//!      `log_findings`. If `Error`-level but unmatched: queue for
//!      Penny LLM classification (handled by a separate task to keep
//!      the scanner fast).
//!   4. Persist new cursor.
//!
//! Operator-facing surface:
//!   - `loganalysis.list`        : list non-dismissed findings (filter
//!                                  by host_id / severity / since)
//!   - `loganalysis.dismiss`     : mark as handled
//!   - `loganalysis.scan_now`    : force a tick for one host
//!   - `loganalysis.set_interval`: per-host poll interval override

pub mod comments;
pub mod gadgets;
pub mod llm;
pub mod model;
pub mod rules;
pub mod scanner;
pub mod store;

pub use gadgets::LogAnalyzerProvider;
pub use model::{Finding, Severity};
pub use scanner::{run_background_scanner, ScannerConfig};
