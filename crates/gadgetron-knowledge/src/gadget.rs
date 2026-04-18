//! Gadget provider for the knowledge layer.
//!
//! Implements `gadgetron_core::agent::tools::GadgetProvider` for the
//! `"knowledge"` category. Exposes wiki CRUD/search plus optional
//! `web.search` and semantic search variants.
//!
//! Terminology (per `docs/architecture/glossary.md`):
//! - **Gadget** = MCP tool consumed by Penny. Defined by a `GadgetSchema`.
//! - **GadgetProvider** = Rust supplier of Gadgets, owned by a Bundle.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §6.2 + §6.3`,
//! `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`,
//! `docs/design/core/knowledge-plug-architecture.md §2.2.2`.
//!
//! # W3-KL-1 delegation
//!
//! This module used to hold `Arc<Wiki>` + `Arc<SemanticBackend>` directly
//! and call them both for every `wiki.*` gadget. After the W3-KL-1 cutover
//! it holds `Arc<KnowledgeService>` and routes through the knowledge plane
//! contract. The `wiki.*` gadget surface + error variants are preserved
//! verbatim so Penny prompts, CLI, Web UI, and 576+ existing tests keep
//! working — the plumbing change is invisible to external callers.

use std::sync::Arc;

use async_trait::async_trait;
use gadgetron_core::agent::config::{EnvResolver, StdEnv};
use gadgetron_core::agent::tools::{
    GadgetError, GadgetProvider, GadgetResult, GadgetSchema, GadgetTier,
};
use gadgetron_core::error::{GadgetronError, KnowledgeErrorKind, WikiErrorKind};
use gadgetron_core::knowledge::{
    AuthenticatedContext, KnowledgeHit, KnowledgeHitKind, KnowledgePutRequest, KnowledgeQuery,
    KnowledgeQueryMode,
};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use base64::Engine;
use gadgetron_core::ingest::{
    ExtractError, ExtractHints, ExtractedDocument, Extractor, StructureHint,
};

use crate::config::KnowledgeConfig;
use crate::error::{SearchError, WikiError};
use crate::ingest::{ImportRequest, IngestPipeline};
use crate::keyword_index::WikiKeywordIndex;
use crate::llm_wiki::LlmWikiStore;
use crate::search::{SearxngClient, WebSearch};
use crate::semantic::{normalize_page_content, SemanticBackend};
use crate::semantic_index::SemanticPgVectorIndex;
use crate::service::{KnowledgeService, KnowledgeServiceBuilder};
use crate::wiki::Wiki;

/// The concrete `GadgetProvider` for knowledge-layer tools.
///
/// Holds one `Arc<KnowledgeService>` that owns the canonical store
/// (`LlmWikiStore`) + keyword index (`WikiKeywordIndex`) + optional
/// semantic index (`SemanticPgVectorIndex`). `web.search` is separate —
/// it is NOT in the knowledge plane per authority doc §2.1.3.
///
/// `normalize_on_write` matches the legacy behavior: when semantic
/// indexing is enabled, incoming wiki writes are run through
/// `normalize_page_content` to inject frontmatter defaults before the
/// canonical store sees them.
pub struct KnowledgeGadgetProvider {
    service: Arc<KnowledgeService>,
    web_search: Option<Arc<dyn WebSearch>>,
    max_search_results: usize,
    /// True when a semantic index is registered on the service, so writes
    /// should normalize frontmatter (created/updated/source defaults) per
    /// `docs/design/phase2/05-knowledge-semantic.md §6`.
    normalize_on_write: bool,
    /// RAW ingestion pipeline. Always constructed; wires through the same
    /// `KnowledgeService` as the other wiki.* gadgets so `wiki.import`
    /// goes through the knowledge plane contract (`write()` delegation,
    /// derived-plug fanout, etc.) — see design 11 §4.4 and
    /// `docs/design/core/knowledge-plug-architecture.md` §3.3.
    ingest_pipeline: Arc<IngestPipeline>,
    /// Default markdown extractor — an internal near-noop used by
    /// `wiki.import` when the operator hasn't registered a Bundle
    /// extractor yet (W3-KL-2 is Bundle-less on the production path).
    /// Design 11 §8.1: markdown needs no heavyweight parsing, so the
    /// default guarantees `wiki.import(..., content_type: "text/markdown")`
    /// works out of the box even on the simplest deployment.
    default_markdown_extractor: Arc<dyn Extractor>,
}

impl KnowledgeGadgetProvider {
    /// Build the provider from a validated `KnowledgeConfig`.
    ///
    /// `pg_pool` is optional so the same config can still expose keyword-only
    /// wiki tools when semantic indexing is configured but PostgreSQL is not
    /// reachable in the current process.
    pub fn new(config: KnowledgeConfig, pg_pool: Option<PgPool>) -> Result<Self, WikiError> {
        Self::new_with_env(config, pg_pool, &StdEnv)
    }

    pub fn new_with_env(
        config: KnowledgeConfig,
        pg_pool: Option<PgPool>,
        env: &dyn EnvResolver,
    ) -> Result<Self, WikiError> {
        let wiki_config = config.to_wiki_config().map_err(|msg| {
            WikiError::kind_with_message(
                WikiErrorKind::GitCorruption {
                    path: String::new(),
                    reason: msg.clone(),
                },
                format!("knowledge config translation failed: {msg}"),
            )
        })?;
        let wiki = Arc::new(Wiki::open(wiki_config)?);

        let (web_search, max_search_results): (Option<Arc<dyn WebSearch>>, usize) = match config
            .search
        {
            Some(sc) => {
                let max = sc.max_results as usize;
                let client = SearxngClient::new(&sc).map_err(|e| {
                    WikiError::kind_with_message(
                        WikiErrorKind::GitCorruption {
                            path: String::new(),
                            reason: format!("searxng client construction failed: {e}"),
                        },
                        "failed to build SearXNG client from [knowledge.search] config".to_string(),
                    )
                })?;
                (Some(Arc::new(client) as Arc<dyn WebSearch>), max.max(1))
            }
            None => (None, 10),
        };

        let semantic_backend =
            SemanticBackend::from_config(pg_pool, config.embedding.as_ref(), env)?.map(Arc::new);

        Self::build_service(wiki, web_search, semantic_backend, max_search_results)
    }

