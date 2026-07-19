-- E4-R3/R3.5: durable Manager outcome ledger, corrective directives, and
-- terminal-exception webhook delivery audit.

CREATE TABLE manager_oversight_records (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id            UUID NOT NULL REFERENCES tenants(id),
    source_kind          TEXT NOT NULL CHECK (source_kind IN (
                             'workbench_action', 'bundle_job', 'knowledge_job', 'directive'
                         )),
    source_id            TEXT NOT NULL CHECK (length(source_id) BETWEEN 1 AND 300),
    actor_user_id        UUID,
    agent_label          TEXT NOT NULL CHECK (length(agent_label) BETWEEN 1 AND 200),
    agent_role           TEXT NOT NULL CHECK (length(agent_role) BETWEEN 1 AND 100),
    goal                 TEXT NOT NULL CHECK (length(goal) BETWEEN 1 AND 2000),
    target_kind          TEXT NOT NULL CHECK (target_kind IN (
                             'action', 'job', 'configuration', 'knowledge_revision'
                         )),
    target_id            TEXT NOT NULL CHECK (length(target_id) BETWEEN 1 AND 512),
    target_revision      TEXT CHECK (target_revision IS NULL OR length(target_revision) BETWEEN 1 AND 256),
    policy_decision      TEXT NOT NULL CHECK (policy_decision IN ('auto', 'review', 'deny', 'unknown')),
    policy_revision      TEXT CHECK (policy_revision IS NULL OR length(policy_revision) BETWEEN 1 AND 256),
    evidence_refs        JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence_refs) = 'array'),
    current_stage        TEXT NOT NULL CHECK (current_stage IN ('target', 'plan', 'execute', 'verify')),
    outcome              TEXT NOT NULL CHECK (outcome IN (
                             'pending', 'pending_review', 'succeeded', 'failed',
                             'safe_stopped', 'cancelled'
                         )),
    verification_state   TEXT NOT NULL CHECK (verification_state IN (
                             'pending', 'verified', 'failed', 'not_provided'
                         )),
    action_summary       TEXT NOT NULL CHECK (length(action_summary) BETWEEN 1 AND 2000),
    before_summary       TEXT CHECK (before_summary IS NULL OR length(before_summary) <= 4000),
    after_summary        TEXT CHECK (after_summary IS NULL OR length(after_summary) <= 4000),
    rollback_summary     TEXT CHECK (rollback_summary IS NULL OR length(rollback_summary) <= 4000),
    duration_ms          BIGINT NOT NULL DEFAULT 0 CHECK (duration_ms >= 0),
    cost_minor_units     BIGINT NOT NULL DEFAULT 0 CHECK (cost_minor_units >= 0),
    revision             BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at          TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, source_kind, source_id),
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX manager_oversight_tenant_created
    ON manager_oversight_records (tenant_id, created_at DESC, id DESC);
CREATE INDEX manager_oversight_attention
    ON manager_oversight_records (tenant_id, outcome, created_at DESC)
    WHERE outcome IN ('failed', 'safe_stopped', 'cancelled');

