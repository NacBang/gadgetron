//! R2.5 Knowledge job and change-set HTTP surface.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Extension, Json, Router,
};
use gadgetron_core::{
    agent::config::ConversationAgentProfile, context::TenantContext, error::GadgetronError,
};
use gadgetron_knowledge::vault::note_relative_path;
use gadgetron_xaas::{
    knowledge_agent_profiles, knowledge_collections as collections,
    knowledge_evolution::{KnowledgeEvolutionCandidate, KnowledgeEvolutionReadiness},
    knowledge_jobs::{
        self as jobs, BundleRoleSnapshot, EnqueueKnowledgeJob, JobBudget, KnowledgeJobError,
        KnowledgeJobKind, KnowledgeJobRole, RuntimeSnapshot, VerifiedOutcomeSnapshot,
    },
    knowledge_sources as sources,
    knowledge_spaces::{self as spaces, SpaceActor, SpaceRole},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    knowledge_jobs::{
        BundleExecutionSnapshot, LessonRevisionTarget, LESSON_REVISION_TARGET_INPUT_KEY,
    },
    server::AppState,
    web::bundle_runtime::BundleKnowledgeRoleExecutionContract,
};

use super::workbench::WorkbenchHttpError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/knowledge/spaces/{space_id}/jobs",
            get(list_jobs_handler).post(enqueue_job_handler),
        )
        .route("/knowledge/jobs/{job_id}", get(get_job_handler))
        .route(
            "/knowledge/bundles/{bundle_id}/agent-roles",
            get(list_bundle_agent_roles_handler),
        )
        .route("/knowledge/jobs/{job_id}/cancel", post(cancel_job_handler))
        .route("/knowledge/jobs/{job_id}/retry", post(retry_job_handler))
        .route(
            "/knowledge/spaces/{space_id}/change-sets",
            get(list_change_sets_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/duplicate-groups",
            get(list_duplicate_groups_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/merge-change-sets",
            post(create_merge_change_set_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/evolution",
            get(list_evolution_handler),
        )
        .route(
            "/knowledge/change-sets/{change_set_id}",
            get(get_change_set_handler).put(edit_change_set_handler),
        )
        .route(
            "/knowledge/change-sets/{change_set_id}/accept",
            post(accept_change_set_handler),
        )
        .route(
            "/knowledge/change-sets/{change_set_id}/reject",
            post(reject_change_set_handler),
        )
        .route(
            "/knowledge/change-sets/{change_set_id}/retry-apply",
            post(retry_change_set_apply_handler),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartRole {
    SourceScout,
    Researcher,
    InsightSynthesizer,
    Gardener,
}

impl StartRole {
    fn job_role(&self) -> KnowledgeJobRole {
        match self {
            Self::SourceScout => KnowledgeJobRole::SourceScout,
            Self::Researcher => KnowledgeJobRole::Researcher,
            Self::InsightSynthesizer => KnowledgeJobRole::InsightSynthesizer,
            Self::Gardener => KnowledgeJobRole::Gardener,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StartJobRequest {
    pub role: StartRole,
    pub output_vault_id: Uuid,
    pub question: String,
    #[serde(default)]
    pub collection_id: Option<Uuid>,
    #[serde(default)]
    pub collection_revision: Option<i64>,
    #[serde(default)]
    pub source_ids: Vec<Uuid>,
    #[serde(default)]
    pub outcome_ids: Vec<Uuid>,
    #[serde(default)]
    pub lesson_revision: Option<LessonRevisionRequest>,
    #[serde(default)]
    pub bundle_role: Option<StartBundleRole>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LessonRevisionRequest {
    pub object_id: Uuid,
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartBundleRole {
    pub bundle_id: String,
    pub role_id: String,
}

pub(crate) struct EnqueueJobOptions {
    pub kind: KnowledgeJobKind,
    pub idempotency_key: Option<String>,
    pub extra_input: BTreeMap<String, Value>,
}

impl Default for EnqueueJobOptions {
    fn default() -> Self {
        Self {
            kind: KnowledgeJobKind::OnDemand,
            idempotency_key: None,
            extra_input: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ExpectedRevision {
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize)]
pub struct ChangeSetDecisionRequest {
    pub expected_revision: i64,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

const fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize)]
pub struct JobListResponse {
    pub jobs: Vec<jobs::KnowledgeJobRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct JobDetailResponse {
    pub job: jobs::KnowledgeJobRow,
    pub sources: Vec<jobs::KnowledgeJobSourceRow>,
    pub artifacts: Vec<jobs::KnowledgeJobArtifactRow>,
}

#[derive(Debug, Serialize)]
pub struct ChangeSetListResponse {
    pub change_sets: Vec<jobs::KnowledgeChangeSetRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct DuplicateGroupListResponse {
    pub groups: Vec<DuplicateGroup>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    pub id: String,
    pub confidence: &'static str,
    pub match_reasons: Vec<&'static str>,
    pub candidates: Vec<DuplicateCandidate>,
}

#[derive(Debug, Serialize)]
pub struct DuplicateCandidate {
    pub object_id: Uuid,
    pub vault_id: Uuid,
    pub home_bundle_id: String,
    pub title: Option<String>,
    pub path: String,
    pub content_hash: Option<String>,
    pub revision: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMergeChangeSetRequest {
    pub idempotency_key: Uuid,
    pub sources: Vec<MergeSourceRequest>,
    pub master_object_id: Uuid,
    #[serde(default)]
    pub field_sources: BTreeMap<String, Uuid>,
    pub body_strategy: MergeBodyStrategy,
    #[serde(default)]
    pub incoming_object_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeSourceRequest {
    pub object_id: Uuid,
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeBodyStrategy {
    KeepCurrent,
    UseIncoming,
    KeepBoth,
}

#[derive(Debug, Serialize)]
pub struct EvolutionListResponse {
    pub traces: Vec<jobs::KnowledgeEvolutionTrace>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct BundleAgentRoleListResponse {
    pub bundle_id: String,
    pub enabled: bool,
    pub roles: Vec<BundleAgentRoleView>,
}

#[derive(Debug, Serialize)]
pub struct BundleAgentRoleView {
    pub id: String,
    pub label: String,
    pub description: String,
    pub core_role: String,
}

pub async fn list_bundle_agent_roles_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(bundle_id): Path<String>,
) -> Result<Json<BundleAgentRoleListResponse>, WorkbenchHttpError> {
    actor(&ctx)?;
    let manager = super::workbench::require_runtime_manager(&state)?;
    let status = manager.status(&bundle_id).await?;
    if status.state != super::bundle_runtime::BundleRuntimeState::Enabled {
        return Ok(Json(BundleAgentRoleListResponse {
            bundle_id,
            enabled: false,
            roles: Vec::new(),
        }));
    }
    let projection = manager.knowledge_profiles(&bundle_id)?;
    let roles = projection
        .roles
        .into_iter()
        .filter(|declaration| {
            declaration
                .job
                .triggers
                .iter()
                .any(|trigger| matches!(trigger, gadgetron_bundle_sdk::JobTrigger::OnDemand))
                && declaration.role.core_role.as_str() != "operator"
        })
        .map(|declaration| BundleAgentRoleView {
            id: declaration.role.id.as_str().to_string(),
            label: declaration.role.label,
            description: declaration.role.description,
            core_role: declaration.role.core_role.as_str().to_string(),
        })
        .collect();
    Ok(Json(BundleAgentRoleListResponse {
        bundle_id,
        enabled: true,
        roles,
    }))
}

pub async fn enqueue_job_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Json(request): Json<StartJobRequest>,
) -> Result<Json<jobs::KnowledgeJobRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    Ok(Json(
        enqueue_job(
            &state,
            actor,
            space_id,
            request,
            EnqueueJobOptions::default(),
        )
        .await?,
    ))
}

pub(crate) async fn enqueue_job(
    state: &AppState,
    actor: SpaceActor,
    space_id: Uuid,
    request: StartJobRequest,
    options: EnqueueJobOptions,
) -> Result<jobs::KnowledgeJobRow, WorkbenchHttpError> {
    let question = request.question.trim();
    if question.is_empty() || question.chars().count() > 4_000 {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Knowledge job topic must contain 1..4000 characters".to_string(),
        });
    }
    let live = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.agent_brain.as_ref())
        .map(|brain| brain.load_full())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge jobs require an active Penny runtime".to_string(),
            ))
        })?;
    let role = request.role.job_role();
    if request.source_ids.iter().collect::<HashSet<_>>().len() != request.source_ids.len()
        || request.outcome_ids.iter().collect::<HashSet<_>>().len() != request.outcome_ids.len()
    {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Knowledge job evidence contains duplicates".to_string(),
        });
    }
    if request.lesson_revision.is_some()
        && (role != KnowledgeJobRole::Researcher
            || request.outcome_ids.is_empty()
            || !request.source_ids.is_empty()
            || request.collection_id.is_some()
            || request.collection_revision.is_some())
    {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Lesson revision requires a Researcher, one or more verified Outcomes, no caller-selected Sources, and no Collection binding".to_string(),
        });
    }
    match role {
        KnowledgeJobRole::SourceScout
            if !request.source_ids.is_empty() || !request.outcome_ids.is_empty() =>
        {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail:
                    "Source Scout snapshots Space coverage automatically; do not select evidence"
                        .to_string(),
            });
        }
        KnowledgeJobRole::InsightSynthesizer
            if request.source_ids.len() < 2 || request.outcome_ids.is_empty() =>
        {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Insight synthesis requires at least two Sources and one verified Outcome"
                    .to_string(),
            });
        }
        _ => {}
    }
    let bound_collection = if let Some(collection_id) = request.collection_id {
        if role != KnowledgeJobRole::Researcher {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Only a Researcher job can be bound to a Collection".to_string(),
            });
        }
        let expected_revision = request.collection_revision.ok_or_else(|| {
            WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Collection research requires its current revision".to_string(),
            }
        })?;
        let collection = collections::get_collection(
            pool(state)?,
            actor,
            collection_id,
            SpaceRole::Contributor,
            true,
        )
        .await?;
        let (collection, _, _) = super::knowledge_collections::ensure_current_snapshot(
            state,
            pool(state)?,
            actor,
            collection,
            expected_revision,
        )
        .await?;
        let bundle_role = request.bundle_role.as_ref().ok_or_else(|| {
            WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Collection research requires a signed Bundle Researcher role".to_string(),
            }
        })?;
        if collection.space_id != space_id
            || collection.output_vault_id != request.output_vault_id
            || collection.bundle_id != bundle_role.bundle_id
        {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Collection, Space, output Vault and Bundle role must describe one research domain"
                    .to_string(),
            });
        }
        let current_sources = collections::source_health(pool(state)?, actor, collection.id)
            .await?
            .into_iter()
            .filter(|row| row.health == "current")
            .filter_map(|row| row.source_id)
            .collect::<HashSet<_>>();
        if request.source_ids.is_empty()
            || request
                .source_ids
                .iter()
                .any(|source_id| !current_sources.contains(source_id))
        {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Collection research accepts only its current collected Sources"
                    .to_string(),
            });
        }
        Some(collection)
    } else {
        if request.collection_revision.is_some() {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Collection revision requires a Collection id".to_string(),
            });
        }
        None
    };
    let core_role_id = role.as_str();
    let global_profile = ConversationAgentProfile::from_agent(&live);
    let effective = knowledge_agent_profiles::resolve_role_profile(
        pool(state)?,
        actor.tenant_id,
        &global_profile,
        core_role_id,
        request
            .bundle_role
            .as_ref()
            .map(|bundle_role| (bundle_role.bundle_id.as_str(), bundle_role.role_id.as_str())),
    )
    .await
    .map_err(role_profile_error)?;
    let mut prompt_contract_revision = match role {
        KnowledgeJobRole::SourceScout => "source-scout-v1",
        KnowledgeJobRole::Researcher => "researcher-v2",
        KnowledgeJobRole::InsightSynthesizer => "insight-synthesizer-v1",
        KnowledgeJobRole::Gardener => "gardener-v1",
    }
    .to_string();
    let mut bundle_snapshot = None;
    let mut bundle_contract = None;
    let mut max_sources = 12;
    let mut bundle_max_wall_seconds = 3_600;
    if let Some(bundle_role) = &request.bundle_role {
        let contract = super::workbench::require_runtime_manager(state)?
            .knowledge_role_execution_contract(&bundle_role.bundle_id, &bundle_role.role_id)
            .await?;
        if contract.role.core_role.as_str() != core_role_id {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail:
                    "The selected Bundle AI role does not match the requested Knowledge job role"
                        .to_string(),
            });
        }
        prompt_contract_revision = contract.role.prompt_contract_revision.clone();
        if let Some(collection) = &contract.collection {
            max_sources = i32::try_from(collection.budget.max_sources)
                .unwrap_or(100)
                .clamp(1, 100);
            bundle_max_wall_seconds = i32::try_from(collection.budget.max_wall_seconds)
                .unwrap_or(3_600)
                .clamp(5, 3_600);
        }
        if let Some(budget) = contract.job.budget {
            bundle_max_wall_seconds = bundle_max_wall_seconds.min(
                i32::try_from(budget.max_wall_seconds)
                    .unwrap_or(3_600)
                    .clamp(5, 3_600),
            );
        }
        bundle_snapshot = Some(bundle_role_snapshot(&contract));
        bundle_contract = Some(contract);
    }
    if let (Some(collection), Some(contract)) = (&bound_collection, &bundle_contract) {
        let profile_matches = contract
            .collection
            .as_ref()
            .is_some_and(|profile| profile.id.as_str() == collection.profile_id);
        if contract.package_manifest_sha256 != collection.package_manifest_sha256
            || !profile_matches
        {
            return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                detail:
                    "Collection snapshot does not match the active signed Bundle research contract"
                        .to_string(),
            });
        }
    }
    let profile = super::workbench::canonicalize_registered_endpoint_profile(
        pool(state)?,
        actor.tenant_id,
        effective.selection.clone().into_profile(),
    )
    .await
    .map_err(WorkbenchHttpError::Core)?
    .resolve_auto(question);
    let policy_revision = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.policy_evaluator.as_ref())
        .ok_or_else(|| WorkbenchHttpError::PolicyUnavailable {
            detail: "Knowledge jobs require the common policy evaluator".to_string(),
        })?
        .active_identity(actor.tenant_id)
        .await
        .map_err(|error| WorkbenchHttpError::PolicyUnavailable {
            detail: error.detail,
        })?
        .to_revision_ref();
    let max_wall_seconds = i32::try_from(live.request_timeout_secs)
        .unwrap_or(3_600)
        .clamp(5, 3_600)
        .min(bundle_max_wall_seconds);
    let runtime = runtime_snapshot(
        profile.clone(),
        prompt_contract_revision,
        &policy_revision,
        effective.source.as_str(),
        effective.profile_ref.clone(),
    );
    let bundle_execution = if let Some(contract) = bundle_contract.as_ref() {
        let followup = if let Some(followup_role) = contract.role.followup_role.as_ref() {
            let followup_role_id = followup_role.as_str();
            let followup_contract = super::workbench::require_runtime_manager(state)?
                .knowledge_role_execution_contract(&contract.bundle_id, followup_role_id)
                .await?;
            if followup_contract.role.core_role.as_str() != KnowledgeJobRole::Gardener.as_str() {
                return Err(WorkbenchHttpError::KnowledgeInvalidInput {
                    detail: "A Bundle Knowledge follow-up role must be a Gardener/Distiller"
                        .to_string(),
                });
            }
            let followup_effective = knowledge_agent_profiles::resolve_role_profile(
                pool(state)?,
                actor.tenant_id,
                &global_profile,
                KnowledgeJobRole::Gardener.as_str(),
                Some((&contract.bundle_id, followup_role_id)),
            )
            .await
            .map_err(role_profile_error)?;
            let followup_profile = super::workbench::canonicalize_registered_endpoint_profile(
                pool(state)?,
                actor.tenant_id,
                followup_effective.selection.clone().into_profile(),
            )
            .await
            .map_err(WorkbenchHttpError::Core)?
            .resolve_auto(question);
            let followup_wall_seconds = contract_max_wall_seconds(
                &followup_contract,
                i32::try_from(live.request_timeout_secs)
                    .unwrap_or(3_600)
                    .clamp(5, 3_600),
            );
            let followup_runtime = runtime_snapshot(
                followup_profile,
                followup_contract.role.prompt_contract_revision.clone(),
                &policy_revision,
                followup_effective.source.as_str(),
                followup_effective.profile_ref,
            );
            Some(Box::new(bundle_execution_snapshot(
                &followup_contract,
                followup_runtime,
                followup_wall_seconds,
                None,
            )))
        } else {
            None
        };
        Some(bundle_execution_snapshot(
            contract,
            runtime.clone(),
            max_wall_seconds,
            followup,
        ))
    } else {
        None
    };
    let outcome_snapshot = if request.outcome_ids.is_empty() {
        Vec::new()
    } else {
        verified_outcome_snapshot(pool(state)?, actor, space_id, &request.outcome_ids).await?
    };
    let lesson_revision_target = if let Some(revision) = request.lesson_revision.as_ref() {
        Some(
            load_lesson_revision_target(
                state,
                actor,
                space_id,
                request.output_vault_id,
                revision,
                &outcome_snapshot,
            )
            .await?,
        )
    } else {
        None
    };
    let source_ids = lesson_revision_target
        .as_ref()
        .map(|target| target.source_ids.clone())
        .unwrap_or_else(|| request.source_ids.clone());
    let idempotency_key = options.idempotency_key.unwrap_or_else(|| {
        idempotency_key(KnowledgeJobIdentity {
            actor,
            space_id,
            role,
            vault_id: request.output_vault_id,
            question,
            source_ids: &source_ids,
            outcome_ids: &request.outcome_ids,
            collection_id: bound_collection.as_ref().map(|collection| collection.id),
            collection_revision: bound_collection
                .as_ref()
                .map(|collection| collection.revision),
            profile: &profile,
            policy_revision: &policy_revision,
            role_profile_ref: &effective.profile_ref,
            bundle_execution: bundle_execution.as_ref(),
            lesson_revision_target: lesson_revision_target.as_ref(),
        })
    });
    let mut input = match role {
        KnowledgeJobRole::SourceScout => {
            source_scout_input(pool(state)?, actor, space_id, question).await?
        }
        KnowledgeJobRole::InsightSynthesizer => serde_json::json!({
            "question": question,
            "outcomes": outcome_snapshot,
        }),
        _ if outcome_snapshot.is_empty() => serde_json::json!({"question": question}),
        _ => serde_json::json!({
            "question": question,
            "outcomes": outcome_snapshot,
        }),
    };
    if let Some(execution) = bundle_execution {
        input
            .as_object_mut()
            .expect("Knowledge job input is an object")
            .insert(
                "bundle_execution".to_string(),
                serde_json::to_value(execution).map_err(|error| {
                    WorkbenchHttpError::Core(GadgetronError::Config(format!(
                        "Bundle Knowledge execution snapshot failed: {error}"
                    )))
                })?,
            );
    }
    if let Some(collection) = bound_collection {
        input
            .as_object_mut()
            .expect("Knowledge job input is an object")
            .insert(
                "collection_binding".to_string(),
                serde_json::json!({
                    "collection_id": collection.id,
                    "collection_revision": collection.revision,
                }),
            );
    }
    if let Some(target) = lesson_revision_target {
        input
            .as_object_mut()
            .expect("Knowledge job input is an object")
            .insert(
                LESSON_REVISION_TARGET_INPUT_KEY.to_string(),
                serde_json::to_value(target).map_err(|error| {
                    WorkbenchHttpError::Core(GadgetronError::Config(format!(
                        "Lesson revision target snapshot failed: {error}"
                    )))
                })?,
            );
    }
    input
        .as_object_mut()
        .expect("Knowledge job input is an object")
        .extend(options.extra_input);
    let job = jobs::enqueue(
        pool(state)?,
        actor,
        EnqueueKnowledgeJob {
            space_id,
            output_vault_id: request.output_vault_id,
            role,
            kind: options.kind,
            priority: 10,
            input,
            idempotency_key,
            source_ids,
            runtime,
            bundle_role: bundle_snapshot,
            budget: JobBudget {
                max_tokens: 16_384,
                max_sources,
                max_wall_seconds,
                max_attempts: 3,
            },
            scheduled_at: None,
        },
    )
    .await?;
    Ok(job)
}

