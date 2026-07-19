use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    knowledge_spaces::{self as spaces, KnowledgeSpaceError, SpaceActor, SpaceRole},
    service_principals::{self, ServicePrincipalError},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionLocator {
    pub url: String,
    #[serde(default)]
    pub title: String,
    pub source_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionQuery {
    pub provider: String,
    pub query: String,
    pub scope: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub window_days: u32,
}

#[derive(Debug, Clone)]
pub struct CreateKnowledgeCollection {
    pub space_id: Uuid,
    pub output_vault_id: Uuid,
    pub bundle_id: String,
    pub profile_id: String,
    pub label: String,
    pub topic: String,
    pub connector: String,
    pub source_classes: Vec<String>,
    pub allowed_domains: Vec<String>,
    pub freshness_seconds: i64,
    pub schedule: Option<String>,
    pub schedule_enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub max_sources: i32,
    pub max_bytes: i64,
    pub max_wall_seconds: i32,
    pub package_manifest_sha256: String,
    pub recipe_asset_id: String,
    pub recipe_sha256: String,
    pub locators: Vec<CollectionLocator>,
    pub queries: Vec<CollectionQuery>,
}

#[derive(Debug, Clone)]
pub struct UpdateKnowledgeCollection {
    pub expected_revision: i64,
    pub topic: String,
    pub status: String,
    pub schedule_enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub locators: Vec<CollectionLocator>,
    pub queries: Vec<CollectionQuery>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeCollectionRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub space_id: Uuid,
    pub output_vault_id: Uuid,
    pub bundle_id: String,
    pub profile_id: String,
    pub label: String,
    pub topic: String,
    pub status: String,
    pub connector: String,
    pub source_classes: Vec<String>,
    pub allowed_domains: Vec<String>,
    pub freshness_seconds: i64,
    pub schedule: Option<String>,
    pub schedule_enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub max_sources: i32,
    pub max_bytes: i64,
    pub max_wall_seconds: i32,
    pub package_manifest_sha256: String,
    pub recipe_asset_id: String,
    pub recipe_sha256: String,
    pub locators: Value,
    pub queries: Value,
    pub cursor: Value,
    pub created_by_user_id: Uuid,
    pub updated_by_user_id: Uuid,
    pub last_enqueued_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl KnowledgeCollectionRow {
    pub fn parsed_locators(&self) -> Result<Vec<CollectionLocator>, KnowledgeCollectionError> {
        serde_json::from_value(self.locators.clone()).map_err(|error| {
            KnowledgeCollectionError::InvalidPersisted(format!(
                "collection locators are invalid: {error}"
            ))
        })
    }

    pub fn parsed_queries(&self) -> Result<Vec<CollectionQuery>, KnowledgeCollectionError> {
        serde_json::from_value(self.queries.clone()).map_err(|error| {
            KnowledgeCollectionError::InvalidPersisted(format!(
                "collection queries are invalid: {error}"
            ))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionRunTrigger {
    OnDemand,
    Schedule,
    Retry,
}

impl CollectionRunTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::Schedule => "schedule",
            Self::Retry => "retry",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnqueueCollectionRun {
    pub expected_collection_revision: i64,
    pub trigger: CollectionRunTrigger,
    pub requested_by_user_id: Uuid,
    pub on_behalf_of_user_id: Uuid,
    pub tool_policy_revision: String,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub next_schedule_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnqueuedCollectionRun {
    pub run: KnowledgeCollectionRunRow,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeCollectionRunRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub collection_id: Uuid,
    pub space_id: Uuid,
    pub output_vault_id: Uuid,
    pub trigger: String,
    pub parent_run_id: Option<Uuid>,
    pub status: String,
    pub service_actor_user_id: Uuid,
    pub requested_by_user_id: Uuid,
    pub on_behalf_of_user_id: Uuid,
    pub bundle_id: String,
    pub profile_id: String,
    pub connector: String,
    pub source_classes: Vec<String>,
    pub allowed_domains: Vec<String>,
    pub freshness_seconds: i64,
    pub package_manifest_sha256: String,
    pub recipe_asset_id: String,
    pub recipe_sha256: String,
    pub tool_policy_revision: String,
    pub max_sources: i32,
    pub max_bytes: i64,
    pub max_wall_seconds: i32,
    pub used_items: i32,
    pub used_bytes: i64,
    pub cursor_before: Value,
    pub cursor_after: Value,
    pub attempt: i32,
    pub scheduled_at: DateTime<Utc>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub terminal_reason: Option<String>,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeCollectionRunItemRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub run_id: Uuid,
    pub collection_id: Uuid,
    pub position: i32,
    pub locator: String,
    pub title: String,
    pub source_class: String,
    pub status: String,
    pub previous_source_id: Option<Uuid>,
    pub source_id: Option<Uuid>,
    pub canonical_locator: Option<String>,
    pub content_hash: Option<String>,
    pub byte_size: Option<i64>,
    pub http_status: Option<i32>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub fresh_until: Option<DateTime<Utc>>,
    pub deletion_observed_at: Option<DateTime<Utc>>,
    pub failure_code: Option<String>,
    pub failure_detail: Option<String>,
    pub attempt_no: i32,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CompleteCollectionItem {
    pub status: String,
    pub source_id: Option<Uuid>,
    pub canonical_locator: Option<String>,
    pub content_hash: Option<String>,
    pub byte_size: Option<i64>,
    pub http_status: Option<i32>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub fresh_until: Option<DateTime<Utc>>,
    pub deletion_observed_at: Option<DateTime<Utc>>,
    pub failure_code: Option<String>,
    pub failure_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CollectionSourceHealthRow {
    pub locator: String,
    pub title: String,
    pub source_class: String,
    pub health: String,
    pub observation_status: String,
    pub source_id: Option<Uuid>,
    pub previous_source_id: Option<Uuid>,
    pub content_hash: Option<String>,
    pub byte_size: Option<i64>,
    pub http_status: Option<i32>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub fresh_until: Option<DateTime<Utc>>,
    pub deletion_observed_at: Option<DateTime<Utc>>,
    pub failure_code: Option<String>,
    pub failure_detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionRunControl {
    Continue,
    CancelRequested,
    WallTimeExceeded,
    ByteBudgetExceeded,
}

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeCollectionError {
    #[error("collection persistence failed")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Space(#[from] KnowledgeSpaceError),
    #[error(transparent)]
    ServicePrincipal(#[from] ServicePrincipalError),
    #[error("collection input is invalid: {0}")]
    InvalidInput(String),
    #[error("persisted collection state is invalid: {0}")]
    InvalidPersisted(String),
    #[error("collection or run was not found")]
    NotFound,
    #[error("collection or run revision changed")]
    Conflict,
    #[error("collection run lease is not owned by this worker")]
    LeaseLost,
}

const COLLECTION_COLUMNS: &str = r#"id, tenant_id, space_id, output_vault_id, bundle_id,
    profile_id, label, topic, status, connector, source_classes, allowed_domains,
    freshness_seconds, schedule, schedule_enabled, next_run_at, max_sources, max_bytes,
    max_wall_seconds, package_manifest_sha256, recipe_asset_id, recipe_sha256, locators,
    queries, cursor, created_by_user_id, updated_by_user_id, last_enqueued_at, last_run_at,
    revision, created_at, updated_at"#;

const RUN_COLUMNS: &str = r#"id, tenant_id, collection_id, space_id, output_vault_id,
    trigger, parent_run_id, status, service_actor_user_id, requested_by_user_id,
    on_behalf_of_user_id, bundle_id, profile_id, connector, source_classes, allowed_domains,
    freshness_seconds, package_manifest_sha256, recipe_asset_id, recipe_sha256,
    tool_policy_revision, max_sources, max_bytes, max_wall_seconds, used_items, used_bytes,
    cursor_before, cursor_after, attempt, scheduled_at, lease_owner, lease_expires_at,
    heartbeat_at, cancel_requested_at, terminal_reason, revision, created_at, started_at,
    finished_at, updated_at"#;

const ITEM_COLUMNS: &str = r#"id, tenant_id, run_id, collection_id, position, locator, title,
    source_class, status, previous_source_id, source_id, canonical_locator, content_hash,
    byte_size, http_status, fetched_at, fresh_until, deletion_observed_at, failure_code,
    failure_detail, attempt_no, revision, created_at, updated_at"#;

pub async fn create_collection(
    pool: &PgPool,
    actor: SpaceActor,
    request: CreateKnowledgeCollection,
) -> Result<KnowledgeCollectionRow, KnowledgeCollectionError> {
    validate_collection_input(CollectionInput {
        topic: &request.topic,
        source_classes: &request.source_classes,
        allowed_domains: &request.allowed_domains,
        schedule: request.schedule.as_deref(),
        schedule_enabled: request.schedule_enabled,
        next_run_at: request.next_run_at,
        max_sources: request.max_sources,
        max_bytes: request.max_bytes,
        max_wall_seconds: request.max_wall_seconds,
        locators: &request.locators,
        queries: &request.queries,
    })?;
    require_output_vault(pool, actor, request.space_id, request.output_vault_id).await?;
    let locators = serde_json::to_value(&request.locators)
        .map_err(|error| KnowledgeCollectionError::InvalidInput(error.to_string()))?;
    let queries = serde_json::to_value(&request.queries)
        .map_err(|error| KnowledgeCollectionError::InvalidInput(error.to_string()))?;
    sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        r#"INSERT INTO knowledge_collections
           (tenant_id, space_id, output_vault_id, bundle_id, profile_id, label, topic,
            connector, source_classes, allowed_domains, freshness_seconds, schedule,
            schedule_enabled, next_run_at, max_sources, max_bytes, max_wall_seconds,
            package_manifest_sha256, recipe_asset_id, recipe_sha256, locators, queries,
            created_by_user_id, updated_by_user_id)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$23)
           RETURNING {COLLECTION_COLUMNS}"#
    ))
    .bind(actor.tenant_id)
    .bind(request.space_id)
    .bind(request.output_vault_id)
    .bind(request.bundle_id)
    .bind(request.profile_id)
    .bind(request.label)
    .bind(request.topic)
    .bind(request.connector)
    .bind(request.source_classes)
    .bind(request.allowed_domains)
    .bind(request.freshness_seconds)
    .bind(request.schedule)
    .bind(request.schedule_enabled)
    .bind(request.next_run_at)
    .bind(request.max_sources)
    .bind(request.max_bytes)
    .bind(request.max_wall_seconds)
    .bind(request.package_manifest_sha256)
    .bind(request.recipe_asset_id)
    .bind(request.recipe_sha256)
    .bind(locators)
    .bind(queries)
    .bind(actor.user_id)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)
}

pub async fn update_collection(
    pool: &PgPool,
    actor: SpaceActor,
    collection_id: Uuid,
    request: UpdateKnowledgeCollection,
) -> Result<KnowledgeCollectionRow, KnowledgeCollectionError> {
    let current = get_collection(pool, actor, collection_id, SpaceRole::Contributor, true).await?;
    validate_collection_input(CollectionInput {
        topic: &request.topic,
        source_classes: &current.source_classes,
        allowed_domains: &current.allowed_domains,
        schedule: current.schedule.as_deref(),
        schedule_enabled: request.schedule_enabled,
        next_run_at: request.next_run_at,
        max_sources: current.max_sources,
        max_bytes: current.max_bytes,
        max_wall_seconds: current.max_wall_seconds,
        locators: &request.locators,
        queries: &request.queries,
    })?;
    if !matches!(request.status.as_str(), "active" | "paused") {
        return Err(KnowledgeCollectionError::InvalidInput(
            "collection status must be active or paused".to_string(),
        ));
    }
    let locators = serde_json::to_value(request.locators)
        .map_err(|error| KnowledgeCollectionError::InvalidInput(error.to_string()))?;
    let queries = serde_json::to_value(request.queries)
        .map_err(|error| KnowledgeCollectionError::InvalidInput(error.to_string()))?;
    let row = sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        r#"UPDATE knowledge_collections SET
             topic = $4, status = $5, schedule_enabled = $6, next_run_at = $7,
             locators = $8, queries = $9, updated_by_user_id = $10, revision = revision + 1,
             updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND deleted_at IS NULL
             AND status <> 'archived'
           RETURNING {COLLECTION_COLUMNS}"#
    ))
    .bind(collection_id)
    .bind(actor.tenant_id)
    .bind(request.expected_revision)
    .bind(request.topic)
    .bind(request.status)
    .bind(request.schedule_enabled)
    .bind(request.next_run_at)
    .bind(locators)
    .bind(queries)
    .bind(actor.user_id)
    .fetch_optional(pool)
    .await?;
    resolve_collection_cas(pool, actor.tenant_id, collection_id, row).await
}

pub async fn rebind_collection_package(
    pool: &PgPool,
    actor: SpaceActor,
    collection_id: Uuid,
    expected_revision: i64,
    package_manifest_sha256: String,
) -> Result<KnowledgeCollectionRow, KnowledgeCollectionError> {
    get_collection(pool, actor, collection_id, SpaceRole::Contributor, true).await?;
    if package_manifest_sha256.len() != 64
        || !package_manifest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(KnowledgeCollectionError::InvalidInput(
            "collection package digest must be lowercase SHA-256".to_string(),
        ));
    }
    let row = sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        r#"UPDATE knowledge_collections SET
             package_manifest_sha256 = $4, updated_by_user_id = $5,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3
             AND deleted_at IS NULL AND status <> 'archived'
           RETURNING {COLLECTION_COLUMNS}"#
    ))
    .bind(collection_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .bind(package_manifest_sha256)
    .bind(actor.user_id)
    .fetch_optional(pool)
    .await?;
    resolve_collection_cas(pool, actor.tenant_id, collection_id, row).await
}

pub async fn archive_collection(
    pool: &PgPool,
    actor: SpaceActor,
    collection_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeCollectionRow, KnowledgeCollectionError> {
    get_collection(pool, actor, collection_id, SpaceRole::Contributor, true).await?;
    let row = sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        r#"UPDATE knowledge_collections SET status = 'archived', schedule_enabled = FALSE,
             next_run_at = NULL, deleted_at = NOW(), updated_by_user_id = $4,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND deleted_at IS NULL
           RETURNING {COLLECTION_COLUMNS}"#
    ))
    .bind(collection_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .bind(actor.user_id)
    .fetch_optional(pool)
    .await?;
    resolve_collection_cas(pool, actor.tenant_id, collection_id, row).await
}

pub async fn get_collection(
    pool: &PgPool,
    actor: SpaceActor,
    collection_id: Uuid,
    required: SpaceRole,
    require_active: bool,
) -> Result<KnowledgeCollectionRow, KnowledgeCollectionError> {
    let row = find_collection(pool, actor.tenant_id, collection_id)
        .await?
        .ok_or(KnowledgeCollectionError::NotFound)?;
    spaces::require_role(pool, actor, row.space_id, required, require_active).await?;
    Ok(row)
}

pub async fn list_collections(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
) -> Result<Vec<KnowledgeCollectionRow>, KnowledgeCollectionError> {
    spaces::require_role(pool, actor, space_id, SpaceRole::Viewer, false).await?;
    Ok(sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        "SELECT {COLLECTION_COLUMNS} FROM knowledge_collections WHERE tenant_id = $1 AND space_id = $2 AND deleted_at IS NULL ORDER BY updated_at DESC, id"
    ))
    .bind(actor.tenant_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_bundle_collections(
    pool: &PgPool,
    actor: SpaceActor,
    bundle_id: &str,
    limit: i64,
) -> Result<Vec<KnowledgeCollectionRow>, KnowledgeCollectionError> {
    let space_ids: Vec<Uuid> = spaces::effective_spaces(pool, actor)
        .await?
        .into_iter()
        .map(|entry| entry.space.id)
        .collect();
    if space_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        r#"SELECT {COLLECTION_COLUMNS} FROM knowledge_collections
           WHERE tenant_id = $1 AND bundle_id = $2 AND space_id = ANY($3)
             AND deleted_at IS NULL
           ORDER BY updated_at DESC, id LIMIT $4"#
    ))
    .bind(actor.tenant_id)
    .bind(bundle_id)
    .bind(space_ids)
    .bind(limit.clamp(1, 201))
    .fetch_all(pool)
    .await?)
}

pub async fn due_collections(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<KnowledgeCollectionRow>, KnowledgeCollectionError> {
    Ok(sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        r#"SELECT {COLLECTION_COLUMNS} FROM knowledge_collections
           WHERE deleted_at IS NULL AND status = 'active' AND schedule_enabled = TRUE
             AND next_run_at <= NOW()
           ORDER BY next_run_at, tenant_id, id LIMIT $1"#
    ))
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?)
}

