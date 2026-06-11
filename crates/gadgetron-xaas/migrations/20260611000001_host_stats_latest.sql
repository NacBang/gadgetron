-- Latest full ServerStats snapshot per host (ISSUE 38).
--
-- Written by the server-monitor background poller (1 Hz UPSERT, one row
-- per host) and read by the `server.stats` workbench action. This
-- decouples UI reads from SSH collection: N concurrent viewers share
-- ONE collector per host instead of each opening their own SSH session
-- against the monitored machine.
--
-- One row per host — history lives in `host_metrics`; this table is
-- only the "current card" the UI renders.
CREATE TABLE IF NOT EXISTS host_stats_latest (
    host_id    UUID PRIMARY KEY,
    tenant_id  UUID        NOT NULL,
    stats      JSONB       NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS host_stats_latest_tenant_idx
    ON host_stats_latest (tenant_id);
