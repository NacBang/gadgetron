//! R2.2 Source-to-Vault primitives: tenant blob storage, bounded extraction,
//! and Obsidian YAML/legacy TOML note parsing.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use gadgetron_core::ingest::{
    BlobError, BlobId, BlobMetadata, BlobRef, BlobStore, ExtractHints, Extractor, StructureHint,
};
use gadgetron_plug_document_formats::PdfExtractor;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::wiki::secrets::{check_audit_patterns, check_block_patterns};

pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FilesystemBlobStore {
    pool: PgPool,
    root: PathBuf,
    max_bytes: usize,
}

#[derive(Debug, FromRow)]
struct BlobRow {
    id: Uuid,
    tenant_id: Uuid,
    content_hash: String,
    storage_key: String,
    byte_size: i64,
}

impl FilesystemBlobStore {
    pub fn new(pool: PgPool, root: impl Into<PathBuf>) -> Self {
        Self {
            pool,
            root: root.into(),
            max_bytes: MAX_SOURCE_BYTES,
        }
    }

    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes.min(MAX_SOURCE_BYTES);
        self
    }

    fn blob_path(&self, tenant_id: Uuid, storage_key: &str) -> Result<PathBuf, BlobError> {
        validate_storage_key(storage_key)?;
        Ok(self
            .root
            .join("tenants")
            .join(tenant_id.to_string())
            .join("blobs")
            .join(storage_key))
    }

    async fn row(&self, id: BlobId) -> Result<BlobRow, BlobError> {
        sqlx::query_as::<_, BlobRow>(
            "SELECT id, tenant_id, content_hash, storage_key, byte_size \
             FROM knowledge_blobs WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| BlobError::Database(error.to_string()))?
        .ok_or(BlobError::NotFound(id))
    }
}

#[async_trait::async_trait]
impl BlobStore for FilesystemBlobStore {
    async fn put(&self, bytes: &[u8], meta: &BlobMetadata) -> Result<BlobRef, BlobError> {
        if bytes.len() > self.max_bytes {
            return Err(BlobError::TooLarge {
                size: bytes.len() as u64,
                limit: self.max_bytes as u64,
            });
        }
        let tenant_id = Uuid::parse_str(&meta.tenant_id)
            .map_err(|error| BlobError::Database(format!("invalid tenant id: {error}")))?;
        let created_by = Uuid::parse_str(&meta.imported_by)
            .map_err(|error| BlobError::Database(format!("invalid importer id: {error}")))?;
        let digest = hex::encode(Sha256::digest(bytes));
        let content_hash = format!("sha256:{digest}");
        let storage_key = format!("sha256/{}/{digest}", &digest[..2]);
        let path = self.blob_path(tenant_id, &storage_key)?;

        if let Some(existing) = sqlx::query_as::<_, BlobRow>(
            "SELECT id, tenant_id, content_hash, storage_key, byte_size \
             FROM knowledge_blobs WHERE tenant_id = $1 AND content_hash = $2 \
             AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(&content_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| BlobError::Database(error.to_string()))?
        {
            verify_blob_file(&path, &existing.content_hash, existing.byte_size)?;
            return Ok(blob_ref(existing, true));
        }

        write_content_addressed(&path, bytes)?;
        let row = sqlx::query_as::<_, BlobRow>(
            r#"INSERT INTO knowledge_blobs
               (tenant_id, content_hash, storage_key, byte_size, content_type, original_name, created_by)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               ON CONFLICT (tenant_id, content_hash) DO UPDATE SET deleted_at = NULL
               RETURNING id, tenant_id, content_hash, storage_key, byte_size"#,
        )
        .bind(tenant_id)
        .bind(&content_hash)
        .bind(&storage_key)
        .bind(bytes.len() as i64)
        .bind(&meta.content_type)
        .bind(&meta.filename)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| BlobError::Database(error.to_string()))?;
        Ok(blob_ref(row, false))
    }

    async fn get(&self, id: &BlobId) -> Result<Vec<u8>, BlobError> {
        let row = self.row(*id).await?;
        let path = self.blob_path(row.tenant_id, &row.storage_key)?;
        let bytes = fs::read(path)?;
        verify_blob_bytes(&bytes, &row.content_hash, row.byte_size)?;
        Ok(bytes)
    }