pub async fn enqueue_run(
    pool: &PgPool,
    actor: SpaceActor,
    collection_id: Uuid,
    request: EnqueueCollectionRun,
) -> Result<EnqueuedCollectionRun, KnowledgeCollectionError> {
    let collection =
        get_collection(pool, actor, collection_id, SpaceRole::Contributor, true).await?;
    if request.requested_by_user_id != actor.user_id
        || request.on_behalf_of_user_id != actor.user_id
    {
        return Err(KnowledgeCollectionError::InvalidInput(
            "on-demand collection actor snapshot does not match the authenticated actor"
                .to_string(),
        ));
    }
    enqueue_snapshot(pool, collection, request, None).await
}

pub async fn enqueue_scheduled_run(
    pool: &PgPool,
    collection: KnowledgeCollectionRow,
    tool_policy_revision: String,
    next_schedule_at: DateTime<Utc>,
) -> Result<EnqueuedCollectionRun, KnowledgeCollectionError> {
    let service = service_principals::ensure_knowledge_agent(pool, collection.tenant_id).await?;
    let revision = collection.revision;
    let owner = collection.created_by_user_id;
    enqueue_snapshot(
        pool,
        collection,
        EnqueueCollectionRun {
            expected_collection_revision: revision,
            trigger: CollectionRunTrigger::Schedule,
            requested_by_user_id: service.user_id,
            on_behalf_of_user_id: owner,
            tool_policy_revision,
            scheduled_at: None,
            next_schedule_at: Some(next_schedule_at),
        },
        None,
    )
    .await
}

