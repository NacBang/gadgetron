//! R2.2 Source-to-Vault HTTP orchestration.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{header, HeaderValue, Response};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gadgetron_core::context::TenantContext;
use gadgetron_core::ingest::{BlobId, BlobMetadata, BlobStore};
use gadgetron_knowledge::source::{
    extract_source, parse_obsidian_note, serialize_obsidian_note, ExtractedSource,
    FilesystemBlobStore, NoteFrontmatterFormat, MAX_SOURCE_BYTES,
};
use gadgetron_knowledge::vault::{note_relative_path, VaultNoteRevisionWrite};
use gadgetron_xaas::knowledge_sources::{self as sources, AttachSourceBlob, CreateSource};
use gadgetron_xaas::knowledge_spaces::{
    EnsureVault, KnowledgeObjectRow, KnowledgeSpaceError, SpaceActor, SpaceRole,
};
use gadgetron_xaas::{conversations, knowledge_spaces as spaces};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::server::AppState;

use super::safe_fetch::{fetch_public_https, SafeFetchPolicy, SafeFetchResponse};
use super::workbench::{GatewayWorkbenchService, WorkbenchHttpError};

const MULTIPART_OVERHEAD_BYTES: usize = 64 * 1024;
const SOURCE_CONTENT_TYPES: &[&str] = &[
    "text/markdown",
    "text/plain",
    "text/html",
    "application/xhtml+xml",
    "application/json",
    "application/pdf",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/knowledge/vaults/{vault_id}/sources/fetch",
            post(fetch_source_handler),
        )
        .route(
            "/knowledge/spaces/{space_id}/sources",
            get(list_sources_handler),
        )
        .route(
            "/knowledge/sources/{source_id}",
            get(get_source_handler).delete(delete_source_handler),
        )
        .route(
            "/knowledge/sources/{source_id}/blob",
            get(get_source_blob_handler),
        )
        .route(
            "/knowledge/sources/{source_id}/retry",
            post(retry_source_handler),
        )
        .route(
            "/knowledge/conversations/{conversation_id}/attachments",
            get(list_chat_attachments_handler).delete(purge_chat_attachments_handler),
        )
        .route(
            "/knowledge/conversations/{conversation_id}/attachments/fetch",
            post(fetch_chat_attachment_handler),
        )
        .route(
            "/knowledge/conversations/{conversation_id}/attachments/{source_id}/promote",
            post(promote_chat_attachment_handler),
        )
        .route(
            "/knowledge/conversations/{conversation_id}/attachments/{source_id}",
            axum::routing::delete(delete_chat_attachment_handler),
        )
        .route(
            "/knowledge/objects/{object_id}/note",
            get(get_note_handler)
                .put(put_note_handler)
                .delete(delete_note_handler),
        )
        .route(
            "/knowledge/vaults/{vault_id}/notes",
            post(create_note_handler),
        )
}

pub fn upload_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/knowledge/vaults/{vault_id}/sources/upload",
            post(upload_source_handler),
        )
        .route(
            "/knowledge/conversations/{conversation_id}/attachments/upload",
            post(upload_chat_attachment_handler),
        )
        .layer(DefaultBodyLimit::max(
            MAX_SOURCE_BYTES + MULTIPART_OVERHEAD_BYTES,
        ))
}

#[derive(Debug, Deserialize)]
pub struct FetchSourceRequest {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub(crate) struct CaptureUrlSource {
    pub vault_id: Uuid,
    pub url: String,
    pub title: String,
    pub max_bytes: usize,
    pub allowed_domains: Option<Vec<String>>,
    pub timeout: Duration,
    pub retention: SourceRetention,
    pub conversation_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceRetention {
    Versioned,
    Purgeable,
    ChatOnly,
}

impl SourceRetention {
    const fn source_kind(self) -> &'static str {
        match self {
            Self::Versioned => "article",
            Self::Purgeable => "social_snapshot",
            Self::ChatOnly => "chat_attachment",
        }
    }

