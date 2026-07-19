CREATE UNIQUE INDEX IF NOT EXISTS news_article_source_revision_unique
    ON news_article_snapshots (tenant_id, topic_id, source_id, source_revision);
