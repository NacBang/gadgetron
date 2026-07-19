-- R3.4a: pin each reviewed Knowledge change set to the immutable Candidate
-- artifact that produced it. Existing v1 jobs remain readable without an
-- inferred Candidate link.

ALTER TABLE knowledge_change_sets
    ADD COLUMN candidate_artifact_id UUID;

ALTER TABLE knowledge_change_sets
    ADD CONSTRAINT knowledge_change_sets_candidate_artifact_fk
    FOREIGN KEY (tenant_id, candidate_artifact_id)
    REFERENCES knowledge_job_artifacts(tenant_id, id);

CREATE INDEX knowledge_change_sets_candidate_artifact_idx
    ON knowledge_change_sets (tenant_id, candidate_artifact_id)
    WHERE candidate_artifact_id IS NOT NULL;

CREATE UNIQUE INDEX knowledge_change_sets_one_per_candidate
    ON knowledge_change_sets (tenant_id, candidate_artifact_id)
    WHERE candidate_artifact_id IS NOT NULL;

-- Source pins and immutable artifacts are provenance, not disposable queue
-- scratch. Queue retry still updates the job projection without deleting it.
REVOKE DELETE ON knowledge_jobs, knowledge_job_sources FROM gadgetron_app;