    /// Construct directly from an already-opened `Wiki` + optional
    /// `WebSearch`. Used by tests and by callers that want to share a
    /// `Wiki` across multiple provider instances.
    pub fn with_components(
        wiki: Arc<Wiki>,
        web_search: Option<Arc<dyn WebSearch>>,
        max_search_results: usize,
    ) -> Self {
        Self::build_service(wiki, web_search, None, max_search_results)
            .expect("in-memory service construction cannot fail")
    }

    #[cfg(test)]
    pub(crate) fn with_components_and_semantic(
        wiki: Arc<Wiki>,
        web_search: Option<Arc<dyn WebSearch>>,
        semantic: Option<Arc<SemanticBackend>>,
        max_search_results: usize,
    ) -> Self {
        Self::build_service(wiki, web_search, semantic, max_search_results)
            .expect("in-memory service construction cannot fail")
    }

    /// Construct from a pre-built `KnowledgeService`. The authoritative
    /// entrypoint once bundle integration lands — bundles build their own
    /// `KnowledgeService` via the `ctx.plugs.knowledge_stores` etc.
    /// registries.
    pub fn from_service(
        service: Arc<KnowledgeService>,
        web_search: Option<Arc<dyn WebSearch>>,
        max_search_results: usize,
    ) -> Self {
        let normalize = service
            .index_plugs()
            .any(|p| p.as_str() == "semantic-pgvector");
        let ingest_pipeline = Arc::new(IngestPipeline::new(service.clone()));
        let default_markdown_extractor: Arc<dyn Extractor> =
            Arc::new(InternalMarkdownExtractor::new());
        Self {
            service,
            web_search,
            max_search_results: max_search_results.max(1),
            normalize_on_write: normalize,
            ingest_pipeline,
            default_markdown_extractor,
        }
    }

    fn build_service(
        wiki: Arc<Wiki>,
        web_search: Option<Arc<dyn WebSearch>>,
        semantic: Option<Arc<SemanticBackend>>,
        max_search_results: usize,
    ) -> Result<Self, WikiError> {
        let store =
            Arc::new(LlmWikiStore::new(wiki).map_err(|e| {
                WikiError::Frontmatter(format!("llm-wiki construction failed: {e}"))
            })?);
        let keyword = Arc::new(
            WikiKeywordIndex::new()
                .map_err(|e| WikiError::Frontmatter(format!("wiki-keyword construction: {e}")))?,
        );
        let mut builder = KnowledgeServiceBuilder::new()
            .canonical_store(store)
            .add_index(keyword);
        if let Some(backend) = semantic.clone() {
            let sem_idx = Arc::new(SemanticPgVectorIndex::new(backend).map_err(|e| {
                WikiError::Frontmatter(format!("semantic-pgvector construction: {e}"))
            })?);
            builder = builder.add_index(sem_idx);
        }
        let service = builder
            .build()
            .map_err(|e| WikiError::Frontmatter(format!("knowledge service build: {e}")))?;

        let ingest_pipeline = Arc::new(IngestPipeline::new(service.clone()));
        let default_markdown_extractor: Arc<dyn Extractor> =
            Arc::new(InternalMarkdownExtractor::new());

        Ok(Self {
            service,
            web_search,
            max_search_results: max_search_results.max(1),
            normalize_on_write: semantic.is_some(),
            ingest_pipeline,
            default_markdown_extractor,
        })
    }

    /// Expose the knowledge service — used by `maintenance::run_reindex`
    /// to trigger `reindex_all` through the plug architecture.
    pub fn service(&self) -> &Arc<KnowledgeService> {
        &self.service
    }
}

#[async_trait]
impl GadgetProvider for KnowledgeGadgetProvider {
    fn category(&self) -> &'static str {
        "knowledge"
    }

    fn gadget_schemas(&self) -> Vec<GadgetSchema> {
        let mut out = vec![
            schema_wiki_list(),
            schema_wiki_get(),
            schema_wiki_search(),
            schema_wiki_write(),
            schema_wiki_delete(),
            schema_wiki_rename(),
            schema_wiki_import(),
        ];
        if self.web_search.is_some() {
            out.push(schema_web_search());
        }
        out
    }

    async fn call(&self, name: &str, args: Value) -> Result<GadgetResult, GadgetError> {
        match name {
            "wiki.list" => self.call_wiki_list().await,
            "wiki.get" => self.call_wiki_get(args).await,
            "wiki.search" => self.call_wiki_search(args).await,
            "wiki.write" => self.call_wiki_write(args).await,
            "wiki.delete" => self.call_wiki_delete(args).await,
            "wiki.rename" => self.call_wiki_rename(args).await,
            "wiki.import" => self.call_wiki_import(args).await,
            "web.search" => self.call_web_search(args).await,
            other => Err(GadgetError::UnknownGadget(other.to_string())),
        }
    }
}

