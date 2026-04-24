-- Per-conversation turn counter + marker for when Penny last replaced
-- the title with a rolling summary. Titles are regenerated every few
-- turns so the sidebar reflects what the conversation is *now* about
-- rather than just the first question ever asked.

ALTER TABLE conversations
    ADD COLUMN IF NOT EXISTS turn_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE conversations
    ADD COLUMN IF NOT EXISTS summary_turn_at INTEGER NOT NULL DEFAULT 0;
