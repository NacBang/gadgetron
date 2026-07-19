//! R2.2 PostgreSQL Source Ledger and note revision registry.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::knowledge_spaces::{
    require_role, KnowledgeObjectRow, KnowledgeSpaceError, KnowledgeVaultRow, SpaceActor, SpaceRole,
};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeSourceRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub vault_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub source_kind: String,
    pub status: String,
    pub title: String,
    pub original_name: String,
    pub requested_uri: Option<String>,
    pub final_uri: Option<String>,
    pub content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub content_hash: Option<String>,
    pub blob_id: Option<Uuid>,
    pub extracted_object_id: Option<Uuid>,
    pub failure_code: Option<String>,
    pub failure_detail: Option<String>,
    pub attempt_count: i32,
    pub created_by: Uuid,
    pub revision: i64,
    pub fetched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeSourceAttemptRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub source_id: Uuid,
    pub attempt_no: i32,
    pub phase: String,
    pub outcome: String,
    pub final_uri: Option<String>,
    pub http_status: Option<i32>,
    pub content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub content_hash: Option<String>,
    pub failure_code: Option<String>,
    pub failure_detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateSource {
    pub vault_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub source_kind: String,
    pub title: String,
    pub original_name: String,
    pub requested_uri: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AttachSourceBlob {
    pub blob_id: Uuid,
    pub content_type: String,
    pub byte_size: i64,
    pub content_hash: String,
    pub final_uri: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SourceAttempt {
    pub source_id: Uuid,
    pub attempt_no: i32,
    pub phase: String,
    pub outcome: String,
    pub final_uri: Option<String>,
    pub http_status: Option<i32>,
    pub content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub content_hash: Option<String>,
    pub failure_code: Option<String>,
    pub failure_detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MaterializeIncidentSnapshot {
    pub source_id: Uuid,
    pub object_id: Uuid,
    pub vault_id: Uuid,
    pub title: String,
    pub original_name: String,
    pub final_uri: String,
    pub content_type: String,
    pub byte_size: i64,
    pub content_hash: String,
    pub blob_id: Uuid,
    pub path: String,
    pub note_content_hash: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct NoteObjectLocation {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub vault_id: Uuid,
    pub source_id: Option<Uuid>,
    pub path: String,
    pub status: String,
    pub content_hash: Option<String>,
    pub revision: i64,
    pub space_id: Uuid,
    pub home_bundle_id: String,
}

pub async fn create_pending_source(
    pool: &PgPool,
    actor: SpaceActor,
    request: CreateSource,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    let vault = load_vault(pool, actor.tenant_id, request.vault_id).await?;
    require_role(pool, actor, vault.space_id, SpaceRole::Contributor, true).await?;
    if vault.owner_state != "enabled" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    if !matches!(
        request.source_kind.as_str(),
        "upload" | "article" | "social_snapshot" | "chat_attachment" | "incident_snapshot"
    ) {
        return Err(KnowledgeSpaceError::InvalidInput(
            "source_kind must be upload, article, social_snapshot, chat_attachment, or incident_snapshot"
                .to_string(),
        ));
    }
    if matches!(request.source_kind.as_str(), "article" | "social_snapshot")
        && request.requested_uri.is_none()
    {
        return Err(KnowledgeSpaceError::InvalidInput(
            "URL source requires requested_uri".to_string(),
        ));
    }
    if request.source_kind == "chat_attachment" && request.conversation_id.is_none() {
        return Err(KnowledgeSpaceError::InvalidInput(
            "chat attachment requires conversation_id".to_string(),
        ));
    }
    sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"INSERT INTO knowledge_sources
           (tenant_id, vault_id, conversation_id, source_kind, title, original_name,
            requested_uri, attempt_count, created_by)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 1, $8)
           RETURNING {}"#,
        source_columns()
    ))
    .bind(actor.tenant_id)
    .bind(request.vault_id)
    .bind(request.conversation_id)
    .bind(request.source_kind)
    .bind(request.title)
    .bind(request.original_name)
    .bind(request.requested_uri)
    .bind(actor.user_id)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)
}

