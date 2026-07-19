-- Public social responses must be removable without rewriting ordinary
-- versioned article/upload semantics. The Source ledger marks this storage
-- class explicitly; Core keeps its raw bytes outside Git-backed Vault notes.

ALTER TABLE knowledge_sources
    DROP CONSTRAINT knowledge_sources_source_kind_check,
    DROP CONSTRAINT knowledge_sources_check;

ALTER TABLE knowledge_sources
    ADD CONSTRAINT knowledge_sources_source_kind_check
        CHECK (source_kind IN ('upload', 'article', 'social_snapshot')),
    ADD CONSTRAINT knowledge_sources_locator_check
        CHECK (
            (source_kind = 'upload' AND requested_uri IS NULL)
            OR (source_kind IN ('article', 'social_snapshot') AND requested_uri IS NOT NULL)
        );
