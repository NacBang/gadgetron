ALTER TABLE knowledge_change_sets
    ALTER COLUMN job_id DROP NOT NULL,
    ADD COLUMN origin TEXT NOT NULL DEFAULT 'gardener'
        CHECK (origin IN ('gardener', 'user'));
