CREATE TABLE IF NOT EXISTS tenants (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       VARCHAR(255) NOT NULL,
    status     VARCHAR(16)  NOT NULL DEFAULT 'Active'
                   CHECK (status IN ('Active', 'Suspended', 'Deleted')),
    created_at TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS tenants_status_idx ON tenants(status);