/// Atomically register a fully materialized incident snapshot after its blob
/// and Git note have been durably written. A failed transaction leaves no
/// partial Source/Object rows; the content-addressed blob may remain safely
/// deduplicated for a later retry.
pub async fn materialize_incident_snapshot(
    pool: &PgPool,
    actor: SpaceActor,
    request: MaterializeIncidentSnapshot,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    let vault = load_vault(pool, actor.tenant_id, request.vault_id).await?;
    require_role(pool, actor, vault.space_id, SpaceRole::Contributor, true).await?;
    if vault.owner_state != "enabled" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    if request.title.trim().is_empty()
        || request.content_type != "application/json"
        || request.byte_size < 0
        || !request.content_hash.starts_with("sha256:")
        || request.final_uri.is_empty()
        || request.path.is_empty()
        || request.note_content_hash.len() != 64
    {
        return Err(KnowledgeSpaceError::InvalidInput(
            "incident snapshot material is invalid".to_string(),
        ));
    }
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"INSERT INTO knowledge_sources
           (id, tenant_id, vault_id, source_kind, status, title, original_name,
            final_uri, content_type, byte_size, content_hash, blob_id,
            attempt_count, created_by, fetched_at)
           VALUES ($1,$2,$3,'incident_snapshot','pending',$4,$5,$6,$7,$8,$9,$10,1,$11,NOW())"#,
    )
    .bind(request.source_id)
    .bind(actor.tenant_id)
    .bind(request.vault_id)
    .bind(&request.title)
    .bind(&request.original_name)
    .bind(&request.final_uri)
    .bind(&request.content_type)
    .bind(request.byte_size)
    .bind(&request.content_hash)
    .bind(request.blob_id)
    .bind(actor.user_id)
    .execute(&mut *transaction)
    .await
    .map_err(map_constraint)?;
    sqlx::query(
        r#"INSERT INTO knowledge_objects
           (id, tenant_id, vault_id, source_id, canonical_kind, path,
            content_hash, created_by)
           VALUES ($1,$2,$3,$4,'note',$5,$6,$7)"#,
    )
    .bind(request.object_id)
    .bind(actor.tenant_id)
    .bind(request.vault_id)
    .bind(request.source_id)
    .bind(&request.path)
    .bind(&request.note_content_hash)
    .bind(actor.user_id)
    .execute(&mut *transaction)
    .await
    .map_err(map_constraint)?;
    let source = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"UPDATE knowledge_sources SET
             status = 'extracted', extracted_object_id = $3,
             revision = revision + 1, updated_at = NOW()
           WHERE tenant_id = $1 AND id = $2 AND status = 'pending'
           RETURNING {}"#,
        source_columns()
    ))
    .bind(actor.tenant_id)
    .bind(request.source_id)
    .bind(request.object_id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        r#"INSERT INTO knowledge_source_attempts
           (tenant_id, source_id, attempt_no, phase, outcome, final_uri,
            content_type, byte_size, content_hash)
           VALUES ($1,$2,1,'extract','succeeded',$3,$4,$5,$6)"#,
    )
    .bind(actor.tenant_id)
    .bind(request.source_id)
    .bind(request.final_uri)
    .bind(request.content_type)
    .bind(request.byte_size)
    .bind(request.content_hash)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(source)
}

pub async fn attach_source_blob(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    expected_revision: i64,
    blob: AttachSourceBlob,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    let current = get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    if current.revision != expected_revision || current.status != "pending" {
        return Err(KnowledgeSpaceError::RevisionConflict);
    }
    let row = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"UPDATE knowledge_sources SET
             blob_id = $4, content_type = $5, byte_size = $6, content_hash = $7,
             final_uri = $8, fetched_at = CASE WHEN requested_uri IS NOT NULL THEN NOW() ELSE fetched_at END,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND status = 'pending'
           RETURNING {}"#,
        source_columns()
    ))
    .bind(source_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .bind(blob.blob_id)
    .bind(blob.content_type)
    .bind(blob.byte_size)
    .bind(blob.content_hash)
    .bind(blob.final_uri)
    .fetch_optional(pool)
    .await?;
    resolve_source_cas(pool, actor.tenant_id, source_id, row).await
}