async fn enqueue_snapshot(
    pool: &PgPool,
    collection: KnowledgeCollectionRow,
    request: EnqueueCollectionRun,
    parent_run_id: Option<Uuid>,
) -> Result<EnqueuedCollectionRun, KnowledgeCollectionError> {
    if collection.revision != request.expected_collection_revision || collection.status != "active"
    {
        return Err(KnowledgeCollectionError::Conflict);
    }
    if request.tool_policy_revision.trim().is_empty() || request.tool_policy_revision.len() > 256 {
        return Err(KnowledgeCollectionError::InvalidInput(
            "tool policy revision is invalid".to_string(),
        ));
    }
    let locators = collection.parsed_locators()?;
    let service = service_principals::ensure_knowledge_agent(pool, collection.tenant_id).await?;
    let mut tx = pool.begin().await?;
    let scheduled_at = request.scheduled_at.unwrap_or_else(Utc::now);
    let inserted = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        r#"INSERT INTO knowledge_collection_runs
           (tenant_id, collection_id, space_id, output_vault_id, trigger, parent_run_id,
            service_actor_user_id, requested_by_user_id, on_behalf_of_user_id,
            bundle_id, profile_id, connector, source_classes, allowed_domains,
            freshness_seconds, package_manifest_sha256, recipe_asset_id, recipe_sha256,
            tool_policy_revision, max_sources, max_bytes, max_wall_seconds,
            cursor_before, cursor_after, scheduled_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$23,$24)
           ON CONFLICT DO NOTHING RETURNING {RUN_COLUMNS}"#
    ))
    .bind(collection.tenant_id)
    .bind(collection.id)
    .bind(collection.space_id)
    .bind(collection.output_vault_id)
    .bind(request.trigger.as_str())
    .bind(parent_run_id)
    .bind(service.user_id)
    .bind(request.requested_by_user_id)
    .bind(request.on_behalf_of_user_id)
    .bind(&collection.bundle_id)
    .bind(&collection.profile_id)
    .bind(&collection.connector)
    .bind(&collection.source_classes)
    .bind(&collection.allowed_domains)
    .bind(collection.freshness_seconds)
    .bind(&collection.package_manifest_sha256)
    .bind(&collection.recipe_asset_id)
    .bind(&collection.recipe_sha256)
    .bind(request.tool_policy_revision)
    .bind(collection.max_sources)
    .bind(collection.max_bytes)
    .bind(collection.max_wall_seconds)
    .bind(&collection.cursor)
    .bind(scheduled_at)
    .fetch_optional(&mut *tx)
    .await?;
    let (run, created) = if let Some(run) = inserted {
        insert_run_items(&mut tx, &collection, &run, &locators).await?;
        (run, true)
    } else {
        let run = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
            "SELECT {RUN_COLUMNS} FROM knowledge_collection_runs WHERE tenant_id = $1 AND collection_id = $2 AND status IN ('queued','running')"
        ))
        .bind(collection.tenant_id)
        .bind(collection.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(KnowledgeCollectionError::Conflict)?;
        (run, false)
    };
    if created || request.next_schedule_at.is_some() {
        sqlx::query(
            r#"UPDATE knowledge_collections SET last_enqueued_at = NOW(),
               next_run_at = COALESCE($3, next_run_at),
               updated_at = NOW() WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(collection.tenant_id)
        .bind(collection.id)
        .bind(request.next_schedule_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(EnqueuedCollectionRun { run, created })
}

async fn insert_run_items(
    tx: &mut Transaction<'_, Postgres>,
    collection: &KnowledgeCollectionRow,
    run: &KnowledgeCollectionRunRow,
    locators: &[CollectionLocator],
) -> Result<(), KnowledgeCollectionError> {
    for (position, locator) in locators.iter().enumerate() {
        let previous_source_id: Option<Uuid> = sqlx::query_scalar(
            r#"SELECT source_id FROM knowledge_collection_run_items
               WHERE tenant_id = $1 AND collection_id = $2 AND locator = $3
                 AND source_id IS NOT NULL AND status IN ('captured','unchanged')
               ORDER BY updated_at DESC, id DESC LIMIT 1"#,
        )
        .bind(collection.tenant_id)
        .bind(collection.id)
        .bind(&locator.url)
        .fetch_optional(&mut **tx)
        .await?
        .flatten();
        sqlx::query(
            r#"INSERT INTO knowledge_collection_run_items
               (tenant_id, run_id, collection_id, position, locator, title, source_class,
                previous_source_id)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"#,
        )
        .bind(collection.tenant_id)
        .bind(run.id)
        .bind(collection.id)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .bind(&locator.url)
        .bind(&locator.title)
        .bind(&locator.source_class)
        .bind(previous_source_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn lease_next(
    pool: &PgPool,
    worker_id: &str,
    lease_seconds: i32,
) -> Result<Option<KnowledgeCollectionRunRow>, KnowledgeCollectionError> {
    if worker_id.is_empty() || worker_id.len() > 200 || !(5..=300).contains(&lease_seconds) {
        return Err(KnowledgeCollectionError::InvalidInput(
            "worker lease input is invalid".to_string(),
        ));
    }
    let mut tx = pool.begin().await?;
    let expired: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"UPDATE knowledge_collection_runs SET status = 'queued', lease_owner = NULL,
             lease_expires_at = NULL, heartbeat_at = NULL, revision = revision + 1,
             updated_at = NOW()
           WHERE status = 'running' AND lease_expires_at < NOW()
           RETURNING tenant_id, id"#,
    )
    .fetch_all(&mut *tx)
    .await?;
    for (tenant_id, run_id) in expired {
        sqlx::query(
            r#"UPDATE knowledge_collection_run_items SET status = 'pending',
               revision = revision + 1, updated_at = NOW()
               WHERE tenant_id = $1 AND run_id = $2 AND status = 'fetching'"#,
        )
        .bind(tenant_id)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
    }
    let selected: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM knowledge_collection_runs
           WHERE status = 'queued' AND scheduled_at <= NOW()
           ORDER BY scheduled_at, created_at, id FOR UPDATE SKIP LOCKED LIMIT 1"#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(run_id) = selected else {
        tx.commit().await?;
        return Ok(None);
    };
    let run = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        r#"UPDATE knowledge_collection_runs SET status = 'running', lease_owner = $2,
             lease_expires_at = NOW() + make_interval(secs => $3), heartbeat_at = NOW(),
             started_at = COALESCE(started_at, NOW()), attempt = attempt + 1,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 RETURNING {RUN_COLUMNS}"#
    ))
    .bind(run_id)
    .bind(worker_id)
    .bind(f64::from(lease_seconds))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(run))
}

