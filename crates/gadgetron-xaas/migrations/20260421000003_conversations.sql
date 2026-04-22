-- ISSUE 31 TASK 31.1 — per-user conversation tracking.
--
-- Each row corresponds to one chat thread visible in the left-rail
-- sidebar. `id` is what the frontend sends back as
-- `X-Gadgetron-Conversation-Id` on every turn, so the same Claude Code
-- `--resume <session>` session is re-entered for that row's history.
--
-- Soft delete via `deleted_at` so we can restore if an operator
-- accidentally wipes a valuable conversation; the list endpoint
-- filters `WHERE deleted_at IS NULL`.

CREATE TABLE IF NOT EXISTS conversations (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id            UUID NOT NULL,
    user_id              UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    claude_session_uuid  UUID,
    title                TEXT NOT NULL DEFAULT 'New chat',
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at           TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS conversations_user_updated_idx
    ON conversations(user_id, updated_at DESC)
    WHERE deleted_at IS NULL;

COMMENT ON TABLE conversations IS
    'Per-user chat threads — one row per left-rail entry.';
