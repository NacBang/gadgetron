-- E4-R2/R2.2: immutable original bytes + Source Ledger and extraction attempts.
--
-- PostgreSQL owns identity, tenant ACL, lifecycle and revisions. Original
-- bytes live in a tenant content-addressed filesystem store; extracted notes
-- live in the tenant Git/Obsidian Vault.

CREATE TABLE knowledge_blobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id),
    content_hash    TEXT NOT NULL CHECK (content_hash ~ '^sha256:[0-9a-f]{64}$'),
    storage_key     TEXT NOT NULL CHECK (storage_key ~ '^sha256/[0-9a-f]{2}/[0-9a-f]{64}$'),
    byte_size       BIGINT NOT NULL CHECK (byte_size BETWEEN 0 AND 16777216),
    content_type    TEXT NOT NULL CHECK (length(btrim(content_type)) BETWEEN 1 AND 255),
    original_name   TEXT NOT NULL DEFAULT '' CHECK (length(original_name) <= 512),
    created_by      UUID NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at      TIMESTAMPTZ,
    UNIQUE (tenant_id, content_hash),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES users(tenant_id, id)
);

CREATE INDEX idx_knowledge_blobs_tenant_created
    ON knowledge_blobs (tenant_id, created_at DESC) WHERE deleted_at IS NULL;

CREATE TABLE knowledge_sources (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID NOT NULL REFERENCES tenants(id),
    vault_id            UUID NOT NULL,
    source_kind         TEXT NOT NULL CHECK (source_kind IN ('upload', 'article')),
    status              TEXT NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'extracted', 'failed', 'needs_ocr', 'deleted')),
    title               TEXT NOT NULL DEFAULT '' CHECK (length(title) <= 512),
    original_name       TEXT NOT NULL DEFAULT '' CHECK (length(original_name) <= 512),
    requested_uri       TEXT CHECK (requested_uri IS NULL OR length(requested_uri) <= 4096),
    final_uri           TEXT CHECK (final_uri IS NULL OR length(final_uri) <= 4096),
    content_type        TEXT CHECK (content_type IS NULL OR length(content_type) <= 255),
    byte_size           BIGINT CHECK (byte_size IS NULL OR byte_size BETWEEN 0 AND 16777216),
    content_hash        TEXT CHECK (content_hash IS NULL OR content_hash ~ '^sha256:[0-9a-f]{64}$'),
    blob_id             UUID,
    extracted_object_id UUID,
    failure_code        TEXT CHECK (failure_code IS NULL OR failure_code ~ '^[a-z][a-z0-9_]{0,62}$'),
    failure_detail      TEXT CHECK (failure_detail IS NULL OR length(failure_detail) <= 1024),
    attempt_count       INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    created_by          UUID NOT NULL,
    revision            BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    fetched_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at          TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, vault_id) REFERENCES knowledge_vaults(tenant_id, id),
    FOREIGN KEY (tenant_id, blob_id) REFERENCES knowledge_blobs(tenant_id, id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES users(tenant_id, id),
    CHECK ((source_kind = 'upload' AND requested_uri IS NULL) OR source_kind = 'article'),
    CHECK ((blob_id IS NULL AND content_hash IS NULL AND byte_size IS NULL)
           OR (blob_id IS NOT NULL AND content_hash IS NOT NULL AND byte_size IS NOT NULL))
);

CREATE INDEX idx_knowledge_sources_vault_status
    ON knowledge_sources (tenant_id, vault_id, status, created_at DESC);
CREATE INDEX idx_knowledge_sources_blob
    ON knowledge_sources (tenant_id, blob_id) WHERE blob_id IS NOT NULL;

CREATE TABLE knowledge_source_attempts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id),
    source_id       UUID NOT NULL,
    attempt_no      INTEGER NOT NULL CHECK (attempt_no > 0),
    phase           TEXT NOT NULL CHECK (phase IN ('upload', 'fetch', 'extract', 'reconcile')),
    outcome         TEXT NOT NULL CHECK (outcome IN ('succeeded', 'failed', 'needs_ocr')),
    final_uri       TEXT CHECK (final_uri IS NULL OR length(final_uri) <= 4096),
    http_status     INTEGER CHECK (http_status IS NULL OR http_status BETWEEN 100 AND 599),
    content_type    TEXT CHECK (content_type IS NULL OR length(content_type) <= 255),
    byte_size       BIGINT CHECK (byte_size IS NULL OR byte_size BETWEEN 0 AND 16777216),
    content_hash    TEXT CHECK (content_hash IS NULL OR content_hash ~ '^sha256:[0-9a-f]{64}$'),
    failure_code    TEXT CHECK (failure_code IS NULL OR failure_code ~ '^[a-z][a-z0-9_]{0,62}$'),
    failure_detail  TEXT CHECK (failure_detail IS NULL OR length(failure_detail) <= 1024),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (source_id, attempt_no, phase),
    FOREIGN KEY (tenant_id, source_id) REFERENCES knowledge_sources(tenant_id, id)
);

CREATE INDEX idx_knowledge_source_attempts_source
    ON knowledge_source_attempts (tenant_id, source_id, attempt_no, created_at);

ALTER TABLE knowledge_objects
    ADD COLUMN source_id UUID,
    ADD CONSTRAINT knowledge_objects_source_fk
        FOREIGN KEY (tenant_id, source_id) REFERENCES knowledge_sources(tenant_id, id);

ALTER TABLE knowledge_sources
    ADD CONSTRAINT knowledge_sources_extracted_object_fk
        FOREIGN KEY (tenant_id, extracted_object_id) REFERENCES knowledge_objects(tenant_id, id);

CREATE UNIQUE INDEX knowledge_objects_source_note_key
    ON knowledge_objects (tenant_id, source_id)
    WHERE source_id IS NOT NULL AND status <> 'tombstone';

GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_blobs TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_sources TO gadgetron_app;
GRANT SELECT, INSERT ON knowledge_source_attempts TO gadgetron_app;
