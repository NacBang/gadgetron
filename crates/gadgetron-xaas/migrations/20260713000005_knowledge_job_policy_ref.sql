-- Versioned policy identities include a UUID, revision and sha256 digest. The
-- original 64-character limit predates that canonical representation.

ALTER TABLE knowledge_jobs
    DROP CONSTRAINT knowledge_jobs_tool_policy_revision_check;

ALTER TABLE knowledge_jobs
    ADD CONSTRAINT knowledge_jobs_tool_policy_revision_check
    CHECK (length(tool_policy_revision) BETWEEN 1 AND 256);
