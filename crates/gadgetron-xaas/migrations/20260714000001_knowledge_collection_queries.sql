-- R3.4d: endpoint-free provider queries retained with generated collection locators.

ALTER TABLE knowledge_collections
    ADD COLUMN queries JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(queries) = 'array');
