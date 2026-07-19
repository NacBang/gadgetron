-- Distinguish OpenAI Responses API endpoints from Chat Completions-only
-- endpoints. Codex custom providers require Responses semantics for native
-- agent/tool events; `/v1/models` reachability alone is not sufficient.

ALTER TABLE llm_endpoints
    DROP CONSTRAINT IF EXISTS llm_endpoints_protocol_check;

ALTER TABLE llm_endpoints
    ADD CONSTRAINT llm_endpoints_protocol_check CHECK (
        protocol IN ('openai_chat', 'openai_responses', 'anthropic_messages')
    );
