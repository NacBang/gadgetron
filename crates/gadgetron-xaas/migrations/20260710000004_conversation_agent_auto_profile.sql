-- Allow the durable conversation profile to preserve an Auto effort intent.
-- The gateway resolves it to a concrete tier per turn; subprocesses never see
-- the literal `auto` value.

ALTER TABLE conversations
    DROP CONSTRAINT IF EXISTS conversations_agent_effort_check;

ALTER TABLE conversations
    ADD CONSTRAINT conversations_agent_effort_check CHECK (
        agent_effort IS NULL OR agent_effort IN ('auto', 'low', 'medium', 'high', 'xhigh', 'max')
    );

COMMENT ON COLUMN conversations.agent_effort IS
    'Per-conversation reasoning effort intent; auto is resolved per turn, NULL only for unmigrated/unstarted rows.';
