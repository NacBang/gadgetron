CREATE TABLE IF NOT EXISTS server_profile_revisions (
    tenant_id    UUID        NOT NULL,
    profile_id   TEXT        NOT NULL CHECK (
        profile_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(profile_id) <= 64
    ),
    revision     UUID        NOT NULL,
    scope        TEXT        NOT NULL CHECK (scope IN ('platform_base', 'cluster', 'role')),
    label        TEXT        NOT NULL CHECK (length(label) BETWEEN 1 AND 120),
    spec         JSONB       NOT NULL CHECK (jsonb_typeof(spec) = 'object'),
    created_by   TEXT        NOT NULL CHECK (length(created_by) BETWEEN 1 AND 128),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, profile_id, revision)
);

CREATE INDEX IF NOT EXISTS server_profile_revisions_created_idx
    ON server_profile_revisions (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS server_cluster_revisions (
    tenant_id               UUID        NOT NULL,
    cluster_id              TEXT        NOT NULL CHECK (
        cluster_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(cluster_id) <= 64
    ),
    revision                UUID        NOT NULL,
    label                   TEXT        NOT NULL CHECK (length(label) BETWEEN 1 AND 120),
    environment             TEXT        NOT NULL CHECK (length(environment) BETWEEN 1 AND 64),
    purpose                 TEXT        NOT NULL CHECK (length(purpose) BETWEEN 1 AND 512),
    base_profile_id         TEXT        NOT NULL,
    base_profile_revision   UUID        NOT NULL,
    cluster_profile_id      TEXT        NOT NULL,
    cluster_profile_revision UUID       NOT NULL,
    roles                   JSONB       NOT NULL CHECK (
        jsonb_typeof(roles) = 'array' AND jsonb_array_length(roles) BETWEEN 1 AND 20
    ),
    created_by              TEXT        NOT NULL CHECK (length(created_by) BETWEEN 1 AND 128),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, cluster_id, revision),
    FOREIGN KEY (tenant_id, base_profile_id, base_profile_revision)
        REFERENCES server_profile_revisions (tenant_id, profile_id, revision),
    FOREIGN KEY (tenant_id, cluster_profile_id, cluster_profile_revision)
        REFERENCES server_profile_revisions (tenant_id, profile_id, revision)
);

CREATE TABLE IF NOT EXISTS server_clusters (
    tenant_id                UUID        NOT NULL,
    cluster_id               TEXT        NOT NULL CHECK (
        cluster_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(cluster_id) <= 64
    ),
    revision                 UUID        NOT NULL,
    label                    TEXT        NOT NULL CHECK (length(label) BETWEEN 1 AND 120),
    environment              TEXT        NOT NULL CHECK (length(environment) BETWEEN 1 AND 64),
    purpose                  TEXT        NOT NULL CHECK (length(purpose) BETWEEN 1 AND 512),
    base_profile_id          TEXT        NOT NULL,
    base_profile_revision    UUID        NOT NULL,
    cluster_profile_id       TEXT        NOT NULL,
    cluster_profile_revision UUID        NOT NULL,
    roles                    JSONB       NOT NULL CHECK (
        jsonb_typeof(roles) = 'array' AND jsonb_array_length(roles) BETWEEN 1 AND 20
    ),
    status                   TEXT        NOT NULL CHECK (status IN ('active', 'paused', 'retired')),
    updated_at               TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, cluster_id),
    FOREIGN KEY (tenant_id, cluster_id, revision)
        REFERENCES server_cluster_revisions (tenant_id, cluster_id, revision)
);

