-- Runtime-neutral chat transcript storage.
--
-- `conversations` remains the sidebar metadata row. This table stores the
-- user/assistant text needed to restore a clicked chat regardless of whether
-- Penny is backed by Claude Code jsonl files or Codex exec sessions.

CREATE TABLE IF NOT EXISTS conversation_messages (
    id              BIGSERIAL PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tenant_id       UUID NOT NULL,
    user_id         UUID NOT NULL,
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content         TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS conversation_messages_conversation_order_idx
    ON conversation_messages (conversation_id, id);

CREATE INDEX IF NOT EXISTS conversation_messages_owner_idx
    ON conversation_messages (tenant_id, user_id, conversation_id);