async fn verified_outcome_snapshot(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    outcome_ids: &[Uuid],
) -> Result<Vec<VerifiedOutcomeSnapshot>, WorkbenchHttpError> {
    let mut rows = sqlx::query_as::<_, VerifiedOutcomeSnapshot>(
        r#"SELECT id, experience_revision, consumer_bundle_id,
                  subject_owner_bundle, subject_kind, subject_stable_id,
                  subject_revision, operation_id, context_query_id,
                  context_revision, predicate_result, verification_summary,
                  used_citations, created_at
           FROM knowledge_outcome_feedback
           WHERE tenant_id = $1 AND actor_user_id = $2
             AND id = ANY($3) AND predicate_result = 'satisfied'
             AND feedback_json->'authority'->'allowed_space_ids' ? $4"#,
    )
    .bind(actor.tenant_id)
    .bind(actor.user_id)
    .bind(outcome_ids)
    .bind(space_id.to_string())
    .fetch_all(pool)
    .await
    .map_err(|error| {
        WorkbenchHttpError::Core(GadgetronError::Config(format!(
            "Verified Outcome snapshot failed: {error}"
        )))
    })?;
    if rows.len() != outcome_ids.len() {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Every Outcome must be satisfied and visible to this actor in this Space"
                .to_string(),
        });
    }
    rows.sort_by_key(|row| row.id);
    Ok(rows)
}

