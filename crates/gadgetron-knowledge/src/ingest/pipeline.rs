//! Source-Ledger-backed RAW import orchestration for `wiki.import`.
//!
//! New imports never write the legacy single-user wiki. They preserve the
//! agent-facing base64 Markdown/plain signature while storing original bytes,
//! Source/attempt rows, and a source-linked note in the authenticated user's
//! Personal Space `core` Vault. Existing `imports/*.md` remain readable
//! through the legacy knowledge service but are not rewritten.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use gadgetron_core::{
    bundle::PlugId,
    error::{GadgetronError, KnowledgeErrorKind},
    ingest::{BlobMetadata, BlobStore, ExtractHints, Extractor},
};
use gadgetron_xaas::{
    knowledge_sources::{self as sources, AttachSourceBlob, CreateSource, SourceAttempt},
    knowledge_spaces::{self as spaces, EnsureVault, KnowledgeSpaceError, SpaceActor},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    ingest::title::resolve_title,
    source::{serialize_obsidian_note, FilesystemBlobStore},
    vault::{note_relative_path, validate_note_relative_path, TenantVaultLayout, VaultLayoutError},
};

const SOURCE_LEDGER_PLUG: &str = "source-ledger";

/// Source-backed import coordinator. `backend = None` is the deliberate
/// no-DB/CLI fail-closed configuration.
pub struct IngestPipeline {
    backend: Option<SourceImportBackend>,
}

#[derive(Clone)]
struct SourceImportBackend {
    pool: PgPool,
    vault_layout: TenantVaultLayout,
}

impl std::fmt::Debug for IngestPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngestPipeline")
            .field("source_ledger_available", &self.backend.is_some())
            .finish()
    }
}

impl IngestPipeline {
    pub fn source_ledger(pool: PgPool, vault_root: impl Into<PathBuf>) -> Self {
        Self {
            backend: Some(SourceImportBackend {
                pool,
                vault_layout: TenantVaultLayout::new(vault_root),
            }),
        }
    }

    pub const fn unavailable() -> Self {
        Self { backend: None }
    }

    pub const fn is_available(&self) -> bool {
        self.backend.is_some()
    }

