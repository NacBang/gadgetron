-- E4-R2/R2.5: durable Researcher/Gardener jobs and reviewable change sets.

CREATE TABLE knowledge_jobs (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                UUID NOT NULL REFERENCES tenants(id),
    space_id                 UUID NOT NULL,
    output_vault_id          UUID NOT NULL,
    role                     TEXT NOT NULL CHECK (role IN ('researcher', 'gardener')),
    kind                     TEXT NOT NULL CHECK (kind IN ('on_demand', 'source_ingest', 'scheduled', 'follow_up')),
    status                   TEXT NOT NULL DEFAULT 'queued'
                                 CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    priority                 SMALLINT NOT NULL DEFAULT 0 CHECK (priority BETWEEN -100 AND 100),
    service_actor_user_id    UUID NOT NULL,
    requested_by_user_id     UUID NOT NULL,
    on_behalf_of_user_id     UUID,
    input                    JSONB NOT NULL CHECK (jsonb_typeof(input) = 'object'),
    input_hash               TEXT NOT NULL CHECK (input_hash ~ '^[0-9a-f]{64}$'),
    idempotency_key          TEXT NOT NULL CHECK (length(idempotency_key) BETWEEN 1 AND 200),
    runtime_backend          TEXT NOT NULL CHECK (runtime_backend IN ('claude_code', 'codex_exec')),
    runtime_model            TEXT NOT NULL DEFAULT '' CHECK (length(runtime_model) <= 200),
    runtime_effort           TEXT NOT NULL CHECK (runtime_effort IN ('low', 'medium', 'high', 'xhigh', 'max', 'ultra')),
    runtime_endpoint_id      UUID,
    runtime_model_source     TEXT NOT NULL CHECK (runtime_model_source IN ('default', 'local')),
    runtime_local_base_url   TEXT NOT NULL DEFAULT '' CHECK (length(runtime_local_base_url) <= 2048),
    runtime_local_api_key_env TEXT NOT NULL DEFAULT '' CHECK (length(runtime_local_api_key_env) <= 128),
    prompt_contract_revision TEXT NOT NULL CHECK (length(prompt_contract_revision) BETWEEN 1 AND 64),
    tool_policy_revision     TEXT NOT NULL CHECK (length(tool_policy_revision) BETWEEN 1 AND 64),
    max_tokens               INTEGER NOT NULL CHECK (max_tokens BETWEEN 256 AND 200000),
    max_sources              INTEGER NOT NULL CHECK (max_sources BETWEEN 1 AND 100),
    max_wall_seconds         INTEGER NOT NULL CHECK (max_wall_seconds BETWEEN 5 AND 3600),
    used_tokens              INTEGER NOT NULL DEFAULT 0 CHECK (used_tokens >= 0),
    used_sources             INTEGER NOT NULL DEFAULT 0 CHECK (used_sources >= 0),
    progress_percent         SMALLINT NOT NULL DEFAULT 0 CHECK (progress_percent BETWEEN 0 AND 100),
    checkpoint               JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(checkpoint) = 'object'),
    attempt                  INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    max_attempts             INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 10),
    scheduled_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_owner              TEXT CHECK (lease_owner IS NULL OR length(lease_owner) BETWEEN 1 AND 128),
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
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, output_vault_id) REFERENCES knowledge_vaults(tenant_id, id),
    FOREIGN KEY (tenant_id, service_actor_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, requested_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, on_behalf_of_user_id) REFERENCES users(tenant_id, id),
    CHECK ((status = 'running') = (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)),
    CHECK (on_behalf_of_user_id IS NOT NULL OR requested_by_user_id = service_actor_user_id),
    CHECK ((runtime_model_source = 'local') = (runtime_endpoint_id IS NOT NULL))
);

CREATE UNIQUE INDEX knowledge_jobs_active_idempotency
    ON knowledge_jobs (tenant_id, idempotency_key)
    WHERE status IN ('queued', 'running');
CREATE INDEX knowledge_jobs_claim
    ON knowledge_jobs (status, scheduled_at, priority DESC, created_at)
    WHERE status IN ('queued', 'running');
CREATE INDEX knowledge_jobs_space_history
    ON knowledge_jobs (tenant_id, space_id, created_at DESC);