async fn load_lesson_revision_target(
    state: &AppState,
    actor: SpaceActor,
    space_id: Uuid,
    output_vault_id: Uuid,
    request: &LessonRevisionRequest,
    outcomes: &[VerifiedOutcomeSnapshot],
) -> Result<LessonRevisionTarget, WorkbenchHttpError> {
    let location = sources::note_location(
        pool(state)?,
        actor,
        request.object_id,
        SpaceRole::Contributor,
        true,
    )
    .await?;
    if location.space_id != space_id
        || location.vault_id != output_vault_id
        || location.revision != request.expected_revision
    {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Lesson revision target is stale or outside the selected Space and Vault"
                .to_string(),
        });
    }
    let content_hash =
        location
            .content_hash
            .clone()
            .ok_or_else(|| WorkbenchHttpError::KnowledgeInvalidInput {
                detail: "Lesson revision target has no durable content hash".to_string(),
            })?;
    let source_revision = if let Some(source_id) = location.source_id {
        sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM knowledge_sources WHERE tenant_id = $1 AND id = $2",
        )
        .bind(actor.tenant_id)
        .bind(source_id)
        .fetch_optional(pool(state)?)
        .await
        .map_err(|error| {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "Lesson citation source revision lookup failed: {error}"
            )))
        })?
        .unwrap_or(location.revision)
    } else {
        location.revision
    }
    .to_string();
    let citation_id = format!("{}:{}", location.id, location.revision);
    if outcomes.iter().any(|outcome| {
        !outcome.used_citations.as_array().is_some_and(|citations| {
            citations.iter().any(|citation| {
                citation.get("citation_id").and_then(Value::as_str) == Some(citation_id.as_str())
                    && citation.get("source_revision").and_then(Value::as_str)
                        == Some(source_revision.as_str())
            })
        })
    }) {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Every selected verified Outcome must cite the exact reviewed Lesson revision"
                .to_string(),
        });
    }
    let layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.clone())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge Vault is unavailable".to_string(),
            ))
        })?;
    let tenant_id = actor.tenant_id;
    let read_space_id = location.space_id;
    let home_bundle_id = location.home_bundle_id.clone();
    let path = location.path.clone();
    let expected_hash = content_hash.clone();
    let note = tokio::task::spawn_blocking(move || {
        layout.open_or_init(tenant_id)?.read_note_reconciled(
            read_space_id,
            &home_bundle_id,
            &path,
            Some(&expected_hash),
        )
    })
    .await
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?;
    if note.externally_changed {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let raw =
        String::from_utf8(note.bytes).map_err(|_| WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Lesson revision target is not UTF-8".to_string(),
        })?;
    let parsed = gadgetron_knowledge::source::parse_obsidian_note(&raw).map_err(|error| {
        WorkbenchHttpError::KnowledgeInvalidInput {
            detail: format!("Lesson revision target is invalid: {error}"),
        }
    })?;
    if parsed
        .properties
        .get("knowledge_kind")
        .and_then(Value::as_str)
        != Some("lesson")
        || !matches!(
            parsed
                .properties
                .get("review_state")
                .and_then(Value::as_str),
            Some("reviewed" | "verified")
        )
    {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Lesson revision target must be a reviewed Lesson".to_string(),
        });
    }
    let source_ids = lesson_source_ids(&parsed.properties)?;
    let originating_subject = originating_subject(&parsed.properties)
        .map_err(|detail| WorkbenchHttpError::KnowledgeInvalidInput { detail })?;
    Ok(LessonRevisionTarget {
        object_id: location.id,
        expected_revision: location.revision,
        content_hash,
        title: parsed
            .properties
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Knowledge note")
            .to_string(),
        body: parsed.body,
        source_ids,
        originating_subject,
    })
}

fn lesson_source_ids(
    properties: &BTreeMap<String, Value>,
) -> Result<Vec<Uuid>, WorkbenchHttpError> {
    let values = properties
        .get("source_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Lesson revision target must retain Source provenance".to_string(),
        })?;
    let mut source_ids = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| WorkbenchHttpError::KnowledgeInvalidInput {
                    detail: "Lesson source_ids must contain UUIDs".to_string(),
                })?
                .parse()
                .map_err(|_| WorkbenchHttpError::KnowledgeInvalidInput {
                    detail: "Lesson source_ids must contain UUIDs".to_string(),
                })
        })
        .collect::<Result<Vec<Uuid>, _>>()?;
    source_ids.sort_unstable();
    source_ids.dedup();
    if source_ids.is_empty() {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "Lesson revision target must retain Source provenance".to_string(),
        });
    }
    Ok(source_ids)
}

async fn source_scout_input(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    question: &str,
) -> Result<Value, WorkbenchHttpError> {
    const MAX_COVERAGE_ROWS: usize = 100;
    let visible = sources::list_sources(pool, actor, space_id).await?;
    let rows = visible
        .iter()
        .take(MAX_COVERAGE_ROWS)
        .map(|source| {
            let locator = source
                .final_uri
                .as_deref()
                .or(source.requested_uri.as_deref());
            serde_json::json!({
                "title": source.title.chars().take(300).collect::<String>(),
                "source_kind": source.source_kind,
                "status": source.status,
                "origin_host": locator.and_then(source_origin_host),
                "content_type": source.content_type,
                "fetched_at": source.fetched_at,
                "updated_at": source.updated_at,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "question": question,
        "coverage": {
            "source_count": visible.len(),
            "shown_count": rows.len(),
            "truncated": visible.len() > rows.len(),
            "sources": rows,
        }
    }))
}

fn source_origin_host(locator: &str) -> Option<String> {
    reqwest::Url::parse(locator)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
}

pub async fn list_jobs_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<JobListResponse>, WorkbenchHttpError> {
    let rows = jobs::list_for_space(pool(&state)?, actor(&ctx)?, space_id, query.limit).await?;
    let returned = rows.len();
    Ok(Json(JobListResponse {
        jobs: rows,
        returned,
    }))
}

pub async fn get_job_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<JobDetailResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let job = jobs::get(pool(&state)?, actor, job_id).await?;
    let sources = jobs::sources(pool(&state)?, actor, job_id).await?;
    let artifacts = jobs::artifacts(pool(&state)?, actor, job_id).await?;
    Ok(Json(JobDetailResponse {
        job,
        sources,
        artifacts,
    }))
}

pub async fn cancel_job_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(job_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<jobs::KnowledgeJobRow>, WorkbenchHttpError> {
    Ok(Json(
        jobs::request_cancel(
            pool(&state)?,
            actor(&ctx)?,
            job_id,
            request.expected_revision,
        )
        .await?,
    ))
}

pub async fn retry_job_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(job_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<jobs::KnowledgeJobRow>, WorkbenchHttpError> {
    Ok(Json(
        jobs::retry(
            pool(&state)?,
            actor(&ctx)?,
            job_id,
            request.expected_revision,
        )
        .await?,
    ))
}

pub async fn list_change_sets_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ChangeSetListResponse>, WorkbenchHttpError> {
    let rows = jobs::list_change_sets(pool(&state)?, actor(&ctx)?, space_id, query.limit).await?;
    let returned = rows.len();
    Ok(Json(ChangeSetListResponse {
        change_sets: rows,
        returned,
    }))
}

pub async fn list_duplicate_groups_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
) -> Result<Json<DuplicateGroupListResponse>, WorkbenchHttpError> {
    let rows =
        spaces::list_objects(pool(&state)?, actor(&ctx)?, space_id, None, Some("note")).await?;
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|row| row.owner_state == "enabled")
        .collect();
    let mut parents: Vec<usize> = (0..rows.len()).collect();
    for left in 0..rows.len() {
        for right in (left + 1)..rows.len() {
            if rows[left].vault_id == rows[right].vault_id
                && duplicate_match(
                    rows[left].content_hash.as_deref(),
                    rows[left].title.as_deref(),
                    rows[right].content_hash.as_deref(),
                    rows[right].title.as_deref(),
                )
            {
                union_duplicate_roots(&mut parents, left, right);
            }
        }
    }
    let mut grouped: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..rows.len() {
        let root = duplicate_root(&mut parents, index);
        grouped.entry(root).or_default().push(index);
    }
    let mut groups: Vec<_> = grouped
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|mut members| {
            members.sort_by_key(|index| rows[*index].id);
            let same_hash = members.iter().enumerate().any(|(position, left)| {
                members.iter().skip(position + 1).any(|right| {
                    nonempty_equal(
                        rows[*left].content_hash.as_deref(),
                        rows[*right].content_hash.as_deref(),
                    )
                })
            });
            let same_title = members.iter().enumerate().any(|(position, left)| {
                members.iter().skip(position + 1).any(|right| {
                    normalized_title(rows[*left].title.as_deref()).is_some_and(|title| {
                        normalized_title(rows[*right].title.as_deref()).as_deref()
                            == Some(title.as_str())
                    })
                })
            });
            let stable_ids = members
                .iter()
                .map(|index| rows[*index].id.to_string())
                .collect::<Vec<_>>()
                .join(":");
            let group_hash = hex::encode(Sha256::digest(stable_ids.as_bytes()));
            let candidates = members
                .into_iter()
                .map(|index| DuplicateCandidate {
                    object_id: rows[index].id,
                    vault_id: rows[index].vault_id,
                    home_bundle_id: rows[index].home_bundle_id.clone(),
                    title: rows[index].title.clone(),
                    path: rows[index].path.clone(),
                    content_hash: rows[index].content_hash.clone(),
                    revision: rows[index].revision,
                    updated_at: rows[index].updated_at,
                })
                .collect();
            DuplicateGroup {
                id: format!("exact:{}", &group_hash[..24]),
                confidence: "exact",
                match_reasons: [
                    same_hash.then_some("content_hash"),
                    same_title.then_some("normalized_title"),
                ]
                .into_iter()
                .flatten()
                .collect(),
                candidates,
            }
        })
        .collect();
    groups.sort_by(|left, right| left.id.cmp(&right.id));
    let returned = groups.len();
    Ok(Json(DuplicateGroupListResponse { groups, returned }))
}

