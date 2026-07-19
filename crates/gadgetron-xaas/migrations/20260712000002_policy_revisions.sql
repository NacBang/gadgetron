-- E4-R3/R3.2a: tenant-scoped immutable autonomy policy revisions and
-- deterministic decision events. Runtime enforcement is R3.2b.

CREATE TABLE policy_revisions (
    tenant_id        UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    policy_id        UUID        NOT NULL,
    revision         BIGINT      NOT NULL CHECK (revision > 0),
    schema_version   INTEGER     NOT NULL CHECK (schema_version > 0),
    source           TEXT        NOT NULL CHECK (
        source IN ('legacy_migration', 'manager', 'rollback', 'system')
    ),
    document         JSONB       NOT NULL CHECK (jsonb_typeof(document) = 'object'),
    document_hash    TEXT        NOT NULL CHECK (document_hash ~ '^sha256:[0-9a-f]{64}$'),
    legacy_modes     JSONB       NULL CHECK (
        legacy_modes IS NULL OR jsonb_typeof(legacy_modes) = 'object'
    ),
    created_by       UUID        NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    superseded_at    TIMESTAMPTZ NULL,
    PRIMARY KEY (tenant_id, policy_id, revision)
);

CREATE UNIQUE INDEX policy_revisions_one_active_per_tenant
    ON policy_revisions (tenant_id)
    WHERE superseded_at IS NULL;

CREATE INDEX policy_revisions_tenant_history
    ON policy_revisions (tenant_id, revision DESC);

CREATE TABLE policy_decision_events (
    event_id          UUID        PRIMARY KEY,
    tenant_id         UUID        NOT NULL,
    policy_id         UUID        NOT NULL,
    policy_revision   BIGINT      NOT NULL,
    policy_hash       TEXT        NOT NULL CHECK (policy_hash ~ '^sha256:[0-9a-f]{64}$'),
    input             JSONB       NOT NULL CHECK (jsonb_typeof(input) = 'object'),
    input_hash        TEXT        NOT NULL CHECK (input_hash ~ '^sha256:[0-9a-f]{64}$'),
    trace             JSONB       NOT NULL CHECK (jsonb_typeof(trace) = 'object'),
    trace_hash        TEXT        NOT NULL CHECK (trace_hash ~ '^sha256:[0-9a-f]{64}$'),
    decision          TEXT        NOT NULL CHECK (decision IN ('auto', 'review', 'deny')),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (tenant_id, policy_id, policy_revision)
        REFERENCES policy_revisions (tenant_id, policy_id, revision)
        ON DELETE RESTRICT
);

CREATE INDEX policy_decision_events_tenant_created
    ON policy_decision_events (tenant_id, created_at DESC, event_id DESC);
