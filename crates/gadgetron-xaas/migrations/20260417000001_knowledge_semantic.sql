CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE wiki_pages (
    page_name TEXT PRIMARY KEY,
    frontmatter JSONB NOT NULL DEFAULT '{}',
    content_hash TEXT NOT NULL,
    tenant_id TEXT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX wiki_pages_tags_idx ON wiki_pages USING GIN ((frontmatter->'tags'));
CREATE INDEX wiki_pages_tenant_id_idx
    ON wiki_pages (tenant_id)
    WHERE tenant_id IS NOT NULL;

CREATE TABLE wiki_chunks (
    id BIGSERIAL PRIMARY KEY,
    page_name TEXT NOT NULL,
    chunk_index INT NOT NULL,
    section TEXT,
    content TEXT NOT NULL,
    content_tsv TSVECTOR GENERATED ALWAYS AS (to_tsvector('simple', content)) STORED,
    embedding vector(1536),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(page_name, chunk_index)
);

CREATE INDEX wiki_chunks_embedding_idx
    ON wiki_chunks USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX wiki_chunks_tsv_idx ON wiki_chunks USING GIN (content_tsv);
CREATE INDEX wiki_chunks_page_name_idx ON wiki_chunks (page_name);