pub async fn heartbeat(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: &str,
    lease_seconds: i32,
) -> Result<(KnowledgeCollectionRunRow, CollectionRunControl), KnowledgeCollectionError> {
    let run = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        r#"UPDATE knowledge_collection_runs SET
             lease_expires_at = NOW() + make_interval(secs => $3), heartbeat_at = NOW(),
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND status = 'running' AND lease_owner = $2
           RETURNING {RUN_COLUMNS}"#
    ))
    .bind(run_id)
    .bind(worker_id)
    .bind(f64::from(lease_seconds))
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeCollectionError::LeaseLost)?;
    let control = run_control(&run);
    Ok((run, control))
}

pub async fn claim_next_item(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: &str,
) -> Result<Option<KnowledgeCollectionRunItemRow>, KnowledgeCollectionError> {
    let mut tx = pool.begin().await?;
    let owns: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM knowledge_collection_runs
           WHERE id = $1 AND status = 'running' AND lease_owner = $2
             AND lease_expires_at > NOW())"#,
    )
    .bind(run_id)
    .bind(worker_id)
    .fetch_one(&mut *tx)
    .await?;
    if !owns {
        return Err(KnowledgeCollectionError::LeaseLost);
    }
    let item_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM knowledge_collection_run_items
           WHERE run_id = $1 AND status = 'pending'
           ORDER BY position FOR UPDATE SKIP LOCKED LIMIT 1"#,
    )
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?;
    let item = if let Some(item_id) = item_id {
        Some(
            sqlx::query_as::<_, KnowledgeCollectionRunItemRow>(&format!(
                r#"UPDATE knowledge_collection_run_items SET status = 'fetching',
                   attempt_no = attempt_no + 1, revision = revision + 1, updated_at = NOW()
                   WHERE id = $1 RETURNING {ITEM_COLUMNS}"#
            ))
            .bind(item_id)
            .fetch_one(&mut *tx)
            .await?,
        )
    } else {
        None
    };
    tx.commit().await?;
    Ok(item)
}

