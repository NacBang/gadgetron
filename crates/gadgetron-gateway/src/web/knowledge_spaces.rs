//! R2.1 Knowledge Space / Domain Vault HTTP surface.

use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
    Extension, Json, Router,
};
use gadgetron_core::{context::TenantContext, error::GadgetronError};
use gadgetron_knowledge::source::{parse_obsidian_note, serialize_obsidian_note};
use gadgetron_knowledge::vault::note_relative_path;
use gadgetron_xaas::knowledge_sources as sources;
use gadgetron_xaas::knowledge_spaces::{
    self as spaces, CreateProject, CreateShare, EnsureVault, KnowledgeSpaceError, PrincipalKind,
    RegisterObject, ShareMode, SpaceActor, SpaceRole, VaultOwnerState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{default_onboarding::apply_default_team_guide_titles, server::AppState};

use super::workbench::WorkbenchHttpError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/knowledge/spaces", get(list_spaces_handler))
        .route(
            "/knowledge/spaces/personal",
            post(ensure_personal_space_handler),
        )
        .route(
            "/knowledge/spaces/team/{team_id}",
            post(ensure_team_space_handler),
        )
        .route(
            "/knowledge/spaces/tenant-shared",
            post(ensure_tenant_shared_space_handler),
        )
        .route("/knowledge/projects", post(create_project_handler))
        .route(
            "/knowledge/projects/{project_id}/archive",
            post(archive_project_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/grants",
            post(upsert_grant_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/grants/{grant_id}",
            delete(revoke_grant_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/vaults",
            get(list_vaults_handler).post(ensure_vault_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/objects",
            get(list_objects_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/experience",
            get(list_experience_handler),
        )
        .route(
            "/knowledge/vaults/{vault_id}/objects",
            post(register_object_handler),
        )
        .route(
            "/knowledge/objects/{object_id}/shares",
            get(list_object_shares_handler).post(share_object_handler),
        )
        .route("/knowledge/shares/{share_id}", delete(revoke_share_handler))
        .route(
            "/knowledge/vault-owners/{bundle_id}",
            put(set_bundle_owner_state_handler),
        )
}

#[derive(Debug, Deserialize)]
pub struct TitleRequest {
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct GrantRequest {
    pub principal_kind: PrincipalKind,
    pub principal_id: String,
    pub role: SpaceRole,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct ExpectedRevision {
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize)]
pub struct OwnerStateRequest {
    pub state: VaultOwnerState,
}

#[derive(Debug, Serialize)]
pub struct SpaceListResponse {
    pub spaces: Vec<spaces::EffectiveSpaceRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct VaultProvisionResponse {
    pub vault: spaces::KnowledgeVaultRow,
    pub physical: gadgetron_knowledge::vault::DomainVaultPath,
}

#[derive(Debug, Serialize)]
pub struct VaultListResponse {
    pub vaults: Vec<spaces::KnowledgeVaultRow>,
    pub returned: usize,
}

#[derive(Debug, Deserialize)]
pub struct ObjectListQuery {
    pub home_bundle_id: Option<String>,
    pub canonical_kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ObjectListResponse {
    pub objects: Vec<spaces::KnowledgeObjectListRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct ShareListResponse {
    pub shares: Vec<spaces::KnowledgeShareRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct ExperienceListResponse {
    pub exchanges: Vec<super::intelligence_context::ContextExchangeSummary>,
    pub outcomes: Vec<super::intelligence_context::OutcomeFeedbackSummary>,
}

pub async fn list_spaces_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<SpaceListResponse>, WorkbenchHttpError> {
    let rows = spaces::effective_spaces(pool(&state)?, actor(&ctx)?).await?;
    let returned = rows.len();
    Ok(Json(SpaceListResponse {
        spaces: rows,
        returned,
    }))
}

pub async fn ensure_personal_space_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<TitleRequest>,
) -> Result<Json<spaces::KnowledgeSpaceRow>, WorkbenchHttpError> {
    Ok(Json(
        spaces::ensure_personal_space(pool(&state)?, actor(&ctx)?, &request.title).await?,
    ))
}

pub async fn ensure_team_space_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(team_id): Path<String>,
    Json(request): Json<TitleRequest>,
) -> Result<Json<spaces::KnowledgeSpaceRow>, WorkbenchHttpError> {
    Ok(Json(
        spaces::ensure_team_space(pool(&state)?, actor(&ctx)?, &team_id, &request.title).await?,
    ))
}

pub async fn ensure_tenant_shared_space_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<TitleRequest>,
) -> Result<Json<spaces::KnowledgeSpaceRow>, WorkbenchHttpError> {
    Ok(Json(
        spaces::ensure_tenant_shared_space(pool(&state)?, actor(&ctx)?, &request.title).await?,
    ))
}

pub async fn create_project_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(request): Json<CreateProject>,
) -> Result<Json<spaces::ProjectWithSpace>, WorkbenchHttpError> {
    Ok(Json(
        spaces::create_project(pool(&state)?, actor(&ctx)?, request).await?,
    ))
}

pub async fn archive_project_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(project_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<spaces::ProjectWithSpace>, WorkbenchHttpError> {
    Ok(Json(
        spaces::archive_project(
            pool(&state)?,
            actor(&ctx)?,
            project_id,
            request.expected_revision,
        )
        .await?,
    ))
}

pub async fn upsert_grant_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Json(request): Json<GrantRequest>,
) -> Result<Json<spaces::KnowledgeSpaceGrantRow>, WorkbenchHttpError> {
    Ok(Json(
        spaces::upsert_grant(
            pool(&state)?,
            actor(&ctx)?,
            space_id,
            request.principal_kind,
            &request.principal_id,
            request.role,
            request.expires_at,
        )
        .await?,
    ))
}

pub async fn revoke_grant_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path((space_id, grant_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<spaces::KnowledgeSpaceGrantRow>, WorkbenchHttpError> {
    Ok(Json(
        spaces::revoke_grant(
            pool(&state)?,
            actor(&ctx)?,
            space_id,
            grant_id,
            request.expected_revision,
        )
        .await?,
    ))
}

pub async fn ensure_vault_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Json(request): Json<EnsureVault>,
) -> Result<Json<VaultProvisionResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let vault = spaces::ensure_vault(pool(&state)?, actor, space_id, request).await?;
    let layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.as_ref())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Domain Vault layout requires [knowledge] configuration".to_string(),
            ))
        })?;
    let repository = layout.open_or_init(ctx.tenant_id).map_err(vault_error)?;
    let physical = repository
        .ensure_domain(space_id, &vault.home_bundle_id)
        .map_err(vault_error)?;
    Ok(Json(VaultProvisionResponse { vault, physical }))
}

pub async fn list_vaults_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
) -> Result<Json<VaultListResponse>, WorkbenchHttpError> {
    let vaults = spaces::list_vaults(pool(&state)?, actor(&ctx)?, space_id).await?;
    let returned = vaults.len();
    Ok(Json(VaultListResponse { vaults, returned }))
}

