-- E4-R2/R2.3: rebuildable tenant Knowledge Graph generations.
--
-- Vault/Source/Object revisions remain canonical. These tables are a derived
-- query plane and may be deleted then rebuilt from canonical state.

CREATE TABLE knowledge_graph_generations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id),
    schema_version  INTEGER NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    state           TEXT NOT NULL CHECK (state IN ('building', 'active', 'superseded', 'failed')),
    input_digest    TEXT NOT NULL CHECK (input_digest ~ '^sha256:[0-9a-f]{64}$'),
    graph_revision  BIGINT NOT NULL DEFAULT 1 CHECK (graph_revision > 0),
    node_count      INTEGER NOT NULL DEFAULT 0 CHECK (node_count BETWEEN 0 AND 50000),
    edge_count      INTEGER NOT NULL DEFAULT 0 CHECK (edge_count BETWEEN 0 AND 200000),
    built_by        UUID NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    activated_at    TIMESTAMPTZ,
    superseded_at   TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, built_by) REFERENCES users(tenant_id, id)
);

CREATE UNIQUE INDEX knowledge_graph_one_active_generation
    ON knowledge_graph_generations (tenant_id) WHERE state = 'active';
CREATE INDEX knowledge_graph_generation_history
    ON knowledge_graph_generations (tenant_id, created_at DESC);

CREATE TABLE knowledge_graph_nodes (
    generation_id       UUID NOT NULL,
    tenant_id           UUID NOT NULL,
    stable_node_id      TEXT NOT NULL CHECK (length(stable_node_id) BETWEEN 3 AND 512),
    space_id            UUID NOT NULL,
    vault_id            UUID,
    node_kind           TEXT NOT NULL CHECK (length(node_kind) BETWEEN 1 AND 64),
    canonical_id        UUID,
    canonical_revision  BIGINT NOT NULL CHECK (canonical_revision >= 0),
    home_bundle_id      TEXT NOT NULL CHECK (length(home_bundle_id) BETWEEN 1 AND 128),
    title               TEXT NOT NULL DEFAULT '' CHECK (length(title) <= 512),
    status              TEXT NOT NULL CHECK (status IN ('active', 'owner_unavailable', 'tombstone')),
    freshness           TEXT NOT NULL CHECK (freshness IN ('current', 'stale')),
    content_hash        TEXT,
    metadata            JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
    PRIMARY KEY (tenant_id, generation_id, stable_node_id),
    FOREIGN KEY (tenant_id, generation_id)
        REFERENCES knowledge_graph_generations(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, vault_id) REFERENCES knowledge_vaults(tenant_id, id)
);

CREATE INDEX knowledge_graph_nodes_space_kind
    ON knowledge_graph_nodes (tenant_id, generation_id, space_id, node_kind, stable_node_id);
CREATE INDEX knowledge_graph_nodes_canonical
    ON knowledge_graph_nodes (tenant_id, canonical_id) WHERE canonical_id IS NOT NULL;

CREATE TABLE knowledge_graph_edges (
    generation_id       UUID NOT NULL,
    tenant_id           UUID NOT NULL,
    stable_edge_id      TEXT NOT NULL CHECK (stable_edge_id ~ '^[0-9a-f]{64}$'),
    from_node_id        TEXT NOT NULL,
    to_node_id          TEXT,
    target_ref          TEXT NOT NULL CHECK (length(target_ref) BETWEEN 1 AND 1024),
    relation_kind       TEXT NOT NULL CHECK (relation_kind ~ '^[a-z][a-z0-9_-]*(:[a-z][a-z0-9_-]*)?$'),
    source_space_id     UUID NOT NULL,
    target_space_id     UUID,
    home_bundle_id      TEXT NOT NULL CHECK (length(home_bundle_id) BETWEEN 1 AND 128),
    producer_kind       TEXT NOT NULL CHECK (producer_kind IN ('system', 'import', 'user', 'agent', 'bundle')),
    producer_revision   BIGINT NOT NULL CHECK (producer_revision >= 0),
    status              TEXT NOT NULL CHECK (status IN ('active', 'broken', 'stale', 'owner_unavailable')),
    evidence            JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(evidence) = 'object'),
    PRIMARY KEY (tenant_id, generation_id, stable_edge_id),
    FOREIGN KEY (tenant_id, generation_id, from_node_id)
        REFERENCES knowledge_graph_nodes(tenant_id, generation_id, stable_node_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, generation_id, to_node_id)
        REFERENCES knowledge_graph_nodes(tenant_id, generation_id, stable_node_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, source_space_id) REFERENCES knowledge_spaces(tenant_id, id),
    FOREIGN KEY (tenant_id, target_space_id) REFERENCES knowledge_spaces(tenant_id, id)
);

CREATE INDEX knowledge_graph_edges_outgoing
    ON knowledge_graph_edges (tenant_id, generation_id, source_space_id, from_node_id, relation_kind);
CREATE INDEX knowledge_graph_edges_incoming
    ON knowledge_graph_edges (tenant_id, generation_id, target_space_id, to_node_id, relation_kind)
    WHERE to_node_id IS NOT NULL;
CREATE INDEX knowledge_graph_edges_diagnostics
    ON knowledge_graph_edges (tenant_id, generation_id, status, relation_kind);

GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_graph_generations TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_graph_nodes TO gadgetron_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON knowledge_graph_edges TO gadgetron_app;