    #[tracing::instrument(
        level = "info",
        name = "knowledge.import.source_ledger",
        skip(self, actor, request, extractor),
        fields(
            tenant_id = %actor.tenant_id,
            user_id = %actor.user_id,
            content_type = %request.content_type,
            byte_size = request.bytes.len(),
        )
    )]
    pub async fn import(
        &self,
        actor: SpaceActor,
        request: ImportRequest,
        extractor: Arc<dyn Extractor>,
    ) -> Result<ImportReceipt, GadgetronError> {
        let backend = self.backend.as_ref().ok_or_else(|| {
            backend_error("wiki.import requires PostgreSQL Source Ledger configuration")
        })?;

        let space = spaces::ensure_personal_space(&backend.pool, actor, "Personal")
            .await
            .map_err(registry_error)?;
        let vault = spaces::ensure_vault(
            &backend.pool,
            actor,
            space.id,
            EnsureVault {
                home_bundle_id: "core".to_string(),
                knowledge_schema_id: "core.knowledge".to_string(),
                schema_version: 1,
            },
        )
        .await
        .map_err(registry_error)?;

        let extracted = extractor
            .extract(
                &request.bytes,
                &request.content_type,
                &ExtractHints::default(),
            )
            .await
            .map_err(|error| {
                invalid_error(format!(
                    "extractor {:?} failed ({}): {error}",
                    extractor.name(),
                    error.code()
                ))
            })?;
        let title = resolve_title(
            request.title_hint.as_deref(),
            &extracted.structure,
            "Imported source",
        );

        let requested_object_id = Uuid::new_v4();
        let path = request
            .target_path
            .clone()
            .unwrap_or_else(|| note_relative_path(&title, requested_object_id));
        validate_note_relative_path(&path).map_err(vault_error)?;
        let existing = sources::source_object_at_path(&backend.pool, actor, vault.id, &path)
            .await
            .map_err(registry_error)?;
        if existing.is_some() && !request.overwrite {
            return Err(invalid_error(format!(
                "source note path {path:?} already exists; set overwrite=true to replace it"
            )));
        }
        let object_id = existing
            .as_ref()
            .map_or(requested_object_id, |object| object.id);
        let original_name = std::path::Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("import.md")
            .to_string();

        let source = sources::create_pending_source(
            &backend.pool,
            actor,
            CreateSource {
                vault_id: vault.id,
                conversation_id: None,
                source_kind: "upload".to_string(),
                title: title.clone(),
                original_name: original_name.clone(),
                requested_uri: None,
            },
        )
        .await
        .map_err(registry_error)?;

        let store = FilesystemBlobStore::new(backend.pool.clone(), backend.vault_layout.root());
        let blob = store
            .put(
                &request.bytes,
                &BlobMetadata {
                    tenant_id: actor.tenant_id.to_string(),
                    content_type: request.content_type.clone(),
                    filename: original_name,
                    byte_size: request.bytes.len() as u64,
                    imported_by: actor.user_id.to_string(),
                },
            )
            .await
            .map_err(|error| {
                if error.code() == "blob_too_large" {
                    invalid_error(error.to_string())
                } else {
                    backend_error(error.to_string())
                }
            })?;
        let source = sources::attach_source_blob(
            &backend.pool,
            actor,
            source.id,
            source.revision,
            AttachSourceBlob {
                blob_id: blob.id.0,
                content_type: request.content_type.clone(),
                byte_size: request.bytes.len() as i64,
                content_hash: blob.content_hash.clone(),
                final_uri: request.source_uri.clone(),
            },
        )
        .await
        .map_err(registry_error)?;
        sources::record_attempt(
            &backend.pool,
            actor.tenant_id,
            attempt(
                &source,
                "upload",
                "succeeded",
                request.source_uri.clone(),
                &request.content_type,
                request.bytes.len(),
                &blob.content_hash,
            ),
        )
        .await
        .map_err(registry_error)?;

        let note = build_source_note(SourceNoteInput {
            object_id,
            source_id: source.id,
            space_id: space.id,
            title: &title,
            content_type: &request.content_type,
            content_hash: &blob.content_hash,
            source_uri: request.source_uri.as_deref(),
            created_at: source.created_at,
            extracted,
        })?;
        let note_hash = hex::encode(Sha256::digest(note.as_bytes()));
        let object = sources::bind_source_object(
            &backend.pool,
            actor,
            source.id,
            object_id,
            &path,
            &note_hash,
            request.overwrite,
        )
        .await
        .map_err(registry_error)?;

        let repository = backend
            .vault_layout
            .open_or_init(actor.tenant_id)
            .map_err(vault_error)?;
        repository
            .ensure_domain(space.id, "core")
            .map_err(vault_error)?;
        let note_state = repository
            .write_note(
                space.id,
                "core",
                &path,
                note.as_bytes(),
                "vault: import source into Personal Space",
            )
            .map_err(vault_error)?;

        let source =
            sources::complete_source(&backend.pool, actor, source.id, source.revision, object.id)
                .await
                .map_err(registry_error)?;
        sources::record_attempt(
            &backend.pool,
            actor.tenant_id,
            attempt(
                &source,
                "extract",
                "succeeded",
                request.source_uri,
                &request.content_type,
                request.bytes.len(),
                &blob.content_hash,
            ),
        )
        .await
        .map_err(registry_error)?;

        let _ = request.auto_enrich;
        Ok(ImportReceipt {
            path,
            canonical_plug: PlugId::new(SOURCE_LEDGER_PLUG)
                .expect("source-ledger is a valid static Plug id"),
            revision: note_state.git_revision,
            byte_size: request.bytes.len() as u64,
            content_hash: blob.content_hash,
            derived_failures: Vec::new(),
            source_id: source.id,
            object_id: object.id,
            object_revision: object.revision,
            blob_existed: blob.existed,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub target_path: Option<String>,
    pub title_hint: Option<String>,
    pub auto_enrich: bool,
    pub overwrite: bool,
    pub source_uri: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportReceipt {
    pub path: String,
    pub canonical_plug: PlugId,
    pub revision: String,
    pub byte_size: u64,
    pub content_hash: String,
    pub derived_failures: Vec<PlugId>,
    pub source_id: Uuid,
    pub object_id: Uuid,
    pub object_revision: i64,
    pub blob_existed: bool,
}

struct SourceNoteInput<'a> {
    object_id: Uuid,
    source_id: Uuid,
    space_id: Uuid,
    title: &'a str,
    content_type: &'a str,
    content_hash: &'a str,
    source_uri: Option<&'a str>,
    created_at: chrono::DateTime<chrono::Utc>,
    extracted: gadgetron_core::ingest::ExtractedDocument,
}

fn build_source_note(input: SourceNoteInput<'_>) -> Result<String, GadgetronError> {
    let mut properties = BTreeMap::new();
    properties.insert("id".into(), serde_json::json!(input.object_id));
    properties.insert("title".into(), serde_json::json!(input.title));
    properties.insert("kind".into(), serde_json::json!("note"));
    properties.insert("status".into(), serde_json::json!("draft"));
    properties.insert("space_id".into(), serde_json::json!(input.space_id));
    properties.insert("home_bundle_id".into(), serde_json::json!("core"));
    properties.insert("source_ids".into(), serde_json::json!([input.source_id]));
    properties.insert(
        "source_hashes".into(),
        serde_json::json!([input.content_hash]),
    );
    properties.insert(
        "source_content_type".into(),
        serde_json::json!(input.content_type),
    );
    properties.insert("source_retention".into(), serde_json::json!("versioned"));
    properties.insert(
        "created".into(),
        serde_json::json!(input.created_at.to_rfc3339()),
    );
    properties.insert(
        "updated".into(),
        serde_json::json!(input.created_at.to_rfc3339()),
    );
    if input.extracted.source_metadata.is_object() {
        properties.insert("extraction".into(), input.extracted.source_metadata);
    }
    if let Some(source_uri) = input.source_uri {
        properties.insert("source_uri".into(), serde_json::json!(source_uri));
    }
    if !input.extracted.warnings.is_empty() {
        properties.insert(
            "extraction_warnings".into(),
            serde_json::json!(input
                .extracted
                .warnings
                .iter()
                .map(|warning| &warning.message)
                .collect::<Vec<_>>()),
        );
    }
    let body = if input
        .extracted
        .plain_text
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim_start().starts_with("# "))
    {
        input.extracted.plain_text
    } else {
        format!("# {}\n\n{}", input.title, input.extracted.plain_text)
    };
    serialize_obsidian_note(&properties, &body)
        .map_err(|error| invalid_error(format!("source note serialization failed: {error}")))
}

fn attempt(
    source: &sources::KnowledgeSourceRow,
    phase: &str,
    outcome: &str,
    final_uri: Option<String>,
    content_type: &str,
    byte_size: usize,
    content_hash: &str,
) -> SourceAttempt {
    SourceAttempt {
        source_id: source.id,
        attempt_no: source.attempt_count,
        phase: phase.to_string(),
        outcome: outcome.to_string(),
        final_uri,
        http_status: None,
        content_type: Some(content_type.to_string()),
        byte_size: Some(byte_size as i64),
        content_hash: Some(content_hash.to_string()),
        failure_code: None,
        failure_detail: None,
    }
}

fn registry_error(error: KnowledgeSpaceError) -> GadgetronError {
    match error {
        KnowledgeSpaceError::InvalidInput(detail) => invalid_error(detail),
        KnowledgeSpaceError::Conflict => invalid_error("source note path already exists"),
        KnowledgeSpaceError::RevisionConflict => {
            invalid_error("source revision changed; refresh and retry")
        }
        KnowledgeSpaceError::NotFound | KnowledgeSpaceError::Forbidden => {
            invalid_error("wiki.import requires an active authenticated tenant user")
        }
        KnowledgeSpaceError::Database(error) => backend_error(error.to_string()),
    }
}

fn vault_error(error: VaultLayoutError) -> GadgetronError {
    match error {
        VaultLayoutError::InvalidNotePath(_) => invalid_error(error.to_string()),
        other => backend_error(other.to_string()),
    }
}

fn invalid_error(reason: impl Into<String>) -> GadgetronError {
    let reason = reason.into();
    GadgetronError::Knowledge {
        kind: KnowledgeErrorKind::InvalidQuery {
            reason: reason.clone(),
        },
        message: reason,
    }
}

fn backend_error(message: impl Into<String>) -> GadgetronError {
    GadgetronError::Knowledge {
        kind: KnowledgeErrorKind::BackendUnavailable {
            plug: SOURCE_LEDGER_PLUG.to_string(),
        },
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_pipeline_is_explicit() {
        assert!(!IngestPipeline::unavailable().is_available());
    }
}