pub async fn list_objects_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
    Query(query): Query<ObjectListQuery>,
) -> Result<Json<ObjectListResponse>, WorkbenchHttpError> {
    let mut objects = spaces::list_objects(
        pool(&state)?,
        actor(&ctx)?,
        space_id,
        query.home_bundle_id.as_deref(),
        query.canonical_kind.as_deref(),
    )
    .await?;
    apply_default_team_guide_titles(&mut objects);
    let returned = objects.len();
    Ok(Json(ObjectListResponse { objects, returned }))
}

pub async fn list_experience_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
) -> Result<Json<ExperienceListResponse>, WorkbenchHttpError> {
    let vault_layout = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.clone())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge Experience requires [knowledge] configuration".to_string(),
            ))
        })?;
    let service = super::intelligence_context::IntelligenceContextService::new(
        pool(&state)?.clone(),
        vault_layout,
    );
    let binding = super::intelligence_context::IntelligenceActorBinding {
        tenant_id: ctx.tenant_id,
        actor_id: actor(&ctx)?.user_id,
        authority_actor_id: None,
        acting_space_id: Some(space_id),
    };
    let exchanges = service
        .list_exchanges(binding, space_id, 100)
        .await
        .map_err(map_intelligence_error)?;
    let outcomes = service
        .list_feedback(binding, space_id, 100)
        .await
        .map_err(map_intelligence_error)?;
    Ok(Json(ExperienceListResponse {
        exchanges,
        outcomes,
    }))
}

pub async fn register_object_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(vault_id): Path<Uuid>,
    Json(request): Json<RegisterObject>,
) -> Result<Json<spaces::KnowledgeObjectRow>, WorkbenchHttpError> {
    Ok(Json(
        spaces::register_object(pool(&state)?, actor(&ctx)?, vault_id, request).await?,
    ))
}

pub async fn share_object_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(object_id): Path<Uuid>,
    Json(request): Json<CreateShare>,
) -> Result<Json<spaces::KnowledgeShareRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let share = if request.mode == ShareMode::Reference {
        spaces::share_object(pool(&state)?, actor, object_id, request).await?
    } else {
        materialize_share(&state, actor, object_id, request).await?
    };
    super::knowledge_graph::reconcile_after_change(&state, actor).await;
    Ok(Json(share))
}

