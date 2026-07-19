-- CORE-T3: transactional domain-event bridge into the existing Knowledge
-- Source/Researcher/Gardener pipeline.

ALTER TABLE knowledge_jobs
    DROP CONSTRAINT knowledge_jobs_kind_check,
    ADD CONSTRAINT knowledge_jobs_kind_check CHECK (
        kind IN ('on_demand', 'source_ingest', 'scheduled', 'follow_up', 'event')
    );

ALTER TABLE manager_oversight_records
    DROP CONSTRAINT manager_oversight_records_source_kind_check,
    ADD CONSTRAINT manager_oversight_records_source_kind_check CHECK (
        source_kind IN (
            'workbench_action', 'bundle_job', 'knowledge_job', 'knowledge_event', 'directive'
        )
    );

ALTER TABLE knowledge_sources
    DROP CONSTRAINT knowledge_sources_source_kind_check,
    DROP CONSTRAINT knowledge_sources_locator_check;

ALTER TABLE knowledge_sources
    ADD CONSTRAINT knowledge_sources_source_kind_check CHECK (
        source_kind IN (
            'upload', 'article', 'social_snapshot', 'chat_attachment',
            'incident_snapshot'
        )
    ),
    ADD CONSTRAINT knowledge_sources_locator_check CHECK (
        (source_kind IN ('upload', 'incident_snapshot') AND requested_uri IS NULL)
        OR source_kind = 'chat_attachment'
        OR (
            source_kind IN ('article', 'social_snapshot')
            AND (status = 'deleted' OR requested_uri IS NOT NULL)
        )
    );

ALTER TABLE knowledge_objects
    ADD COLUMN originating_owner_bundle TEXT,
    ADD COLUMN originating_subject_kind TEXT,
    ADD COLUMN originating_subject_id TEXT,
    ADD COLUMN originating_subject_revision TEXT,
    ADD CONSTRAINT knowledge_objects_originating_subject_check CHECK (
        (
            originating_owner_bundle IS NULL
            AND originating_subject_kind IS NULL
            AND originating_subject_id IS NULL
            AND originating_subject_revision IS NULL
        ) OR (
            originating_owner_bundle ~ '^[a-z][a-z0-9-]{0,62}[a-z0-9]$'
            AND originating_subject_kind ~ '^[a-z][a-z0-9._-]{0,126}[a-z0-9]$'
            AND length(originating_subject_id) BETWEEN 1 AND 256
            AND length(originating_subject_revision) BETWEEN 1 AND 256
        )
    );

CREATE INDEX knowledge_objects_originating_subject_idx
    ON knowledge_objects (
        tenant_id,
        originating_owner_bundle,
        originating_subject_kind,
        originating_subject_id,
        originating_subject_revision
    )
    WHERE originating_owner_bundle IS NOT NULL AND status = 'active';

CREATE TABLE knowledge_event_outbox (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                   UUID NOT NULL REFERENCES tenants(id),
    descriptor_id               TEXT NOT NULL,
    event_kind                  TEXT NOT NULL,
    publisher_bundle_id         TEXT NOT NULL,
    subject_kind                TEXT NOT NULL,
    subject_id                  TEXT NOT NULL CHECK (length(subject_id) BETWEEN 1 AND 256),
    subject_revision            TEXT NOT NULL CHECK (length(subject_revision) BETWEEN 1 AND 256),
    snapshot                    JSONB NOT NULL CHECK (jsonb_typeof(snapshot) = 'object'),
    snapshot_hash               TEXT NOT NULL CHECK (snapshot_hash ~ '^sha256:[0-9a-f]{64}$'),
    source_title                TEXT NOT NULL CHECK (length(btrim(source_title)) BETWEEN 1 AND 512),
    source_path_prefix          TEXT NOT NULL CHECK (source_path_prefix ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    acting_space_id             UUID NOT NULL,
    output_vault_bundle         TEXT NOT NULL,
    knowledge_schema_id         TEXT NOT NULL CHECK (length(btrim(knowledge_schema_id)) BETWEEN 1 AND 256),
    researcher_bundle_id        TEXT NOT NULL,
    researcher_role_id          TEXT NOT NULL,
    requested_by_user_id        UUID NOT NULL,
    service_actor_user_id       UUID NOT NULL,
    effective_role              TEXT NOT NULL CHECK (effective_role IN ('contributor', 'curator', 'manager')),
    status                      TEXT NOT NULL DEFAULT 'pending'
                                CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
    attempt_count               INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 3),
    next_attempt_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_owner                 TEXT,
    lease_expires_at            TIMESTAMPTZ,
    last_error                  TEXT CHECK (last_error IS NULL OR length(last_error) <= 1000),
    source_id                   UUID,
    knowledge_job_id            UUID,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at                TIMESTAMPTZ,
    oversight_recorded_at       TIMESTAMPTZ,
    UNIQUE (tenant_id, publisher_bundle_id, event_kind, subject_kind,
            subject_id, subject_revision, researcher_bundle_id, researcher_role_id),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, acting_space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, requested_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, service_actor_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, source_id) REFERENCES knowledge_sources(tenant_id, id),
    FOREIGN KEY (tenant_id, knowledge_job_id) REFERENCES knowledge_jobs(tenant_id, id),
    CHECK (
        (status = 'completed' AND source_id IS NOT NULL AND knowledge_job_id IS NOT NULL
         AND completed_at IS NOT NULL AND lease_owner IS NULL AND lease_expires_at IS NULL)
        OR (status <> 'completed' AND source_id IS NULL AND knowledge_job_id IS NULL
            AND completed_at IS NULL)
    )
);

CREATE INDEX knowledge_event_outbox_claim_idx
    ON knowledge_event_outbox (next_attempt_at, created_at, id)
    WHERE status IN ('pending', 'processing');

CREATE INDEX knowledge_event_outbox_manager_idx
    ON knowledge_event_outbox (tenant_id, status, updated_at DESC);

CREATE INDEX knowledge_event_outbox_oversight_idx
    ON knowledge_event_outbox (updated_at, id)
    WHERE status = 'failed' AND oversight_recorded_at IS NULL;

GRANT SELECT, INSERT, UPDATE ON knowledge_event_outbox TO gadgetron_app;