pub async fn complete_source(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    expected_revision: i64,
    object_id: Uuid,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    let current = get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    if current.revision != expected_revision || current.blob_id.is_none() {
        return Err(KnowledgeSpaceError::RevisionConflict);
    }
    let row = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"UPDATE knowledge_sources SET
             status = 'extracted', extracted_object_id = $4,
             failure_code = NULL, failure_detail = NULL,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND status = 'pending'
           RETURNING {}"#,
        source_columns()
    ))
    .bind(source_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .bind(object_id)
    .fetch_optional(pool)
    .await?;
    resolve_source_cas(pool, actor.tenant_id, source_id, row).await
}

pub async fn fail_source(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    expected_revision: i64,
    needs_ocr: bool,
    failure_code: &str,
    failure_detail: &str,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    let status = if needs_ocr { "needs_ocr" } else { "failed" };
    let row = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"UPDATE knowledge_sources SET
             status = $4, failure_code = $5, failure_detail = $6,
             revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND status = 'pending'
           RETURNING {}"#,
        source_columns()
    ))
    .bind(source_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .bind(status)
    .bind(failure_code)
    .bind(truncate_detail(failure_detail))
    .fetch_optional(pool)
    .await?;
    resolve_source_cas(pool, actor.tenant_id, source_id, row).await
}

pub async fn begin_retry(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    let row = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"UPDATE knowledge_sources SET
             status = 'pending', failure_code = NULL, failure_detail = NULL,
             attempt_count = attempt_count + 1, revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3
             AND status IN ('failed', 'needs_ocr')
           RETURNING {}"#,
        source_columns()
    ))
    .bind(source_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .fetch_optional(pool)
    .await?;
    resolve_source_cas(pool, actor.tenant_id, source_id, row).await
}

pub async fn delete_source(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    let row = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"UPDATE knowledge_sources SET
             status = 'deleted', deleted_at = NOW(), revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3 AND deleted_at IS NULL
           RETURNING {}"#,
        source_columns()
    ))
    .bind(source_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .fetch_optional(pool)
    .await?;
    resolve_source_cas(pool, actor.tenant_id, source_id, row).await
}

pub async fn purge_purgeable_source(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    let current = get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    if !matches!(
        current.source_kind.as_str(),
        "social_snapshot" | "chat_attachment"
    ) {
        return Err(KnowledgeSpaceError::InvalidInput(
            "hard purge is limited to purgeable sources".to_string(),
        ));
    }
    let mut transaction = pool.begin().await?;
    let row = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"UPDATE knowledge_sources SET
             status = 'deleted', title = 'Purged source', original_name = '',
             requested_uri = NULL, final_uri = NULL, content_type = NULL,
             byte_size = NULL, content_hash = NULL, blob_id = NULL,
             extracted_object_id = NULL, failure_code = NULL, failure_detail = NULL,
             fetched_at = NULL, deleted_at = NOW(), revision = revision + 1,
             updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3
             AND source_kind IN ('social_snapshot', 'chat_attachment') AND deleted_at IS NULL
           RETURNING {}"#,
        source_columns()
    ))
    .bind(source_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return resolve_source_cas(pool, actor.tenant_id, source_id, None).await;
    };
    sqlx::query(
        r#"UPDATE knowledge_source_attempts SET
             final_uri = NULL, content_type = NULL, byte_size = NULL,
             content_hash = NULL, failure_detail = NULL
           WHERE tenant_id = $1 AND source_id = $2"#,
    )
    .bind(actor.tenant_id)
    .bind(source_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(row)
}

pub async fn live_blob_reference_count(
    pool: &PgPool,
    tenant_id: Uuid,
    blob_id: Uuid,
) -> Result<i64, KnowledgeSpaceError> {
    Ok(sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM knowledge_sources
           WHERE tenant_id = $1 AND blob_id = $2 AND deleted_at IS NULL"#,
    )
    .bind(tenant_id)
    .bind(blob_id)
    .fetch_one(pool)
    .await?)
}

