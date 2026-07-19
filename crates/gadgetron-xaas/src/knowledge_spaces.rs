//! PostgreSQL registry and ACL resolver for R2.1 Knowledge Spaces.
//!
//! Canonical Markdown bytes live in `gadgetron-knowledge::vault`; this module
//! owns tenant-pinned identity, permissions, lifecycle and monotone revisions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceActor {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceRole {
    Viewer,
    Contributor,
    Curator,
    Manager,
}

impl SpaceRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Contributor => "contributor",
            Self::Curator => "curator",
            Self::Manager => "manager",
        }
    }

    const fn weight(self) -> u8 {
        match self {
            Self::Viewer => 1,
            Self::Contributor => 2,
            Self::Curator => 3,
            Self::Manager => 4,
        }
    }

    fn parse(value: &str) -> Result<Self, KnowledgeSpaceError> {
        match value {
            "viewer" => Ok(Self::Viewer),
            "contributor" => Ok(Self::Contributor),
            "curator" => Ok(Self::Curator),
            "manager" => Ok(Self::Manager),
            _ => Err(KnowledgeSpaceError::InvalidInput(format!(
                "unknown Space role {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    User,
    Team,
    Group,
}

impl PrincipalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Team => "team",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareMode {
    Reference,
    Snapshot,
    Fork,
    Promote,
    Synthesize,
}

impl ShareMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Snapshot => "snapshot",
            Self::Fork => "fork",
            Self::Promote => "promote",
            Self::Synthesize => "synthesize",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultOwnerState {
    Enabled,
    Degraded,
    OwnerUnavailable,
}

impl VaultOwnerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Degraded => "degraded",
            Self::OwnerUnavailable => "owner_unavailable",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeSpaceError {
    #[error("invalid Knowledge Space input: {0}")]
    InvalidInput(String),
    #[error("Knowledge Space resource not found")]
    NotFound,
    #[error("Knowledge Space access denied")]
    Forbidden,
    #[error("Knowledge Space revision conflict")]
    RevisionConflict,
    #[error("Knowledge Space state already exists")]
    Conflict,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProjectRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub slug: String,
    pub title: String,
    pub goal: String,
    pub status: String,
    pub owner_user_id: Uuid,
    pub policy: serde_json::Value,
    pub policy_revision: i64,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeSpaceRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub kind: String,
    pub title: String,
    pub owner_user_id: Option<Uuid>,
    pub owner_team_id: Option<String>,
    pub owner_project_id: Option<Uuid>,
    pub status: String,
    pub policy: serde_json::Value,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveSpaceRow {
    #[serde(flatten)]
    pub space: KnowledgeSpaceRow,
    pub effective_role: SpaceRole,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeSpaceGrantRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub space_id: Uuid,
    pub principal_kind: String,
    pub principal_id: String,
    pub role: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeVaultRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub space_id: Uuid,
    pub home_bundle_id: String,
    pub knowledge_schema_id: String,
    pub schema_version: i32,
    pub owner_state: String,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeObjectRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub vault_id: Uuid,
    pub canonical_kind: String,
    pub path: String,
    pub status: String,
    pub content_hash: Option<String>,
    pub created_by: Uuid,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeObjectListRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub vault_id: Uuid,
    pub source_id: Option<Uuid>,
    pub canonical_kind: String,
    pub path: String,
    pub status: String,
    pub content_hash: Option<String>,
    pub created_by: Uuid,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub space_id: Uuid,
    pub home_bundle_id: String,
    pub owner_state: String,
    pub title: Option<String>,
    pub knowledge_kind: String,
    pub freshness: String,
    pub review_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeShareRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub source_space_id: Uuid,
    pub source_object_id: Uuid,
    pub source_revision: i64,
    pub target_space_id: Uuid,
    pub mode: String,
    pub follow_latest: bool,
    pub target_object_id: Option<Uuid>,
    pub policy_disposition: String,
    pub created_by: Uuid,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProject {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default = "empty_object")]
    pub policy: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectWithSpace {
    pub project: ProjectRow,
    pub space: KnowledgeSpaceRow,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnsureVault {
    pub home_bundle_id: String,
    #[serde(default = "default_schema_id")]
    pub knowledge_schema_id: String,
    #[serde(default = "default_schema_version")]
    pub schema_version: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterObject {
    pub canonical_kind: String,
    pub path: String,
    #[serde(default)]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateShare {
    pub target_space_id: Uuid,
    pub source_revision: i64,
    pub mode: ShareMode,
    #[serde(default)]
    pub follow_latest: bool,
    #[serde(default = "default_policy_disposition")]
    pub policy_disposition: String,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}
fn default_schema_id() -> String {
    "core.knowledge".to_string()
}
const fn default_schema_version() -> i32 {
    1
}
fn default_policy_disposition() -> String {
    "allowed".to_string()
}

pub async fn create_project(
    pool: &PgPool,
    actor: SpaceActor,
    request: CreateProject,
) -> Result<ProjectWithSpace, KnowledgeSpaceError> {
    ensure_active_actor(pool, actor).await?;
    let mut tx = pool.begin().await?;
    let project = sqlx::query_as::<_, ProjectRow>(
        r#"INSERT INTO projects
           (tenant_id, slug, title, goal, owner_user_id, policy)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, tenant_id, slug, title, goal, status, owner_user_id,
                     policy, policy_revision, revision, created_at, updated_at"#,
    )
    .bind(actor.tenant_id)
    .bind(request.slug)
    .bind(request.title)
    .bind(request.goal)
    .bind(actor.user_id)
    .bind(request.policy)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_constraint)?;
    let space = insert_space(
        &mut tx,
        actor.tenant_id,
        "project",
        &project.title,
        None,
        None,
        Some(project.id),
    )
    .await?;
    tx.commit().await?;
    Ok(ProjectWithSpace { project, space })
}

pub async fn ensure_personal_space(
    pool: &PgPool,
    actor: SpaceActor,
    title: &str,
) -> Result<KnowledgeSpaceRow, KnowledgeSpaceError> {
    ensure_active_actor(pool, actor).await?;
    if let Some(row) = find_owned_space(pool, actor.tenant_id, "personal", actor.user_id).await? {
        return Ok(row);
    }
    let mut tx = pool.begin().await?;
    let result = insert_space(
        &mut tx,
        actor.tenant_id,
        "personal",
        title,
        Some(actor.user_id),
        None,
        None,
    )
    .await;
    match result {
        Ok(row) => {
            tx.commit().await?;
            Ok(row)
        }
        Err(KnowledgeSpaceError::Conflict) => {
            tx.rollback().await?;
            find_owned_space(pool, actor.tenant_id, "personal", actor.user_id)
                .await?
                .ok_or(KnowledgeSpaceError::Conflict)
        }
        Err(error) => Err(error),
    }
}

pub async fn ensure_team_space(
    pool: &PgPool,
    actor: SpaceActor,
    team_id: &str,
    title: &str,
) -> Result<KnowledgeSpaceRow, KnowledgeSpaceError> {
    let actor_role = ensure_active_actor(pool, actor).await?;
    let team_role: Option<String> = sqlx::query_scalar(
        r#"SELECT tm.role FROM team_members tm
           JOIN teams t ON t.id = tm.team_id
           WHERE t.tenant_id = $1 AND tm.team_id = $2 AND tm.user_id = $3"#,
    )
    .bind(actor.tenant_id)
    .bind(team_id)
    .bind(actor.user_id)
    .fetch_optional(pool)
    .await?;
    if actor_role != "admin" && team_role.as_deref() != Some("lead") {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    if let Some(row) = sqlx::query_as::<_, KnowledgeSpaceRow>(&format!(
        "{} WHERE tenant_id = $1 AND kind = 'team' AND owner_team_id = $2",
        space_select()
    ))
    .bind(actor.tenant_id)
    .bind(team_id)
    .fetch_optional(pool)
    .await?
    {
        return Ok(row);
    }
    let mut tx = pool.begin().await?;
    let row = insert_space(
        &mut tx,
        actor.tenant_id,
        "team",
        title,
        None,
        Some(team_id),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn ensure_tenant_shared_space(
    pool: &PgPool,
    actor: SpaceActor,
    title: &str,
) -> Result<KnowledgeSpaceRow, KnowledgeSpaceError> {
    if ensure_active_actor(pool, actor).await? != "admin" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    if let Some(row) = sqlx::query_as::<_, KnowledgeSpaceRow>(&format!(
        "{} WHERE tenant_id = $1 AND kind = 'tenant_shared'",
        space_select()
    ))
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    {
        return Ok(row);
    }
    let mut tx = pool.begin().await?;
    let row = insert_space(
        &mut tx,
        actor.tenant_id,
        "tenant_shared",
        title,
        None,
        None,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn effective_spaces(
    pool: &PgPool,
    actor: SpaceActor,
) -> Result<Vec<EffectiveSpaceRow>, KnowledgeSpaceError> {
    ensure_active_actor(pool, actor).await?;
    let spaces = sqlx::query_as::<_, KnowledgeSpaceRow>(&format!(
        "{} WHERE tenant_id = $1 ORDER BY kind, title, id",
        space_select()
    ))
    .bind(actor.tenant_id)
    .fetch_all(pool)
    .await?;
    let mut visible = Vec::new();
    for space in spaces {
        if let Some(role) = effective_role(pool, actor, &space).await? {
            visible.push(EffectiveSpaceRow {
                space,
                effective_role: role,
            });
        }
    }
    Ok(visible)
}

pub async fn upsert_grant(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    principal_kind: PrincipalKind,
    principal_id: &str,
    role: SpaceRole,
    expires_at: Option<DateTime<Utc>>,
) -> Result<KnowledgeSpaceGrantRow, KnowledgeSpaceError> {
    require_role(pool, actor, space_id, SpaceRole::Manager, true).await?;
    validate_principal(pool, actor.tenant_id, principal_kind, principal_id).await?;
    let row = sqlx::query_as::<_, KnowledgeSpaceGrantRow>(
        r#"INSERT INTO knowledge_space_grants
           (tenant_id, space_id, principal_kind, principal_id, role, expires_at, created_by)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (space_id, principal_kind, principal_id) DO UPDATE SET
             role = EXCLUDED.role,
             expires_at = EXCLUDED.expires_at,
             revoked_at = NULL,
             revision = knowledge_space_grants.revision + 1,
             updated_at = NOW(),
             created_by = EXCLUDED.created_by
           RETURNING id, tenant_id, space_id, principal_kind, principal_id, role,
                     expires_at, created_by, revision, created_at, updated_at, revoked_at"#,
    )
    .bind(actor.tenant_id)
    .bind(space_id)
    .bind(principal_kind.as_str())
    .bind(principal_id)
    .bind(role.as_str())
    .bind(expires_at)
    .bind(actor.user_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn revoke_grant(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    grant_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeSpaceGrantRow, KnowledgeSpaceError> {
    require_role(pool, actor, space_id, SpaceRole::Manager, true).await?;
    let row = sqlx::query_as::<_, KnowledgeSpaceGrantRow>(
        r#"UPDATE knowledge_space_grants SET
             revoked_at = NOW(), revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND space_id = $3
             AND revision = $4 AND revoked_at IS NULL
           RETURNING id, tenant_id, space_id, principal_kind, principal_id, role,
                     expires_at, created_by, revision, created_at, updated_at, revoked_at"#,
    )
    .bind(grant_id)
    .bind(actor.tenant_id)
    .bind(space_id)
    .bind(expected_revision)
    .fetch_optional(pool)
    .await?;
    resolve_cas(
        pool,
        "knowledge_space_grants",
        grant_id,
        actor.tenant_id,
        row,
    )
    .await
}

pub async fn ensure_vault(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    request: EnsureVault,
) -> Result<KnowledgeVaultRow, KnowledgeSpaceError> {
    require_role(pool, actor, space_id, SpaceRole::Manager, true).await?;
    if request.schema_version <= 0 || request.knowledge_schema_id.trim().is_empty() {
        return Err(KnowledgeSpaceError::InvalidInput(
            "schema id/version must be non-empty and positive".to_string(),
        ));
    }
    let row = sqlx::query_as::<_, KnowledgeVaultRow>(
        r#"INSERT INTO knowledge_vaults
           (tenant_id, space_id, home_bundle_id, knowledge_schema_id, schema_version)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (tenant_id, space_id, home_bundle_id) DO UPDATE SET
             knowledge_schema_id = EXCLUDED.knowledge_schema_id,
             schema_version = EXCLUDED.schema_version,
             revision = CASE WHEN
               knowledge_vaults.knowledge_schema_id IS DISTINCT FROM EXCLUDED.knowledge_schema_id
               OR knowledge_vaults.schema_version IS DISTINCT FROM EXCLUDED.schema_version
             THEN knowledge_vaults.revision + 1 ELSE knowledge_vaults.revision END,
             updated_at = CASE WHEN
               knowledge_vaults.knowledge_schema_id IS DISTINCT FROM EXCLUDED.knowledge_schema_id
               OR knowledge_vaults.schema_version IS DISTINCT FROM EXCLUDED.schema_version
             THEN NOW() ELSE knowledge_vaults.updated_at END
           WHERE knowledge_vaults.owner_state = 'enabled'
           RETURNING id, tenant_id, space_id, home_bundle_id, knowledge_schema_id,
                     schema_version, owner_state, revision, created_at, updated_at"#,
    )
    .bind(actor.tenant_id)
    .bind(space_id)
    .bind(request.home_bundle_id)
    .bind(request.knowledge_schema_id)
    .bind(request.schema_version)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)?;
    Ok(row)
}

/// Internal service-actor path for a signed Bundle's own Domain Vault.
///
/// Both the service actor and the inherited on-behalf actor must be active
/// contributors in the exact Space. The gateway supplies `home_bundle_id`
/// only from a verified `KnowledgeEventDescriptor`; this function does not
/// permit a service actor to create arbitrary Spaces or cross-domain Vaults.
pub async fn ensure_service_bundle_vault(
    pool: &PgPool,
    service_actor: SpaceActor,
    on_behalf_actor: SpaceActor,
    space_id: Uuid,
    request: EnsureVault,
) -> Result<KnowledgeVaultRow, KnowledgeSpaceError> {
    if service_actor.tenant_id != on_behalf_actor.tenant_id {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    require_role(pool, service_actor, space_id, SpaceRole::Contributor, true).await?;
    require_role(
        pool,
        on_behalf_actor,
        space_id,
        SpaceRole::Contributor,
        true,
    )
    .await?;
    if request.schema_version <= 0 || request.knowledge_schema_id.trim().is_empty() {
        return Err(KnowledgeSpaceError::InvalidInput(
            "schema id/version must be non-empty and positive".to_string(),
        ));
    }
    sqlx::query_as::<_, KnowledgeVaultRow>(
        r#"INSERT INTO knowledge_vaults
           (tenant_id, space_id, home_bundle_id, knowledge_schema_id, schema_version)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (tenant_id, space_id, home_bundle_id) DO UPDATE SET
             knowledge_schema_id = EXCLUDED.knowledge_schema_id,
             schema_version = EXCLUDED.schema_version,
             revision = CASE WHEN
               knowledge_vaults.knowledge_schema_id IS DISTINCT FROM EXCLUDED.knowledge_schema_id
               OR knowledge_vaults.schema_version IS DISTINCT FROM EXCLUDED.schema_version
             THEN knowledge_vaults.revision + 1 ELSE knowledge_vaults.revision END,
             updated_at = CASE WHEN
               knowledge_vaults.knowledge_schema_id IS DISTINCT FROM EXCLUDED.knowledge_schema_id
               OR knowledge_vaults.schema_version IS DISTINCT FROM EXCLUDED.schema_version
             THEN NOW() ELSE knowledge_vaults.updated_at END
           RETURNING id, tenant_id, space_id, home_bundle_id, knowledge_schema_id,
                     schema_version, owner_state, revision, created_at, updated_at"#,
    )
    .bind(service_actor.tenant_id)
    .bind(space_id)
    .bind(request.home_bundle_id)
    .bind(request.knowledge_schema_id)
    .bind(request.schema_version)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)
}

pub async fn list_vaults(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
) -> Result<Vec<KnowledgeVaultRow>, KnowledgeSpaceError> {
    require_role(pool, actor, space_id, SpaceRole::Viewer, false).await?;
    Ok(sqlx::query_as::<_, KnowledgeVaultRow>(
        r#"SELECT id, tenant_id, space_id, home_bundle_id, knowledge_schema_id,
                  schema_version, owner_state, revision, created_at, updated_at
           FROM knowledge_vaults
           WHERE tenant_id = $1 AND space_id = $2
           ORDER BY home_bundle_id"#,
    )
    .bind(actor.tenant_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_objects(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    home_bundle_id: Option<&str>,
    canonical_kind: Option<&str>,
) -> Result<Vec<KnowledgeObjectListRow>, KnowledgeSpaceError> {
    require_role(pool, actor, space_id, SpaceRole::Viewer, false).await?;
    Ok(sqlx::query_as::<_, KnowledgeObjectListRow>(
        r#"SELECT o.id, o.tenant_id, o.vault_id, o.source_id, o.canonical_kind,
                  o.path, o.status, o.content_hash, o.created_by, o.revision,
                  o.created_at, o.updated_at, v.space_id, v.home_bundle_id,
                  v.owner_state, n.title,
                  COALESCE(n.node_kind, 'note') AS knowledge_kind,
                  COALESCE(n.freshness, 'unknown') AS freshness,
                  n.metadata ->> 'review_state' AS review_state
           FROM knowledge_objects o
           JOIN knowledge_vaults v
             ON v.tenant_id = o.tenant_id AND v.id = o.vault_id
           LEFT JOIN knowledge_graph_generations g
             ON g.tenant_id = o.tenant_id AND g.state = 'active'
           LEFT JOIN knowledge_graph_nodes n
             ON n.tenant_id = o.tenant_id AND n.generation_id = g.id
            AND n.stable_node_id = 'note:' || o.id::TEXT
            AND n.status <> 'tombstone'
           WHERE o.tenant_id = $1 AND v.space_id = $2
             AND o.status <> 'tombstone'
             AND ($3::TEXT IS NULL OR v.home_bundle_id = $3)
             AND ($4::TEXT IS NULL OR o.canonical_kind = $4)
           ORDER BY o.updated_at DESC, o.id"#,
    )
    .bind(actor.tenant_id)
    .bind(space_id)
    .bind(home_bundle_id)
    .bind(canonical_kind)
    .fetch_all(pool)
    .await?)
}

pub async fn register_object(
    pool: &PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
    request: RegisterObject,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    gadgetron_core::knowledge::KnowledgePath::new(&request.path).map_err(|error| {
        KnowledgeSpaceError::InvalidInput(format!("invalid knowledge object path: {error}"))
    })?;
    let vault: KnowledgeVaultRow = sqlx::query_as(
        r#"SELECT id, tenant_id, space_id, home_bundle_id, knowledge_schema_id,
                  schema_version, owner_state, revision, created_at, updated_at
           FROM knowledge_vaults WHERE id = $1 AND tenant_id = $2"#,
    )
    .bind(vault_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    require_role(pool, actor, vault.space_id, SpaceRole::Contributor, true).await?;
    if vault.owner_state != "enabled" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    let row = sqlx::query_as::<_, KnowledgeObjectRow>(
        r#"INSERT INTO knowledge_objects
           (tenant_id, vault_id, canonical_kind, path, content_hash, created_by)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, tenant_id, vault_id, canonical_kind, path, status,
                     content_hash, created_by, revision, created_at, updated_at"#,
    )
    .bind(actor.tenant_id)
    .bind(vault_id)
    .bind(request.canonical_kind)
    .bind(request.path)
    .bind(request.content_hash)
    .bind(actor.user_id)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)?;
    Ok(row)
}

pub async fn share_object(
    pool: &PgPool,
    actor: SpaceActor,
    object_id: Uuid,
    request: CreateShare,
) -> Result<KnowledgeShareRow, KnowledgeSpaceError> {
    share_object_with_target(pool, actor, object_id, request, None).await
}

pub async fn share_object_with_target(
    pool: &PgPool,
    actor: SpaceActor,
    object_id: Uuid,
    request: CreateShare,
    target_object_id: Option<Uuid>,
) -> Result<KnowledgeShareRow, KnowledgeSpaceError> {
    let source: (Uuid, i64) = sqlx::query_as(
        r#"SELECT v.space_id, o.revision
           FROM knowledge_objects o
           JOIN knowledge_vaults v ON v.id = o.vault_id AND v.tenant_id = o.tenant_id
           WHERE o.id = $1 AND o.tenant_id = $2 AND o.status = 'active'"#,
    )
    .bind(object_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    if source.1 != request.source_revision {
        return Err(KnowledgeSpaceError::RevisionConflict);
    }
    require_role(pool, actor, source.0, SpaceRole::Manager, true).await?;
    require_role(
        pool,
        actor,
        request.target_space_id,
        SpaceRole::Contributor,
        true,
    )
    .await?;
    if matches!(request.mode, ShareMode::Promote | ShareMode::Synthesize)
        && request.policy_disposition != "reviewed"
    {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    if request.mode == ShareMode::Reference && target_object_id.is_some() {
        return Err(KnowledgeSpaceError::InvalidInput(
            "reference shares cannot own a target object".to_string(),
        ));
    }
    if request.mode != ShareMode::Reference && target_object_id.is_none() {
        return Err(KnowledgeSpaceError::InvalidInput(
            "materialized share modes require a target object".to_string(),
        ));
    }
    if request.follow_latest != (request.mode == ShareMode::Reference) {
        return Err(KnowledgeSpaceError::InvalidInput(
            "only reference shares may follow the latest revision".to_string(),
        ));
    }
    if let Some(target_object_id) = target_object_id {
        let target_space: Option<Uuid> = sqlx::query_scalar(
            r#"SELECT v.space_id
               FROM knowledge_objects o
               JOIN knowledge_vaults v ON v.id = o.vault_id AND v.tenant_id = o.tenant_id
               WHERE o.id = $1 AND o.tenant_id = $2 AND o.status = 'active'"#,
        )
        .bind(target_object_id)
        .bind(actor.tenant_id)
        .fetch_optional(pool)
        .await?;
        if target_space != Some(request.target_space_id) {
            return Err(KnowledgeSpaceError::InvalidInput(
                "share target object must belong to the target Space".to_string(),
            ));
        }
    }
    let row = sqlx::query_as::<_, KnowledgeShareRow>(
        r#"INSERT INTO knowledge_shares
           (tenant_id, source_space_id, source_object_id, source_revision,
            target_space_id, mode, follow_latest, target_object_id,
            policy_disposition, created_by)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
           RETURNING id, tenant_id, source_space_id, source_object_id, source_revision,
                     target_space_id, mode, follow_latest, target_object_id,
                     policy_disposition, created_by, revision, created_at, updated_at, revoked_at"#,
    )
    .bind(actor.tenant_id)
    .bind(source.0)
    .bind(object_id)
    .bind(request.source_revision)
    .bind(request.target_space_id)
    .bind(request.mode.as_str())
    .bind(request.follow_latest)
    .bind(target_object_id)
    .bind(request.policy_disposition)
    .bind(actor.user_id)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)?;
    Ok(row)
}

pub async fn list_object_shares(
    pool: &PgPool,
    actor: SpaceActor,
    object_id: Uuid,
) -> Result<Vec<KnowledgeShareRow>, KnowledgeSpaceError> {
    let source_space: Uuid = sqlx::query_scalar(
        r#"SELECT v.space_id
           FROM knowledge_objects o
           JOIN knowledge_vaults v ON v.id = o.vault_id AND v.tenant_id = o.tenant_id
           WHERE o.id = $1 AND o.tenant_id = $2 AND o.status <> 'tombstone'"#,
    )
    .bind(object_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    require_role(pool, actor, source_space, SpaceRole::Viewer, false).await?;
    let visible_targets: std::collections::BTreeSet<Uuid> = effective_spaces(pool, actor)
        .await?
        .into_iter()
        .map(|space| space.space.id)
        .collect();
    let mut rows = sqlx::query_as::<_, KnowledgeShareRow>(
        r#"SELECT id, tenant_id, source_space_id, source_object_id, source_revision,
                  target_space_id, mode, follow_latest, target_object_id,
                  policy_disposition, created_by, revision, created_at, updated_at,
                  revoked_at
           FROM knowledge_shares
           WHERE tenant_id = $1 AND source_object_id = $2 AND revoked_at IS NULL
           ORDER BY created_at DESC, id"#,
    )
    .bind(actor.tenant_id)
    .bind(object_id)
    .fetch_all(pool)
    .await?;
    rows.retain(|share| visible_targets.contains(&share.target_space_id));
    Ok(rows)
}

pub async fn revoke_share(
    pool: &PgPool,
    actor: SpaceActor,
    share_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeShareRow, KnowledgeSpaceError> {
    let source_space: Uuid = sqlx::query_scalar(
        "SELECT source_space_id FROM knowledge_shares WHERE id = $1 AND tenant_id = $2",
    )
    .bind(share_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    require_role(pool, actor, source_space, SpaceRole::Manager, true).await?;
    let row = sqlx::query_as::<_, KnowledgeShareRow>(
        r#"UPDATE knowledge_shares SET
             revoked_at = NOW(), revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND revoked_at IS NULL
           RETURNING id, tenant_id, source_space_id, source_object_id, source_revision,
                     target_space_id, mode, follow_latest, target_object_id,
                     policy_disposition, created_by, revision, created_at, updated_at, revoked_at"#,
    )
    .bind(share_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .fetch_optional(pool)
    .await?;
    resolve_cas(pool, "knowledge_shares", share_id, actor.tenant_id, row).await
}

pub async fn set_bundle_owner_state(
    pool: &PgPool,
    actor: SpaceActor,
    home_bundle_id: &str,
    state: VaultOwnerState,
) -> Result<Vec<KnowledgeVaultRow>, KnowledgeSpaceError> {
    if ensure_active_actor(pool, actor).await? != "admin" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    set_bundle_owner_state_system(pool, actor.tenant_id, home_bundle_id, state).await
}

/// Core-owned lifecycle projection after a signed Bundle runtime transition.
/// The runtime endpoint already enforces its Management trust gate; this seam
/// stays tenant-pinned without reinterpreting the initiating user's Space role.
pub async fn set_bundle_owner_state_system(
    pool: &PgPool,
    tenant_id: Uuid,
    home_bundle_id: &str,
    state: VaultOwnerState,
) -> Result<Vec<KnowledgeVaultRow>, KnowledgeSpaceError> {
    let rows = sqlx::query_as::<_, KnowledgeVaultRow>(
        r#"UPDATE knowledge_vaults SET
             owner_state = $3, revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND home_bundle_id = $2 AND owner_state <> $3
           RETURNING id, tenant_id, space_id, home_bundle_id, knowledge_schema_id,
                     schema_version, owner_state, revision, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(home_bundle_id)
    .bind(state.as_str())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn archive_project(
    pool: &PgPool,
    actor: SpaceActor,
    project_id: Uuid,
    expected_revision: i64,
) -> Result<ProjectWithSpace, KnowledgeSpaceError> {
    let actor_role = ensure_active_actor(pool, actor).await?;
    let project: ProjectRow = sqlx::query_as(
        r#"SELECT id, tenant_id, slug, title, goal, status, owner_user_id,
                  policy, policy_revision, revision, created_at, updated_at
           FROM projects WHERE id = $1 AND tenant_id = $2"#,
    )
    .bind(project_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    if project.owner_user_id != actor.user_id && actor_role != "admin" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    if project.revision != expected_revision {
        return Err(KnowledgeSpaceError::RevisionConflict);
    }
    let mut tx = pool.begin().await?;
    let project = sqlx::query_as::<_, ProjectRow>(
        r#"UPDATE projects SET status = 'archived', revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3
           RETURNING id, tenant_id, slug, title, goal, status, owner_user_id,
                     policy, policy_revision, revision, created_at, updated_at"#,
    )
    .bind(project_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(KnowledgeSpaceError::RevisionConflict)?;
    let space = sqlx::query_as::<_, KnowledgeSpaceRow>(&format!(
        "UPDATE knowledge_spaces SET status = 'archived', revision = revision + 1, updated_at = NOW() \
         WHERE tenant_id = $1 AND kind = 'project' AND owner_project_id = $2 RETURNING {}",
        space_columns()
    ))
    .bind(actor.tenant_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ProjectWithSpace { project, space })
}

async fn insert_space(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    kind: &str,
    title: &str,
    owner_user_id: Option<Uuid>,
    owner_team_id: Option<&str>,
    owner_project_id: Option<Uuid>,
) -> Result<KnowledgeSpaceRow, KnowledgeSpaceError> {
    sqlx::query_as::<_, KnowledgeSpaceRow>(&format!(
        "INSERT INTO knowledge_spaces \
         (tenant_id, kind, title, owner_user_id, owner_team_id, owner_project_id) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {}",
        space_columns()
    ))
    .bind(tenant_id)
    .bind(kind)
    .bind(title)
    .bind(owner_user_id)
    .bind(owner_team_id)
    .bind(owner_project_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(map_constraint)
}

async fn find_owned_space(
    pool: &PgPool,
    tenant_id: Uuid,
    kind: &str,
    owner_id: Uuid,
) -> Result<Option<KnowledgeSpaceRow>, KnowledgeSpaceError> {
    Ok(sqlx::query_as::<_, KnowledgeSpaceRow>(&format!(
        "{} WHERE tenant_id = $1 AND kind = $2 AND owner_user_id = $3",
        space_select()
    ))
    .bind(tenant_id)
    .bind(kind)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?)
}

async fn ensure_active_actor(
    pool: &PgPool,
    actor: SpaceActor,
) -> Result<String, KnowledgeSpaceError> {
    sqlx::query_scalar(
        "SELECT role FROM users WHERE id = $1 AND tenant_id = $2 AND is_active = TRUE",
    )
    .bind(actor.user_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::Forbidden)
}

async fn effective_role(
    pool: &PgPool,
    actor: SpaceActor,
    space: &KnowledgeSpaceRow,
) -> Result<Option<SpaceRole>, KnowledgeSpaceError> {
    let actor_role = ensure_active_actor(pool, actor).await?;
    if actor_role == "admin" {
        return Ok(Some(SpaceRole::Manager));
    }
    let mut best = None;
    if space.owner_user_id == Some(actor.user_id) {
        best = stronger(best, SpaceRole::Manager);
    }
    if let Some(project_id) = space.owner_project_id {
        let owner: Option<Uuid> = sqlx::query_scalar(
            "SELECT owner_user_id FROM projects WHERE id = $1 AND tenant_id = $2",
        )
        .bind(project_id)
        .bind(actor.tenant_id)
        .fetch_optional(pool)
        .await?;
        if owner == Some(actor.user_id) {
            best = stronger(best, SpaceRole::Manager);
        }
    }
    if let Some(team_id) = &space.owner_team_id {
        let membership: Option<String> = sqlx::query_scalar(
            r#"SELECT tm.role FROM team_members tm
               JOIN teams t ON t.id = tm.team_id
               WHERE t.tenant_id = $1 AND tm.team_id = $2 AND tm.user_id = $3"#,
        )
        .bind(actor.tenant_id)
        .bind(team_id)
        .bind(actor.user_id)
        .fetch_optional(pool)
        .await?;
        match membership.as_deref() {
            Some("lead") => best = stronger(best, SpaceRole::Manager),
            Some("member") => best = stronger(best, SpaceRole::Contributor),
            _ => {}
        }
    }
    if space.kind == "tenant_shared" {
        best = stronger(best, SpaceRole::Viewer);
    }
    let explicit: Vec<String> = sqlx::query_scalar(
        r#"SELECT g.role
           FROM knowledge_space_grants g
           WHERE g.tenant_id = $1 AND g.space_id = $2
             AND g.revoked_at IS NULL
             AND (g.expires_at IS NULL OR g.expires_at > NOW())
             AND (
               (g.principal_kind = 'user' AND g.principal_id = $3)
               OR (g.principal_kind = 'team' AND EXISTS (
                    SELECT 1 FROM team_members tm JOIN teams t ON t.id = tm.team_id
                    WHERE t.tenant_id = $1 AND tm.user_id = $4 AND tm.team_id = g.principal_id))
               OR (g.principal_kind = 'group' AND EXISTS (
                    SELECT 1 FROM user_groups ug JOIN groups p ON p.id = ug.group_id
                    WHERE p.tenant_id = $1 AND ug.user_id = $4 AND ug.group_id = g.principal_id))
             )"#,
    )
    .bind(actor.tenant_id)
    .bind(space.id)
    .bind(actor.user_id.to_string())
    .bind(actor.user_id)
    .fetch_all(pool)
    .await?;
    for role in explicit {
        best = stronger(best, SpaceRole::parse(&role)?);
    }
    Ok(best)
}

pub(crate) async fn require_role(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    required: SpaceRole,
    require_active: bool,
) -> Result<KnowledgeSpaceRow, KnowledgeSpaceError> {
    let space = sqlx::query_as::<_, KnowledgeSpaceRow>(&format!(
        "{} WHERE tenant_id = $1 AND id = $2",
        space_select()
    ))
    .bind(actor.tenant_id)
    .bind(space_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    if require_active && space.status != "active" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    let role = effective_role(pool, actor, &space)
        .await?
        .ok_or(KnowledgeSpaceError::Forbidden)?;
    if role.weight() < required.weight() {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    Ok(space)
}

async fn validate_principal(
    pool: &PgPool,
    tenant_id: Uuid,
    kind: PrincipalKind,
    principal_id: &str,
) -> Result<(), KnowledgeSpaceError> {
    let exists: bool = match kind {
        PrincipalKind::User => {
            let id = Uuid::parse_str(principal_id).map_err(|_| {
                KnowledgeSpaceError::InvalidInput("user principal must be a UUID".to_string())
            })?;
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM users WHERE tenant_id = $1 AND id = $2)",
            )
            .bind(tenant_id)
            .bind(id)
            .fetch_one(pool)
            .await?
        }
        PrincipalKind::Team => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM teams WHERE tenant_id = $1 AND id = $2)",
            )
            .bind(tenant_id)
            .bind(principal_id)
            .fetch_one(pool)
            .await?
        }
        PrincipalKind::Group => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM groups WHERE tenant_id = $1 AND id = $2)",
            )
            .bind(tenant_id)
            .bind(principal_id)
            .fetch_one(pool)
            .await?
        }
    };
    if exists {
        Ok(())
    } else {
        Err(KnowledgeSpaceError::NotFound)
    }
}

fn stronger(current: Option<SpaceRole>, candidate: SpaceRole) -> Option<SpaceRole> {
    match current {
        Some(role) if role.weight() >= candidate.weight() => Some(role),
        _ => Some(candidate),
    }
}

fn map_constraint(error: sqlx::Error) -> KnowledgeSpaceError {
    match &error {
        sqlx::Error::Database(database) if matches!(database.code().as_deref(), Some("23505")) => {
            KnowledgeSpaceError::Conflict
        }
        sqlx::Error::Database(database)
            if matches!(database.code().as_deref(), Some("23503" | "23514")) =>
        {
            KnowledgeSpaceError::InvalidInput(database.message().to_string())
        }
        _ => KnowledgeSpaceError::Database(error),
    }
}

async fn resolve_cas<T>(
    pool: &PgPool,
    table: &str,
    id: Uuid,
    tenant_id: Uuid,
    row: Option<T>,
) -> Result<T, KnowledgeSpaceError> {
    if let Some(row) = row {
        return Ok(row);
    }
    let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1 AND tenant_id = $2)");
    let exists: bool = sqlx::query_scalar(&query)
        .bind(id)
        .bind(tenant_id)
        .fetch_one(pool)
        .await?;
    if exists {
        Err(KnowledgeSpaceError::RevisionConflict)
    } else {
        Err(KnowledgeSpaceError::NotFound)
    }
}

const fn space_columns() -> &'static str {
    "id, tenant_id, kind, title, owner_user_id, owner_team_id, owner_project_id, \
     status, policy, revision, created_at, updated_at"
}

fn space_select() -> String {
    format!("SELECT {} FROM knowledge_spaces", space_columns())
}
