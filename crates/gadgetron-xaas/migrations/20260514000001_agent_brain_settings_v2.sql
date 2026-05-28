-- Extend agent_brain_settings with the high-level admin UI fields.
--
-- The new admin form exposes three axes:
--   * agent          — Claude Code vs Codex Exec subprocess runtime
--   * model_source   — Default (CLI built-in) vs Local OpenAI-compatible LLM
--   * local_base_url / local_api_key_env — populated when model_source = Local
--   * effort         — reasoning effort tier (low/medium/high/xhigh/max)
--
-- Existing rows pre-migration get sensible defaults: agent = 'claude_code'
-- (matches the historical "ClaudeMax" default), model_source = 'default',
-- empty local_* fields, effort = 'max' (most thorough — same expectation
-- the admin UI now exposes as the default for new entries).
--
-- All columns are non-null with defaults so the row_to_settings path can
-- stay total and panic-free.

ALTER TABLE agent_brain_settings
    ADD COLUMN IF NOT EXISTS agent TEXT NOT NULL DEFAULT 'claude_code',
    ADD COLUMN IF NOT EXISTS model_source TEXT NOT NULL DEFAULT 'default',
    ADD COLUMN IF NOT EXISTS local_base_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS local_api_key_env TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS effort TEXT NOT NULL DEFAULT 'max';

-- Defensive value normalization for pre-existing rows whose column
-- default was applied at ALTER time (so they carry the right literal).
UPDATE agent_brain_settings
SET agent = COALESCE(NULLIF(agent, ''), 'claude_code')
WHERE agent IS NULL OR agent = '';

UPDATE agent_brain_settings
SET model_source = COALESCE(NULLIF(model_source, ''), 'default')
WHERE model_source IS NULL OR model_source = '';

UPDATE agent_brain_settings
SET effort = COALESCE(NULLIF(effort, ''), 'max')
WHERE effort IS NULL OR effort = '';
