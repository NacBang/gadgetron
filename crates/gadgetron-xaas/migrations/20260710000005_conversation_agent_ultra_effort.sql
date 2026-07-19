-- Codex 0.144.1 advertises an `ultra` reasoning tier for GPT-5.6 Sol/Terra.
-- Preserve that explicit per-conversation intent. Runtime/model capability
-- normalization remains in gadgetron-core; the DB constraint only validates
-- the serialized AgentEffort vocabulary.

ALTER TABLE conversations
    DROP CONSTRAINT IF EXISTS conversations_agent_effort_check;

ALTER TABLE conversations
    ADD CONSTRAINT conversations_agent_effort_check CHECK (
        agent_effort IS NULL OR agent_effort IN (
            'auto', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra'
        )
    );

COMMENT ON COLUMN conversations.agent_effort IS
    'Per-conversation reasoning effort intent; auto is resolved per turn, ultra is capability-gated to supported Codex models.';