    const fn is_purgeable(self) -> bool {
        matches!(self, Self::Purgeable | Self::ChatOnly)
    }
}

#[derive(Debug, Deserialize)]
pub struct PromoteChatAttachmentRequest {
    pub vault_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct PurgeChatAttachmentsResponse {
    pub purged: usize,
}

#[derive(Debug, Deserialize)]
pub struct RetrySourceRequest {
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize)]
pub struct PutNoteRequest {
    pub expected_revision: i64,
    pub expected_git_revision: String,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateNoteRequest {
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct SourceIngestResponse {
    pub source: sources::KnowledgeSourceRow,
    pub object: KnowledgeObjectRow,
    pub git_revision: String,
    pub blob_existed: bool,
    pub http_status: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct SourceListResponse {
    pub sources: Vec<sources::KnowledgeSourceRow>,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
pub struct SourceDetailResponse {
    pub source: sources::KnowledgeSourceRow,
    pub attempts: Vec<sources::KnowledgeSourceAttemptRow>,
    pub extraction: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct NoteResponse {
    pub object_id: Uuid,
    pub source_id: Option<Uuid>,
    pub revision: i64,
    pub content_hash: String,
    pub git_revision: String,
    pub frontmatter_format: String,
    pub properties: BTreeMap<String, serde_json::Value>,
    pub body: String,
    pub external_edit_reconciled: bool,
}

#[derive(Debug, Serialize)]
pub struct DeleteNoteResponse {
    pub object: KnowledgeObjectRow,
    pub git_revision: String,
}

pub async fn upload_source_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(vault_id): Path<Uuid>,
    multipart: Multipart,
) -> Result<Json<SourceIngestResponse>, WorkbenchHttpError> {
    let upload = read_source_multipart(multipart).await?;
    let actor = actor(&ctx)?;
    if let Some(conversation_id) = upload.conversation_id {
        ensure_conversation_owner(pool(&state)?, actor, conversation_id).await?;
    }
    ingest_uploaded_source(
        &state,
        actor,
        vault_id,
        upload.conversation_id,
        "upload",
        SourceRetention::Versioned,
        upload,
    )
    .await
    .map(Json)
}

pub async fn upload_chat_attachment_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(conversation_id): Path<Uuid>,
    multipart: Multipart,
) -> Result<Json<SourceIngestResponse>, WorkbenchHttpError> {
    let upload = read_source_multipart(multipart).await?;
    if upload
        .conversation_id
        .is_some_and(|form_id| form_id != conversation_id)
    {
        return Err(invalid(
            "multipart conversation_id does not match the route",
        ));
    }
    let actor = actor(&ctx)?;
    let vault_id = ensure_chat_vault(&state, actor, conversation_id).await?;
    ingest_uploaded_source(
        &state,
        actor,
        vault_id,
        Some(conversation_id),
        "chat_attachment",
        SourceRetention::ChatOnly,
        upload,
    )
    .await
    .map(Json)
}

struct UploadedSource {
    bytes: Vec<u8>,
    original_name: String,
    content_type: String,
    title: String,
    conversation_id: Option<Uuid>,
}

async fn read_source_multipart(
    mut multipart: Multipart,
) -> Result<UploadedSource, WorkbenchHttpError> {
    let mut bytes = None;
    let mut original_name = String::new();
    let mut content_type = String::new();
    let mut title = String::new();
    let mut conversation_id = None;
    while let Some(field) = multipart.next_field().await.map_err(invalid_multipart)? {
        match field.name().unwrap_or_default() {
            "file" => {
                if bytes.is_some() {
                    return Err(invalid("multipart must contain exactly one file part"));
                }
                original_name = safe_filename(field.file_name().unwrap_or("source"));
                content_type = field.content_type().map(str::to_string).unwrap_or_default();
                let body = field.bytes().await.map_err(invalid_multipart)?;
                if body.len() > MAX_SOURCE_BYTES {
                    return Err(invalid("source file exceeds the 16 MiB limit"));
                }
                bytes = Some(body.to_vec());
            }
            "title" => {
                title = field.text().await.map_err(invalid_multipart)?;
                if title.chars().count() > 512 {
                    return Err(invalid("source title exceeds 512 characters"));
                }
            }
            "conversation_id" => {
                if conversation_id.is_some() {
                    return Err(invalid("multipart may contain one conversation_id"));
                }
                conversation_id = Some(
                    Uuid::parse_str(field.text().await.map_err(invalid_multipart)?.trim())
                        .map_err(|_| invalid("conversation_id must be a UUID"))?,
                );
            }
            _ => return Err(invalid("unknown multipart field")),
        }
    }
    let bytes = bytes.ok_or_else(|| invalid("multipart file part is required"))?;
    if bytes.is_empty() {
        return Err(invalid("source file must not be empty"));
    }
    content_type = normalized_upload_content_type(&content_type, &original_name)?;
    Ok(UploadedSource {
        bytes,
        original_name,
        content_type,
        title,
        conversation_id,
    })
}

#[allow(clippy::too_many_arguments)]
async fn ingest_uploaded_source(
    state: &AppState,
    actor: SpaceActor,
    vault_id: Uuid,
    conversation_id: Option<Uuid>,
    source_kind: &str,
    retention: SourceRetention,
    upload: UploadedSource,
) -> Result<SourceIngestResponse, WorkbenchHttpError> {
    let source = sources::create_pending_source(
        pool(state)?,
        actor,
        CreateSource {
            vault_id,
            conversation_id,
            source_kind: source_kind.to_string(),
            title: upload.title,
            original_name: upload.original_name,
            requested_uri: None,
        },
    )
    .await?;
    ingest_pending(
        state,
        actor,
        source,
        upload.bytes,
        upload.content_type,
        None,
        None,
        "upload",
        retention,
    )
    .await
}

pub async fn fetch_source_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(vault_id): Path<Uuid>,
    Json(request): Json<FetchSourceRequest>,
) -> Result<Json<SourceIngestResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    if let Some(conversation_id) = request.conversation_id {
        ensure_conversation_owner(pool(&state)?, actor, conversation_id).await?;
    }
    capture_url_source(
        &state,
        actor,
        CaptureUrlSource {
            vault_id,
            url: request.url,
            title: request.title,
            max_bytes: MAX_SOURCE_BYTES,
            allowed_domains: None,
            timeout: Duration::from_secs(20),
            retention: SourceRetention::Versioned,
            conversation_id: request.conversation_id,
        },
    )
    .await
    .map(Json)
}

pub async fn fetch_chat_attachment_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<FetchSourceRequest>,
) -> Result<Json<SourceIngestResponse>, WorkbenchHttpError> {
    if request
        .conversation_id
        .is_some_and(|body_id| body_id != conversation_id)
    {
        return Err(invalid("conversation_id does not match the route"));
    }
    let actor = actor(&ctx)?;
    let vault_id = ensure_chat_vault(&state, actor, conversation_id).await?;
    capture_url_source(
        &state,
        actor,
        CaptureUrlSource {
            vault_id,
            url: request.url,
            title: request.title,
            max_bytes: MAX_SOURCE_BYTES,
            allowed_domains: None,
            timeout: Duration::from_secs(20),
            retention: SourceRetention::ChatOnly,
            conversation_id: Some(conversation_id),
        },
    )
    .await
    .map(Json)
}

pub(crate) async fn capture_url_source(
    state: &AppState,
    actor: SpaceActor,
    request: CaptureUrlSource,
) -> Result<SourceIngestResponse, WorkbenchHttpError> {
    if request.max_bytes == 0 || request.max_bytes > MAX_SOURCE_BYTES {
        return Err(invalid(
            "source capture byte limit is outside the Core boundary",
        ));
    }
    let source = sources::create_pending_source(
        pool(state)?,
        actor,
        CreateSource {
            vault_id: request.vault_id,
            conversation_id: request.conversation_id,
            source_kind: request.retention.source_kind().to_string(),
            title: request.title,
            original_name: source_name_from_url(&request.url),
            requested_uri: Some(request.url.clone()),
        },
    )
    .await?;
    let fetched = match fetch_public_https(
        &request.url,
        SafeFetchPolicy {
            max_bytes: request.max_bytes,
            max_redirects: 3,
            allowed_content_types: SOURCE_CONTENT_TYPES,
            allowed_domains: request.allowed_domains.as_deref(),
            timeout: request.timeout,
        },
    )
    .await
    {
        Ok(fetched) => fetched,
        Err(error) => {
            return Err(persist_failure(
                state,
                actor,
                source,
                "fetch",
                error.code(),
                &error.to_string(),
                false,
                error.http_status().map(i32::from),
                None,
            )
            .await)
        }
    };
    ingest_fetched(state, actor, source, fetched, request.retention).await
}

pub async fn retry_source_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(source_id): Path<Uuid>,
    Json(request): Json<RetrySourceRequest>,
) -> Result<Json<SourceIngestResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let source =
        sources::begin_retry(pool(&state)?, actor, source_id, request.expected_revision).await?;
    if let Some(blob_id) = source.blob_id {
        let bytes = blob_store(&state)?
            .get(&BlobId(blob_id))
            .await
            .map_err(blob_error)?;
        let content_type = source
            .content_type
            .clone()
            .ok_or_else(|| invalid("retained source blob has no content type"))?;
        return ingest_pending(
            &state,
            actor,
            source.clone(),
            bytes,
            content_type,
            source.final_uri.clone(),
            None,
            if source.requested_uri.is_some() {
                "fetch"
            } else {
                "upload"
            },
            if matches!(
                source.source_kind.as_str(),
                "social_snapshot" | "chat_attachment"
            ) {
                if source.source_kind == "chat_attachment" {
                    SourceRetention::ChatOnly
                } else {
                    SourceRetention::Purgeable
                }
            } else {
                SourceRetention::Versioned
            },
        )
        .await
        .map(Json);
    }
    let url = source
        .requested_uri
        .clone()
        .ok_or_else(|| invalid("failed source has neither blob nor requested URL"))?;
    let fetched = match fetch_public_https(
        &url,
        SafeFetchPolicy {
            max_bytes: MAX_SOURCE_BYTES,
            max_redirects: 3,
            allowed_content_types: SOURCE_CONTENT_TYPES,
            allowed_domains: None,
            timeout: Duration::from_secs(20),
        },
    )
    .await
    {
        Ok(fetched) => fetched,
        Err(error) => {
            return Err(persist_failure(
                &state,
                actor,
                source,
                "fetch",
                error.code(),
                &error.to_string(),
                false,
                error.http_status().map(i32::from),
                None,
            )
            .await)
        }
    };
    let retention = match source.source_kind.as_str() {
        "social_snapshot" => SourceRetention::Purgeable,
        "chat_attachment" => SourceRetention::ChatOnly,
        _ => SourceRetention::Versioned,
    };
    ingest_fetched(&state, actor, source, fetched, retention)
        .await
        .map(Json)
}

