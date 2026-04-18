//! `IngestPipeline` — the RAW → wiki orchestration path.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §4.4.
//!
//! # Why a separate struct (vs a free function)
//!
//! The pipeline owns its dependencies (`KnowledgeService`, future blob store,
//! future enrichment client). Holding them on `Self` lets the `wiki.import`
//! Gadget call `pipeline.import(...)` without re-threading three `Arc`s per
//! call. A free function would work today but is guaranteed to grow state in
//! W3-KL-3 (blob store, enricher) — starting with the struct keeps the call
//! surface stable.
//!
//! # Invariants (see module-level doc in `mod.rs`)
//!
//! - `KnowledgeService::write` is the canonical write path. No backdoor.
//! - `content_hash` is `sha256("sha256:" + hex(bytes))` — opaque to downstream
//!   pipelines, consumed only as a frontmatter string.
//! - Frontmatter is composed BEFORE the service write, then prepended to the
//!   extracted markdown body. The llm-wiki store will parse it back on read.

use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use gadgetron_core::bundle::PlugId;
use gadgetron_core::error::GadgetronError;
use gadgetron_core::ingest::{ExtractHints, Extractor};
use gadgetron_core::knowledge::{AuthenticatedContext, KnowledgePutRequest};
use sha2::{Digest, Sha256};

use crate::ingest::title::{resolve_target_path, resolve_title};
use crate::service::KnowledgeService;

/// Pipeline entry point. Cheap to clone (holds `Arc`s). One instance per
/// `KnowledgeGadgetProvider` / CLI / web handler is the expected model.
pub struct IngestPipeline {
    service: Arc<KnowledgeService>,
}

impl std::fmt::Debug for IngestPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngestPipeline")
            .field("canonical_plug", self.service.canonical_plug())
            .finish()
    }
}

impl IngestPipeline {
    /// Build a pipeline bound to one `KnowledgeService`. The extractor is
    /// caller-chosen per-import (via the `import` method) so the same
    /// pipeline can serve markdown, PDF, HTML, etc. without refit.
    pub fn new(service: Arc<KnowledgeService>) -> Self {
        Self { service }
    }

    /// Expose the underlying `KnowledgeService` — needed by the gadget
    /// layer when surfacing list/search/get alongside import.
    pub fn service(&self) -> &Arc<KnowledgeService> {
        &self.service
    }

