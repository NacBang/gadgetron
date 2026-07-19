//! R2.3 canonical-input to derived Knowledge Graph orchestration and HTTP API.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gadgetron_core::context::TenantContext;
use gadgetron_knowledge::graph::{
    normalized_title, parse_graph_note, stable_edge_id, NoteRelationSpec,
};
use gadgetron_xaas::knowledge_spaces::{self as spaces, SpaceActor};
use gadgetron_xaas::{
    knowledge_evolution::{KnowledgeEvolutionCandidate, KnowledgeFreshnessStatus},
    knowledge_graph::{
        self as graph, GraphDirection, GraphEdgeInput, GraphNodeInput, GraphSnapshotInput,
        NeighborhoodQuery, PathQuery, ReconcileMode, MAX_QUERY_NODES,
    },
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::AppState;

use super::workbench::WorkbenchHttpError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/knowledge/graph/generation", get(generation_handler))
        .route("/knowledge/graph/rebuild", post(rebuild_handler))
        .route("/knowledge/graph/nodes/{node_id}", get(node_handler))
        .route("/knowledge/graph/neighborhood", post(neighborhood_handler))
        .route("/knowledge/graph/backlinks", post(backlinks_handler))
        .route("/knowledge/graph/path", post(path_handler))
        .route("/knowledge/graph/diagnostics", post(diagnostics_handler))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebuildMode {
    Full,
    Incremental,
}

#[derive(Debug, Deserialize)]
pub struct RebuildRequest {
    pub mode: RebuildMode,
}

#[derive(Debug, Deserialize)]
pub struct GraphScopeRequest {
    #[serde(default)]
    pub space_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct NeighborhoodRequest {
    pub center_node_id: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    #[serde(default = "default_node_limit")]
    pub node_limit: usize,
    #[serde(default = "default_edge_limit")]
    pub edge_limit: usize,
    #[serde(default)]
    pub direction: DirectionRequest,
    #[serde(default)]
    pub relation_kinds: Vec<String>,
    #[serde(default)]
    pub space_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectionRequest {
    Outgoing,
    Incoming,
    #[default]
    Both,
}

#[derive(Debug, Deserialize)]
pub struct BacklinksRequest {
    pub node_id: String,
    #[serde(default = "default_edge_limit")]
    pub limit: usize,
    #[serde(default)]
    pub relation_kinds: Vec<String>,
    #[serde(default)]
    pub space_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct GraphPathRequest {
    pub from_node_id: String,
    pub to_node_id: String,
    #[serde(default = "default_path_depth")]
    pub max_depth: u8,
    #[serde(default = "default_path_count")]
    pub max_paths: usize,
    #[serde(default)]
    pub relation_kinds: Vec<String>,
    #[serde(default)]
    pub space_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct DiagnosticsRequest {
    #[serde(default = "default_diagnostics_limit")]
    pub limit: usize,
    #[serde(default)]
    pub space_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct RebuildResponse {
    pub result: graph::GraphMaterializeResult,
    pub input_nodes: usize,
    pub input_edges: usize,
}

#[derive(sqlx::FromRow)]
struct CandidateGraphInputRow {
    id: Uuid,
    job_id: Uuid,
    space_id: Uuid,
    vault_id: Uuid,
    home_bundle_id: String,
    owner_state: String,
    title: String,
    summary: String,
    payload: serde_json::Value,
    citations: serde_json::Value,
    content_hash: String,
    change_set_id: Option<Uuid>,
    review_status: Option<String>,
    materialized_object_id: Option<Uuid>,
}

pub async fn generation_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<graph::GraphGenerationRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let visible = visible_spaces(pool(&state)?, actor, &[]).await?;
    if visible.is_empty() {
        return Err(WorkbenchHttpError::KnowledgeNotFound);
    }
    Ok(Json(
        graph::active_generation(pool(&state)?, actor.tenant_id).await?,
    ))
}

pub async fn rebuild_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<RebuildRequest>,
) -> Result<Json<RebuildResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    graph::require_tenant_admin(pool(&state)?, actor.tenant_id, actor.user_id).await?;
    let snapshot = build_snapshot(&state, actor.tenant_id).await?;
    let input_nodes = snapshot.nodes.len();
    let input_edges = snapshot.edges.len();
    let mode = match request.mode {
        RebuildMode::Full => ReconcileMode::Full,
        RebuildMode::Incremental => ReconcileMode::Incremental,
    };
    let result = graph::materialize(
        pool(&state)?,
        actor.tenant_id,
        actor.user_id,
        mode,
        snapshot,
    )
    .await?;
    Ok(Json(RebuildResponse {
        result,
        input_nodes,
        input_edges,
    }))
}

pub async fn node_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(node_id): Path<String>,
) -> Result<Json<graph::GraphNodeRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let visible = visible_spaces(pool(&state)?, actor, &[]).await?;
    Ok(Json(
        graph::get_node(pool(&state)?, actor.tenant_id, &visible, &node_id).await?,
    ))
}

pub async fn neighborhood_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<NeighborhoodRequest>,
) -> Result<Json<graph::GraphNeighborhood>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let visible = visible_spaces(pool(&state)?, actor, &request.space_ids).await?;
    Ok(Json(
        graph::neighborhood(
            pool(&state)?,
            actor.tenant_id,
            &visible,
            NeighborhoodQuery {
                center_node_id: request.center_node_id,
                max_depth: request.depth,
                node_limit: request.node_limit,
                edge_limit: request.edge_limit,
                direction: direction(request.direction),
                relation_kinds: request.relation_kinds,
            },
        )
        .await?,
    ))
}