async fn ingest_fetched(
    state: &AppState,
    actor: SpaceActor,
    source: sources::KnowledgeSourceRow,
    fetched: SafeFetchResponse,
    retention: SourceRetention,
) -> Result<SourceIngestResponse, WorkbenchHttpError> {
    ingest_pending(
        state,
        actor,
        source,
        fetched.bytes,
        fetched.content_type,
        Some(fetched.final_url),
        Some(i32::from(fetched.status)),
        "fetch",
        retention,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn ingest_pending(
    state: &AppState,
    actor: SpaceActor,
    source: sources::KnowledgeSourceRow,
    bytes: Vec<u8>,
    content_type: String,
    final_uri: Option<String>,
    http_status: Option<i32>,
    acquisition_phase: &str,
    retention: SourceRetention,
) -> Result<SourceIngestResponse, WorkbenchHttpError> {
    let store = blob_store(state)?;
    let blob = match store
        .put(
            &bytes,
            &BlobMetadata {
                tenant_id: actor.tenant_id.to_string(),
                content_type: content_type.clone(),
                filename: source.original_name.clone(),
                byte_size: bytes.len() as u64,
                imported_by: actor.user_id.to_string(),
            },
        )
        .await
    {
        Ok(blob) => blob,
        Err(error) => {
            return Err(persist_failure(
                state,
                actor,
                source,
                acquisition_phase,
                error.code(),
                &error.to_string(),
                false,
                http_status,
                Some(content_type),
            )
            .await)
        }
    };
    let source = sources::attach_source_blob(
        pool(state)?,
        actor,
        source.id,
        source.revision,
        AttachSourceBlob {
            blob_id: blob.id.0,
            content_type: content_type.clone(),
            byte_size: bytes.len() as i64,
            content_hash: blob.content_hash.clone(),
            final_uri: final_uri.clone(),
        },
    )
    .await?;
    sources::record_attempt(
        pool(state)?,
        actor.tenant_id,
        attempt(
            &source,
            acquisition_phase,
            "succeeded",
            final_uri.clone(),
            http_status,
            Some(content_type.clone()),
            Some(bytes.len() as i64),
            Some(blob.content_hash.clone()),
            None,
            None,
        ),
    )
    .await?;

    let extracted = match extract_source(&bytes, &content_type).await {
        Ok(extracted) => extracted,
        Err(error) => {
            return Err(persist_failure(
                state,
                actor,
                source,
                "extract",
                error.code(),
                &error.to_string(),
                error.needs_ocr(),
                None,
                Some(content_type),
            )
            .await)
        }
    };
    let vault =
        sources::source_vault(pool(state)?, actor, source.id, SpaceRole::Contributor, true).await?;
    let existing = sources::source_object(pool(state)?, actor, source.id).await?;
    let object_id = existing
        .as_ref()
        .map_or_else(Uuid::new_v4, |object| object.id);
    let title = source_title(&source);
    let path = existing
        .as_ref()
        .map(|object| object.path.clone())
        .unwrap_or_else(|| note_relative_path(&title, object_id));
    let note = build_source_note(
        object_id,
        &source,
        &vault,
        SourceNoteMaterial {
            title: &title,
            content_type: &content_type,
            content_hash: &blob.content_hash,
            extracted,
            retention,
        },
    )?;
    let content_hash = hex::encode(Sha256::digest(note.as_bytes()));
    let object = match existing {
        Some(object) if object.content_hash.as_deref() == Some(content_hash.as_str()) => object,
        Some(object) => {
            sources::update_note_hash(
                pool(state)?,
                actor,
                object.id,
                object.revision,
                &content_hash,
            )
            .await?
        }
        None => {
            sources::register_source_object(
                pool(state)?,
                actor,
                source.id,
                object_id,
                &path,
                &content_hash,
            )
            .await?
        }
    };
    let repository = vault_repository(state, actor.tenant_id)?;
    repository
        .ensure_domain(vault.space_id, &vault.home_bundle_id)
        .map_err(vault_error)?;
    let note_state = match repository.write_note(
        vault.space_id,
        &vault.home_bundle_id,
        &path,
        note.as_bytes(),
        "vault: extract source into note",
    ) {
        Ok(state) => state,
        Err(error) => {
            return Err(persist_failure(
                state,
                actor,
                source,
                "extract",
                "vault_write_failed",
                &error.to_string(),
                false,
                None,
                Some(content_type),
            )
            .await)
        }
    };
    let completed =
        sources::complete_source(pool(state)?, actor, source.id, source.revision, object.id)
            .await?;
    sources::record_attempt(
        pool(state)?,
        actor.tenant_id,
        attempt(
            &completed,
            "extract",
            "succeeded",
            final_uri,
            None,
            Some(content_type),
            Some(bytes.len() as i64),
            Some(blob.content_hash),
            None,
            None,
        ),
    )
    .await?;
    super::knowledge_graph::reconcile_after_change(state, actor).await;
    Ok(SourceIngestResponse {
        source: completed,
        object,
        git_revision: note_state.git_revision,
        blob_existed: blob.existed,
        http_status,
    })
}

#[allow(clippy::too_many_arguments)]
async fn persist_failure(
    state: &AppState,
    actor: SpaceActor,
    source: sources::KnowledgeSourceRow,
    phase: &str,
    code: &str,
    detail: &str,
    needs_ocr: bool,
    http_status: Option<i32>,
    content_type: Option<String>,
) -> WorkbenchHttpError {
    let pool = match pool(state) {
        Ok(pool) => pool,
        Err(error) => return error,
    };
    if let Err(error) = sources::record_attempt(
        pool,
        actor.tenant_id,
        attempt(
            &source,
            phase,
            if needs_ocr { "needs_ocr" } else { "failed" },
            source.final_uri.clone(),
            http_status,
            content_type,
            source.byte_size,
            source.content_hash.clone(),
            Some(code.to_string()),
            Some(detail.to_string()),
        ),
    )
    .await
    {
        return error.into();
    }
    if let Err(error) = sources::fail_source(
        pool,
        actor,
        source.id,
        source.revision,
        needs_ocr,
        code,
        detail,
    )
    .await
    {
        return error.into();
    }
    super::knowledge_graph::reconcile_after_change(state, actor).await;
    WorkbenchHttpError::KnowledgeSourceFailed {
        source_id: source.id,
        code: code.to_string(),
        detail: detail.chars().take(1024).collect(),
    }
}

pub async fn list_sources_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(space_id): Path<Uuid>,
) -> Result<Json<SourceListResponse>, WorkbenchHttpError> {
    let rows = sources::list_sources(pool(&state)?, actor(&ctx)?, space_id).await?;
    let returned = rows.len();
    Ok(Json(SourceListResponse {
        sources: rows,
        returned,
    }))
}

pub async fn get_source_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(source_id): Path<Uuid>,
) -> Result<Json<SourceDetailResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let source =
        sources::get_source(pool(&state)?, actor, source_id, SpaceRole::Viewer, false).await?;
    let attempts = sources::source_attempts(pool(&state)?, actor, source_id).await?;
    let extraction = source_extraction_metadata(&state, actor, &source).await?;
    Ok(Json(SourceDetailResponse {
        source,
        attempts,
        extraction,
    }))
}

