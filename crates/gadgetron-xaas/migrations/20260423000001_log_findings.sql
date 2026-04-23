-- Log analyzer bundle — incremental scan + findings store.
--
-- `log_scan_cursor` is the "where did we leave off" record per (host,
-- source). `last_cursor` is opaque text — for `dmesg` it's a `--since`
-- ISO timestamp, for `journalctl` it's the journal cursor token, for a
-- file source it's the byte offset. Schema doesn't care; the scanner
-- agrees with itself.
--
-- `log_findings` is the operator-facing surface. One row per detected
-- anomaly; identical patterns within a 1-hour window are folded into
-- the same row (count++, ts_last bumped) so a chatty kernel doesn't
-- carpet-bomb the UI. `dismissed_at` IS NULL for unread; the dismiss
-- button stamps it. We never hard-delete so historical incidents stay
-- queryable for postmortems.

CREATE TABLE IF NOT EXISTS log_scan_cursor (
    host_id      UUID NOT NULL,
    source       TEXT NOT NULL,    -- 'dmesg' | 'journal' | 'auth' | 'kern' | 'syslog' | 'path:<file>'
    last_cursor  TEXT,
    last_scanned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (host_id, source)
);

CREATE TABLE IF NOT EXISTS log_findings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    host_id         UUID NOT NULL,
    source          TEXT NOT NULL,
    severity        TEXT NOT NULL,
    category        TEXT NOT NULL,
    summary         TEXT NOT NULL,
    excerpt         TEXT NOT NULL,
    ts_first        TIMESTAMPTZ NOT NULL DEFAULT now(),
    ts_last         TIMESTAMPTZ NOT NULL DEFAULT now(),
    count           INTEGER NOT NULL DEFAULT 1,
    dismissed_at    TIMESTAMPTZ,
    dismissed_by    UUID,
    classified_by   TEXT NOT NULL DEFAULT 'rule',  -- 'rule' | 'penny' | 'manual'
    CONSTRAINT log_findings_severity_check CHECK (
        severity IN ('critical', 'high', 'medium', 'info')
    )
);

CREATE INDEX IF NOT EXISTS log_findings_open_idx
    ON log_findings(tenant_id, host_id, severity, ts_last DESC)
    WHERE dismissed_at IS NULL;

CREATE INDEX IF NOT EXISTS log_findings_dedupe_idx
    ON log_findings(host_id, source, category, dismissed_at);

-- Per-host poll interval override. NULL falls back to the global
-- default (defined in code; today 120 s).
CREATE TABLE IF NOT EXISTS log_scan_config (
    host_id          UUID PRIMARY KEY,
    interval_secs    INTEGER NOT NULL DEFAULT 120,
    enabled          BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE log_findings IS
    'Anomalies detected by the log-analyzer bundle. One row per
     (host, category) within a folding window; dismissed_at flips
     non-null when the operator clicks "✓".';