    /// Execute one end-to-end import.
    ///
    /// Steps per design 11 §4.4:
    ///
    /// 1. extract
    /// 2. resolve title
    /// 3. resolve target path
    /// 4. build markdown body (extractor output is treated as markdown)
    /// 5. prepend frontmatter
    /// 6. delegate to `KnowledgeService::write`
    /// 7. hash the original bytes for citation
    ///
    /// The `extractor` is passed by value (`Arc`) so the caller can pick
    /// an appropriate impl per content-type without the pipeline carrying
    /// a registry.
    #[tracing::instrument(
        level = "info",
        name = "knowledge.import",
        skip(self, actor, request, extractor),
        fields(
            content_type = %request.content_type,
            byte_size = request.bytes.len(),
        )
    )]
    pub async fn import(
        &self,
        actor: &AuthenticatedContext,
        request: ImportRequest,
        extractor: Arc<dyn Extractor>,
    ) -> Result<ImportReceipt, GadgetronError> {
        // Step 1: extract. Propagate extractor errors as
        // `GadgetronError::Config` since extractor plumbing is an operator
        // concern — the wire error code (`extract_*`) is available via
        // `.code()` for richer surfacing in W3-KL-3.
        let extracted = extractor
            .extract(
                &request.bytes,
                &request.content_type,
                &ExtractHints::default(),
            )
            .await
            .map_err(|e| {
                GadgetronError::Config(format!(
                    "extractor {:?} failed ({}): {e}",
                    extractor.name(),
                    e.code()
                ))
            })?;

        // Step 2: title resolution. Explicit hint > first heading > "imported".
        let title = resolve_title(
            request.title_hint.as_deref(),
            &extracted.structure,
            "imported",
        );

        // Step 3: target-path resolution. `overwrite = true` is honored
        // by the store when the path collides; the pipeline does NOT
        // decide supersession — that's in W3-KL-3 (design 11 §9.2).
        let target_path = resolve_target_path(request.target_path.as_deref(), &title);

        // Step 7 (early, for frontmatter): hash the original bytes.
        // Conceptually step 7 per the docstring above but we need the
        // string before step 5.
        let content_hash = sha256_hex_prefixed(&request.bytes);

        // Step 4+5: frontmatter + body. The frontmatter uses TOML per the
        // existing `wiki::frontmatter` format so the llm-wiki store can
        // parse it back without a new parser.
        let frontmatter = compose_frontmatter(&FrontmatterInput {
            source_content_type: &request.content_type,
            source_bytes_hash: &content_hash,
            imported_by: "penny", // W3-KL-2 placeholder; identity lands in 08 doc
            source_uri: request.source_uri.as_deref(),
            title: Some(&title),
        });
        let markdown = format!("{frontmatter}{body}", body = extracted.plain_text);

        // Step 6: delegate to KnowledgeService — the W3-KL-1 invariant.
        // The pipeline MUST NOT write directly to `LlmWikiStore`. If this
        // line regresses to `self.store.put(...)` the whole knowledge plane
        // cutover is undone.
        let receipt = self
            .service
            .write(
                actor,
                KnowledgePutRequest {
                    path: target_path,
                    markdown,
                    create_only: !request.overwrite,
                    overwrite: request.overwrite,
                    // Ingest pipeline (RAW import) has no candidate provenance.
                    provenance: Default::default(),
                },
            )
            .await?;

        Ok(ImportReceipt {
            path: receipt.path,
            canonical_plug: receipt.canonical_plug,
            revision: receipt.revision,
            byte_size: request.bytes.len() as u64,
            content_hash,
            derived_failures: receipt.derived_failures,
        })
    }
}

/// Caller input to [`IngestPipeline::import`].
///
/// Field docstrings mirror [`gadgetron_core::ingest::ImportOpts`] but this
/// struct additionally carries the bytes + content-type for a one-shot
/// import. The MCP tool schema (`wiki.import`) deserializes directly into
/// this shape (minus `bytes`, which arrives base64-encoded).
#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub target_path: Option<String>,
    pub title_hint: Option<String>,
    /// W3-KL-2: caller may set `true` but the pipeline ignores it — it
    /// always treats auto-enrichment as off. Enrichment plumbing lands in
    /// W3-KL-3 alongside the Penny RAG system prompt.
    pub auto_enrich: bool,
    pub overwrite: bool,
    pub source_uri: Option<String>,
}

/// Receipt returned by [`IngestPipeline::import`].
///
/// `byte_size` / `content_hash` are the citation anchors — surfaced in
/// frontmatter and echoed here so the caller (Penny / CLI / web UI) can
/// show a confirmation without re-reading the page. `derived_failures`
/// mirrors the `KnowledgeWriteReceipt` field per design 11 §4.4 step 7.
#[derive(Debug, Clone)]
pub struct ImportReceipt {
    pub path: String,
    pub canonical_plug: PlugId,
    pub revision: String,
    pub byte_size: u64,
    pub content_hash: String,
    pub derived_failures: Vec<PlugId>,
}

struct FrontmatterInput<'a> {
    source_content_type: &'a str,
    source_bytes_hash: &'a str,
    imported_by: &'a str,
    source_uri: Option<&'a str>,
    title: Option<&'a str>,
}

