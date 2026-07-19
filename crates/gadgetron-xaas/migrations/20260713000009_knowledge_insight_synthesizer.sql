-- R3.4a: verified Outcome-backed Insight synthesis jobs.

ALTER TABLE knowledge_jobs
    DROP CONSTRAINT IF EXISTS knowledge_jobs_role_check;
ALTER TABLE knowledge_jobs
    ADD CONSTRAINT knowledge_jobs_role_check
    CHECK (role IN ('source_scout', 'researcher', 'insight_synthesizer', 'gardener'));