pub async fn list_object_shares_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(object_id): Path<Uuid>,
) -> Result<Json<ShareListResponse>, WorkbenchHttpError> {
    let shares = spaces::list_object_shares(pool(&state)?, actor(&ctx)?, object_id).await?;
    let returned = shares.len();
    Ok(Json(ShareListResponse { shares, returned }))
}

pub async fn revoke_share_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(share_id): Path<Uuid>,
    Json(request): Json<ExpectedRevision>,
) -> Result<Json<spaces::KnowledgeShareRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let share =
        spaces::revoke_share(pool(&state)?, actor, share_id, request.expected_revision).await?;
    super::knowledge_graph::reconcile_after_change(&state, actor).await;
    Ok(Json(share))
}

async fn materialize_share(
    state: &AppState,
    actor: SpaceActor,
    source_object_id: Uuid,
    request: CreateShare,
) -> Result<spaces::KnowledgeShareRow, WorkbenchHttpError> {
    let source = sources::note_location(
        pool(state)?,
        actor,
        source_object_id,
        SpaceRole::Manager,
        true,
    )
    .await?;
    if source.revision != request.source_revision {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let visible_spaces = spaces::effective_spaces(pool(state)?, actor).await?;
    let target_space = visible_spaces
        .iter()
        .find(|row| row.space.id == request.target_space_id)
        .ok_or(WorkbenchHttpError::KnowledgeNotFound)?;
    if matches!(request.mode, ShareMode::Promote | ShareMode::Synthesize)
        && !matches!(target_space.space.kind.as_str(), "team" | "tenant_shared")
    {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "promotion target must be a Team or Tenant Shared Space".to_string(),
        });
    }

    let source_vault = spaces::list_vaults(pool(state)?, actor, source.space_id)
        .await?
        .into_iter()
        .find(|vault| vault.id == source.vault_id)
        .ok_or(WorkbenchHttpError::KnowledgeNotFound)?;
    let (target_bundle, target_schema, target_schema_version) =
        if matches!(request.mode, ShareMode::Promote | ShareMode::Synthesize) {
            ("core".to_string(), "core.knowledge".to_string(), 1)
        } else {
            (
                source.home_bundle_id.clone(),
                source_vault.knowledge_schema_id,
                source_vault.schema_version,
            )
        };
    let target_vault = match spaces::list_vaults(pool(state)?, actor, request.target_space_id)
        .await?
        .into_iter()
        .find(|vault| vault.home_bundle_id == target_bundle)
    {
        Some(vault) => vault,
        None => {
            spaces::ensure_vault(
                pool(state)?,
                actor,
                request.target_space_id,
                EnsureVault {
                    home_bundle_id: target_bundle.clone(),
                    knowledge_schema_id: target_schema,
                    schema_version: target_schema_version,
                },
            )
            .await?
        }
    };

    let repository = state
        .workbench
        .as_ref()
        .and_then(|workbench| workbench.vault_layout.as_ref())
        .ok_or_else(|| {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge share materialization requires [knowledge] configuration".to_string(),
            ))
        })?
        .open_or_init(actor.tenant_id)
        .map_err(vault_error)?;
    let lock = repository
        .acquire_lock(Duration::from_secs(5))
        .map_err(vault_error)?;
    let source_note = repository
        .read_note_reconciled_locked(
            &lock,
            source.space_id,
            &source.home_bundle_id,
            &source.path,
            source.content_hash.as_deref(),
        )
        .map_err(vault_error)?;
    drop(lock);
    let source_raw = String::from_utf8(source_note.bytes).map_err(|_| {
        WorkbenchHttpError::KnowledgeInvalidInput {
            detail: "source note is not valid UTF-8".to_string(),
        }
    })?;
    let parsed = parse_obsidian_note(&source_raw).map_err(|error| {
        WorkbenchHttpError::KnowledgeInvalidInput {
            detail: format!("source note frontmatter is invalid: {error}"),
        }
    })?;
    let target_object_id = Uuid::new_v4();
    let target_title = parsed
        .properties
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Shared knowledge");
    let target_path = note_relative_path(target_title, target_object_id);
    let now = chrono::Utc::now().to_rfc3339();
    let mut properties = parsed.properties;
    properties.insert("id".to_string(), serde_json::json!(target_object_id));
    properties.insert(
        "space_id".to_string(),
        serde_json::json!(request.target_space_id),
    );
    properties.insert(
        "home_bundle_id".to_string(),
        serde_json::json!(target_bundle.clone()),
    );
    properties.insert("kind".to_string(), serde_json::json!("note"));
    properties.insert(
        "share_mode".to_string(),
        serde_json::json!(request.mode.as_str()),
    );
    properties.insert(
        "derived_from".to_string(),
        serde_json::json!([source_object_id]),
    );
    properties.insert(
        "source_space_id".to_string(),
        serde_json::json!(source.space_id),
    );
    properties.insert(
        "source_revision".to_string(),
        serde_json::json!(source.revision),
    );
    properties.insert("created".to_string(), serde_json::json!(now.clone()));
    properties.insert("updated".to_string(), serde_json::json!(now));
    let raw = serialize_obsidian_note(&properties, &parsed.body).map_err(|error| {
        WorkbenchHttpError::KnowledgeInvalidInput {
            detail: format!("shared note serialization failed: {error}"),
        }
    })?;
    if let Some(secret) = gadgetron_knowledge::wiki::secrets::check_block_patterns(&raw).first() {
        return Err(WorkbenchHttpError::KnowledgeInvalidInput {
            detail: format!(
                "shared note contains blocked credential pattern {}",
                secret.pattern_name
            ),
        });
    }
    let content_hash = hex::encode(Sha256::digest(raw.as_bytes()));
    let (target_object, _) = sources::register_manual_note(
        pool(state)?,
        actor,
        target_vault.id,
        target_object_id,
        &target_path,
        Some(&content_hash),
    )
    .await?;
    if let Err(error) = repository
        .ensure_domain(request.target_space_id, &target_bundle)
        .and_then(|_| {
            repository.write_note(
                request.target_space_id,
                &target_bundle,
                &target_path,
                raw.as_bytes(),
                "vault: materialize knowledge share",
            )
        })
    {
        let _ = sources::delete_note(
            pool(state)?,
            actor,
            target_object_id,
            target_object.revision,
        )
        .await;
        return Err(vault_error(error));
    }
    match spaces::share_object_with_target(
        pool(state)?,
        actor,
        source_object_id,
        request,
        Some(target_object_id),
    )
    .await
    {
        Ok(share) => Ok(share),
        Err(error) => {
            if let Ok(cleanup_lock) = repository.acquire_lock(Duration::from_secs(5)) {
                let _ = repository.archive_note_locked(
                    &cleanup_lock,
                    target_vault.space_id,
                    &target_bundle,
                    &target_path,
                );
            }
            let _ = sources::delete_note(
                pool(state)?,
                actor,
                target_object_id,
                target_object.revision,
            )
            .await;
            Err(error.into())
        }
    }
}

