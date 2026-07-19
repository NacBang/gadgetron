-- E4-R2/R2.1: Domain Vault, Knowledge Spaces and Shared Mesh registry.
--
-- PostgreSQL owns identity, ACL, lifecycle and monotone revisions. Canonical
-- Markdown bytes remain in the tenant Git/Obsidian Vault. Every polymorphic
-- lookup is tenant-pinned in the service; composite FKs below harden the
-- owner/vault/object paths that PostgreSQL can express directly.

ALTER TABLE users
    ADD CONSTRAINT users_tenant_id_id_key UNIQUE (tenant_id, id);
ALTER TABLE teams
    ADD CONSTRAINT teams_tenant_id_id_key UNIQUE (tenant_id, id);
ALTER TABLE groups
    ADD CONSTRAINT groups_tenant_id_id_key UNIQUE (tenant_id, id);

CREATE TABLE projects (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id),
    slug            TEXT NOT NULL CHECK (slug ~ '^[a-z][a-z0-9-]{0,62}[a-z0-9]$'),
    title           TEXT NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    goal            TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active', 'archived')),
    owner_user_id   UUID NOT NULL,
    policy          JSONB NOT NULL DEFAULT '{}'::JSONB
                        CHECK (jsonb_typeof(policy) = 'object'),
    policy_revision BIGINT NOT NULL DEFAULT 1 CHECK (policy_revision > 0),
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, slug),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, owner_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX idx_projects_tenant_status ON projects (tenant_id, status, slug);

CREATE TABLE knowledge_spaces (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id         UUID NOT NULL REFERENCES tenants(id),
    kind              TEXT NOT NULL
                          CHECK (kind IN ('personal', 'project', 'team', 'tenant_shared')),
    title             TEXT NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    owner_user_id     UUID,
    owner_team_id     TEXT,
    owner_project_id  UUID,
    status            TEXT NOT NULL DEFAULT 'active'
                          CHECK (status IN ('active', 'archived')),
    policy            JSONB NOT NULL DEFAULT '{}'::JSONB
                          CHECK (jsonb_typeof(policy) = 'object'),
    revision          BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, owner_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, owner_team_id) REFERENCES teams(tenant_id, id),
    FOREIGN KEY (tenant_id, owner_project_id) REFERENCES projects(tenant_id, id),
    CHECK (
        (kind = 'personal' AND owner_user_id IS NOT NULL AND owner_team_id IS NULL AND owner_project_id IS NULL)
        OR (kind = 'team' AND owner_user_id IS NULL AND owner_team_id IS NOT NULL AND owner_project_id IS NULL)
        OR (kind = 'project' AND owner_user_id IS NULL AND owner_team_id IS NULL AND owner_project_id IS NOT NULL)
        OR (kind = 'tenant_shared' AND owner_user_id IS NULL AND owner_team_id IS NULL AND owner_project_id IS NULL)
    )
);

CREATE UNIQUE INDEX knowledge_spaces_personal_owner_key
    ON knowledge_spaces (tenant_id, owner_user_id) WHERE kind = 'personal';
CREATE UNIQUE INDEX knowledge_spaces_team_owner_key
    ON knowledge_spaces (tenant_id, owner_team_id) WHERE kind = 'team';
CREATE UNIQUE INDEX knowledge_spaces_project_owner_key
    ON knowledge_spaces (tenant_id, owner_project_id) WHERE kind = 'project';
CREATE UNIQUE INDEX knowledge_spaces_tenant_shared_key
    ON knowledge_spaces (tenant_id) WHERE kind = 'tenant_shared';
CREATE INDEX idx_knowledge_spaces_tenant_status
    ON knowledge_spaces (tenant_id, status, kind);

CREATE TABLE knowledge_space_grants (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id),
    space_id        UUID NOT NULL,
    principal_kind  TEXT NOT NULL CHECK (principal_kind IN ('user', 'team', 'group')),
    principal_id    TEXT NOT NULL CHECK (length(btrim(principal_id)) BETWEEN 1 AND 128),
    role            TEXT NOT NULL CHECK (role IN ('viewer', 'contributor', 'curator', 'manager')),
    expires_at      TIMESTAMPTZ,
    created_by      UUID NOT NULL,
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at      TIMESTAMPTZ,
    UNIQUE (space_id, principal_kind, principal_id),
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES users(tenant_id, id)
);

