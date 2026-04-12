CREATE TABLE IF NOT EXISTS quota_configs (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          UUID    NOT NULL REFERENCES tenants(id) ON DELETE CASCADE UNIQUE,
    daily_limit_cents  BIGINT  NOT NULL DEFAULT 1000000,
    monthly_limit_cents BIGINT NOT NULL DEFAULT 10000000,
    daily_used_cents   BIGINT  NOT NULL DEFAULT 0,
    monthly_used_cents BIGINT  NOT NULL DEFAULT 0,
    rpm_limit          INTEGER NOT NULL DEFAULT 60,
    tpm_limit          INTEGER NOT NULL DEFAULT 100000,
    concurrent_max     INTEGER NOT NULL DEFAULT 10,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS quota_configs_tenant_idx ON quota_configs(tenant_id);
