//! R2.3 rebuildable PostgreSQL Knowledge Graph generation store.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::knowledge_spaces::KnowledgeSpaceError;

pub const GRAPH_SCHEMA_VERSION: i32 = 1;
pub const MAX_GRAPH_NODES: usize = 50_000;
pub const MAX_GRAPH_EDGES: usize = 200_000;
pub const MAX_QUERY_NODES: usize = 200;
pub const MAX_QUERY_EDGES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNodeInput {
    pub stable_node_id: String,
    pub space_id: Uuid,
    pub vault_id: Option<Uuid>,
    pub node_kind: String,
    pub canonical_id: Option<Uuid>,
    pub canonical_revision: i64,
    pub home_bundle_id: String,
    pub title: String,
    pub status: String,
    pub freshness: String,
    pub content_hash: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdgeInput {
    pub stable_edge_id: String,
    pub from_node_id: String,
    pub to_node_id: Option<String>,
    pub target_ref: String,
    pub relation_kind: String,
    pub source_space_id: Uuid,
    pub target_space_id: Option<Uuid>,
    pub home_bundle_id: String,
    pub producer_kind: String,
    pub producer_revision: i64,
    pub status: String,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSnapshotInput {
    pub nodes: Vec<GraphNodeInput>,
    pub edges: Vec<GraphEdgeInput>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GraphGenerationRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub schema_version: i32,
    pub state: String,
    pub input_digest: String,
    pub graph_revision: i64,
    pub node_count: i32,
    pub edge_count: i32,
    pub built_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub superseded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GraphNodeRow {
    pub generation_id: Uuid,
    pub tenant_id: Uuid,
    pub stable_node_id: String,
    pub space_id: Uuid,
    pub vault_id: Option<Uuid>,
    pub node_kind: String,
    pub canonical_id: Option<Uuid>,
    pub canonical_revision: i64,
    pub home_bundle_id: String,
    pub title: String,
    pub status: String,
    pub freshness: String,
    pub content_hash: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GraphEdgeRow {
    pub generation_id: Uuid,
    pub tenant_id: Uuid,
    pub stable_edge_id: String,
    pub from_node_id: String,
    pub to_node_id: Option<String>,
    pub target_ref: String,
    pub relation_kind: String,
    pub source_space_id: Uuid,
    pub target_space_id: Option<Uuid>,
    pub home_bundle_id: String,
    pub producer_kind: String,
    pub producer_revision: i64,
    pub status: String,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileMode {
    Full,
    Incremental,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphMaterializeResult {
    pub generation: GraphGenerationRow,
    pub changed: bool,
    pub mode: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphDirection {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Clone, Copy)]
struct GraphReadScope<'a> {
    pool: &'a PgPool,
    tenant_id: Uuid,
    generation_id: Uuid,
    visible_spaces: &'a [Uuid],
}

#[derive(Debug, Clone)]
pub struct NeighborhoodQuery {
    pub center_node_id: String,
    pub max_depth: u8,
    pub node_limit: usize,
    pub edge_limit: usize,
    pub direction: GraphDirection,
    pub relation_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphNeighborhood {
    pub generation: GraphGenerationRow,
    pub center_node_id: String,
    pub nodes: Vec<GraphNodeRow>,
    pub edges: Vec<GraphEdgeRow>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct PathQuery {
    pub from_node_id: String,
    pub to_node_id: String,
    pub max_depth: u8,
    pub max_paths: usize,
    pub relation_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphPath {
    pub node_ids: Vec<String>,
    pub edge_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphPathResult {
    pub generation: GraphGenerationRow,
    pub paths: Vec<GraphPath>,
    pub nodes: Vec<GraphNodeRow>,
    pub edges: Vec<GraphEdgeRow>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphDiagnostics {
    pub generation: GraphGenerationRow,
    pub orphan_nodes: Vec<GraphNodeRow>,
    pub broken_edges: Vec<GraphEdgeRow>,
    pub stale_nodes: Vec<GraphNodeRow>,
    pub contradiction_edges: Vec<GraphEdgeRow>,
    pub truncated: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GraphSourceInputRow {
    pub id: Uuid,
    pub vault_id: Uuid,
    pub space_id: Uuid,
    pub home_bundle_id: String,
    pub source_kind: String,
    pub status: String,
    pub title: String,
    pub requested_uri: Option<String>,
    pub final_uri: Option<String>,
    pub content_type: Option<String>,
    pub content_hash: Option<String>,
    pub revision: i64,
    pub owner_state: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GraphObjectInputRow {
    pub id: Uuid,
    pub vault_id: Uuid,
    pub space_id: Uuid,
    pub home_bundle_id: String,
    pub path: String,
    pub status: String,
    pub content_hash: Option<String>,
    pub revision: i64,
    pub source_id: Option<Uuid>,
    pub owner_state: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GraphShareInputRow {
    pub id: Uuid,
    pub source_space_id: Uuid,
    pub source_object_id: Uuid,
    pub source_revision: i64,
    pub target_space_id: Uuid,
    pub mode: String,
    pub follow_latest: bool,
    pub target_object_id: Option<Uuid>,
    pub policy_disposition: String,
    pub revision: i64,
    pub home_bundle_id: String,
}

impl GraphSnapshotInput {
    pub fn normalize_and_validate(mut self) -> Result<(Self, String), KnowledgeSpaceError> {
        if self.nodes.len() > MAX_GRAPH_NODES || self.edges.len() > MAX_GRAPH_EDGES {
            return Err(KnowledgeSpaceError::InvalidInput(format!(
                "graph exceeds v1 ceiling: {} nodes/{MAX_GRAPH_NODES}, {} edges/{MAX_GRAPH_EDGES}",
                self.nodes.len(),
                self.edges.len()
            )));
        }
        self.nodes
            .sort_by(|left, right| left.stable_node_id.cmp(&right.stable_node_id));
        self.edges
            .sort_by(|left, right| left.stable_edge_id.cmp(&right.stable_edge_id));
        let mut node_ids = HashSet::with_capacity(self.nodes.len());
        for node in &self.nodes {
            if !node_ids.insert(node.stable_node_id.as_str()) {
                return Err(KnowledgeSpaceError::InvalidInput(format!(
                    "duplicate graph node id {:?}",
                    node.stable_node_id
                )));
            }
            validate_node(node)?;
        }
        let mut edge_ids = HashSet::with_capacity(self.edges.len());
        for edge in &self.edges {
            if !edge_ids.insert(edge.stable_edge_id.as_str()) {
                return Err(KnowledgeSpaceError::InvalidInput(format!(
                    "duplicate graph edge id {:?}",
                    edge.stable_edge_id
                )));
            }
            if !node_ids.contains(edge.from_node_id.as_str())
                || edge
                    .to_node_id
                    .as_deref()
                    .is_some_and(|id| !node_ids.contains(id))
            {
                return Err(KnowledgeSpaceError::InvalidInput(format!(
                    "edge {:?} references a missing materialized node",
                    edge.stable_edge_id
                )));
            }
            validate_edge(edge)?;
        }
        let bytes = serde_json::to_vec(&self).map_err(|error| {
            KnowledgeSpaceError::InvalidInput(format!("graph canonical JSON failed: {error}"))
        })?;
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        Ok((self, digest))
    }
}

pub async fn require_tenant_admin(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), KnowledgeSpaceError> {
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM users WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if role.as_deref() == Some("admin") {
        Ok(())
    } else {
        Err(KnowledgeSpaceError::Forbidden)
    }
}

pub async fn canonical_inputs(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<
    (
        Vec<GraphSourceInputRow>,
        Vec<GraphObjectInputRow>,
        Vec<GraphShareInputRow>,
    ),
    KnowledgeSpaceError,
> {
    let sources = sqlx::query_as::<_, GraphSourceInputRow>(
        r#"SELECT s.id, s.vault_id, v.space_id, v.home_bundle_id, s.source_kind,
                  s.status, s.title, s.requested_uri, s.final_uri, s.content_type,
                  s.content_hash, s.revision, v.owner_state
           FROM knowledge_sources s
           JOIN knowledge_vaults v ON v.tenant_id = s.tenant_id AND v.id = s.vault_id
           WHERE s.tenant_id = $1
           ORDER BY s.id"#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    let objects = sqlx::query_as::<_, GraphObjectInputRow>(
        r#"SELECT o.id, o.vault_id, v.space_id, v.home_bundle_id, o.path, o.status,
                  o.content_hash, o.revision, o.source_id, v.owner_state
           FROM knowledge_objects o
           JOIN knowledge_vaults v ON v.tenant_id = o.tenant_id AND v.id = o.vault_id
           WHERE o.tenant_id = $1 AND o.canonical_kind = 'note'
           ORDER BY o.id"#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    let shares = sqlx::query_as::<_, GraphShareInputRow>(
        r#"SELECT sh.id, sh.source_space_id, sh.source_object_id, sh.source_revision,
                  sh.target_space_id, sh.mode, sh.follow_latest, sh.target_object_id,
                  sh.policy_disposition, sh.revision, v.home_bundle_id
           FROM knowledge_shares sh
           JOIN knowledge_objects o
             ON o.tenant_id = sh.tenant_id AND o.id = sh.source_object_id
           JOIN knowledge_vaults v
             ON v.tenant_id = o.tenant_id AND v.id = o.vault_id
           WHERE sh.tenant_id = $1 AND sh.revoked_at IS NULL
           ORDER BY sh.id"#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok((sources, objects, shares))
}

pub async fn materialize(
    pool: &PgPool,
    tenant_id: Uuid,
    built_by: Uuid,
    mode: ReconcileMode,
    snapshot: GraphSnapshotInput,
) -> Result<GraphMaterializeResult, KnowledgeSpaceError> {
    let (snapshot, digest) = snapshot.normalize_and_validate()?;
    let mut tx = pool.begin().await?;
    lock_tenant(&mut tx, tenant_id).await?;
    require_tenant_user(&mut tx, tenant_id, built_by).await?;
    let active = active_generation_tx(&mut tx, tenant_id).await?;
    if active
        .as_ref()
        .is_some_and(|generation| generation.input_digest == digest)
    {
        tx.commit().await?;
        return Ok(GraphMaterializeResult {
            generation: active.expect("checked Some active generation"),
            changed: false,
            mode: "noop",
        });
    }
    let result = match (mode, active) {
        (ReconcileMode::Incremental, Some(generation)) => {
            apply_delta(&mut tx, tenant_id, generation, &digest, &snapshot).await?
        }
        (_, active) => {
            replace_generation(&mut tx, tenant_id, built_by, active, &digest, &snapshot).await?
        }
    };
    tx.commit().await?;
    Ok(result)
}

pub async fn active_generation(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<GraphGenerationRow, KnowledgeSpaceError> {
    sqlx::query_as::<_, GraphGenerationRow>(
        r#"SELECT id, tenant_id, schema_version, state, input_digest, graph_revision,
                  node_count, edge_count, built_by, created_at, activated_at, superseded_at
           FROM knowledge_graph_generations
           WHERE tenant_id = $1 AND state = 'active'"#,
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)
}

pub async fn get_node(
    pool: &PgPool,
    tenant_id: Uuid,
    visible_spaces: &[Uuid],
    stable_node_id: &str,
) -> Result<GraphNodeRow, KnowledgeSpaceError> {
    if visible_spaces.is_empty() {
        return Err(KnowledgeSpaceError::NotFound);
    }
    let generation = active_generation(pool, tenant_id).await?;
    sqlx::query_as::<_, GraphNodeRow>(
        r#"SELECT generation_id, tenant_id, stable_node_id, space_id, vault_id,
                  node_kind, canonical_id, canonical_revision, home_bundle_id, title,
                  status, freshness, content_hash, metadata
           FROM knowledge_graph_nodes
           WHERE tenant_id = $1 AND generation_id = $2 AND stable_node_id = $3
             AND space_id = ANY($4) AND status <> 'tombstone'
             AND (node_kind <> 'reference'
                  OR (metadata->>'source_space_id')::UUID = ANY($4))"#,
    )
    .bind(tenant_id)
    .bind(generation.id)
    .bind(stable_node_id)
    .bind(visible_spaces)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)
}

pub async fn neighborhood(
    pool: &PgPool,
    tenant_id: Uuid,
    visible_spaces: &[Uuid],
    query: NeighborhoodQuery,
) -> Result<GraphNeighborhood, KnowledgeSpaceError> {
    validate_neighborhood(&query)?;
    let generation = active_generation(pool, tenant_id).await?;
    let center = get_node(pool, tenant_id, visible_spaces, &query.center_node_id).await?;
    let scope = GraphReadScope {
        pool,
        tenant_id,
        generation_id: generation.id,
        visible_spaces,
    };
    let mut nodes = HashMap::from([(center.stable_node_id.clone(), center)]);
    let mut edges = HashMap::new();
    let mut frontier = vec![query.center_node_id.clone()];
    let mut truncated = false;
    for _ in 0..query.max_depth {
        if frontier.is_empty() || nodes.len() >= query.node_limit || edges.len() >= query.edge_limit
        {
            truncated |= !frontier.is_empty();
            break;
        }
        let fetched = fetch_adjacent_edges(
            scope,
            &frontier,
            query.direction,
            &query.relation_kinds,
            query.edge_limit.saturating_sub(edges.len()) + 1,
        )
        .await?;
        if fetched.len() > query.edge_limit.saturating_sub(edges.len()) {
            truncated = true;
        }
        let mut next_ids = BTreeSet::new();
        for edge in fetched
            .into_iter()
            .take(query.edge_limit.saturating_sub(edges.len()))
        {
            if edge.from_node_id != query.center_node_id {
                next_ids.insert(edge.from_node_id.clone());
            }
            if let Some(to) = &edge.to_node_id {
                if to != &query.center_node_id {
                    next_ids.insert(to.clone());
                }
            }
            edges.insert(edge.stable_edge_id.clone(), edge);
        }
        next_ids.retain(|id| !nodes.contains_key(id));
        let room = query.node_limit.saturating_sub(nodes.len());
        if next_ids.len() > room {
            truncated = true;
        }
        let ids: Vec<_> = next_ids.into_iter().take(room).collect();
        let fetched_nodes = fetch_nodes(scope, &ids).await?;
        frontier = fetched_nodes
            .iter()
            .map(|node| node.stable_node_id.clone())
            .collect();
        for node in fetched_nodes {
            nodes.insert(node.stable_node_id.clone(), node);
        }
    }
    let mut nodes: Vec<_> = nodes.into_values().collect();
    let mut edges: Vec<_> = edges.into_values().collect();
    nodes.sort_by(|left, right| left.stable_node_id.cmp(&right.stable_node_id));
    edges.sort_by(|left, right| left.stable_edge_id.cmp(&right.stable_edge_id));
    Ok(GraphNeighborhood {
        generation,
        center_node_id: query.center_node_id,
        nodes,
        edges,
        truncated,
    })
}

pub async fn paths(
    pool: &PgPool,
    tenant_id: Uuid,
    visible_spaces: &[Uuid],
    query: PathQuery,
) -> Result<GraphPathResult, KnowledgeSpaceError> {
    if query.max_depth == 0 || query.max_depth > 6 || !(1..=10).contains(&query.max_paths) {
        return Err(KnowledgeSpaceError::InvalidInput(
            "graph path requires depth 1..=6 and max_paths 1..=10".to_string(),
        ));
    }
    let generation = active_generation(pool, tenant_id).await?;
    get_node(pool, tenant_id, visible_spaces, &query.from_node_id).await?;
    get_node(pool, tenant_id, visible_spaces, &query.to_node_id).await?;
    let scope = GraphReadScope {
        pool,
        tenant_id,
        generation_id: generation.id,
        visible_spaces,
    };
    let mut queue = VecDeque::from([GraphPath {
        node_ids: vec![query.from_node_id.clone()],
        edge_ids: Vec::new(),
    }]);
    let mut found = Vec::new();
    let mut seen_edges = HashMap::<String, GraphEdgeRow>::new();
    let mut truncated = false;
    while let Some(path) = queue.pop_front() {
        if path.edge_ids.len() >= usize::from(query.max_depth) {
            continue;
        }
        let current = path.node_ids.last().expect("path always has a node");
        let outgoing = fetch_adjacent_edges(
            scope,
            std::slice::from_ref(current),
            GraphDirection::Outgoing,
            &query.relation_kinds,
            MAX_QUERY_EDGES + 1,
        )
        .await?;
        if outgoing.len() > MAX_QUERY_EDGES {
            truncated = true;
        }
        for edge in outgoing.into_iter().take(MAX_QUERY_EDGES) {
            let Some(next) = edge.to_node_id.clone() else {
                continue;
            };
            if edge.status == "broken" || path.node_ids.contains(&next) {
                continue;
            }
            seen_edges.insert(edge.stable_edge_id.clone(), edge.clone());
            let mut candidate = path.clone();
            candidate.node_ids.push(next.clone());
            candidate.edge_ids.push(edge.stable_edge_id);
            if next == query.to_node_id {
                found.push(candidate);
                if found.len() >= query.max_paths {
                    truncated |= !queue.is_empty();
                    queue.clear();
                    break;
                }
            } else if queue.len() < MAX_QUERY_NODES {
                queue.push_back(candidate);
            } else {
                truncated = true;
            }
        }
    }
    let node_ids: BTreeSet<_> = found
        .iter()
        .flat_map(|path| path.node_ids.iter().cloned())
        .collect();
    let nodes = fetch_nodes(scope, &node_ids.into_iter().collect::<Vec<_>>()).await?;
    let mut edges: Vec<_> = seen_edges.into_values().collect();
    edges.sort_by(|left, right| left.stable_edge_id.cmp(&right.stable_edge_id));
    Ok(GraphPathResult {
        generation,
        paths: found,
        nodes,
        edges,
        truncated,
    })
}

pub async fn diagnostics(
    pool: &PgPool,
    tenant_id: Uuid,
    visible_spaces: &[Uuid],
    limit: usize,
) -> Result<GraphDiagnostics, KnowledgeSpaceError> {
    if visible_spaces.is_empty() {
        return Err(KnowledgeSpaceError::NotFound);
    }
    if !(1..=100).contains(&limit) {
        return Err(KnowledgeSpaceError::InvalidInput(
            "diagnostics limit must be 1..=100".to_string(),
        ));
    }
    let generation = active_generation(pool, tenant_id).await?;
    let orphan_nodes = sqlx::query_as::<_, GraphNodeRow>(&format!(
        r#"SELECT {} FROM knowledge_graph_nodes n
           WHERE n.tenant_id = $1 AND n.generation_id = $2 AND n.space_id = ANY($3)
             AND n.status <> 'tombstone'
             AND (n.node_kind <> 'reference'
                  OR (n.metadata->>'source_space_id')::UUID = ANY($3))
             AND NOT EXISTS (
               SELECT 1 FROM knowledge_graph_edges e
               WHERE e.tenant_id = n.tenant_id AND e.generation_id = n.generation_id
                 AND (e.from_node_id = n.stable_node_id OR e.to_node_id = n.stable_node_id))
           ORDER BY n.stable_node_id LIMIT $4"#,
        node_columns("n")
    ))
    .bind(tenant_id)
    .bind(generation.id)
    .bind(visible_spaces)
    .bind(limit as i64 + 1)
    .fetch_all(pool)
    .await?;
    let broken_edges = diagnostic_edges(
        pool,
        tenant_id,
        generation.id,
        visible_spaces,
        "status = 'broken'",
        limit,
    )
    .await?;
    let stale_nodes = sqlx::query_as::<_, GraphNodeRow>(&format!(
        "SELECT {} FROM knowledge_graph_nodes n WHERE n.tenant_id = $1 AND n.generation_id = $2 AND n.space_id = ANY($3) AND (n.node_kind <> 'reference' OR (n.metadata->>'source_space_id')::UUID = ANY($3)) AND n.freshness = 'stale' ORDER BY n.stable_node_id LIMIT $4",
        node_columns("n")
    ))
    .bind(tenant_id)
    .bind(generation.id)
    .bind(visible_spaces)
    .bind(limit as i64 + 1)
    .fetch_all(pool)
    .await?;
    let contradiction_edges = diagnostic_edges(
        pool,
        tenant_id,
        generation.id,
        visible_spaces,
        "relation_kind = 'contradicts' AND status <> 'broken'",
        limit,
    )
    .await?;
    let truncated = [
        orphan_nodes.len(),
        broken_edges.len(),
        stale_nodes.len(),
        contradiction_edges.len(),
    ]
    .into_iter()
    .any(|count| count > limit);
    Ok(GraphDiagnostics {
        generation,
        orphan_nodes: orphan_nodes.into_iter().take(limit).collect(),
        broken_edges: broken_edges.into_iter().take(limit).collect(),
        stale_nodes: stale_nodes.into_iter().take(limit).collect(),
        contradiction_edges: contradiction_edges.into_iter().take(limit).collect(),
        truncated,
    })
}

async fn replace_generation(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    built_by: Uuid,
    active: Option<GraphGenerationRow>,
    digest: &str,
    snapshot: &GraphSnapshotInput,
) -> Result<GraphMaterializeResult, KnowledgeSpaceError> {
    let generation_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO knowledge_graph_generations
           (id, tenant_id, schema_version, state, input_digest, node_count, edge_count, built_by)
           VALUES ($1, $2, $3, 'building', $4, $5, $6, $7)"#,
    )
    .bind(generation_id)
    .bind(tenant_id)
    .bind(GRAPH_SCHEMA_VERSION)
    .bind(digest)
    .bind(snapshot.nodes.len() as i32)
    .bind(snapshot.edges.len() as i32)
    .bind(built_by)
    .execute(&mut **tx)
    .await?;
    insert_nodes(tx, tenant_id, generation_id, &snapshot.nodes).await?;
    insert_edges(tx, tenant_id, generation_id, &snapshot.edges).await?;
    if let Some(active) = active {
        sqlx::query(
            "UPDATE knowledge_graph_generations SET state = 'superseded', superseded_at = NOW() WHERE tenant_id = $1 AND id = $2 AND state = 'active'",
        )
        .bind(tenant_id)
        .bind(active.id)
        .execute(&mut **tx)
        .await?;
    }
    let generation = sqlx::query_as::<_, GraphGenerationRow>(&format!(
        "UPDATE knowledge_graph_generations SET state = 'active', activated_at = NOW() WHERE tenant_id = $1 AND id = $2 AND state = 'building' RETURNING {}",
        generation_columns()
    ))
    .bind(tenant_id)
    .bind(generation_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(GraphMaterializeResult {
        generation,
        changed: true,
        mode: "full",
    })
}

async fn apply_delta(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    generation: GraphGenerationRow,
    digest: &str,
    snapshot: &GraphSnapshotInput,
) -> Result<GraphMaterializeResult, KnowledgeSpaceError> {
    let node_ids: Vec<_> = snapshot
        .nodes
        .iter()
        .map(|node| node.stable_node_id.clone())
        .collect();
    let edge_ids: Vec<_> = snapshot
        .edges
        .iter()
        .map(|edge| edge.stable_edge_id.clone())
        .collect();
    sqlx::query(
        "DELETE FROM knowledge_graph_edges WHERE tenant_id = $1 AND generation_id = $2 AND NOT (stable_edge_id = ANY($3))",
    )
    .bind(tenant_id)
    .bind(generation.id)
    .bind(&edge_ids)
    .execute(&mut **tx)
    .await?;
    upsert_nodes(tx, tenant_id, generation.id, &snapshot.nodes).await?;
    upsert_edges(tx, tenant_id, generation.id, &snapshot.edges).await?;
    // Desired edges have already cleared references to vanished nodes.
    // Deleting nodes before this upsert would trip the FK when a Source
    // deletion changes a resolved edge into a broken edge.
    sqlx::query(
        "DELETE FROM knowledge_graph_nodes WHERE tenant_id = $1 AND generation_id = $2 AND NOT (stable_node_id = ANY($3))",
    )
    .bind(tenant_id)
    .bind(generation.id)
    .bind(&node_ids)
    .execute(&mut **tx)
    .await?;
    let generation = sqlx::query_as::<_, GraphGenerationRow>(&format!(
        r#"UPDATE knowledge_graph_generations SET input_digest = $3,
             graph_revision = graph_revision + 1, node_count = $4, edge_count = $5,
             activated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND state = 'active' RETURNING {}"#,
        generation_columns()
    ))
    .bind(tenant_id)
    .bind(generation.id)
    .bind(digest)
    .bind(snapshot.nodes.len() as i32)
    .bind(snapshot.edges.len() as i32)
    .fetch_one(&mut **tx)
    .await?;
    Ok(GraphMaterializeResult {
        generation,
        changed: true,
        mode: "incremental",
    })
}

async fn insert_nodes(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    generation_id: Uuid,
    nodes: &[GraphNodeInput],
) -> Result<(), KnowledgeSpaceError> {
    write_nodes(tx, tenant_id, generation_id, nodes, false).await
}

async fn upsert_nodes(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    generation_id: Uuid,
    nodes: &[GraphNodeInput],
) -> Result<(), KnowledgeSpaceError> {
    write_nodes(tx, tenant_id, generation_id, nodes, true).await
}

async fn write_nodes(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    generation_id: Uuid,
    nodes: &[GraphNodeInput],
    upsert: bool,
) -> Result<(), KnowledgeSpaceError> {
    if nodes.is_empty() {
        return Ok(());
    }
    let payload = serde_json::to_value(nodes).map_err(json_error)?;
    let conflict = if upsert {
        r#"ON CONFLICT (tenant_id, generation_id, stable_node_id) DO UPDATE SET
           space_id = EXCLUDED.space_id, vault_id = EXCLUDED.vault_id,
           node_kind = EXCLUDED.node_kind, canonical_id = EXCLUDED.canonical_id,
           canonical_revision = EXCLUDED.canonical_revision,
           home_bundle_id = EXCLUDED.home_bundle_id, title = EXCLUDED.title,
           status = EXCLUDED.status, freshness = EXCLUDED.freshness,
           content_hash = EXCLUDED.content_hash, metadata = EXCLUDED.metadata"#
    } else {
        ""
    };
    sqlx::query(&format!(
        r#"INSERT INTO knowledge_graph_nodes
           (generation_id, tenant_id, stable_node_id, space_id, vault_id, node_kind,
            canonical_id, canonical_revision, home_bundle_id, title, status, freshness,
            content_hash, metadata)
           SELECT $1, $2, x.stable_node_id, x.space_id, x.vault_id, x.node_kind,
                  x.canonical_id, x.canonical_revision, x.home_bundle_id, x.title,
                  x.status, x.freshness, x.content_hash, x.metadata
           FROM jsonb_to_recordset($3) AS x(
             stable_node_id text, space_id uuid, vault_id uuid, node_kind text,
             canonical_id uuid, canonical_revision bigint, home_bundle_id text,
             title text, status text, freshness text, content_hash text, metadata jsonb)
           {conflict}"#
    ))
    .bind(generation_id)
    .bind(tenant_id)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_edges(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    generation_id: Uuid,
    edges: &[GraphEdgeInput],
) -> Result<(), KnowledgeSpaceError> {
    write_edges(tx, tenant_id, generation_id, edges, false).await
}

async fn upsert_edges(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    generation_id: Uuid,
    edges: &[GraphEdgeInput],
) -> Result<(), KnowledgeSpaceError> {
    write_edges(tx, tenant_id, generation_id, edges, true).await
}

async fn write_edges(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    generation_id: Uuid,
    edges: &[GraphEdgeInput],
    upsert: bool,
) -> Result<(), KnowledgeSpaceError> {
    if edges.is_empty() {
        return Ok(());
    }
    let payload = serde_json::to_value(edges).map_err(json_error)?;
    let conflict = if upsert {
        r#"ON CONFLICT (tenant_id, generation_id, stable_edge_id) DO UPDATE SET
           from_node_id = EXCLUDED.from_node_id, to_node_id = EXCLUDED.to_node_id,
           target_ref = EXCLUDED.target_ref, relation_kind = EXCLUDED.relation_kind,
           source_space_id = EXCLUDED.source_space_id, target_space_id = EXCLUDED.target_space_id,
           home_bundle_id = EXCLUDED.home_bundle_id, producer_kind = EXCLUDED.producer_kind,
           producer_revision = EXCLUDED.producer_revision, status = EXCLUDED.status,
           evidence = EXCLUDED.evidence"#
    } else {
        ""
    };
    sqlx::query(&format!(
        r#"INSERT INTO knowledge_graph_edges
           (generation_id, tenant_id, stable_edge_id, from_node_id, to_node_id,
            target_ref, relation_kind, source_space_id, target_space_id, home_bundle_id,
            producer_kind, producer_revision, status, evidence)
           SELECT $1, $2, x.stable_edge_id, x.from_node_id, x.to_node_id,
                  x.target_ref, x.relation_kind, x.source_space_id, x.target_space_id,
                  x.home_bundle_id, x.producer_kind, x.producer_revision, x.status, x.evidence
           FROM jsonb_to_recordset($3) AS x(
             stable_edge_id text, from_node_id text, to_node_id text, target_ref text,
             relation_kind text, source_space_id uuid, target_space_id uuid,
             home_bundle_id text, producer_kind text, producer_revision bigint,
             status text, evidence jsonb)
           {conflict}"#
    ))
    .bind(generation_id)
    .bind(tenant_id)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn fetch_adjacent_edges(
    scope: GraphReadScope<'_>,
    frontier: &[String],
    direction: GraphDirection,
    relation_kinds: &[String],
    limit: usize,
) -> Result<Vec<GraphEdgeRow>, KnowledgeSpaceError> {
    if frontier.is_empty() {
        return Ok(Vec::new());
    }
    let direction_sql = match direction {
        GraphDirection::Outgoing => "e.from_node_id = ANY($4)",
        GraphDirection::Incoming => "e.to_node_id = ANY($4)",
        GraphDirection::Both => "(e.from_node_id = ANY($4) OR e.to_node_id = ANY($4))",
    };
    let relation_sql = if relation_kinds.is_empty() {
        "TRUE"
    } else {
        "e.relation_kind = ANY($5)"
    };
    Ok(sqlx::query_as::<_, GraphEdgeRow>(&format!(
        r#"SELECT {} FROM knowledge_graph_edges e
           WHERE e.tenant_id = $1 AND e.generation_id = $2
             AND e.source_space_id = ANY($3)
             AND (e.target_space_id IS NULL OR e.target_space_id = ANY($3))
             AND {direction_sql} AND {relation_sql}
           ORDER BY e.stable_edge_id LIMIT $6"#,
        edge_columns("e")
    ))
    .bind(scope.tenant_id)
    .bind(scope.generation_id)
    .bind(scope.visible_spaces)
    .bind(frontier)
    .bind(relation_kinds)
    .bind(limit as i64)
    .fetch_all(scope.pool)
    .await?)
}

async fn fetch_nodes(
    scope: GraphReadScope<'_>,
    ids: &[String],
) -> Result<Vec<GraphNodeRow>, KnowledgeSpaceError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, GraphNodeRow>(&format!(
        "SELECT {} FROM knowledge_graph_nodes n WHERE n.tenant_id = $1 AND n.generation_id = $2 AND n.space_id = ANY($3) AND (n.node_kind <> 'reference' OR (n.metadata->>'source_space_id')::UUID = ANY($3)) AND n.stable_node_id = ANY($4) AND n.status <> 'tombstone' ORDER BY n.stable_node_id",
        node_columns("n")
    ))
    .bind(scope.tenant_id)
    .bind(scope.generation_id)
    .bind(scope.visible_spaces)
    .bind(ids)
    .fetch_all(scope.pool)
    .await?)
}

async fn diagnostic_edges(
    pool: &PgPool,
    tenant_id: Uuid,
    generation_id: Uuid,
    visible_spaces: &[Uuid],
    predicate: &str,
    limit: usize,
) -> Result<Vec<GraphEdgeRow>, KnowledgeSpaceError> {
    Ok(sqlx::query_as::<_, GraphEdgeRow>(&format!(
        "SELECT {} FROM knowledge_graph_edges e WHERE e.tenant_id = $1 AND e.generation_id = $2 AND e.source_space_id = ANY($3) AND (e.target_space_id IS NULL OR e.target_space_id = ANY($3)) AND {predicate} ORDER BY e.stable_edge_id LIMIT $4",
        edge_columns("e")
    ))
    .bind(tenant_id)
    .bind(generation_id)
    .bind(visible_spaces)
    .bind(limit as i64 + 1)
    .fetch_all(pool)
    .await?)
}

async fn lock_tenant(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 2303))")
        .bind(tenant_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_tenant_user(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), KnowledgeSpaceError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE tenant_id = $1 AND id = $2)")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(KnowledgeSpaceError::NotFound)
    }
}

async fn active_generation_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<Option<GraphGenerationRow>, sqlx::Error> {
    sqlx::query_as::<_, GraphGenerationRow>(
        r#"SELECT id, tenant_id, schema_version, state, input_digest, graph_revision,
                  node_count, edge_count, built_by, created_at, activated_at, superseded_at
           FROM knowledge_graph_generations
           WHERE tenant_id = $1 AND state = 'active'"#,
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
}

fn generation_columns() -> &'static str {
    "id, tenant_id, schema_version, state, input_digest, graph_revision, node_count, edge_count, built_by, created_at, activated_at, superseded_at"
}

fn node_columns(prefix: &str) -> String {
    prefixed(
        prefix,
        "generation_id, tenant_id, stable_node_id, space_id, vault_id, node_kind, canonical_id, canonical_revision, home_bundle_id, title, status, freshness, content_hash, metadata",
    )
}

fn edge_columns(prefix: &str) -> String {
    prefixed(
        prefix,
        "generation_id, tenant_id, stable_edge_id, from_node_id, to_node_id, target_ref, relation_kind, source_space_id, target_space_id, home_bundle_id, producer_kind, producer_revision, status, evidence",
    )
}

fn prefixed(prefix: &str, columns: &str) -> String {
    columns
        .split(", ")
        .map(|column| {
            if prefix.is_empty() {
                column.to_string()
            } else {
                format!("{prefix}.{column}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_node(node: &GraphNodeInput) -> Result<(), KnowledgeSpaceError> {
    if node.stable_node_id.len() < 3
        || node.stable_node_id.len() > 512
        || node.title.chars().count() > 512
        || !matches!(
            node.status.as_str(),
            "active" | "owner_unavailable" | "tombstone"
        )
        || !matches!(node.freshness.as_str(), "current" | "stale")
        || !node.metadata.is_object()
    {
        return Err(KnowledgeSpaceError::InvalidInput(format!(
            "invalid graph node {:?}",
            node.stable_node_id
        )));
    }
    Ok(())
}

fn validate_edge(edge: &GraphEdgeInput) -> Result<(), KnowledgeSpaceError> {
    let valid_hash = edge.stable_edge_id.len() == 64
        && edge
            .stable_edge_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if !valid_hash
        || edge.target_ref.is_empty()
        || edge.target_ref.len() > 1024
        || !matches!(
            edge.status.as_str(),
            "active" | "broken" | "stale" | "owner_unavailable"
        )
        || !matches!(
            edge.producer_kind.as_str(),
            "system" | "import" | "user" | "agent" | "bundle"
        )
        || !edge.evidence.is_object()
    {
        return Err(KnowledgeSpaceError::InvalidInput(format!(
            "invalid graph edge {:?}",
            edge.stable_edge_id
        )));
    }
    Ok(())
}

fn validate_neighborhood(query: &NeighborhoodQuery) -> Result<(), KnowledgeSpaceError> {
    if query.max_depth == 0
        || query.max_depth > 3
        || !(1..=MAX_QUERY_NODES).contains(&query.node_limit)
        || !(1..=MAX_QUERY_EDGES).contains(&query.edge_limit)
    {
        return Err(KnowledgeSpaceError::InvalidInput(
            "graph neighborhood requires depth 1..=3, nodes 1..=200, edges 1..=500".to_string(),
        ));
    }
    Ok(())
}

fn json_error(error: serde_json::Error) -> KnowledgeSpaceError {
    KnowledgeSpaceError::InvalidInput(format!("graph JSON failed: {error}"))
}