CREATE INDEX idx_knowledge_space_grants_effective
    ON knowledge_space_grants (tenant_id, space_id, principal_kind, principal_id)
    WHERE revoked_at IS NULL;

CREATE TABLE knowledge_vaults (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID NOT NULL REFERENCES tenants(id),
    space_id            UUID NOT NULL,
    home_bundle_id      TEXT NOT NULL
                            CHECK (home_bundle_id ~ '^[a-z][a-z0-9-]{0,62}[a-z0-9]$' OR home_bundle_id = 'core'),
    knowledge_schema_id TEXT NOT NULL DEFAULT 'core.knowledge',
    schema_version      INTEGER NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    owner_state         TEXT NOT NULL DEFAULT 'enabled'
                            CHECK (owner_state IN ('enabled', 'degraded', 'owner_unavailable')),
    revision            BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, space_id, home_bundle_id),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id)
);

CREATE INDEX idx_knowledge_vaults_owner
    ON knowledge_vaults (tenant_id, home_bundle_id, owner_state);

CREATE TABLE knowledge_objects (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id),
    vault_id        UUID NOT NULL,
    canonical_kind  TEXT NOT NULL
                        CHECK (canonical_kind IN ('source', 'note', 'candidate', 'lesson', 'insight', 'domain_entity')),
    path            TEXT NOT NULL CHECK (length(btrim(path)) BETWEEN 1 AND 1024),
    status          TEXT NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active', 'archived', 'tombstone', 'owner_unavailable')),
    content_hash    TEXT CHECK (content_hash IS NULL OR content_hash ~ '^[0-9a-f]{64}$'),
    created_by      UUID NOT NULL,
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    UNIQUE (vault_id, path),
    FOREIGN KEY (tenant_id, vault_id) REFERENCES knowledge_vaults(tenant_id, id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES users(tenant_id, id)
);

CREATE INDEX idx_knowledge_objects_vault_status
    ON knowledge_objects (tenant_id, vault_id, status, canonical_kind);

CREATE TABLE knowledge_shares (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID NOT NULL REFERENCES tenants(id),
    source_space_id     UUID NOT NULL,
    source_object_id    UUID NOT NULL,
    source_revision     BIGINT NOT NULL CHECK (source_revision > 0),
    target_space_id     UUID NOT NULL,
    mode                TEXT NOT NULL
                            CHECK (mode IN ('reference', 'snapshot', 'fork', 'promote', 'synthesize')),
    follow_latest       BOOLEAN NOT NULL DEFAULT FALSE,
    target_object_id    UUID,
    policy_disposition  TEXT NOT NULL DEFAULT 'allowed'
                            CHECK (policy_disposition IN ('allowed', 'reviewed', 'pending_review')),
    created_by          UUID NOT NULL,
    revision            BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at          TIMESTAMPTZ,
    CHECK (source_space_id <> target_space_id),
    CHECK (mode = 'reference' OR follow_latest = FALSE),
    FOREIGN KEY (tenant_id, source_space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, target_space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, source_object_id) REFERENCES knowledge_objects(tenant_id, id),
    FOREIGN KEY (tenant_id, target_object_id) REFERENCES knowledge_objects(tenant_id, id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES users(tenant_id, id)
);

CREATE UNIQUE INDEX knowledge_shares_active_key
    ON knowledge_shares (tenant_id, source_object_id, target_space_id, mode)
    WHERE revoked_at IS NULL;
CREATE INDEX idx_knowledge_shares_target_active
    ON knowledge_shares (tenant_id, target_space_id, created_at DESC)
    WHERE revoked_at IS NULL;

GRANT SELECT, INSERT, UPDATE, DELETE ON projects TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_spaces TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_space_grants TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_vaults TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_objects TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_shares TO gadgetron_app;