pub async fn create_merge_change_set_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Json(request): Json<CreateMergeChangeSetRequest>,
) -> Result<Json<jobs::KnowledgeChangeSetRow>, WorkbenchHttpError> {
    if !(2..=20).contains(&request.sources.len()) || request.field_sources.len() > 32 {
        return Err(knowledge_invalid("Select between 2 and 20 duplicate notes"));
    }
    let actor = actor(&ctx)?;
    let mut source_ids = HashSet::new();
    let mut snapshots = Vec::with_capacity(request.sources.len());
    for source in &request.sources {
        if source.expected_revision < 1 || !source_ids.insert(source.object_id) {
            return Err(knowledge_invalid("Duplicate note selection is invalid"));
        }
        let snapshot = load_merge_note_snapshot(&state, actor, space_id, source.object_id).await?;
        if snapshot.revision != source.expected_revision {
            return Err(WorkbenchHttpError::KnowledgeConflict);
        }
        snapshots.push(snapshot);
    }
    let master_index = snapshots
        .iter()
        .position(|snapshot| snapshot.object_id == request.master_object_id)
        .ok_or_else(|| knowledge_invalid("The primary note must be in the selected group"))?;
    let output_vault_id = snapshots[master_index].vault_id;
    if snapshots
        .iter()
        .any(|snapshot| snapshot.vault_id != output_vault_id)
        || !snapshots_form_exact_group(&snapshots)
    {
        return Err(knowledge_invalid(
            "Selected notes are not one exact duplicate group in the same Knowledge domain",
        ));
    }
    let mut properties = snapshots[master_index].properties.clone();
    for key in MERGE_SYSTEM_PROPERTIES {
        properties.remove(key);
    }
    let mut title = snapshots[master_index].title.clone();
    for (field, source_id) in &request.field_sources {
        if !valid_merge_field(field) {
            return Err(knowledge_invalid("A selected merge field is not editable"));
        }
        let source = snapshots
            .iter()
            .find(|snapshot| snapshot.object_id == *source_id)
            .ok_or_else(|| knowledge_invalid("A field source is outside the selected group"))?;
        if field == "title" {
            title.clone_from(&source.title);
        } else if let Some(value) = source.properties.get(field) {
            properties.insert(field.clone(), value.clone());
        } else {
            properties.remove(field);
        }
    }
    let body = match request.body_strategy {
        MergeBodyStrategy::KeepCurrent => snapshots[master_index].body.clone(),
        MergeBodyStrategy::UseIncoming => {
            let incoming_id = request
                .incoming_object_id
                .ok_or_else(|| knowledge_invalid("Choose the incoming note body to keep"))?;
            snapshots
                .iter()
                .find(|snapshot| {
                    snapshot.object_id == incoming_id && incoming_id != request.master_object_id
                })
                .map(|snapshot| snapshot.body.clone())
                .ok_or_else(|| {
                    knowledge_invalid("The incoming body must come from another selected note")
                })?
        }
        MergeBodyStrategy::KeepBoth => {
            let mut bodies = Vec::with_capacity(snapshots.len());
            bodies.push(snapshots[master_index].body.trim().to_string());
            bodies.extend(
                snapshots
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != master_index)
                    .map(|(_, snapshot)| snapshot.body.trim().to_string()),
            );
            bodies
                .into_iter()
                .filter(|body| !body.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    };
    if title.trim().is_empty() || title.chars().count() > 512 || body.len() > 16 * 1024 * 1024 {
        return Err(knowledge_invalid(
            "The merged note title or body is outside supported limits",
        ));
    }
    let expected_git_revision = current_git_revision(&state, actor.tenant_id).await?;
    let operations = serde_json::json!([{
        "op": "merge_notes",
        "sources": request.sources.iter().map(|source| serde_json::json!({
            "object_id": source.object_id,
            "expected_revision": source.expected_revision,
        })).collect::<Vec<_>>(),
        "title": title.trim(),
        "body": body,
        "properties": properties,
    }]);
    let input = jobs::ChangeSetInput {
        candidate_artifact_id: None,
        title: format!("Merge {} duplicate notes", request.sources.len()),
        summary: format!(
            "Keep ‘{}’ as the primary note and combine {} exact duplicates.",
            title.trim(),
            request.sources.len()
        ),
        operations,
        citations: Value::Array(Vec::new()),
        expected_git_revision: Some(expected_git_revision),
        materialization_key: format!("user-merge:{}:{}", actor.user_id, request.idempotency_key),
    };
    Ok(Json(
        jobs::create_user_merge_change_set(pool(&state)?, actor, space_id, output_vault_id, input)
            .await?,
    ))
}

pub async fn list_evolution_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<EvolutionListResponse>, WorkbenchHttpError> {
    let traces =
        jobs::evolution_for_space(pool(&state)?, actor(&ctx)?, space_id, query.limit).await?;
    let returned = traces.len();
    Ok(Json(EvolutionListResponse { traces, returned }))
}

pub async fn get_change_set_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(change_set_id): Path<Uuid>,
) -> Result<Json<jobs::KnowledgeChangeSetRow>, WorkbenchHttpError> {
    Ok(Json(
        jobs::get_change_set(pool(&state)?, actor(&ctx)?, change_set_id).await?,
    ))
}

pub async fn edit_change_set_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(change_set_id): Path<Uuid>,
    Json(mut request): Json<jobs::EditChangeSet>,
) -> Result<Json<jobs::KnowledgeChangeSetRow>, WorkbenchHttpError> {
    let layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.clone())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge Vault is unavailable".to_string(),
            ))
        })?;
    let tenant_id = ctx.tenant_id;
    request.expected_git_revision = Some(
        tokio::task::spawn_blocking(move || {
            layout
                .open_or_init(tenant_id)?
                .snapshot()
                .map(|snapshot| snapshot.git_head)
        })
        .await
        .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?
        .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?,
    );
    Ok(Json(
        jobs::edit_change_set(pool(&state)?, actor(&ctx)?, change_set_id, request).await?,
    ))
}

pub async fn accept_change_set_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(change_set_id): Path<Uuid>,
    Json(request): Json<ChangeSetDecisionRequest>,
) -> Result<Json<jobs::KnowledgeChangeSetRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let accepted = jobs::decide_change_set(
        pool(&state)?,
        actor,
        change_set_id,
        request.expected_revision,
        jobs::ChangeSetDecision::Accept,
        request.rationale.as_deref(),
    )
    .await?;
    Ok(Json(materialize_change_set(&state, actor, accepted).await?))
}

pub async fn reject_change_set_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(change_set_id): Path<Uuid>,
    Json(request): Json<ChangeSetDecisionRequest>,
) -> Result<Json<jobs::KnowledgeChangeSetRow>, WorkbenchHttpError> {
    Ok(Json(
        jobs::decide_change_set(
            pool(&state)?,
            actor(&ctx)?,
            change_set_id,
            request.expected_revision,
            jobs::ChangeSetDecision::Reject,
            request.rationale.as_deref(),
        )
        .await?,
    ))
}

pub async fn retry_change_set_apply_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(change_set_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<jobs::KnowledgeChangeSetRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let change_set = jobs::get_change_set(pool(&state)?, actor, change_set_id).await?;
    if change_set.revision != request.expected_revision || change_set.status != "failed_retryable" {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    Ok(Json(
        retry_materialization(&state, actor, change_set).await?,
    ))
}

#[derive(Debug, FromRow)]
struct MaterializationObjectRow {
    path: String,
    content_hash: Option<String>,
    revision: i64,
    originating_owner_bundle: Option<String>,
    originating_subject_kind: Option<String>,
    originating_subject_id: Option<String>,
    originating_subject_revision: Option<String>,
}

impl MaterializationObjectRow {
    fn originating_subject(&self) -> Result<Option<jobs::OriginatingSubject>, String> {
        match (
            self.originating_owner_bundle.as_ref(),
            self.originating_subject_kind.as_ref(),
            self.originating_subject_id.as_ref(),
            self.originating_subject_revision.as_ref(),
        ) {
            (None, None, None, None) => Ok(None),
            (Some(owner_bundle), Some(subject_kind), Some(subject_id), Some(subject_revision)) => {
                Ok(Some(jobs::OriginatingSubject {
                    owner_bundle: owner_bundle.clone(),
                    subject_kind: subject_kind.clone(),
                    subject_id: subject_id.clone(),
                    subject_revision: subject_revision.clone(),
                }))
            }
            _ => Err("A target note has partial originating subject metadata".to_string()),
        }
    }
}

struct PreparedMaterialization {
    writes: Vec<gadgetron_knowledge::vault::VaultNoteWrite>,
    objects: Vec<jobs::MaterializedObjectInput>,
}

struct EvolutionMaterialization {
    candidate_artifact_id: Uuid,
    candidate: KnowledgeEvolutionCandidate,
}

struct MergeNoteSnapshot {
    object_id: Uuid,
    vault_id: Uuid,
    revision: i64,
    content_hash: String,
    title: String,
    properties: BTreeMap<String, Value>,
    body: String,
}

const MERGE_SYSTEM_PROPERTIES: [&str; 9] = [
    "id",
    "title",
    "change_set",
    "canonical_change",
    "derived_from",
    "supersedes",
    "source_revisions",
    "content_hash",
    "originating_subject",
];

async fn retry_materialization(
    state: &AppState,
    actor: SpaceActor,
    change_set: jobs::KnowledgeChangeSetRow,
) -> Result<jobs::KnowledgeChangeSetRow, WorkbenchHttpError> {
    let layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.clone())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge Vault is unavailable".to_string(),
            ))
        })?;
    let tenant_id = actor.tenant_id;
    let current_git_revision = tokio::task::spawn_blocking(move || {
        layout
            .open_or_init(tenant_id)?
            .snapshot()
            .map(|snapshot| snapshot.git_head)
    })
    .await
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?;
    let (operations, target_changed) = refresh_operation_revisions(
        pool(state)?,
        actor,
        change_set.output_vault_id,
        &change_set.operations,
    )
    .await
    .map_err(|detail| WorkbenchHttpError::KnowledgeInvalidInput { detail })?;

    if target_changed {
        return Ok(jobs::return_materialization_to_review(
            pool(state)?,
            actor,
            change_set.id,
            change_set.revision,
            operations,
            &current_git_revision,
            "A target note changed after this proposal was reviewed. Review the refreshed diff before accepting it again.",
        )
        .await?);
    }

    let refreshed = jobs::refresh_materialization_retry(
        pool(state)?,
        actor,
        change_set.id,
        change_set.revision,
        &current_git_revision,
    )
    .await?;
    materialize_change_set(state, actor, refreshed).await
}