/// Emit the ingest frontmatter block per design 11 §5.2. Returns a string
/// that, when prepended to the extracted markdown body, yields a complete
/// wiki page parseable by `wiki::frontmatter::parse_page`.
///
/// Format:
///
/// ```text
/// ---
/// source = "imported"
/// source_content_type = "..."
/// source_bytes_hash = "sha256:..."
/// source_imported_at = "2026-04-18T10:24:00Z"
/// imported_by = "..."
/// title = "..."             (optional)
/// source_uri = "..."        (optional)
/// ---
/// <body>
/// ```
fn compose_frontmatter(input: &FrontmatterInput<'_>) -> String {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut out = String::with_capacity(256);
    out.push_str("---\n");
    out.push_str("source = \"imported\"\n");
    out.push_str(&format!(
        "source_content_type = \"{}\"\n",
        toml_escape(input.source_content_type)
    ));
    out.push_str(&format!(
        "source_bytes_hash = \"{}\"\n",
        toml_escape(input.source_bytes_hash)
    ));
    out.push_str(&format!("source_imported_at = \"{now}\"\n"));
    out.push_str(&format!(
        "imported_by = \"{}\"\n",
        toml_escape(input.imported_by)
    ));
    if let Some(t) = input.title {
        out.push_str(&format!("title = \"{}\"\n", toml_escape(t)));
    }
    if let Some(uri) = input.source_uri {
        out.push_str(&format!("source_uri = \"{}\"\n", toml_escape(uri)));
    }
    out.push_str("---\n");
    out
}