async fn source_extraction_metadata(
    state: &AppState,
    actor: SpaceActor,
    source: &sources::KnowledgeSourceRow,
) -> Result<Option<serde_json::Value>, WorkbenchHttpError> {
    let Some(object_id) = source.extracted_object_id else {
        return Ok(None);
    };
    let location = match sources::note_location(
        pool(state)?,
        actor,
        object_id,
        SpaceRole::Viewer,
        false,
    )
    .await
    {
        Ok(location) => location,
        Err(KnowledgeSpaceError::NotFound) => {
            tracing::warn!(
                source_id = %source.id,
                %object_id,
                "source detail extraction note is missing or inactive"
            );
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    if location.source_id != Some(source.id) {
        return Err(invalid(
            "source extraction note does not match its Source Ledger identity",
        ));
    }
    let repository = match vault_repository(state, actor.tenant_id) {
        Ok(repository) => repository,
        Err(error) => {
            tracing::warn!(
                source_id = %source.id,
                %object_id,
                detail = ?error,
                "source detail extraction repository is unavailable"
            );
            return Ok(None);
        }
    };
    let note = match repository.read_note_exact(
        location.space_id,
        &location.home_bundle_id,
        &location.path,
        location.content_hash.as_deref(),
    ) {
        Ok(note) => note,
        Err(error) => {
            tracing::warn!(
                source_id = %source.id,
                %object_id,
                detail = %error,
                "source detail extraction note could not be read"
            );
            return Ok(None);
        }
    };
    if note.externally_changed {
        tracing::warn!(
            source_id = %source.id,
            %object_id,
            "source detail extraction note changed outside the registry"
        );
        return Ok(None);
    }
    let raw = match String::from_utf8(note.bytes) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!(
                source_id = %source.id,
                %object_id,
                detail = %error,
                "source detail extraction note is not valid UTF-8"
            );
            return Ok(None);
        }
    };
    let parsed = match parse_obsidian_note(&raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(
                source_id = %source.id,
                %object_id,
                detail = %error,
                "source detail extraction frontmatter could not be parsed"
            );
            return Ok(None);
        }
    };
    validate_note_identity(&parsed.properties, &location)?;
    Ok(parsed.properties.get("extraction").cloned())
}