async fn refresh_operation_revisions(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
    operations: &Value,
) -> Result<(Value, bool), String> {
    let mut refreshed = operations.clone();
    let operations = refreshed
        .as_array_mut()
        .ok_or_else(|| "Change-set operations are invalid".to_string())?;
    let mut changed = false;
    for operation in operations {
        match required_str(operation, "op")? {
            "create_note" => {}
            "update_note" | "link" => {
                changed |= refresh_expected_revision(pool, actor, vault_id, operation, "object_id")
                    .await?;
            }
            "merge_notes" => {
                let sources = operation
                    .get_mut("sources")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| "Change-set field sources is required".to_string())?;
                for source in sources {
                    changed |=
                        refresh_expected_revision(pool, actor, vault_id, source, "object_id")
                            .await?;
                }
            }
            "split_note" => {
                changed |=
                    refresh_expected_revision(pool, actor, vault_id, operation, "source_object_id")
                        .await?;
            }
            kind => return Err(format!("Unsupported change-set operation {kind}")),
        }
    }
    Ok((refreshed, changed))
}

async fn refresh_expected_revision(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
    reference: &mut Value,
    object_id_field: &str,
) -> Result<bool, String> {
    let object_id = required_uuid(reference, object_id_field)?;
    let expected_revision = required_i64(reference, "expected_revision")?;
    let current = load_materialization_object(pool, actor, vault_id, object_id).await?;
    let changed = current.revision != expected_revision;
    if changed {
        reference
            .as_object_mut()
            .ok_or_else(|| "Change-set object reference is invalid".to_string())?
            .insert(
                "expected_revision".to_string(),
                Value::from(current.revision),
            );
    }
    Ok(changed)
}

async fn materialize_change_set(
    state: &AppState,
    actor: SpaceActor,
    accepted: jobs::KnowledgeChangeSetRow,
) -> Result<jobs::KnowledgeChangeSetRow, WorkbenchHttpError> {
    let materializing =
        jobs::begin_materialization(pool(state)?, actor, accepted.id, accepted.revision).await?;
    let result = prepare_and_write_change_set(state, actor, &materializing).await;
    let (git_revision, objects) = match result {
        Ok(result) => result,
        Err(detail) => {
            return Ok(jobs::fail_materialization(
                pool(state)?,
                actor,
                materializing.id,
                materializing.revision,
                &clip_error(&detail),
            )
            .await?);
        }
    };
    match jobs::complete_materialization(
        pool(state)?,
        actor,
        materializing.id,
        materializing.revision,
        &git_revision,
        &objects,
    )
    .await
    {
        Ok(applied) => {
            super::knowledge_graph::reconcile_after_change(state, actor).await;
            Ok(applied)
        }
        Err(error) => Ok(jobs::fail_materialization(
            pool(state)?,
            actor,
            materializing.id,
            materializing.revision,
            &clip_error(&error.to_string()),
        )
        .await?),
    }
}

