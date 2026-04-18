//! Caller-facing ingest option bundles.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §4.4 and §7.1.
//!
//! These structs are the ABI between a caller (web UI, CLI, Penny via
//! `wiki.import`) and `gadgetron-knowledge::ingest::IngestPipeline`.
//! They live in `gadgetron-core` so the knowledge layer and the future
//! `plugin-web-scrape` bundle (both of which synthesize import requests on
//! the caller's behalf) share one wire shape.

use serde::{Deserialize, Serialize};

/// Options a caller passes to the import path.
///
/// Field semantics:
///
/// - `target_path` — override the pipeline's auto-derived wiki path. When
///   `None`, the pipeline runs title-resolution fallback (see design 11
///   §4.4 step 2).
/// - `title_hint` — override the extractor-supplied title. Falls back to
///   the first heading hint, then the filename-derived default.
/// - `overwrite` — mirror of [`crate::knowledge::KnowledgePutRequest`] `overwrite`;
///   triggers supersession-chain logic when an existing page is at
///   `target_path`.
/// - `auto_enrich` — caller opt-in to LLM-backed tag/type/summary
///   enrichment. W3-KL-2 honors the field but always treats it as `false`
///   (enrichment plumbing lands in W3-KL-3).
/// - `source_uri` — URL provenance, copied into the frontmatter's
///   `source_uri`. Used only when the caller fetched via
///   `plugin-web-scrape::web.fetch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportOpts {
    #[serde(default)]
    pub target_path: Option<String>,
    #[serde(default)]
    pub title_hint: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub auto_enrich: bool,
    #[serde(default)]
    pub source_uri: Option<String>,
}

/// Options for the sibling `wiki.enrich` MCP tool (design 11 §7.2).
///
/// W3-KL-2 stub — enrichment orchestration lands in W3-KL-3 alongside the
/// Penny RAG system prompt. Kept here so the wire shape is shared from day
/// one; the tool provider can construct `EnrichOpts` without a
/// forward-declaration dance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnrichOpts {
    /// Re-enrich even if the page already carries `auto_enriched_by`
    /// frontmatter (design 11 §7.2 `force`).
    #[serde(default)]
    pub force: bool,
}
