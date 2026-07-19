-- E4-R3/R3.2b: actual enforcement path and authorization disposition.

ALTER TABLE policy_decision_events
    ADD COLUMN enforcement_path TEXT NOT NULL DEFAULT 'legacy_record'
        CHECK (enforcement_path IN (
            'legacy_record', 'tool', 'workbench_action', 'review_resume',
            'bundle_background', 'knowledge_background'
        )),
    ADD COLUMN authorization_state TEXT NOT NULL DEFAULT 'legacy_record'
        CHECK (authorization_state IN (
            'legacy_record', 'auto', 'denied', 'pending_review', 'approved_review'
        )),
    ADD COLUMN approval_id UUID NULL;

CREATE INDEX policy_decision_events_tenant_path_created
    ON policy_decision_events (tenant_id, enforcement_path, created_at DESC, event_id DESC);

CREATE INDEX policy_decision_events_approval
    ON policy_decision_events (tenant_id, approval_id)
    WHERE approval_id IS NOT NULL;