pub async fn delete_source_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(source_id): Path<Uuid>,
    Json(request): Json<RetrySourceRequest>,
) -> Result<Json<sources::KnowledgeSourceRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    delete_source(&state, actor, source_id, request.expected_revision)
        .await
        .map(Json)
}

async fn delete_source(
    state: &AppState,
    actor: SpaceActor,
    source_id: Uuid,
    expected_revision: i64,
) -> Result<sources::KnowledgeSourceRow, WorkbenchHttpError> {
    let current =
        sources::get_source(pool(state)?, actor, source_id, SpaceRole::Contributor, true).await?;
    if current.revision != expected_revision {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let source = if matches!(
        current.source_kind.as_str(),
        "social_snapshot" | "chat_attachment"
    ) {
        let object = sources::source_object(pool(state)?, actor, source_id).await?;
        if let Some(object) = object {
            let location = sources::note_location(
                pool(state)?,
                actor,
                object.id,
                SpaceRole::Contributor,
                true,
            )
            .await?;
            let repository = vault_repository(state, actor.tenant_id)?;
            let lock = repository
                .acquire_lock(Duration::from_secs(5))
                .map_err(vault_error)?;
            repository
                .archive_note_locked(
                    &lock,
                    location.space_id,
                    &location.home_bundle_id,
                    &location.path,
                )
                .map_err(vault_error)?;
            sources::delete_note(pool(state)?, actor, object.id, object.revision).await?;
        }
        let blob_id = current
            .blob_id
            .ok_or_else(|| invalid("purgeable source has no retained blob"))?;
        let purged =
            sources::purge_purgeable_source(pool(state)?, actor, source_id, expected_revision)
                .await?;
        if sources::live_blob_reference_count(pool(state)?, actor.tenant_id, blob_id).await? == 0 {
            blob_store(state)?
                .delete(&BlobId(blob_id))
                .await
                .map_err(blob_error)?;
        }
        purged
    } else {
        sources::delete_source(pool(state)?, actor, source_id, expected_revision).await?
    };
    super::knowledge_graph::reconcile_after_change(state, actor).await;
    Ok(source)
}

pub async fn list_chat_attachments_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<SourceListResponse>, WorkbenchHttpError> {
    let rows =
        sources::list_conversation_sources(pool(&state)?, actor(&ctx)?, conversation_id).await?;
    let returned = rows.len();
    Ok(Json(SourceListResponse {
        sources: rows,
        returned,
    }))
}

pub async fn purge_chat_attachments_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<PurgeChatAttachmentsResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    ensure_conversation_owner(pool(&state)?, actor, conversation_id).await?;
    let rows = sources::list_conversation_sources(pool(&state)?, actor, conversation_id).await?;
    let mut purged = 0;
    for source in rows
        .into_iter()
        .filter(|source| source.source_kind == "chat_attachment")
    {
        delete_source(&state, actor, source.id, source.revision).await?;
        purged += 1;
    }
    Ok(Json(PurgeChatAttachmentsResponse { purged }))
}

pub async fn promote_chat_attachment_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path((conversation_id, source_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<PromoteChatAttachmentRequest>,
) -> Result<Json<SourceIngestResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    ensure_conversation_owner(pool(&state)?, actor, conversation_id).await?;
    let source =
        sources::get_source(pool(&state)?, actor, source_id, SpaceRole::Viewer, false).await?;
    if source.conversation_id != Some(conversation_id) || source.source_kind != "chat_attachment" {
        return Err(WorkbenchHttpError::KnowledgeNotFound);
    }
    if source.status != "extracted" {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let blob_id = source
        .blob_id
        .ok_or_else(|| invalid("chat attachment has no retained blob"))?;
    let bytes = blob_store(&state)?
        .get(&BlobId(blob_id))
        .await
        .map_err(blob_error)?;
    let content_type = source
        .content_type
        .clone()
        .ok_or_else(|| invalid("chat attachment has no retained content type"))?;
    let versioned_kind = if source.requested_uri.is_some() {
        "article"
    } else {
        "upload"
    };
    let promoted = sources::create_pending_source(
        pool(&state)?,
        actor,
        CreateSource {
            vault_id: request.vault_id,
            conversation_id: Some(conversation_id),
            source_kind: versioned_kind.to_string(),
            title: source.title.clone(),
            original_name: source.original_name.clone(),
            requested_uri: source.requested_uri.clone(),
        },
    )
    .await?;
    ingest_pending(
        &state,
        actor,
        promoted,
        bytes,
        content_type,
        source.final_uri.clone(),
        None,
        if source.requested_uri.is_some() {
            "fetch"
        } else {
            "upload"
        },
        SourceRetention::Versioned,
    )
    .await
    .map(Json)
}

pub async fn delete_chat_attachment_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path((conversation_id, source_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<RetrySourceRequest>,
) -> Result<Json<sources::KnowledgeSourceRow>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    ensure_conversation_owner(pool(&state)?, actor, conversation_id).await?;
    let source = sources::get_source(
        pool(&state)?,
        actor,
        source_id,
        SpaceRole::Contributor,
        true,
    )
    .await?;
    if source.conversation_id != Some(conversation_id) || source.source_kind != "chat_attachment" {
        return Err(WorkbenchHttpError::KnowledgeNotFound);
    }
    delete_source(&state, actor, source_id, request.expected_revision)
        .await
        .map(Json)
}

pub async fn get_source_blob_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(source_id): Path<Uuid>,
) -> Result<Response<Body>, WorkbenchHttpError> {
    let source = sources::get_source(
        pool(&state)?,
        actor(&ctx)?,
        source_id,
        SpaceRole::Viewer,
        false,
    )
    .await?;
    let blob_id = source
        .blob_id
        .ok_or(WorkbenchHttpError::KnowledgeNotFound)?;
    let bytes = blob_store(&state)?
        .get(&BlobId(blob_id))
        .await
        .map_err(blob_error)?;
    let mut response = bytes.into_response();
    if let Some(content_type) = source.content_type {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type)
                .map_err(|_| invalid("invalid stored content type"))?,
        );
    }
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    Ok(response)
}

