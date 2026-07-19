-- R3.4a: immutable deployment-wide ontology revisions and package provenance.
--
-- Registration records exact signed schema bytes and their normalized SDK
-- contract. Tenant activation and object classification are separate lifecycle
-- tables added by the following Knowledge Evolution Kernel slice.

CREATE TABLE knowledge_ontology_revisions (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_bundle_id     TEXT NOT NULL
                            CHECK (owner_bundle_id ~ '^[a-z0-9]+(-[a-z0-9]+)*$'
                                   AND length(owner_bundle_id) <= 64),
    schema_id           TEXT NOT NULL
                            CHECK (schema_id ~ '^[a-z0-9]+(-[a-z0-9]+)*$'
                                   AND length(schema_id) <= 64),
    schema_version      INTEGER NOT NULL CHECK (schema_version > 0),
    schema_sha256       TEXT NOT NULL CHECK (schema_sha256 ~ '^[0-9a-f]{64}$'),
    format_version      INTEGER NOT NULL CHECK (format_version = 1),
    legacy_adapter      BOOLEAN NOT NULL DEFAULT FALSE,
    schema_bytes        BYTEA NOT NULL CHECK (octet_length(schema_bytes) > 0),
    normalized_ontology JSONB NOT NULL
                            CHECK (jsonb_typeof(normalized_ontology) = 'object'),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (owner_bundle_id, schema_id, schema_version),
    UNIQUE (id, owner_bundle_id, schema_id)
);

CREATE INDEX knowledge_ontology_revisions_digest_idx
    ON knowledge_ontology_revisions (schema_sha256);

CREATE TABLE knowledge_ontology_package_provenance (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    ontology_revision_id    UUID NOT NULL REFERENCES knowledge_ontology_revisions(id),
    package_version         TEXT NOT NULL
                                CHECK (length(btrim(package_version)) BETWEEN 1 AND 128),
    package_manifest_sha256 TEXT NOT NULL
                                CHECK (package_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    registered_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (ontology_revision_id, package_manifest_sha256)
);

CREATE INDEX knowledge_ontology_package_digest_idx
    ON knowledge_ontology_package_provenance (package_manifest_sha256);

GRANT SELECT, INSERT ON knowledge_ontology_revisions TO gadgetron_app;
GRANT SELECT, INSERT ON knowledge_ontology_package_provenance TO gadgetron_app;
