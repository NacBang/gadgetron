-- ISSUE 11 TASK 11.3 — Postgres-backed spend tracking.
--
-- `quota_configs` already holds `daily_used_cents` and
-- `monthly_used_cents` but never rolls them over on the day / month
-- boundary. We add a `usage_day DATE` column that records which UTC
-- day the cumulative counters refer to. The record_post path's UPDATE
-- uses CASE expressions to reset the daily counter when
-- `usage_day != CURRENT_DATE` and the monthly counter when the month
-- changed — no background job needed; rollover happens on the first
-- post-rollover request.
--
-- Default to CURRENT_DATE for existing rows so the first request
-- after this migration doesn't spuriously zero their monthly usage.
ALTER TABLE quota_configs
    ADD COLUMN IF NOT EXISTS usage_day DATE NOT NULL DEFAULT CURRENT_DATE;

-- Index lets an operator SELECT usage for all tenants on a given
-- day without a full scan once tenant count grows.
CREATE INDEX IF NOT EXISTS quota_configs_usage_day_idx
    ON quota_configs (usage_day);