async fn prepare_and_write_change_set(
    state: &AppState,
    actor: SpaceActor,
    change_set: &jobs::KnowledgeChangeSetRow,
) -> Result<(String, Vec<jobs::MaterializedObjectInput>), String> {
    let layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.clone())
        .ok_or_else(|| "Knowledge Vault is unavailable".to_string())?;
    let pg_pool = state
        .pg_pool
        .as_ref()
        .ok_or_else(|| "Knowledge jobs require PostgreSQL".to_string())?;
    let home_bundle_id: String = sqlx::query_scalar(
        r#"SELECT home_bundle_id FROM knowledge_vaults
           WHERE tenant_id = $1 AND id = $2 AND space_id = $3 AND owner_state = 'enabled'"#,
    )
    .bind(actor.tenant_id)
    .bind(change_set.output_vault_id)
    .bind(change_set.space_id)
    .fetch_optional(pg_pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "The output Vault is unavailable".to_string())?;
    let prepared =
        prepare_operations(pg_pool, layout.as_ref(), actor, change_set, &home_bundle_id).await?;
    let expected_git_revision = change_set.expected_git_revision.clone();
    let space_id = change_set.space_id;
    let repository = layout
        .open_or_init(actor.tenant_id)
        .map_err(|error| error.to_string())?;
    let writes = prepared.writes;
    let states = tokio::task::spawn_blocking(move || {
        repository.write_notes_batch(
            space_id,
            &home_bundle_id,
            writes,
            expected_git_revision.as_deref(),
            "knowledge: apply reviewed Gardener change set",
        )
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    let hashes: BTreeMap<_, _> = states
        .iter()
        .map(|state| (state.path.as_str(), state.content_hash.as_str()))
        .collect();
    let objects = prepared
        .objects
        .into_iter()
        .map(|mut object| {
            object.content_hash = hashes
                .get(object.path.as_str())
                .copied()
                .unwrap_or_default()
                .to_string();
            object
        })
        .collect();
    let git_revision = states
        .first()
        .map(|state| state.git_revision.clone())
        .ok_or_else(|| "The change set produced no Vault writes".to_string())?;
    Ok((git_revision, objects))
}

async fn prepare_operations(
    pool: &sqlx::PgPool,
    layout: &gadgetron_knowledge::vault::TenantVaultLayout,
    actor: SpaceActor,
    change_set: &jobs::KnowledgeChangeSetRow,
    home_bundle_id: &str,
) -> Result<PreparedMaterialization, String> {
    let operations = change_set
        .operations
        .as_array()
        .ok_or_else(|| "Change-set operations are invalid".to_string())?;
    let evolution = load_evolution_materialization(pool, actor, change_set).await?;
    let pinned_origin = pinned_originating_subject(pool, actor, change_set).await?;
    let pinned_lesson_revision = pinned_lesson_revision_target(pool, actor, change_set).await?;
    if pinned_lesson_revision.is_some() && evolution.is_none() {
        return Err(
            "A pinned Lesson revision requires a reviewed structured Candidate".to_string(),
        );
    }
    if evolution.is_some() {
        let expected_operation = if pinned_lesson_revision.is_some() {
            "update_note"
        } else {
            "create_note"
        };
        if operations.len() != 1
            || operations[0].get("op").and_then(Value::as_str) != Some(expected_operation)
        {
            return Err(format!(
                "A structured Candidate must materialize as one {expected_operation} operation",
            ));
        }
    }
    let mut writes = Vec::with_capacity(operations.len());
    let mut objects = Vec::with_capacity(operations.len());
    let mut touched = HashSet::new();
    for (position, operation) in operations.iter().enumerate() {
        let kind = required_str(operation, "op")?;
        match kind {
            "create_note" => {
                let id = deterministic_object_id(change_set.id, position);
                if !touched.insert(id) {
                    return Err("A note is changed more than once in one change set".to_string());
                }
                let title = required_str(operation, "title")?;
                let body = required_str(operation, "body")?;
                let mut properties = optional_properties(operation)?;
                apply_pinned_originating_subject(&mut properties, pinned_origin.as_ref())?;
                validate_review_relation_targets(pool, actor, change_set.space_id, &properties)
                    .await?;
                properties.insert("id".to_string(), Value::String(id.to_string()));
                properties.insert("title".to_string(), Value::String(title.to_string()));
                properties.insert(
                    "change_set".to_string(),
                    Value::String(change_set.id.to_string()),
                );
                if let Some(evolution) = evolution.as_ref() {
                    apply_evolution_properties(&mut properties, evolution)?;
                }
                push_new_note(&mut writes, &mut objects, id, body, &properties)?;
            }
            "update_note" | "link" => {
                let id = required_uuid(operation, "object_id")?;
                if !touched.insert(id) {
                    return Err("A note is changed more than once in one change set".to_string());
                }
                let expected_revision = required_i64(operation, "expected_revision")?;
                let object =
                    load_materialization_object(pool, actor, change_set.output_vault_id, id)
                        .await?;
                if object.revision != expected_revision {
                    return Err("A target note changed after the proposal was created".to_string());
                }
                if let Some(target) = pinned_lesson_revision.as_ref() {
                    if kind != "update_note"
                        || id != target.object_id
                        || expected_revision != target.expected_revision
                        || object.content_hash.as_deref() != Some(target.content_hash.as_str())
                    {
                        return Err(
                            "A Lesson revision must retain the Core-pinned object, revision and content hash"
                                .to_string(),
                        );
                    }
                }
                let repository = layout
                    .open_or_init(actor.tenant_id)
                    .map_err(|error| error.to_string())?;
                let path = object.path.clone();
                let expected_hash = object.content_hash.clone();
                let bundle = home_bundle_id.to_string();
                let space_id = change_set.space_id;
                let note = tokio::task::spawn_blocking(move || {
                    repository.read_note_reconciled(
                        space_id,
                        &bundle,
                        &path,
                        expected_hash.as_deref(),
                    )
                })
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
                if note.externally_changed {
                    return Err("A target note changed in the Vault after review began".to_string());
                }
                let raw = String::from_utf8(note.bytes)
                    .map_err(|_| "A target note is not UTF-8".to_string())?;
                let mut parsed = gadgetron_knowledge::source::parse_obsidian_note(&raw)
                    .map_err(|error| error.to_string())?;
                let registry_origin = object.originating_subject()?;
                if originating_subject(&parsed.properties)? != registry_origin {
                    return Err(
                        "A target note originating subject does not match the Core registry"
                            .to_string(),
                    );
                }
                if let Some(target) = pinned_lesson_revision.as_ref() {
                    let title = parsed
                        .properties
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Knowledge note");
                    if title != target.title
                        || parsed.body != target.body
                        || materialization_lesson_source_ids(&parsed.properties)?
                            != target.source_ids
                        || registry_origin != target.originating_subject
                    {
                        return Err(
                            "The pinned Lesson changed after the revision proposal was created"
                                .to_string(),
                        );
                    }
                }
                if kind == "update_note" {
                    parsed.body = required_str(operation, "body")?.to_string();
                    if let Some(title) = operation.get("title").and_then(Value::as_str) {
                        parsed
                            .properties
                            .insert("title".to_string(), Value::String(title.to_string()));
                    }
                    if let Some(evolution) = evolution.as_ref() {
                        apply_evolution_properties(&mut parsed.properties, evolution)?;
                    }
                } else {
                    let target_id = required_uuid(operation, "target_object_id")?;
                    load_materialization_object(pool, actor, change_set.output_vault_id, target_id)
                        .await?;
                    let relation = operation
                        .get("relation")
                        .and_then(Value::as_str)
                        .unwrap_or("Related");
                    let link = format!("[[{target_id}]]");
                    if !parsed.body.contains(&link) {
                        parsed.body = format!("{}\n\n{relation}: {link}\n", parsed.body.trim_end());
                    }
                }
                let bytes = gadgetron_knowledge::source::serialize_obsidian_note(
                    &parsed.properties,
                    &parsed.body,
                )
                .map_err(|error| error.to_string())?
                .into_bytes();
                writes.push(gadgetron_knowledge::vault::VaultNoteWrite {
                    relative_path: object.path.clone(),
                    bytes,
                });
                objects.push(jobs::MaterializedObjectInput {
                    id,
                    path: object.path,
                    content_hash: String::new(),
                    expected_revision: Some(expected_revision),
                    originating_subject: registry_origin,
                });
            }
            "merge_notes" => {
                let sources = revisioned_note_refs(operation, "sources")?;
                if !(2..=20).contains(&sources.len()) {
                    return Err("A merge must contain between 2 and 20 source notes".to_string());
                }
                let mut unique_sources = HashSet::new();
                for (source_id, expected_revision) in &sources {
                    if !unique_sources.insert(*source_id) {
                        return Err("A merge source note is repeated".to_string());
                    }
                    let source = load_materialization_object(
                        pool,
                        actor,
                        change_set.output_vault_id,
                        *source_id,
                    )
                    .await?;
                    if source.revision != *expected_revision {
                        return Err(
                            "A merge source changed after the proposal was created".to_string()
                        );
                    }
                }
                let id = deterministic_object_id(change_set.id, position);
                if !touched.insert(id) {
                    return Err("A note is changed more than once in one change set".to_string());
                }
                let title = required_str(operation, "title")?;
                let body = required_str(operation, "body")?;
                let mut properties = optional_properties(operation)?;
                apply_pinned_originating_subject(&mut properties, pinned_origin.as_ref())?;
                validate_review_relation_targets(pool, actor, change_set.space_id, &properties)
                    .await?;
                let source_ids: Vec<_> = sources
                    .iter()
                    .map(|(source_id, _)| Value::String(source_id.to_string()))
                    .collect();
                let source_revisions: BTreeMap<_, _> = sources
                    .iter()
                    .map(|(source_id, revision)| (source_id.to_string(), Value::from(*revision)))
                    .collect();
                properties.insert("id".to_string(), Value::String(id.to_string()));
                properties.insert("title".to_string(), Value::String(title.to_string()));
                properties.insert(
                    "change_set".to_string(),
                    Value::String(change_set.id.to_string()),
                );
                properties.insert(
                    "canonical_change".to_string(),
                    Value::String("merge".to_string()),
                );
                properties.insert("derived_from".to_string(), Value::Array(source_ids.clone()));
                properties.insert("supersedes".to_string(), Value::Array(source_ids));
                properties.insert(
                    "source_revisions".to_string(),
                    serde_json::to_value(source_revisions).map_err(|error| error.to_string())?,
                );
                push_new_note(&mut writes, &mut objects, id, body, &properties)?;
            }
            "split_note" => {
                let source_id = required_uuid(operation, "source_object_id")?;
                let expected_revision = required_i64(operation, "expected_revision")?;
                let source =
                    load_materialization_object(pool, actor, change_set.output_vault_id, source_id)
                        .await?;
                if source.revision != expected_revision {
                    return Err(
                        "The split source changed after the proposal was created".to_string()
                    );
                }
                let outputs = operation
                    .get("outputs")
                    .and_then(Value::as_array)
                    .filter(|outputs| (2..=20).contains(&outputs.len()))
                    .ok_or_else(|| {
                        "A split must contain between 2 and 20 output notes".to_string()
                    })?;
                for (output_position, output) in outputs.iter().enumerate() {
                    let id =
                        deterministic_split_object_id(change_set.id, position, output_position);
                    if !touched.insert(id) {
                        return Err(
                            "A note is changed more than once in one change set".to_string()
                        );
                    }
                    let title = required_str(output, "title")?;
                    let body = required_str(output, "body")?;
                    let mut properties = optional_properties(output)?;
                    apply_pinned_originating_subject(&mut properties, pinned_origin.as_ref())?;
                    validate_review_relation_targets(pool, actor, change_set.space_id, &properties)
                        .await?;
                    properties.insert("id".to_string(), Value::String(id.to_string()));
                    properties.insert("title".to_string(), Value::String(title.to_string()));
                    properties.insert(
                        "change_set".to_string(),
                        Value::String(change_set.id.to_string()),
                    );
                    properties.insert(
                        "canonical_change".to_string(),
                        Value::String("split".to_string()),
                    );
                    properties.insert(
                        "derived_from".to_string(),
                        Value::String(source_id.to_string()),
                    );
                    properties.insert(
                        "source_revision".to_string(),
                        Value::from(expected_revision),
                    );
                    push_new_note(&mut writes, &mut objects, id, body, &properties)?;
                }
            }
            _ => return Err(format!("Unsupported change-set operation {kind}")),
        }
    }
    Ok(PreparedMaterialization { writes, objects })
}

async fn pinned_originating_subject(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    change_set: &jobs::KnowledgeChangeSetRow,
) -> Result<Option<jobs::OriginatingSubject>, String> {
    let Some(job_id) = change_set.job_id else {
        return if change_set.origin == "user" {
            Ok(None)
        } else {
            Err("The change-set job snapshot is unavailable".to_string())
        };
    };
    let input: Value = sqlx::query_scalar(
        "SELECT input FROM knowledge_jobs WHERE tenant_id = $1 AND id = $2 AND space_id = $3",
    )
    .bind(actor.tenant_id)
    .bind(job_id)
    .bind(change_set.space_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "The change-set job snapshot is unavailable".to_string())?;
    input
        .get("originating_subject")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("Pinned originating subject is invalid: {error}"))
}

async fn pinned_lesson_revision_target(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    change_set: &jobs::KnowledgeChangeSetRow,
) -> Result<Option<LessonRevisionTarget>, String> {
    let Some(job_id) = change_set.job_id else {
        return if change_set.origin == "user" {
            Ok(None)
        } else {
            Err("The change-set job snapshot is unavailable".to_string())
        };
    };
    let input: Value = sqlx::query_scalar(
        "SELECT input FROM knowledge_jobs WHERE tenant_id = $1 AND id = $2 AND space_id = $3",
    )
    .bind(actor.tenant_id)
    .bind(job_id)
    .bind(change_set.space_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "The change-set job snapshot is unavailable".to_string())?;
    input
        .get(LESSON_REVISION_TARGET_INPUT_KEY)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("Pinned Lesson revision target is invalid: {error}"))
}

fn materialization_lesson_source_ids(
    properties: &BTreeMap<String, Value>,
) -> Result<Vec<Uuid>, String> {
    let values = properties
        .get("source_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| "A pinned Lesson is missing Source provenance".to_string())?;
    let mut source_ids = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "A pinned Lesson Source reference is invalid".to_string())?
                .parse()
                .map_err(|_| "A pinned Lesson Source reference is invalid".to_string())
        })
        .collect::<Result<Vec<Uuid>, _>>()?;
    source_ids.sort_unstable();
    source_ids.dedup();
    if source_ids.is_empty() {
        return Err("A pinned Lesson is missing Source provenance".to_string());
    }
    Ok(source_ids)
}

fn apply_pinned_originating_subject(
    properties: &mut BTreeMap<String, Value>,
    pinned: Option<&jobs::OriginatingSubject>,
) -> Result<(), String> {
    let supplied = originating_subject(properties)?;
    match (pinned, supplied.as_ref()) {
        (None, Some(_)) => {
            return Err(
                "originating_subject is reserved for a Core-pinned domain event".to_string(),
            )
        }
        (Some(expected), Some(actual)) if actual != expected => {
            return Err("originating_subject does not match the Core-pinned event".to_string())
        }
        _ => {}
    }
    if let Some(pinned) = pinned {
        properties.insert(
            "originating_subject".to_string(),
            serde_json::to_value(pinned).map_err(|error| error.to_string())?,
        );
    }
    Ok(())
}