pub async fn complete_item(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: &str,
    item_id: Uuid,
    result: CompleteCollectionItem,
) -> Result<KnowledgeCollectionRunItemRow, KnowledgeCollectionError> {
    validate_item_result(&result)?;
    let mut tx = pool.begin().await?;
    let item = sqlx::query_as::<_, KnowledgeCollectionRunItemRow>(&format!(
        r#"UPDATE knowledge_collection_run_items SET status = $4, source_id = $5,
             canonical_locator = $6, content_hash = $7, byte_size = $8, http_status = $9,
             fetched_at = $10, fresh_until = $11, deletion_observed_at = $12,
             failure_code = $13, failure_detail = $14, revision = revision + 1,
             updated_at = NOW()
           WHERE id = $1 AND run_id = $2 AND status = 'fetching'
             AND EXISTS (SELECT 1 FROM knowledge_collection_runs r
                         WHERE r.id = run_id AND r.status = 'running'
                           AND r.lease_owner = $3 AND r.lease_expires_at > NOW())
           RETURNING {ITEM_COLUMNS}"#
    ))
    .bind(item_id)
    .bind(run_id)
    .bind(worker_id)
    .bind(&result.status)
    .bind(result.source_id)
    .bind(&result.canonical_locator)
    .bind(&result.content_hash)
    .bind(result.byte_size)
    .bind(result.http_status)
    .bind(result.fetched_at)
    .bind(result.fresh_until)
    .bind(result.deletion_observed_at)
    .bind(&result.failure_code)
    .bind(result.failure_detail.as_deref().map(truncate_detail))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(KnowledgeCollectionError::LeaseLost)?;
    let added_bytes = result.byte_size.unwrap_or_default();
    sqlx::query(
        r#"UPDATE knowledge_collection_runs SET used_items = used_items + 1,
           used_bytes = used_bytes + $4,
           cursor_after = jsonb_build_object('last_completed_position', $5, 'run_id', id),
           revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND status = 'running' AND lease_owner = $2
             AND lease_expires_at > NOW() AND tenant_id = $3"#,
    )
    .bind(run_id)
    .bind(worker_id)
    .bind(item.tenant_id)
    .bind(added_bytes)
    .bind(item.position)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(item)
}

pub async fn finish_run(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: &str,
    terminal_reason: Option<&str>,
) -> Result<KnowledgeCollectionRunRow, KnowledgeCollectionError> {
    let mut tx = pool.begin().await?;
    let run = find_run_tx(&mut tx, run_id)
        .await?
        .ok_or(KnowledgeCollectionError::NotFound)?;
    if run.status != "running" || run.lease_owner.as_deref() != Some(worker_id) {
        return Err(KnowledgeCollectionError::LeaseLost);
    }
    let skipped_reason = terminal_reason.map(truncate_detail);
    sqlx::query(
        r#"UPDATE knowledge_collection_run_items SET status = 'skipped',
           failure_code = COALESCE(failure_code, 'collection_stopped'),
           failure_detail = COALESCE(failure_detail, $3), revision = revision + 1,
           updated_at = NOW()
           WHERE tenant_id = $1 AND run_id = $2 AND status IN ('pending','fetching')"#,
    )
    .bind(run.tenant_id)
    .bind(run.id)
    .bind(skipped_reason.clone())
    .execute(&mut *tx)
    .await?;
    let counts: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT COUNT(*),
                  COUNT(*) FILTER (WHERE status IN ('captured','unchanged','deleted')),
                  COUNT(*) FILTER (WHERE status IN ('failed','skipped'))
           FROM knowledge_collection_run_items WHERE tenant_id = $1 AND run_id = $2"#,
    )
    .bind(run.tenant_id)
    .bind(run.id)
    .fetch_one(&mut *tx)
    .await?;
    let status = if run.cancel_requested_at.is_some() {
        "cancelled"
    } else if counts.0 > 0 && counts.1 == counts.0 && counts.2 == 0 {
        "succeeded"
    } else if counts.1 > 0 {
        "partial"
    } else {
        "failed"
    };
    let terminal = terminal_reason.map(truncate_detail).or_else(|| {
        (status == "failed").then(|| "collection produced no successful observations".to_string())
    });
    let finished = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        r#"UPDATE knowledge_collection_runs SET status = $3, terminal_reason = $4,
             lease_owner = NULL, lease_expires_at = NULL, heartbeat_at = NOW(),
             finished_at = NOW(), revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 RETURNING {RUN_COLUMNS}"#
    ))
    .bind(run.id)
    .bind(run.tenant_id)
    .bind(status)
    .bind(terminal)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE knowledge_collections SET cursor = $3, last_run_at = NOW(),
           updated_at = NOW() WHERE tenant_id = $1 AND id = $2"#,
    )
    .bind(run.tenant_id)
    .bind(run.collection_id)
    .bind(&finished.cursor_after)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(finished)
}

