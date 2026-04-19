-- ISSUE 14 TASK 14.1 — identity + self-service foundation.
--
-- Adds the `users` / `teams` / `team_members` / `user_sessions` tables per
-- `docs/design/phase2/08-identity-and-users.md` §2.2. Extends `api_keys` with
-- `user_id` + `label` for key ownership (§2.2.3) and `audit_log` with
-- actor columns (§2.2.5) so every audit row can attribute to a specific user
-- and key.
--
-- P2B bootstrap: all new rows default to tenant_id =
-- '00000000-0000-0000-0000-000000000001' (the hardcoded "default" tenant from
-- phase1). This migration does NOT insert that row — an earlier migration
-- (or the bootstrap flow) owns it.
--
-- No retroactive `users` rows are inserted here. The bootstrap flow (ISSUE 14
-- TASK 14.2) creates the first admin from `[auth.bootstrap]` config when the
-- `users` table is empty. Existing `api_keys` rows temporarily carry
-- user_id = NULL (column is nullable here) — TASK 14.3 adds the admin-user
-- backfill script that NULLs can land on, then flips the column to NOT NULL
-- in a follow-up migration (20260420000005 reserved).

-- ---------------------------------------------------------------------------
-- users
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id)
                        DEFAULT '00000000-0000-0000-0000-000000000001',
    email           TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    role            TEXT NOT NULL CHECK (role IN ('member', 'admin', 'service')),
    password_hash   TEXT,                      -- argon2id. service role => NULL.
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at   TIMESTAMPTZ,

    UNIQUE (tenant_id, email),
    CHECK (role != 'service' OR password_hash IS NULL)
);

CREATE INDEX IF NOT EXISTS idx_users_tenant_active
    ON users (tenant_id, is_active) WHERE is_active;
CREATE INDEX IF NOT EXISTS idx_users_email_active
    ON users (email) WHERE is_active;

-- ---------------------------------------------------------------------------
-- teams + team_members
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS teams (
    id              TEXT PRIMARY KEY,
    tenant_id       UUID NOT NULL REFERENCES tenants(id)
                        DEFAULT '00000000-0000-0000-0000-000000000001',
    display_name    TEXT NOT NULL,
    description     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by      UUID REFERENCES users(id),

    -- kebab-case, 32 chars max. 'admins' reserved as virtual team.
    CHECK (id ~ '^[a-z][a-z0-9-]{0,31}$'),
    CHECK (id != 'admins')
);

CREATE TABLE IF NOT EXISTS team_members (
    team_id         TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            TEXT NOT NULL DEFAULT 'member'
                        CHECK (role IN ('member', 'lead')),
    added_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    added_by        UUID REFERENCES users(id),

    PRIMARY KEY (team_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_team_members_user
    ON team_members (user_id);

-- ---------------------------------------------------------------------------
-- user_sessions (web UI login)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS user_sessions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tenant_id       UUID NOT NULL,
    cookie_hash     TEXT NOT NULL,             -- SHA-256 of secure cookie
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at      TIMESTAMPTZ NOT NULL,
    last_active_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    user_agent      TEXT,
    ip_address      INET,
    revoked_at      TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_sessions_cookie_active
    ON user_sessions (cookie_hash) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_sessions_user_active
    ON user_sessions (user_id, expires_at) WHERE revoked_at IS NULL;

-- ---------------------------------------------------------------------------
-- api_keys ← user_id + label (nullable for now; tightened in TASK 14.3)
-- ---------------------------------------------------------------------------
ALTER TABLE api_keys
    ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id),
    ADD COLUMN IF NOT EXISTS label   TEXT;

CREATE INDEX IF NOT EXISTS idx_api_keys_user
    ON api_keys (user_id) WHERE revoked_at IS NULL;

-- ---------------------------------------------------------------------------
-- audit_log ← actor columns
-- ---------------------------------------------------------------------------
ALTER TABLE audit_log
    ADD COLUMN IF NOT EXISTS actor_user_id     UUID REFERENCES users(id),
    ADD COLUMN IF NOT EXISTS actor_api_key_id  UUID REFERENCES api_keys(id),
    ADD COLUMN IF NOT EXISTS impersonated_by   TEXT,
    ADD COLUMN IF NOT EXISTS parent_request_id TEXT;

CREATE INDEX IF NOT EXISTS idx_audit_user_time
    ON audit_log (actor_user_id, timestamp DESC);
