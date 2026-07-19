CREATE TABLE IF NOT EXISTS news_article_snapshots (
    tenant_id UUID NOT NULL,
    article_id UUID NOT NULL,
    topic_id UUID NOT NULL,
    canonical_url TEXT NOT NULL,
    headline TEXT NOT NULL,
    publisher TEXT NOT NULL,
    source_class TEXT NOT NULL CHECK (source_class IN ('official', 'editorial', 'community')),
    source_id UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    published_at TIMESTAMPTZ,
    fetched_at TIMESTAMPTZ NOT NULL,
    content_hash TEXT NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    summary TEXT NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, article_id),
    UNIQUE (tenant_id, topic_id, canonical_url, content_hash)
);

CREATE INDEX IF NOT EXISTS news_article_topic_time_idx
    ON news_article_snapshots (tenant_id, topic_id, fetched_at DESC);

CREATE TABLE IF NOT EXISTS news_events (
    tenant_id UUID NOT NULL,
    event_id UUID NOT NULL,
    topic_id UUID NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('developing', 'confirmed', 'corrected', 'uncertain', 'closed')),
    first_seen_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    official_sources INTEGER NOT NULL DEFAULT 0 CHECK (official_sources >= 0),
    editorial_sources INTEGER NOT NULL DEFAULT 0 CHECK (editorial_sources >= 0),
    community_sources INTEGER NOT NULL DEFAULT 0 CHECK (community_sources >= 0),
    supporting_claims INTEGER NOT NULL DEFAULT 0 CHECK (supporting_claims >= 0),
    contradicting_claims INTEGER NOT NULL DEFAULT 0 CHECK (contradicting_claims >= 0),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, event_id),
    CHECK (last_seen_at >= first_seen_at)
);

CREATE INDEX IF NOT EXISTS news_event_topic_time_idx
    ON news_events (tenant_id, topic_id, last_seen_at DESC);

CREATE TABLE IF NOT EXISTS news_claims (
    tenant_id UUID NOT NULL,
    claim_id UUID NOT NULL,
    event_id UUID NOT NULL,
    article_id UUID,
    statement TEXT NOT NULL,
    speaker TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN ('reported', 'corroborated', 'contradicted', 'corrected', 'unverified')),
    source_id UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    observed_at TIMESTAMPTZ NOT NULL,
    supersedes_claim_id UUID,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, claim_id),
    FOREIGN KEY (tenant_id, event_id) REFERENCES news_events (tenant_id, event_id),
    FOREIGN KEY (tenant_id, article_id) REFERENCES news_article_snapshots (tenant_id, article_id),
    FOREIGN KEY (tenant_id, supersedes_claim_id) REFERENCES news_claims (tenant_id, claim_id),
    CHECK (status <> 'corrected' OR supersedes_claim_id IS NOT NULL),
    CHECK (supersedes_claim_id IS NULL OR supersedes_claim_id <> claim_id)
);

CREATE INDEX IF NOT EXISTS news_claim_event_time_idx
    ON news_claims (tenant_id, event_id, observed_at DESC);

CREATE TABLE IF NOT EXISTS news_briefings (
    tenant_id UUID NOT NULL,
    briefing_id UUID NOT NULL,
    topic_id UUID NOT NULL,
    title TEXT NOT NULL,
    key_changes TEXT NOT NULL,
    why_it_matters TEXT NOT NULL,
    open_questions TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN ('current', 'aging', 'stale', 'conflicted')),
    window_start TIMESTAMPTZ NOT NULL,
    window_end TIMESTAMPTZ NOT NULL,
    official_sources INTEGER NOT NULL DEFAULT 0 CHECK (official_sources >= 0),
    editorial_sources INTEGER NOT NULL DEFAULT 0 CHECK (editorial_sources >= 0),
    community_sources INTEGER NOT NULL DEFAULT 0 CHECK (community_sources >= 0),
    supporting_claims INTEGER NOT NULL DEFAULT 0 CHECK (supporting_claims >= 0),
    contradicting_claims INTEGER NOT NULL DEFAULT 0 CHECK (contradicting_claims >= 0),
    citations JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(citations) = 'array'),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, briefing_id),
    CHECK (window_end >= window_start)
);

CREATE INDEX IF NOT EXISTS news_briefing_topic_time_idx
    ON news_briefings (tenant_id, topic_id, window_end DESC);
