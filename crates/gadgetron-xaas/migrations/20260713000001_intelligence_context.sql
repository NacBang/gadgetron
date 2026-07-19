-- R3.4a: immutable Core-mediated context reuse and verified outcome feedback.

CREATE TABLE knowledge_context_exchanges (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id               UUID NOT NULL REFERENCES tenants(id),
    actor_user_id           UUID NOT NULL,
    consumer_bundle_id      TEXT NOT NULL
                                CHECK (consumer_bundle_id ~ '^[a-z][a-z0-9-]{1,63}$'),
    query_id                TEXT NOT NULL CHECK (length(btrim(query_id)) BETWEEN 1 AND 256),
    subject_owner_bundle    TEXT NOT NULL
                                CHECK (subject_owner_bundle ~ '^[a-z][a-z0-9-]{1,63}$'),
    subject_kind            TEXT NOT NULL CHECK (length(btrim(subject_kind)) BETWEEN 1 AND 160),
    subject_stable_id       TEXT NOT NULL CHECK (length(btrim(subject_stable_id)) BETWEEN 1 AND 256),
    subject_revision        TEXT NOT NULL CHECK (length(btrim(subject_revision)) BETWEEN 1 AND 256),
    question                TEXT NOT NULL CHECK (length(btrim(question)) BETWEEN 1 AND 2048),
    context_revision        TEXT NOT NULL CHECK (length(btrim(context_revision)) BETWEEN 1 AND 256),
    coverage                TEXT NOT NULL CHECK (coverage IN ('complete', 'partial', 'unavailable')),
    citation_count          INTEGER NOT NULL CHECK (citation_count BETWEEN 0 AND 256),
    gap_count               INTEGER NOT NULL CHECK (gap_count BETWEEN 0 AND 128),
    query_json              JSONB NOT NULL CHECK (jsonb_typeof(query_json) = 'object'),
    pack_json               JSONB NOT NULL CHECK (jsonb_typeof(pack_json) = 'object'),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, consumer_bundle_id, query_id),
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX knowledge_context_exchanges_subject_idx
    ON knowledge_context_exchanges
       (tenant_id, subject_owner_bundle, subject_kind, subject_stable_id, created_at DESC);

CREATE TABLE knowledge_outcome_feedback (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id               UUID NOT NULL REFERENCES tenants(id),
    actor_user_id           UUID NOT NULL,
    consumer_bundle_id      TEXT NOT NULL
                                CHECK (consumer_bundle_id ~ '^[a-z][a-z0-9-]{1,63}$'),
    feedback_id             TEXT NOT NULL CHECK (length(btrim(feedback_id)) BETWEEN 1 AND 256),
    experience_revision     TEXT NOT NULL CHECK (experience_revision ~ '^sha256:[0-9a-f]{64}$'),
    subject_owner_bundle    TEXT NOT NULL
                                CHECK (subject_owner_bundle ~ '^[a-z][a-z0-9-]{1,63}$'),
    subject_kind            TEXT NOT NULL CHECK (length(btrim(subject_kind)) BETWEEN 1 AND 160),
    subject_stable_id       TEXT NOT NULL CHECK (length(btrim(subject_stable_id)) BETWEEN 1 AND 256),
    subject_revision        TEXT NOT NULL CHECK (length(btrim(subject_revision)) BETWEEN 1 AND 256),
    operation_id            TEXT NOT NULL CHECK (length(btrim(operation_id)) BETWEEN 1 AND 256),
    context_query_id        TEXT,
    context_revision        TEXT,
    predicate_result        TEXT NOT NULL
                                CHECK (predicate_result IN ('satisfied', 'failed', 'indeterminate')),
    verification_summary    TEXT NOT NULL
                                CHECK (length(btrim(verification_summary)) BETWEEN 1 AND 2048),
    before_state            JSONB NOT NULL CHECK (jsonb_typeof(before_state) = 'object'),
    after_state             JSONB NOT NULL CHECK (jsonb_typeof(after_state) = 'object'),
    used_citations          JSONB NOT NULL DEFAULT '[]'::JSONB
                                CHECK (jsonb_typeof(used_citations) = 'array'),
    feedback_json           JSONB NOT NULL CHECK (jsonb_typeof(feedback_json) = 'object'),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, consumer_bundle_id, feedback_id),
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id),
    FOREIGN KEY (tenant_id, consumer_bundle_id, context_query_id)
        REFERENCES knowledge_context_exchanges(tenant_id, consumer_bundle_id, query_id),
    CHECK ((context_query_id IS NULL AND context_revision IS NULL)
           OR (context_query_id IS NOT NULL AND context_revision IS NOT NULL))
);

CREATE INDEX knowledge_outcome_feedback_subject_idx
    ON knowledge_outcome_feedback
       (tenant_id, subject_owner_bundle, subject_kind, subject_stable_id, created_at DESC);

GRANT SELECT, INSERT ON knowledge_context_exchanges TO gadgetron_app;
GRANT SELECT, INSERT ON knowledge_outcome_feedback TO gadgetron_app;
