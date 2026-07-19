-- R3.4a: Source Scout durable jobs and approval-only proposal artifacts.

ALTER TABLE knowledge_jobs
    DROP CONSTRAINT IF EXISTS knowledge_jobs_role_check;
ALTER TABLE knowledge_jobs
    ADD CONSTRAINT knowledge_jobs_role_check
    CHECK (role IN ('source_scout', 'researcher', 'gardener'));

ALTER TABLE knowledge_job_artifacts
    DROP CONSTRAINT IF EXISTS knowledge_job_artifacts_kind_check;
ALTER TABLE knowledge_job_artifacts
    ADD CONSTRAINT knowledge_job_artifacts_kind_check
    CHECK (kind IN ('source_proposal', 'dossier', 'partial_dossier', 'candidate', 'agent_output'));
