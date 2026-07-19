-- E4-R3/R3.6: durable, acting-context-bound autonomous goals and attempts.

CREATE TABLE autonomy_goals (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                UUID NOT NULL REFERENCES tenants(id),
    goal_key                 TEXT NOT NULL CHECK (length(goal_key) BETWEEN 1 AND 300),
    source_kind              TEXT NOT NULL DEFAULT 'bundle_schedule'
                                 CHECK (source_kind IN ('bundle_schedule', 'personal', 'manager')),
    status                   TEXT NOT NULL DEFAULT 'context_required'
                                 CHECK (status IN ('context_required', 'ready', 'running', 'retry_wait', 'paused', 'retired', 'safe_stopped')),
    context_state            TEXT NOT NULL DEFAULT 'missing'
                                 CHECK (context_state IN ('ready', 'missing', 'unsupported_space', 'actor_forbidden', 'service_grant_required')),
    goal                     TEXT NOT NULL CHECK (length(btrim(goal)) BETWEEN 1 AND 1000),
    owner_bundle_id          TEXT NOT NULL CHECK (length(owner_bundle_id) BETWEEN 1 AND 128),
    recipe_id                TEXT NOT NULL CHECK (length(recipe_id) BETWEEN 1 AND 128),
    package_manifest_sha256  TEXT NOT NULL CHECK (package_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    target_kind              TEXT NOT NULL CHECK (length(target_kind) BETWEEN 1 AND 64),
    target_id                TEXT NOT NULL CHECK (length(target_id) BETWEEN 1 AND 256),
    target_revision          TEXT NOT NULL CHECK (length(target_revision) BETWEEN 1 AND 256),
    target_label             TEXT NOT NULL CHECK (length(btrim(target_label)) BETWEEN 1 AND 300),
    acting_space_id          UUID,
    requested_by_user_id     UUID,
    service_actor_user_id    UUID,
    effective_role           TEXT CHECK (effective_role IS NULL OR effective_role IN ('viewer', 'contributor', 'curator', 'manager')),
    last_policy_revision     TEXT CHECK (last_policy_revision IS NULL OR length(last_policy_revision) <= 256),
    interval_seconds         INTEGER NOT NULL CHECK (interval_seconds BETWEEN 10 AND 2592000),
    max_wall_seconds         INTEGER NOT NULL CHECK (max_wall_seconds BETWEEN 5 AND 3600),
    attempt                  INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    max_attempts             INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 10),
    next_run_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_owner              TEXT CHECK (lease_owner IS NULL OR length(lease_owner) BETWEEN 1 AND 128),
    lease_expires_at         TIMESTAMPTZ,
    heartbeat_at             TIMESTAMPTZ,
    checkpoint               JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(checkpoint) = 'object'),
    last_outcome             TEXT CHECK (last_outcome IS NULL OR length(last_outcome) <= 64),
    last_verification        TEXT CHECK (last_verification IS NULL OR length(last_verification) <= 2000),
    last_started_at          TIMESTAMPTZ,
    last_finished_at         TIMESTAMPTZ,
    revision                 BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, goal_key),
    FOREIGN KEY (tenant_id, acting_space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, requested_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, service_actor_user_id) REFERENCES users(tenant_id, id),
    CHECK ((status = 'running') = (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)),
    CHECK (context_state <> 'ready' OR (acting_space_id IS NOT NULL AND requested_by_user_id IS NOT NULL AND service_actor_user_id IS NOT NULL AND effective_role IS NOT NULL))
);

CREATE INDEX autonomy_goals_due
    ON autonomy_goals (next_run_at, created_at, id)
    WHERE status IN ('ready', 'retry_wait');
CREATE INDEX autonomy_goals_tenant_history
    ON autonomy_goals (tenant_id, updated_at DESC, id);

CREATE TABLE autonomy_goal_runs (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                UUID NOT NULL,
    goal_id                  UUID NOT NULL,
    attempt                  INTEGER NOT NULL CHECK (attempt BETWEEN 1 AND 10),
    status                   TEXT NOT NULL DEFAULT 'running'
                                 CHECK (status IN ('running', 'succeeded', 'failed', 'interrupted', 'safe_stopped')),
    worker_id                TEXT NOT NULL CHECK (length(worker_id) BETWEEN 1 AND 128),
    acting_space_id          UUID NOT NULL,
    acting_space_revision    BIGINT NOT NULL CHECK (acting_space_revision > 0),
    requested_by_user_id     UUID NOT NULL,
    service_actor_user_id    UUID NOT NULL,
    effective_role           TEXT NOT NULL CHECK (effective_role IN ('contributor', 'curator', 'manager')),
    package_manifest_sha256  TEXT NOT NULL CHECK (package_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    target_revision          TEXT NOT NULL CHECK (length(target_revision) BETWEEN 1 AND 256),
    policy_revision          TEXT CHECK (policy_revision IS NULL OR length(policy_revision) <= 256),
    context_snapshot         JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(context_snapshot) = 'object'),
    checkpoint               JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(checkpoint) = 'object'),
    runtime_job_id           TEXT CHECK (runtime_job_id IS NULL OR length(runtime_job_id) <= 256),
    outcome                  TEXT CHECK (outcome IS NULL OR length(outcome) <= 64),
    verification_state       TEXT CHECK (verification_state IS NULL OR length(verification_state) <= 64),
    verification_summary     TEXT CHECK (verification_summary IS NULL OR length(verification_summary) <= 2000),
    evidence_refs            JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence_refs) = 'array'),
    started_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at              TIMESTAMPTZ,
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, goal_id) REFERENCES autonomy_goals(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, acting_space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, requested_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, service_actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE UNIQUE INDEX autonomy_goal_runs_one_active
    ON autonomy_goal_runs (goal_id) WHERE status = 'running';
CREATE INDEX autonomy_goal_runs_history
    ON autonomy_goal_runs (tenant_id, goal_id, started_at DESC, id);

CREATE TABLE autonomy_goal_events (
    id              BIGSERIAL PRIMARY KEY,
    tenant_id       UUID NOT NULL,
    goal_id         UUID NOT NULL,
    run_id          UUID,
    event_kind      TEXT NOT NULL CHECK (event_kind IN ('context_required', 'ready', 'leased', 'heartbeat', 'context_pinned', 'retry_scheduled', 'succeeded', 'failed', 'interrupted', 'safe_stopped', 'paused', 'retired')),
    actor_user_id   UUID,
    summary         TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 1024),
    details         JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(details) = 'object'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (tenant_id, goal_id) REFERENCES autonomy_goals(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, run_id) REFERENCES autonomy_goal_runs(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX autonomy_goal_events_history
    ON autonomy_goal_events (tenant_id, goal_id, id);

GRANT SELECT, INSERT, UPDATE, DELETE ON autonomy_goals TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON autonomy_goal_runs TO gadgetron_app;
GRANT SELECT, INSERT ON autonomy_goal_events TO gadgetron_app;
GRANT USAGE, SELECT ON SEQUENCE autonomy_goal_events_id_seq TO gadgetron_app;
