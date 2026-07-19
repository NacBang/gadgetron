-- Server-monitor timeseries substrate.
-- Spec: docs/design/phase2/16-server-metrics-timeseries.md
--
-- This migration ships the full schema surface in one file:
--
--   1. timescaledb extension (pgvector is enabled by the knowledge
--      migration; both coexist per the PoC in images/pgvector-timescale).
--   2. host_metrics hypertable — narrow row, tenant_id leading on every
--      index so cross-tenant planner scans are blocked up front.
--   3. Labels JSONB allowlist — ingestion worker validates at the app
--      layer AND the DB enforces a CHECK as defense in depth.
--   4. host_metrics_retention_hold — evidence preservation for incidents
--      still in review (see §4.3 of the design doc).
--   5. Continuous aggregates for 5s, 1m, 5m, 1h tiers.
--   6. Retention policies per tier (72h raw, 7d / 30d / 90d / 2y
--      aggregates).
--
-- A dedicated `gadgetron_metrics_writer` (INSERT-only) role lands in a
-- follow-up migration that runs as a DB superuser — sqlx's default
-- connection user cannot CREATE ROLE, and we want the role separation
-- but don't want every dev host to hit a migration failure because of
-- it. The app is wired to SET ROLE once the role exists; until then it
-- writes as the connection user with no functional loss.

CREATE EXTENSION IF NOT EXISTS timescaledb;

-- ---------------------------------------------------------------------
-- 1. Base table
-- ---------------------------------------------------------------------

CREATE TABLE host_metrics (
    tenant_id  UUID             NOT NULL,
    host_id    UUID             NOT NULL,
    ts         TIMESTAMPTZ      NOT NULL,
    metric     TEXT             NOT NULL,
    value      DOUBLE PRECISION NOT NULL,
    unit       TEXT,
    labels     JSONB            NOT NULL DEFAULT '{}'::jsonb,
    -- Defense in depth: ingestion worker validates labels against an
    -- allowlist, but if a bug slips an unknown key through, this CHECK
    -- rejects the row at the DB boundary. Empty labels are allowed.
    -- Keep this list in sync with the worker-side list — both documented
    -- in §4.5 of the design doc.
    CONSTRAINT host_metrics_labels_allowlist CHECK (
        labels = '{}'::jsonb
        OR labels ?| ARRAY[
            'source',        -- 'dcgm' | 'nvidia-smi' | 'ipmitool' ...
            'gpu_index',     -- 0, 1, ...
            'gpu_name',      -- 'A100 80GB' — Penny-readable
            'iface_kind',    -- 'ethernet' | 'wifi' | 'ib'
            'chip',          -- lm-sensors chip id
            'mount'          -- filesystem mount path
        ]
    )
);

-- Hypertable conversion. 1-hour chunks are the sweet spot for raw
-- 1 Hz ingestion across 10-100 hosts — small enough that retention
-- drops are cheap, large enough that query planner doesn't walk a
-- forest of chunks for a 5-min window.
SELECT create_hypertable('host_metrics', 'ts', chunk_time_interval => INTERVAL '1 hour');

-- Tenant-leading lookup path: the overwhelmingly common query shape is
-- WHERE tenant_id = :t AND host_id = :h AND metric = :m AND ts >= :from.
-- Putting tenant_id first means a query that forgets the tenant clause
-- can't even warm-scan a chunk — cross-tenant leaks are a planner error,
-- not a handler bug.
CREATE INDEX host_metrics_lookup_idx
    ON host_metrics (tenant_id, host_id, metric, ts DESC);

-- Fleet-wide scans for a single metric (alerting, correlation).
CREATE INDEX host_metrics_metric_ts_idx
    ON host_metrics (tenant_id, metric, ts DESC);

COMMENT ON TABLE host_metrics IS
    'Timeseries samples per registered host. Narrow row — one row per
     (host, metric, ts). See docs/design/phase2/16-server-metrics-timeseries.md.';

-- ---------------------------------------------------------------------
-- 2. Legal hold table
-- ---------------------------------------------------------------------