pub async fn backlinks_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<BacklinksRequest>,
) -> Result<Json<graph::GraphNeighborhood>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let visible = visible_spaces(pool(&state)?, actor, &request.space_ids).await?;
    Ok(Json(
        graph::neighborhood(
            pool(&state)?,
            actor.tenant_id,
            &visible,
            NeighborhoodQuery {
                center_node_id: request.node_id,
                max_depth: 1,
                node_limit: request.limit.saturating_add(1).min(MAX_QUERY_NODES),
                edge_limit: request.limit,
                direction: GraphDirection::Incoming,
                relation_kinds: request.relation_kinds,
            },
        )
        .await?,
    ))
}

pub async fn path_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<GraphPathRequest>,
) -> Result<Json<graph::GraphPathResult>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let visible = visible_spaces(pool(&state)?, actor, &request.space_ids).await?;
    Ok(Json(
        graph::paths(
            pool(&state)?,
            actor.tenant_id,
            &visible,
            PathQuery {
                from_node_id: request.from_node_id,
                to_node_id: request.to_node_id,
                max_depth: request.max_depth,
                max_paths: request.max_paths,
                relation_kinds: request.relation_kinds,
            },
        )
        .await?,
    ))
}

pub async fn diagnostics_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<DiagnosticsRequest>,
) -> Result<Json<graph::GraphDiagnostics>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let visible = visible_spaces(pool(&state)?, actor, &request.space_ids).await?;
    Ok(Json(
        graph::diagnostics(pool(&state)?, actor.tenant_id, &visible, request.limit).await?,
    ))
}