async fn load_evolution_materialization(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    change_set: &jobs::KnowledgeChangeSetRow,
) -> Result<Option<EvolutionMaterialization>, String> {
    let Some(candidate_artifact_id) = change_set.candidate_artifact_id else {
        return Ok(None);
    };
    let artifact = jobs::get_artifact(pool, actor, candidate_artifact_id)
        .await
        .map_err(|error| error.to_string())?;
    if artifact.kind != "candidate" || artifact.space_id != change_set.space_id {
        return Err("The reviewed Candidate artifact is unavailable".to_string());
    }
    let candidate =
        KnowledgeEvolutionCandidate::parse_and_validate(artifact.payload, &artifact.citations)
            .map_err(|error| error.to_string())?;
    if candidate.readiness() != KnowledgeEvolutionReadiness::ReadyForReview {
        return Err("Insight needs verified Outcome evidence before materialization".to_string());
    }
    if !candidate.verified_outcome_ids.is_empty() {
        let verified: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM knowledge_outcome_feedback
               WHERE tenant_id = $1 AND id = ANY($2) AND predicate_result = 'satisfied'"#,
        )
        .bind(actor.tenant_id)
        .bind(&candidate.verified_outcome_ids)
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
        if verified != candidate.verified_outcome_ids.len() as i64 {
            return Err("Candidate Outcome evidence is no longer verifiable".to_string());
        }
    }
    Ok(Some(EvolutionMaterialization {
        candidate_artifact_id,
        candidate,
    }))
}

fn apply_evolution_properties(
    properties: &mut BTreeMap<String, Value>,
    evolution: &EvolutionMaterialization,
) -> Result<(), String> {
    let candidate = &evolution.candidate;
    properties.insert(
        "knowledge_kind".to_string(),
        Value::String(candidate.target_kind.as_str().to_string()),
    );
    properties.insert(
        "review_state".to_string(),
        Value::String(
            if candidate.verified_outcome_ids.is_empty() {
                "reviewed"
            } else {
                "verified"
            }
            .to_string(),
        ),
    );
    properties.insert(
        "evolution_candidate_id".to_string(),
        Value::String(evolution.candidate_artifact_id.to_string()),
    );
    properties.insert(
        "derived_from".to_string(),
        Value::String(evolution.candidate_artifact_id.to_string()),
    );
    properties.insert(
        "source_ids".to_string(),
        serde_json::to_value(candidate.source_ids()).map_err(|error| error.to_string())?,
    );
    properties.insert(
        "applicability".to_string(),
        serde_json::to_value(&candidate.applicability).map_err(|error| error.to_string())?,
    );
    properties.insert(
        "limitations".to_string(),
        serde_json::to_value(&candidate.limitations).map_err(|error| error.to_string())?,
    );
    properties.insert(
        "freshness".to_string(),
        Value::String(candidate.freshness.status.as_str().to_string()),
    );
    properties.insert(
        "freshness_reason".to_string(),
        Value::String(candidate.freshness.reason.clone()),
    );
    if let Some(review_after) = candidate.freshness.review_after.as_ref() {
        properties.insert(
            "review_after".to_string(),
            Value::String(review_after.clone()),
        );
    }
    properties.insert(
        "confidence".to_string(),
        serde_json::to_value(candidate.confidence).map_err(|error| error.to_string())?,
    );
    properties.insert(
        "importance".to_string(),
        serde_json::to_value(candidate.importance_score()).map_err(|error| error.to_string())?,
    );
    if !candidate.verified_outcome_ids.is_empty() {
        properties.insert(
            "outcome_of".to_string(),
            serde_json::to_value(&candidate.verified_outcome_ids)
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(())
}

async fn load_materialization_object(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
    object_id: Uuid,
) -> Result<MaterializationObjectRow, String> {
    sqlx::query_as::<_, MaterializationObjectRow>(
        r#"SELECT path, content_hash, revision,
                  originating_owner_bundle, originating_subject_kind,
                  originating_subject_id, originating_subject_revision
           FROM knowledge_objects
           WHERE tenant_id = $1 AND vault_id = $2 AND id = $3 AND status = 'active'"#,
    )
    .bind(actor.tenant_id)
    .bind(vault_id)
    .bind(object_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "A target note is unavailable".to_string())
}

const REVIEW_RELATION_PROPERTIES: [&str; 4] =
    ["applies_to", "supports", "contradicts", "bridge_to"];

async fn validate_review_relation_targets(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    output_space_id: Uuid,
    properties: &BTreeMap<String, Value>,
) -> Result<(), String> {
    for target_id in review_relation_targets(properties)? {
        let target = sources::note_location(pool, actor, target_id, SpaceRole::Viewer, false)
            .await
            .map_err(|_| "A reviewed relation target is unavailable".to_string())?;
        if target.space_id != output_space_id {
            return Err(
                "A cross-Space relation requires an explicit shared reference in the output Space"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn review_relation_targets(properties: &BTreeMap<String, Value>) -> Result<Vec<Uuid>, String> {
    let mut targets = Vec::new();
    for property in REVIEW_RELATION_PROPERTIES {
        let Some(value) = properties.get(property) else {
            continue;
        };
        let refs = match value {
            Value::String(value) => vec![value.as_str()],
            Value::Array(values) => values
                .iter()
                .map(|value| {
                    value.as_str().ok_or_else(|| {
                        format!("Reviewed relation property {property} must contain note UUIDs")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => {
                return Err(format!(
                    "Reviewed relation property {property} must contain note UUIDs"
                ))
            }
        };
        for target in refs {
            targets.push(target.parse().map_err(|_| {
                format!("Reviewed relation property {property} contains an invalid note UUID")
            })?);
        }
    }
    targets.sort_unstable();
    targets.dedup();
    Ok(targets)
}

fn required_str<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Change-set field {field} is required"))
}

fn required_uuid(value: &Value, field: &str) -> Result<Uuid, String> {
    required_str(value, field)?
        .parse()
        .map_err(|_| format!("Change-set field {field} is not a UUID"))
}

fn required_i64(value: &Value, field: &str) -> Result<i64, String> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Change-set field {field} is invalid"))
}

fn optional_properties(value: &Value) -> Result<BTreeMap<String, Value>, String> {
    value
        .get("properties")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("Invalid note properties: {error}"))
        .map(Option::unwrap_or_default)
}

fn push_new_note(
    writes: &mut Vec<gadgetron_knowledge::vault::VaultNoteWrite>,
    objects: &mut Vec<jobs::MaterializedObjectInput>,
    id: Uuid,
    body: &str,
    properties: &BTreeMap<String, Value>,
) -> Result<(), String> {
    let bytes = gadgetron_knowledge::source::serialize_obsidian_note(properties, body)
        .map_err(|error| error.to_string())?
        .into_bytes();
    let title = properties
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Knowledge note");
    let path = note_relative_path(title, id);
    writes.push(gadgetron_knowledge::vault::VaultNoteWrite {
        relative_path: path.clone(),
        bytes,
    });
    objects.push(jobs::MaterializedObjectInput {
        id,
        path,
        content_hash: String::new(),
        expected_revision: None,
        originating_subject: originating_subject(properties)?,
    });
    Ok(())
}

fn originating_subject(
    properties: &BTreeMap<String, Value>,
) -> Result<Option<jobs::OriginatingSubject>, String> {
    properties
        .get("originating_subject")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("Invalid originating_subject metadata: {error}"))
}

fn revisioned_note_refs(value: &Value, field: &str) -> Result<Vec<(Uuid, i64)>, String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("Change-set field {field} is required"))?
        .iter()
        .map(|reference| {
            Ok((
                required_uuid(reference, "object_id")?,
                required_i64(reference, "expected_revision")?,
            ))
        })
        .collect()
}

fn deterministic_object_id(change_set_id: Uuid, position: usize) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(change_set_id.as_bytes());
    hasher.update(position.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn deterministic_split_object_id(
    change_set_id: Uuid,
    operation_position: usize,
    output_position: usize,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"split-note");
    hasher.update(change_set_id.as_bytes());
    hasher.update(operation_position.to_be_bytes());
    hasher.update(output_position.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn normalized_title(title: Option<&str>) -> Option<String> {
    let normalized = title?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn nonempty_equal(left: Option<&str>, right: Option<&str>) -> bool {
    matches!((left, right), (Some(left), Some(right)) if !left.is_empty() && left == right)
}

fn duplicate_match(
    left_hash: Option<&str>,
    left_title: Option<&str>,
    right_hash: Option<&str>,
    right_title: Option<&str>,
) -> bool {
    nonempty_equal(left_hash, right_hash)
        || normalized_title(left_title)
            .zip(normalized_title(right_title))
            .is_some_and(|(left, right)| left == right)
}

fn duplicate_root(parents: &mut [usize], index: usize) -> usize {
    if parents[index] != index {
        parents[index] = duplicate_root(parents, parents[index]);
    }
    parents[index]
}

fn union_duplicate_roots(parents: &mut [usize], left: usize, right: usize) {
    let left = duplicate_root(parents, left);
    let right = duplicate_root(parents, right);
    if left != right {
        parents[right] = left;
    }
}

fn snapshots_form_exact_group(snapshots: &[MergeNoteSnapshot]) -> bool {
    let mut reached = HashSet::from([0usize]);
    let mut changed = true;
    while changed {
        changed = false;
        for left in 0..snapshots.len() {
            if !reached.contains(&left) {
                continue;
            }
            for right in 0..snapshots.len() {
                if reached.contains(&right) {
                    continue;
                }
                if duplicate_match(
                    Some(&snapshots[left].content_hash),
                    Some(&snapshots[left].title),
                    Some(&snapshots[right].content_hash),
                    Some(&snapshots[right].title),
                ) {
                    reached.insert(right);
                    changed = true;
                }
            }
        }
    }
    reached.len() == snapshots.len()
}

fn valid_merge_field(field: &str) -> bool {
    field == "title"
        || (!field.is_empty()
            && field.len() <= 100
            && field != "body"
            && !field.starts_with('_')
            && !MERGE_SYSTEM_PROPERTIES.contains(&field))
}

async fn load_merge_note_snapshot(
    state: &AppState,
    actor: SpaceActor,
    space_id: Uuid,
    object_id: Uuid,
) -> Result<MergeNoteSnapshot, WorkbenchHttpError> {
    let location =
        sources::note_location(pool(state)?, actor, object_id, SpaceRole::Contributor, true)
            .await?;
    if location.space_id != space_id {
        return Err(WorkbenchHttpError::KnowledgeNotFound);
    }
    let layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.clone())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge Vault is unavailable".to_string(),
            ))
        })?;
    let tenant_id = actor.tenant_id;
    let home_bundle_id = location.home_bundle_id.clone();
    let path = location.path.clone();
    let expected_hash = location.content_hash.clone();
    let note = tokio::task::spawn_blocking(move || {
        layout.open_or_init(tenant_id)?.read_note_reconciled(
            space_id,
            &home_bundle_id,
            &path,
            expected_hash.as_deref(),
        )
    })
    .await
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?;
    if note.externally_changed {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let raw = String::from_utf8(note.bytes)
        .map_err(|_| knowledge_invalid("A selected note is not valid UTF-8"))?;
    let parsed = gadgetron_knowledge::source::parse_obsidian_note(&raw)
        .map_err(|error| knowledge_invalid(&format!("A selected note is invalid: {error}")))?;
    let title = parsed
        .properties
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&location.path)
        .to_string();
    Ok(MergeNoteSnapshot {
        object_id,
        vault_id: location.vault_id,
        revision: location.revision,
        content_hash: note.content_hash,
        title,
        properties: parsed.properties,
        body: parsed.body,
    })
}

async fn current_git_revision(
    state: &AppState,
    tenant_id: Uuid,
) -> Result<String, WorkbenchHttpError> {
    let layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.clone())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge Vault is unavailable".to_string(),
            ))
        })?;
    tokio::task::spawn_blocking(move || {
        layout
            .open_or_init(tenant_id)?
            .snapshot()
            .map(|snapshot| snapshot.git_head)
    })
    .await
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))?
    .map_err(|error| WorkbenchHttpError::Core(GadgetronError::Config(error.to_string())))
}

