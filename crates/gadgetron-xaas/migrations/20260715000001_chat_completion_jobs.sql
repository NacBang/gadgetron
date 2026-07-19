CREATE TABLE IF NOT EXISTS chat_completion_jobs (
    job_id           UUID PRIMARY KEY,
    conversation_id  UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tenant_id        UUID NOT NULL,
    user_id          UUID NOT NULL,
    model             TEXT NOT NULL,
    agent_profile     JSONB,
    status            TEXT NOT NULL CHECK (status IN ('streaming', 'complete', 'error', 'cancelled')),
    chunk_count       INTEGER NOT NULL DEFAULT 0 CHECK (chunk_count >= 0),
    error_message     TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at       TIMESTAMPTZ,
    CHECK (
        (status = 'streaming' AND finished_at IS NULL AND error_message IS NULL)
        OR (status = 'error' AND finished_at IS NOT NULL AND error_message IS NOT NULL)
        OR (
            status IN ('complete', 'cancelled')
            AND finished_at IS NOT NULL
            AND error_message IS NULL
        )
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS chat_completion_jobs_one_streaming_per_conversation
    ON chat_completion_jobs (conversation_id)
    WHERE status = 'streaming';

CREATE INDEX IF NOT EXISTS chat_completion_jobs_owner_conversation_finished
    ON chat_completion_jobs (tenant_id, user_id, conversation_id, finished_at DESC);