pub(crate) async fn build_snapshot(
    state: &AppState,
    tenant_id: Uuid,
) -> Result<GraphSnapshotInput, WorkbenchHttpError> {
    let (sources, objects, shares) = graph::canonical_inputs(pool(state)?, tenant_id).await?;
    let repository = vault_repository(state, tenant_id)?;
    let lock = repository
        .acquire_lock(Duration::from_secs(5))
        .map_err(vault_error)?;
    let mut nodes = Vec::new();
    let mut parsed_notes = Vec::new();
    for source in &sources {
        if source.status == "deleted" {
            continue;
        }
        nodes.push(GraphNodeInput {
            stable_node_id: format!("source:{}", source.id),
            space_id: source.space_id,
            vault_id: Some(source.vault_id),
            node_kind: "source".to_string(),
            canonical_id: Some(source.id),
            canonical_revision: source.revision,
            home_bundle_id: source.home_bundle_id.clone(),
            title: source.title.clone(),
            status: owner_status(&source.owner_state).to_string(),
            freshness: "current".to_string(),
            content_hash: source.content_hash.clone(),
            metadata: serde_json::json!({
                "content_type": source.content_type,
                "final_uri": source.final_uri,
                "requested_uri": source.requested_uri,
                "source_kind": source.source_kind,
                "source_status": source.status,
            }),
        });
    }
    for object in &objects {
        if object.status != "active" {
            continue;
        }
        let note = repository
            .read_note_reconciled_locked(
                &lock,
                object.space_id,
                &object.home_bundle_id,
                &object.path,
                object.content_hash.as_deref(),
            )
            .map_err(vault_error)?;
        let mut revision = object.revision;
        let mut content_hash = note.content_hash.clone();
        if note.externally_changed {
            let updated = gadgetron_xaas::knowledge_sources::update_note_hash_system(
                pool(state)?,
                tenant_id,
                object.id,
                object.revision,
                &note.content_hash,
            )
            .await?;
            revision = updated.revision;
            content_hash = updated.content_hash.unwrap_or(note.content_hash);
        }
        let raw =
            String::from_utf8(note.bytes).map_err(|_| invalid("graph note is not valid UTF-8"))?;
        let parsed = parse_graph_note(&raw, &object.path)
            .map_err(|error| invalid(format!("graph note parse failed: {error}")))?;
        let stable_node_id = format!("note:{}", object.id);
        let node_kind = match parsed
            .properties
            .get("knowledge_kind")
            .and_then(serde_json::Value::as_str)
        {
            Some("lesson") => "lesson",
            Some("insight") => "insight",
            _ => "note",
        };
        let freshness = match parsed
            .properties
            .get("freshness")
            .and_then(serde_json::Value::as_str)
        {
            Some("time_sensitive" | "unknown") => "stale",
            _ => "current",
        };
        nodes.push(GraphNodeInput {
            stable_node_id: stable_node_id.clone(),
            space_id: object.space_id,
            vault_id: Some(object.vault_id),
            node_kind: node_kind.to_string(),
            canonical_id: Some(object.id),
            canonical_revision: revision,
            home_bundle_id: object.home_bundle_id.clone(),
            title: parsed.title.clone(),
            status: owner_status(&object.owner_state).to_string(),
            freshness: freshness.to_string(),
            content_hash: Some(content_hash),
            metadata: serde_json::json!({
                "path": object.path,
                "source_id": object.source_id,
                "knowledge_kind": parsed.properties.get("knowledge_kind"),
                "review_state": parsed.properties.get("review_state"),
                "evolution_candidate_id": parsed.properties.get("evolution_candidate_id"),
                "applicability": parsed.properties.get("applicability"),
                "limitations": parsed.properties.get("limitations"),
                "confidence": parsed.properties.get("confidence"),
                "importance": parsed.properties.get("importance"),
            }),
        });
        let mut relations = parsed.relations;
        if let Some(source_id) = object.source_id {
            for relation_kind in ["cites", "derived_from"] {
                relations.push(NoteRelationSpec {
                    relation_kind: relation_kind.to_string(),
                    target_ref: source_id.to_string(),
                    evidence_kind: "source_registry".to_string(),
                    locator: None,
                });
            }
        }
        parsed_notes.push((
            stable_node_id,
            object.space_id,
            object.home_bundle_id.clone(),
            revision,
            relations,
        ));
    }
    let candidates = sqlx::query_as::<_, CandidateGraphInputRow>(
        r#"SELECT artifact.id, artifact.job_id, artifact.space_id,
                  job.output_vault_id AS vault_id, vault.home_bundle_id, vault.owner_state,
                  artifact.title, artifact.summary, artifact.payload, artifact.citations,
                  artifact.content_hash, change_set.id AS change_set_id,
                  change_set.status AS review_status, change_set.materialized_object_id
           FROM knowledge_job_artifacts artifact
           JOIN knowledge_jobs job
             ON job.tenant_id = artifact.tenant_id AND job.id = artifact.job_id
           JOIN knowledge_vaults vault
             ON vault.tenant_id = artifact.tenant_id AND vault.id = job.output_vault_id
           LEFT JOIN knowledge_change_sets change_set
             ON change_set.tenant_id = artifact.tenant_id
            AND change_set.candidate_artifact_id = artifact.id
           WHERE artifact.tenant_id = $1 AND artifact.kind = 'candidate'"#,
    )
    .bind(tenant_id)
    .fetch_all(pool(state)?)
    .await
    .map_err(spaces::KnowledgeSpaceError::from)?;
    for candidate_row in candidates {
        let structured = KnowledgeEvolutionCandidate::parse_and_validate(
            candidate_row.payload.clone(),
            &candidate_row.citations,
        )
        .ok();
        let freshness = structured.as_ref().map_or("current", |candidate| {
            if candidate.freshness.status == KnowledgeFreshnessStatus::Current {
                "current"
            } else {
                "stale"
            }
        });
        let readiness = structured
            .as_ref()
            .map(KnowledgeEvolutionCandidate::readiness);
        nodes.push(GraphNodeInput {
            stable_node_id: format!("candidate:{}", candidate_row.id),
            space_id: candidate_row.space_id,
            vault_id: Some(candidate_row.vault_id),
            node_kind: "candidate".to_string(),
            canonical_id: Some(candidate_row.id),
            canonical_revision: 1,
            home_bundle_id: candidate_row.home_bundle_id.clone(),
            title: candidate_row.title.clone(),
            status: owner_status(&candidate_row.owner_state).to_string(),
            freshness: freshness.to_string(),
            content_hash: Some(candidate_row.content_hash.clone()),
            metadata: serde_json::json!({
                "job_id": candidate_row.job_id,
                "summary": candidate_row.summary,
                "target_kind": structured.as_ref().map(|candidate| candidate.target_kind.as_str()),
                "claim": structured.as_ref().map(|candidate| candidate.claim.as_str()),
                "applicability": structured.as_ref().map(|candidate| &candidate.applicability),
                "limitations": structured.as_ref().map(|candidate| &candidate.limitations),
                "confidence": structured.as_ref().map(|candidate| candidate.confidence),
                "importance": structured.as_ref().map(KnowledgeEvolutionCandidate::importance_score),
                "readiness": readiness,
                "change_set_id": candidate_row.change_set_id,
                "review_status": candidate_row.review_status,
                "materialized_object_id": candidate_row.materialized_object_id,
                "legacy": structured.is_none(),
            }),
        });
        for citation in candidate_row.citations.as_array().into_iter().flatten() {
            let Some(source_id) = citation
                .get("source_id")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            for relation_kind in ["cites", "derived_from"] {
                parsed_notes.push((
                    format!("candidate:{}", candidate_row.id),
                    candidate_row.space_id,
                    candidate_row.home_bundle_id.clone(),
                    1,
                    vec![NoteRelationSpec {
                        relation_kind: relation_kind.to_string(),
                        target_ref: source_id.to_string(),
                        evidence_kind: "candidate_citation".to_string(),
                        locator: citation
                            .get("locator")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    }],
                ));
            }
        }
    }
    let by_id: HashMap<_, _> = nodes
        .iter()
        .map(|node| (node.stable_node_id.clone(), node.clone()))
        .collect();
    let mut by_reference: HashMap<(Uuid, String), Vec<String>> = HashMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.node_kind.as_str(), "note" | "lesson" | "insight"))
    {
        index_reference(
            &mut by_reference,
            node.space_id,
            &node.title,
            &node.stable_node_id,
        );
        if let Some(path) = node
            .metadata
            .get("path")
            .and_then(serde_json::Value::as_str)
        {
            index_reference(&mut by_reference, node.space_id, path, &node.stable_node_id);
            if let Some(without_extension) = path.strip_suffix(".md") {
                index_reference(
                    &mut by_reference,
                    node.space_id,
                    without_extension,
                    &node.stable_node_id,
                );
                if let Some(file_stem) = without_extension.rsplit('/').next() {
                    index_reference(
                        &mut by_reference,
                        node.space_id,
                        file_stem,
                        &node.stable_node_id,
                    );
                }
            }
        }
    }
    let mut edges = BTreeMap::new();
    let mut stale_notes = HashSet::new();
    for (from, source_space_id, home_bundle_id, revision, relations) in parsed_notes {
        for relation in relations {
            let target = resolve_target(&relation, source_space_id, &by_id, &by_reference);
            let to_node_id = target.as_ref().map(|node| node.stable_node_id.clone());
            let target_space_id = target.as_ref().map(|node| node.space_id);
            let source_unavailable = by_id
                .get(&from)
                .is_some_and(|node| node.status == "owner_unavailable");
            let status = if target.is_none() {
                stale_notes.insert(from.clone());
                "broken"
            } else if source_unavailable
                || target
                    .as_ref()
                    .is_some_and(|node| node.status == "owner_unavailable")
            {
                "owner_unavailable"
            } else {
                "active"
            };
            let edge_id = stable_edge_id(
                &from,
                &relation.relation_kind,
                &relation.target_ref,
                revision,
                &relation.evidence_kind,
                relation.locator.as_deref(),
            );
            edges.entry(edge_id.clone()).or_insert(GraphEdgeInput {
                stable_edge_id: edge_id,
                from_node_id: from.clone(),
                to_node_id,
                target_ref: relation.target_ref,
                relation_kind: relation.relation_kind,
                source_space_id,
                target_space_id,
                home_bundle_id: home_bundle_id.clone(),
                producer_kind: "system".to_string(),
                producer_revision: revision,
                status: status.to_string(),
                evidence: serde_json::json!({
                    "kind": relation.evidence_kind,
                    "locator": relation.locator,
                }),
            });
        }
    }
    for share in shares.into_iter().filter(|share| share.mode == "reference") {
        let source_id = format!("note:{}", share.source_object_id);
        let Some(source) = by_id.get(&source_id) else {
            continue;
        };
        let reference_id = format!("share:{}", share.id);
        nodes.push(GraphNodeInput {
            stable_node_id: reference_id.clone(),
            space_id: share.target_space_id,
            vault_id: None,
            node_kind: "reference".to_string(),
            canonical_id: None,
            canonical_revision: share.revision,
            home_bundle_id: share.home_bundle_id.clone(),
            title: source.title.clone(),
            status: source.status.clone(),
            freshness: source.freshness.clone(),
            content_hash: source.content_hash.clone(),
            metadata: serde_json::json!({
                "mode": share.mode,
                "source_space_id": share.source_space_id,
                "source_object_id": share.source_object_id,
                "source_revision": share.source_revision,
                "follow_latest": share.follow_latest,
                "policy_disposition": share.policy_disposition,
            }),
        });
        let edge_id = stable_edge_id(
            &reference_id,
            "bridge_to",
            &source_id,
            share.revision,
            "knowledge_share",
            None,
        );
        edges.insert(
            edge_id.clone(),
            GraphEdgeInput {
                stable_edge_id: edge_id,
                from_node_id: reference_id,
                to_node_id: Some(source_id.clone()),
                target_ref: source_id,
                relation_kind: "bridge_to".to_string(),
                source_space_id: share.target_space_id,
                target_space_id: Some(share.source_space_id),
                home_bundle_id: share.home_bundle_id,
                producer_kind: "system".to_string(),
                producer_revision: share.revision,
                status: source.status.clone(),
                evidence: serde_json::json!({
                    "kind": "knowledge_share",
                    "share_id": share.id,
                    "source_revision": share.source_revision,
                }),
            },
        );
    }
    for node in &mut nodes {
        if stale_notes.contains(&node.stable_node_id) {
            node.freshness = "stale".to_string();
        }
    }
    Ok(GraphSnapshotInput {
        nodes,
        edges: edges.into_values().collect(),
    })
}

