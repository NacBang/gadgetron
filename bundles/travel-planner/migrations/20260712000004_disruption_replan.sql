CREATE TABLE IF NOT EXISTS travel_disruptions (
    tenant_id UUID NOT NULL,
    disruption_id UUID NOT NULL,
    trip_id UUID NOT NULL,
    affected_item_id UUID,
    kind TEXT NOT NULL CHECK (kind IN ('delay', 'cancellation', 'closure', 'advisory', 'availability', 'other')),
    severity TEXT NOT NULL CHECK (severity IN ('low', 'medium', 'high', 'critical')),
    summary TEXT NOT NULL CHECK (char_length(summary) BETWEEN 1 AND 500),
    impact TEXT NOT NULL CHECK (char_length(impact) BETWEEN 1 AND 1000),
    source_ref TEXT NOT NULL CHECK (char_length(source_ref) BETWEEN 1 AND 1000),
    observed_at TIMESTAMPTZ NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'resolved')),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, disruption_id),
    FOREIGN KEY (tenant_id, trip_id)
        REFERENCES travel_trips (tenant_id, trip_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, affected_item_id)
        REFERENCES travel_itinerary_items (tenant_id, item_id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS travel_disruptions_open_idx
    ON travel_disruptions (tenant_id, state, observed_at DESC, disruption_id);

CREATE TABLE IF NOT EXISTS travel_replans (
    tenant_id UUID NOT NULL,
    proposal_id UUID NOT NULL,
    disruption_id UUID NOT NULL,
    trip_id UUID NOT NULL,
    item_id UUID NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('proposed', 'applied', 'rolled_back', 'safe_stopped')
    ),
    reason TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 1000),
    evidence_ref TEXT NOT NULL CHECK (char_length(evidence_ref) BETWEEN 1 AND 1000),
    booking_impact TEXT NOT NULL CHECK (
        booking_impact IN ('none', 'manual_change', 'cancellation')
    ),
    cost_change_minor BIGINT NOT NULL,
    expected_item_revision BIGINT NOT NULL CHECK (expected_item_revision > 0),
    before_state JSONB NOT NULL,
    proposed_state JSONB NOT NULL,
    applied_item_revision BIGINT,
    operation_id UUID,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, proposal_id),
    FOREIGN KEY (tenant_id, disruption_id)
        REFERENCES travel_disruptions (tenant_id, disruption_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, trip_id)
        REFERENCES travel_trips (tenant_id, trip_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES travel_itinerary_items (tenant_id, item_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS travel_replans_disruption_idx
    ON travel_replans (tenant_id, disruption_id, created_at DESC, proposal_id);

CREATE TABLE IF NOT EXISTS travel_operation_outcomes (
    tenant_id UUID NOT NULL,
    operation_id UUID NOT NULL,
    proposal_id UUID NOT NULL,
    target_item_id UUID NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('apply_replan', 'rollback_replan')),
    before_state JSONB NOT NULL,
    after_state JSONB NOT NULL,
    observed_outcome TEXT NOT NULL CHECK (
        observed_outcome IN ('succeeded', 'failed', 'indeterminate')
    ),
    attempts SMALLINT NOT NULL CHECK (attempts BETWEEN 0 AND 2),
    actor_ref TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, operation_id),
    FOREIGN KEY (tenant_id, proposal_id)
        REFERENCES travel_replans (tenant_id, proposal_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, target_item_id)
        REFERENCES travel_itinerary_items (tenant_id, item_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS travel_operation_outcomes_proposal_idx
    ON travel_operation_outcomes (tenant_id, proposal_id, created_at DESC);
