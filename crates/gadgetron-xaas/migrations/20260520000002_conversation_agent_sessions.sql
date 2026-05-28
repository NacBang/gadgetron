CREATE TABLE IF NOT EXISTS conversation_agent_sessions (
    id BIGSERIAL PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL,
    user_id UUID NOT NULL,
    backend TEXT NOT NULL,
    backend_session_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (conversation_id, backend)
);

CREATE INDEX IF NOT EXISTS idx_conversation_agent_sessions_owner
    ON conversation_agent_sessions (tenant_id, user_id, conversation_id);
