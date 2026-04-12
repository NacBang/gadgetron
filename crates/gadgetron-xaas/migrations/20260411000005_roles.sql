-- SEC-3: INSERT-only role for audit log integrity.
-- Uses EXCEPTION handler to handle concurrent CREATE ROLE from parallel tests.
-- PostgreSQL raises unique_violation (23505) on pg_authid, not duplicate_object (42710).
DO $$
BEGIN
    CREATE ROLE gadgetron_app LOGIN;
EXCEPTION
    WHEN duplicate_object OR unique_violation THEN NULL;
END
$$;

GRANT SELECT, INSERT, UPDATE, DELETE ON tenants TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON api_keys TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON quota_configs TO gadgetron_app;
GRANT SELECT, INSERT ON audit_log TO gadgetron_app;
