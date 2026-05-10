CREATE TABLE IF NOT EXISTS audit_log (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID         NOT NULL REFERENCES tenants(id),
    api_key_id    UUID         NOT NULL,
    request_id    UUID         NOT NULL,
    model         VARCHAR(255),
    provider      VARCHAR(64),
    status        VARCHAR(32)  NOT NULL DEFAULT 'ok'
                      CHECK (status IN ('ok', 'error', 'stream_interrupted')),
    input_tokens  INTEGER      NOT NULL DEFAULT 0,
    output_tokens INTEGER      NOT NULL DEFAULT 0,
    cost_cents    BIGINT       NOT NULL DEFAULT 0,
    latency_ms    INTEGER      NOT NULL DEFAULT 0,
    timestamp     TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS audit_log_tenant_ts_idx ON audit_log(tenant_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS audit_log_request_idx   ON audit_log(request_id);