pub async fn release_lease(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: &str,
) -> Result<(), KnowledgeCollectionError> {
    let mut tx = pool.begin().await?;
    let released = sqlx::query(
        r#"UPDATE knowledge_collection_runs SET status = 'queued', lease_owner = NULL,
           lease_expires_at = NULL, heartbeat_at = NOW(), revision = revision + 1,
           updated_at = NOW() WHERE id = $1 AND status = 'running' AND lease_owner = $2"#,
    )
    .bind(run_id)
    .bind(worker_id)
    .execute(&mut *tx)
    .await?;
    if released.rows_affected() == 0 {
        return Err(KnowledgeCollectionError::LeaseLost);
    }
    sqlx::query(
        r#"UPDATE knowledge_collection_run_items SET status = 'pending',
           revision = revision + 1, updated_at = NOW()
           WHERE run_id = $1 AND status = 'fetching'"#,
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn request_cancel(
    pool: &PgPool,
    actor: SpaceActor,
    run_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeCollectionRunRow, KnowledgeCollectionError> {
    let current = get_run(pool, actor, run_id).await?;
    if !matches!(current.status.as_str(), "queued" | "running") {
        return Err(KnowledgeCollectionError::Conflict);
    }
    spaces::require_role(pool, actor, current.space_id, SpaceRole::Contributor, true).await?;
    let row = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        r#"UPDATE knowledge_collection_runs SET cancel_requested_at = NOW(),
             status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END,
             finished_at = CASE WHEN status = 'queued' THEN NOW() ELSE finished_at END,
             terminal_reason = CASE WHEN status = 'queued' THEN 'cancelled by user' ELSE terminal_reason END,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND status IN ('queued','running')
           RETURNING {RUN_COLUMNS}"#
    ))
    .bind(run_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .fetch_optional(pool)
    .await?;
    resolve_run_cas(pool, actor.tenant_id, run_id, row).await
}

pub async fn retry_run(
    pool: &PgPool,
    actor: SpaceActor,
    run_id: Uuid,
    expected_revision: i64,
) -> Result<EnqueuedCollectionRun, KnowledgeCollectionError> {
    let parent = get_run(pool, actor, run_id).await?;
    spaces::require_role(pool, actor, parent.space_id, SpaceRole::Contributor, true).await?;
    if parent.revision != expected_revision
        || !matches!(
            parent.status.as_str(),
            "succeeded" | "partial" | "failed" | "cancelled"
        )
    {
        return Err(KnowledgeCollectionError::Conflict);
    }
    let collection = find_collection(pool, actor.tenant_id, parent.collection_id)
        .await?
        .ok_or(KnowledgeCollectionError::NotFound)?;
    if collection.status != "active" {
        return Err(KnowledgeCollectionError::Conflict);
    }
    let service = service_principals::ensure_knowledge_agent(pool, actor.tenant_id).await?;
    let mut tx = pool.begin().await?;
    let collection_active: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM knowledge_collections
           WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL AND status = 'active'
           FOR SHARE)"#,
    )
    .bind(actor.tenant_id)
    .bind(parent.collection_id)
    .fetch_one(&mut *tx)
    .await?;
    if !collection_active {
        return Err(KnowledgeCollectionError::Conflict);
    }
    let inserted = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        r#"INSERT INTO knowledge_collection_runs
           (tenant_id, collection_id, space_id, output_vault_id, trigger, parent_run_id,
            service_actor_user_id, requested_by_user_id, on_behalf_of_user_id,
            bundle_id, profile_id, connector, source_classes, allowed_domains,
            freshness_seconds, package_manifest_sha256, recipe_asset_id, recipe_sha256,
            tool_policy_revision, max_sources, max_bytes, max_wall_seconds,
            cursor_before, cursor_after, scheduled_at)
           SELECT tenant_id, collection_id, space_id, output_vault_id, 'retry', id,
                  $4, $5, $5, bundle_id, profile_id, connector, source_classes,
                  allowed_domains, freshness_seconds, package_manifest_sha256,
                  recipe_asset_id, recipe_sha256, tool_policy_revision, max_sources,
                  max_bytes, max_wall_seconds, cursor_before, cursor_before, NOW()
           FROM knowledge_collection_runs
           WHERE tenant_id = $1 AND id = $2 AND revision = $3
             AND status IN ('succeeded','partial','failed','cancelled')
           ON CONFLICT DO NOTHING RETURNING {RUN_COLUMNS}"#
    ))
    .bind(actor.tenant_id)
    .bind(parent.id)
    .bind(expected_revision)
    .bind(service.user_id)
    .bind(actor.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (run, created) = if let Some(run) = inserted {
        sqlx::query(
            r#"INSERT INTO knowledge_collection_run_items
               (tenant_id, run_id, collection_id, position, locator, title, source_class,
                previous_source_id)
               SELECT tenant_id, $3, collection_id, position, locator, title, source_class,
                      COALESCE(source_id, previous_source_id)
               FROM knowledge_collection_run_items
               WHERE tenant_id = $1 AND run_id = $2 ORDER BY position"#,
        )
        .bind(actor.tenant_id)
        .bind(parent.id)
        .bind(run.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE knowledge_collections SET last_enqueued_at = NOW(), updated_at = NOW()
               WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(actor.tenant_id)
        .bind(parent.collection_id)
        .execute(&mut *tx)
        .await?;
        (run, true)
    } else {
        let active = sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
            r#"SELECT {RUN_COLUMNS} FROM knowledge_collection_runs
               WHERE tenant_id = $1 AND collection_id = $2
                 AND status IN ('queued','running')"#
        ))
        .bind(actor.tenant_id)
        .bind(parent.collection_id)
        .fetch_optional(&mut *tx)
        .await?;
        match active {
            Some(run) => (run, false),
            None => return Err(KnowledgeCollectionError::Conflict),
        }
    };
    tx.commit().await?;
    Ok(EnqueuedCollectionRun { run, created })
}

