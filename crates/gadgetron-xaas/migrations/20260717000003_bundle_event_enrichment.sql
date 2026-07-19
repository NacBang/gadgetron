-- CORE-T1: signed Bundle event enrichment reuses the durable autonomy plane.

ALTER TABLE autonomy_goals
    DROP CONSTRAINT autonomy_goals_source_kind_check,
    ADD CONSTRAINT autonomy_goals_source_kind_check CHECK (
        source_kind IN ('bundle_schedule', 'bundle_event', 'personal', 'manager')
    ),
    DROP CONSTRAINT autonomy_goals_status_check,
    ADD CONSTRAINT autonomy_goals_status_check CHECK (
        status IN (
            'context_required', 'ready', 'running', 'retry_wait', 'paused',
            'retired', 'safe_stopped', 'succeeded', 'failed_provider',
            'failed_policy', 'stale_subject'
        )
    ),
    ADD COLUMN event_kind TEXT,
    ADD COLUMN subject_bundle_id TEXT,
    ADD COLUMN subject_kind TEXT,
    ADD COLUMN event_payload JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(event_payload) = 'object'),
    ADD COLUMN agent_role_id TEXT,
    ADD COLUMN result_gadget TEXT,
    ADD COLUMN agent_profile_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(agent_profile_snapshot) = 'object'),
    ADD CONSTRAINT autonomy_goals_bundle_event_shape CHECK (
        source_kind <> 'bundle_event'
        OR (
            event_kind IS NOT NULL AND length(event_kind) BETWEEN 1 AND 128
            AND subject_bundle_id IS NOT NULL AND length(subject_bundle_id) BETWEEN 1 AND 128
            AND subject_kind IS NOT NULL AND length(subject_kind) BETWEEN 1 AND 128
            AND agent_role_id IS NOT NULL AND length(agent_role_id) BETWEEN 1 AND 128
            AND result_gadget IS NOT NULL AND length(result_gadget) BETWEEN 1 AND 256
            AND target_revision ~ '^[0-9a-f]{64}$'
            AND agent_profile_snapshot <> '{}'::jsonb
        )
    );

CREATE UNIQUE INDEX autonomy_goals_bundle_event_dedup
    ON autonomy_goals (
        tenant_id, subject_bundle_id, subject_kind, target_id,
        target_revision, owner_bundle_id, agent_role_id
    )
    WHERE source_kind = 'bundle_event';

ALTER TABLE autonomy_goal_runs
    DROP CONSTRAINT autonomy_goal_runs_status_check,
    ADD CONSTRAINT autonomy_goal_runs_status_check CHECK (
        status IN (
            'running', 'succeeded', 'failed', 'interrupted', 'safe_stopped',
            'failed_provider', 'failed_policy', 'stale_subject'
        )
    ),
    ADD COLUMN agent_profile_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(agent_profile_snapshot) = 'object');

ALTER TABLE autonomy_goal_events
    DROP CONSTRAINT autonomy_goal_events_event_kind_check,
    ADD CONSTRAINT autonomy_goal_events_event_kind_check CHECK (
        event_kind IN (
            'context_required', 'ready', 'leased', 'heartbeat', 'context_pinned',
            'retry_scheduled', 'succeeded', 'failed', 'interrupted', 'safe_stopped',
            'paused', 'retired', 'failed_provider', 'failed_policy', 'stale_subject'
        )
    );

CREATE TABLE autonomy_event_receipts (
    job_id              UUID PRIMARY KEY,
    tenant_id           UUID NOT NULL,
    run_id              UUID NOT NULL,
    subject_revision    TEXT NOT NULL CHECK (subject_revision ~ '^[0-9a-f]{64}$'),
    result_hash         TEXT NOT NULL CHECK (result_hash ~ '^[0-9a-f]{64}$'),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, job_id),
    FOREIGN KEY (tenant_id, job_id) REFERENCES autonomy_goals(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, run_id) REFERENCES autonomy_goal_runs(tenant_id, id) ON DELETE CASCADE
);

GRANT SELECT, INSERT, UPDATE ON autonomy_event_receipts TO gadgetron_app;