pub async fn record_attempt(
    pool: &PgPool,
    tenant_id: Uuid,
    attempt: SourceAttempt,
) -> Result<KnowledgeSourceAttemptRow, KnowledgeSpaceError> {
    sqlx::query_as::<_, KnowledgeSourceAttemptRow>(
        r#"INSERT INTO knowledge_source_attempts
           (tenant_id, source_id, attempt_no, phase, outcome, final_uri, http_status,
            content_type, byte_size, content_hash, failure_code, failure_detail)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
           RETURNING id, tenant_id, source_id, attempt_no, phase, outcome, final_uri,
                     http_status, content_type, byte_size, content_hash, failure_code,
                     failure_detail, created_at"#,
    )
    .bind(tenant_id)
    .bind(attempt.source_id)
    .bind(attempt.attempt_no)
    .bind(attempt.phase)
    .bind(attempt.outcome)
    .bind(attempt.final_uri)
    .bind(attempt.http_status)
    .bind(attempt.content_type)
    .bind(attempt.byte_size)
    .bind(attempt.content_hash)
    .bind(attempt.failure_code)
    .bind(
        attempt
            .failure_detail
            .map(|detail| truncate_detail(&detail)),
    )
    .fetch_one(pool)
    .await
    .map_err(map_constraint)
}

pub async fn get_source(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    required: SpaceRole,
    require_active: bool,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    let row = sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        "SELECT {} FROM knowledge_sources WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
        source_columns()
    ))
    .bind(source_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    let space_id: Uuid = sqlx::query_scalar(
        "SELECT space_id FROM knowledge_vaults WHERE id = $1 AND tenant_id = $2",
    )
    .bind(row.vault_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    require_role(pool, actor, space_id, required, require_active).await?;
    Ok(row)
}

pub async fn list_sources(
    pool: &PgPool,
    actor: SpaceActor,
    space_id: Uuid,
) -> Result<Vec<KnowledgeSourceRow>, KnowledgeSpaceError> {
    require_role(pool, actor, space_id, SpaceRole::Viewer, false).await?;
    Ok(sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"SELECT {} FROM knowledge_sources s
           JOIN knowledge_vaults v ON v.id = s.vault_id AND v.tenant_id = s.tenant_id
           WHERE s.tenant_id = $1 AND v.space_id = $2 AND s.deleted_at IS NULL
           ORDER BY s.created_at DESC, s.id"#,
        prefixed_source_columns("s")
    ))
    .bind(actor.tenant_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?)
}

/// List active Sources explicitly linked to a conversation owned by `actor`.
/// Conversation ownership is checked in the same query so a leaked UUID never
/// becomes an attachment-discovery channel across users or tenants.
pub async fn list_conversation_sources(
    pool: &PgPool,
    actor: SpaceActor,
    conversation_id: Uuid,
) -> Result<Vec<KnowledgeSourceRow>, KnowledgeSpaceError> {
    Ok(sqlx::query_as::<_, KnowledgeSourceRow>(&format!(
        r#"SELECT {} FROM knowledge_sources s
           JOIN conversations c ON c.id = s.conversation_id
           WHERE s.tenant_id = $1 AND s.conversation_id = $2
             AND c.tenant_id = $1 AND c.user_id = $3 AND c.deleted_at IS NULL
             AND s.deleted_at IS NULL
           ORDER BY s.created_at, s.id"#,
        prefixed_source_columns("s")
    ))
    .bind(actor.tenant_id)
    .bind(conversation_id)
    .bind(actor.user_id)
    .fetch_all(pool)
    .await?)
}

/// Load citation-ready attachment revisions for deterministic Penny pinning.
pub async fn list_ready_conversation_sources(
    pool: &PgPool,
    actor: SpaceActor,
    conversation_id: Uuid,
) -> Result<Vec<(KnowledgeSourceRow, KnowledgeObjectRow)>, KnowledgeSpaceError> {
    let sources = list_conversation_sources(pool, actor, conversation_id).await?;
    let mut ready = Vec::new();
    for source in sources.into_iter().rev() {
        if ready.len() == 16 {
            break;
        }
        if source.status != "extracted" {
            continue;
        }
        if let Some(location) = source_object(pool, actor, source.id).await? {
            if location.status == "active" {
                ready.push((source, location));
            }
        }
    }
    ready.reverse();
    Ok(ready)
}