pub async fn get_run(
    pool: &PgPool,
    actor: SpaceActor,
    run_id: Uuid,
) -> Result<KnowledgeCollectionRunRow, KnowledgeCollectionError> {
    let row = find_run(pool, run_id)
        .await?
        .filter(|row| row.tenant_id == actor.tenant_id)
        .ok_or(KnowledgeCollectionError::NotFound)?;
    spaces::require_role(pool, actor, row.space_id, SpaceRole::Viewer, false).await?;
    Ok(row)
}

pub async fn list_runs(
    pool: &PgPool,
    actor: SpaceActor,
    collection_id: Uuid,
    limit: i64,
) -> Result<Vec<KnowledgeCollectionRunRow>, KnowledgeCollectionError> {
    let collection = get_collection(pool, actor, collection_id, SpaceRole::Viewer, false).await?;
    Ok(sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        r#"SELECT {RUN_COLUMNS} FROM knowledge_collection_runs
           WHERE tenant_id = $1 AND collection_id = $2
           ORDER BY created_at DESC, id DESC LIMIT $3"#
    ))
    .bind(actor.tenant_id)
    .bind(collection.id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?)
}

pub async fn run_items(
    pool: &PgPool,
    actor: SpaceActor,
    run_id: Uuid,
) -> Result<Vec<KnowledgeCollectionRunItemRow>, KnowledgeCollectionError> {
    let run = get_run(pool, actor, run_id).await?;
    Ok(sqlx::query_as::<_, KnowledgeCollectionRunItemRow>(&format!(
        "SELECT {ITEM_COLUMNS} FROM knowledge_collection_run_items WHERE tenant_id = $1 AND run_id = $2 ORDER BY position"
    ))
    .bind(actor.tenant_id)
    .bind(run.id)
    .fetch_all(pool)
    .await?)
}

pub async fn validate_execution_actor(
    pool: &PgPool,
    actor: SpaceActor,
    run: &KnowledgeCollectionRunRow,
) -> Result<(), KnowledgeCollectionError> {
    if actor.tenant_id != run.tenant_id || actor.user_id != run.on_behalf_of_user_id {
        return Err(KnowledgeCollectionError::InvalidInput(
            "collection execution actor does not match the pinned on-behalf-of user".to_string(),
        ));
    }
    service_principals::validate_knowledge_agent(pool, run.tenant_id, run.service_actor_user_id)
        .await?;
    require_output_vault(pool, actor, run.space_id, run.output_vault_id).await
}

pub async fn source_health(
    pool: &PgPool,
    actor: SpaceActor,
    collection_id: Uuid,
) -> Result<Vec<CollectionSourceHealthRow>, KnowledgeCollectionError> {
    let collection = get_collection(pool, actor, collection_id, SpaceRole::Viewer, false).await?;
    Ok(sqlx::query_as::<_, CollectionSourceHealthRow>(
        r#"SELECT locator, title, source_class,
                  CASE
                    WHEN status = 'deleted' THEN 'deleted'
                    WHEN status IN ('failed','skipped') THEN 'failed'
                    WHEN fresh_until IS NULL THEN 'never'
                    WHEN fresh_until > NOW() THEN 'current'
                    ELSE 'stale'
                  END AS health,
                  status AS observation_status, source_id, previous_source_id, content_hash,
                  byte_size, http_status, fetched_at, fresh_until, deletion_observed_at,
                  failure_code, failure_detail
           FROM (
             SELECT DISTINCT ON (locator) locator, title, source_class, status, source_id,
                    previous_source_id, content_hash, byte_size, http_status, fetched_at,
                    fresh_until, deletion_observed_at, failure_code, failure_detail,
                    updated_at, id
             FROM knowledge_collection_run_items
             WHERE tenant_id = $1 AND collection_id = $2
               AND status IN ('captured','unchanged','deleted','failed','skipped')
             ORDER BY locator, updated_at DESC, id DESC
           ) latest ORDER BY locator"#,
    )
    .bind(actor.tenant_id)
    .bind(collection.id)
    .fetch_all(pool)
    .await?)
}

fn run_control(run: &KnowledgeCollectionRunRow) -> CollectionRunControl {
    if run.cancel_requested_at.is_some() {
        return CollectionRunControl::CancelRequested;
    }
    if run.used_bytes >= run.max_bytes {
        return CollectionRunControl::ByteBudgetExceeded;
    }
    if run.started_at.is_some_and(|started| {
        Utc::now().signed_duration_since(started).num_seconds() >= i64::from(run.max_wall_seconds)
    }) {
        return CollectionRunControl::WallTimeExceeded;
    }
    CollectionRunControl::Continue
}

struct CollectionInput<'a> {
    topic: &'a str,
    source_classes: &'a [String],
    allowed_domains: &'a [String],
    schedule: Option<&'a str>,
    schedule_enabled: bool,
    next_run_at: Option<DateTime<Utc>>,
    max_sources: i32,
    max_bytes: i64,
    max_wall_seconds: i32,
    locators: &'a [CollectionLocator],
    queries: &'a [CollectionQuery],
}

