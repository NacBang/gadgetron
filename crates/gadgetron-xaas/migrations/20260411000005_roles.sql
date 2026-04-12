-- SEC-3: INSERT-only role for audit log integrity.
-- Requires the application to connect as 'gadgetron_app' role.
-- Run with a superuser or rds_superuser connection.
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'gadgetron_app') THEN
        CREATE ROLE gadgetron_app LOGIN;
    END IF;
END
$$;

GRANT SELECT, INSERT, UPDATE, DELETE ON tenants TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON api_keys TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON quota_configs TO gadgetron_app;
GRANT SELECT, INSERT ON audit_log TO gadgetron_app;