/// Derived graph refresh after a canonical Source/note mutation. Canonical
/// success is never rolled back when this recoverable projection fails; the
/// admin full rebuild path is the repair authority.
pub(crate) async fn reconcile_after_change(state: &AppState, actor: SpaceActor) {
    let result = async {
        let snapshot = build_snapshot(state, actor.tenant_id).await?;
        graph::materialize(
            pool(state)?,
            actor.tenant_id,
            actor.user_id,
            ReconcileMode::Incremental,
            snapshot,
        )
        .await
        .map_err(WorkbenchHttpError::from)
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(
            target: "knowledge_graph",
            tenant_id = %actor.tenant_id,
            error = ?error,
            "canonical mutation succeeded but graph reconcile failed; full rebuild remains available"
        );
    }
}

fn resolve_target<'a>(
    relation: &NoteRelationSpec,
    source_space_id: Uuid,
    by_id: &'a HashMap<String, GraphNodeInput>,
    by_reference: &'a HashMap<(Uuid, String), Vec<String>>,
) -> Option<&'a GraphNodeInput> {
    if let Some(target) = by_id.get(&relation.target_ref) {
        return Some(target);
    }
    if let Ok(id) = Uuid::parse_str(&relation.target_ref) {
        if relation.relation_kind == "derived_from" {
            if let Some(target) = by_id.get(&format!("candidate:{id}")) {
                return Some(target);
            }
        }
        if relation.relation_kind == "cites" {
            if let Some(target) = by_id.get(&format!("source:{id}")) {
                return Some(target);
            }
        }
        for candidate in [
            format!("note:{id}"),
            format!("candidate:{id}"),
            format!("source:{id}"),
        ] {
            if let Some(target) = by_id.get(&candidate) {
                return Some(target);
            }
        }
    }
    let key = (source_space_id, normalized_title(&relation.target_ref));
    let candidates = by_reference.get(&key)?;
    (candidates.len() == 1)
        .then(|| by_id.get(&candidates[0]))
        .flatten()
}

