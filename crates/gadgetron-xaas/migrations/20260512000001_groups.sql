-- Groups: access-permission bucket, orthogonal to teams and role.
--
-- Three identity dimensions in Gadgetron, each independently editable:
--   1. users.role           — settings privilege (admin / member / service)
--   2. teams + team_members — collaboration unit (who works together,
--                             carries per-membership role member|lead)
--   3. groups + user_groups — access-permission bucket (this migration)
--
-- Permission policies attached to groups (scopes, resource ACLs) are a
-- Phase 2 concern. This migration ships only the schema (the group
-- itself + user membership). Shape mirrors `teams` so the admin
-- handler set stays symmetric; the deliberate difference is no
-- per-membership role on `user_groups` — groups are flat access
-- buckets, not collaborative sub-structures.

CREATE TABLE IF NOT EXISTS groups (
    id              TEXT PRIMARY KEY,
    tenant_id       UUID NOT NULL REFERENCES tenants(id)
                        DEFAULT '00000000-0000-0000-0000-000000000001',
    display_name    TEXT NOT NULL,
    description     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by      UUID REFERENCES users(id),

    -- kebab-case, 32 chars max (mirrors teams).
    CHECK (id ~ '^[a-z][a-z0-9-]{0,31}$')
);

CREATE TABLE IF NOT EXISTS user_groups (
    group_id        TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    added_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    added_by        UUID REFERENCES users(id),

    PRIMARY KEY (group_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_user_groups_user
    ON user_groups (user_id);
