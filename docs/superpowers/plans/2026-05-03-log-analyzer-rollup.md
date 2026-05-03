# Log Analyzer Roll-Up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Catch recurring SMART disk failures and roll repeated findings into one operator-visible Logs card.

**Architecture:** Add a `fingerprint` to classifications and persisted findings. The scanner remains line-oriented, while the store owns duplicate prevention with a Postgres partial unique index and atomic upsert.

**Tech Stack:** Rust, sqlx/Postgres migrations, Next.js/React Logs page, Vitest.

---

### Task 1: Classification Contract

**Files:**
- Modify: `bundles/log-analyzer/src/model.rs`
- Modify: `bundles/log-analyzer/src/rules.rs`
- Modify: `bundles/log-analyzer/src/llm.rs`

- [x] Add failing rule tests for SMART pending sectors, offline uncorrectable sectors, and smartd mail delivery failure.
- [x] Add `Classification::fingerprint`.
- [x] Generate device-specific SMART disk fingerprints and category fallback fingerprints.
- [x] Update LLM classifications to use category fallback fingerprints.
- [x] Run `cargo test -p gadgetron-bundle-log-analyzer rules -- --nocapture`.

### Task 2: Store Roll-Up

**Files:**
- Create: `crates/gadgetron-xaas/migrations/20260503000003_log_finding_fingerprint.sql`
- Modify: `bundles/log-analyzer/src/model.rs`
- Modify: `bundles/log-analyzer/src/store.rs`

- [x] Add `log_findings.fingerprint`.
- [x] Backfill existing rows with `category`.
- [x] Add a partial unique index for open findings.
- [x] Change `upsert_finding` to mute dismissed matches by fingerprint and atomically upsert open matches.
- [x] Run `cargo test -p gadgetron-bundle-log-analyzer`.

### Task 3: Scanner Defaults

**Files:**
- Modify: `crates/gadgetron-cli/src/main.rs`
- Modify: `bundles/log-analyzer/src/scanner.rs`

- [x] Default automatic scanner to enabled unless `GADGETRON_LOG_ANALYZER_AUTO` is an explicit off value.
- [x] Increase first journal lookback to six hours.
- [x] Run `cargo test -p gadgetron-cli log_analyzer -- --nocapture` where available, then `cargo check -p gadgetron-cli`.

### Task 4: Logs UI State

**Files:**
- Modify: `crates/gadgetron-web/web/app/(shell)/findings/page.tsx`

- [x] Add `fingerprint` to the `Finding` type.
- [x] Show first/last seen metadata and latest excerpt only.
- [x] Replace the generic empty state with "not scanned yet" vs "no open findings."
- [x] Run `npm run test -- WorkbenchShell.test.tsx` and `npm run build`.

### Task 5: Live Verification

**Files:**
- No new source files.

- [x] Rebuild and restart Gadgetron.
- [x] Run `loganalysis.scan_now` for `dg5R-PRO6000-8`.
- [x] Confirm Postgres has one open SMART disk-health finding for `/dev/sdb` with repeated occurrences folded into `count`.
- [x] Confirm Logs API returns the finding and UI no longer shows a misleading empty state.