CREATE INDEX IF NOT EXISTS server_clusters_status_idx
    ON server_clusters (tenant_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS server_enrollments (
    tenant_id                 UUID        NOT NULL,
    enrollment_id             UUID        NOT NULL,
    target_id                 TEXT        NOT NULL CHECK (
        target_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(target_id) <= 64
    ),
    cluster_id                TEXT        NOT NULL,
    cluster_revision          UUID        NOT NULL,
    role_id                   TEXT        NOT NULL CHECK (
        role_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(role_id) <= 64
    ),
    base_profile_id           TEXT        NOT NULL,
    base_profile_revision     UUID        NOT NULL,
    cluster_profile_id        TEXT        NOT NULL,
    cluster_profile_revision  UUID        NOT NULL,
    role_profile_id           TEXT        NOT NULL,
    role_profile_revision     UUID        NOT NULL,
    effective_profile         JSONB       NOT NULL CHECK (jsonb_typeof(effective_profile) = 'object'),
    required_commissioning    JSONB       NOT NULL CHECK (jsonb_typeof(required_commissioning) = 'array'),
    required_qualification    JSONB       NOT NULL CHECK (jsonb_typeof(required_qualification) = 'array'),
    lifecycle_state           TEXT        NOT NULL CHECK (lifecycle_state IN (
        'discovered', 'commissioning', 'ready_to_configure', 'configuring', 'qualifying',
        'active', 'draining', 'maintenance', 'quarantined', 'retired'
    )),
    health_status             TEXT        NOT NULL CHECK (
        health_status IN ('unknown', 'healthy', 'degraded', 'unreachable')
    ),
    compliance_status         TEXT        NOT NULL CHECK (
        compliance_status IN ('unknown', 'compliant', 'drift', 'blocked')
    ),
    commissioning_status      TEXT        NOT NULL CHECK (
        commissioning_status IN ('not_configured', 'pending', 'running', 'passed', 'warning', 'failed')
    ),
    qualification_status      TEXT        NOT NULL CHECK (
        qualification_status IN ('not_configured', 'pending', 'running', 'passed', 'warning', 'failed')
    ),
    plan                      JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(plan) = 'object'),
    progress                  JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(progress) = 'object'),
    last_error                JSONB,
    revision                  UUID        NOT NULL,
    created_by                TEXT        NOT NULL CHECK (length(created_by) BETWEEN 1 AND 128),
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL,
    activated_at              TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, enrollment_id),
    FOREIGN KEY (tenant_id, cluster_id, cluster_revision)
        REFERENCES server_cluster_revisions (tenant_id, cluster_id, revision),
    FOREIGN KEY (tenant_id, base_profile_id, base_profile_revision)
        REFERENCES server_profile_revisions (tenant_id, profile_id, revision),
    FOREIGN KEY (tenant_id, cluster_profile_id, cluster_profile_revision)
        REFERENCES server_profile_revisions (tenant_id, profile_id, revision),
    FOREIGN KEY (tenant_id, role_profile_id, role_profile_revision)
        REFERENCES server_profile_revisions (tenant_id, profile_id, revision),
    CHECK (lifecycle_state <> 'active' OR (
        commissioning_status IN ('passed', 'warning') AND
        qualification_status IN ('passed', 'warning')
    )),
    CHECK (last_error IS NULL OR jsonb_typeof(last_error) = 'object')
);

CREATE UNIQUE INDEX IF NOT EXISTS server_enrollments_active_target_idx
    ON server_enrollments (tenant_id, target_id)
    WHERE lifecycle_state <> 'retired';

CREATE INDEX IF NOT EXISTS server_enrollments_cluster_state_idx
    ON server_enrollments (tenant_id, cluster_id, lifecycle_state, updated_at DESC);

CREATE TABLE IF NOT EXISTS server_validation_results (
    tenant_id      UUID        NOT NULL,
    result_id      UUID        NOT NULL,
    enrollment_id  UUID        NOT NULL,
    gate            TEXT        NOT NULL CHECK (gate IN ('commissioning', 'qualification')),
    suite           TEXT        NOT NULL CHECK (
        suite IN ('readiness', 'qualification', 'failure_epilogue', 'burn_in', 'distributed')
    ),
    check_id        TEXT        NOT NULL CHECK (
        check_id ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$' AND length(check_id) <= 64
    ),
    status          TEXT        NOT NULL CHECK (
        status IN ('pass', 'warning', 'fail', 'skipped', 'not_applicable')
    ),
    required        BOOLEAN     NOT NULL,
    summary         TEXT        NOT NULL CHECK (length(summary) BETWEEN 1 AND 512),
    details         JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(details) = 'object'),
    observed_at     TIMESTAMPTZ NOT NULL,
    recorded_by     TEXT        NOT NULL CHECK (length(recorded_by) BETWEEN 1 AND 128),
    PRIMARY KEY (tenant_id, result_id),
    FOREIGN KEY (tenant_id, enrollment_id)
        REFERENCES server_enrollments (tenant_id, enrollment_id)
);

CREATE INDEX IF NOT EXISTS server_validation_results_gate_idx
    ON server_validation_results (tenant_id, enrollment_id, gate, observed_at DESC);