pub async fn source_attempts(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
) -> Result<Vec<KnowledgeSourceAttemptRow>, KnowledgeSpaceError> {
    get_source(pool, actor, source_id, SpaceRole::Viewer, false).await?;
    Ok(sqlx::query_as(
        r#"SELECT id, tenant_id, source_id, attempt_no, phase, outcome, final_uri,
                  http_status, content_type, byte_size, content_hash, failure_code,
                  failure_detail, created_at
           FROM knowledge_source_attempts
           WHERE tenant_id = $1 AND source_id = $2
           ORDER BY attempt_no, created_at, phase"#,
    )
    .bind(actor.tenant_id)
    .bind(source_id)
    .fetch_all(pool)
    .await?)
}

pub async fn source_vault(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    required: SpaceRole,
    require_active: bool,
) -> Result<KnowledgeVaultRow, KnowledgeSpaceError> {
    let source = get_source(pool, actor, source_id, required, require_active).await?;
    load_vault(pool, actor.tenant_id, source.vault_id).await
}

pub async fn register_source_object(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    object_id: Uuid,
    path: &str,
    content_hash: &str,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    bind_source_object(pool, actor, source_id, object_id, path, content_hash, false).await
}

/// Return the registered object at an exact Vault note path, if any.
///
/// The lookup is tenant- and ACL-pinned so callers can choose the stable
/// object id before composing YAML frontmatter. Tombstones remain visible to
/// this write-side lookup because the `(vault_id, path)` uniqueness contract
/// still owns their path.
pub async fn source_object_at_path(
    pool: &PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
    path: &str,
) -> Result<Option<KnowledgeObjectRow>, KnowledgeSpaceError> {
    gadgetron_core::knowledge::KnowledgePath::new(path).map_err(|error| {
        KnowledgeSpaceError::InvalidInput(format!("invalid source note path: {error}"))
    })?;
    let vault = load_vault(pool, actor.tenant_id, vault_id).await?;
    require_role(pool, actor, vault.space_id, SpaceRole::Contributor, true).await?;
    Ok(sqlx::query_as::<_, KnowledgeObjectRow>(
        r#"SELECT id, tenant_id, vault_id, canonical_kind, path, status,
                  content_hash, created_by, revision, created_at, updated_at
           FROM knowledge_objects
           WHERE tenant_id = $1 AND vault_id = $2 AND path = $3"#,
    )
    .bind(actor.tenant_id)
    .bind(vault_id)
    .bind(path)
    .fetch_optional(pool)
    .await?)
}

/// Bind a pending Source to an exact Vault note path.
///
/// `overwrite = false` preserves create-only behavior. With explicit
/// overwrite, the stable object id/path is retained while its current Source
/// provenance and content revision advance to the newly imported Source.
#[allow(clippy::too_many_arguments)]
pub async fn bind_source_object(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
    object_id: Uuid,
    path: &str,
    content_hash: &str,
    overwrite: bool,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    gadgetron_core::knowledge::KnowledgePath::new(path).map_err(|error| {
        KnowledgeSpaceError::InvalidInput(format!("invalid source note path: {error}"))
    })?;
    let source = get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    if source.status != "pending" || source.blob_id.is_none() {
        return Err(KnowledgeSpaceError::RevisionConflict);
    }
    if let Some(existing) = source_object(pool, actor, source_id).await? {
        return if existing.path == path {
            Ok(existing)
        } else {
            Err(KnowledgeSpaceError::Conflict)
        };
    }
    if let Some(existing) = source_object_at_path(pool, actor, source.vault_id, path).await? {
        if !overwrite {
            return Err(KnowledgeSpaceError::Conflict);
        }
        return sqlx::query_as::<_, KnowledgeObjectRow>(
            r#"UPDATE knowledge_objects SET
                 source_id = $4, content_hash = $5, status = 'active',
                 revision = revision + 1, updated_at = NOW()
               WHERE id = $1 AND tenant_id = $2 AND vault_id = $3
               RETURNING id, tenant_id, vault_id, canonical_kind, path, status,
                         content_hash, created_by, revision, created_at, updated_at"#,
        )
        .bind(existing.id)
        .bind(actor.tenant_id)
        .bind(source.vault_id)
        .bind(source_id)
        .bind(content_hash)
        .fetch_one(pool)
        .await
        .map_err(map_constraint);
    }
    sqlx::query_as::<_, KnowledgeObjectRow>(
        r#"INSERT INTO knowledge_objects
           (id, tenant_id, vault_id, source_id, canonical_kind, path, content_hash, created_by)
           VALUES ($1, $2, $3, $4, 'note', $5, $6, $7)
           RETURNING id, tenant_id, vault_id, canonical_kind, path, status,
                     content_hash, created_by, revision, created_at, updated_at"#,
    )
    .bind(object_id)
    .bind(actor.tenant_id)
    .bind(source.vault_id)
    .bind(source_id)
    .bind(path)
    .bind(content_hash)
    .bind(actor.user_id)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)
}

