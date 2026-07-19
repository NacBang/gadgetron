-- Tenant-adjustable concurrency ceiling for durable Knowledge agent jobs.
--
-- Leasing locks the tenant row together with the selected job, so concurrent
-- workers cannot both observe the same remaining slot and exceed this limit.

ALTER TABLE tenants
    ADD COLUMN knowledge_job_concurrency_limit INTEGER NOT NULL DEFAULT 4
        CHECK (knowledge_job_concurrency_limit BETWEEN 1 AND 64);

