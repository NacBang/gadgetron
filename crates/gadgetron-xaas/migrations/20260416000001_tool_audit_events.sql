-- ADR-P2A-06 Implementation status addendum item 1:
-- Persistent `ToolCallCompleted` audit records emitted from
-- `gadgetron-kairos::stream::event_to_chat_chunks` through
-- `ToolAuditEventSink`. Schema includes `conversation_id` and
-- `claude_session_uuid` from day one so the A5–A7 native-session
-- integration does not require a second migration.

CREATE TABLE tool_audit_events (
    id                  BIGSERIAL PRIMARY KEY,
    request_id          UUID,
    tool_name           TEXT NOT NULL,
    tier                TEXT NOT NULL CHECK (tier IN ('read', 'write', 'destructive')),
    category            TEXT NOT NULL,
    outcome             TEXT NOT NULL CHECK (outcome IN ('success', 'error')),
    error_code          TEXT,
    elapsed_ms          BIGINT NOT NULL DEFAULT 0,
    conversation_id     TEXT NULL,
    claude_session_uuid TEXT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX tool_audit_events_created_at_idx ON tool_audit_events (created_at DESC);
CREATE INDEX tool_audit_events_tool_name_idx ON tool_audit_events (tool_name);
CREATE INDEX tool_audit_events_conversation_id_idx
    ON tool_audit_events (conversation_id) WHERE conversation_id IS NOT NULL;