    async fn delete(&self, id: &BlobId) -> Result<(), BlobError> {
        let row = self.row(*id).await?;
        let references: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM knowledge_sources WHERE blob_id = $1 AND deleted_at IS NULL",
        )
        .bind(id.0)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| BlobError::Database(error.to_string()))?;
        if references > 0 {
            return Err(BlobError::Database(format!(
                "blob {id} still has {references} live source references"
            )));
        }
        sqlx::query("UPDATE knowledge_blobs SET deleted_at = NOW() WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|error| BlobError::Database(error.to_string()))?;
        let path = self.blob_path(row.tenant_id, &row.storage_key)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn exists(&self, id: &BlobId) -> Result<bool, BlobError> {
        match self.row(*id).await {
            Ok(row) => Ok(self.blob_path(row.tenant_id, &row.storage_key)?.is_file()),
            Err(BlobError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

fn blob_ref(row: BlobRow, existed: bool) -> BlobRef {
    BlobRef {
        id: BlobId(row.id),
        content_hash: row.content_hash,
        storage_uri: format!("vault://{}/blobs/{}", row.tenant_id, row.storage_key),
        byte_size: row.byte_size as u64,
        existed,
    }
}

fn validate_storage_key(value: &str) -> Result<(), BlobError> {
    let parts: Vec<_> = value.split('/').collect();
    let valid = parts.len() == 3
        && parts[0] == "sha256"
        && parts[1].len() == 2
        && parts[2].len() == 64
        && parts[1].bytes().all(|byte| byte.is_ascii_hexdigit())
        && parts[2].bytes().all(|byte| byte.is_ascii_hexdigit());
    if valid {
        Ok(())
    } else {
        Err(BlobError::StorageUnavailable(
            "invalid content-addressed storage key".to_string(),
        ))
    }
}

fn write_content_addressed(path: &Path, bytes: &[u8]) -> Result<(), BlobError> {
    let parent = path
        .parent()
        .ok_or_else(|| BlobError::StorageUnavailable("blob path has no parent".to_string()))?;
    fs::create_dir_all(parent)?;
    if path.exists() {
        return verify_blob_file(
            path,
            &format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
            bytes.len() as i64,
        );
    }
    let temporary = parent.join(format!(".blob-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn verify_blob_file(
    path: &Path,
    expected_hash: &str,
    expected_bytes: i64,
) -> Result<(), BlobError> {
    verify_blob_bytes(&fs::read(path)?, expected_hash, expected_bytes)
}

fn verify_blob_bytes(
    bytes: &[u8],
    expected_hash: &str,
    expected_bytes: i64,
) -> Result<(), BlobError> {
    let actual = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
    if actual != expected_hash || bytes.len() as i64 != expected_bytes {
        return Err(BlobError::StorageUnavailable(
            "content-addressed blob checksum mismatch".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedSource {
    pub markdown: String,
    pub metadata: serde_json::Value,
    pub audit_secret_patterns: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceExtractError {
    #[error("unsupported source content type {0:?}")]
    UnsupportedContentType(String),
    #[error("source bytes are not valid UTF-8")]
    InvalidUtf8,
    #[error("source contains blocked credential pattern {0}")]
    CredentialBlocked(String),
    #[error("PDF signature is missing")]
    InvalidPdfMagic,
    #[error("PDF has no usable text layer")]
    NeedsOcr,
    #[error("source extraction failed: {0}")]
    Extraction(String),
}

impl SourceExtractError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedContentType(_) => "unsupported_content_type",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::CredentialBlocked(_) => "credential_blocked",
            Self::InvalidPdfMagic => "invalid_pdf",
            Self::NeedsOcr => "needs_ocr",
            Self::Extraction(_) => "extraction_failed",
        }
    }

    pub fn needs_ocr(&self) -> bool {
        matches!(self, Self::NeedsOcr)
    }
}

pub async fn extract_source(
    bytes: &[u8],
    content_type: &str,
) -> Result<ExtractedSource, SourceExtractError> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let (markdown, metadata) = match mime.as_str() {
        "text/markdown" | "text/plain" => {
            let text = std::str::from_utf8(bytes).map_err(|_| SourceExtractError::InvalidUtf8)?;
            (text.to_string(), serde_json::json!({"format": mime}))
        }
        "text/html" | "application/xhtml+xml" => {
            std::str::from_utf8(bytes).map_err(|_| SourceExtractError::InvalidUtf8)?;
            let text = html2text::from_read(Cursor::new(bytes), 100)
                .map_err(|error| SourceExtractError::Extraction(error.to_string()))?;
            (
                text,
                serde_json::json!({"format": mime, "render_width": 100}),
            )
        }
        "application/json" => {
            let value: serde_json::Value = serde_json::from_slice(bytes)
                .map_err(|error| SourceExtractError::Extraction(error.to_string()))?;
            let pretty = serde_json::to_string_pretty(&value)
                .map_err(|error| SourceExtractError::Extraction(error.to_string()))?;
            (
                format!("```json\n{pretty}\n```"),
                serde_json::json!({"format": mime}),
            )
        }
        "application/pdf" => {
            if !bytes.starts_with(b"%PDF-") {
                return Err(SourceExtractError::InvalidPdfMagic);
            }
            let extracted = PdfExtractor::new()
                .extract(bytes, "application/pdf", &ExtractHints::default())
                .await
                .map_err(|error| SourceExtractError::Extraction(error.to_string()))?;
            if extracted.plain_text.trim().is_empty() {
                return Err(SourceExtractError::NeedsOcr);
            }
            let mut metadata = extracted.source_metadata;
            let pages: Vec<_> = extracted
                .structure
                .iter()
                .filter_map(|hint| match hint {
                    StructureHint::PageBreak {
                        byte_offset,
                        page_number,
                    } => Some(serde_json::json!({
                        "page": page_number,
                        "byte_offset": byte_offset,
                    })),
                    _ => None,
                })
                .collect();
            if !pages.is_empty() {
                metadata
                    .as_object_mut()
                    .ok_or_else(|| {
                        SourceExtractError::Extraction(
                            "PDF extractor metadata must be a JSON object".to_string(),
                        )
                    })?
                    .insert("pages".to_string(), serde_json::Value::Array(pages));
            }
            (extracted.plain_text, metadata)
        }
        _ => return Err(SourceExtractError::UnsupportedContentType(mime)),
    };

    if let Some(blocked) = check_block_patterns(&markdown).first() {
        return Err(SourceExtractError::CredentialBlocked(
            blocked.pattern_name.to_string(),
        ));
    }
    let mut audit_secret_patterns: Vec<_> = check_audit_patterns(&markdown)
        .into_iter()
        .map(|matched| matched.pattern_name.to_string())
        .collect();
    audit_secret_patterns.sort();
    audit_secret_patterns.dedup();
    Ok(ExtractedSource {
        markdown,
        metadata,
        audit_secret_patterns,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedObsidianNote {
    pub properties: BTreeMap<String, serde_json::Value>,
    pub body: String,
    pub format: NoteFrontmatterFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteFrontmatterFormat {
    None,
    Yaml,
    LegacyToml,
}

#[derive(Debug, thiserror::Error)]
pub enum NoteFormatError {
    #[error("note frontmatter fence is not closed")]
    MissingFence,
    #[error("note frontmatter is neither an object-shaped YAML document nor legacy TOML")]
    InvalidFrontmatter,
    #[error("note frontmatter serialization failed: {0}")]
    Serialize(String),
}

pub fn parse_obsidian_note(raw: &str) -> Result<ParsedObsidianNote, NoteFormatError> {
    let Some(after_open) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return Ok(ParsedObsidianNote {
            properties: BTreeMap::new(),
            body: raw.to_string(),
            format: NoteFrontmatterFormat::None,
        });
    };
    let (frontmatter, body) = split_fenced(after_open).ok_or(NoteFormatError::MissingFence)?;
    if let Ok(properties) = serde_yaml::from_str::<BTreeMap<String, serde_json::Value>>(frontmatter)
    {
        return Ok(ParsedObsidianNote {
            properties,
            body: body.to_string(),
            format: NoteFrontmatterFormat::Yaml,
        });
    }
    let legacy: toml::Table =
        toml::from_str(frontmatter).map_err(|_| NoteFormatError::InvalidFrontmatter)?;
    let properties = legacy
        .into_iter()
        .map(|(key, value)| {
            serde_json::to_value(value)
                .map(|value| (key, value))
                .map_err(|_| NoteFormatError::InvalidFrontmatter)
        })
        .collect::<Result<_, _>>()?;
    Ok(ParsedObsidianNote {
        properties,
        body: body.to_string(),
        format: NoteFrontmatterFormat::LegacyToml,
    })
}

pub fn serialize_obsidian_note(
    properties: &BTreeMap<String, serde_json::Value>,
    body: &str,
) -> Result<String, NoteFormatError> {
    if properties.is_empty() {
        return Ok(body.to_string());
    }
    let yaml = serde_yaml::to_string(properties)
        .map_err(|error| NoteFormatError::Serialize(error.to_string()))?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let body_separator = if body.starts_with('\n') { "" } else { "\n" };
    Ok(format!(
        "---\n{}\n---{body_separator}{body}",
        yaml.trim_end()
    ))
}

fn split_fenced(raw: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    for segment in raw.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']);
        if line == "---" {
            let body = &raw[offset + segment.len()..];
            return Some((&raw[..offset], body));
        }
        offset += segment.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsidian_yaml_and_legacy_toml_both_parse_and_new_write_is_yaml() {
        let yaml = "---\nid: 8f6f8fea-88fe-47f7-a2c4-144c0cf08b66\ntags:\n  - ops\n---\n# Body\n";
        let parsed = parse_obsidian_note(yaml).unwrap();
        assert_eq!(parsed.format, NoteFrontmatterFormat::Yaml);
        assert_eq!(parsed.body, "# Body\n");
        let rewritten = serialize_obsidian_note(&parsed.properties, &parsed.body).unwrap();
        assert!(rewritten.contains("id: 8f6f8fea-88fe-47f7-a2c4-144c0cf08b66"));
        assert!(!rewritten.contains("id ="));

        let toml = "---\ntags = [\"legacy\"]\ntype = \"note\"\n---\nlegacy\n";
        let parsed = parse_obsidian_note(toml).unwrap();
        assert_eq!(parsed.format, NoteFrontmatterFormat::LegacyToml);
        assert_eq!(parsed.body, "legacy\n");
    }

    #[tokio::test]
    async fn extraction_rejects_credentials_and_scanned_pdf_shape() {
        let blocked = extract_source(b"AKIAIOSFODNN7EXAMPLE", "text/plain")
            .await
            .unwrap_err();
        assert_eq!(blocked.code(), "credential_blocked");
        let invalid = extract_source(b"not pdf", "application/pdf")
            .await
            .unwrap_err();
        assert_eq!(invalid.code(), "invalid_pdf");
        let unsupported = extract_source(b"binary", "application/octet-stream")
            .await
            .unwrap_err();
        assert_eq!(unsupported.code(), "unsupported_content_type");
    }

    #[tokio::test]
    async fn json_source_is_pretty_printed_without_losing_provider_fields() {
        let extracted = extract_source(
            br#"{"items":[{"question_id":42,"content_license":"CC BY-SA 4.0"}],"has_more":false}"#,
            "application/json; charset=utf-8",
        )
        .await
        .unwrap();

        assert!(extracted.markdown.starts_with("```json\n{"));
        assert!(extracted.markdown.contains("\"question_id\": 42"));
        assert!(extracted
            .markdown
            .contains("\"content_license\": \"CC BY-SA 4.0\""));
        assert_eq!(extracted.metadata["format"], "application/json");
    }

    #[tokio::test]
    async fn source_dispatch_extracts_a_text_layer_pdf_with_page_metadata() {
        use base64::Engine as _;
        let mut pdf = base64::engine::general_purpose::STANDARD
            .decode("JVBERi0xLjQKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBvYmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUl0gL0NvdW50IDEgPj4KZW5kb2JqCjMgMCBvYmoKPDwgL1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCA2MTIgNzkyXSAvUmVzb3VyY2VzIDw8IC9Gb250IDw8IC9GMSA0IDAgUiA+PiA+PiAvQ29udGVudHMgNSAwIFIgPj4KZW5kb2JqCjQgMCBvYmoKPDwgL1R5cGUgL0ZvbnQgL1N1YnR5cGUgL1R5cGUxIC9CYXNlRm9udCAvSGVsdmV0aWNhID4+CmVuZG9iago1IDAgb2JqCjw8IC9MZW5ndGggNTggPj4Kc3RyZWFtCkJUCi9GMSAyNCBUZgo3MiA3MjAgVGQKKEhlbGxvIFdvcmxkIGZyb20gR2FkZ2V0cm9uKSBUagpFVAplbmRzdHJlYW0KZW5kb2JqCnhyZWYKMCA2CjAwMDAwMDAwMDAgNjU1MzUgZiAKMDAwMDAwMDAwOSAwMDAwMCBuIAowMDAwMDAwMDU4IDAwMDAwIG4gCjAwMDAwMDAxMTUgMDAwMDAgbiAKMDAwMDAwMDI0MSAwMDAwMCBuIAowMDAwMDAwMzExIDAwMDAwIG4gCnRyYWlsZXIKPDwgL1NpemUgNiAvUm9vdCAxIDAgUiA+PgpzdGFydHhyZWYKNDE4CiUlRU9GCg==")
            .unwrap();
        let extracted = extract_source(&pdf, "application/pdf").await.unwrap();
        assert!(extracted.markdown.contains("Hello World from Gadgetron"));
        assert_eq!(extracted.metadata["source_format"], "pdf");
        assert_eq!(extracted.metadata["page_count"], 1);

        let needle = b"Hello World from Gadgetron";
        let start = pdf
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap();
        pdf[start..start + needle.len()].fill(b' ');
        let scanned = extract_source(&pdf, "application/pdf").await.unwrap_err();
        assert!(scanned.needs_ocr());
        assert_eq!(scanned.code(), "needs_ocr");
    }

    #[tokio::test]
    async fn source_dispatch_preserves_pdf_page_break_locators() {
        use base64::Engine as _;
        let pdf = base64::engine::general_purpose::STANDARD
            .decode("JVBERi0xLjQKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBvYmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUiA2IDAgUl0gL0NvdW50IDIgPj4KZW5kb2JqCjMgMCBvYmoKPDwgL1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCA2MTIgNzkyXSAvUmVzb3VyY2VzIDw8IC9Gb250IDw8IC9GMSA0IDAgUiA+PiA+PiAvQ29udGVudHMgNSAwIFIgPj4KZW5kb2JqCjQgMCBvYmoKPDwgL1R5cGUgL0ZvbnQgL1N1YnR5cGUgL1R5cGUxIC9CYXNlRm9udCAvSGVsdmV0aWNhID4+CmVuZG9iago1IDAgb2JqCjw8IC9MZW5ndGggNDUgPj4Kc3RyZWFtCkJUCi9GMSAyNCBUZgo3MiA3MjAgVGQKKFBhZ2UgT25lIFRleHQpIFRqCkVUCmVuZHN0cmVhbQplbmRvYmoKNiAwIG9iago8PCAvVHlwZSAvUGFnZSAvUGFyZW50IDIgMCBSIC9NZWRpYUJveCBbMCAwIDYxMiA3OTJdIC9SZXNvdXJjZXMgPDwgL0ZvbnQgPDwgL0YxIDQgMCBSID4+ID4+IC9Db250ZW50cyA3IDAgUiA+PgplbmRvYmoKNyAwIG9iago8PCAvTGVuZ3RoIDQ4ID4+CnN0cmVhbQpCVAovRjEgMjQgVGYKNzIgNzIwIFRkCihQYWdlIFR3byBDb250ZW50KSBUagpFVAplbmRzdHJlYW0KZW5kb2JqCnhyZWYKMCA4CjAwMDAwMDAwMDAgNjU1MzUgZiAKMDAwMDAwMDAwOSAwMDAwMCBuIAowMDAwMDAwMDU4IDAwMDAwIG4gCjAwMDAwMDAxMjEgMDAwMDAgbiAKMDAwMDAwMDI0NyAwMDAwMCBuIAowMDAwMDAwMzE3IDAwMDAwIG4gCjAwMDAwMDA0MTEgMDAwMDAgbiAKMDAwMDAwMDUzNyAwMDAwMCBuIAp0cmFpbGVyCjw8IC9TaXplIDggL1Jvb3QgMSAwIFIgPj4Kc3RhcnR4cmVmCjYzNAolJUVPRgo=")
            .unwrap();

        let extracted = extract_source(&pdf, "application/pdf").await.unwrap();
        assert_eq!(extracted.metadata["page_count"], 2);
        let pages = extracted.metadata["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["page"], 2);
        let byte_offset = pages[0]["byte_offset"].as_u64().unwrap() as usize;
        assert_eq!(extracted.markdown.as_bytes()[byte_offset], b'\x0c');
    }
}