fn index_reference(
    index: &mut HashMap<(Uuid, String), Vec<String>>,
    space_id: Uuid,
    reference: &str,
    node_id: &str,
) {
    let key = (space_id, normalized_title(reference));
    if key.1.is_empty() {
        return;
    }
    let matches = index.entry(key).or_default();
    if !matches.iter().any(|candidate| candidate == node_id) {
        matches.push(node_id.to_string());
    }
}

async fn visible_spaces(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    requested: &[Uuid],
) -> Result<Vec<Uuid>, WorkbenchHttpError> {
    let effective = spaces::effective_spaces(pool, actor).await?;
    let visible: HashSet<_> = effective.into_iter().map(|space| space.space.id).collect();
    if requested.is_empty() {
        let mut ids: Vec<_> = visible.into_iter().collect();
        ids.sort();
        return Ok(ids);
    }
    if requested.iter().any(|space_id| !visible.contains(space_id)) {
        return Err(WorkbenchHttpError::KnowledgeNotFound);
    }
    let mut ids = requested.to_vec();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(
            "Knowledge Graph requires PostgreSQL".to_string(),
        ))
    })
}

fn vault_repository(
    state: &AppState,
    tenant_id: Uuid,
) -> Result<gadgetron_knowledge::vault::TenantVaultRepository, WorkbenchHttpError> {
    state
        .workbench
        .as_deref()
        .and_then(|workbench| workbench.vault_layout.as_ref())
        .ok_or_else(|| invalid("Knowledge Graph requires [knowledge].vault_path"))?
        .open_or_init(tenant_id)
        .map_err(vault_error)
}