CREATE TABLE manager_oversight_events (
    id                   BIGSERIAL PRIMARY KEY,
    tenant_id            UUID NOT NULL,
    oversight_id         UUID NOT NULL,
    stage                TEXT NOT NULL CHECK (stage IN ('target', 'plan', 'execute', 'verify')),
    state                TEXT NOT NULL CHECK (state IN ('recorded', 'started', 'completed', 'failed', 'skipped')),
    summary              TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 2000),
    evidence_refs        JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence_refs) = 'array'),
    actor_user_id        UUID,
    occurred_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (tenant_id, oversight_id)
        REFERENCES manager_oversight_records(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX manager_oversight_events_record
    ON manager_oversight_events (tenant_id, oversight_id, id);

CREATE TABLE manager_directives (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id            UUID NOT NULL REFERENCES tenants(id),
    oversight_id         UUID NOT NULL,
    target_kind          TEXT NOT NULL CHECK (target_kind IN (
                             'action', 'job', 'configuration', 'knowledge_revision'
                         )),
    target_id            TEXT NOT NULL CHECK (length(target_id) BETWEEN 1 AND 512),
    target_revision      TEXT CHECK (target_revision IS NULL OR length(target_revision) BETWEEN 1 AND 256),
    issued_by_user_id    UUID NOT NULL,
    instruction          TEXT NOT NULL CHECK (length(instruction) BETWEEN 1 AND 4000),
    desired_outcome      TEXT NOT NULL CHECK (length(desired_outcome) BETWEEN 1 AND 2000),
    constraints          JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(constraints) = 'array'),
    priority             TEXT NOT NULL CHECK (priority IN ('normal', 'urgent')),
    state                TEXT NOT NULL CHECK (state IN (
                             'issued', 'acknowledged', 'planned', 'executing', 'verifying',
                             'resolved', 'failed', 'escalated'
                         )),
    plan_summary         TEXT CHECK (plan_summary IS NULL OR length(plan_summary) <= 4000),
    execution_summary    TEXT CHECK (execution_summary IS NULL OR length(execution_summary) <= 4000),
    verification_summary TEXT CHECK (verification_summary IS NULL OR length(verification_summary) <= 4000),
    before_summary       TEXT CHECK (before_summary IS NULL OR length(before_summary) <= 4000),
    after_summary        TEXT CHECK (after_summary IS NULL OR length(after_summary) <= 4000),
    evidence_refs        JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence_refs) = 'array'),
    due_at               TIMESTAMPTZ,
    revision             BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at          TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, oversight_id),
    FOREIGN KEY (tenant_id, oversight_id)
        REFERENCES manager_oversight_records(tenant_id, id),
    FOREIGN KEY (tenant_id, issued_by_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX manager_directives_tenant_state
    ON manager_directives (tenant_id, state, priority DESC, created_at DESC);

CREATE TABLE manager_directive_events (
    id                   BIGSERIAL PRIMARY KEY,
    tenant_id            UUID NOT NULL,
    directive_id         UUID NOT NULL,
    state                TEXT NOT NULL CHECK (state IN (
                             'issued', 'acknowledged', 'planned', 'executing', 'verifying',
                             'resolved', 'failed', 'escalated'
                         )),
    summary              TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 2000),
    actor_user_id        UUID NOT NULL,
    occurred_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (tenant_id, directive_id)
        REFERENCES manager_directives(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX manager_directive_events_directive
    ON manager_directive_events (tenant_id, directive_id, id);

CREATE TABLE manager_exceptions (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id            UUID NOT NULL REFERENCES tenants(id),
    oversight_id         UUID NOT NULL,
    directive_id         UUID,
    severity             TEXT NOT NULL CHECK (severity IN ('warning', 'error', 'critical')),
    summary              TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 300),
    state                TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'acknowledged', 'resolved')),
    acknowledged_by_user_id UUID,
    resolved_by_user_id  UUID,
    revision             BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    occurred_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    acknowledged_at      TIMESTAMPTZ,
    resolved_at          TIMESTAMPTZ,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, oversight_id),
    FOREIGN KEY (tenant_id, oversight_id)
        REFERENCES manager_oversight_records(tenant_id, id),
    FOREIGN KEY (tenant_id, directive_id) REFERENCES manager_directives(tenant_id, id),
    FOREIGN KEY (tenant_id, acknowledged_by_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, resolved_by_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX manager_exceptions_inbox
    ON manager_exceptions (tenant_id, state, severity DESC, occurred_at DESC);

CREATE TABLE manager_exception_events (
    id                   BIGSERIAL PRIMARY KEY,
    tenant_id            UUID NOT NULL,
    exception_id         UUID NOT NULL,
    state                TEXT NOT NULL CHECK (state IN ('open', 'acknowledged', 'resolved')),
    actor_user_id        UUID,
    summary              TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 1000),
    occurred_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (tenant_id, exception_id)
        REFERENCES manager_exceptions(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE TABLE manager_webhook_settings (
    tenant_id            UUID PRIMARY KEY REFERENCES tenants(id),
    enabled              BOOLEAN NOT NULL DEFAULT FALSE,
    endpoint_url         TEXT CHECK (endpoint_url IS NULL OR length(endpoint_url) BETWEEN 1 AND 4096),
    destination_host     TEXT CHECK (destination_host IS NULL OR length(destination_host) BETWEEN 1 AND 255),
    review_base_url      TEXT CHECK (review_base_url IS NULL OR length(review_base_url) BETWEEN 1 AND 2048),
    updated_by_user_id   UUID NOT NULL,
    revision             BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (tenant_id, updated_by_user_id) REFERENCES users(tenant_id, id),
    CHECK (NOT enabled OR (endpoint_url IS NOT NULL AND destination_host IS NOT NULL AND review_base_url IS NOT NULL))
);

CREATE TABLE manager_webhook_deliveries (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id            UUID NOT NULL REFERENCES tenants(id),
    exception_id         UUID NOT NULL,
    idempotency_key      TEXT NOT NULL CHECK (length(idempotency_key) BETWEEN 1 AND 200),
    state                TEXT NOT NULL CHECK (state IN (
                             'pending', 'sent', 'failed_retryable', 'failed_terminal'
                         )),
    attempt_count        INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 4),
    next_attempt_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_http_status     INTEGER,
    last_error_code      TEXT CHECK (last_error_code IS NULL OR length(last_error_code) <= 100),
    delivered_at         TIMESTAMPTZ,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, exception_id),
    UNIQUE (tenant_id, idempotency_key),
    FOREIGN KEY (tenant_id, exception_id)
        REFERENCES manager_exceptions(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX manager_webhook_delivery_queue
    ON manager_webhook_deliveries (state, next_attempt_at, created_at)
    WHERE state IN ('pending', 'failed_retryable');

GRANT SELECT, INSERT, UPDATE ON manager_oversight_records TO gadgetron_app;
GRANT SELECT, INSERT ON manager_oversight_events TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON manager_directives TO gadgetron_app;
GRANT SELECT, INSERT ON manager_directive_events TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON manager_exceptions TO gadgetron_app;
GRANT SELECT, INSERT ON manager_exception_events TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON manager_webhook_settings TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE ON manager_webhook_deliveries TO gadgetron_app;
GRANT USAGE, SELECT ON SEQUENCE manager_oversight_events_id_seq TO gadgetron_app;
GRANT USAGE, SELECT ON SEQUENCE manager_directive_events_id_seq TO gadgetron_app;
GRANT USAGE, SELECT ON SEQUENCE manager_exception_events_id_seq TO gadgetron_app;
