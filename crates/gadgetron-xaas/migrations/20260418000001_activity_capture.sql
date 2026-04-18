-- W3-KC-1c: Knowledge Candidate Capture Plane
-- Authority: docs/design/core/knowledge-candidate-curation.md §2.1
-- gen_random_uuid() is provided by pgcrypto, already available via the
-- existing DEFAULT usage in tenants / audit_log / etc. No additional
-- CREATE EXTENSION needed — pgcrypto is loaded as part of the PG 16
-- default install on this stack.

CREATE TABLE activity_events (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    actor_user_id UUID NOT NULL,
    request_id UUID,
    origin TEXT NOT NULL,           -- ActivityOrigin snake_case: user_direct, penny, system
    kind TEXT NOT NULL,             -- ActivityKind snake_case: direct_action, gadget_tool_call, approval_decision, runtime_observation, knowledge_writeback
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    source_bundle TEXT,
    source_capability TEXT,
    audit_event_id UUID,
    facts JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_activity_events_tenant_created
    ON activity_events (tenant_id, created_at DESC);

CREATE INDEX idx_activity_events_audit
    ON activity_events (audit_event_id)
    WHERE audit_event_id IS NOT NULL;

CREATE TABLE knowledge_candidates (
    id UUID PRIMARY KEY,
    activity_event_id UUID NOT NULL REFERENCES activity_events(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL,
    actor_user_id UUID NOT NULL,
    summary TEXT NOT NULL,
    proposed_path TEXT,
    provenance JSONB NOT NULL DEFAULT '{}'::jsonb,
    disposition TEXT NOT NULL,      -- KnowledgeCandidateDisposition snake_case
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_knowledge_candidates_tenant_created
    ON knowledge_candidates (tenant_id, created_at DESC);

CREATE INDEX idx_knowledge_candidates_pending
    ON knowledge_candidates (disposition, created_at DESC)
    WHERE disposition IN ('pending_penny_decision', 'pending_user_confirmation');

CREATE TABLE candidate_decisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    candidate_id UUID NOT NULL REFERENCES knowledge_candidates(id) ON DELETE CASCADE,
    decision TEXT NOT NULL,         -- CandidateDecisionKind snake_case: accept, reject, escalate_to_user
    decided_by_user_id UUID,
    decided_by_penny BOOLEAN NOT NULL DEFAULT FALSE,
    rationale TEXT,
    decided_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_candidate_decisions_candidate
    ON candidate_decisions (candidate_id, decided_at DESC);
