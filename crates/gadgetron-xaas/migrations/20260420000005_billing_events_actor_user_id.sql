-- ISSUE 23 — add `actor_user_id` to billing_events so per-user spend
-- reports can select WHERE actor_user_id = $1 without joining to
-- audit_log. Nullable because chat events emitted pre-ISSUE-23 don't
-- carry it; legacy keys predating ISSUE 14 TASK 14.1 backfill also
-- have user_id=NULL.
--
-- No REFERENCES users(id) FK — callers at different layers (quota
-- record_post, action_service, MCP invoke) populate the field from
-- heterogeneous sources (ValidatedKey.user_id, AuthenticatedContext.user_id,
-- api_key_id-as-placeholder during P2A). An FK would make every new
-- caller a potential source of silent insert failures. The data path
-- is best-effort telemetry; operator reconciliation queries can
-- LEFT JOIN users(id) at read time.

ALTER TABLE billing_events
    ADD COLUMN IF NOT EXISTS actor_user_id UUID;

CREATE INDEX IF NOT EXISTS billing_events_actor_user_idx
    ON billing_events (actor_user_id, created_at DESC);