pub async fn set_bundle_owner_state_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(bundle_id): Path<String>,
    Json(request): Json<OwnerStateRequest>,
) -> Result<Json<Vec<spaces::KnowledgeVaultRow>>, WorkbenchHttpError> {
    Ok(Json(
        spaces::set_bundle_owner_state(pool(&state)?, actor(&ctx)?, &bundle_id, request.state)
            .await?,
    ))
}

fn actor(ctx: &TenantContext) -> Result<SpaceActor, WorkbenchHttpError> {
    let user_id = ctx
        .actor_user_id
        .ok_or(WorkbenchHttpError::KnowledgeForbidden)?;
    Ok(SpaceActor {
        tenant_id: ctx.tenant_id,
        user_id,
    })
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "Knowledge Spaces require PostgreSQL".to_string(),
        ))
    })
}

fn vault_error(error: gadgetron_knowledge::vault::VaultLayoutError) -> WorkbenchHttpError {
    WorkbenchHttpError::Core(GadgetronError::Config(format!(
        "Domain Vault filesystem reconciliation failed: {error}"
    )))
}

fn map_intelligence_error(
    error: super::intelligence_context::IntelligenceContextError,
) -> WorkbenchHttpError {
    use super::intelligence_context::IntelligenceContextError;
    match error {
        IntelligenceContextError::Invalid(detail) => {
            WorkbenchHttpError::KnowledgeInvalidInput { detail }
        }
        IntelligenceContextError::Forbidden => WorkbenchHttpError::KnowledgeForbidden,
        IntelligenceContextError::Conflict => WorkbenchHttpError::KnowledgeConflict,
        IntelligenceContextError::Unavailable | IntelligenceContextError::Persistence => {
            WorkbenchHttpError::Core(GadgetronError::Config(
                "Knowledge Experience is temporarily unavailable".to_string(),
            ))
        }
    }
}

impl From<KnowledgeSpaceError> for WorkbenchHttpError {
    fn from(error: KnowledgeSpaceError) -> Self {
        match error {
            KnowledgeSpaceError::InvalidInput(detail) => Self::KnowledgeInvalidInput { detail },
            KnowledgeSpaceError::NotFound => Self::KnowledgeNotFound,
            KnowledgeSpaceError::Forbidden => Self::KnowledgeForbidden,
            KnowledgeSpaceError::RevisionConflict | KnowledgeSpaceError::Conflict => {
                Self::KnowledgeConflict
            }
            KnowledgeSpaceError::Database(error) => Self::Core(GadgetronError::Config(format!(
                "Knowledge Space database: {error}"
            ))),
        }
    }
}
