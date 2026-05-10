CREATE TABLE IF NOT EXISTS agent_brain_settings (
    tenant_id UUID PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    mode TEXT NOT NULL,
    external_base_url TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    external_auth_token_env TEXT NOT NULL DEFAULT '',
    custom_model_option BOOLEAN NOT NULL DEFAULT FALSE,
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
