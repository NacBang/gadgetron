-- Durable per-conversation Penny execution profile.
--
-- Global agent_brain_settings now defines defaults for NEW conversations.
-- Once agent_backend is set on a conversation it is immutable; model and
-- effort may change between turns within that backend.

ALTER TABLE conversations
    ADD COLUMN IF NOT EXISTS agent_backend TEXT CHECK (
        agent_backend IS NULL OR agent_backend IN ('claude_code', 'codex_exec')
    ),
    ADD COLUMN IF NOT EXISTS agent_model TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS agent_effort TEXT CHECK (
        agent_effort IS NULL OR agent_effort IN ('low', 'medium', 'high', 'xhigh', 'max')
    ),
    ADD COLUMN IF NOT EXISTS agent_model_source TEXT CHECK (
        agent_model_source IS NULL OR agent_model_source IN ('default', 'local')
    ),
    ADD COLUMN IF NOT EXISTS agent_local_base_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS agent_local_api_key_env TEXT NOT NULL DEFAULT '';

-- Preserve the runtime identity of legacy conversations that already have a
-- native backend session. A historical bug allowed both backend rows; choose
-- the most recently updated one deterministically without deleting either.
WITH latest_backend AS (
    SELECT DISTINCT ON (conversation_id)
           conversation_id, backend
    FROM conversation_agent_sessions
    WHERE backend IN ('claude_code', 'codex_exec')
    ORDER BY conversation_id, updated_at DESC, id DESC
)
UPDATE conversations AS conversation
SET agent_backend = latest.backend,
    agent_effort = COALESCE(conversation.agent_effort, 'max'),
    agent_model_source = COALESCE(conversation.agent_model_source, 'default')
FROM latest_backend AS latest
WHERE conversation.id = latest.conversation_id
  AND conversation.agent_backend IS NULL;

CREATE INDEX IF NOT EXISTS conversations_agent_backend_idx
    ON conversations (agent_backend)
    WHERE deleted_at IS NULL AND agent_backend IS NOT NULL;

COMMENT ON COLUMN conversations.agent_backend IS
    'Pinned Penny subprocess runtime. NULL until the first profile/turn; immutable afterwards.';
COMMENT ON COLUMN conversations.agent_model IS
    'Per-conversation model id; may change between turns within the pinned runtime.';
COMMENT ON COLUMN conversations.agent_effort IS
    'Per-conversation reasoning effort; NULL only for unmigrated/unstarted rows.';
