-- SEC-3: INSERT-only role for audit log integrity.
-- Uses EXCEPTION handler instead of IF NOT EXISTS to avoid race conditions
-- when multiple test databases run migrations in parallel.
DO $$
BEGIN
    CREATE ROLE gadgetron_app LOGIN;
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

GRANT SELECT, INSERT, UPDATE, DELETE ON tenants TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON api_keys TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON quota_configs TO gadgetron_app;
GRANT SELECT, INSERT ON audit_log TO gadgetron_app;
