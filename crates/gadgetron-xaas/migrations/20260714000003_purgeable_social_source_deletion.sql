ALTER TABLE knowledge_sources
    DROP CONSTRAINT IF EXISTS knowledge_sources_locator_check;

ALTER TABLE knowledge_sources
    ADD CONSTRAINT knowledge_sources_locator_check CHECK (
        (source_kind = 'upload' AND requested_uri IS NULL)
        OR (
            source_kind IN ('article', 'social_snapshot')
            AND (status = 'deleted' OR requested_uri IS NOT NULL)
        )
    );