pub async fn get_note_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(object_id): Path<Uuid>,
) -> Result<Json<NoteResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let location =
        sources::note_location(pool(&state)?, actor, object_id, SpaceRole::Viewer, false).await?;
    let repository = vault_repository(&state, actor.tenant_id)?;
    let lock = repository
        .acquire_lock(Duration::from_secs(5))
        .map_err(vault_error)?;
    let note = repository
        .read_note_reconciled_locked(
            &lock,
            location.space_id,
            &location.home_bundle_id,
            &location.path,
            location.content_hash.as_deref(),
        )
        .map_err(vault_error)?;
    let revision = if note.externally_changed {
        sources::update_note_hash_system(
            pool(&state)?,
            actor.tenant_id,
            object_id,
            location.revision,
            &note.content_hash,
        )
        .await?
        .revision
    } else {
        location.revision
    };
    let raw = String::from_utf8(note.bytes)
        .map_err(|_| invalid("Domain Vault note is not valid UTF-8"))?;
    let parsed = parse_obsidian_note(&raw).map_err(|error| invalid(error.to_string()))?;
    validate_note_identity(&parsed.properties, &location)?;
    drop(lock);
    if note.externally_changed {
        super::knowledge_graph::reconcile_after_change(&state, actor).await;
    }
    Ok(Json(NoteResponse {
        object_id,
        source_id: location.source_id,
        revision,
        content_hash: note.content_hash,
        git_revision: note.git_revision,
        frontmatter_format: format_name(parsed.format).to_string(),
        properties: parsed.properties,
        body: parsed.body,
        external_edit_reconciled: note.externally_changed,
    }))
}

pub async fn create_note_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(vault_id): Path<Uuid>,
    Json(request): Json<CreateNoteRequest>,
) -> Result<Json<NoteResponse>, WorkbenchHttpError> {
    let title = request.title.trim();
    if title.is_empty() || title.chars().count() > 512 {
        return Err(invalid("note title must contain 1 to 512 characters"));
    }
    if request.body.len() > MAX_SOURCE_BYTES {
        return Err(invalid("note body exceeds the 16 MiB limit"));
    }
    let actor = actor(&ctx)?;
    let vault = sources::manual_note_vault(pool(&state)?, actor, vault_id).await?;
    let object_id = Uuid::new_v4();
    let path = note_relative_path(title, object_id);
    let now = chrono::Utc::now().to_rfc3339();
    let mut properties = BTreeMap::new();
    properties.insert("id".into(), serde_json::json!(object_id));
    properties.insert("title".into(), serde_json::json!(title));
    properties.insert("kind".into(), serde_json::json!("note"));
    properties.insert("status".into(), serde_json::json!("draft"));
    properties.insert("space_id".into(), serde_json::json!(vault.space_id));
    properties.insert(
        "home_bundle_id".into(),
        serde_json::json!(vault.home_bundle_id),
    );
    properties.insert("source_ids".into(), serde_json::json!([]));
    properties.insert("source_hashes".into(), serde_json::json!([]));
    properties.insert("created".into(), serde_json::json!(now));
    properties.insert("updated".into(), serde_json::json!(now));
    let body = if request.body.trim().is_empty() {
        format!("# {title}\n\n")
    } else {
        request.body
    };
    let raw =
        serialize_obsidian_note(&properties, &body).map_err(|error| invalid(error.to_string()))?;
    if let Some(secret) = gadgetron_knowledge::wiki::secrets::check_block_patterns(&raw).first() {
        return Err(invalid(format!(
            "note contains blocked credential pattern {}",
            secret.pattern_name
        )));
    }
    let content_hash = hex::encode(Sha256::digest(raw.as_bytes()));
    let (object, _) = sources::register_manual_note(
        pool(&state)?,
        actor,
        vault_id,
        object_id,
        &path,
        Some(&content_hash),
    )
    .await?;
    let repository = vault_repository(&state, actor.tenant_id)?;
    let note = match repository
        .ensure_domain(vault.space_id, &vault.home_bundle_id)
        .and_then(|_| {
            repository.write_note(
                vault.space_id,
                &vault.home_bundle_id,
                &path,
                raw.as_bytes(),
                "vault: create note",
            )
        }) {
        Ok(note) => note,
        Err(error) => {
            let _ = sources::delete_note(pool(&state)?, actor, object_id, object.revision).await;
            return Err(vault_error(error));
        }
    };
    super::knowledge_graph::reconcile_after_change(&state, actor).await;
    Ok(Json(NoteResponse {
        object_id,
        source_id: None,
        revision: object.revision,
        content_hash: note.content_hash,
        git_revision: note.git_revision,
        frontmatter_format: "yaml".to_string(),
        properties,
        body,
        external_edit_reconciled: false,
    }))
}

