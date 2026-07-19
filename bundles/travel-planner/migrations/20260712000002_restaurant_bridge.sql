ALTER TABLE travel_itinerary_items
    ADD COLUMN IF NOT EXISTS external_owner_bundle TEXT,
    ADD COLUMN IF NOT EXISTS external_entity_id UUID,
    ADD COLUMN IF NOT EXISTS external_entity_revision BIGINT,
    ADD COLUMN IF NOT EXISTS external_branch_id UUID,
    ADD COLUMN IF NOT EXISTS external_snapshot JSONB,
    ADD COLUMN IF NOT EXISTS supporting_source_id UUID,
    ADD COLUMN IF NOT EXISTS supporting_source_revision BIGINT,
    ADD COLUMN IF NOT EXISTS contradicting_source_id UUID,
    ADD COLUMN IF NOT EXISTS contradicting_source_revision BIGINT;

ALTER TABLE travel_itinerary_items
    DROP CONSTRAINT IF EXISTS travel_itinerary_external_bridge_check;

ALTER TABLE travel_itinerary_items
    ADD CONSTRAINT travel_itinerary_external_bridge_check CHECK (
        (external_owner_bundle IS NULL
            AND external_entity_id IS NULL
            AND external_entity_revision IS NULL
            AND external_branch_id IS NULL
            AND external_snapshot IS NULL
            AND supporting_source_id IS NULL
            AND supporting_source_revision IS NULL
            AND contradicting_source_id IS NULL
            AND contradicting_source_revision IS NULL)
        OR
        (external_owner_bundle = 'restaurant-research'
            AND external_entity_id IS NOT NULL
            AND external_entity_revision > 0
            AND external_branch_id IS NOT NULL
            AND external_snapshot IS NOT NULL
            AND supporting_source_id IS NOT NULL
            AND supporting_source_revision > 0
            AND ((contradicting_source_id IS NULL AND contradicting_source_revision IS NULL)
                OR (contradicting_source_id IS NOT NULL AND contradicting_source_revision > 0)))
    );

CREATE INDEX IF NOT EXISTS travel_itinerary_external_bridge_idx
    ON travel_itinerary_items (tenant_id, external_owner_bundle, external_entity_id)
    WHERE external_owner_bundle IS NOT NULL;
