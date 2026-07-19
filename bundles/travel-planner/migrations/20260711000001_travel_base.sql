CREATE TABLE IF NOT EXISTS travel_trips (
    tenant_id UUID NOT NULL,
    trip_id UUID NOT NULL,
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    origin TEXT NOT NULL CHECK (char_length(origin) BETWEEN 1 AND 200),
    start_date DATE NOT NULL,
    end_date DATE NOT NULL,
    timezone TEXT NOT NULL CHECK (char_length(timezone) BETWEEN 1 AND 64),
    traveler_count SMALLINT NOT NULL CHECK (traveler_count BETWEEN 1 AND 100),
    status TEXT NOT NULL CHECK (status IN ('draft', 'planned', 'active', 'completed', 'cancelled')),
    currency TEXT NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    budget_amount_minor BIGINT NOT NULL CHECK (budget_amount_minor >= 0),
    notes TEXT NOT NULL DEFAULT '' CHECK (char_length(notes) <= 4096),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, trip_id),
    CHECK (end_date >= start_date)
);

CREATE TABLE IF NOT EXISTS travel_itinerary_items (
    tenant_id UUID NOT NULL,
    item_id UUID NOT NULL,
    trip_id UUID NOT NULL,
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    kind TEXT NOT NULL CHECK (kind IN ('transport', 'lodging', 'activity', 'meal', 'buffer', 'other')),
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL,
    timezone TEXT NOT NULL CHECK (char_length(timezone) BETWEEN 1 AND 64),
    place TEXT NOT NULL CHECK (char_length(place) BETWEEN 1 AND 300),
    status TEXT NOT NULL CHECK (status IN ('proposed', 'planned', 'confirmed', 'completed', 'cancelled')),
    notes TEXT NOT NULL DEFAULT '' CHECK (char_length(notes) <= 4096),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, item_id),
    FOREIGN KEY (tenant_id, trip_id) REFERENCES travel_trips (tenant_id, trip_id) ON DELETE CASCADE,
    CHECK (ends_at > starts_at)
);

CREATE INDEX IF NOT EXISTS travel_itinerary_trip_time_idx
    ON travel_itinerary_items (tenant_id, trip_id, starts_at, item_id);

CREATE TABLE IF NOT EXISTS travel_constraints (
    tenant_id UUID NOT NULL,
    constraint_id UUID NOT NULL,
    trip_id UUID NOT NULL,
    strength TEXT NOT NULL CHECK (strength IN ('hard', 'soft')),
    scope TEXT NOT NULL CHECK (char_length(scope) BETWEEN 1 AND 100),
    rule_text TEXT NOT NULL CHECK (char_length(rule_text) BETWEEN 1 AND 1000),
    provenance TEXT NOT NULL DEFAULT '' CHECK (char_length(provenance) <= 1000),
    conflict_status TEXT NOT NULL CHECK (conflict_status IN ('clear', 'potential', 'violated', 'resolved')),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, constraint_id),
    FOREIGN KEY (tenant_id, trip_id) REFERENCES travel_trips (tenant_id, trip_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS travel_constraints_trip_idx
    ON travel_constraints (tenant_id, trip_id, strength, constraint_id);

CREATE TABLE IF NOT EXISTS travel_budget_items (
    tenant_id UUID NOT NULL,
    budget_item_id UUID NOT NULL,
    trip_id UUID NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('transport', 'lodging', 'food', 'activity', 'fees', 'other')),
    label TEXT NOT NULL CHECK (char_length(label) BETWEEN 1 AND 200),
    quoted_amount_minor BIGINT NOT NULL CHECK (quoted_amount_minor >= 0),
    actual_amount_minor BIGINT CHECK (actual_amount_minor IS NULL OR actual_amount_minor >= 0),
    currency TEXT NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    observed_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, budget_item_id),
    FOREIGN KEY (tenant_id, trip_id) REFERENCES travel_trips (tenant_id, trip_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS travel_budget_trip_idx
    ON travel_budget_items (tenant_id, trip_id, currency, category, budget_item_id);
