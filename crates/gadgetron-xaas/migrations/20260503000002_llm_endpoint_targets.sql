ALTER TABLE llm_endpoints
    ADD COLUMN IF NOT EXISTS target_kind TEXT NOT NULL DEFAULT 'external';

ALTER TABLE llm_endpoints
    ADD COLUMN IF NOT EXISTS target_host_id UUID;

ALTER TABLE llm_endpoints
    ADD COLUMN IF NOT EXISTS upstream_endpoint_id UUID REFERENCES llm_endpoints(id) ON DELETE SET NULL;

ALTER TABLE llm_endpoints
    ADD COLUMN IF NOT EXISTS listen_port INTEGER;

ALTER TABLE llm_endpoints
    ADD COLUMN IF NOT EXISTS auth_token_env TEXT;
