-- ISSUE 12 TASK 12.1 — integer-cent billing ledger.
--
-- One row per billable event. Populated by the quota path's
-- `record_post` hook (today: chat completions; future TASKs extend
-- to tool calls + workbench actions). Integer cents per ADR-D-8 —
-- no floats, no rounding drift across queries.
--
-- `source_event_id` is a loose FK (no ON DELETE CASCADE) because
-- the source-of-truth event lives in one of three audit tables
-- (`audit_log`, `tool_audit_events`, `action_audit_events`) and
-- we don't want a single pg constraint to block writes if the
-- source row isn't there yet (race — billing fires first).
CREATE TABLE IF NOT EXISTS billing_events (
    id                BIGSERIAL PRIMARY KEY,
    tenant_id         UUID         NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_kind        TEXT         NOT NULL CHECK (event_kind IN ('chat', 'tool', 'action')),
    source_event_id   UUID,
    cost_cents        BIGINT       NOT NULL,
    model             TEXT,
    provider          TEXT,
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- Hot path for "give me this tenant's billing over a window" — the
-- most common invoice query.
CREATE INDEX IF NOT EXISTS billing_events_tenant_created_idx
    ON billing_events (tenant_id, created_at DESC);

-- Secondary index for cross-tenant spend reports bucketed by kind
-- (e.g. "total chat spend today across all tenants").
CREATE INDEX IF NOT EXISTS billing_events_kind_created_idx
    ON billing_events (event_kind, created_at DESC);
