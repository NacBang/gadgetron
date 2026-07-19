-- E4-R3/R3.4a: Core-owned deterministic collection configuration and runs.

CREATE TABLE knowledge_collections (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                UUID NOT NULL REFERENCES tenants(id),
    space_id                 UUID NOT NULL,
    output_vault_id          UUID NOT NULL,
    bundle_id                TEXT NOT NULL CHECK (bundle_id ~ '^[a-z][a-z0-9-]{0,63}$'),
    profile_id               TEXT NOT NULL CHECK (profile_id ~ '^[a-z][a-z0-9-]{0,63}$'),
    label                    TEXT NOT NULL CHECK (length(btrim(label)) BETWEEN 1 AND 160),
    topic                    TEXT NOT NULL CHECK (length(btrim(topic)) BETWEEN 1 AND 500),
    status                   TEXT NOT NULL DEFAULT 'active'
                                 CHECK (status IN ('active', 'paused', 'archived')),
    connector                TEXT NOT NULL CHECK (connector ~ '^[a-z][a-z0-9-]{0,63}$'),
    source_classes           TEXT[] NOT NULL CHECK (cardinality(source_classes) BETWEEN 1 AND 16),
    allowed_domains          TEXT[] NOT NULL CHECK (cardinality(allowed_domains) BETWEEN 1 AND 64),
    freshness_seconds        BIGINT NOT NULL CHECK (freshness_seconds BETWEEN 60 AND 31536000),
    schedule                 TEXT CHECK (schedule IS NULL OR length(btrim(schedule)) BETWEEN 1 AND 160),
    schedule_enabled         BOOLEAN NOT NULL DEFAULT FALSE,
    next_run_at              TIMESTAMPTZ,
    max_sources              INTEGER NOT NULL CHECK (max_sources BETWEEN 1 AND 100),
    max_bytes                BIGINT NOT NULL CHECK (max_bytes BETWEEN 1 AND 1073741824),
    max_wall_seconds         INTEGER NOT NULL CHECK (max_wall_seconds BETWEEN 1 AND 3600),
    package_manifest_sha256  TEXT NOT NULL CHECK (package_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    recipe_asset_id          TEXT NOT NULL CHECK (recipe_asset_id ~ '^[a-z][a-z0-9-]{0,63}$'),
    recipe_sha256            TEXT NOT NULL CHECK (recipe_sha256 ~ '^[0-9a-f]{64}$'),
    locators                 JSONB NOT NULL CHECK (jsonb_typeof(locators) = 'array'),
    cursor                   JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(cursor) = 'object'),
    created_by_user_id       UUID NOT NULL,
    updated_by_user_id       UUID NOT NULL,
    last_enqueued_at         TIMESTAMPTZ,
    last_run_at              TIMESTAMPTZ,
    revision                 BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at               TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, output_vault_id) REFERENCES knowledge_vaults(tenant_id, id),
    FOREIGN KEY (tenant_id, created_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, updated_by_user_id) REFERENCES users(tenant_id, id),
    CHECK ((schedule_enabled = FALSE) OR (schedule IS NOT NULL AND next_run_at IS NOT NULL))
);

CREATE INDEX knowledge_collections_due_idx
    ON knowledge_collections (next_run_at, tenant_id, id)
    WHERE deleted_at IS NULL AND status = 'active' AND schedule_enabled = TRUE;
