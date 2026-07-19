CREATE TABLE IF NOT EXISTS restaurant_branches (
    tenant_id UUID NOT NULL,
    branch_id UUID NOT NULL,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    address TEXT NOT NULL CHECK (char_length(address) BETWEEN 1 AND 500),
    cuisine TEXT NOT NULL CHECK (char_length(cuisine) BETWEEN 1 AND 120),
    status TEXT NOT NULL CHECK (status IN ('open', 'temporarily_closed', 'closed', 'unknown')),
    source_id UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    observed_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, branch_id)
);

CREATE INDEX IF NOT EXISTS restaurant_branches_name_idx
    ON restaurant_branches (tenant_id, name, branch_id);

CREATE TABLE IF NOT EXISTS restaurant_menu_items (
    tenant_id UUID NOT NULL,
    menu_item_id UUID NOT NULL,
    branch_id UUID NOT NULL,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    category TEXT NOT NULL CHECK (char_length(category) BETWEEN 1 AND 100),
    price_minor BIGINT CHECK (price_minor IS NULL OR price_minor >= 0),
    currency TEXT CHECK (currency IS NULL OR currency ~ '^[A-Z]{3}$'),
    dietary_notes TEXT NOT NULL DEFAULT '' CHECK (char_length(dietary_notes) <= 500),
    allergen_notes TEXT NOT NULL DEFAULT '' CHECK (char_length(allergen_notes) <= 500),
    source_id UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    observed_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, menu_item_id),
    FOREIGN KEY (tenant_id, branch_id) REFERENCES restaurant_branches (tenant_id, branch_id) ON DELETE CASCADE,
    CHECK ((price_minor IS NULL) = (currency IS NULL))
);

CREATE INDEX IF NOT EXISTS restaurant_menu_branch_idx
    ON restaurant_menu_items (tenant_id, branch_id, category, name, menu_item_id);

CREATE TABLE IF NOT EXISTS restaurant_review_snapshots (
    tenant_id UUID NOT NULL,
    review_id UUID NOT NULL,
    branch_id UUID NOT NULL,
    source_name TEXT NOT NULL CHECK (char_length(source_name) BETWEEN 1 AND 120),
    passage TEXT NOT NULL CHECK (char_length(passage) BETWEEN 1 AND 2000),
    bias_context TEXT NOT NULL DEFAULT '' CHECK (char_length(bias_context) <= 500),
    sentiment TEXT NOT NULL CHECK (sentiment IN ('positive', 'mixed', 'negative', 'unrated')),
    source_id UUID NOT NULL,
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    captured_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, review_id),
    FOREIGN KEY (tenant_id, branch_id) REFERENCES restaurant_branches (tenant_id, branch_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS restaurant_review_branch_idx
    ON restaurant_review_snapshots (tenant_id, branch_id, captured_at DESC, review_id);

CREATE TABLE IF NOT EXISTS restaurant_recommendations (
    tenant_id UUID NOT NULL,
    recommendation_id UUID NOT NULL,
    branch_id UUID NOT NULL,
    query TEXT NOT NULL CHECK (char_length(query) BETWEEN 1 AND 500),
    reason TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 1000),
    conditions TEXT NOT NULL DEFAULT '' CHECK (char_length(conditions) <= 1000),
    freshness TEXT NOT NULL CHECK (freshness IN ('current', 'aging', 'stale', 'conflicted')),
    supporting_source_id UUID NOT NULL,
    supporting_source_revision BIGINT NOT NULL CHECK (supporting_source_revision > 0),
    contradicting_source_id UUID,
    contradicting_source_revision BIGINT,
    valid_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, recommendation_id),
    FOREIGN KEY (tenant_id, branch_id) REFERENCES restaurant_branches (tenant_id, branch_id) ON DELETE CASCADE,
    CHECK ((contradicting_source_id IS NULL) = (contradicting_source_revision IS NULL)),
    CHECK (contradicting_source_revision IS NULL OR contradicting_source_revision > 0)
);

CREATE INDEX IF NOT EXISTS restaurant_recommendation_freshness_idx
    ON restaurant_recommendations (tenant_id, freshness, valid_at DESC, recommendation_id);

CREATE TABLE IF NOT EXISTS restaurant_visit_outcomes (
    tenant_id UUID NOT NULL,
    outcome_id UUID NOT NULL,
    recommendation_id UUID NOT NULL,
    visited_at TIMESTAMPTZ NOT NULL,
    result TEXT NOT NULL CHECK (result IN ('better_than_expected', 'as_expected', 'worse_than_expected', 'not_visited')),
    feedback TEXT NOT NULL DEFAULT '' CHECK (char_length(feedback) <= 2000),
    actual_cost_minor BIGINT CHECK (actual_cost_minor IS NULL OR actual_cost_minor >= 0),
    currency TEXT CHECK (currency IS NULL OR currency ~ '^[A-Z]{3}$'),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, outcome_id),
    FOREIGN KEY (tenant_id, recommendation_id) REFERENCES restaurant_recommendations (tenant_id, recommendation_id) ON DELETE CASCADE,
    CHECK ((actual_cost_minor IS NULL) = (currency IS NULL))
);

