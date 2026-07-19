-- R3.4a: explicit tenant ontology activation and append-only object mapping.
-- Effective state is always the latest event; history rows are never updated.

CREATE TABLE knowledge_ontology_activation_events (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id               UUID NOT NULL REFERENCES tenants(id),
    ontology_revision_id    UUID NOT NULL,
    owner_bundle_id         TEXT NOT NULL,
    schema_id               TEXT NOT NULL,
    activation_revision     BIGINT NOT NULL CHECK (activation_revision > 0),
    action                  TEXT NOT NULL CHECK (action IN ('activate', 'deactivate')),
    actor_user_id           UUID NOT NULL,
    reason                  TEXT NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 2048),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, owner_bundle_id, schema_id, activation_revision),
    FOREIGN KEY (ontology_revision_id, owner_bundle_id, schema_id)
        REFERENCES knowledge_ontology_revisions(id, owner_bundle_id, schema_id),
    FOREIGN KEY (tenant_id, actor_user_id) REFERENCES users(tenant_id, id)
);

CREATE INDEX knowledge_ontology_activation_latest_idx
    ON knowledge_ontology_activation_events
       (tenant_id, owner_bundle_id, schema_id, activation_revision DESC);

CREATE TABLE knowledge_ontology_mapping_events (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id               UUID NOT NULL,
    object_id               UUID NOT NULL,
    object_revision         BIGINT NOT NULL CHECK (object_revision > 0),
    mapping_revision        BIGINT NOT NULL CHECK (mapping_revision > 0),
    disposition             TEXT NOT NULL
                                CHECK (disposition IN ('proposed', 'active', 'unmapped')),
    ontology_revision_id    UUID REFERENCES knowledge_ontology_revisions(id),
    type_id                 TEXT CHECK (type_id IS NULL OR length(type_id) BETWEEN 1 AND 128),
    confidence              REAL CHECK (confidence IS NULL OR confidence BETWEEN 0.0 AND 1.0),
    evidence                JSONB NOT NULL DEFAULT '{}'::JSONB
                                CHECK (jsonb_typeof(evidence) = 'object'),
    reason                  TEXT NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 2048),
    recorded_by             UUID NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, object_id, mapping_revision),
    FOREIGN KEY (tenant_id, object_id) REFERENCES knowledge_objects(tenant_id, id),
    FOREIGN KEY (tenant_id, recorded_by) REFERENCES users(tenant_id, id),
    CHECK (
        (disposition IN ('proposed', 'active')
         AND ontology_revision_id IS NOT NULL AND type_id IS NOT NULL)
        OR
        (disposition = 'unmapped'
         AND ontology_revision_id IS NULL AND type_id IS NULL)
    )
);

CREATE INDEX knowledge_ontology_mapping_latest_idx
    ON knowledge_ontology_mapping_events
       (tenant_id, object_id, mapping_revision DESC);
CREATE INDEX knowledge_ontology_mapping_revision_idx
    ON knowledge_ontology_mapping_events
       (tenant_id, ontology_revision_id, type_id, disposition);

GRANT SELECT, INSERT ON knowledge_ontology_activation_events TO gadgetron_app;
GRANT SELECT, INSERT ON knowledge_ontology_mapping_events TO gadgetron_app;
