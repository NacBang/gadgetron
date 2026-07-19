-- Durable per-target monitoring state for R1.8.

CREATE TABLE IF NOT EXISTS server_target_health (
    tenant_id            UUID        NOT NULL,
    target_id            TEXT        NOT NULL CHECK (
        target_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(target_id) <= 64
    ),
    host_id              UUID        NOT NULL,
    status               TEXT        NOT NULL CHECK (
        status IN ('reachable', 'healthy', 'degraded', 'unreachable')
    ),
    last_probe_kind      TEXT        NOT NULL,
    last_attempt_at      TIMESTAMPTZ NOT NULL,
    last_success_at      TIMESTAMPTZ,
    consecutive_failures INTEGER     NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    last_error_code      TEXT,
    last_error_message   TEXT,
    last_duration_ms     BIGINT CHECK (last_duration_ms IS NULL OR last_duration_ms >= 0),
    revision             UUID        NOT NULL DEFAULT gen_random_uuid(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, target_id),
    UNIQUE (tenant_id, host_id),
    CHECK ((last_error_code IS NULL) = (last_error_message IS NULL)),
    CHECK (last_error_code IS NULL OR length(last_error_code) <= 64),
    CHECK (last_error_message IS NULL OR length(last_error_message) <= 512)
);

CREATE INDEX IF NOT EXISTS server_target_health_attention_idx
    ON server_target_health (tenant_id, status, last_attempt_at DESC)
    WHERE status IN ('degraded', 'unreachable');

ALTER TABLE server_job_runs
    ADD COLUMN IF NOT EXISTS actor_ref TEXT;
ALTER TABLE server_job_runs
    DROP CONSTRAINT IF EXISTS server_job_runs_actor_ref_check;
ALTER TABLE server_job_runs
    ADD CONSTRAINT server_job_runs_actor_ref_check CHECK (
        actor_ref IS NULL OR (length(actor_ref) > 0 AND length(actor_ref) <= 128)
    );