fn actor(ctx: &TenantContext) -> Result<SpaceActor, WorkbenchHttpError> {
    Ok(SpaceActor {
        tenant_id: ctx.tenant_id,
        user_id: ctx
            .actor_user_id
            .ok_or(WorkbenchHttpError::KnowledgeForbidden)?,
    })
}

fn direction(direction: DirectionRequest) -> GraphDirection {
    match direction {
        DirectionRequest::Outgoing => GraphDirection::Outgoing,
        DirectionRequest::Incoming => GraphDirection::Incoming,
        DirectionRequest::Both => GraphDirection::Both,
    }
}

fn owner_status(owner_state: &str) -> &'static str {
    if owner_state == "enabled" {
        "active"
    } else {
        "owner_unavailable"
    }
}

fn invalid(detail: impl Into<String>) -> WorkbenchHttpError {
    WorkbenchHttpError::KnowledgeInvalidInput {
        detail: detail.into(),
    }
}

fn vault_error(error: gadgetron_knowledge::vault::VaultLayoutError) -> WorkbenchHttpError {
    invalid(format!("Domain Vault graph operation failed: {error}"))
}

const fn default_depth() -> u8 {
    2
}
const fn default_path_depth() -> u8 {
    4
}
const fn default_node_limit() -> usize {
    100
}
const fn default_edge_limit() -> usize {
    200
}
const fn default_path_count() -> usize {
    3
}
const fn default_diagnostics_limit() -> usize {
    50
}
