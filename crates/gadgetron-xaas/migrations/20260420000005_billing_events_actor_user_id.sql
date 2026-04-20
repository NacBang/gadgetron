-- ISSUE 23 — add `actor_user_id` to billing_events so per-user spend
-- reports can select WHERE actor_user_id = $1 (with tenant_id also
-- pinned) without joining to audit_log. Nullable because:
--   * chat events emitted pre-ISSUE-23 don't carry it
--   * chat path through ISSUE 24 passes NULL (QuotaToken doesn't
--     thread user_id yet)
--   * action path through ISSUE 24 passes NULL
--     (`AuthenticatedContext.user_id` is an api_key_id placeholder
--      in the workbench; only tool-path + ISSUE 24-updated
--      AuthenticatedContext can populate it safely)
--   * legacy keys predating ISSUE 14 TASK 14.1 backfill still have
--     `api_keys.user_id = NULL`
--
-- INTENTIONALLY no REFERENCES users(id) FK — callers at different
-- layers (quota record_post, action_service, MCP invoke) populate
-- from heterogeneous sources, and a strict FK would turn a buggy or
-- not-yet-migrated caller into a silent insert failure (best-effort
-- telemetry would stop logging). Do NOT add the FK back in a follow-
-- up migration without a corresponding audit of every caller's
-- source of user_id. Security review on ISSUE 23 accepts the
-- trade-off; operator reconciliation queries LEFT JOIN users(id) at
-- read time.
--
-- `actor_user_id` is a denormalized projection of
-- `audit_log.actor_user_id` (source of truth). Invoice materializer
-- SHOULD prefer the audit_log join when both are present; this
-- column exists for query-ergonomic per-user spend reports only.

ALTER TABLE billing_events
    ADD COLUMN IF NOT EXISTS actor_user_id UUID;

-- Index is composite (tenant_id, actor_user_id, created_at DESC)
-- so per-user spend queries MUST filter by tenant_id to hit it.
-- This defends against "SELECT * FROM billing_events WHERE
-- actor_user_id = $X" without tenant pinning (cross-tenant leakage
-- risk in reports). Matches `billing_events_tenant_created_idx`
-- discipline.
CREATE INDEX IF NOT EXISTS billing_events_tenant_actor_user_idx
    ON billing_events (tenant_id, actor_user_id, created_at DESC);