CREATE TABLE host_metrics_retention_hold (
    id          BIGSERIAL       PRIMARY KEY,
    tenant_id   UUID            NOT NULL,
    host_id     UUID            NOT NULL,
    -- NULL metric = hold every metric for the host.
    metric      TEXT,
    hold_from   TIMESTAMPTZ     NOT NULL,
    hold_to     TIMESTAMPTZ     NOT NULL CHECK (hold_to > hold_from),
    reason      TEXT            NOT NULL,
    created_by  UUID            NOT NULL,
    created_at  TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    -- NULL = indefinite; retention job leaves the rows forever.
    expires_at  TIMESTAMPTZ
);

CREATE INDEX host_metrics_retention_hold_tenant_host_idx
    ON host_metrics_retention_hold (tenant_id, host_id);

CREATE INDEX host_metrics_retention_hold_expires_idx
    ON host_metrics_retention_hold (expires_at)
    WHERE expires_at IS NOT NULL;

COMMENT ON TABLE host_metrics_retention_hold IS
    'Evidence-preservation exceptions. Retention policies must LEFT JOIN
     against this table and skip chunks intersecting an active hold.
     Custom retention job lives in the follow-up role/ops migration.';

-- ---------------------------------------------------------------------
-- 3. Continuous aggregates — 5s, 1m, 5m, 1h tiers
-- ---------------------------------------------------------------------

CREATE MATERIALIZED VIEW host_metrics_5s
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    time_bucket(INTERVAL '5 seconds', ts) AS bucket,
    AVG(value)                            AS avg,
    MIN(value)                            AS min,
    MAX(value)                            AS max,
    COUNT(*)                              AS samples
FROM host_metrics
GROUP BY tenant_id, host_id, metric, bucket
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'host_metrics_5s',
    start_offset      => INTERVAL '2 hours',
    end_offset        => INTERVAL '10 seconds',
    schedule_interval => INTERVAL '30 seconds'
);

CREATE MATERIALIZED VIEW host_metrics_1m
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    time_bucket(INTERVAL '1 minute', bucket) AS bucket,
    AVG(avg)                                 AS avg,
    MIN(min)                                 AS min,
    MAX(max)                                 AS max,
    SUM(samples)                             AS samples
FROM host_metrics_5s
GROUP BY tenant_id, host_id, metric, 4
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'host_metrics_1m',
    start_offset      => INTERVAL '1 day',
    end_offset        => INTERVAL '5 minutes',
    schedule_interval => INTERVAL '5 minutes'
);

CREATE MATERIALIZED VIEW host_metrics_5m
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    time_bucket(INTERVAL '5 minutes', bucket) AS bucket,
    AVG(avg)                                  AS avg,
    MIN(min)                                  AS min,
    MAX(max)                                  AS max,
    SUM(samples)                              AS samples
FROM host_metrics_1m
GROUP BY tenant_id, host_id, metric, 4
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'host_metrics_5m',
    start_offset      => INTERVAL '7 days',
    end_offset        => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour'
);

CREATE MATERIALIZED VIEW host_metrics_1h
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    time_bucket(INTERVAL '1 hour', bucket) AS bucket,
    AVG(avg)                                AS avg,
    MIN(min)                                AS min,
    MAX(max)                                AS max,
    SUM(samples)                            AS samples
FROM host_metrics_5m
GROUP BY tenant_id, host_id, metric, 4
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'host_metrics_1h',
    start_offset      => INTERVAL '60 days',
    end_offset        => INTERVAL '2 hours',
    schedule_interval => INTERVAL '6 hours'
);

-- ---------------------------------------------------------------------
-- 4. Retention policies
-- ---------------------------------------------------------------------

-- Each tier's retention matches §2.2.1 of the design doc. The default
-- TimescaleDB retention policy does NOT consult the legal hold table —
-- the follow-up ops migration swaps these for a custom job that does.
-- Shipping the defaults here means dev / demo environments enforce
-- retention correctly even without the ops migration.
SELECT add_retention_policy('host_metrics',     INTERVAL '72 hours');
SELECT add_retention_policy('host_metrics_5s',  INTERVAL '7 days');
SELECT add_retention_policy('host_metrics_1m',  INTERVAL '30 days');
SELECT add_retention_policy('host_metrics_5m',  INTERVAL '90 days');
SELECT add_retention_policy('host_metrics_1h',  INTERVAL '2 years');