pub async fn put_note_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(object_id): Path<Uuid>,
    Json(mut request): Json<PutNoteRequest>,
) -> Result<Json<NoteResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let repository = vault_repository(&state, actor.tenant_id)?;
    let lock = repository
        .acquire_lock(Duration::from_secs(5))
        .map_err(vault_error)?;
    let location = sources::note_location(
        pool(&state)?,
        actor,
        object_id,
        SpaceRole::Contributor,
        true,
    )
    .await?;
    if location.revision != request.expected_revision {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let current = repository
        .read_note_reconciled_locked(
            &lock,
            location.space_id,
            &location.home_bundle_id,
            &location.path,
            location.content_hash.as_deref(),
        )
        .map_err(vault_error)?;
    if current.externally_changed {
        sources::update_note_hash_system(
            pool(&state)?,
            actor.tenant_id,
            object_id,
            location.revision,
            &current.content_hash,
        )
        .await?;
        drop(lock);
        super::knowledge_graph::reconcile_after_change(&state, actor).await;
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    validate_note_identity(&request.properties, &location)?;
    force_note_identity(&mut request.properties, &location);
    request.properties.insert(
        "updated".to_string(),
        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    let raw = serialize_obsidian_note(&request.properties, &request.body)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(secret) = gadgetron_knowledge::wiki::secrets::check_block_patterns(&raw).first() {
        return Err(invalid(format!(
            "note contains blocked credential pattern {}",
            secret.pattern_name
        )));
    }
    let note = repository
        .write_note_at_revision_locked(
            &lock,
            VaultNoteRevisionWrite {
                space_id: location.space_id,
                home_bundle_id: &location.home_bundle_id,
                relative_path: &location.path,
                bytes: raw.as_bytes(),
                expected_git_revision: &request.expected_git_revision,
                message: "vault: edit Obsidian note",
            },
        )
        .map_err(vault_error)?;
    let object = sources::update_note_hash(
        pool(&state)?,
        actor,
        object_id,
        request.expected_revision,
        &note.content_hash,
    )
    .await?;
    drop(lock);
    super::knowledge_graph::reconcile_after_change(&state, actor).await;
    Ok(Json(NoteResponse {
        object_id,
        source_id: location.source_id,
        revision: object.revision,
        content_hash: note.content_hash,
        git_revision: note.git_revision,
        frontmatter_format: "yaml".to_string(),
        properties: request.properties,
        body: request.body,
        external_edit_reconciled: false,
    }))
}

pub async fn delete_note_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Path(object_id): Path<Uuid>,
    Json(request): Json<RetrySourceRequest>,
) -> Result<Json<DeleteNoteResponse>, WorkbenchHttpError> {
    let actor = actor(&ctx)?;
    let repository = vault_repository(&state, actor.tenant_id)?;
    let lock = repository
        .acquire_lock(Duration::from_secs(5))
        .map_err(vault_error)?;
    let location = sources::note_location(
        pool(&state)?,
        actor,
        object_id,
        SpaceRole::Contributor,
        true,
    )
    .await?;
    if location.revision != request.expected_revision {
        return Err(WorkbenchHttpError::KnowledgeConflict);
    }
    let git_revision = repository
        .archive_note_locked(
            &lock,
            location.space_id,
            &location.home_bundle_id,
            &location.path,
        )
        .map_err(vault_error)?;
    let object =
        sources::delete_note(pool(&state)?, actor, object_id, request.expected_revision).await?;
    drop(lock);
    super::knowledge_graph::reconcile_after_change(&state, actor).await;
    Ok(Json(DeleteNoteResponse {
        object,
        git_revision,
    }))
}

struct SourceNoteMaterial<'a> {
    title: &'a str,
    content_type: &'a str,
    content_hash: &'a str,
    extracted: ExtractedSource,
    retention: SourceRetention,
}

fn build_source_note(
    object_id: Uuid,
    source: &sources::KnowledgeSourceRow,
    vault: &gadgetron_xaas::knowledge_spaces::KnowledgeVaultRow,
    material: SourceNoteMaterial<'_>,
) -> Result<String, WorkbenchHttpError> {
    let SourceNoteMaterial {
        title,
        content_type,
        content_hash,
        extracted,
        retention,
    } = material;
    let mut properties = BTreeMap::new();
    properties.insert("id".into(), serde_json::json!(object_id));
    properties.insert("title".into(), serde_json::json!(title));
    properties.insert("kind".into(), serde_json::json!("note"));
    properties.insert("status".into(), serde_json::json!("draft"));
    properties.insert("space_id".into(), serde_json::json!(vault.space_id));
    properties.insert(
        "home_bundle_id".into(),
        serde_json::json!(vault.home_bundle_id),
    );
    properties.insert("source_ids".into(), serde_json::json!([source.id]));
    properties.insert("source_hashes".into(), serde_json::json!([content_hash]));
    properties.insert(
        "source_content_type".into(),
        serde_json::json!(content_type),
    );
    properties.insert(
        "source_retention".into(),
        serde_json::json!(match retention {
            SourceRetention::Versioned => "versioned",
            SourceRetention::Purgeable | SourceRetention::ChatOnly => "purgeable",
        }),
    );
    properties.insert(
        "created".into(),
        serde_json::json!(source.created_at.to_rfc3339()),
    );
    properties.insert(
        "updated".into(),
        serde_json::json!(source.created_at.to_rfc3339()),
    );
    if extracted.metadata.is_object() {
        properties.insert("extraction".into(), extracted.metadata);
    }
    if !extracted.audit_secret_patterns.is_empty() {
        properties.insert(
            "secret_pattern_warnings".into(),
            serde_json::json!(extracted.audit_secret_patterns),
        );
    }
    let body = if retention.is_purgeable() {
        format!(
            "# {title}\n\nThe original content is stored in a purgeable Source blob and is not versioned in this Vault."
        )
    } else if extracted
        .markdown
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim_start().starts_with("# "))
    {
        extracted.markdown
    } else {
        format!("# {title}\n\n{}", extracted.markdown)
    };
    serialize_obsidian_note(&properties, &body).map_err(|error| invalid(error.to_string()))
}

fn validate_note_identity(
    properties: &BTreeMap<String, serde_json::Value>,
    location: &sources::NoteObjectLocation,
) -> Result<(), WorkbenchHttpError> {
    for (key, expected) in [
        ("id", location.id.to_string()),
        ("space_id", location.space_id.to_string()),
        ("home_bundle_id", location.home_bundle_id.clone()),
    ] {
        if let Some(actual) = properties.get(key).and_then(|value| value.as_str()) {
            if actual != expected {
                return Err(invalid(format!(
                    "note property {key} does not match its stable registry identity"
                )));
            }
        }
    }
    Ok(())
}

fn force_note_identity(
    properties: &mut BTreeMap<String, serde_json::Value>,
    location: &sources::NoteObjectLocation,
) {
    properties.insert("id".into(), serde_json::json!(location.id));
    properties.insert("space_id".into(), serde_json::json!(location.space_id));
    properties.insert(
        "home_bundle_id".into(),
        serde_json::json!(location.home_bundle_id),
    );
    if let Some(source_id) = location.source_id {
        properties.insert("source_ids".into(), serde_json::json!([source_id]));
    }
}

