-- ISSUE 30 TASK 30.1 — Google OAuth identity linkage.
--
-- `google_sub` carries the stable OIDC `sub` claim Google returns for a
-- given user. NULL for users created via password-only flow; on first
-- Google sign-in we upsert this column.
--
-- Uniqueness is enforced per-tenant: two separate tenants may both
-- onboard the same person via their own Google account (rare but
-- possible), and the tenant boundary is the authoritative scope for
-- identity in Gadgetron.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS google_sub TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS users_tenant_google_sub_uniq
    ON users(tenant_id, google_sub)
    WHERE google_sub IS NOT NULL;
