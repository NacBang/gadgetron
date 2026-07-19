-- Preserve one idempotent operational dispatch edge for each durable incident
-- revision. Core attaches the optional Intelligence event to the insert; the
-- incident row remains authoritative when no provider is enabled.

CREATE TABLE IF NOT EXISTS server_incident_enrichment_dispatches (
    tenant_id         UUID        NOT NULL,
    dispatch_id       UUID        NOT NULL DEFAULT gen_random_uuid(),
    incident_id       UUID        NOT NULL,
    database_revision UUID        NOT NULL,
    subject_revision  TEXT        NOT NULL CHECK (
        subject_revision ~ '^[0-9a-f]{64}$'
    ),
    emitted_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, dispatch_id),
    UNIQUE (tenant_id, incident_id, subject_revision),
    FOREIGN KEY (tenant_id, incident_id)
        REFERENCES server_incidents (tenant_id, incident_id)
);

CREATE INDEX IF NOT EXISTS server_incident_enrichment_dispatches_incident_idx
    ON server_incident_enrichment_dispatches (
        tenant_id,
        incident_id,
        emitted_at DESC
    );

COMMENT ON TABLE server_incident_enrichment_dispatches IS
    'Operational incident revision dispatch ledger; Intelligence results remain provider-owned and optional.';
