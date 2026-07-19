-- R1.4 Server Administrator operational vertical.
--
-- Repairs legacy tenant keys for brokered writes, then adds the current
-- inventory/topology projection and immutable Bundle-owned outcome ledger.

ALTER TABLE host_stats_latest
    DROP CONSTRAINT IF EXISTS host_stats_latest_pkey;
ALTER TABLE host_stats_latest
    ADD PRIMARY KEY (tenant_id, host_id);

ALTER TABLE log_scan_cursor
    ADD COLUMN IF NOT EXISTS tenant_id UUID;
UPDATE log_scan_cursor AS cursor
   SET tenant_id = latest.tenant_id
  FROM host_stats_latest AS latest
 WHERE cursor.tenant_id IS NULL
   AND cursor.host_id = latest.host_id;
-- A scan cursor is rebuildable execution state, not user evidence. Legacy
-- rows that cannot be attributed safely are dropped instead of guessed.
DELETE FROM log_scan_cursor WHERE tenant_id IS NULL;
ALTER TABLE log_scan_cursor ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE log_scan_cursor DROP CONSTRAINT IF EXISTS log_scan_cursor_pkey;
ALTER TABLE log_scan_cursor ADD PRIMARY KEY (tenant_id, host_id, source);

ALTER TABLE log_scan_config
    ADD COLUMN IF NOT EXISTS tenant_id UUID;
UPDATE log_scan_config AS config
   SET tenant_id = latest.tenant_id
  FROM host_stats_latest AS latest
 WHERE config.tenant_id IS NULL
   AND config.host_id = latest.host_id;
DELETE FROM log_scan_config WHERE tenant_id IS NULL;
ALTER TABLE log_scan_config ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE log_scan_config DROP CONSTRAINT IF EXISTS log_scan_config_pkey;
ALTER TABLE log_scan_config ADD PRIMARY KEY (tenant_id, host_id);

ALTER TABLE alert_state DROP CONSTRAINT IF EXISTS alert_state_pkey;
ALTER TABLE alert_state ADD PRIMARY KEY (tenant_id, fingerprint);

CREATE TABLE IF NOT EXISTS server_assets_latest (
    tenant_id      UUID        NOT NULL,
    host_id        UUID        NOT NULL,
    target_id      TEXT        NOT NULL,
    inventory      JSONB       NOT NULL DEFAULT '{}'::jsonb,
    topology       JSONB       NOT NULL DEFAULT '{}'::jsonb,
    observed_at    TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, host_id),
    UNIQUE (tenant_id, target_id)
);

CREATE INDEX IF NOT EXISTS server_assets_latest_observed_idx
    ON server_assets_latest (tenant_id, observed_at DESC);

CREATE TABLE IF NOT EXISTS server_operation_outcomes (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id         UUID        NOT NULL,
    operation_id      TEXT        NOT NULL,
    target_kind       TEXT        NOT NULL,
    target_id         TEXT        NOT NULL,
    action            TEXT        NOT NULL,
    before_state      JSONB       NOT NULL,
    after_state       JSONB       NOT NULL,
    observed_outcome  TEXT        NOT NULL CHECK (
        observed_outcome IN ('succeeded', 'failed', 'indeterminate')
    ),
    actor_ref         TEXT        NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS server_operation_outcomes_target_idx
    ON server_operation_outcomes (tenant_id, target_kind, target_id, created_at DESC);

CREATE TABLE IF NOT EXISTS server_job_runs (
    job_id         TEXT        NOT NULL,
    tenant_id      UUID        NOT NULL,
    recipe_id      TEXT        NOT NULL,
    target_id      TEXT        NOT NULL,
    status         TEXT        NOT NULL CHECK (
        status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')
    ),
    progress       JSONB       NOT NULL DEFAULT '{}'::jsonb,
    result         JSONB,
    started_at     TIMESTAMPTZ NOT NULL,
    finished_at    TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, job_id)
);

CREATE INDEX IF NOT EXISTS server_job_runs_status_idx
    ON server_job_runs (tenant_id, status, started_at DESC);