pub async fn register_manual_note(
    pool: &PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
    object_id: Uuid,
    path: &str,
    content_hash: Option<&str>,
) -> Result<(KnowledgeObjectRow, KnowledgeVaultRow), KnowledgeSpaceError> {
    gadgetron_core::knowledge::KnowledgePath::new(path).map_err(|error| {
        KnowledgeSpaceError::InvalidInput(format!("invalid knowledge object path: {error}"))
    })?;
    let vault = load_vault(pool, actor.tenant_id, vault_id).await?;
    require_role(pool, actor, vault.space_id, SpaceRole::Contributor, true).await?;
    if vault.owner_state != "enabled" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    let object = sqlx::query_as::<_, KnowledgeObjectRow>(
        r#"INSERT INTO knowledge_objects
           (id, tenant_id, vault_id, canonical_kind, path, content_hash, created_by)
           VALUES ($1, $2, $3, 'note', $4, $5, $6)
           RETURNING id, tenant_id, vault_id, canonical_kind, path, status,
                     content_hash, created_by, revision, created_at, updated_at"#,
    )
    .bind(object_id)
    .bind(actor.tenant_id)
    .bind(vault_id)
    .bind(path)
    .bind(content_hash)
    .bind(actor.user_id)
    .fetch_one(pool)
    .await
    .map_err(map_constraint)?;
    Ok((object, vault))
}

pub async fn manual_note_vault(
    pool: &PgPool,
    actor: SpaceActor,
    vault_id: Uuid,
) -> Result<KnowledgeVaultRow, KnowledgeSpaceError> {
    let vault = load_vault(pool, actor.tenant_id, vault_id).await?;
    require_role(pool, actor, vault.space_id, SpaceRole::Contributor, true).await?;
    if vault.owner_state != "enabled" {
        return Err(KnowledgeSpaceError::Forbidden);
    }
    Ok(vault)
}

