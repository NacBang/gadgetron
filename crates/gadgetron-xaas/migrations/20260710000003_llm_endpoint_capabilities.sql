-- Durable local-LLM capability snapshots and endpoint references.
--
-- Connectivity, runtime protocol compatibility, and actual model tool use
-- are deliberately independent. Secret values remain outside PostgreSQL;
-- only the operator-approved environment variable name is persisted.

ALTER TABLE llm_endpoints
    ADD COLUMN IF NOT EXISTS discovered_models JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS runtime_compatibility TEXT NOT NULL DEFAULT 'unverified',
    ADD COLUMN IF NOT EXISTS tool_status TEXT NOT NULL DEFAULT 'untested',
    ADD COLUMN IF NOT EXISTS tool_model_id TEXT,
    ADD COLUMN IF NOT EXISTS last_tool_probe_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_tool_error TEXT,
    ADD COLUMN IF NOT EXISTS capability_details JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE llm_endpoints
    ADD CONSTRAINT llm_endpoints_discovered_models_array_check
        CHECK (jsonb_typeof(discovered_models) = 'array'),
    ADD CONSTRAINT llm_endpoints_capability_details_object_check
        CHECK (jsonb_typeof(capability_details) = 'object'),
    ADD CONSTRAINT llm_endpoints_runtime_compatibility_check
        CHECK (runtime_compatibility IN (
            'unverified', 'codex_exec', 'claude_code',
            'bridge_required', 'incompatible'
        )),
    ADD CONSTRAINT llm_endpoints_tool_status_check
        CHECK (tool_status IN ('untested', 'passed', 'failed', 'unsupported'));

ALTER TABLE agent_brain_settings
    ADD COLUMN IF NOT EXISTS llm_endpoint_id UUID
        REFERENCES llm_endpoints(id) ON DELETE SET NULL;

ALTER TABLE conversations
    ADD COLUMN IF NOT EXISTS agent_endpoint_id UUID
        REFERENCES llm_endpoints(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS conversations_agent_endpoint_idx
    ON conversations (agent_endpoint_id)
    WHERE deleted_at IS NULL AND agent_endpoint_id IS NOT NULL;

COMMENT ON COLUMN llm_endpoints.runtime_compatibility IS
    'Detected Penny adapter: codex_exec Responses, claude_code Messages, bridge_required, or incompatible.';
COMMENT ON COLUMN llm_endpoints.tool_status IS
    'Actual function/tool-call smoke result for tool_model_id; route reachability alone never sets passed.';
COMMENT ON COLUMN conversations.agent_endpoint_id IS
    'Tenant-validated local LLM registry reference; URL/env columns are immutable execution snapshots.';