CREATE INDEX knowledge_collections_space_idx
    ON knowledge_collections (tenant_id, space_id, updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE TABLE knowledge_collection_runs (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                UUID NOT NULL REFERENCES tenants(id),
    collection_id            UUID NOT NULL,
    space_id                 UUID NOT NULL,
    output_vault_id          UUID NOT NULL,
    trigger                  TEXT NOT NULL CHECK (trigger IN ('on_demand', 'schedule', 'retry')),
    parent_run_id            UUID,
    status                   TEXT NOT NULL DEFAULT 'queued'
                                 CHECK (status IN ('queued', 'running', 'succeeded', 'partial', 'failed', 'cancelled')),
    service_actor_user_id    UUID NOT NULL,
    requested_by_user_id     UUID NOT NULL,
    on_behalf_of_user_id     UUID NOT NULL,
    bundle_id                TEXT NOT NULL CHECK (bundle_id ~ '^[a-z][a-z0-9-]{0,63}$'),
    profile_id               TEXT NOT NULL CHECK (profile_id ~ '^[a-z][a-z0-9-]{0,63}$'),
    connector                TEXT NOT NULL CHECK (connector ~ '^[a-z][a-z0-9-]{0,63}$'),
    source_classes           TEXT[] NOT NULL,
    allowed_domains          TEXT[] NOT NULL,
    freshness_seconds        BIGINT NOT NULL CHECK (freshness_seconds BETWEEN 60 AND 31536000),
    package_manifest_sha256  TEXT NOT NULL CHECK (package_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    recipe_asset_id          TEXT NOT NULL CHECK (recipe_asset_id ~ '^[a-z][a-z0-9-]{0,63}$'),
    recipe_sha256            TEXT NOT NULL CHECK (recipe_sha256 ~ '^[0-9a-f]{64}$'),
    tool_policy_revision     TEXT NOT NULL CHECK (length(btrim(tool_policy_revision)) BETWEEN 1 AND 256),
    max_sources              INTEGER NOT NULL CHECK (max_sources BETWEEN 1 AND 100),
    max_bytes                BIGINT NOT NULL CHECK (max_bytes BETWEEN 1 AND 1073741824),
    max_wall_seconds         INTEGER NOT NULL CHECK (max_wall_seconds BETWEEN 1 AND 3600),
    used_items               INTEGER NOT NULL DEFAULT 0 CHECK (used_items >= 0),
    used_bytes               BIGINT NOT NULL DEFAULT 0 CHECK (used_bytes >= 0),
    cursor_before            JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(cursor_before) = 'object'),
    cursor_after             JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(cursor_after) = 'object'),
    attempt                  INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    scheduled_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_owner              TEXT,
    lease_expires_at         TIMESTAMPTZ,
    heartbeat_at             TIMESTAMPTZ,
    cancel_requested_at      TIMESTAMPTZ,
    terminal_reason          TEXT CHECK (terminal_reason IS NULL OR length(terminal_reason) <= 1024),
    revision                 BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at               TIMESTAMPTZ,
    finished_at              TIMESTAMPTZ,
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, collection_id) REFERENCES knowledge_collections(tenant_id, id),
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, output_vault_id) REFERENCES knowledge_vaults(tenant_id, id),
    FOREIGN KEY (tenant_id, service_actor_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, requested_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, on_behalf_of_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, parent_run_id) REFERENCES knowledge_collection_runs(tenant_id, id)
);

CREATE UNIQUE INDEX knowledge_collection_one_active_run_idx
    ON knowledge_collection_runs (tenant_id, collection_id)
    WHERE status IN ('queued', 'running');
CREATE INDEX knowledge_collection_runs_lease_idx
    ON knowledge_collection_runs (status, scheduled_at, created_at)
    WHERE status IN ('queued', 'running');
CREATE INDEX knowledge_collection_runs_space_idx
    ON knowledge_collection_runs (tenant_id, space_id, created_at DESC);

CREATE TABLE knowledge_collection_run_items (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id             UUID NOT NULL REFERENCES tenants(id),
    run_id                UUID NOT NULL,
    collection_id         UUID NOT NULL,
    position              INTEGER NOT NULL CHECK (position >= 0),
    locator               TEXT NOT NULL CHECK (length(btrim(locator)) BETWEEN 1 AND 4096),
    title                 TEXT NOT NULL DEFAULT '' CHECK (length(title) <= 512),
    source_class          TEXT NOT NULL CHECK (source_class ~ '^[a-z][a-z0-9-]{0,63}$'),
    status                TEXT NOT NULL DEFAULT 'pending'
                              CHECK (status IN ('pending', 'fetching', 'captured', 'unchanged', 'deleted', 'failed', 'skipped')),
    previous_source_id    UUID,
    source_id             UUID,
    canonical_locator     TEXT CHECK (canonical_locator IS NULL OR length(canonical_locator) <= 4096),
    content_hash          TEXT CHECK (content_hash IS NULL OR content_hash ~ '^sha256:[0-9a-f]{64}$'),
    byte_size             BIGINT CHECK (byte_size IS NULL OR byte_size BETWEEN 0 AND 16777216),
    http_status           INTEGER CHECK (http_status IS NULL OR http_status BETWEEN 100 AND 599),
    fetched_at            TIMESTAMPTZ,
    fresh_until           TIMESTAMPTZ,
    deletion_observed_at  TIMESTAMPTZ,
    failure_code          TEXT CHECK (failure_code IS NULL OR failure_code ~ '^[a-z][a-z0-9_]{0,62}$'),
    failure_detail        TEXT CHECK (failure_detail IS NULL OR length(failure_detail) <= 1024),
    attempt_no            INTEGER NOT NULL DEFAULT 0 CHECK (attempt_no >= 0),
    revision              BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, run_id, position),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, run_id) REFERENCES knowledge_collection_runs(tenant_id, id),
    FOREIGN KEY (tenant_id, collection_id) REFERENCES knowledge_collections(tenant_id, id),
    FOREIGN KEY (tenant_id, previous_source_id) REFERENCES knowledge_sources(tenant_id, id),
    FOREIGN KEY (tenant_id, source_id) REFERENCES knowledge_sources(tenant_id, id)
);

CREATE INDEX knowledge_collection_items_next_idx
    ON knowledge_collection_run_items (tenant_id, run_id, position)
    WHERE status IN ('pending', 'fetching');
CREATE INDEX knowledge_collection_items_health_idx
    ON knowledge_collection_run_items (tenant_id, collection_id, locator, created_at DESC);

GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_collections TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON knowledge_collection_runs TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON knowledge_collection_run_items TO gadgetron_app;
