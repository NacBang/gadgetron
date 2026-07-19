-- Passive Gadgetini cooling observations owned by Server Administrator.

CREATE TABLE IF NOT EXISTS server_gadgetini_latest (
    tenant_id                  UUID             NOT NULL,
    gadgetini_id               TEXT             NOT NULL CHECK (
        gadgetini_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(gadgetini_id) <= 64
    ),
    parent_target_id           TEXT             NOT NULL CHECK (
        parent_target_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(parent_target_id) <= 64
    ),
    attach_mode                TEXT             NOT NULL CHECK (attach_mode IN ('direct', 'usb')),
    observation_status         TEXT             NOT NULL CHECK (
        observation_status IN ('observed', 'partial', 'unreachable', 'not_supported')
    ),
    air_humidity_pct           DOUBLE PRECISION,
    air_temp_c                 DOUBLE PRECISION,
    chassis_stable             BOOLEAN,
    coolant_delta_t_c          DOUBLE PRECISION,
    coolant_leak_detected      BOOLEAN,
    coolant_level_ok           BOOLEAN,
    coolant_temp_inlet1_c      DOUBLE PRECISION,
    coolant_temp_inlet2_c      DOUBLE PRECISION,
    coolant_temp_outlet1_c     DOUBLE PRECISION,
    coolant_temp_outlet2_c     DOUBLE PRECISION,
    host_status_code           BIGINT,
    warnings                   JSONB            NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(warnings) = 'array'
    ),
    observed_at                TIMESTAMPTZ       NOT NULL,
    updated_at                 TIMESTAMPTZ       NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, gadgetini_id),
    CHECK (gadgetini_id <> parent_target_id)
);

CREATE INDEX IF NOT EXISTS server_gadgetini_latest_parent_idx
    ON server_gadgetini_latest (tenant_id, parent_target_id, observed_at DESC);

CREATE TABLE IF NOT EXISTS server_gadgetini_observations (
    observation_id             UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                  UUID             NOT NULL,
    gadgetini_id               TEXT             NOT NULL CHECK (
        gadgetini_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(gadgetini_id) <= 64
    ),
    parent_target_id           TEXT             NOT NULL CHECK (
        parent_target_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(parent_target_id) <= 64
    ),
    attach_mode                TEXT             NOT NULL CHECK (attach_mode IN ('direct', 'usb')),
    observation_status         TEXT             NOT NULL CHECK (
        observation_status IN ('observed', 'partial', 'unreachable', 'not_supported')
    ),
    air_humidity_pct           DOUBLE PRECISION,
    air_temp_c                 DOUBLE PRECISION,
    chassis_stable             BOOLEAN,
    coolant_delta_t_c          DOUBLE PRECISION,
    coolant_leak_detected      BOOLEAN,
    coolant_level_ok           BOOLEAN,
    coolant_temp_inlet1_c      DOUBLE PRECISION,
    coolant_temp_inlet2_c      DOUBLE PRECISION,
    coolant_temp_outlet1_c     DOUBLE PRECISION,
    coolant_temp_outlet2_c     DOUBLE PRECISION,
    host_status_code           BIGINT,
    warnings                   JSONB            NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(warnings) = 'array'
    ),
    observed_at                TIMESTAMPTZ       NOT NULL,
    CHECK (gadgetini_id <> parent_target_id)
);

CREATE INDEX IF NOT EXISTS server_gadgetini_observations_history_idx
    ON server_gadgetini_observations (tenant_id, gadgetini_id, observed_at DESC);