fn schema_wiki_list() -> GadgetSchema {
    GadgetSchema {
        name: "wiki.list".into(),
        tier: GadgetTier::Read,
        description: "List all pages in the Penny wiki. Returns page names \
            (use forward slashes for subdirectories). Call wiki.list first to \
            discover what pages exist before searching or fetching by name."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_wiki_get() -> GadgetSchema {
    GadgetSchema {
        name: "wiki.get".into(),
        tier: GadgetTier::Read,
        description: "Fetch a wiki page by its logical name. Use wiki.get \
            when you already know the exact page name (e.g. from a previous \
            wiki.list or wiki.search result). Page names use forward slashes \
            for subdirectories and do NOT include the .md suffix."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_wiki_search() -> GadgetSchema {
    GadgetSchema {
        name: "wiki.search".into(),
        tier: GadgetTier::Read,
        description: "Semantic + keyword search over the wiki. Returns pages \
            ranked by relevance with a short snippet."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 512 },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 10
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_wiki_write() -> GadgetSchema {
    GadgetSchema {
        name: "wiki.write".into(),
        tier: GadgetTier::Write,
        description: "Write or overwrite a wiki page. Content is markdown. \
            Auto-commits to git on success. Default size limit is 1 MiB. Path \
            must not contain '..' or absolute paths. Writes containing \
            unambiguous credentials (PEM keys, AWS access keys, GCP service \
            accounts) are rejected before touching disk."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 },
                "content": { "type": "string", "minLength": 0 }
            },
            "required": ["name", "content"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_wiki_delete() -> GadgetSchema {
    GadgetSchema {
        name: "wiki.delete".into(),
        tier: GadgetTier::Write,
        description: "Delete a wiki page. Soft delete by default: the page is \
            moved to `_archived/<YYYY-MM-DD>/<name>.md` with a git commit. \
            The operator can permanently remove archived pages later. Use \
            when the user asks to remove stale or wrong information."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_wiki_rename() -> GadgetSchema {
    GadgetSchema {
        name: "wiki.rename".into(),
        tier: GadgetTier::Write,
        description: "Rename or move a wiki page. Both `from` and `to` are \
            page names without the `.md` extension. Forward slashes are \
            treated as subdirectories. Fails with a conflict error if the \
            destination already exists. Use for reorganizing or correcting \
            page paths."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "minLength": 1, "maxLength": 256 },
                "to":   { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["from", "to"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_wiki_import() -> GadgetSchema {
    GadgetSchema {
        name: "wiki.import".into(),
        tier: GadgetTier::Write,
        description: "Import a RAW document (markdown / plain text) into the \
            wiki. Bytes are base64-encoded in `bytes`, `content_type` is the \
            MIME type (e.g. `text/markdown`). The pipeline extracts a title \
            (explicit `title_hint` > first heading > fallback), kebab-cases \
            it into `imports/<title>.md`, prepends source-tracking \
            frontmatter, and writes through the canonical store. \
            PDF / docx / pptx extractors land in a later release."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "bytes": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Base64-encoded RAW bytes (standard alphabet)."
                },
                "content_type": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 256,
                    "description": "MIME type of `bytes`. Currently `text/markdown` and `text/plain` are supported."
                },
                "target_path": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 256,
                    "description": "Optional wiki path override; when omitted the pipeline derives `imports/<kebab-title>`."
                },
                "title_hint": {
                    "type": "string",
                    "maxLength": 256,
                    "description": "Optional caller-supplied title; overrides the first-heading fallback."
                },
                "overwrite": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, allow overwriting an existing page at `target_path`."
                },
                "auto_enrich": {
                    "type": "boolean",
                    "default": false,
                    "description": "Caller may set `true` but W3-KL-2 treats this as a no-op; enrichment lands in a later release."
                },
                "source_uri": {
                    "type": "string",
                    "format": "uri",
                    "description": "Optional URL provenance; copied into the page's `source_uri` frontmatter."
                }
            },
            "required": ["bytes", "content_type"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_web_search() -> GadgetSchema {
    GadgetSchema {
        name: "web.search".into(),
        tier: GadgetTier::Read,
        description: "Search the web for information not in the wiki. Returns \
            up to 10 results (title, URL, snippet) via a self-hosted SearXNG \
            proxy. Use when the user's question cannot be answered from the \
            wiki alone."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 512 }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

// ---------------------------------------------------------------------------
// Gadget call impls — all routed through `KnowledgeService`
// ---------------------------------------------------------------------------

impl KnowledgeGadgetProvider {
    fn actor(&self) -> AuthenticatedContext {
        AuthenticatedContext
    }

    async fn call_wiki_list(&self) -> Result<GadgetResult, GadgetError> {
        let pages = self
            .service
            .list(&self.actor())
            .await
            .map_err(map_knowledge_err_generic)?;
        Ok(GadgetResult {
            content: json!({ "pages": pages }),
            is_error: false,
        })
    }

    async fn call_wiki_get(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let name = required_string_arg(&args, "name")?;
        match self.service.get(&self.actor(), &name).await {
            Ok(Some(doc)) => Ok(GadgetResult {
                content: json!({
                    "name": name,
                    "content": doc.markdown,
                }),
                is_error: false,
            }),
            Ok(None) => Err(GadgetError::Execution(format!("page {name:?} not found"))),
            Err(e) => Err(map_knowledge_err_read(e, &name)),
        }
    }

    async fn call_wiki_search(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let query = required_string_arg(&args, "query")?;
        let limit = parse_search_limit(&args);
        // `Auto` mode at the service level dispatches to every enabled
        // search plug — keyword-only when no semantic plug is registered,
        // hybrid when both are present. The previous in-provider
        // "semantic primary, fall back to keyword" heuristic is now the
        // service's fusion algorithm.
        let q = KnowledgeQuery {
            text: query.clone(),
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
            mode: KnowledgeQueryMode::Auto,
            include_relations: false,
        };
        let hits = self
            .service
            .search(&self.actor(), &q)
            .await
            .map_err(map_knowledge_err_generic)?;
        Ok(GadgetResult {
            content: json!({
                "query": query,
                "hits": hits.into_iter().map(search_hit_payload).collect::<Vec<_>>(),
            }),
            is_error: false,
        })
    }

    async fn call_wiki_write(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let name = required_string_arg(&args, "name")?;
        let raw_content = required_string_arg(&args, "content")?;
        let content = if self.normalize_on_write {
            normalize_page_content(&raw_content).map_err(map_wiki_err_write_legacy)?
        } else {
            raw_content
        };
        let bytes = content.len();

        match self
            .service
            .write(
                &self.actor(),
                KnowledgePutRequest {
                    path: name.clone(),
                    markdown: content,
                    create_only: false,
                    overwrite: false,
                    // Penny-authored wiki.write has no candidate provenance.
                    provenance: Default::default(),
                },
            )
            .await
        {
            Ok(receipt) => Ok(GadgetResult {
                content: json!({
                    "name": receipt.path,
                    "bytes": bytes,
                    // Historical surface compatibility: `commit_oid` was
                    // `Option<String>` under the old `Wiki` API. The
                    // `KnowledgeWriteReceipt::revision` is always present
                    // ("uncommitted" sentinel when autocommit = false).
                    "commit_oid": if receipt.revision == "uncommitted" {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(receipt.revision)
                    },
                }),
                is_error: false,
            }),
            Err(e) => Err(map_knowledge_err_write(e)),
        }
    }

    async fn call_wiki_delete(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let name = required_string_arg(&args, "name")?;
        match self.service.delete(&self.actor(), &name).await {
            Ok(_failures) => Ok(GadgetResult {
                content: json!({
                    "name": name,
                    // Archive path is an llm-wiki-specific detail the
                    // knowledge plane contract does not surface. Returning
                    // a stable operator message keeps the gadget output
                    // backward-compatible with the previous `Wiki::delete`
                    // return value for downstream callers that only
                    // rendered the message string.
                    "archived_to": format!("_archived/{}/{}", chrono::Utc::now().format("%Y-%m-%d"), name),
                    "message": "soft-deleted; archived copy preserved in _archived/",
                }),
                is_error: false,
            }),
            Err(e) => Err(map_knowledge_err_read(e, &name)),
        }
    }

    async fn call_wiki_rename(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let from = required_string_arg(&args, "from")?;
        let to = required_string_arg(&args, "to")?;
        match self.service.rename(&self.actor(), &from, &to).await {
            Ok(receipt) => Ok(GadgetResult {
                content: json!({
                    "from": from,
                    "to": receipt.path,
                    "commit_oid": if receipt.revision == "uncommitted" {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(receipt.revision)
                    },
                }),
                is_error: false,
            }),
            Err(e) => Err(map_knowledge_err_write(e)),
        }
    }

    /// Dispatch `wiki.import`. Only the built-in markdown extractor is
    /// wired here; content-type `application/pdf` is rejected at the
    /// gadget layer with an `InvalidArgs`.
    ///
    /// # W3-KL-3 deferred (D-20260418-13): PDF path via Bundle install
    ///
    /// Importing PDFs through `wiki.import` requires the
    /// `DocumentFormatsBundle` (with the `pdf` feature enabled) to
    /// register a `PdfExtractor` on the `extractors` axis of the bundle
    /// registry. That registry wiring is not yet part of the production
    /// startup path — it lands with W3-BUN-1 / W3-KL-4.
    ///
    /// Until then, PDF extraction is exercised in two places:
    ///
    /// 1. **Crate-level unit tests** — `plugins/plugin-document-formats/
    ///    src/pdf.rs` validates `PdfExtractor::extract` end-to-end
    ///    against a hand-crafted PDF fixture.
    /// 2. **Bundle-level integration test** — `crates/gadgetron-
    ///    knowledge/tests/rag_citation_e2e.rs::pdf_extractor_produces_
    ///    pipeline_ready_output` constructs a `PdfExtractor`
    ///    directly (simulating what `BundleRegistry::install_all` will
    ///    do) and confirms the `ExtractedDocument` shape matches what
    ///    `IngestPipeline::import` consumes.
    ///
    /// Once the Bundle install path is live, this method will look up
    /// an `Arc<dyn Extractor>` by content-type prefix from the registry
    /// instead of always reaching for `default_markdown_extractor`.
    async fn call_wiki_import(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let bytes_b64 = required_string_arg(&args, "bytes")?;
        let content_type = required_string_arg(&args, "content_type")?;
        let target_path = optional_string_arg(&args, "target_path");
        let title_hint = optional_string_arg(&args, "title_hint");
        let source_uri = optional_string_arg(&args, "source_uri");
        let overwrite = args
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let auto_enrich = args
            .get("auto_enrich")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Decode base64 in the gadget layer so the pipeline API stays on
        // raw `Vec<u8>`. Pass-through of decode errors as InvalidArgs —
        // per design 11 §9, malformed input is a caller concern.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(bytes_b64.as_bytes())
            .map_err(|e| GadgetError::InvalidArgs(format!("bytes must be valid base64: {e}")))?;

        // W3-KL-2: pick extractor by content_type prefix. Only the
        // internal markdown extractor is available on this path — PDF
        // and friends land when Bundle-driven extractors can register
        // on the `extractors` axis.
        let primary = content_type
            .split(';')
            .next()
            .unwrap_or(&content_type)
            .trim()
            .to_ascii_lowercase();
        let supported = self.default_markdown_extractor.supported_content_types();
        if !supported.iter().any(|t| t.eq_ignore_ascii_case(&primary)) {
            return Err(GadgetError::InvalidArgs(format!(
                "content_type {primary:?} is not supported by the built-in markdown extractor; \
                 supported: {supported:?}"
            )));
        }

        let request = ImportRequest {
            bytes: decoded,
            content_type: content_type.clone(),
            target_path,
            title_hint,
            auto_enrich,
            overwrite,
            source_uri,
        };

        match self
            .ingest_pipeline
            .import(
                &self.actor(),
                request,
                self.default_markdown_extractor.clone(),
            )
            .await
        {
            Ok(receipt) => Ok(GadgetResult {
                content: json!({
                    "path": receipt.path,
                    "canonical_plug": receipt.canonical_plug.as_str(),
                    "revision": receipt.revision,
                    "byte_size": receipt.byte_size,
                    "content_hash": receipt.content_hash,
                    "derived_failures": receipt
                        .derived_failures
                        .iter()
                        .map(|p| p.as_str().to_string())
                        .collect::<Vec<_>>(),
                }),
                is_error: false,
            }),
            Err(e) => Err(map_knowledge_err_write(e)),
        }
    }

    async fn call_web_search(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let Some(client) = &self.web_search else {
            return Err(GadgetError::Denied {
                reason: "web.search is not configured on this server".into(),
            });
        };
        let query = required_string_arg(&args, "query")?;
        match client.search(&query).await {
            Ok(results) => {
                let capped: Vec<_> = results.into_iter().take(self.max_search_results).collect();
                Ok(GadgetResult {
                    content: json!({
                        "query": query,
                        "results": capped,
                    }),
                    is_error: false,
                })
            }
            Err(e) => Err(map_search_err(e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Payload + error mapping (compatibility shims)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct SearchHitPayload {
    page_name: String,
    score: f32,
    section: Option<String>,
    snippet: Option<String>,
}

fn search_hit_payload(hit: KnowledgeHit) -> SearchHitPayload {
    SearchHitPayload {
        page_name: hit.path,
        score: hit.score,
        section: match hit.source_kind {
            // The legacy payload carried `section` only when it came from
            // the semantic backend's chunk heading. Keyword hits have
            // `None`. Relation hits (future) are tagged via source_kind
            // but the payload shape predates them — keep section None.
            KnowledgeHitKind::SearchIndex | KnowledgeHitKind::Canonical => hit.title.clone(),
            KnowledgeHitKind::RelationEdge => None,
        },
        snippet: Some(hit.snippet).filter(|s| !s.is_empty()),
    }
}

fn parse_search_limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(|v| v.as_u64())
        .or_else(|| args.get("max_results").and_then(|v| v.as_u64()))
        .map(|n| n as usize)
        .unwrap_or(10)
        .clamp(1, 50)
}

fn required_string_arg(args: &Value, field: &str) -> Result<String, GadgetError> {
    match args.get(field) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(GadgetError::InvalidArgs(format!(
            "field '{field}' must not be empty"
        ))),
        Some(_) => Err(GadgetError::InvalidArgs(format!(
            "field '{field}' must be a string"
        ))),
        None => Err(GadgetError::InvalidArgs(format!(
            "missing required field '{field}'"
        ))),
    }
}

/// `wiki.import` permits several optional string fields (title_hint,
/// target_path, source_uri). Returns `None` for missing-or-null-or-empty
/// so the pipeline's `Option<&str>` consumers don't need to branch on
/// empty strings.
fn optional_string_arg(args: &Value, field: &str) -> Option<String> {
    args.get(field).and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    })
}

// ---- GadgetronError -> GadgetError translation ----
//
// Historical wiki.* gadget error surface:
//   - path traversal / oversize / credential block -> `InvalidArgs` / `Denied`
//   - page-not-found -> `Execution("page {name:?} not found")`
//   - other -> `Execution("wiki storage error")`
//
// The tests assert that exact shape, so the adapter below preserves it when
// bridging from `GadgetronError::Knowledge` back to `GadgetError`.

fn map_knowledge_err_generic(err: GadgetronError) -> GadgetError {
    match err {
        GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::InvalidQuery { reason },
            ..
        } => {
            if reason.contains("invalid path") {
                GadgetError::InvalidArgs("invalid page path".into())
            } else {
                GadgetError::InvalidArgs(reason)
            }
        }
        _ => GadgetError::Execution("wiki operation failed".into()),
    }
}

fn map_knowledge_err_read(err: GadgetronError, name: &str) -> GadgetError {
    match err {
        GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::DocumentNotFound { .. },
            ..
        } => GadgetError::Execution(format!("page {name:?} not found")),
        other => map_knowledge_err_generic(other),
    }
}

fn map_knowledge_err_write(err: GadgetronError) -> GadgetError {
    match err {
        GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::InvalidQuery { reason },
            ..
        } => {
            // Preserve the shape of the historical variant mapping:
            //   path escape -> "invalid page path"
            //   too large   -> "page too large: ..."
            //   credential  -> `Denied { reason: "credential pattern ..." }`
            //   conflict    -> Execution("wiki git conflict ...")
            if reason.contains("invalid path") {
                GadgetError::InvalidArgs("invalid page path".into())
            } else if reason.contains("exceeds") {
                // `size {bytes} bytes exceeds {limit}-byte limit` — normalize
                // the phrasing to match the legacy test assertion
                // `page too large: {bytes} bytes exceeds ...`.
                let msg = normalize_page_too_large_reason(&reason);
                GadgetError::InvalidArgs(msg)
            } else if reason.contains("credential") {
                GadgetError::Denied { reason }
            } else {
                GadgetError::InvalidArgs(reason)
            }
        }
        GadgetronError::Knowledge {
            kind: KnowledgeErrorKind::BackendUnavailable { .. },
            message,
        } if message.contains("git conflict") => {
            GadgetError::Execution("wiki git conflict — resolve manually and retry".into())
        }
        _ => GadgetError::Execution("wiki storage error".into()),
    }
}

/// Reshape `"page \"foo\" size 100 bytes exceeds 10-byte limit"` → the
/// legacy `"page too large: 100 bytes exceeds the 10-byte limit"` string.
fn normalize_page_too_large_reason(reason: &str) -> String {
    // Best-effort: if the reason starts with the new prefix, swap to the
    // legacy phrasing. Otherwise return the reason unchanged so the test
    // still sees "bytes exceeds ... limit".
    if let Some(rest) = reason.strip_prefix("page ") {
        // rest ~= "\"foo\" size 100 bytes exceeds 10-byte limit"
        // Drop everything up to and including "size " to get the numbers.
        if let Some(after_size) = rest.find("size ") {
            let nums = &rest[after_size + 5..]; // "100 bytes exceeds 10-byte limit"
            return format!("page too large: {nums}");
        }
    }
    format!("page too large: {reason}")
}

fn map_wiki_err_write_legacy(err: WikiError) -> GadgetError {
    // Only reachable from the `normalize_page_content` pre-write path,
    // which never produces a `WikiErrorKind`. Keep the generic execution
    // error to preserve parity with pre-W3 behaviour.
    match err {
        WikiError::Kind {
            kind: WikiErrorKind::PathEscape { .. },
            ..
        } => GadgetError::InvalidArgs("invalid page path".into()),
        _ => GadgetError::Execution("wiki storage error".into()),
    }
}

fn map_search_err(err: SearchError) -> GadgetError {
    match err {
        SearchError::Http(_) => GadgetError::Execution("web search upstream HTTP error".into()),
        SearchError::Parse(_) => GadgetError::Execution("web search response parse failed".into()),
        SearchError::Upstream(_) => GadgetError::Execution("web search upstream error".into()),
    }
}

// ---------------------------------------------------------------------------
// Internal markdown extractor — default ingest path for W3-KL-2.
//
// Mirrors `gadgetron-bundle-document-formats::MarkdownExtractor` shape but
// lives inline in the knowledge crate so `wiki.import` works without a
// Bundle installation. When W3-KL-3 wires `DocumentFormatsBundle` through
// a real `BundleRegistry`, `KnowledgeGadgetProvider::from_service_with_extractor`
// (future) will replace this default with the registered extractor.
// ---------------------------------------------------------------------------

/// In-crate markdown extractor. Kept minimal — only the hooks the pipeline
/// needs: UTF-8 validation + `StructureHint::Heading` emission for ATX
/// headings. A copy of `plugin-document-formats::markdown::MarkdownExtractor`
/// would be cleaner but the plan (D-20260418-11) explicitly sets up
/// `default-markdown-extractor` as in-crate so W3-KL-2 is independent of
/// the bundle crate's install flow.
#[derive(Debug, Default, Clone)]
struct InternalMarkdownExtractor;

impl InternalMarkdownExtractor {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Extractor for InternalMarkdownExtractor {
    fn name(&self) -> &str {
        "markdown-internal"
    }

    fn supported_content_types(&self) -> &[&str] {
        &["text/markdown", "text/plain"]
    }

    async fn extract(
        &self,
        bytes: &[u8],
        _content_type: &str,
        _hints: &ExtractHints,
    ) -> Result<ExtractedDocument, ExtractError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| ExtractError::Malformed(format!("non-utf8 input: {e}")))?;
        let structure = detect_internal_headings(text);
        Ok(ExtractedDocument {
            plain_text: text.to_string(),
            structure,
            source_metadata: serde_json::json!({
                "extractor": "markdown-internal",
            }),
            warnings: Vec::new(),
        })
    }
}

/// ATX heading detection, fence-aware. See
/// `plugin-document-formats::markdown::detect_headings` for the full
/// rationale — this is a trimmed-down copy to keep the gadget crate
/// independent of the bundle crate at link time.
fn detect_internal_headings(text: &str) -> Vec<StructureHint> {
    let mut hints = Vec::new();
    let mut offset = 0usize;
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            offset += line.len();
            continue;
        }
        if in_fence {
            offset += line.len();
            continue;
        }
        let line_start = offset + (line.len() - trimmed.len());
        let mut level = 0u8;
        for b in trimmed.bytes() {
            if b == b'#' {
                level += 1;
                if level > 6 {
                    break;
                }
            } else {
                break;
            }
        }
        if (1..=6).contains(&level) {
            let after = &trimmed[level as usize..];
            let is_heading = after.starts_with(' ') || after.trim().is_empty();
            if is_heading {
                let t = after
                    .trim_start_matches(' ')
                    .trim_end_matches(['\n', '\r', '#', ' '])
                    .trim_end()
                    .to_string();
                hints.push(StructureHint::Heading {
                    level,
                    byte_offset: line_start,
                    text: t,
                });
            }
        }
        offset += line.len();
    }
    hints
}

// ---------------------------------------------------------------------------
// Tests — preserved from the legacy implementation plus new delegation
// assertions.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::hash::{Hash, Hasher};

    use super::*;
    use crate::embedding::{EmbeddingError, EmbeddingProvider};
    use crate::semantic::SemanticBackend;
    use async_trait::async_trait;
    use gadgetron_core::agent::tools::GadgetError;
    use gadgetron_testing::harness::pg::PgHarness;
    use tempfile::TempDir;

    use crate::config::{EmbeddingWriteMode, WikiConfig};

    fn fresh_provider_no_search() -> (TempDir, KnowledgeGadgetProvider) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "t@test.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let provider = KnowledgeGadgetProvider::with_components(wiki, None, 10);
        (dir, provider)
    }

    #[derive(Clone)]
    struct FakeEmbeddingProvider {
        dimension: usize,
    }

    impl FakeEmbeddingProvider {
        fn new(dimension: usize) -> Self {
            Self { dimension }
        }

        fn embed_one(&self, text: &str) -> Vec<f32> {
            let mut out = vec![0.0; self.dimension];
            for token in crate::wiki::tokenize(text) {
                let idx = stable_hash(&token) % self.dimension;
                out[idx] += 1.0;
            }
            out
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FakeEmbeddingProvider {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            Ok(texts.iter().map(|text| self.embed_one(text)).collect())
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        fn model_name(&self) -> &str {
            "fake-test-embedding"
        }
    }

    fn stable_hash(text: &str) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish() as usize
    }

    async fn semantic_pg_available() -> bool {
        let admin_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            return false;
        };

        let available: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
            "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
        )
        .fetch_optional(&pool)
        .await;
        pool.close().await;
        matches!(available, Ok(Some(_)))
    }

    #[test]
    fn category_is_knowledge() {
        let (_dir, p) = fresh_provider_no_search();
        assert_eq!(p.category(), "knowledge");
    }

    #[test]
    fn gadget_schemas_no_search_has_seven_tools() {
        // W3-KL-2 adds `wiki.import` to the surface; the six legacy
        // wiki.* gadgets remain. `web.search` still only appears when
        // the provider is built with a SearxngClient.
        let (_dir, p) = fresh_provider_no_search();
        let schemas = p.gadget_schemas();
        assert_eq!(schemas.len(), 7);
        let names: Vec<_> = schemas.iter().map(|s| s.name.as_str()).collect();
        for expected in [
            "wiki.list",
            "wiki.get",
            "wiki.search",
            "wiki.write",
            "wiki.delete",
            "wiki.rename",
            "wiki.import",
        ] {
            assert!(
                names.contains(&expected),
                "missing {expected:?} in {names:?}"
            );
        }
        assert!(!names.contains(&"web.search"));
    }

    #[test]
    fn wiki_search_schema_uses_limit_field() {
        let (_dir, p) = fresh_provider_no_search();
        let schema = p
            .gadget_schemas()
            .into_iter()
            .find(|s| s.name == "wiki.search")
            .expect("wiki.search schema");
        assert!(schema.input_schema["properties"].get("limit").is_some());
        assert!(schema.input_schema["properties"]
            .get("max_results")
            .is_none());
    }

    #[test]
    fn wiki_list_tier_is_read_wiki_write_tier_is_write() {
        let (_dir, p) = fresh_provider_no_search();
        let schemas = p.gadget_schemas();
        let list = schemas.iter().find(|s| s.name == "wiki.list").unwrap();
        let write = schemas.iter().find(|s| s.name == "wiki.write").unwrap();
        assert!(matches!(list.tier, GadgetTier::Read));
        assert!(matches!(write.tier, GadgetTier::Write));
    }

    #[tokio::test]
    async fn wiki_write_then_get_round_trips_content_without_semantic_mode() {
        let (_dir, p) = fresh_provider_no_search();
        let write_result = p
            .call(
                "wiki.write",
                json!({"name": "home", "content": "# Home\n\nbody"}),
            )
            .await
            .unwrap();
        assert!(!write_result.is_error);
        assert_eq!(write_result.content["bytes"], 12);

        let get_result = p.call("wiki.get", json!({"name": "home"})).await.unwrap();
        assert_eq!(get_result.content["content"], "# Home\n\nbody");
    }

    #[tokio::test]
    async fn wiki_list_reflects_writes() {
        let (_dir, p) = fresh_provider_no_search();
        p.call("wiki.write", json!({"name": "a", "content": "x"}))
            .await
            .unwrap();
        p.call("wiki.write", json!({"name": "b", "content": "y"}))
            .await
            .unwrap();
        let result = p.call("wiki.list", json!({})).await.unwrap();
        let pages = result.content["pages"].as_array().unwrap();
        let names: Vec<&str> = pages.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"a"), "expected 'a' in {names:?}");
        assert!(names.contains(&"b"), "expected 'b' in {names:?}");
        assert!(pages.len() >= 2);
    }

    #[tokio::test]
    async fn wiki_search_finds_by_keyword() {
        let (_dir, p) = fresh_provider_no_search();
        p.call(
            "wiki.write",
            json!({"name": "notes", "content": "quarterly review tomorrow"}),
        )
        .await
        .unwrap();
        p.call(
            "wiki.write",
            json!({"name": "grocery", "content": "milk bread"}),
        )
        .await
        .unwrap();
        let result = p
            .call("wiki.search", json!({"query": "quarterly", "limit": 10}))
            .await
            .unwrap();
        let hits = result.content["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["page_name"], "notes");
    }

    #[tokio::test]
    async fn wiki_write_pem_block_returns_denied() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call(
                "wiki.write",
                json!({
                    "name": "leaked",
                    "content": "-----BEGIN RSA PRIVATE KEY-----\nbody\n-----END RSA PRIVATE KEY-----"
                }),
            )
            .await
            .expect_err("should be blocked");
        match err {
            GadgetError::Denied { reason } => {
                assert!(reason.contains("pem_private_key"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wiki_write_path_traversal_returns_invalid_args() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.write", json!({"name": "../escape", "content": "x"}))
            .await
            .expect_err("path escape");
        assert!(matches!(err, GadgetError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn wiki_write_oversize_returns_invalid_args_with_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "T".into(),
            git_author_email: "t@t.local".into(),
            max_page_bytes: 10,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let p = KnowledgeGadgetProvider::with_components(wiki, None, 10);
        let err = p
            .call(
                "wiki.write",
                json!({"name": "big", "content": "x".repeat(100)}),
            )
            .await
            .expect_err("too large");
        match err {
            GadgetError::InvalidArgs(msg) => {
                assert!(msg.contains("page too large"));
                assert!(msg.contains("100"));
                assert!(msg.contains("10"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wiki_get_missing_field_returns_invalid_args() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.get", json!({}))
            .await
            .expect_err("missing name");
        match err {
            GadgetError::InvalidArgs(msg) => assert!(msg.contains("'name'")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wiki_get_wrong_type_returns_invalid_args() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.get", json!({"name": 42}))
            .await
            .expect_err("wrong type");
        assert!(matches!(err, GadgetError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn wiki_get_missing_file_returns_execution_not_found() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.get", json!({"name": "ghost"}))
            .await
            .expect_err("not found");
        match err {
            GadgetError::Execution(msg) => assert!(msg.contains("not found")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_tool_returns_unknown_tool_error() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("wiki.nonexistent", json!({}))
            .await
            .expect_err("unknown");
        match err {
            GadgetError::UnknownGadget(name) => assert_eq!(name, "wiki.nonexistent"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn web_search_without_config_returns_denied() {
        let (_dir, p) = fresh_provider_no_search();
        let err = p
            .call("web.search", json!({"query": "rust"}))
            .await
            .expect_err("denied");
        match err {
            GadgetError::Denied { reason } => assert!(reason.contains("not configured")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// W3-KL-1 delegation regression test.
    ///
    /// Verifies that `wiki.*` gadget calls route through `KnowledgeService`
    /// rather than touching `Wiki` directly. We inspect the service's
    /// plug registry to confirm `llm-wiki` canonical + `wiki-keyword`
    /// index are wired.
    #[test]
    fn knowledge_gadget_provider_delegates_to_service() {
        let (_dir, p) = fresh_provider_no_search();
        let svc = p.service();
        assert_eq!(svc.canonical_plug().as_str(), "llm-wiki");
        let indexes: Vec<&str> = svc.index_plugs().map(|p| p.as_str()).collect();
        assert!(
            indexes.contains(&"wiki-keyword"),
            "service must carry wiki-keyword index, got {indexes:?}"
        );
    }

    #[tokio::test]
    async fn semantic_write_sync_indexes_into_postgres_and_searches_by_page_name() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let dir = tempfile::tempdir().unwrap();
        let cfg = WikiConfig {
            root: dir.path().join("wiki"),
            autocommit: true,
            git_author_name: "Test".into(),
            git_author_email: "test@example.local".into(),
            max_page_bytes: 1024 * 1024,
        };
        let wiki = Arc::new(Wiki::open(cfg).unwrap());
        let semantic = Arc::new(SemanticBackend::new(
            harness.pool.clone(),
            Arc::new(FakeEmbeddingProvider::new(1536)) as Arc<dyn EmbeddingProvider>,
            EmbeddingWriteMode::Sync,
        ));
        let provider =
            KnowledgeGadgetProvider::with_components_and_semantic(wiki, None, Some(semantic), 10);

        provider
            .call(
                "wiki.write",
                json!({
                    "name": "incidents/fan-boot",
                    "content": "# Fan Boot\n\n## Symptom\n\nGPU fan error during boot.\n\n## Fix\n\nReseat the power cable."
                }),
            )
            .await
            .expect("write");

        let chunk_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM wiki_chunks WHERE page_name = $1")
                .bind("incidents/fan-boot")
                .fetch_one(&harness.pool)
                .await
                .expect("count chunks");
        assert!(chunk_count > 0, "expected indexed chunks");

        let result = provider
            .call("wiki.search", json!({"query": "fan boot gpu", "limit": 5}))
            .await
            .expect("search");
        let hits = result.content["hits"].as_array().expect("hits array");
        assert!(!hits.is_empty(), "semantic search should return a hit");
        assert_eq!(hits[0]["page_name"], "incidents/fan-boot");

        harness.cleanup().await;
    }

    #[tokio::test]
    async fn knowledge_provider_new_with_env_enables_semantic_when_pool_and_key_exist() {
        if !semantic_pg_available().await {
            eprintln!("skipping semantic pg test: vector extension is unavailable");
            return;
        }
        let harness = PgHarness::new().await;
        let dir = tempfile::tempdir().unwrap();
        let raw = format!(
            r#"
[knowledge]
wiki_path = "{}"
wiki_autocommit = true

[knowledge.embedding]
provider = "openai_compat"
base_url = "https://example.invalid/v1"
api_key_env = "OPENAI_API_KEY"
model = "text-embedding-3-small"
dimension = 1536
write_mode = "sync"
timeout_secs = 5
"#,
            dir.path().join("wiki").display()
        );
        let cfg = KnowledgeConfig::extract_from_toml_str(&raw)
            .expect("parse")
            .expect("knowledge cfg");
        let env = gadgetron_core::agent::config::FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let provider =
            KnowledgeGadgetProvider::new_with_env(cfg, Some(harness.pool.clone()), &env).unwrap();

        let schema = provider
            .gadget_schemas()
            .into_iter()
            .find(|s| s.name == "wiki.search")
            .expect("search schema");
        assert!(schema.input_schema["properties"].get("limit").is_some());
        // New surface check: semantic-pgvector index is registered.
        let indexes: Vec<&str> = provider
            .service()
            .index_plugs()
            .map(|p| p.as_str())
            .collect();
        assert!(
            indexes.contains(&"semantic-pgvector"),
            "semantic plug must be registered; got {indexes:?}"
        );
        harness.cleanup().await;
    }
}
