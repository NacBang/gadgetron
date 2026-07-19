-- Alert/trigger layer (ISSUE 65) — persisted alert-instance state.
--
-- The background alert evaluator (bundles/server-monitor/src/alerts.rs)
-- turns detected conditions — open `log_findings` at/above a severity
-- floor, and stale `host_stats_latest` snapshots (host offline) — into
-- firing alerts on the in-process ActivityBus (→ /events/ws). This
-- table is the DURABLE source of truth for instance state so the
-- `for`/pending timer and dedup survive a restart (the ActivityBus is
-- an ephemeral live tap, never storage).
--
-- One row per alert INSTANCE, keyed by a stable `fingerprint` (e.g.
-- `finding:<uuid>` or `host_offline:<host_id>`). Identity = fingerprint
-- only, mirroring Alertmanager: re-evaluating the same condition every
-- tick maps to the same row, so notifications fire on state EDGES, not
-- every tick. Resolved instances are deleted by the evaluator after the
-- resolve edge is emitted (v1 keeps no alert history table).
--
-- `state` lifecycle: pending → firing → (deleted on resolve). `pending`
-- is the anti-flap waiting room (value sustained for `for_secs` before
-- firing); with the v1 sources both fire immediately (their sources are
-- already debounced), but the column models the future threshold-rule
-- path.

CREATE TABLE IF NOT EXISTS alert_state (
    fingerprint      TEXT PRIMARY KEY,
    tenant_id        UUID NOT NULL,
    host_id          UUID,
    rule_key         TEXT NOT NULL,         -- 'log_finding' | 'host_offline'
    severity         TEXT NOT NULL,
    message          TEXT NOT NULL,
    state            TEXT NOT NULL,         -- 'pending' | 'firing'
    pending_since    TIMESTAMPTZ NOT NULL DEFAULT now(),
    active_since     TIMESTAMPTZ,           -- when it entered 'firing'
    last_notified_at TIMESTAMPTZ,
    last_eval_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT alert_state_severity_check CHECK (
        severity IN ('critical', 'high', 'medium', 'info')
    ),
    CONSTRAINT alert_state_state_check CHECK (
        state IN ('pending', 'firing')
    )
);

-- Tenant-leading (cross-tenant isolation) firing-set scan: "what is
-- currently firing for this tenant, hottest first".
CREATE INDEX IF NOT EXISTS alert_state_firing_idx
    ON alert_state(tenant_id, severity, last_eval_at DESC)
    WHERE state = 'firing';

COMMENT ON TABLE alert_state IS
    'Durable per-instance state for the alert/trigger evaluator (ISSUE
     65). Keyed by fingerprint; rows are pending/firing only — resolved
     instances are deleted once their resolve edge is published.';
