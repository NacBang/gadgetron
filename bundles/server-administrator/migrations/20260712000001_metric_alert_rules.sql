-- Tenant-owned threshold profiles evaluated by Server Administrator telemetry.

CREATE TABLE IF NOT EXISTS server_metric_alert_rules (
    tenant_id          UUID             NOT NULL,
    rule_id            TEXT             NOT NULL CHECK (
        rule_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(rule_id) <= 64
    ),
    label              TEXT             NOT NULL CHECK (
        length(label) BETWEEN 1 AND 128
    ),
    metric_pattern     TEXT             NOT NULL CHECK (
        length(metric_pattern) BETWEEN 1 AND 128
        AND metric_pattern ~ '^[A-Za-z0-9_.:@*-]+$'
    ),
    direction          TEXT             NOT NULL CHECK (
        direction IN ('above', 'below')
    ),
    high_threshold     DOUBLE PRECISION NOT NULL,
    critical_threshold DOUBLE PRECISION NOT NULL,
    for_seconds        INTEGER          NOT NULL DEFAULT 0 CHECK (
        for_seconds BETWEEN 0 AND 86400
    ),
    enabled            BOOLEAN          NOT NULL DEFAULT TRUE,
    revision           UUID             NOT NULL,
    updated_at         TIMESTAMPTZ      NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, rule_id),
    CHECK (
        (direction = 'above' AND critical_threshold >= high_threshold)
        OR (direction = 'below' AND critical_threshold <= high_threshold)
    )
);

CREATE INDEX IF NOT EXISTS server_metric_alert_rules_enabled_idx
    ON server_metric_alert_rules (tenant_id, enabled, updated_at DESC);

COMMENT ON TABLE server_metric_alert_rules IS
    'Configurable metric thresholds. Hardware-specific temperature and power limits live here instead of parser or UI constants.';
