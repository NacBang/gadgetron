-- Label-preserving Server Administrator history tiers for bounded range views.
-- Legacy aggregates remain byte-identical for upgrade compatibility; these
-- Bundle-owned tiers prevent different disks or sensors from being averaged
-- together and retain weighted sample counts through every coarser tier.

CREATE MATERIALIZED VIEW server_metric_history_5s
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    labels,
    unit,
    time_bucket(INTERVAL '5 seconds', ts) AS bucket,
    AVG(value)                            AS avg,
    MIN(value)                            AS min,
    MAX(value)                            AS max,
    COUNT(*)                              AS samples
FROM host_metrics
GROUP BY tenant_id, host_id, metric, labels, unit, 6
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'server_metric_history_5s',
    start_offset      => INTERVAL '72 hours',
    end_offset        => INTERVAL '10 seconds',
    schedule_interval => INTERVAL '30 seconds'
);

CREATE MATERIALIZED VIEW server_metric_history_1m
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    labels,
    unit,
    time_bucket(INTERVAL '1 minute', bucket) AS bucket,
    SUM(avg * samples::DOUBLE PRECISION)
        / NULLIF(SUM(samples), 0)::DOUBLE PRECISION AS avg,
    MIN(min)                                        AS min,
    MAX(max)                                        AS max,
    SUM(samples)                                    AS samples
FROM server_metric_history_5s
GROUP BY tenant_id, host_id, metric, labels, unit, 6
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'server_metric_history_1m',
    start_offset      => INTERVAL '1 day',
    end_offset        => INTERVAL '5 minutes',
    schedule_interval => INTERVAL '5 minutes'
);

CREATE MATERIALIZED VIEW server_metric_history_5m
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    labels,
    unit,
    time_bucket(INTERVAL '5 minutes', bucket) AS bucket,
    SUM(avg * samples::DOUBLE PRECISION)
        / NULLIF(SUM(samples), 0)::DOUBLE PRECISION AS avg,
    MIN(min)                                        AS min,
    MAX(max)                                        AS max,
    SUM(samples)                                    AS samples
FROM server_metric_history_1m
GROUP BY tenant_id, host_id, metric, labels, unit, 6
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'server_metric_history_5m',
    start_offset      => INTERVAL '7 days',
    end_offset        => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour'
);

CREATE MATERIALIZED VIEW server_metric_history_1h
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    labels,
    unit,
    time_bucket(INTERVAL '1 hour', bucket) AS bucket,
    SUM(avg * samples::DOUBLE PRECISION)
        / NULLIF(SUM(samples), 0)::DOUBLE PRECISION AS avg,
    MIN(min)                                        AS min,
    MAX(max)                                        AS max,
    SUM(samples)                                    AS samples
FROM server_metric_history_5m
GROUP BY tenant_id, host_id, metric, labels, unit, 6
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'server_metric_history_1h',
    start_offset      => INTERVAL '60 days',
    end_offset        => INTERVAL '2 hours',
    schedule_interval => INTERVAL '6 hours'
);

CREATE MATERIALIZED VIEW server_metric_history_6h
WITH (timescaledb.continuous) AS
SELECT
    tenant_id,
    host_id,
    metric,
    labels,
    unit,
    time_bucket(INTERVAL '6 hours', bucket) AS bucket,
    SUM(avg * samples::DOUBLE PRECISION)
        / NULLIF(SUM(samples), 0)::DOUBLE PRECISION AS avg,
    MIN(min)                                        AS min,
    MAX(max)                                        AS max,
    SUM(samples)                                    AS samples
FROM server_metric_history_1h
GROUP BY tenant_id, host_id, metric, labels, unit, 6
WITH NO DATA;

SELECT add_continuous_aggregate_policy(
    'server_metric_history_6h',
    start_offset      => INTERVAL '90 days',
    end_offset        => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour'
);

SELECT add_retention_policy('server_metric_history_5s', INTERVAL '7 days');
SELECT add_retention_policy('server_metric_history_1m', INTERVAL '30 days');
SELECT add_retention_policy('server_metric_history_5m', INTERVAL '90 days');
SELECT add_retention_policy('server_metric_history_1h', INTERVAL '2 years');
SELECT add_retention_policy('server_metric_history_6h', INTERVAL '2 years');