fn knowledge_invalid(detail: &str) -> WorkbenchHttpError {
    WorkbenchHttpError::KnowledgeInvalidInput {
        detail: detail.to_string(),
    }
}

fn clip_error(detail: &str) -> String {
    detail.chars().take(1_000).collect()
}

fn actor(ctx: &TenantContext) -> Result<SpaceActor, WorkbenchHttpError> {
    Ok(SpaceActor {
        tenant_id: ctx.tenant_id,
        user_id: ctx
            .actor_user_id
            .ok_or(WorkbenchHttpError::KnowledgeForbidden)?,
    })
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "Knowledge jobs require PostgreSQL".to_string(),
        ))
    })
}

fn runtime_snapshot(
    profile: ConversationAgentProfile,
    prompt_contract_revision: String,
    policy_revision: &str,
    role_profile_source: &str,
    role_profile_ref: String,
) -> RuntimeSnapshot {
    RuntimeSnapshot {
        backend: profile.backend.as_str().to_string(),
        model: profile.model,
        effort: profile.effort.as_str().to_string(),
        endpoint_id: profile.llm_endpoint_id,
        model_source: match profile.model_source {
            gadgetron_core::agent::config::ModelSource::Default => "default",
            gadgetron_core::agent::config::ModelSource::Local => "local",
        }
        .to_string(),
        local_base_url: profile.local_base_url,
        local_api_key_env: profile.local_api_key_env,
        prompt_contract_revision,
        tool_policy_revision: policy_revision.to_string(),
        role_profile_source: Some(role_profile_source.to_string()),
        role_profile_ref: Some(role_profile_ref),
    }
}

fn bundle_role_snapshot(contract: &BundleKnowledgeRoleExecutionContract) -> BundleRoleSnapshot {
    BundleRoleSnapshot {
        bundle_id: contract.bundle_id.clone(),
        bundle_role_id: contract.role.id.as_str().to_string(),
        package_manifest_sha256: contract.package_manifest_sha256.clone(),
        recipe_asset_id: contract.role.recipe_asset.as_str().to_string(),
        recipe_sha256: contract.recipe_sha256.clone(),
    }
}

fn bundle_execution_snapshot(
    contract: &BundleKnowledgeRoleExecutionContract,
    runtime: RuntimeSnapshot,
    max_wall_seconds: i32,
    followup: Option<Box<BundleExecutionSnapshot>>,
) -> BundleExecutionSnapshot {
    BundleExecutionSnapshot {
        bundle_role: bundle_role_snapshot(contract),
        prompt_contract_revision: contract.role.prompt_contract_revision.clone(),
        max_wall_seconds,
        runtime,
        recipe: contract.recipe.clone(),
        gadget_allowlist: contract
            .job
            .gadget_allowlist
            .iter()
            .map(|gadget| gadget.as_str().to_string())
            .collect(),
        followup,
    }
}

fn contract_max_wall_seconds(
    contract: &BundleKnowledgeRoleExecutionContract,
    global_limit: i32,
) -> i32 {
    let collection_limit = contract
        .collection
        .as_ref()
        .map(|collection| {
            i32::try_from(collection.budget.max_wall_seconds)
                .unwrap_or(3_600)
                .clamp(5, 3_600)
        })
        .unwrap_or(3_600);
    let job_limit = contract
        .job
        .budget
        .map(|budget| {
            i32::try_from(budget.max_wall_seconds)
                .unwrap_or(3_600)
                .clamp(5, 3_600)
        })
        .unwrap_or(3_600);
    global_limit
        .clamp(5, 3_600)
        .min(collection_limit)
        .min(job_limit)
}

struct KnowledgeJobIdentity<'a> {
    actor: SpaceActor,
    space_id: Uuid,
    role: KnowledgeJobRole,
    vault_id: Uuid,
    question: &'a str,
    source_ids: &'a [Uuid],
    outcome_ids: &'a [Uuid],
    collection_id: Option<Uuid>,
    collection_revision: Option<i64>,
    profile: &'a ConversationAgentProfile,
    policy_revision: &'a str,
    role_profile_ref: &'a str,
    bundle_execution: Option<&'a BundleExecutionSnapshot>,
    lesson_revision_target: Option<&'a LessonRevisionTarget>,
}

fn idempotency_key(identity: KnowledgeJobIdentity<'_>) -> String {
    let mut sources = identity.source_ids.to_vec();
    sources.sort_unstable();
    let mut outcomes = identity.outcome_ids.to_vec();
    outcomes.sort_unstable();
    let payload = serde_json::json!({
        "tenant_id": identity.actor.tenant_id,
        "requested_by": identity.actor.user_id,
        "space_id": identity.space_id,
        "vault_id": identity.vault_id,
        "role": identity.role.as_str(),
        "question": identity.question,
        "source_ids": sources,
        "outcome_ids": outcomes,
        "collection_id": identity.collection_id,
        "collection_revision": identity.collection_revision,
        "backend": identity.profile.backend.as_str(),
        "model": identity.profile.model,
        "effort": identity.profile.effort.as_str(),
        "endpoint_id": identity.profile.llm_endpoint_id,
        "policy_revision": identity.policy_revision,
        "role_profile_ref": identity.role_profile_ref,
        "lesson_revision_target": identity.lesson_revision_target,
        "bundle_execution": identity.bundle_execution,
    });
    let digest = Sha256::digest(serde_json::to_vec(&payload).unwrap_or_default());
    format!("knowledge-job:{}", hex::encode(digest))
}

fn role_profile_error(
    error: knowledge_agent_profiles::KnowledgeRoleProfileError,
) -> WorkbenchHttpError {
    match error {
        knowledge_agent_profiles::KnowledgeRoleProfileError::InvalidInput(detail)
        | knowledge_agent_profiles::KnowledgeRoleProfileError::InvalidPersisted(detail) => {
            WorkbenchHttpError::KnowledgeInvalidInput { detail }
        }
        knowledge_agent_profiles::KnowledgeRoleProfileError::Conflict => {
            WorkbenchHttpError::KnowledgeConflict
        }
        knowledge_agent_profiles::KnowledgeRoleProfileError::Database(error) => {
            WorkbenchHttpError::Core(GadgetronError::Config(format!(
                "Knowledge AI role profile: {error}"
            )))
        }
    }
}

impl From<KnowledgeJobError> for WorkbenchHttpError {
    fn from(error: KnowledgeJobError) -> Self {
        match error {
            KnowledgeJobError::InvalidInput(detail) => Self::KnowledgeInvalidInput { detail },
            KnowledgeJobError::NotFound => Self::KnowledgeNotFound,
            KnowledgeJobError::Conflict | KnowledgeJobError::LeaseLost => Self::KnowledgeConflict,
            KnowledgeJobError::Space(error) => error.into(),
            KnowledgeJobError::ServicePrincipal(_) => Self::KnowledgeForbidden,
            KnowledgeJobError::Database(error) => Self::Core(GadgetronError::Config(format!(
                "Knowledge job database: {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use uuid::Uuid;

    use gadgetron_xaas::knowledge_jobs::OriginatingSubject;

    use super::{apply_pinned_originating_subject, review_relation_targets};

    #[test]
    fn reviewed_relation_properties_accept_only_stable_note_ids() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let properties = BTreeMap::from([
            ("applies_to".to_string(), json!([first, second, first])),
            ("status".to_string(), json!("reviewed")),
        ]);
        let mut expected = vec![first, second];
        expected.sort_unstable();
        assert_eq!(review_relation_targets(&properties).unwrap(), expected);

        let invalid = BTreeMap::from([("bridge_to".to_string(), json!(["server-one"]))]);
        assert!(review_relation_targets(&invalid)
            .unwrap_err()
            .contains("invalid note UUID"));
    }

    #[test]
    fn originating_subject_is_reserved_for_the_core_pinned_event() {
        let pinned = OriginatingSubject {
            owner_bundle: "server-administrator".into(),
            subject_kind: "server-administrator.server-incident".into(),
            subject_id: "incident-1".into(),
            subject_revision: "revision-1".into(),
        };
        let mut properties = BTreeMap::new();
        apply_pinned_originating_subject(&mut properties, Some(&pinned)).unwrap();
        assert_eq!(
            properties["originating_subject"],
            serde_json::to_value(&pinned).unwrap()
        );

        let mut forged = BTreeMap::from([(
            "originating_subject".to_string(),
            json!({
                "owner_bundle": "other-bundle",
                "subject_kind": "other-bundle.other-subject",
                "subject_id": "other",
                "subject_revision": "other"
            }),
        )]);
        assert!(apply_pinned_originating_subject(&mut forged, None)
            .unwrap_err()
            .contains("reserved"));
        assert!(apply_pinned_originating_subject(&mut forged, Some(&pinned))
            .unwrap_err()
            .contains("does not match"));
    }
}
