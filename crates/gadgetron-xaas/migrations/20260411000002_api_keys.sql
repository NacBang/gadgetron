CREATE TABLE IF NOT EXISTS api_keys (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID         NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    prefix       VARCHAR(32)  NOT NULL,
    key_hash     CHAR(64)     NOT NULL,
    kind         VARCHAR(16)  NOT NULL CHECK (kind IN ('live', 'test', 'virtual')),
    scopes       TEXT[]        NOT NULL DEFAULT ARRAY['OpenAiCompat']::TEXT[],
    name         VARCHAR(255),
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ,
    UNIQUE (prefix, key_hash)
);

CREATE INDEX IF NOT EXISTS api_keys_tenant_idx ON api_keys(tenant_id) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS api_keys_hash_idx   ON api_keys(key_hash)  WHERE revoked_at IS NULL;

ALTER TABLE api_keys
    ADD CONSTRAINT api_keys_scopes_check CHECK (array_length(scopes, 1) >= 1);