fn validate_collection_input(input: CollectionInput<'_>) -> Result<(), KnowledgeCollectionError> {
    if input.topic.trim().is_empty() || input.topic.chars().count() > 500 {
        return Err(KnowledgeCollectionError::InvalidInput(
            "topic must contain 1..500 characters".to_string(),
        ));
    }
    if input.source_classes.is_empty() || input.allowed_domains.is_empty() {
        return Err(KnowledgeCollectionError::InvalidInput(
            "source classes and allowed domains cannot be empty".to_string(),
        ));
    }
    if !(1..=100).contains(&input.max_sources)
        || !(1..=1_073_741_824).contains(&input.max_bytes)
        || !(1..=3_600).contains(&input.max_wall_seconds)
        || input.locators.is_empty()
        || input.locators.len() > input.max_sources as usize
    {
        return Err(KnowledgeCollectionError::InvalidInput(
            "collection locators or budget are invalid".to_string(),
        ));
    }
    if input.schedule_enabled && (input.schedule.is_none() || input.next_run_at.is_none()) {
        return Err(KnowledgeCollectionError::InvalidInput(
            "enabled collection schedule requires an expression and next run".to_string(),
        ));
    }
    let mut urls = std::collections::BTreeSet::new();
    for locator in input.locators {
        if locator.url.len() > 4_096
            || !locator.url.starts_with("https://")
            || locator.title.len() > 512
            || !input
                .source_classes
                .iter()
                .any(|class| class == &locator.source_class)
            || !urls.insert(locator.url.as_str())
        {
            return Err(KnowledgeCollectionError::InvalidInput(
                "collection locator is invalid, duplicated, or uses an undeclared source class"
                    .to_string(),
            ));
        }
    }
    if !input.queries.is_empty() && input.queries.len() != input.locators.len() {
        return Err(KnowledgeCollectionError::InvalidInput(
            "provider queries must map one-to-one to generated collection locators".to_string(),
        ));
    }
    let mut providers = std::collections::BTreeSet::new();
    for query in input.queries {
        let provider_valid = !query.provider.is_empty()
            && query.provider.len() <= 64
            && query.provider.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit() && index > 0
                    || byte == b'-' && index > 0
            });
        if !provider_valid
            || !providers.insert(query.provider.as_str())
            || query.query.trim().is_empty()
            || query.query.chars().count() > 500
            || query.scope.trim().is_empty()
            || query.scope.len() > 256
            || query.tags.len() > 8
            || query
                .tags
                .iter()
                .any(|tag| tag.trim().is_empty() || tag.len() > 63)
            || query
                .language
                .as_ref()
                .is_some_and(|language| language.trim().is_empty() || language.len() > 63)
            || !(1..=3_650).contains(&query.window_days)
        {
            return Err(KnowledgeCollectionError::InvalidInput(
                "collection provider query is invalid or duplicated".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_item_result(result: &CompleteCollectionItem) -> Result<(), KnowledgeCollectionError> {
    if !matches!(
        result.status.as_str(),
        "captured" | "unchanged" | "deleted" | "failed"
    ) || result
        .byte_size
        .is_some_and(|size| !(0..=16_777_216).contains(&size))
        || result
            .http_status
            .is_some_and(|status| !(100..=599).contains(&status))
    {
        return Err(KnowledgeCollectionError::InvalidInput(
            "collection item result is invalid".to_string(),
        ));
    }
    match result.status.as_str() {
        "captured" | "unchanged"
            if result.source_id.is_none()
                || result.content_hash.is_none()
                || result.byte_size.is_none()
                || result.fetched_at.is_none()
                || result.fresh_until.is_none() =>
        {
            Err(KnowledgeCollectionError::InvalidInput(
                "successful collection item is missing snapshot provenance".to_string(),
            ))
        }
        "deleted" if result.deletion_observed_at.is_none() => {
            Err(KnowledgeCollectionError::InvalidInput(
                "deleted collection item is missing observation time".to_string(),
            ))
        }
        "failed" if result.failure_code.is_none() => Err(KnowledgeCollectionError::InvalidInput(
            "failed collection item is missing a failure code".to_string(),
        )),
        _ => Ok(()),
    }
}

async fn require_output_vault(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
    vault_id: Uuid,
) -> Result<(), KnowledgeCollectionError> {
    spaces::require_role(pool, actor, space_id, SpaceRole::Contributor, true).await?;
    let valid: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM knowledge_vaults
           WHERE tenant_id = $1 AND id = $2 AND space_id = $3 AND owner_state = 'enabled')"#,
    )
    .bind(actor.tenant_id)
    .bind(vault_id)
    .bind(space_id)
    .fetch_one(pool)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(KnowledgeCollectionError::InvalidInput(
            "output Vault is not active in the selected Space".to_string(),
        ))
    }
}

async fn find_collection(
    pool: &PgPool,
    tenant_id: Uuid,
    collection_id: Uuid,
) -> Result<Option<KnowledgeCollectionRow>, sqlx::Error> {
    sqlx::query_as::<_, KnowledgeCollectionRow>(&format!(
        "SELECT {COLLECTION_COLUMNS} FROM knowledge_collections WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL"
    ))
    .bind(tenant_id)
    .bind(collection_id)
    .fetch_optional(pool)
    .await
}

async fn find_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Option<KnowledgeCollectionRunRow>, sqlx::Error> {
    sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        "SELECT {RUN_COLUMNS} FROM knowledge_collection_runs WHERE id = $1"
    ))
    .bind(run_id)
    .fetch_optional(pool)
    .await
}

async fn find_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<KnowledgeCollectionRunRow>, sqlx::Error> {
    sqlx::query_as::<_, KnowledgeCollectionRunRow>(&format!(
        "SELECT {RUN_COLUMNS} FROM knowledge_collection_runs WHERE id = $1 FOR UPDATE"
    ))
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await
}

async fn resolve_collection_cas(
    pool: &PgPool,
    tenant_id: Uuid,
    collection_id: Uuid,
    row: Option<KnowledgeCollectionRow>,
) -> Result<KnowledgeCollectionRow, KnowledgeCollectionError> {
    if let Some(row) = row {
        return Ok(row);
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM knowledge_collections WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant_id)
    .bind(collection_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Err(KnowledgeCollectionError::Conflict)
    } else {
        Err(KnowledgeCollectionError::NotFound)
    }
}

async fn resolve_run_cas(
    pool: &PgPool,
    tenant_id: Uuid,
    run_id: Uuid,
    row: Option<KnowledgeCollectionRunRow>,
) -> Result<KnowledgeCollectionRunRow, KnowledgeCollectionError> {
    if let Some(row) = row {
        return Ok(row);
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM knowledge_collection_runs WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant_id)
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Err(KnowledgeCollectionError::Conflict)
    } else {
        Err(KnowledgeCollectionError::NotFound)
    }
}

fn truncate_detail(detail: &str) -> String {
    detail.chars().take(1_024).collect()
}

fn map_constraint(error: sqlx::Error) -> KnowledgeCollectionError {
    match &error {
        sqlx::Error::Database(database) if matches!(database.code().as_deref(), Some("23505")) => {
            KnowledgeCollectionError::Conflict
        }
        sqlx::Error::Database(database)
            if matches!(database.code().as_deref(), Some("23503" | "23514")) =>
        {
            KnowledgeCollectionError::InvalidInput(database.message().to_string())
        }
        _ => KnowledgeCollectionError::Database(error),
    }
}