#[allow(clippy::too_many_arguments)]
fn attempt(
    source: &sources::KnowledgeSourceRow,
    phase: &str,
    outcome: &str,
    final_uri: Option<String>,
    http_status: Option<i32>,
    content_type: Option<String>,
    byte_size: Option<i64>,
    content_hash: Option<String>,
    failure_code: Option<String>,
    failure_detail: Option<String>,
) -> sources::SourceAttempt {
    sources::SourceAttempt {
        source_id: source.id,
        attempt_no: source.attempt_count,
        phase: phase.to_string(),
        outcome: outcome.to_string(),
        final_uri,
        http_status,
        content_type,
        byte_size,
        content_hash,
        failure_code,
        failure_detail,
    }
}

fn source_title(source: &sources::KnowledgeSourceRow) -> String {
    let candidate = if source.title.trim().is_empty() {
        source.original_name.trim()
    } else {
        source.title.trim()
    };
    if candidate.is_empty() {
        "Imported source".to_string()
    } else {
        candidate.chars().take(512).collect()
    }
}

fn safe_filename(value: &str) -> String {
    std::path::Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("source")
        .chars()
        .take(512)
        .collect()
}

fn source_name_from_url(value: &str) -> String {
    reqwest::Url::parse(value)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .map(safe_filename)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "article".to_string())
}

fn normalized_upload_content_type(
    header_value: &str,
    filename: &str,
) -> Result<String, WorkbenchHttpError> {
    let supplied = header_value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let inferred = match std::path::Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md" | "markdown") => Some("text/markdown"),
        Some("txt") => Some("text/plain"),
        Some("pdf") => Some("application/pdf"),
        Some("html" | "htm") => Some("text/html"),
        _ => None,
    };
    let normalized = if supplied.is_empty() || supplied == "application/octet-stream" {
        inferred.unwrap_or_default().to_string()
    } else {
        supplied
    };
    if SOURCE_CONTENT_TYPES.contains(&normalized.as_str()) {
        Ok(normalized)
    } else {
        Err(invalid(format!(
            "unsupported source content type {normalized:?}"
        )))
    }
}

fn format_name(format: NoteFrontmatterFormat) -> &'static str {
    match format {
        NoteFrontmatterFormat::None => "none",
        NoteFrontmatterFormat::Yaml => "yaml",
        NoteFrontmatterFormat::LegacyToml => "legacy_toml",
    }
}

fn pool(state: &AppState) -> Result<&sqlx::PgPool, WorkbenchHttpError> {
    state.pg_pool.as_ref().ok_or_else(|| {
        WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(
            "Source-to-Vault requires PostgreSQL".to_string(),
        ))
    })
}

fn workbench(state: &AppState) -> Result<&GatewayWorkbenchService, WorkbenchHttpError> {
    state.workbench.as_deref().ok_or_else(|| {
        WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(
            "Source-to-Vault requires Workbench configuration".to_string(),
        ))
    })
}

fn blob_store(state: &AppState) -> Result<Arc<FilesystemBlobStore>, WorkbenchHttpError> {
    let layout = workbench(state)?
        .vault_layout
        .as_ref()
        .ok_or_else(|| invalid("Source-to-Vault requires [knowledge].vault_path"))?;
    Ok(Arc::new(FilesystemBlobStore::new(
        pool(state)?.clone(),
        layout.root(),
    )))
}

fn vault_repository(
    state: &AppState,
    tenant_id: Uuid,
) -> Result<gadgetron_knowledge::vault::TenantVaultRepository, WorkbenchHttpError> {
    workbench(state)?
        .vault_layout
        .as_ref()
        .ok_or_else(|| invalid("Source-to-Vault requires [knowledge].vault_path"))?
        .open_or_init(tenant_id)
        .map_err(vault_error)
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

async fn ensure_conversation_owner(
    pool: &sqlx::PgPool,
    actor: SpaceActor,
    conversation_id: Uuid,
) -> Result<(), WorkbenchHttpError> {
    conversations::ensure_conversation_owner(pool, conversation_id, actor.tenant_id, actor.user_id)
        .await
        .map_err(|error| match error {
            conversations::ConversationError::OwnershipMismatch
            | conversations::ConversationError::NotFound => WorkbenchHttpError::KnowledgeNotFound,
            other => WorkbenchHttpError::Core(gadgetron_core::error::GadgetronError::Config(
                format!("conversation attachment ownership check failed: {other}"),
            )),
        })
}

async fn ensure_chat_vault(
    state: &AppState,
    actor: SpaceActor,
    conversation_id: Uuid,
) -> Result<Uuid, WorkbenchHttpError> {
    ensure_conversation_owner(pool(state)?, actor, conversation_id).await?;
    let space = spaces::ensure_personal_space(pool(state)?, actor, "Personal").await?;
    let vault = spaces::ensure_vault(
        pool(state)?,
        actor,
        space.id,
        EnsureVault {
            home_bundle_id: "core".to_string(),
            knowledge_schema_id: "core.knowledge".to_string(),
            schema_version: 1,
        },
    )
    .await?;
    Ok(vault.id)
}

fn invalid(detail: impl Into<String>) -> WorkbenchHttpError {
    WorkbenchHttpError::KnowledgeInvalidInput {
        detail: detail.into(),
    }
}

fn invalid_multipart(error: axum::extract::multipart::MultipartError) -> WorkbenchHttpError {
    invalid(format!("invalid source multipart body: {error}"))
}

fn vault_error(error: gadgetron_knowledge::vault::VaultLayoutError) -> WorkbenchHttpError {
    match error {
        gadgetron_knowledge::vault::VaultLayoutError::GitRevisionConflict { .. } => {
            WorkbenchHttpError::KnowledgeConflict
        }
        error => invalid(format!("Domain Vault operation failed: {error}")),
    }
}

fn blob_error(error: gadgetron_core::ingest::BlobError) -> WorkbenchHttpError {
    match error {
        gadgetron_core::ingest::BlobError::NotFound(_) => WorkbenchHttpError::KnowledgeNotFound,
        other => invalid(format!("source blob operation failed: {other}")),
    }
}