/// Minimal TOML basic-string escape. Enough for the fields we emit in
/// frontmatter (MIME types, UUIDs, RFC3339 timestamps, kebab-case
/// identifiers); does NOT attempt full TOML spec compliance — the
/// `wiki::frontmatter` parser reads back what we write. If a caller
/// sneaks a control character in, they'll trip the TOML parser loudly.
fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn sha256_hex_prefixed(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex::encode(digest))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WikiConfig;
    use crate::llm_wiki::LlmWikiStore;
    use crate::service::KnowledgeServiceBuilder;
    use crate::wiki::Wiki;
    use async_trait::async_trait;
    use gadgetron_core::ingest::{ExtractError, ExtractedDocument, StructureHint};
    use tempfile::TempDir;

    /// Minimal extractor that returns what it's told to. Used to exercise
    /// title-fallback branches without baking in MarkdownExtractor
    /// coupling.
    #[derive(Debug)]
    struct CannedExtractor {
        plain_text: String,
        structure: Vec<StructureHint>,
    }

    #[async_trait]
    impl Extractor for CannedExtractor {
        fn name(&self) -> &str {
            "canned-test-extractor"
        }
        fn supported_content_types(&self) -> &[&str] {
            &["text/markdown"]
        }
        async fn extract(
            &self,
            _bytes: &[u8],
            _content_type: &str,
            _hints: &ExtractHints,
        ) -> Result<ExtractedDocument, ExtractError> {
            Ok(ExtractedDocument {
                plain_text: self.plain_text.clone(),
                structure: self.structure.clone(),
                source_metadata: serde_json::Value::Null,
                warnings: Vec::new(),
            })
        }
    }

    fn fresh_pipeline() -> (TempDir, Arc<IngestPipeline>) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "t@test.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let store = Arc::new(LlmWikiStore::new(wiki).unwrap());
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .build()
            .unwrap();
        (dir, Arc::new(IngestPipeline::new(svc)))
    }

    #[tokio::test]
    async fn ingest_pipeline_imports_markdown_into_canonical_store() {
        // End-to-end happy path: canned extractor returns "# Hello\nBody",
        // pipeline writes the page via the service, and the returned
        // receipt reports the canonical plug id.
        let (_dir, pipeline) = fresh_pipeline();
        let actor = AuthenticatedContext;
        let extractor: Arc<dyn Extractor> = Arc::new(CannedExtractor {
            plain_text: "# Hello\n\nBody".into(),
            structure: vec![StructureHint::Heading {
                level: 1,
                byte_offset: 0,
                text: "Hello".into(),
            }],
        });

        let receipt = pipeline
            .import(
                &actor,
                ImportRequest {
                    bytes: b"# Hello\n\nBody".to_vec(),
                    content_type: "text/markdown".into(),
                    target_path: None,
                    title_hint: None,
                    auto_enrich: false,
                    overwrite: false,
                    source_uri: None,
                },
                extractor,
            )
            .await
            .expect("import must succeed");

        assert_eq!(receipt.path, "imports/hello");
        assert_eq!(receipt.canonical_plug.as_str(), "llm-wiki");
        assert!(!receipt.content_hash.is_empty());
        assert!(receipt.content_hash.starts_with("sha256:"));
        assert_eq!(receipt.byte_size, 13);
    }

    #[tokio::test]
    async fn ingest_pipeline_uses_title_hint_when_provided() {
        // Explicit `title_hint` wins over the first-heading fallback —
        // regression guard for the precedence order in `resolve_title`.
        let (_dir, pipeline) = fresh_pipeline();
        let actor = AuthenticatedContext;
        let extractor: Arc<dyn Extractor> = Arc::new(CannedExtractor {
            plain_text: "# Heading\n".into(),
            structure: vec![StructureHint::Heading {
                level: 1,
                byte_offset: 0,
                text: "Heading".into(),
            }],
        });

        let receipt = pipeline
            .import(
                &actor,
                ImportRequest {
                    bytes: b"# Heading\n".to_vec(),
                    content_type: "text/markdown".into(),
                    target_path: None,
                    title_hint: Some("Custom Title".into()),
                    auto_enrich: false,
                    overwrite: false,
                    source_uri: None,
                },
                extractor,
            )
            .await
            .expect("import");
        assert_eq!(receipt.path, "imports/custom-title");
    }

    #[tokio::test]
    async fn ingest_pipeline_falls_back_to_first_heading() {
        // No title_hint → resolver pulls "Doc Heading" from the
        // StructureHint array and kebabs it into the path.
        let (_dir, pipeline) = fresh_pipeline();
        let actor = AuthenticatedContext;
        let extractor: Arc<dyn Extractor> = Arc::new(CannedExtractor {
            plain_text: "# Doc Heading\n\n## Sub".into(),
            structure: vec![
                StructureHint::Heading {
                    level: 1,
                    byte_offset: 0,
                    text: "Doc Heading".into(),
                },
                StructureHint::Heading {
                    level: 2,
                    byte_offset: 20,
                    text: "Sub".into(),
                },
            ],
        });

        let receipt = pipeline
            .import(
                &actor,
                ImportRequest {
                    bytes: b"# Doc Heading\n\n## Sub".to_vec(),
                    content_type: "text/markdown".into(),
                    target_path: None,
                    title_hint: None,
                    auto_enrich: false,
                    overwrite: false,
                    source_uri: None,
                },
                extractor,
            )
            .await
            .expect("import");
        assert_eq!(receipt.path, "imports/doc-heading");
    }

    #[tokio::test]
    async fn ingest_pipeline_composes_frontmatter_with_source_fields() {
        // Frontmatter must carry `source = "imported"` + content-type +
        // hash. Read the page back through the service and inspect.
        let (_dir, pipeline) = fresh_pipeline();
        let actor = AuthenticatedContext;
        let extractor: Arc<dyn Extractor> = Arc::new(CannedExtractor {
            plain_text: "# Hello\n".into(),
            structure: vec![StructureHint::Heading {
                level: 1,
                byte_offset: 0,
                text: "Hello".into(),
            }],
        });

        let receipt = pipeline
            .import(
                &actor,
                ImportRequest {
                    bytes: b"# Hello\n".to_vec(),
                    content_type: "text/markdown".into(),
                    target_path: Some("notes/custom".into()),
                    title_hint: None,
                    auto_enrich: false,
                    overwrite: false,
                    source_uri: Some("https://example.com/post".into()),
                },
                extractor,
            )
            .await
            .expect("import");

        let svc = pipeline.service();
        let doc = svc
            .get(&actor, &receipt.path)
            .await
            .expect("get")
            .expect("page exists");
        let fm = doc.frontmatter.as_object().expect("frontmatter is object");
        assert_eq!(fm.get("source").and_then(|v| v.as_str()), Some("imported"));
        // `source_content_type`, `source_bytes_hash`, etc. aren't on the
        // strongly-typed `WikiFrontmatter` struct (they land under
        // `extra` — `parse_page` flattens them). Verify by round-tripping
        // through `extra`.
        let extra = fm.get("extra");
        let combined = match extra {
            Some(v) if v.is_object() => v.clone(),
            _ => serde_json::Value::Object(fm.clone()),
        };
        let obj = combined.as_object().unwrap();
        // Accept either top-level or extra-nested shape depending on
        // how serde serializes the flattened HashMap.
        let hash = obj
            .get("source_bytes_hash")
            .or_else(|| fm.get("source_bytes_hash"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            hash.starts_with("sha256:"),
            "frontmatter should carry source_bytes_hash; got {fm:?}"
        );

        // Source URI must be present.
        let uri = obj
            .get("source_uri")
            .or_else(|| fm.get("source_uri"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(uri, "https://example.com/post");
    }

    #[tokio::test]
    async fn ingest_pipeline_propagates_derived_failures() {
        // When the canonical store write succeeds but a derived index
        // fails, `ImportReceipt::derived_failures` must surface the
        // failing plug. Uses `FakeFailingIndex` inline so the test is
        // self-contained.
        use async_trait::async_trait;
        use gadgetron_core::error::KnowledgeErrorKind;
        use gadgetron_core::knowledge::{
            KnowledgeChangeEvent, KnowledgeHit, KnowledgeIndex, KnowledgeQuery, KnowledgeQueryMode,
        };

        #[derive(Debug)]
        struct FailingIdx {
            plug_id: PlugId,
        }
        #[async_trait]
        impl KnowledgeIndex for FailingIdx {
            fn plug_id(&self) -> &PlugId {
                &self.plug_id
            }
            fn mode(&self) -> KnowledgeQueryMode {
                KnowledgeQueryMode::Keyword
            }
            async fn search(
                &self,
                _: &AuthenticatedContext,
                _q: &KnowledgeQuery,
            ) -> Result<Vec<KnowledgeHit>, GadgetronError> {
                Ok(Vec::new())
            }
            async fn reset(&self) -> Result<(), GadgetronError> {
                Ok(())
            }
            async fn apply(
                &self,
                _: &AuthenticatedContext,
                _event: KnowledgeChangeEvent,
            ) -> Result<(), GadgetronError> {
                Err(GadgetronError::Knowledge {
                    kind: KnowledgeErrorKind::BackendUnavailable {
                        plug: "flaky-derived".into(),
                    },
                    message: "simulated".into(),
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "t@test.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let store = Arc::new(LlmWikiStore::new(wiki).unwrap());
        let failing = Arc::new(FailingIdx {
            plug_id: PlugId::new("flaky-derived").unwrap(),
        });
        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_index(failing)
            .build()
            .unwrap();
        let pipeline = Arc::new(IngestPipeline::new(svc));
        let extractor: Arc<dyn Extractor> = Arc::new(CannedExtractor {
            plain_text: "# X".into(),
            structure: vec![StructureHint::Heading {
                level: 1,
                byte_offset: 0,
                text: "X".into(),
            }],
        });
        let receipt = pipeline
            .import(
                &AuthenticatedContext,
                ImportRequest {
                    bytes: b"# X".to_vec(),
                    content_type: "text/markdown".into(),
                    target_path: None,
                    title_hint: None,
                    auto_enrich: false,
                    overwrite: false,
                    source_uri: None,
                },
                extractor,
            )
            .await
            .expect("store write still succeeds under StoreOnly");
        assert_eq!(receipt.derived_failures.len(), 1);
        assert_eq!(receipt.derived_failures[0].as_str(), "flaky-derived");
    }
}
