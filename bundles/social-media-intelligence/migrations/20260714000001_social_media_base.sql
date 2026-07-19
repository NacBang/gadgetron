CREATE TABLE IF NOT EXISTS social_posts (
    tenant_id       UUID NOT NULL,
    post_id         UUID NOT NULL,
    topic_id        UUID NOT NULL,
    provider        TEXT NOT NULL CHECK (provider IN ('bluesky')),
    external_uri    TEXT NOT NULL CHECK (external_uri LIKE 'at://%'),
    cid             TEXT NOT NULL,
    author_handle   TEXT NOT NULL,
    text_excerpt    TEXT NOT NULL,
    language        TEXT,
    reply_to_uri    TEXT,
    quote_uri       TEXT,
    state           TEXT NOT NULL CHECK (state IN ('current', 'edited', 'deleted', 'moderated')),
    engagement      JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(engagement) = 'object'),
    moderation_labels JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(moderation_labels) = 'array'),
    source_id       UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    fetched_at      TIMESTAMPTZ NOT NULL,
    content_hash    TEXT NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, post_id),
    UNIQUE (tenant_id, topic_id, provider, external_uri, source_id, source_revision)
);

CREATE TABLE IF NOT EXISTS social_conversations (
    tenant_id       UUID NOT NULL,
    conversation_id UUID NOT NULL,
    topic_id        UUID NOT NULL,
    title           TEXT NOT NULL,
    summary         TEXT NOT NULL,
    origin_uri      TEXT NOT NULL CHECK (origin_uri LIKE 'at://%'),
    post_count      INTEGER NOT NULL CHECK (post_count >= 1),
    status          TEXT NOT NULL CHECK (status IN ('current', 'aging', 'stale', 'conflicted')),
    first_seen_at   TIMESTAMPTZ NOT NULL,
    last_seen_at    TIMESTAMPTZ NOT NULL,
    source_ids      JSONB NOT NULL CHECK (jsonb_typeof(source_ids) = 'array'),
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, conversation_id),
    CHECK (last_seen_at >= first_seen_at)
);

CREATE TABLE IF NOT EXISTS social_signals (
    tenant_id       UUID NOT NULL,
    signal_id       UUID NOT NULL,
    conversation_id UUID NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('claim', 'trend', 'audience', 'question', 'correction')),
    statement       TEXT NOT NULL,
    confidence_basis TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('observed', 'speculative', 'corroborated', 'contradicted')),
    supporting_posts JSONB NOT NULL CHECK (jsonb_typeof(supporting_posts) = 'array'),
    contradicting_posts JSONB NOT NULL CHECK (jsonb_typeof(contradicting_posts) = 'array'),
    window_start    TIMESTAMPTZ NOT NULL,
    window_end      TIMESTAMPTZ NOT NULL,
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, signal_id),
    FOREIGN KEY (tenant_id, conversation_id)
        REFERENCES social_conversations(tenant_id, conversation_id) ON DELETE CASCADE,
    CHECK (window_end >= window_start)
);

CREATE TABLE IF NOT EXISTS social_briefings (
    tenant_id       UUID NOT NULL,
    briefing_id     UUID NOT NULL,
    topic_id        UUID NOT NULL,
    title           TEXT NOT NULL,
    key_changes     TEXT NOT NULL,
    why_it_matters  TEXT NOT NULL,
    open_questions  TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('current', 'aging', 'stale', 'conflicted')),
    window_start    TIMESTAMPTZ NOT NULL,
    window_end      TIMESTAMPTZ NOT NULL,
    citations       JSONB NOT NULL CHECK (jsonb_typeof(citations) = 'array'),
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, briefing_id),
    CHECK (window_end >= window_start)
);

CREATE TABLE IF NOT EXISTS social_response_drafts (
    tenant_id       UUID NOT NULL,
    draft_id        UUID NOT NULL,
    briefing_id     UUID NOT NULL,
    provider        TEXT NOT NULL CHECK (provider IN ('bluesky')),
    target_account  TEXT NOT NULL,
    audience        TEXT NOT NULL,
    objective       TEXT NOT NULL,
    body            TEXT NOT NULL,
    impact_preview  TEXT NOT NULL,
    risk_notes      TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('draft', 'reviewed', 'handed_off', 'withdrawn')),
    citations       JSONB NOT NULL CHECK (jsonb_typeof(citations) = 'array'),
    revision        BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, draft_id),
    FOREIGN KEY (tenant_id, briefing_id)
        REFERENCES social_briefings(tenant_id, briefing_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_social_posts_topic_time
    ON social_posts (tenant_id, topic_id, fetched_at DESC);
CREATE INDEX IF NOT EXISTS idx_social_posts_source
    ON social_posts (tenant_id, source_id, source_revision);
CREATE INDEX IF NOT EXISTS idx_social_conversations_topic_time
    ON social_conversations (tenant_id, topic_id, last_seen_at DESC);
CREATE INDEX IF NOT EXISTS idx_social_signals_conversation_time
    ON social_signals (tenant_id, conversation_id, window_end DESC);
CREATE INDEX IF NOT EXISTS idx_social_briefings_topic_time
    ON social_briefings (tenant_id, topic_id, window_end DESC);
CREATE INDEX IF NOT EXISTS idx_social_response_drafts_briefing
    ON social_response_drafts (tenant_id, briefing_id, updated_at DESC);
