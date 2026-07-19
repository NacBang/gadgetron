CREATE TABLE IF NOT EXISTS server_incident_enrichments (
    tenant_id       UUID        NOT NULL,
    id              UUID        NOT NULL,
    incident_id     UUID        NOT NULL,
    subject_revision TEXT       NOT NULL CHECK (
        subject_revision ~ '^[0-9a-f]{64}$'
    ),
    job_id          UUID        NOT NULL,
    subject_snapshot JSONB      NOT NULL CHECK (
        jsonb_typeof(subject_snapshot) = 'object'
    ),
    result          JSONB       NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    result_hash     TEXT        NOT NULL CHECK (result_hash ~ '^[0-9a-f]{64}$'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, incident_id, subject_revision),
    UNIQUE (tenant_id, job_id)
);

CREATE INDEX IF NOT EXISTS server_incident_enrichments_incident_idx
    ON server_incident_enrichments (
        tenant_id,
        incident_id,
        created_at DESC
    );
