CREATE TABLE IF NOT EXISTS community_discussions (
    tenant_id UUID NOT NULL,
    discussion_id UUID NOT NULL,
    topic_id UUID NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('stack-exchange', 'reddit', 'forum')),
    external_id TEXT NOT NULL,
    canonical_url TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'edited', 'deleted', 'locked')),
    score_snapshot INTEGER NOT NULL DEFAULT 0,
    accepted_answer_observed BOOLEAN NOT NULL DEFAULT FALSE,
    source_id UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    fetched_at TIMESTAMPTZ NOT NULL,
    content_hash TEXT NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, discussion_id),
    UNIQUE (tenant_id, topic_id, provider, external_id, source_id, source_revision)
);

CREATE INDEX IF NOT EXISTS community_discussion_topic_time_idx
    ON community_discussions (tenant_id, topic_id, fetched_at DESC);

CREATE TABLE IF NOT EXISTS community_solution_patterns (
    tenant_id UUID NOT NULL,
    pattern_id UUID NOT NULL,
    topic_id UUID NOT NULL,
    title TEXT NOT NULL,
    problem_signature TEXT NOT NULL,
    environment TEXT NOT NULL,
    procedure TEXT NOT NULL,
    rollback TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('reproduced', 'environment_dependent', 'contradicted', 'speculative', 'obsolete')),
    supporting_evidence INTEGER NOT NULL DEFAULT 0 CHECK (supporting_evidence >= 0),
    contradicting_evidence INTEGER NOT NULL DEFAULT 0 CHECK (contradicting_evidence >= 0),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, pattern_id)
);

CREATE INDEX IF NOT EXISTS community_pattern_topic_status_idx
    ON community_solution_patterns (tenant_id, topic_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS community_pattern_evidence (
    tenant_id UUID NOT NULL,
    evidence_id UUID NOT NULL,
    pattern_id UUID NOT NULL,
    discussion_id UUID NOT NULL,
    statement TEXT NOT NULL,
    stance TEXT NOT NULL CHECK (stance IN ('supports', 'contradicts', 'context')),
    source_id UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    observed_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, evidence_id),
    FOREIGN KEY (tenant_id, pattern_id) REFERENCES community_solution_patterns (tenant_id, pattern_id),
    FOREIGN KEY (tenant_id, discussion_id) REFERENCES community_discussions (tenant_id, discussion_id),
    UNIQUE (tenant_id, pattern_id, discussion_id, source_id, source_revision, stance)
);

CREATE INDEX IF NOT EXISTS community_evidence_pattern_time_idx
    ON community_pattern_evidence (tenant_id, pattern_id, observed_at DESC);
