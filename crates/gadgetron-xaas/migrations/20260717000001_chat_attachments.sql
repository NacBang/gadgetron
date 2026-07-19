-- K0.3 Penny context intake: conversation-scoped, purgeable Sources.
--
-- Attachments keep using the existing Source ledger and Vault ACLs. The
-- nullable conversation link also permits explicitly selected, versioned
-- Vault Sources to participate in a chat without changing their retention.

ALTER TABLE knowledge_sources
    ADD COLUMN conversation_id UUID,
    ADD CONSTRAINT knowledge_sources_conversation_fk
        FOREIGN KEY (conversation_id) REFERENCES conversations(id);

ALTER TABLE knowledge_sources
    DROP CONSTRAINT knowledge_sources_source_kind_check,
    DROP CONSTRAINT knowledge_sources_locator_check;

ALTER TABLE knowledge_sources
    ADD CONSTRAINT knowledge_sources_source_kind_check
        CHECK (source_kind IN ('upload', 'article', 'social_snapshot', 'chat_attachment')),
    ADD CONSTRAINT knowledge_sources_locator_check CHECK (
        (source_kind = 'upload' AND requested_uri IS NULL)
        OR source_kind = 'chat_attachment'
        OR (
            source_kind IN ('article', 'social_snapshot')
            AND (status = 'deleted' OR requested_uri IS NOT NULL)
        )
    ),
    ADD CONSTRAINT knowledge_sources_chat_conversation_check CHECK (
        source_kind <> 'chat_attachment' OR conversation_id IS NOT NULL
    );

CREATE INDEX idx_knowledge_sources_conversation
    ON knowledge_sources (tenant_id, conversation_id, created_at DESC)
    WHERE conversation_id IS NOT NULL AND deleted_at IS NULL;