pub async fn source_object(
    pool: &PgPool,
    actor: SpaceActor,
    source_id: Uuid,
) -> Result<Option<KnowledgeObjectRow>, KnowledgeSpaceError> {
    let source = get_source(pool, actor, source_id, SpaceRole::Contributor, true).await?;
    Ok(sqlx::query_as::<_, KnowledgeObjectRow>(
        r#"SELECT id, tenant_id, vault_id, canonical_kind, path, status,
                  content_hash, created_by, revision, created_at, updated_at
           FROM knowledge_objects
           WHERE tenant_id = $1 AND vault_id = $2 AND source_id = $3
             AND status <> 'tombstone'"#,
    )
    .bind(actor.tenant_id)
    .bind(source.vault_id)
    .bind(source_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn note_location(
    pool: &PgPool,
    actor: SpaceActor,
    object_id: Uuid,
    required: SpaceRole,
    require_active: bool,
) -> Result<NoteObjectLocation, KnowledgeSpaceError> {
    let row = sqlx::query_as::<_, NoteObjectLocation>(
        r#"SELECT o.id, o.tenant_id, o.vault_id, o.source_id, o.path, o.status,
                  o.content_hash, o.revision, v.space_id, v.home_bundle_id
           FROM knowledge_objects o
           JOIN knowledge_vaults v ON v.id = o.vault_id AND v.tenant_id = o.tenant_id
           WHERE o.id = $1 AND o.tenant_id = $2 AND o.canonical_kind = 'note'
             AND o.status = 'active'"#,
    )
    .bind(object_id)
    .bind(actor.tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)?;
    require_role(pool, actor, row.space_id, required, require_active).await?;
    Ok(row)
}

pub async fn update_note_hash(
    pool: &PgPool,
    actor: SpaceActor,
    object_id: Uuid,
    expected_revision: i64,
    content_hash: &str,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    note_location(pool, actor, object_id, SpaceRole::Contributor, true).await?;
    update_note_hash_inner(
        pool,
        actor.tenant_id,
        object_id,
        expected_revision,
        content_hash,
    )
    .await
}

pub async fn update_note_hash_system(
    pool: &PgPool,
    tenant_id: Uuid,
    object_id: Uuid,
    expected_revision: i64,
    content_hash: &str,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    update_note_hash_inner(pool, tenant_id, object_id, expected_revision, content_hash).await
}

pub async fn delete_note(
    pool: &PgPool,
    actor: SpaceActor,
    object_id: Uuid,
    expected_revision: i64,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    note_location(pool, actor, object_id, SpaceRole::Contributor, true).await?;
    let row = sqlx::query_as::<_, KnowledgeObjectRow>(
        r#"UPDATE knowledge_objects SET
             status = 'tombstone', revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3
             AND canonical_kind = 'note' AND status = 'active'
           RETURNING id, tenant_id, vault_id, canonical_kind, path, status,
                     content_hash, created_by, revision, created_at, updated_at"#,
    )
    .bind(object_id)
    .bind(actor.tenant_id)
    .bind(expected_revision)
    .fetch_optional(pool)
    .await?;
    resolve_object_cas(pool, actor.tenant_id, object_id, row).await
}

async fn update_note_hash_inner(
    pool: &PgPool,
    tenant_id: Uuid,
    object_id: Uuid,
    expected_revision: i64,
    content_hash: &str,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    if content_hash.len() != 64
        || !content_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(KnowledgeSpaceError::InvalidInput(
            "note content hash must be 64 lowercase hex characters".to_string(),
        ));
    }
    let row = sqlx::query_as::<_, KnowledgeObjectRow>(
        r#"UPDATE knowledge_objects SET
             content_hash = $4, revision = revision + 1, updated_at = NOW()
           WHERE id = $1 AND tenant_id = $2 AND revision = $3
             AND canonical_kind = 'note' AND status = 'active'
           RETURNING id, tenant_id, vault_id, canonical_kind, path, status,
                     content_hash, created_by, revision, created_at, updated_at"#,
    )
    .bind(object_id)
    .bind(tenant_id)
    .bind(expected_revision)
    .bind(content_hash)
    .fetch_optional(pool)
    .await?;
    resolve_object_cas(pool, tenant_id, object_id, row).await
}

async fn load_vault(
    pool: &PgPool,
    tenant_id: Uuid,
    vault_id: Uuid,
) -> Result<KnowledgeVaultRow, KnowledgeSpaceError> {
    sqlx::query_as(
        r#"SELECT id, tenant_id, space_id, home_bundle_id, knowledge_schema_id,
                  schema_version, owner_state, revision, created_at, updated_at
           FROM knowledge_vaults WHERE id = $1 AND tenant_id = $2"#,
    )
    .bind(vault_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeSpaceError::NotFound)
}

async fn resolve_source_cas(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    row: Option<KnowledgeSourceRow>,
) -> Result<KnowledgeSourceRow, KnowledgeSpaceError> {
    if let Some(row) = row {
        return Ok(row);
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM knowledge_sources WHERE id = $1 AND tenant_id = $2)",
    )
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

async fn resolve_object_cas(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    row: Option<KnowledgeObjectRow>,
) -> Result<KnowledgeObjectRow, KnowledgeSpaceError> {
    if let Some(row) = row {
        return Ok(row);
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM knowledge_objects WHERE id = $1 AND tenant_id = $2)",
    )
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

fn source_columns() -> &'static str {
    "id, tenant_id, vault_id, conversation_id, source_kind, status, title, original_name, requested_uri, \
     final_uri, content_type, byte_size, content_hash, blob_id, extracted_object_id, \
     failure_code, failure_detail, attempt_count, created_by, revision, fetched_at, \
     created_at, updated_at, deleted_at"
}

fn prefixed_source_columns(prefix: &str) -> String {
    source_columns()
        .split(", ")
        .map(|column| format!("{prefix}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn truncate_detail(value: &str) -> String {
    value.chars().take(1024).collect()
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
