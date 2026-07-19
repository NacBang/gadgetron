-- R3.4a: tenant-owned Core/Bundle AI role overrides and immutable job identity.

ALTER TABLE llm_endpoints
    ADD CONSTRAINT llm_endpoints_tenant_id_id_key UNIQUE (tenant_id, id);

CREATE TABLE knowledge_agent_role_profiles (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    scope_kind          TEXT NOT NULL CHECK (scope_kind IN ('core', 'bundle')),
    bundle_id           TEXT NOT NULL DEFAULT '' CHECK (length(bundle_id) <= 128),
    role_id             TEXT NOT NULL CHECK (length(role_id) BETWEEN 1 AND 128),
    runtime_backend     TEXT NOT NULL CHECK (runtime_backend IN ('claude_code', 'codex_exec')),
    runtime_model       TEXT NOT NULL DEFAULT '' CHECK (length(runtime_model) <= 200),
    runtime_effort      TEXT NOT NULL CHECK (runtime_effort IN ('auto', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra')),
    runtime_model_source TEXT NOT NULL CHECK (runtime_model_source IN ('default', 'local')),
    runtime_endpoint_id UUID,
    revision            BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_by_user_id  UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, scope_kind, bundle_id, role_id),
    FOREIGN KEY (tenant_id, runtime_endpoint_id)
        REFERENCES llm_endpoints(tenant_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (tenant_id, updated_by_user_id)
        REFERENCES users(tenant_id, id),
    CHECK ((scope_kind = 'core' AND bundle_id = '') OR
           (scope_kind = 'bundle' AND length(bundle_id) BETWEEN 1 AND 128)),
    CHECK ((runtime_model_source = 'local') = (runtime_endpoint_id IS NOT NULL))
);

CREATE INDEX knowledge_agent_role_profiles_tenant
    ON knowledge_agent_role_profiles (tenant_id, scope_kind, bundle_id, role_id);

ALTER TABLE knowledge_jobs
    ADD COLUMN bundle_id TEXT,
    ADD COLUMN bundle_role_id TEXT,
    ADD COLUMN package_manifest_sha256 TEXT,
    ADD COLUMN recipe_asset_id TEXT,
    ADD COLUMN recipe_sha256 TEXT,
    ADD COLUMN role_profile_source TEXT,
    ADD COLUMN role_profile_ref TEXT;

ALTER TABLE knowledge_jobs
    ADD CONSTRAINT knowledge_jobs_bundle_role_snapshot_check CHECK (
        (bundle_id IS NULL AND bundle_role_id IS NULL AND package_manifest_sha256 IS NULL AND
         recipe_asset_id IS NULL AND recipe_sha256 IS NULL) OR
        (length(bundle_id) BETWEEN 1 AND 128 AND length(bundle_role_id) BETWEEN 1 AND 128 AND
         package_manifest_sha256 ~ '^[0-9a-f]{64}$' AND length(recipe_asset_id) BETWEEN 1 AND 128 AND
         recipe_sha256 ~ '^[0-9a-f]{64}$')
    ),
    ADD CONSTRAINT knowledge_jobs_role_profile_snapshot_check CHECK (
        (role_profile_source IS NULL AND role_profile_ref IS NULL) OR
        (role_profile_source IN ('global', 'core', 'bundle') AND role_profile_ref ~ '^[0-9a-f]{64}$')
    );

GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_agent_role_profiles TO gadgetron_app;