CREATE TABLE knowledge_job_sources (
    tenant_id       UUID NOT NULL,
    job_id          UUID NOT NULL,
    source_id       UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    content_hash    TEXT NOT NULL CHECK (content_hash ~ '^sha256:[0-9a-f]{64}$'),
    object_id       UUID NOT NULL,
    object_revision BIGINT NOT NULL CHECK (object_revision > 0),
    object_content_hash TEXT NOT NULL CHECK (object_content_hash ~ '^[0-9a-f]{64}$'),
    position        SMALLINT NOT NULL CHECK (position BETWEEN 0 AND 99),
    PRIMARY KEY (tenant_id, job_id, source_id),
    UNIQUE (job_id, position),
    FOREIGN KEY (tenant_id, job_id) REFERENCES knowledge_jobs(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, source_id) REFERENCES knowledge_sources(tenant_id, id),
    FOREIGN KEY (tenant_id, object_id) REFERENCES knowledge_objects(tenant_id, id)
);

CREATE TABLE knowledge_job_artifacts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    job_id          UUID NOT NULL,
    space_id        UUID NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('dossier', 'partial_dossier', 'candidate', 'agent_output')),
    title           TEXT NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 300),
    summary         TEXT NOT NULL DEFAULT '' CHECK (length(summary) <= 4000),
    payload         JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    citations       JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(citations) = 'array'),
    content_hash    TEXT NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    UNIQUE (job_id, kind, content_hash),
    FOREIGN KEY (tenant_id, job_id) REFERENCES knowledge_jobs(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id)
);

CREATE INDEX knowledge_job_artifacts_job
    ON knowledge_job_artifacts (tenant_id, job_id, created_at);

CREATE TABLE knowledge_change_sets (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id             UUID NOT NULL,
    job_id                UUID NOT NULL,
    space_id              UUID NOT NULL,
    output_vault_id       UUID NOT NULL,
    status                TEXT NOT NULL DEFAULT 'proposed'
                              CHECK (status IN ('proposed', 'pending_user_review', 'accepted', 'materializing', 'applied', 'rejected', 'failed_retryable')),
    title                 TEXT NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 300),
    summary               TEXT NOT NULL DEFAULT '' CHECK (length(summary) <= 4000),
    operations            JSONB NOT NULL CHECK (jsonb_typeof(operations) = 'array'),
    citations             JSONB NOT NULL CHECK (jsonb_typeof(citations) = 'array'),
    created_by_user_id    UUID NOT NULL,
    decided_by_user_id    UUID,
    decision_rationale    TEXT CHECK (decision_rationale IS NULL OR length(decision_rationale) <= 2000),
    expected_git_revision TEXT CHECK (expected_git_revision IS NULL OR length(expected_git_revision) <= 128),
    applied_git_revision  TEXT CHECK (applied_git_revision IS NULL OR length(applied_git_revision) <= 128),
    materialized_object_id UUID,
    materialization_receipt JSONB CHECK (materialization_receipt IS NULL OR jsonb_typeof(materialization_receipt) = 'object'),
    materialization_key   TEXT NOT NULL CHECK (length(materialization_key) BETWEEN 1 AND 200),
    revision              BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    decided_at            TIMESTAMPTZ,
    applied_at            TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, materialization_key),
    UNIQUE (job_id),
    FOREIGN KEY (tenant_id, job_id) REFERENCES knowledge_jobs(tenant_id, id),
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, output_vault_id) REFERENCES knowledge_vaults(tenant_id, id),
    FOREIGN KEY (tenant_id, created_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, decided_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, materialized_object_id) REFERENCES knowledge_objects(tenant_id, id)
);

CREATE INDEX knowledge_change_sets_review
    ON knowledge_change_sets (tenant_id, space_id, status, created_at DESC);

CREATE TABLE knowledge_job_events (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   UUID NOT NULL,
    job_id      UUID NOT NULL,
    event_kind  TEXT NOT NULL CHECK (event_kind IN ('queued', 'leased', 'heartbeat', 'checkpoint', 'cancel_requested', 'retry_queued', 'succeeded', 'failed', 'cancelled')),
    actor_user_id UUID NOT NULL,
    summary     TEXT NOT NULL DEFAULT '' CHECK (length(summary) <= 1024),
    details     JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(details) = 'object'),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (tenant_id, job_id) REFERENCES knowledge_jobs(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX knowledge_job_events_job
    ON knowledge_job_events (tenant_id, job_id, id);

GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_jobs TO gadgetron_app;
GRANT SELECT, INSERT, DELETE ON knowledge_job_sources TO gadgetron_app;
GRANT SELECT, INSERT ON knowledge_job_artifacts TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON knowledge_change_sets TO gadgetron_app;
GRANT SELECT, INSERT ON knowledge_job_events TO gadgetron_app;
GRANT USAGE, SELECT ON SEQUENCE knowledge_job_events_id_seq TO gadgetron_app;
