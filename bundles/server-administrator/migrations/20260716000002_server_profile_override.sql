ALTER TABLE server_profile_revisions
    DROP CONSTRAINT IF EXISTS server_profile_revisions_scope_check;

ALTER TABLE server_profile_revisions
    ADD CONSTRAINT server_profile_revisions_scope_check
    CHECK (scope IN ('platform_base', 'cluster', 'role', 'server'));

ALTER TABLE server_enrollments
    ADD COLUMN IF NOT EXISTS server_profile_id TEXT,
    ADD COLUMN IF NOT EXISTS server_profile_revision UUID;

ALTER TABLE server_enrollments
    ADD CONSTRAINT server_enrollments_server_profile_pair_check
    CHECK ((server_profile_id IS NULL) = (server_profile_revision IS NULL));

ALTER TABLE server_enrollments
    ADD CONSTRAINT server_enrollments_server_profile_revision_fkey
    FOREIGN KEY (tenant_id, server_profile_id, server_profile_revision)
    REFERENCES server_profile_revisions (tenant_id, profile_id, revision);
