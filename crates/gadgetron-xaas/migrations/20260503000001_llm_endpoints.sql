CREATE TABLE IF NOT EXISTS llm_endpoints (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (
        kind IN ('vllm', 'sglang', 'openai_compatible', 'anthropic_proxy', 'ccr')
    ),
    protocol        TEXT NOT NULL CHECK (
        protocol IN ('openai_chat', 'anthropic_messages')
    ),
    base_url        TEXT NOT NULL,
    model_id        TEXT,
    health_status   TEXT NOT NULL DEFAULT 'unknown' CHECK (
        health_status IN ('unknown', 'ok', 'error')
    ),
    last_probe_at   TIMESTAMPTZ,
    last_ok_at      TIMESTAMPTZ,
    last_error      TEXT,
    last_latency_ms INTEGER,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (tenant_id, name)
);

CREATE INDEX IF NOT EXISTS idx_llm_endpoints_tenant_updated
    ON llm_endpoints (tenant_id, updated_at DESC);
